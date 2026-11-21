//! The host's world: the loaded part of the map and the entities on it.
//!
//! [`boot_world`] keeps the world in step with the [`WorldState`] save: it loads the save's map,
//! spawns a PC for every player in the save who has none yet, and loads the sectors around each
//! PC. A new game and a loaded save boot the same way. A player is told the map's name
//! (`EnterMap`) before their PC's `Spawn`.
//!
//! Blocking: the sectors' tile blocking masks, and what their objects block
//! ([`ObjectBlocking`](crate::ObjectBlocking)), with fields the objects do not store taken from
//! their prototypes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

use arcanum_formats::art_id::ArtId;
use arcanum_formats::map::{MapProperties, SectorId, TileLoc};
use arcanum_formats::art_id::ArtIndex;
use arcanum_formats::object::{Object, Prototypes, TYPE_NPC};
use arcanum_formats::sector::Sector;
use arcanum_rules::{CharacterSheet, MALE};
use arcanum_vfs::Vfs;
use bevy_ecs::prelude::*;

use crate::{EntityKind, Gait, LedgerDelta, NetId, NetIds, Outbox, PcStatus, PlayerId, Pool, Session, SessionPlayer, SimRules, WorldState, pc_art};

/// Sectors loaded on each side of a PC's sector (1 = a 3 x 3 block).
pub const SECTOR_RADIUS: u32 = 1;
/// Sectors kept on each side of a PC's sector: one more than are loaded, so walking back and
/// forth over a sector border does not load and unload the same sectors.
pub const KEEP_RADIUS: u32 = 2;

/// The sectors within `radius` of `centre` (fewer at the map's corner).
pub fn sectors_near(centre: SectorId, radius: u32) -> impl Iterator<Item = SectorId> {
    let range = move |c: u32| c.saturating_sub(radius)..=c.saturating_add(radius);
    range(centre.y()).flat_map(move |y| range(centre.x()).map(move |x| SectorId::new(x, y)))
}

/// Whether sector `id` is within `radius` of `centre`.
pub fn sector_within(id: SectorId, centre: SectorId, radius: u32) -> bool {
    id.x().abs_diff(centre.x()) <= radius && id.y().abs_diff(centre.y()) <= radius
}

/// The game data the host reads maps from.
#[derive(Resource, Clone)]
pub struct SimVfs(pub Arc<RwLock<Vfs>>);

/// A tile position, for entities on the map.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position(pub TileLoc);

/// How a critter looks and how fast it walks: its art id and speed stat, as clients get them
/// in its `Spawn`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appearance {
    pub art: ArtId,
    pub speed: i32,
}

impl Appearance {
    pub fn spawn(self, net_id: NetId, kind: EntityKind, position: TileLoc) -> LedgerDelta {
        LedgerDelta::Spawn { net_id, kind, position, art: self.art.0, speed: self.speed }
    }
}

/// A non-player character from the map's `.mob` files, made from this prototype.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Npc {
    pub prototype: u32,
}

/// Sectors whose NPCs have been spawned.
#[derive(Resource, Debug, Default)]
pub struct PopulatedSectors(pub BTreeSet<SectorId>);

/// The walking speed NPCs are announced with until their stats are read (they do not move yet).
pub const NPC_SPEED: i32 = 8;

/// Spawns the NPCs of every loaded sector not populated yet, in sector and file order (so two
/// hosts booted from one save give them the same net ids), and announces each to every player
/// in the world. Their art is their own or their prototype's.
pub fn spawn_npcs(
    mut commands: Commands,
    map: Res<LoadedMap>,
    mut populated: ResMut<PopulatedSectors>,
    mut ids: ResMut<NetIds>,
    session: Res<Session>,
    mut outbox: ResMut<Outbox>,
    vfs: Option<Res<SimVfs>>,
) {
    let mut new: Vec<SectorId> = map.sectors.keys().filter(|id| !populated.0.contains(id)).copied().collect();
    if new.is_empty() {
        return;
    }
    new.sort_by_key(|id| (id.y(), id.x()));
    let players: Vec<PlayerId> = session.players.iter().filter(|(_, s)| matches!(s, SessionPlayer::InWorld(_))).map(|(&p, _)| p).collect();
    for id in new {
        populated.0.insert(id);
        for object in map.mobiles.get(&id).into_iter().flatten().filter(|o| o.object_type == TYPE_NPC) {
            let (Some(position), Some(art), Some(prototype)) = (crate::object_location(object), map.prototypes.art_id(object), object.prototype.prototype()) else { continue };
            let net_id = ids.0.allocate();
            let look = Appearance { art, speed: NPC_SPEED };
            let vfs_guard = vfs.as_ref().and_then(|v| v.0.read().ok());
            let gait = Gait::load(art, NPC_SPEED, |p| vfs_guard.as_deref().and_then(|v| v.read(p).ok()));
            let standpoints = crate::Standpoints::of(object, &map.prototypes, position);
            commands.spawn((net_id, Npc { prototype }, Position(position), look, gait, standpoints));
            let spawn = look.spawn(net_id, EntityKind::Npc { prototype }, position);
            outbox.0.extend(players.iter().map(|&p| (p, spawn.clone())));
        }
    }
}

/// Keeps the host's sectors in step with the PCs. It loads the sectors within [`SECTOR_RADIUS`]
/// of each PC and those of every walking PC's remaining path, and unloads sectors that no PC is
/// within [`KEEP_RADIUS`] of and no path crosses. A sector's NPCs go with it: every player is
/// told they are gone, and they spawn again when the sector loads again ([`spawn_npcs`]).
#[allow(clippy::too_many_arguments)]
pub fn stream_sectors(
    mut commands: Commands,
    vfs: Option<Res<SimVfs>>,
    mut map: ResMut<LoadedMap>,
    mut populated: ResMut<PopulatedSectors>,
    session: Res<Session>,
    mut outbox: ResMut<Outbox>,
    pcs: Query<(&Position, Option<&crate::Walking>), With<PlayerControlled>>,
    npcs: Query<(Entity, &NetId, &Position), With<Npc>>,
) {
    if map.name.is_none() {
        return;
    }
    let vfs_guard = vfs.as_ref().and_then(|v| v.0.read().ok());
    let vfs = vfs_guard.as_deref();
    let mut path_sectors = BTreeSet::new();
    for (&Position(at), walking) in &pcs {
        map.load_around(vfs, at);
        for t in walking.into_iter().flat_map(|w| w.steps.iter()) {
            path_sectors.insert(t.sector());
        }
    }
    for &id in &path_sectors {
        map.load_sector(vfs, id);
    }
    let centres: Vec<SectorId> = pcs.iter().map(|(p, _)| p.0.sector()).collect();
    let gone = map.unload_unless(|id| path_sectors.contains(&id) || centres.iter().any(|&c| sector_within(id, c, KEEP_RADIUS)));
    if gone.is_empty() {
        return;
    }
    let players: Vec<PlayerId> = session.players.iter().filter(|(_, s)| matches!(s, SessionPlayer::InWorld(_))).map(|(&p, _)| p).collect();
    for id in &gone {
        populated.0.remove(id);
    }
    let mut dropped: Vec<(NetId, Entity)> = npcs.iter().filter(|(_, _, p)| gone.contains(&p.0.sector())).map(|(e, &id, _)| (id, e)).collect();
    dropped.sort();
    for (net_id, entity) in dropped {
        commands.entity(entity).despawn();
        outbox.0.extend(players.iter().map(|&p| (p, LedgerDelta::Despawn { net_id })));
    }
}

/// The party's leader and the pace it sets (owner's rules): all players' PCs (and, later, their
/// companions) form one party, led by the host's PC. Out of combat every member moves at the
/// leader's speed, capped at [`crate::PARTY_MAX_SPEED`]; in combat each at its own (no combat
/// yet). This replaces the original's follower leash (0x4ad4d0), which moved a follower more
/// than 30 tiles from its leader next to it.
#[derive(Resource, Debug, Default, Clone)]
pub struct PartyPace {
    /// The player whose PC leads: the host's.
    pub leader: Option<PlayerId>,
    /// The leader's walk at the party's speed, once its PC is in the world.
    pub gait: Option<Gait>,
    /// The leader PC's net id, as players are told it.
    pub net_id: Option<NetId>,
    /// Players told who leads.
    pub told: BTreeSet<PlayerId>,
}

/// Works out the party's pace once the leader's PC is in the world, and tells every player in
/// the world which PC leads (`PartyLeader`).
pub fn update_party_pace(
    mut pace: ResMut<PartyPace>,
    vfs: Option<Res<SimVfs>>,
    session: Res<Session>,
    mut outbox: ResMut<Outbox>,
    pcs: Query<(&NetId, &PlayerControlled, &Appearance)>,
) {
    let Some(leader) = pace.leader else { return };
    let Some((&net_id, _, look)) = pcs.iter().find(|(_, p, _)| p.0 == leader) else { return };
    if pace.net_id != Some(net_id) {
        let vfs_guard = vfs.as_ref().and_then(|v| v.0.read().ok());
        let vfs = vfs_guard.as_deref();
        pace.gait = Some(Gait::load(look.art, crate::party_speed(look.speed), |p| vfs.and_then(|v| v.read(p).ok())));
        pace.net_id = Some(net_id);
        pace.told.clear();
    }
    let players: Vec<PlayerId> = session.players.iter().filter(|(p, s)| matches!(s, SessionPlayer::InWorld(_)) && !pace.told.contains(p)).map(|(&p, _)| p).collect();
    for player in players {
        outbox.0.push((player, LedgerDelta::PartyLeader { net_id }));
        pace.told.insert(player);
    }
}

/// The player who controls this PC.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerControlled(pub PlayerId);

/// A character's sheet.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct Sheet(pub CharacterSheet);

/// The part of the map the host has read.
#[derive(Resource, Debug, Default)]
pub struct LoadedMap {
    /// Folder name under `maps/`; `None` before a world exists.
    pub name: Option<String>,
    /// `map.prp`, if it could be read.
    pub properties: Option<MapProperties>,
    pub sectors: HashMap<SectorId, Sector>,
    /// Sectors looked for and not found (outside the map, or no file).
    pub absent: BTreeSet<SectorId>,
    /// Files that exist but could not be read, as `path: error`.
    pub errors: Vec<String>,
    /// The object prototypes (`proto/*.pro`), for fields the map's objects do not store.
    pub prototypes: Arc<Prototypes>,
    /// The art index, for the ground tiles that block (`tilename.mes` flags); `None` if its
    /// files could not be read (then no tile art blocks).
    pub art: Option<Arc<ArtIndex>>,
    /// What the loaded sectors' objects block.
    pub blocking: crate::ObjectBlocking,
    /// The map's mobile objects (`G_<guid>.mob` files: NPCs, containers, items on the ground) by
    /// sector, in file name order, read at [`LoadedMap::open`]. Objects carried or inside a
    /// container (flag `INVENTORY`), destroyed or off are left out.
    pub mobiles: HashMap<SectorId, Vec<Object>>,
    /// Changes to the map's objects since the original data (doors opened or closed), applied to
    /// each sector as it loads: the save's on the host, the host's on a client.
    pub changes: BTreeMap<SectorId, crate::SectorDelta>,
    /// Whether day locks hold now ([`crate::lock_day`]): doors locked by day or by night block
    /// accordingly. Set from the game clock ([`LoadedMap::set_lock_day`]).
    pub lock_day: bool,
}

impl LoadedMap {
    /// Starts over on `name`, reading its `map.prp`. The prototypes are read once and kept.
    pub fn open(&mut self, vfs: Option<&Vfs>, name: &str) {
        let prototypes = std::mem::take(&mut self.prototypes);
        let art = self.art.take();
        let lock_day = self.lock_day;
        *self = Self { name: Some(name.to_string()), prototypes, art, lock_day, ..Self::default() };
        let Some(vfs) = vfs else { return };
        if self.prototypes.is_empty() {
            let (prototypes, errors) = load_prototypes(vfs);
            self.prototypes = Arc::new(prototypes);
            self.errors.extend(errors);
        }
        if self.art.is_none() && vfs.exists("art/tile/tilename.mes") {
            match ArtIndex::load(|p| vfs.read(p).ok()) {
                Ok(index) => self.art = Some(Arc::new(index)),
                Err(e) => self.errors.push(format!("art index: {e:?}")),
            }
        }
        self.read_mobiles(vfs, name);
        let path = format!("maps/{name}/map.prp");
        if !vfs.exists(&path) {
            return;
        }
        match vfs.read(&path).map_err(|e| e.to_string()).and_then(|d| MapProperties::parse(&d).map_err(|e| e.to_string())) {
            Ok(p) => self.properties = Some(p),
            Err(e) => self.errors.push(format!("{path}: {e}")),
        }
    }

    /// Reads the map's `.mob` files into [`LoadedMap::mobiles`].
    fn read_mobiles(&mut self, vfs: &Vfs, name: &str) {
        let mut files: Vec<String> = vfs.list_dir(&format!("maps/{name}")).unwrap_or_default().into_iter().filter(|p| p.to_ascii_lowercase().ends_with(".mob")).collect();
        files.sort_by_key(|p| p.to_ascii_lowercase());
        for path in files {
            let object = match vfs.read(&path).map_err(|e| e.to_string()).and_then(|d| Object::parse_instance(&d).map_err(|e| e.to_string())) {
                Ok(o) => o,
                Err(e) => {
                    self.errors.push(format!("{path}: {e}"));
                    continue;
                }
            };
            let flags = self.prototypes.int(&object, "flags").unwrap_or(0);
            if flags & (crate::OF_INVENTORY | crate::OF_DESTROYED | crate::OF_OFF) != 0 {
                continue;
            }
            let Some(at) = crate::object_location(&object) else { continue };
            self.mobiles.entry(at.sector()).or_default().push(object);
        }
    }

    /// Loads the sectors within [`SECTOR_RADIUS`] of `at` that are not loaded or known absent.
    pub fn load_around(&mut self, vfs: Option<&Vfs>, at: TileLoc) {
        for id in sectors_near(at.sector(), SECTOR_RADIUS) {
            self.load_sector(vfs, id);
        }
    }

    /// Loads sector `id` unless it is loaded or known absent.
    pub fn load_sector(&mut self, vfs: Option<&Vfs>, id: SectorId) {
        let (Some(vfs), Some(name)) = (vfs, self.name.clone()) else { return };
        if self.sectors.contains_key(&id) || self.absent.contains(&id) {
            return;
        }
        let path = format!("maps/{name}/{}", id.file_name());
        if !vfs.exists(&path) {
            self.absent.insert(id);
            return;
        }
        match vfs.read(&path).map_err(|e| e.to_string()).and_then(|d| Sector::parse(&d).map_err(|e| e.to_string())) {
            Ok(sector) => {
                self.sectors.insert(id, sector);
                self.rebuild_blocking(id);
                self.apply_changes(id);
            }
            Err(e) => {
                self.errors.push(format!("{path}: {e}"));
                self.absent.insert(id);
            }
        }
    }

    /// Works out again what blocks in loaded sector `id`: its ground tiles' art
    /// ([`ArtIndex::tile_blocks`]), its objects and the map's mobile objects on it.
    pub fn rebuild_blocking(&mut self, id: SectorId) {
        self.blocking.remove_sector(id);
        let Some(sector) = self.sectors.get(&id) else { return };
        if let Some(art) = &self.art {
            let (x0, y0) = (id.x() * 64, id.y() * 64);
            for y in 0..64 {
                for x in 0..64 {
                    if art.tile_blocks(sector.tile(x, y).art_id()) {
                        self.blocking.tiles.insert(TileLoc { x: x0 + x, y: y0 + y });
                    }
                }
            }
        }
        self.blocking.add(&sector.objects, &self.prototypes, self.lock_day);
        self.blocking.add(self.mobiles.get(&id).into_iter().flatten(), &self.prototypes, self.lock_day);
    }

    /// Sets whether day locks hold, working out the loaded sectors' blocking again if it changed.
    pub fn set_lock_day(&mut self, day: bool) {
        if self.lock_day == day {
            return;
        }
        self.lock_day = day;
        let loaded: Vec<SectorId> = self.sectors.keys().copied().collect();
        for id in loaded {
            self.rebuild_blocking(id);
        }
    }

    /// Unloads every loaded sector `keep` refuses, with what its objects block. Returns them.
    pub fn unload_unless(&mut self, keep: impl Fn(SectorId) -> bool) -> Vec<SectorId> {
        let gone: Vec<SectorId> = self.sectors.keys().filter(|&&id| !keep(id)).copied().collect();
        for id in &gone {
            self.sectors.remove(id);
            self.blocking.remove_sector(*id);
        }
        gone
    }

    /// The tile blocking mask at `at`: `None` when its sector is not loaded or has no mask. The
    /// mask alone does not decide walkability (see the module docs).
    pub fn tile_mask_blocked(&self, at: TileLoc) -> Option<bool> {
        let (x, y) = at.tile_in_sector();
        self.sectors.get(&at.sector())?.tile_mask_blocked(x, y)
    }
}

/// Folder of the object prototypes (the game's loose `data/proto`, mounted at the VFS root).
pub const PROTO_DIR: &str = "proto";

/// Reads every prototype in [`PROTO_DIR`]. Returns them with the files that could not be read,
/// as `path: error`.
pub fn load_prototypes(vfs: &Vfs) -> (Prototypes, Vec<String>) {
    let mut prototypes = Prototypes::default();
    let mut errors = Vec::new();
    for path in vfs.list_dir(PROTO_DIR).unwrap_or_default().into_iter().filter(|p| p.to_ascii_lowercase().ends_with(".pro")) {
        match vfs.read(&path).map_err(|e| e.to_string()).and_then(|d| Object::parse_prototype(&d).map_err(|e| e.to_string())) {
            Ok(p) => {
                if !prototypes.insert(p) {
                    errors.push(format!("{path}: no prototype number"));
                }
            }
            Err(e) => errors.push(format!("{path}: {e}")),
        }
    }
    (prototypes, errors)
}

/// What the in-game interface shows of a PC with `sheet`: its name, portrait and full pools
/// (nothing takes damage or fatigue yet).
pub fn pc_status(rules: &arcanum_rules::RulesData, sheet: &CharacterSheet) -> PcStatus {
    let derived = rules.derived(sheet);
    PcStatus {
        name: sheet.name.clone(),
        portrait: rules.portrait(sheet).map(str::to_string),
        hit_points: Pool::full(derived.hit_points),
        fatigue: Pool::full(derived.fatigue),
    }
}

/// Boots and grows the world from the [`WorldState`]: see the module docs. Each new PC is
/// announced with a `Spawn` to every player in the session, and the new PC's player is also
/// told about the PCs already there; the state sync at the end of the tick says where they
/// are walking.
#[allow(clippy::too_many_arguments)]
pub fn boot_world(
    mut commands: Commands,
    mut world: ResMut<WorldState>,
    vfs: Option<Res<SimVfs>>,
    mut map: ResMut<LoadedMap>,
    mut ids: ResMut<NetIds>,
    mut session: ResMut<Session>,
    mut outbox: ResMut<Outbox>,
    rules: Res<SimRules>,
    mut clock: ResMut<crate::SyncClock>,
    mut clock_time: ResMut<crate::GameClock>,
    pcs: Query<(&NetId, &PlayerControlled, &Position, &Appearance, &Sheet)>,
    npcs: Query<(&NetId, &Npc, &Position, &Appearance)>,
) {
    let Some(save) = &world.0 else { return };
    let mut kits = Vec::new();
    let vfs_guard = vfs.as_ref().and_then(|v| v.0.read().ok());
    let vfs = vfs_guard.as_deref();
    if map.name.as_deref() != Some(save.map.as_str()) {
        map.open(vfs, &save.map);
        map.changes = save.sector_changes.clone();
        // The clock goes on from the save (a new game's start time).
        *clock_time = crate::GameClock::at(save.time);
        map.lock_day = crate::lock_day(save.time.hour());
    }
    let mut existing: Vec<(NetId, PlayerId, TileLoc, Appearance, PcStatus)> =
        pcs.iter().map(|(id, p, pos, look, sheet)| (*id, p.0, pos.0, *look, pc_status(&rules.0, &sheet.0))).collect();
    for (&player, saved) in &save.players {
        if existing.iter().any(|(_, p, ..)| *p == player) {
            continue;
        }
        let net_id = ids.0.allocate();
        let sheet = &saved.blueprint.sheet;
        let look = Appearance { art: pc_art(sheet.race, sheet.gender == MALE), speed: rules.0.derived(sheet).speed };
        let gait = Gait::load(look.art, look.speed, |p| vfs.and_then(|v| v.read(p).ok()));
        let inventory = saved.inventory.clone().unwrap_or_else(|| {
            let info = crate::item_info(&map.prototypes, |id| art_size(vfs, map.art.as_deref(), id));
            let kit = crate::starting_kit(&saved.blueprint, info, rules.0.derived(sheet).carry_weight, rules.0.aptitude(sheet));
            kits.push((player, kit.clone()));
            kit
        });
        // Only its player sees what it carries.
        outbox.0.push((player, LedgerDelta::Inventory { net_id, ack: 0, inventory: inventory.clone() }));
        commands.spawn((net_id, PlayerControlled(player), Position(saved.position), Sheet(sheet.clone()), look, gait, inventory));
        map.load_around(vfs, saved.position);
        session.players.insert(player, SessionPlayer::InWorld(net_id));
        // Everyone already here learns of the new PC; the new PC's player learns of everyone.
        outbox.0.push((player, LedgerDelta::EnterMap { map: save.map.clone() }));
        if !save.sector_changes.is_empty() {
            outbox.0.push((player, LedgerDelta::MapChanges { changes: save.sector_changes.clone() }));
        }
        let spawn = look.spawn(net_id, EntityKind::Pc { player }, saved.position);
        let status = pc_status(&rules.0, sheet);
        // Its player gets the whole sheet, for the character screen.
        outbox.0.push((player, LedgerDelta::SheetUpdate { ack: 0, sheet: sheet.clone() }));
        for &other in session.players.keys() {
            outbox.0.push((other, spawn.clone()));
            outbox.0.push((other, LedgerDelta::PcStatus { net_id, status: status.clone() }));
        }
        for (id, other, position, other_look, other_status) in &existing {
            outbox.0.push((player, other_look.spawn(*id, EntityKind::Pc { player: *other }, *position)));
            outbox.0.push((player, LedgerDelta::PcStatus { net_id: *id, status: other_status.clone() }));
        }
        let mut npcs: Vec<_> = npcs.iter().collect();
        npcs.sort_by_key(|n| *n.0);
        for (&id, npc, &Position(position), look) in npcs {
            outbox.0.push((player, look.spawn(id, EntityKind::Npc { prototype: npc.prototype }, position)));
        }
        existing.push((net_id, player, saved.position, look, status));
        // The newcomer learns where everyone is walking from a sync at the end of this tick.
        clock.due = true;
    }
    // A starting kit is given once: the save keeps it from now on.
    if !kits.is_empty()
        && let Some(save) = world.0.as_mut()
    {
        for (player, kit) in kits {
            if let Some(saved) = save.players.get_mut(&player) {
                saved.inventory = Some(kit);
            }
        }
    }
}

/// The first frame's size of art `id`, read through `index`.
pub fn art_size(vfs: Option<&Vfs>, index: Option<&ArtIndex>, id: ArtId) -> Option<(u32, u32)> {
    let path = index?.path(id)?;
    let file = arcanum_formats::art::ArtFile::parse(&vfs?.read(&path).ok()?).ok()?;
    let frame = file.frames.first()?.first()?;
    Some((frame.width, frame.height))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use arcanum_formats::test_util::SectorBuilder;
    use arcanum_rules::{CharacterBlueprint, RulesData};

    use super::*;
    use crate::{PlayerIntent, PlayerSave, SheetRequest, SimApp, WorldSave};

    const MAP: &str = "Test";

    /// A map folder with `map.prp` and a sector at (1, 1) whose tile (3, 5) is mask-blocked.
    fn write_map(dir: &Path) {
        let folder = dir.join("maps").join(MAP);
        std::fs::create_dir_all(&folder).unwrap();
        let mut prp = Vec::new();
        for v in [7u32, 1] {
            prp.extend(v.to_le_bytes());
        }
        for v in [256u64, 256] {
            prp.extend(v.to_le_bytes());
        }
        std::fs::write(folder.join("map.prp"), prp).unwrap();
        let mut sector = SectorBuilder::new();
        let i = 5 * 64 + 3;
        sector.blocked[i / 32] |= 1 << (i % 32);
        std::fs::write(folder.join(SectorId::new(1, 1).file_name()), sector.build()).unwrap();
        // An NPC at (80, 80) and a chest at (82, 80), one in a pocket (not on the map), and an
        // NPC in a sector without a file.
        use arcanum_formats::object::Value;
        use arcanum_formats::test_util::ObjectBuilder;
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mob = |name: &str, b: ObjectBuilder| std::fs::write(folder.join(format!("G_{name}.mob")), b.build()).unwrap();
        mob("A", ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(80, 80)));
        mob("B", ObjectBuilder::new(arcanum_formats::object::TYPE_CONTAINER).field("current_aid", Value::Int32(0x7000_0000)).field("location", at(82, 80)));
        mob("C", ObjectBuilder::new(arcanum_formats::object::TYPE_FOOD).field("flags", Value::Int32(crate::OF_INVENTORY)).field("location", at(82, 80)));
        mob("D", ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(10, 10)));
    }

    fn vfs(dir: &Path) -> SimVfs {
        let mut vfs = Vfs::new();
        vfs.mount_dir(dir);
        SimVfs(Arc::new(RwLock::new(vfs)))
    }

    fn save(players: &[(u64, &str, TileLoc)]) -> WorldSave {
        let mut save = WorldSave { map: MAP.into(), ..WorldSave::new_game(&crate::StartMap { map: MAP.into(), position: TileLoc { x: 0, y: 0 }, clock: Default::default() }) };
        for &(id, name, position) in players {
            let sheet = CharacterSheet { name: name.into(), ..Default::default() };
            save.players.insert(PlayerId(id), PlayerSave { blueprint: CharacterBlueprint { sheet, gold: 0, items: vec![], prototype: None }, position, inventory: None });
        }
        save
    }

    fn booted(dir: &Path, save: WorldSave) -> SimApp {
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir));
        sim.boot(save);
        sim.update();
        sim.update();
        sim
    }

    fn pcs(sim: &mut SimApp) -> Vec<(NetId, PlayerId, TileLoc, String)> {
        let world = sim.app.world_mut();
        let mut out: Vec<_> =
            world.query::<(&NetId, &PlayerControlled, &Position, &Sheet)>().iter(world).map(|(n, p, pos, s)| (*n, p.0, pos.0, s.0.name.clone())).collect();
        out.sort_by_key(|pc| pc.1);
        out
    }

    #[test]
    fn a_save_boots_into_pcs_and_the_sectors_around_them() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let at = TileLoc { x: 64 + 10, y: 64 + 20 }; // In sector (1, 1).
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", at)]));
        assert_eq!(pcs(&mut sim), vec![(NetId(0), PlayerId(7), at, "Virgil".to_string())]);
        let map = sim.app.world().resource::<LoadedMap>();
        assert_eq!(map.name.as_deref(), Some(MAP));
        assert_eq!(map.properties.map(|p| (p.width, p.height)), Some((256, 256)));
        assert_eq!(map.sectors.keys().copied().collect::<Vec<_>>(), vec![SectorId::new(1, 1)]);
        assert_eq!(map.absent.len(), 8, "the other eight of the 3 x 3 have no file");
        assert!(map.errors.is_empty(), "{:?}", map.errors);
        assert_eq!(map.tile_mask_blocked(TileLoc { x: 64 + 3, y: 64 + 5 }), Some(true));
        assert_eq!(map.tile_mask_blocked(TileLoc { x: 64 + 4, y: 64 + 5 }), Some(false));
        assert_eq!(map.tile_mask_blocked(TileLoc { x: 300, y: 300 }), None, "not loaded");
        let session = sim.app.world().resource::<Session>();
        assert_eq!(session.players[&PlayerId(7)], SessionPlayer::InWorld(NetId(0)));
    }

    #[test]
    fn booting_twice_gives_the_same_world() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let s = save(&[(7, "Virgil", TileLoc { x: 70, y: 80 }), (9, "Lugard", TileLoc { x: 100, y: 90 })]);
        let mut a = booted(dir.path(), s.clone());
        let mut b = booted(dir.path(), s);
        assert_eq!(pcs(&mut a), pcs(&mut b));
        assert_eq!(pcs(&mut a).len(), 2);
    }

    #[test]
    fn a_pc_at_the_map_corner_loads_without_underflow() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(1, "A", TileLoc { x: 0, y: 0 })]));
        let map = sim.app.world().resource::<LoadedMap>();
        assert_eq!(map.absent.len() + map.sectors.len(), 4, "a 2 x 2 block at the corner");
        assert_eq!(map.sectors.len(), 1, "(1, 1) is within reach");
        assert_eq!(pcs(&mut sim).len(), 1);
    }

    #[test]
    fn a_pc_enters_with_its_starting_kit_once() {
        use arcanum_formats::object::{Object, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let item = |number: u32, kind: u32| {
            let mut o = Object::parse_instance(&ObjectBuilder::new(kind).field("item_weight", Value::Int32(10)).build()).unwrap();
            o.id = prototype_id(number);
            o
        };
        let mut s = save(&[(1, "A", TileLoc { x: 70, y: 70 }), (2, "B", TileLoc { x: 71, y: 70 })]);
        let blueprint = &mut s.players.get_mut(&PlayerId(1)).unwrap().blueprint;
        (blueprint.gold, blueprint.items, blueprint.prototype) = (385, vec![6029], Some(16094));
        let dagger = 6029 + crate::BASIC_TO_PROTOTYPE;
        blueprint.sheet.base[0] = 8;
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut prototypes = Prototypes::default();
        prototypes.insert(item(crate::GOLD, arcanum_formats::object::TYPE_GOLD));
        prototypes.insert(item(dagger, arcanum_formats::object::TYPE_WEAPON));
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        sim.boot(s);
        sim.update();
        sim.update();
        let deltas = sim.take_deltas();
        let sent: Vec<_> = deltas.iter().filter_map(|(p, d)| match d {
            LedgerDelta::Inventory { inventory, .. } => Some((p.0, inventory.clone())),
            _ => None,
        }).collect();
        assert_eq!(sent.len(), 2, "each player gets only their own: {sent:?}");
        let (_, kit) = sent.iter().find(|(p, _)| *p == 1).unwrap();
        assert_eq!(kit.items[0], crate::CarriedItem { prototype: crate::GOLD, location: 0, quantity: 385 });
        assert_eq!(kit.worn(crate::Slot::Weapon).map(|i| i.prototype), Some(dagger), "a pre-made wields its weapon");
        let saved = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(saved.players[&PlayerId(1)].inventory.as_ref(), Some(kit), "kept in the save");
        assert_eq!(saved.players[&PlayerId(2)].inventory, Some(crate::Inventory::default()));

        // Moving the dagger off the hand into the grid goes through the host; a move onto a
        // taken cell is refused, and both are answered.
        let weapon = crate::Slot::Weapon as i32;
        let request = |seq, from, to| (PlayerId(1), PlayerIntent::Inventory { seq, request: crate::InventoryRequest::Move { from, to } });
        sim.receive([request(1, weapon, 55), request(2, 55, 0)]);
        sim.update();
        let answers: Vec<_> = sim.take_deltas().into_iter().filter_map(|(p, d)| match d {
            LedgerDelta::Inventory { ack, inventory, .. } => Some((p.0, ack, inventory)),
            _ => None,
        }).collect();
        assert_eq!(answers.iter().map(|(p, ack, _)| (*p, *ack)).collect::<Vec<_>>(), [(1, 1), (1, 2)]);
        let at = |inv: &crate::Inventory| inv.items.iter().find(|i| i.prototype == dagger).unwrap().location;
        assert_eq!(at(&answers[1].2), 55, "the gold is at 0");
        let saved = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(at(saved.players[&PlayerId(1)].inventory.as_ref().unwrap()), 55, "kept in the save");
    }

    #[test]
    fn the_character_screen_spends_points_through_the_host() {
        use arcanum_rules::{CharacterAction, Pool};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut s = save(&[(1, "A", TileLoc { x: 70, y: 70 }), (2, "B", TileLoc { x: 71, y: 70 })]);
        s.players.get_mut(&PlayerId(1)).unwrap().blueprint.sheet.unspent = 2;
        let mut sim = booted(dir.path(), s);
        sim.take_deltas();
        let request = |seq, request| (PlayerId(1), PlayerIntent::Sheet { seq, request });
        sim.receive([
            request(1, SheetRequest::Open),
            request(2, SheetRequest::Action(CharacterAction::AdjustPool(Pool::HitPoints, 1))),
            request(3, SheetRequest::Action(CharacterAction::SetName("Z".into()))),
            request(4, SheetRequest::Close),
        ]);
        sim.update();
        let deltas = sim.take_deltas();
        let acks: Vec<u32> = deltas.iter().filter_map(|(p, d)| match d {
            LedgerDelta::SheetUpdate { ack, .. } if p.0 == 1 => Some(*ack),
            _ => None,
        }).collect();
        assert_eq!(acks, [1, 2, 3, 4], "every request answered, refused or not");
        assert!(!deltas.iter().any(|(p, d)| p.0 == 2 && matches!(d, LedgerDelta::SheetUpdate { .. })), "only to its player");
        // Both players see the higher maximum.
        for to in [1, 2] {
            let max = deltas.iter().rev().find_map(|(p, d)| match d {
                LedgerDelta::PcStatus { status, .. } if p.0 == to => Some(status.hit_points.max),
                _ => None,
            });
            assert!(max.is_some_and(|m| m > 0), "player {to}: {deltas:?}");
        }
        let saved = &sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(1)].blueprint.sheet;
        assert_eq!((saved.hp_points, saved.unspent, saved.name.as_str()), (1, 1, "A"), "kept in the save, the name refused");
    }

    #[test]
    fn spawns_reach_every_player() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(1, "A", TileLoc { x: 70, y: 70 }), (2, "B", TileLoc { x: 71, y: 70 })]));
        let deltas = sim.take_deltas();
        let spawns = |to: u64| deltas.iter().filter(|(p, d)| p.0 == to && matches!(d, LedgerDelta::Spawn { kind: EntityKind::Pc { .. }, .. })).count();
        assert_eq!((spawns(1), spawns(2)), (2, 2), "{deltas:?}");
        // Each player learns every PC's name and gauges.
        for to in [1, 2] {
            let names: BTreeSet<&str> = deltas
                .iter()
                .filter_map(|(p, d)| match d {
                    LedgerDelta::PcStatus { status, .. } if p.0 == to => {
                        assert!(status.hit_points.max > 0 && status.hit_points.current == status.hit_points.max, "{status:?}");
                        assert!(status.fatigue.max > 0 && status.fatigue.fraction() == 1.0, "{status:?}");
                        Some(status.name.as_str())
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(names, BTreeSet::from(["A", "B"]), "player {to}");
        }
        let enter = LedgerDelta::EnterMap { map: MAP.into() };
        for to in [1, 2] {
            let first = deltas.iter().position(|(p, d)| p.0 == to && *d == enter);
            let spawn = deltas.iter().position(|(p, d)| p.0 == to && matches!(d, LedgerDelta::Spawn { .. }));
            assert!(first < spawn && first.is_some(), "player {to} learns the map first: {deltas:?}");
        }
        // Then a sync says where everyone is.
        assert!(matches!(&deltas.last().unwrap().1, LedgerDelta::StateSync { actors, .. } if actors.len() == 2), "{deltas:?}");
        // No PC is spawned twice on later ticks.
        sim.update();
        assert!(!sim.take_deltas().iter().any(|(_, d)| matches!(d, LedgerDelta::Spawn { .. })));
        assert_eq!(pcs(&mut sim).len(), 2);
    }

    fn walk_ticks(sim: &mut SimApp, ticks: usize) {
        for _ in 0..ticks {
            sim.update();
        }
    }

    fn position(sim: &mut SimApp, player: u64) -> TileLoc {
        pcs(sim).into_iter().find(|pc| pc.1 == PlayerId(player)).unwrap().2
    }

    fn send_move(sim: &mut SimApp, player: u64, seq: u32, path: &[(u32, u32)]) {
        let path = path.iter().map(|&(x, y)| TileLoc { x, y }).collect();
        sim.receive([(PlayerId(player), crate::PlayerIntent::Move { seq, path })]);
    }

    /// Ticks for `tiles` steps at the slowest step of the gait these PCs get (a human's; the
    /// test data has no walk art), rounded up.
    fn ticks_for(tiles: usize) -> usize {
        let gait = crate::Gait::human(crate::walk_fps(8, 0));
        let slowest = gait.step_seconds(TileLoc { x: 1, y: 0 }, TileLoc { x: 0, y: 1 });
        (tiles as f64 * crate::TICK_HZ * slowest).ceil() as usize + 1
    }

    #[test]
    fn a_step_takes_as_long_as_the_gait_says() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", TileLoc { x: 70, y: 80 })]));
        sim.take_deltas();
        let (from, to) = (TileLoc { x: 70, y: 80 }, TileLoc { x: 69, y: 80 });
        let seconds = crate::Gait::human(crate::walk_fps(8, 0)).step_seconds(from, to);
        let ticks = (seconds * crate::TICK_HZ).ceil() as usize;
        send_move(&mut sim, 7, 1, &[(70, 80), (69, 80)]);
        walk_ticks(&mut sim, ticks - 1);
        assert_eq!(position(&mut sim, 7), from, "{ticks} ticks needed");
        walk_ticks(&mut sim, 1);
        assert_eq!(position(&mut sim, 7), to);
    }

    fn t(x: u32, y: u32) -> TileLoc {
        TileLoc { x, y }
    }

    /// PC 0's state in the last sync `player` got, if any.
    fn synced(deltas: &[(PlayerId, LedgerDelta)], player: u64) -> Option<crate::ActorState> {
        deltas.iter().rev().find_map(|(p, d)| match d {
            LedgerDelta::StateSync { actors, .. } if p.0 == player => actors.iter().find(|s| s.net_id == NetId(0)).cloned(),
            _ => None,
        })
    }

    fn state(position: TileLoc, path: &[(u32, u32)], seq: u32) -> crate::ActorState {
        crate::ActorState { net_id: NetId(0), position, path: path.iter().map(|&(x, y)| t(x, y)).collect(), seq }
    }

    #[test]
    fn a_valid_move_walks_the_pc_and_reaches_the_others() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", t(70, 80)), (9, "B", t(90, 90))]));
        sim.take_deltas();
        send_move(&mut sim, 7, 1, &[(70, 80), (71, 81), (72, 81)]);
        sim.update();
        let deltas = sim.take_deltas();
        let walking = state(t(70, 80), &[(71, 81), (72, 81)], 1);
        for to in [7, 9] {
            assert_eq!(synced(&deltas, to), Some(walking.clone()), "player {to} hears at once: {deltas:?}");
        }
        assert_eq!(position(&mut sim, 7), t(70, 80), "not a whole step yet");
        walk_ticks(&mut sim, ticks_for(2));
        assert_eq!(position(&mut sim, 7), t(72, 81));
        let world = sim.app.world_mut();
        assert_eq!(world.query::<&crate::Walking>().iter(world).count(), 0, "stopped at the end");
        assert_eq!(synced(&sim.take_deltas(), 9), Some(state(t(72, 81), &[], 1)), "the stop is synced");
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.players[&PlayerId(7)].position, t(72, 81), "the save has where it stopped");
    }

    #[test]
    fn syncs_go_often_while_walking_and_seldom_when_still() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", t(70, 80))]));
        sim.take_deltas();
        let count = |sim: &mut SimApp, ticks: u32| {
            let mut n = 0;
            for _ in 0..ticks {
                sim.update();
                n += sim.take_deltas().iter().filter(|(_, d)| matches!(d, LedgerDelta::StateSync { .. })).count();
            }
            n
        };
        assert_eq!(count(&mut sim, crate::SYNC_TICKS_IDLE), 1, "once a second when still");
        send_move(&mut sim, 7, 1, &row(80, 70, 76));
        assert_eq!(count(&mut sim, 1), 1, "at once for the move");
        assert_eq!(count(&mut sim, 3 * crate::SYNC_TICKS_MOVING), 3, "ten a second while walking");
        let mut numbers = Vec::new();
        for _ in 0..crate::SYNC_TICKS_MOVING * 2 {
            sim.update();
            numbers.extend(sim.take_deltas().into_iter().filter_map(|(_, d)| match d {
                LedgerDelta::StateSync { number, .. } => Some(number),
                _ => None,
            }));
        }
        assert!(numbers.windows(2).all(|w| w[1] == w[0] + 1), "numbered in turn: {numbers:?}");
    }

    #[test]
    fn a_client_far_behind_gets_a_resync() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", t(70, 80)), (9, "B", t(90, 90))]));
        sim.take_deltas();
        send_move(&mut sim, 7, 1, &row(80, 70, 76));
        let mut resyncs = Vec::new();
        let mut last_number = 0;
        for _ in 0..crate::SYNC_TICKS_MOVING * (crate::RESYNC_BEHIND + 2) {
            sim.update();
            for (to, d) in sim.take_deltas() {
                match d {
                    // Player 7 acknowledges every sync; player 9 never does.
                    LedgerDelta::StateSync { number, .. } if to.0 == 7 => {
                        last_number = number;
                        sim.receive([(to, crate::PlayerIntent::SyncAck { number })]);
                    }
                    LedgerDelta::Resync { number, .. } => resyncs.push((to.0, number)),
                    _ => {}
                }
            }
        }
        assert_eq!(resyncs.len(), 1, "once, then it waits: {resyncs:?}");
        assert_eq!(resyncs[0].0, 9);
        assert!(last_number > crate::RESYNC_BEHIND);
    }

    #[test]
    fn every_player_gets_a_keyframe_regularly() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", t(70, 80)), (9, "B", t(90, 90))]));
        sim.take_deltas();
        let mut keyframes = Vec::new();
        for tick in 0..crate::KEYFRAME_TICKS * 2 {
            sim.update();
            for (to, d) in sim.take_deltas() {
                if let LedgerDelta::Resync { number, .. } = d {
                    sim.receive([(to, crate::PlayerIntent::SyncAck { number })]);
                    keyframes.push((tick, to.0));
                }
                if let LedgerDelta::StateSync { number, .. } = d {
                    sim.receive([(to, crate::PlayerIntent::SyncAck { number })]);
                }
            }
        }
        let ticks: Vec<u32> = keyframes.iter().filter(|k| k.1 == 7).map(|k| k.0).collect();
        assert_eq!(ticks.len(), 2, "{keyframes:?}");
        assert_eq!(ticks[1] - ticks[0], crate::KEYFRAME_TICKS);
        assert_eq!(keyframes.iter().filter(|k| k.1 == 9).count(), 2, "to everyone");
    }

    #[test]
    fn old_and_repeated_moves_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", t(70, 80))]));
        sim.take_deltas();
        send_move(&mut sim, 7, 2, &row(80, 70, 72));
        sim.update();
        assert_eq!(synced(&sim.take_deltas(), 7), Some(state(t(70, 80), &[(71, 80), (72, 80)], 2)));
        // Move 1, overtaken on the way, and a repeat of move 2 arrive late: nothing changes.
        send_move(&mut sim, 7, 1, &[(70, 80), (70, 81)]);
        send_move(&mut sim, 7, 2, &row(80, 70, 72));
        sim.update();
        assert!(synced(&sim.take_deltas(), 7).is_none(), "no sync: nothing changed");
        walk_ticks(&mut sim, ticks_for(2));
        assert_eq!(position(&mut sim, 7), t(72, 80));
    }

    #[test]
    fn a_blocked_move_is_corrected() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let at = t(66, 69);
        let mut sim = booted(dir.path(), save(&[(7, "A", at)]));
        sim.take_deltas();
        // (67, 69) is mask-blocked: the sync for move 4 has the PC standing where it was.
        send_move(&mut sim, 7, 4, &[(66, 69), (67, 69), (68, 69)]);
        sim.update();
        assert_eq!(synced(&sim.take_deltas(), 7), Some(state(at, &[], 4)));
        // Out of the loaded sectors: (1, 1) is the only sector with a file.
        send_move(&mut sim, 7, 5, &[(65, 69), (64, 69), (63, 69)]);
        sim.update();
        assert_eq!(synced(&sim.take_deltas(), 7), Some(state(at, &[], 5)));
        walk_ticks(&mut sim, ticks_for(3));
        assert_eq!(position(&mut sim, 7), at);
    }

    #[test]
    fn a_client_ahead_on_its_route_continues_from_there() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", t(70, 80)), (9, "B", t(90, 90))]));
        sim.take_deltas();
        send_move(&mut sim, 7, 1, &row(80, 71, 75));
        sim.update();
        walk_ticks(&mut sim, ticks_for(1));
        assert_eq!(position(&mut sim, 7), t(71, 80));
        sim.take_deltas();
        // The client (ahead: latency) turns off at (74, 80). The host walks there first.
        send_move(&mut sim, 7, 2, &[(74, 80), (74, 81)]);
        sim.update();
        assert_eq!(synced(&sim.take_deltas(), 9), Some(state(t(71, 80), &[(72, 80), (73, 80), (74, 80), (74, 81)], 2)));
        assert!(position(&mut sim, 7).x < 73, "at the host's pace, not the client's");
        walk_ticks(&mut sim, ticks_for(3));
        assert_eq!(position(&mut sim, 7), t(74, 81));
    }

    #[test]
    fn a_client_claiming_to_be_off_its_route_is_corrected_where_the_host_has_it() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", t(70, 80))]));
        sim.take_deltas();
        send_move(&mut sim, 7, 1, &row(80, 71, 75));
        sim.update();
        walk_ticks(&mut sim, ticks_for(1));
        assert_eq!(position(&mut sim, 7), t(71, 80));
        // The client claims to be at (74, 82), which is neither near the host nor on the route.
        send_move(&mut sim, 7, 2, &[(74, 82), (74, 83)]);
        sim.update();
        let stopped = synced(&sim.take_deltas(), 7).unwrap();
        assert_eq!((stopped.path.len(), stopped.seq), (0, 2), "{stopped:?}");
        assert!(stopped.position.x <= 72, "where the host had it, not where the client said: {stopped:?}");
        walk_ticks(&mut sim, ticks_for(4));
        assert_eq!(position(&mut sim, 7), stopped.position, "and it stopped there");
        // A path from one step ahead of the host is fine.
        let x = stopped.position.x;
        send_move(&mut sim, 7, 3, &[(x + 1, 80), (x + 2, 80)]);
        sim.update();
        walk_ticks(&mut sim, ticks_for(2));
        assert_eq!(position(&mut sim, 7), t(x + 2, 80));
    }

    #[test]
    fn moves_from_players_without_a_pc_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", TileLoc { x: 70, y: 80 })]));
        sim.take_deltas();
        send_move(&mut sim, 8, 1, &[(70, 80), (71, 80)]);
        sim.update();
        assert!(sim.take_deltas().is_empty());
    }

    /// The tiles from `(from, y)` to `(to, y)`, either way.
    fn row(y: u32, from: u32, to: u32) -> Vec<(u32, u32)> {
        let xs: Vec<u32> = if from <= to { (from..=to).collect() } else { (to..=from).rev().collect() };
        xs.into_iter().map(|x| (x, y)).collect()
    }

    #[test]
    fn out_of_combat_pcs_walk_through_each_other() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", TileLoc { x: 70, y: 80 }), (9, "B", TileLoc { x: 73, y: 80 })]));
        sim.take_deltas();
        send_move(&mut sim, 7, 1, &row(80, 70, 76));
        let mut shared = false;
        for _ in 0..ticks_for(6) {
            sim.update();
            shared |= position(&mut sim, 7) == position(&mut sim, 9);
        }
        assert!(shared, "A stood on B's tile on the way");
        assert_eq!(position(&mut sim, 7), TileLoc { x: 76, y: 80 });
        // And onto a taken goal.
        send_move(&mut sim, 7, 2, &row(80, 76, 73));
        walk_ticks(&mut sim, ticks_for(3));
        assert_eq!(position(&mut sim, 7), position(&mut sim, 9));
    }

    #[test]
    fn a_tile_blocked_mid_walk_stops_the_pc() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", TileLoc { x: 70, y: 80 }), (9, "B", TileLoc { x: 90, y: 90 })]));
        sim.take_deltas();
        send_move(&mut sim, 7, 1, &row(80, 70, 76));
        sim.update();
        // A door closes on (74, 80) after the path was accepted.
        let mut map = sim.app.world_mut().resource_mut::<LoadedMap>();
        let mask = map.sectors.get_mut(&SectorId::new(1, 1)).unwrap().blocked.as_mut().unwrap();
        let i = 16 * 64 + 10;
        mask[i / 32] |= 1 << (i % 32);
        walk_ticks(&mut sim, ticks_for(6));
        let stop = TileLoc { x: 73, y: 80 };
        assert_eq!(position(&mut sim, 7), stop);
        let deltas = sim.take_deltas();
        for to in [7, 9] {
            assert_eq!(synced(&deltas, to), Some(state(stop, &[], 1)), "player {to}: {deltas:?}");
        }
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.players[&PlayerId(7)].position, stop);
    }

    #[test]
    fn npcs_spawn_with_their_sector_and_reach_every_player() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", t(70, 80))]));
        let deltas = sim.take_deltas();
        let npc_spawns: Vec<_> = deltas.iter().filter(|(_, d)| matches!(d, LedgerDelta::Spawn { kind: EntityKind::Npc { .. }, .. })).collect();
        assert_eq!(npc_spawns.len(), 1, "one NPC on the loaded sector: {deltas:?}");
        let LedgerDelta::Spawn { net_id, position, art, .. } = &npc_spawns[0].1 else { unreachable!() };
        assert_eq!((*position, *art), (t(80, 80), 0x2800_2000));
        let map = sim.app.world().resource::<LoadedMap>();
        assert_eq!(map.mobiles.values().map(Vec::len).sum::<usize>(), 3, "the pocketed food is left out");
        assert!(map.blocking.tiles.contains(&t(82, 80)), "the chest blocks");
        assert!(!map.blocking.tiles.contains(&t(80, 80)), "NPCs do not block out of combat");
        // A player who joins later hears of it too.
        let npc = *net_id;
        sim.app.world_mut().resource_mut::<WorldState>().add_player(Some(&crate::StartMap { map: MAP.into(), position: t(72, 80), clock: Default::default() }), PlayerId(9), CharacterBlueprint { sheet: CharacterSheet { name: "B".into(), ..Default::default() }, gold: 0, items: vec![], prototype: None });
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(p, d)| p.0 == 9 && matches!(d, LedgerDelta::Spawn { net_id, .. } if *net_id == npc)), "{deltas:?}");
        assert_eq!(deltas.iter().filter(|(p, d)| p.0 == 7 && matches!(d, LedgerDelta::Spawn { kind: EntityKind::Npc { .. }, .. })).count(), 0, "not again");
    }

    /// A map 8 sectors wide with a file for each of the sectors (0..8, 1), an NPC in sector
    /// (1, 1) at (80, 80).
    fn write_wide_map(dir: &Path) {
        use arcanum_formats::object::Value;
        use arcanum_formats::test_util::ObjectBuilder;
        let folder = dir.join("maps").join(MAP);
        std::fs::create_dir_all(&folder).unwrap();
        let mut prp = Vec::new();
        for v in [7u32, 1] {
            prp.extend(v.to_le_bytes());
        }
        for v in [512u64, 256] {
            prp.extend(v.to_le_bytes());
        }
        std::fs::write(folder.join("map.prp"), prp).unwrap();
        for x in 0..8 {
            std::fs::write(folder.join(SectorId::new(x, 1).file_name()), SectorBuilder::new().build()).unwrap();
        }
        let at = Value::Int64(Some(80 | (80i64 << 32)));
        std::fs::write(folder.join("G_A.mob"), ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at).build()).unwrap();
    }

    fn loaded(sim: &SimApp) -> Vec<u32> {
        let mut xs: Vec<u32> = sim.app.world().resource::<LoadedMap>().sectors.keys().map(|id| id.x()).collect();
        xs.sort();
        xs
    }

    #[test]
    fn sectors_follow_the_pcs_and_npcs_go_and_come_back_with_theirs() {
        let dir = tempfile::tempdir().unwrap();
        write_wide_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", t(70, 80))]));
        assert_eq!(loaded(&sim), vec![0, 1, 2]);
        sim.take_deltas();
        // The PC is put three sectors on: the far side loads, sectors 0 and 1 go, and with
        // sector 1 its NPC.
        let set_x = |sim: &mut SimApp, x: u32| {
            let world = sim.app.world_mut();
            let mut q = world.query_filtered::<&mut Position, bevy_ecs::query::With<PlayerControlled>>();
            q.single_mut(world).unwrap().0 = t(x, 80);
        };
        set_x(&mut sim, 4 * 64 + 10);
        sim.update();
        assert_eq!(loaded(&sim), vec![2, 3, 4, 5], "kept within two, loaded within one");
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(p, d)| p.0 == 7 && matches!(d, LedgerDelta::Despawn { .. })), "{deltas:?}");
        let world = sim.app.world_mut();
        assert_eq!(world.query::<&Npc>().iter(world).count(), 0);
        // Back again: the NPC is spawned again.
        set_x(&mut sim, 70);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::Spawn { kind: EntityKind::Npc { .. }, position, .. } if *position == t(80, 80))), "{deltas:?}");
        assert_eq!(loaded(&sim), vec![0, 1, 2, 3], "3 is still within two");
    }

    #[test]
    fn a_path_into_sectors_not_loaded_yet_is_checked_against_them() {
        let dir = tempfile::tempdir().unwrap();
        write_wide_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", t(120, 80))]));
        assert_eq!(loaded(&sim), vec![0, 1, 2]);
        sim.take_deltas();
        // To (200, 80), in sector 3.
        send_move(&mut sim, 7, 1, &row(80, 120, 200));
        sim.update();
        assert_eq!(synced(&sim.take_deltas(), 7).map(|s| s.destination()), Some(t(200, 80)), "accepted");
        assert!(loaded(&sim).contains(&3));
    }

    #[test]
    fn npcs_within_thirty_tiles_of_a_pc_are_active() {
        let dir = tempfile::tempdir().unwrap();
        write_wide_map(dir.path());
        // The NPC stands at (80, 80).
        let mut sim = booted(dir.path(), save(&[(7, "A", t(110, 80))]));
        let active = |sim: &mut SimApp| {
            let world = sim.app.world_mut();
            world.query_filtered::<(), (bevy_ecs::query::With<Npc>, bevy_ecs::query::With<crate::Active>)>().iter(world).count()
        };
        assert_eq!(active(&mut sim), 1, "30 tiles away");
        let world = sim.app.world_mut();
        world.query_filtered::<&mut Position, bevy_ecs::query::With<PlayerControlled>>().single_mut(world).unwrap().0 = t(111, 80);
        sim.update();
        assert_eq!(active(&mut sim), 0, "31 tiles away");
    }

    #[test]
    fn out_of_combat_the_party_walks_at_its_leaders_pace() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", t(70, 80)), (9, "B", t(70, 82))]));
        sim.app.world_mut().resource_mut::<PartyPace>().leader = Some(PlayerId(7));
        // Player 9's PC is slow: half the stride of the leader's.
        let world = sim.app.world_mut();
        let slow = world.query::<(Entity, &PlayerControlled)>().iter(world).find(|(_, p)| p.0 == PlayerId(9)).unwrap().0;
        world.entity_mut(slow).insert(Gait { frame_millis: 58, pixels_per_frame: Gait::HUMAN_PIXELS.map(|p| p / 2.0) });
        sim.update();
        let deltas = sim.take_deltas();
        for to in [7, 9] {
            assert!(deltas.iter().any(|(p, d)| p.0 == to && matches!(d, LedgerDelta::PartyLeader { .. })), "player {to} told: {deltas:?}");
        }
        send_move(&mut sim, 7, 1, &row(80, 70, 73));
        send_move(&mut sim, 9, 1, &row(82, 70, 73));
        let mut arrived = [None, None];
        for tick in 0..ticks_for(6) {
            sim.update();
            for (i, p) in [7, 9].into_iter().enumerate() {
                if arrived[i].is_none() && position(&mut sim, p).x == 73 {
                    arrived[i] = Some(tick);
                }
            }
        }
        assert!(arrived[0].is_some() && arrived[0] == arrived[1], "together: {arrived:?}");
    }

    #[test]
    fn a_wandering_npc_walks_round_its_standpoint_and_players_see_it() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "A", t(70, 80))]));
        sim.take_deltas();
        // As if its npc_flags had ONF_WANDERS.
        let world = sim.app.world_mut();
        world.query_filtered::<&mut crate::Standpoints, bevy_ecs::query::With<Npc>>().single_mut(world).unwrap().wanders = true;
        let npc_at = |sim: &mut SimApp| {
            let world = sim.app.world_mut();
            world.query_filtered::<&Position, bevy_ecs::query::With<Npc>>().single(world).unwrap().0
        };
        let mut seen_walking = false;
        let mut places = BTreeSet::new();
        for _ in 0..30 * 30 {
            sim.update();
            let at = npc_at(&mut sim);
            places.insert((at.x, at.y));
            assert!(crate::within_radius(at, t(80, 80), crate::WANDER_RANGE), "{at:?}");
            for (_, d) in sim.take_deltas() {
                if let LedgerDelta::StateSync { actors, .. } | LedgerDelta::Resync { actors, .. } = d {
                    seen_walking |= actors.iter().any(|a| a.net_id == NetId(1) && !a.path.is_empty());
                }
            }
        }
        assert!(places.len() > 2, "it moved about: {places:?}");
        assert!(seen_walking, "players were told");
    }
}
