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


/// NPCs a script changed (switched off or on, destroyed, transformed, new art): [`spawn_npcs`]
/// spawns them again as they are now.
#[derive(Resource, Debug, Default)]
pub struct ToggledNpcs(pub Vec<(SectorId, u32)>);

/// Spawns the NPCs of every loaded sector not populated yet, in sector and file order (so two
/// hosts booted from one save give them the same net ids), and announces each to every player
/// in the world. Their art is their own or their prototype's. NPCs switched off (object flag
/// 0x2) are not spawned; those a script switches off go, and those it switches on come.
#[allow(clippy::too_many_arguments)]
pub fn spawn_npcs(
    mut commands: Commands,
    map: Res<LoadedMap>,
    mut populated: ResMut<PopulatedSectors>,
    mut ids: ResMut<NetIds>,
    session: Res<Session>,
    mut outbox: ResMut<Outbox>,
    vfs: Option<Res<SimVfs>>,
    mut toggled: ResMut<ToggledNpcs>,
    npcs: Query<(Entity, &NetId, &crate::NpcSource)>,
) {
    let mut new: Vec<SectorId> = map.sectors.keys().filter(|id| !populated.0.contains(id)).copied().collect();
    if new.is_empty() && toggled.0.is_empty() {
        return;
    }
    new.sort_by_key(|id| (id.y(), id.x()));
    let players: Vec<PlayerId> = session.players.iter().filter(|(_, s)| matches!(s, SessionPlayer::InWorld(_))).map(|(&p, _)| p).collect();
    let vfs_guard = vfs.as_ref().and_then(|v| v.0.read().ok());
    let off = |object: &Object| map.prototypes.int(object, "flags").unwrap_or(0) & (crate::OF_OFF | crate::OF_DESTROYED) != 0;
    let spawn = |commands: &mut Commands, ids: &mut NetIds, outbox: &mut Outbox, sector: SectorId, index: usize, object: &Object| {
        let (Some(position), Some(art), Some(prototype)) = (crate::object_location(object), map.prototypes.art_id(object), object.prototype.prototype()) else { return };
        let net_id = ids.0.allocate();
        let speed = crate::combat::npc_speed(object, &map.prototypes);
        let look = Appearance { art, speed };
        let gait = Gait::load(art, speed, |p| vfs_guard.as_deref().and_then(|v| v.read(p).ok()));
        let mut npc = commands.spawn((net_id, Npc { prototype }, crate::NpcSource { sector, index: index as u32 }, Position(position), look, gait));
        // A corpse, or one knocked out, thinks no more: no standpoints, no wandering.
        if !map.npc_down(sector, index as u32) {
            npc.insert(crate::Standpoints::of(object, &map.prototypes, position));
        }
        let spawn = look.spawn(net_id, EntityKind::Npc { prototype }, position);
        outbox.0.extend(players.iter().map(|&p| (p, spawn.clone())));
    };
    // A follower can be away from its own sector: it is not spawned twice.
    let mut spawned: BTreeSet<(SectorId, u32)> = npcs.iter().map(|(_, _, s)| (s.sector, s.index)).collect();
    spawned.extend(map.away.iter().copied());
    for id in new {
        populated.0.insert(id);
        for (index, object) in map.mobiles.get(&id).into_iter().flatten().enumerate().filter(|(i, o)| o.object_type == TYPE_NPC && !off(o) && !spawned.contains(&(id, *i as u32))) {
            spawn(&mut commands, &mut ids, &mut outbox, id, index, object);
        }
    }
    for (sector, index) in std::mem::take(&mut toggled.0) {
        let Some(object) = map.mobiles.get(&sector).and_then(|m| m.get(index as usize)).filter(|o| o.object_type == TYPE_NPC) else { continue };
        // Spawned again as it is now: gone if off or destroyed, with its new look if changed.
        if let Some((entity, &net_id, _)) = npcs.iter().find(|(_, _, s)| s.sector == sector && s.index == index) {
            commands.entity(entity).despawn();
            outbox.0.extend(players.iter().map(|&p| (p, LedgerDelta::Despawn { net_id })));
        }
        if !off(object) && populated.0.contains(&sector) {
            spawn(&mut commands, &mut ids, &mut outbox, sector, index as usize, object);
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
    /// The lighting schemes, read once and kept ([`crate::light::Schemes`]).
    pub light: Arc<crate::light::Schemes>,
    /// The lights the loaded sectors' objects carry ([`crate::light::object_lights`]).
    pub object_lights: std::collections::HashMap<SectorId, Vec<arcanum_formats::sector::Light>>,
    /// The lights critters bear ([`crate::light::update_borne_lights`]).
    pub borne_lights: Vec<arcanum_formats::sector::Light>,
    /// The loaded sectors' lights' art, frame 0 decoded, by art id (`None`: unreadable).
    pub light_frames: std::collections::HashMap<u32, Option<Arc<arcanum_formats::art::ArtFrame>>>,
    /// What the loaded sectors' objects block.
    pub blocking: crate::ObjectBlocking,
    /// What they block for missiles in flight ([`crate::blocking::missile_blocks`]).
    pub missiles: crate::ObjectBlocking,
    /// What sounds pay on their way ([`crate::blocking::SoundCosts`]).
    pub sound: crate::blocking::SoundCosts,
    /// The map's mobile objects (`G_<guid>.mob` files: NPCs, containers, items on the ground) by
    /// sector, in file name order, read at [`LoadedMap::open`]. Objects carried or inside a
    /// container (flag `INVENTORY`), destroyed or off are left out.
    pub mobiles: HashMap<SectorId, Vec<Object>>,
    /// The map's items inside containers (or carried: flag `INVENTORY`), by their own id, for the
    /// containers' stored inventory lists (`container_inventory_list_idx`).
    pub stored_items: HashMap<arcanum_formats::object::ObjectId, Object>,
    /// Changes to the map's objects since the original data (doors opened or closed), applied to
    /// each sector as it loads: the save's on the host, the host's on a client.
    pub changes: BTreeMap<SectorId, crate::SectorDelta>,
    /// Whether day locks hold now ([`crate::lock_day`]): doors locked by day or by night block
    /// accordingly. Set from the game clock ([`LoadedMap::set_lock_day`]).
    pub lock_day: bool,
    /// The map's jump points (`map.jmp`, [`crate::big_move`]).
    pub jump_points: Vec<arcanum_formats::map::JumpPoint>,
    /// Followers from other maps added to the mobile objects: their place here and their home
    /// ([`crate::follow`]).
    pub guests: BTreeMap<(SectorId, u32), crate::follow::NpcHome>,
    /// This map's NPCs away with their leaders: not spawned.
    pub away: BTreeSet<(SectorId, u32)>,
}

impl LoadedMap {
    /// Starts over on `name`, reading its `map.prp`. The prototypes are read once and kept.
    pub fn open(&mut self, vfs: Option<&Vfs>, name: &str) {
        let prototypes = std::mem::take(&mut self.prototypes);
        let art = self.art.take();
        let light = std::mem::take(&mut self.light);
        let lock_day = self.lock_day;
        *self = Self { name: Some(name.to_string()), prototypes, art, light, lock_day, ..Self::default() };
        let Some(vfs) = vfs else { return };
        if self.light.0.is_empty() {
            self.light = Arc::new(crate::light::Schemes::load(vfs));
        }
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
        if let Ok(data) = vfs.read(&format!("maps/{name}/map.jmp")) {
            self.jump_points = arcanum_formats::map::parse_jump_points(&data);
        }
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
    pub(crate) fn read_mobiles(&mut self, vfs: &Vfs, name: &str) {
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
            if flags & (crate::OF_DESTROYED | crate::OF_OFF) != 0 {
                continue;
            }
            if flags & crate::OF_INVENTORY != 0 {
                if crate::is_item(object.object_type) {
                    self.stored_items.insert(object.id, object);
                }
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
                self.read_light_frames(vfs, id);
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
        self.missiles.remove_sector(id);
        self.sound.remove_sector(id);
        let Some(sector) = self.sectors.get(&id) else { return };
        self.missiles.add_missiles(&sector.objects, &self.prototypes);
        self.missiles.add_missiles(self.mobiles.get(&id).into_iter().flatten(), &self.prototypes);
        self.sound.add(&sector.objects, &self.prototypes);
        self.sound.add(self.mobiles.get(&id).into_iter().flatten(), &self.prototypes);
        // Critters' own lights follow them instead ([`crate::light::update_borne_lights`]).
        let lights: Vec<arcanum_formats::sector::Light> = sector.objects.iter().chain(self.mobiles.get(&id).into_iter().flatten()).filter(|o| !matches!(o.object_type, arcanum_formats::object::TYPE_PC | arcanum_formats::object::TYPE_NPC)).flat_map(|o| crate::light::object_lights(o, &self.prototypes)).collect();
        self.object_lights.insert(id, lights);
        if let Some(art) = &self.art {
            let (x0, y0) = (id.x() * 64, id.y() * 64);
            for y in 0..64 {
                for x in 0..64 {
                    if art.tile_blocks(sector.tile(x, y).art_id()) {
                        self.blocking.tiles.insert(TileLoc { x: x0 + x, y: y0 + y });
                        self.sound.ground.insert(TileLoc { x: x0 + x, y: y0 + y });
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
            self.missiles.remove_sector(*id);
            self.sound.remove_sector(*id);
            self.object_lights.remove(id);
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

/// What the in-game interface shows of a PC with `sheet`: its name, portrait and pools
/// (fatigue less what casting spent; nothing takes damage yet).
pub fn pc_status(rules: &arcanum_rules::RulesData, sheet: &CharacterSheet) -> PcStatus {
    let derived = rules.derived(sheet);
    PcStatus {
        name: sheet.name.clone(),
        portrait: rules.portrait(sheet).map(str::to_string),
        hit_points: Pool { current: derived.hit_points - sheet.hp_damage, max: derived.hit_points },
        fatigue: Pool { current: derived.fatigue - sheet.fatigue_damage, max: derived.fatigue },
        level: sheet.level,
        dead: sheet.dead,
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
        map.place_followers(vfs, &save.followers);
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
            save.players.insert(PlayerId(id), PlayerSave { blueprint: CharacterBlueprint { sheet, gold: 0, items: vec![], prototype: None }, position, inventory: None, script_state: Default::default(), worn: None });
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
    fn an_npcs_own_light_goes_where_it_walks() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut b = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70)).field("light_aid", Value::Int32(0x9008_0000u32 as i32)).field("light_color", Value::Int32(0xffffff));
        b.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), b.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 60, y: 60 })]));
        sim.update();
        sim.update();
        let npc = { let world = sim.app.world_mut(); world.query::<(Entity, &crate::Position)>().iter(world).find(|q| q.1.0 == TileLoc { x: 72, y: 70 }).unwrap().0 };
        assert!(sim.app.world().resource::<LoadedMap>().object_lights.values().flatten().next().is_none(), "not a placed light");
        sim.app.world_mut().get_mut::<crate::Position>(npc).unwrap().0 = TileLoc { x: 75, y: 71 };
        for _ in 0..crate::light::BORNE_TICKS {
            sim.update();
        }
        let lights = &sim.app.world().resource::<LoadedMap>().borne_lights;
        assert!(lights.iter().any(|l| (l.x, l.y) == (75, 71)), "{lights:?}");
    }

    #[test]
    fn a_pc_wearing_a_lantern_lights_its_tile() {
        use arcanum_formats::object::{Object, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut lantern = Object::parse_instance(&ObjectBuilder::new(arcanum_formats::object::TYPE_GENERIC).field("item_flags", Value::Int32(0x800)).build()).unwrap();
        lantern.id = prototype_id(9100);
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut prototypes = Prototypes::default();
        prototypes.insert(lantern);
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        sim.boot(save(&[(1, "A", TileLoc { x: 70, y: 70 })]));
        sim.update();
        sim.update();
        let pc = { let world = sim.app.world_mut(); world.query_filtered::<Entity, With<crate::PlayerControlled>>().iter(world).next().unwrap() };
        sim.app.world_mut().get_mut::<crate::Inventory>(pc).unwrap().items.push(crate::CarriedItem { prototype: 9100, location: crate::FIRST_SLOT, quantity: 0, keys: vec![], written: 0, wear: Default::default() });
        for _ in 0..crate::light::BORNE_TICKS {
            sim.update();
        }
        let lights = &sim.app.world().resource::<LoadedMap>().borne_lights;
        assert_eq!(lights.len(), 1);
        assert_eq!(((lights[0].x, lights[0].y), lights[0].color, lights[0].art_id.0), ((70, 70), [0xff, 0xd6, 0xac], crate::light::BORNE_LIGHT_ART), "a large light at the PC");
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
        assert_eq!(kit.items[0], crate::CarriedItem { prototype: crate::GOLD, location: 0, quantity: 385, keys: vec![], written: 0, wear: Default::default() });
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
    fn items_are_picked_up_and_dropped_through_the_host() {
        use arcanum_formats::object::{Object, TYPE_GOLD, TYPE_WEAPON, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        use crate::{GroundItemId, InventoryRequest};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        // 50 gold on the ground beside the PC.
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut gold_mob = ObjectBuilder::new(TYPE_GOLD).field("location", at(71, 70)).field("gold_quantity", Value::Int32(50));
        gold_mob.prototype = prototype_id(crate::GOLD);
        std::fs::write(dir.path().join("maps").join(MAP).join("G_Z.mob"), gold_mob.build()).unwrap();
        let proto = |number: u32, kind: u32| {
            let mut o = Object::parse_instance(&ObjectBuilder::new(kind).field("item_weight", Value::Int32(10)).build()).unwrap();
            o.id = prototype_id(number);
            o
        };
        let mut s = save(&[(1, "A", TileLoc { x: 70, y: 70 }), (2, "B", TileLoc { x: 72, y: 72 })]);
        let player = s.players.get_mut(&PlayerId(1)).unwrap();
        player.blueprint.sheet.base[0] = 8;
        player.inventory = Some(crate::Inventory { items: vec![crate::CarriedItem { prototype: 6049, location: 0, quantity: 0, keys: vec![], written: 0, wear: Default::default() }], ..Default::default() });
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut prototypes = Prototypes::default();
        prototypes.insert(proto(crate::GOLD, TYPE_GOLD));
        prototypes.insert(proto(6049, TYPE_WEAPON));
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        sim.boot(s);
        sim.update();
        sim.update();
        sim.take_deltas();
        let sector = TileLoc { x: 71, y: 70 }.sector();
        let index = sim.app.world().resource::<LoadedMap>().mobiles[&sector].iter().position(|o| o.object_type == TYPE_GOLD).unwrap() as u32;
        let gold = GroundItemId::Mobile { sector, index };
        sim.receive([(PlayerId(1), PlayerIntent::PickUp { item: gold, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        for to in [1, 2] {
            assert!(deltas.iter().any(|(p, d)| p.0 == to && *d == LedgerDelta::GroundItemTaken { item: gold }), "player {to} sees it go");
        }
        let carried = |deltas: &[(PlayerId, LedgerDelta)]| deltas.iter().rev().find_map(|(p, d)| match d {
            LedgerDelta::Inventory { inventory, .. } if p.0 == 1 => Some(inventory.clone()),
            _ => None,
        });
        let inv = carried(&deltas).unwrap();
        assert_eq!(inv.items.iter().find(|i| i.prototype == crate::GOLD).map(|i| i.quantity), Some(50));
        assert!(sim.app.world().resource::<LoadedMap>().ground_item(gold).is_none());
        // The dagger dropped lands on the PC's tile for everyone and can be taken back.
        sim.receive([(PlayerId(1), PlayerIntent::Inventory { seq: 1, request: InventoryRequest::Drop { from: 0 } })]);
        sim.update();
        let deltas = sim.take_deltas();
        let dropped = GroundItemId::Dropped { sector: TileLoc { x: 70, y: 70 }.sector(), number: 0 };
        let ground = crate::GroundItem { prototype: 6049, quantity: 0, location: TileLoc { x: 70, y: 70 }, keys: vec![], written: 0, wear: Default::default() };
        assert!(deltas.iter().any(|(p, d)| p.0 == 2 && *d == LedgerDelta::GroundItemDropped { item: dropped, ground: ground.clone() }));
        assert!(!carried(&deltas).unwrap().items.iter().any(|i| i.prototype == 6049));
        let saved = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!((saved.next_dropped, saved.sector_changes[&sector].taken.contains(&index)), (1, true), "kept in the save");
        assert_eq!(saved.sector_changes[&dropped.sector()].dropped[&0], ground);
        sim.receive([(PlayerId(1), PlayerIntent::PickUp { item: dropped, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(carried(&deltas).unwrap().items.iter().any(|i| i.prototype == 6049));
        assert!(matches!(deltas.iter().find_map(|(p, d)| (p.0 == 1).then_some(d).filter(|d| matches!(d, LedgerDelta::Inventory { .. }))), Some(LedgerDelta::Inventory { ack: 1, .. })), "acknowledges the drop");

        // Dropping the gold onto the dagger makes a pile of both, the dagger first.
        let inv = carried(&deltas).unwrap();
        let at_of = |inv: &crate::Inventory, p: u32| inv.items.iter().find(|i| i.prototype == p).unwrap().location;
        sim.receive([
            (PlayerId(1), PlayerIntent::Inventory { seq: 2, request: InventoryRequest::Drop { from: at_of(&inv, 6049) } }),
            (PlayerId(1), PlayerIntent::Inventory { seq: 3, request: InventoryRequest::Drop { from: at_of(&inv, crate::GOLD) } }),
        ]);
        sim.update();
        let deltas = sim.take_deltas();
        let pile = GroundItemId::Pile { sector: dropped.sector(), number: 2 };
        let contents = deltas.iter().rev().find_map(|(p, d)| match d {
            LedgerDelta::PileChanged { pile: id, contents } if p.0 == 2 && *id == pile => Some(contents.clone()),
            _ => None,
        });
        let contents = contents.flatten().expect("a pile for everyone");
        assert_eq!(contents.inventory.items.iter().map(|i| i.prototype).collect::<Vec<_>>(), [6049, crate::GOLD]);
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::GroundItemTaken { item: GroundItemId::Dropped { number: 1, .. } })), "the bare dagger went into it");
        // Opening it, taking the gold and closing it leaves the dagger bare again.
        sim.receive([(PlayerId(1), PlayerIntent::PickUp { item: pile, seq: 0 })]);
        sim.update();
        assert!(sim.take_deltas().iter().any(|(p, d)| p.0 == 1 && *d == LedgerDelta::PileOpened { pile }));
        let gold_at = at_of(&contents.inventory, crate::GOLD);
        sim.receive([
            (PlayerId(1), PlayerIntent::Inventory { seq: 4, request: InventoryRequest::Take { from: gold_at, to: None } }),
            (PlayerId(1), PlayerIntent::Inventory { seq: 5, request: InventoryRequest::ClosePile }),
        ]);
        sim.update();
        let deltas = sim.take_deltas();
        assert_eq!(carried(&deltas).unwrap().items.iter().find(|i| i.prototype == crate::GOLD).map(|i| i.quantity), Some(50));
        assert!(deltas.iter().any(|(_, d)| *d == LedgerDelta::PileChanged { pile, contents: None }), "the pile goes");
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::GroundItemDropped { ground, .. } if ground.prototype == 6049)), "its last item lies bare");
        let map = sim.app.world().resource::<LoadedMap>();
        assert!(map.pile(pile).is_none());
        assert_eq!(map.items_on_tile(TileLoc { x: 70, y: 70 }).len(), 1);
    }

    #[test]
    fn a_shopkeeper_restocks_a_day_after_the_window_opened() {
        use arcanum_formats::object::{Array, Object, ObjectId, TYPE_GOLD, TYPE_WEAPON, Value};
        use arcanum_formats::test_util::{ObjectBuilder, object_id_bytes, prototype_id};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let guid = |n: u8| ObjectId { kind: ObjectId::KIND_GUID, payload: [n; 16] };
        // A kept dagger (item_flags 0x2000) at 3 and a plain sword at 5.
        for (n, proto, loc, flags) in [(9u8, 6049u32, 3, 0x2000), (10, 6051, 5, 0)] {
            let mut item = ObjectBuilder::new(TYPE_WEAPON).field("flags", Value::Int32(crate::OF_INVENTORY)).field("item_inv_location", Value::Int32(loc)).field("item_flags", Value::Int32(flags));
            item.prototype = prototype_id(proto);
            item.id = guid(n);
            std::fs::write(folder.join(format!("G_I{n}.mob")), item.build()).unwrap();
        }
        let data = [guid(9), guid(10)].iter().flat_map(object_id_bytes).collect();
        let shop = ObjectBuilder::new(TYPE_NPC)
            .field("current_aid", Value::Int32(0x2800_2000))
            .field("location", at(72, 70))
            .field("critter_inventory_source", Value::Int32(1))
            .field("critter_inventory_list_idx", Value::Array(Some(Array { elem_size: 24, id: 0, data, bitset: vec![] })));
        std::fs::write(folder.join("G_0S.mob"), shop.build()).unwrap();
        std::fs::create_dir_all(dir.path().join("Rules")).unwrap();
        std::fs::write(dir.path().join("Rules/InvenSource.mes"), format!("{{1}}{{Shop: 5,5 100,{}}}\n", 6050 - crate::BASIC_TO_PROTOTYPE)).unwrap();
        let proto = |number: u32, kind: u32| {
            let mut o = Object::parse_instance(&ObjectBuilder::new(kind).field("item_weight", Value::Int32(1)).field("item_worth", Value::Int32(10)).build()).unwrap();
            o.id = prototype_id(number);
            o
        };
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut prototypes = Prototypes::default();
        for (n, k) in [(crate::GOLD, TYPE_GOLD), (6049, TYPE_WEAPON), (6050, TYPE_WEAPON), (6051, TYPE_WEAPON)] {
            prototypes.insert(proto(n, k));
        }
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let (npc, object, index) = {
            let world = sim.app.world_mut();
            let map = world.resource::<LoadedMap>();
            let index = map.mobiles[&SectorId::new(1, 1)].iter().position(|o| o.object_type == TYPE_NPC && crate::object_location(o) == Some(TileLoc { x: 72, y: 70 })).unwrap() as u32;
            let npc = *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == index).unwrap().0;
            (npc, crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index }, index)
        };
        let open_window = |sim: &mut SimApp| {
            sim.app.world_mut().resource_mut::<crate::barter::BarterStarts>().0.push((PlayerId(1), npc, object));
            sim.update();
            sim.take_deltas().into_iter().find_map(|(_, d)| match d { LedgerDelta::Barter { view } => Some(view), _ => None }).unwrap()
        };
        let goods = |view: &crate::barter::BarterView| { let mut p: Vec<u32> = view.stock.items.iter().map(|i| i.prototype).collect(); p.sort(); p };
        let first = open_window(&mut sim);
        assert_eq!(goods(&first), [6049, 6051], "no restock on the first opening");
        let due = sim.app.world().resource::<WorldState>().0.as_ref().unwrap().sector_changes[&SectorId::new(1, 1)].npc_restock[&index];
        let now = sim.app.world().resource::<crate::GameClock>().now;
        assert!(due > now.plus_ms(43_199_999) && due <= now.plus_ms(86_400_000), "12 to 24 hours on");
        sim.app.world_mut().resource_mut::<crate::GameClock>().jump(25 * 3_600_000);
        let second = open_window(&mut sim);
        assert_eq!(goods(&second), [6049, 6050, crate::GOLD], "the kept dagger stays, the sword goes, the source rolls");
        assert_eq!(second.npc_gold, 5);
    }

    #[test]
    fn a_pc_fights_an_npc_turn_by_turn_until_it_dies() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut foe = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70)).field("npc_experience_worth", Value::Int32(100)).field("npc_experience_pool", Value::Int32(100));
        foe.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), foe.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let (npc, index) = {
            let world = sim.app.world_mut();
            let map = world.resource::<LoadedMap>();
            let index = map.mobiles[&SectorId::new(1, 1)].iter().position(|o| o.object_type == TYPE_NPC && crate::object_location(o) == Some(TileLoc { x: 72, y: 70 })).unwrap() as u32;
            (*world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == index).unwrap().0, index)
        };
        let pc = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::PlayerControlled)>().iter(world).next().unwrap().0 };
        sim.take_deltas();
        let mut attacks = 0;
        let mut npc_hits = 0;
        let mut ended = false;
        for seq in 0..200 {
            sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: npc, seq })]);
            sim.update();
            for (_, d) in sim.take_deltas() {
                match d {
                    LedgerDelta::Attacked { attacker, target, outcome } => {
                        assert!((attacker, target) == (pc, npc) || (attacker, target) == (npc, pc), "each strikes the other");
                        if attacker == npc {
                            npc_hits += i32::from(outcome.hit);
                        }
                        attacks += 1;
                    }
                    LedgerDelta::Combat { order, turn, .. } if order.is_empty() => {
                        assert_eq!(turn, None);
                        ended = true;
                    }
                    LedgerDelta::Combat { order, .. } => assert_eq!(order, [pc, npc], "the PC first"),
                    _ => {}
                }
            }
            if ended {
                break;
            }
        }
        assert!(ended, "the NPC dies and the fight ends after {attacks} attacks");
        let map = sim.app.world().resource::<LoadedMap>();
        assert!(map.is_dead(SectorId::new(1, 1), index));
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert!(save.sector_changes[&SectorId::new(1, 1)].npc_damage[&index] >= 4, "4 hit points: no stats");
        assert!(save.players[&PlayerId(1)].blueprint.sheet.experience > 0, "experience for the hits and the kill");
        assert_eq!(save.players[&PlayerId(1)].blueprint.sheet.hp_damage > 0, npc_hits > 0, "the NPC's hits hurt the PC");
    }

    #[test]
    fn an_unconscious_pc_loses_its_turn() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut foe = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70)).field("hp_pts", Value::Int32(500));
        foe.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), foe.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let npc = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::NpcSource)>().iter(world).next().unwrap().0 };
        let pc = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::PlayerControlled)>().iter(world).next().unwrap().0 };
        sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: npc, seq: 0 })]);
        sim.update();
        let turn = |deltas: &[(PlayerId, LedgerDelta)]| deltas.iter().rev().find_map(|(_, d)| match d { LedgerDelta::Combat { turn, .. } => Some(*turn), _ => None });
        let first = turn(&sim.take_deltas());
        assert_eq!(first, Some(Some(pc)), "the PC's turn, points left");
        // Knocked out on its own turn: the turn passes to the NPC.
        sim.app.world_mut().resource_mut::<WorldState>().0.as_mut().unwrap().players.get_mut(&PlayerId(1)).unwrap().blueprint.sheet.fatigue_damage = 1000;
        sim.update();
        assert_eq!(turn(&sim.take_deltas()), Some(Some(npc)), "the NPC's turn");
    }

    #[test]
    fn poison_hurts_pcs_and_npcs_and_wears_off() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut npc = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(74, 70)).field("hp_pts", Value::Int32(60));
        npc.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), npc.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut start = save(&[(1, "A", TileLoc { x: 71, y: 70 })]);
        let sheet = &mut start.players.get_mut(&PlayerId(1)).unwrap().blueprint.sheet;
        sheet.base = [10; arcanum_rules::PRIMARY_STATS];
        sheet.poison = 300;
        sheet.spells[crate::HALT_POISON as usize / arcanum_rules::SPELLS_PER_COLLEGE] = 5;
        sim.boot(start);
        sim.update();
        sim.update();
        let sector = SectorId::new(1, 1);
        sim.app.world_mut().resource_mut::<LoadedMap>().set_npc_poison(sector, 0, 100);
        let pc = |sim: &SimApp| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(1)].blueprint.sheet.clone();
        let npc_hp = |sim: &SimApp| sim.app.world().resource::<LoadedMap>().npc_hp(sector, 0).unwrap();
        let full = npc_hp(&sim).1;
        let mut floats = 0;
        for _ in 0..8 {
            sim.app.world_mut().resource_mut::<crate::GameClock>().now.ms += crate::poison::POISON_MS as u32;
            sim.update();
            floats += sim.take_deltas().iter().filter(|(to, d)| *to == PlayerId(1) && matches!(d, LedgerDelta::Hurt { .. })).count();
        }
        assert_eq!(floats, 16, "an HT float for the PC and the NPC each hurt");
        // 8 hurts (2 a time at 300, 1 for the NPC at 100), one wearing off at 2 minutes.
        assert_eq!(pc(&sim).hp_damage, 16);
        assert_eq!(npc_hp(&sim).0, full - 8);
        let recovery = sim.app.world().resource::<crate::SimRules>().0.derived(&pc(&sim)).poison_recovery;
        assert_eq!(pc(&sim).poison, 300 - recovery, "CN's worth off");
        assert_eq!(sim.app.world().resource::<LoadedMap>().npc_poison(sector, 0), 100 - 1, "no stats: 1");
        // Halt Poison on the NPC.
        let npc = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::NpcSource)>().iter(world).next().unwrap().0 };
        sim.receive([(PlayerId(1), crate::PlayerIntent::CastOn { spell: crate::HALT_POISON as usize, target: npc })]);
        sim.update();
        assert_eq!(sim.app.world().resource::<LoadedMap>().npc_poison(sector, 0), 49, "50 off");
    }

    #[test]
    fn owners_who_see_a_pick_fight_or_like_the_pc_less() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        // The owner (name 4242) next to the PC; a stranger also near.
        for (file, name, x, payload) in [("G_0F.mob", 4242, 72, 3), ("G_10.mob", 7, 70, 4)] {
            let mut npc = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(x, 70)).field("name", Value::Int32(name));
            npc.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [payload; 16] };
            std::fs::write(folder.join(file), npc.build()).unwrap();
        }
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut start = save(&[(1, "A", TileLoc { x: 71, y: 70 })]);
        start.players.get_mut(&PlayerId(1)).unwrap().blueprint.sheet.base = [10; arcanum_rules::PRIMARY_STATS];
        sim.boot(start);
        sim.update();
        sim.update();
        sim.take_deltas();
        let owner = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|n| n.1.index == 0 && n.1.sector == SectorId::new(1, 1)).unwrap().0 };
        // It faces the PC.
        let facing = crate::rotation_of_step(TileLoc { x: 72, y: 70 }, TileLoc { x: 71, y: 70 }).unwrap();
        { let world = sim.app.world_mut(); let e = world.query::<(Entity, &NetId)>().iter(world).find(|q| *q.1 == owner).unwrap().0; world.entity_mut(e).insert(crate::Facing(facing)); }
        let key = crate::met_key(crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 });
        let stored = |sim: &SimApp| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(1)].script_state.reactions.get(&key).copied();
        // A failed pick: the owner likes the PC 20 less (its reaction stays above column 10).
        sim.app.world_mut().resource_mut::<crate::lockpick::LockWitnessing>().0.push((PlayerId(1), TileLoc { x: 72, y: 71 }, 4242, false));
        sim.update();
        let base = { let map = sim.app.world().resource::<LoadedMap>(); crate::NpcReaction::of(&map.mobiles[&SectorId::new(1, 1)][0], &map.prototypes).base };
        assert_eq!(stored(&sim), Some(base - 20));
        // An opened lock: the owner fights the PC.
        sim.app.world_mut().resource_mut::<crate::lockpick::LockWitnessing>().0.push((PlayerId(1), TileLoc { x: 72, y: 71 }, 4242, true));
        sim.update();
        sim.update();
        let order = sim.take_deltas().into_iter().find_map(|(_, d)| match d { LedgerDelta::Combat { order, .. } if !order.is_empty() => Some(order), _ => None });
        assert!(order.is_some_and(|o| o.contains(&owner)), "the owner fights");
    }

    #[test]
    fn a_follower_remarks_on_its_leaders_critical_hits() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        use crate::follow::Follower;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        std::fs::create_dir_all(dir.path().join("mes")).unwrap();
        for file in ["gd_npc_m2m", "gd_npc_m2f", "gd_npc_f2m", "gd_npc_f2f"] {
            std::fs::write(dir.path().join("mes").join(format!("{file}.mes")), "{2400}{Nice hit!}").unwrap();
        }
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let npc = |x: u32, y: u32, n: u8, hp: i32| {
            let mut b = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(x, y)).field("hp_pts", Value::Int32(hp)).field("critter_fatigue_pts", Value::Int32(hp));
            b.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [n; 16] };
            b.build()
        };
        // Hard to kill or tire: the fight goes on.
        std::fs::write(folder.join("G_0F.mob"), npc(72, 70, 3, 2000)).unwrap();
        std::fs::write(folder.join("G_10.mob"), npc(72, 71, 4, 2000)).unwrap();
        let sector = SectorId::new(1, 1);
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut start = save(&[(1, "A", TileLoc { x: 71, y: 70 })]);
        let sheet = &mut start.players.get_mut(&PlayerId(1)).unwrap().blueprint.sheet;
        sheet.base = [20; arcanum_rules::PRIMARY_STATS];
        sheet.skills[crate::combat::SKILL_MELEE] = 5;
        sim.boot(start);
        sim.update();
        sim.update();
        let index_at = |sim: &SimApp, x: u32, y: u32| sim.app.world().resource::<LoadedMap>().mobiles[&sector].iter().position(|o| o.object_type == TYPE_NPC && crate::object_location(o) == Some(TileLoc { x, y })).unwrap() as u32;
        let (foe_index, follower_index) = (index_at(&sim, 72, 70), index_at(&sim, 72, 71));
        sim.app.world_mut().resource_mut::<WorldState>().0.as_mut().unwrap().followers.push(Follower { map: MAP.into(), sector, index: follower_index, leader: PlayerId(1), away: None, wait_until: None, warn: 0 });
        for _ in 0..3 {
            sim.update();
        }
        let id = |sim: &mut SimApp, i: u32| { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == i).unwrap().0 };
        let (foe, follower) = (id(&mut sim, foe_index), id(&mut sim, follower_index));
        let (mut criticals, mut remarks, mut attacks) = (0, 0, 0);
        for seq in 0..400 {
            sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: foe, seq })]);
            sim.update();
            for (_, d) in sim.take_deltas() {
                match d {
                    LedgerDelta::Attacked { target, outcome, .. } if target == foe && outcome.critical && outcome.hit => criticals += 1,
                    LedgerDelta::Attacked { .. } => attacks += 1,
                    LedgerDelta::FloatText { speaker, text, .. } if speaker == follower && text == "Nice hit!" => remarks += 1,
                    _ => {}
                }
            }
        }
        assert!(remarks > 0 && remarks <= criticals, "{remarks} remarks on {criticals} criticals (the PC's or the follower's own), {attacks} attacks");
    }

    #[test]
    fn a_knocked_down_critter_gets_up_after_a_recovery_period() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let pc = { let world = sim.app.world_mut(); world.query_filtered::<Entity, With<crate::PlayerControlled>>().iter(world).next().unwrap() };
        sim.app.world_mut().entity_mut(pc).insert(crate::combat::KnockedDown { until: None });
        sim.update();
        let until = sim.app.world().get::<crate::combat::KnockedDown>(pc).and_then(|d| d.until);
        assert!(until.is_some(), "its time is set");
        let anim = |sim: &SimApp| (sim.app.world().get::<Appearance>(pc).unwrap().art.0 >> 6) & 0x1f;
        assert_eq!(anim(&sim), crate::combat::ANIM_LYING, "joiners spawn it lying");
        let now = sim.app.world().resource::<crate::GameClock>().now;
        sim.app.world_mut().resource_mut::<crate::GameClock>().now = now.plus_ms(crate::RECOVERY_MS / 2);
        sim.update();
        assert!(sim.app.world().get::<crate::combat::KnockedDown>(pc).is_some(), "still lying half a period on");
        sim.take_deltas();
        sim.app.world_mut().resource_mut::<crate::GameClock>().now = now.plus_ms(crate::RECOVERY_MS + 1000);
        sim.update();
        assert!(sim.app.world().get::<crate::combat::KnockedDown>(pc).is_none(), "up after a period");
        assert_eq!(anim(&sim), arcanum_formats::art_id::ANIM_STAND, "standing again");
        let id = *sim.app.world().get::<NetId>(pc).unwrap();
        assert!(sim.take_deltas().iter().any(|(_, d)| matches!(d, LedgerDelta::GotUp { net_id } if *net_id == id)), "every player is told");
    }

    #[test]
    fn a_lamed_npc_walks_slower() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut b = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70));
        b.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), b.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let (npc, entity) = { let world = sim.app.world_mut(); world.query::<(Entity, &NetId, &crate::Position)>().iter(world).find(|q| q.2.0 == TileLoc { x: 72, y: 70 }).map(|q| (*q.1, q.0)).unwrap() };
        let speed = |sim: &SimApp| sim.app.world().get::<Appearance>(entity).unwrap().speed;
        let index = sim.app.world().get::<crate::NpcSource>(entity).unwrap().index;
        let whole = { let map = sim.app.world().resource::<LoadedMap>(); crate::combat::npc_speed(&map.mobiles[&SectorId::new(1, 1)][index as usize], &map.prototypes) };
        assert_eq!(speed(&sim), whole, "spawned at its own speed");
        // A critical cripples its legs.
        sim.app.world_mut().resource_mut::<LoadedMap>().injure_npc(SectorId::new(1, 1), index, crate::combat::OCF_CRIPPLED_LEGS);
        sim.take_deltas();
        for _ in 0..crate::combat::HEALTH_TICKS {
            sim.update();
        }
        assert_eq!(speed(&sim), whole - 5);
        assert!(sim.take_deltas().iter().any(|(_, d)| matches!(d, LedgerDelta::Speed { net_id, speed } if *net_id == npc && *speed == whole - 5)), "every player is told");
    }

    #[test]
    fn a_paralyzed_npc_loses_its_turns() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut b = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70)).field("hp_pts", Value::Int32(2000)).field("critter_fatigue_pts", Value::Int32(2000)).field("critter_flags", Value::Int32(crate::combat::OCF_PARALYZED));
        b.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), b.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let npc = { let world = sim.app.world_mut(); world.query::<(&NetId, &crate::Position)>().iter(world).find(|q| q.1.0 == TileLoc { x: 72, y: 70 }).map(|q| *q.0).unwrap() };
        let (mut struck, mut its_blows) = (0, 0);
        for seq in 0..40 {
            sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: npc, seq })]);
            sim.update();
            sim.receive([(PlayerId(1), crate::PlayerIntent::EndTurn)]);
            sim.update();
            for (_, d) in sim.take_deltas() {
                match d {
                    LedgerDelta::Attacked { attacker, .. } if attacker == npc => its_blows += 1,
                    LedgerDelta::Attacked { .. } => struck += 1,
                    _ => {}
                }
            }
        }
        assert!(struck > 0 && its_blows == 0, "struck {struck} times, struck back {its_blows}");
    }

    #[test]
    fn a_surrendered_npc_begs_another_attacker() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        std::fs::create_dir_all(dir.path().join("mes")).unwrap();
        for file in ["gd_npc_m2m", "gd_npc_m2f", "gd_npc_f2m", "gd_npc_f2f"] {
            std::fs::write(dir.path().join("mes").join(format!("{file}.mes")), "{3000}{Mercy!}").unwrap();
        }
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut b = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70)).field("hp_pts", Value::Int32(2000)).field("critter_fatigue_pts", Value::Int32(2000));
        b.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), b.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let (npc, entity) = { let world = sim.app.world_mut(); world.query::<(Entity, &NetId, &crate::Position)>().iter(world).find(|q| q.2.0 == TileLoc { x: 72, y: 70 }).map(|q| (*q.1, q.0)).unwrap() };
        // It gave up to someone else: the NPC at (80, 80).
        let other = { let world = sim.app.world_mut(); world.query::<(&NetId, &crate::Position)>().iter(world).find(|q| q.1.0 == TileLoc { x: 80, y: 80 }).map(|q| *q.0).unwrap() };
        sim.app.world_mut().entity_mut(entity).insert(crate::combat::Surrendered { to: other });
        let mut begs = 0;
        for seq in 0..3 {
            sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: npc, seq })]);
            sim.update();
            begs += sim.take_deltas().into_iter().filter(|(_, d)| matches!(d, LedgerDelta::FloatText { speaker, text, .. } if *speaker == npc && text == "Mercy!")).count();
        }
        assert!(begs > 0, "it begs the PC");
        // The manual clock moves no walker: its flight is set.
        let away = sim.app.world().get::<crate::Walking>(entity).and_then(|w| w.steps.back().copied());
        assert!(away.is_some_and(|t| t.x > 72), "and flees it east: {away:?}");
        assert!(sim.app.world().get::<crate::combat::Surrendered>(entity).is_some(), "and does not fight it");
    }

    #[test]
    fn a_follower_brought_under_a_fifth_calls_to_its_leader() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        use crate::follow::Follower;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        std::fs::create_dir_all(dir.path().join("mes")).unwrap();
        for file in ["gd_npc_m2m", "gd_npc_m2f", "gd_npc_f2m", "gd_npc_f2f"] {
            std::fs::write(dir.path().join("mes").join(format!("{file}.mes")), "{5500}{Help me!}").unwrap();
        }
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut b = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70)).field("hp_pts", Value::Int32(10)).field("critter_fatigue_pts", Value::Int32(500));
        b.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [4; 16] };
        std::fs::write(folder.join("G_0F.mob"), b.build()).unwrap();
        // A foe beside it, hard to hurt.
        let mut foe = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(73, 70)).field("hp_pts", Value::Int32(2000)).field("critter_fatigue_pts", Value::Int32(2000));
        foe.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [5; 16] };
        std::fs::write(folder.join("G_10.mob"), foe.build()).unwrap();
        let sector = SectorId::new(1, 1);
        let mut start = save(&[(1, "A", TileLoc { x: 71, y: 70 })]);
        start.followers.push(Follower { map: MAP.into(), sector, index: 0, leader: PlayerId(1), away: None, wait_until: None, warn: 0 });
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(start);
        for _ in 0..4 {
            sim.update();
        }
        let id = |sim: &mut SimApp, i: u32| { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == i).unwrap().0 };
        let (follower, foe) = (id(&mut sim, 0), id(&mut sim, 1));
        sim.app.world_mut().resource_mut::<crate::combat::CombatStarts>().0.push((foe, follower));
        let mut calls = 0;
        for _ in 0..3000 {
            sim.receive([(PlayerId(1), crate::PlayerIntent::EndTurn)]);
            sim.update();
            calls += sim.take_deltas().iter().filter(|(to, d)| *to == PlayerId(1) && matches!(d, LedgerDelta::FloatText { speaker, text, .. } if *speaker == follower && text == "Help me!")).count();
            if sim.app.world().resource::<LoadedMap>().is_dead(sector, 0) {
                break;
            }
        }
        let hp = sim.app.world().resource::<LoadedMap>().npc_hp(sector, 0);
        let following = { let world = sim.app.world_mut(); world.query::<&crate::follow::Following>().iter(world).count() };
        assert_eq!(calls, 1, "once, as it crosses 20 % ({hp:?}, {following} following)");
    }

    #[test]
    fn a_follower_its_leader_attacks_likes_it_less_and_may_leave() {
        use crate::follow::Follower;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        // Column 9: -20 a hit; column 5: it leaves at or under 1000 reaction (always).
        std::fs::create_dir_all(dir.path().join("rules")).unwrap();
        std::fs::write(dir.path().join("rules").join("ai_params.mes"), "{0}{0 1000 1000 0 6 1000 1001 1000 2 -20 0 2001 100 50 20 1 1}").unwrap();
        // The map's NPC 0 at (80, 80) follows the PC.
        let sector = SectorId::new(1, 1);
        let mut start = save(&[(1, "A", TileLoc { x: 79, y: 80 })]);
        start.followers.push(Follower { map: MAP.into(), sector, index: 0, leader: PlayerId(1), away: None, wait_until: None, warn: 0 });
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(start);
        for _ in 0..4 {
            sim.update();
        }
        let follower = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|n| n.1.index == 0).unwrap().0 };
        let key = crate::met_key(crate::ScriptObject::Mobile { sector, index: 0 });
        let base = { let map = sim.app.world().resource::<LoadedMap>(); crate::NpcReaction::of(&map.mobiles[&sector][0], &map.prototypes).base };
        let before = sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(1)].script_state.reactions.get(&key).copied().unwrap_or(base);
        sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: follower, seq: 0 })]);
        sim.update();
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.players[&PlayerId(1)].script_state.reactions.get(&key).copied(), Some(before - 20), "column 9");
        assert!(save.followers.is_empty(), "it leaves (0x45dfc0)");
        assert_eq!(sim.app.world().resource::<LoadedMap>().npc_flags(sector, 0).unwrap() & crate::follow::LEFT, crate::follow::LEFT);
    }

    #[test]
    fn a_follower_that_stays_after_its_leaders_hit_warns_unless_the_leader_charms() {
        use crate::follow::Follower;
        for (charisma, warns) in [(10, true), (20, false)] {
            let dir = tempfile::tempdir().unwrap();
            write_map(dir.path());
            // Column 9: -20 a hit; column 5 -1000: it never leaves.
            std::fs::create_dir_all(dir.path().join("rules")).unwrap();
            std::fs::write(dir.path().join("rules").join("ai_params.mes"), "{0}{0 1000 1000 0 6 -1000 1001 1000 200 -20 0 2001 100 50 20 1 1}").unwrap();
            let sector = SectorId::new(1, 1);
            let mut start = save(&[(1, "A", TileLoc { x: 79, y: 80 })]);
            start.players.get_mut(&PlayerId(1)).unwrap().blueprint.sheet.base[7] = charisma;
            start.followers.push(Follower { map: MAP.into(), sector, index: 0, leader: PlayerId(1), away: None, wait_until: None, warn: 0 });
            let mut sim = SimApp::new(RulesData::default());
            sim.use_manual_clock();
            sim.app.insert_resource(vfs(dir.path()));
            sim.boot(start);
            for _ in 0..4 {
                sim.update();
            }
            let follower = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|n| n.1.index == 0).unwrap().0 };
            sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: follower, seq: 0 })]);
            sim.update();
            let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
            assert_eq!(save.followers.len(), 1, "it stays");
            assert_eq!(save.followers[0].warn & crate::follow::WARN_REACTION != 0, warns, "charisma {charisma}");
        }
    }

    #[test]
    fn a_follower_refuses_to_fight_its_own_faction() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        use crate::follow::Follower;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let npc = |x: u32, y: u32, faction: i32, n: u8| {
            let mut b = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(x, y)).field("npc_faction", Value::Int32(faction));
            b.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [n; 16] };
            b.build()
        };
        std::fs::write(folder.join("G_0F.mob"), npc(72, 70, 7, 3)).unwrap();
        std::fs::write(folder.join("G_10.mob"), npc(71, 71, 7, 4)).unwrap();
        let sector = SectorId::new(1, 1);
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let (victim_index, follower_index) = {
            let map = sim.app.world().resource::<LoadedMap>();
            let index_at = |x: u32, y: u32| map.mobiles[&sector].iter().position(|o| o.object_type == TYPE_NPC && crate::object_location(o) == Some(TileLoc { x, y })).unwrap() as u32;
            (index_at(72, 70), index_at(71, 71))
        };
        sim.app.world_mut().resource_mut::<WorldState>().0.as_mut().unwrap().followers.push(Follower { map: MAP.into(), sector, index: follower_index, leader: PlayerId(1), away: None, wait_until: None, warn: 0 });
        for _ in 0..3 {
            sim.update();
        }
        let id = |sim: &mut SimApp, i: u32| { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == i).unwrap().0 };
        let (victim, follower) = (id(&mut sim, victim_index), id(&mut sim, follower_index));
        assert!({ let world = sim.app.world_mut(); world.query::<(&NetId, &crate::follow::Following)>().iter(world).any(|f| *f.0 == follower) }, "it follows");
        let key = crate::met_key(crate::ScriptObject::Mobile { sector, index: follower_index });
        let before = sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(1)].script_state.reactions.get(&key).copied();
        sim.take_deltas();
        sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: victim, seq: 0 })]);
        sim.update();
        let orders: Vec<Vec<NetId>> = sim.take_deltas().into_iter().filter_map(|(_, d)| match d { LedgerDelta::Combat { order, .. } => Some(order), _ => None }).collect();
        assert!(!orders.last().unwrap().contains(&follower), "the same faction: it stays out (0x4adcc0 code 2)");
        let after = sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(1)].script_state.reactions.get(&key).copied();
        let base = { let map = sim.app.world().resource::<LoadedMap>(); crate::NpcReaction::of(&map.mobiles[&sector][follower_index as usize], &map.prototypes).base };
        assert_eq!(after, Some(before.unwrap_or(base) - 5), "and likes its leader 5 less");
    }

    #[test]
    fn a_faction_stays_out_against_a_pc_of_its_reputation() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        for (reputed, joins) in [(false, true), (true, false)] {
            let dir = tempfile::tempdir().unwrap();
            write_map(dir.path());
            std::fs::create_dir_all(dir.path().join("rules")).unwrap();
            std::fs::write(dir.path().join("rules").join("gamerep.mes"), "{1003}{7 0 0, +10 0 7}").unwrap();
            let folder = dir.path().join("maps").join(MAP);
            let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
            let npc = |x: u32, n: u8| {
                let mut b = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(x, 70)).field("npc_faction", Value::Int32(7)).field("hp_pts", Value::Int32(100));
                b.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [n; 16] };
                b.build()
            };
            std::fs::write(folder.join("G_0F.mob"), npc(72, 3)).unwrap();
            std::fs::write(folder.join("G_10.mob"), npc(74, 4)).unwrap();
            let mut start = save(&[(1, "A", TileLoc { x: 71, y: 70 })]);
            if reputed {
                start.players.get_mut(&PlayerId(1)).unwrap().script_state.reputations.insert(1003);
            }
            let mut sim = SimApp::new(RulesData::default());
            sim.use_manual_clock();
            sim.app.insert_resource(vfs(dir.path()));
            sim.boot(start);
            sim.update();
            sim.update();
            let id_at = |sim: &mut SimApp, x: u32| { let world = sim.app.world_mut(); let map = world.resource::<LoadedMap>(); let i = map.mobiles[&SectorId::new(1, 1)].iter().position(|o| o.object_type == TYPE_NPC && crate::object_location(o) == Some(TileLoc { x, y: 70 })).unwrap() as u32; *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == i).unwrap().0 };
            let (foe, kin) = (id_at(&mut sim, 72), id_at(&mut sim, 74));
            sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: foe, seq: 0 })]);
            sim.update();
            let world = sim.app.world_mut();
            let fighting = world.query::<(&NetId, Has<crate::combat::Fighting>)>().iter(world).find(|c| *c.0 == kin).unwrap().1;
            assert_eq!(fighting, joins, "reputation with faction 7: {reputed}");
        }
    }

    #[test]
    fn allies_of_the_victim_join_and_neutrals_flee() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let npc = |x: u32, y: u32, faction: i32, n: u8| {
            let mut b = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(x, y)).field("npc_faction", Value::Int32(faction));
            b.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [n; 16] };
            b.build()
        };
        std::fs::write(folder.join("G_0F.mob"), npc(72, 70, 7, 3)).unwrap();
        std::fs::write(folder.join("G_10.mob"), npc(74, 70, 7, 4)).unwrap();
        std::fs::write(folder.join("G_11.mob"), npc(72, 72, 0, 5)).unwrap();
        // A guard bystander (social class 5) stands; the fleeing neutral says a gd_npc 3000 line.
        let mut guard = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(70, 72)).field("npc_social_class", Value::Int32(crate::combat::SOCIAL_CLASS_GUARD));
        guard.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [6; 16] };
        std::fs::write(folder.join("G_12.mob"), guard.build()).unwrap();
        std::fs::create_dir_all(dir.path().join("mes")).unwrap();
        for file in ["gd_npc_m2m", "gd_npc_m2f", "gd_npc_f2m", "gd_npc_f2f"] {
            std::fs::write(dir.path().join("mes").join(format!("{file}.mes")), "{3000}{Run!}").unwrap();
        }
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let ids = {
            let world = sim.app.world_mut();
            let map = world.resource::<LoadedMap>();
            let index_at = |x: u32, y: u32| map.mobiles[&SectorId::new(1, 1)].iter().position(|o| o.object_type == TYPE_NPC && crate::object_location(o) == Some(TileLoc { x, y })).unwrap() as u32;
            let (foe, ally, neutral, guard) = (index_at(72, 70), index_at(74, 70), index_at(72, 72), index_at(70, 72));
            let id = |world: &mut World, i: u32| *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == i).unwrap().0;
            [id(world, foe), id(world, ally), id(world, neutral), id(world, guard)]
        };
        let [foe, ally, neutral, guard] = ids;
        let pc = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::PlayerControlled)>().iter(world).next().unwrap().0 };
        sim.take_deltas();
        sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: foe, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::FloatText { speaker, text, .. } if *speaker == neutral && text == "Run!")), "the neutral barks as it flees");
        let orders: Vec<Vec<NetId>> = deltas.into_iter().filter_map(|(_, d)| match d { LedgerDelta::Combat { order, .. } => Some(order), _ => None }).collect();
        assert_eq!(orders.last().unwrap(), &vec![pc, foe, ally], "the foe's faction joins; the neutral does not");
        let world = sim.app.world_mut();
        let mut q = world.query::<(&NetId, Option<&crate::combat::Fighting>, Has<crate::Walking>)>();
        let mut state = |id: NetId| q.iter(world).find(|c| *c.0 == id).map(|c| (c.1.map(|f| f.target), c.2)).unwrap();
        assert_eq!(state(ally).0, Some(pc));
        assert_eq!(state(neutral), (None, true), "the neutral walks away");
        assert_eq!(state(guard), (None, false), "the guard stands");
        let save = world.resource::<WorldState>().0.clone().unwrap();
        assert!(save.players[&PlayerId(1)].script_state.reactions.values().all(|&r| r <= 0), "the victim likes the PC no more than column 10 (0 with no rows)");
    }

    #[test]
    fn a_prowling_attacker_is_heard_less_by_bystanders() {
        use arcanum_formats::object::{Array, ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        for (prowl, flees) in [(false, true), (true, false)] {
            let dir = tempfile::tempdir().unwrap();
            write_map(dir.path());
            let folder = dir.path().join("maps").join(MAP);
            let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
            let mut foe = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70)).field("hp_pts", Value::Int32(500));
            foe.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
            std::fs::write(folder.join("G_0F.mob"), foe.build()).unwrap();
            // A neutral bystander of Perception 14, 6 tiles off, facing away: it hears 8 tiles,
            // 3 from a prowler of level 20 and Intelligence 20 (value 82 against PE 14).
            let mut stats = [0i32; 6];
            stats[5] = 14;
            let stats = Value::Array(Some(Array { elem_size: 4, id: 1, data: stats.iter().flat_map(|x| x.to_le_bytes()).collect(), bitset: vec![0x3f, 0] }));
            let mut neutral = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(77, 70)).field("critter_stat_base_idx", stats);
            neutral.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [4; 16] };
            std::fs::write(folder.join("G_10.mob"), neutral.build()).unwrap();
            let mut sim = SimApp::new(RulesData::default());
            sim.use_manual_clock();
            sim.app.insert_resource(vfs(dir.path()));
            let mut start = save(&[(1, "A", TileLoc { x: 71, y: 70 })]);
            let sheet = &mut start.players.get_mut(&PlayerId(1)).unwrap().blueprint.sheet;
            sheet.base = [20; arcanum_rules::PRIMARY_STATS];
            sheet.skills[crate::SKILL_PROWLING] = 5;
            sim.boot(start);
            sim.update();
            sim.update();
            let id_at = |sim: &mut SimApp, x: u32| { let world = sim.app.world_mut(); let map = world.resource::<LoadedMap>(); let i = map.mobiles[&SectorId::new(1, 1)].iter().position(|o| o.object_type == TYPE_NPC && crate::object_location(o) == Some(TileLoc { x, y: 70 })).unwrap() as u32; *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == i).unwrap().0 };
            let (foe, neutral) = (id_at(&mut sim, 72), id_at(&mut sim, 77));
            let away = crate::rotation_of_step(TileLoc { x: 77, y: 70 }, TileLoc { x: 78, y: 70 }).unwrap();
            { let world = sim.app.world_mut(); let e = world.query::<(Entity, &NetId)>().iter(world).find(|q| *q.1 == neutral).unwrap().0; world.entity_mut(e).insert(crate::Facing(away)); }
            if prowl {
                sim.receive([(PlayerId(1), crate::PlayerIntent::Prowl { on: true })]);
                sim.update();
            }
            let level = { let world = sim.app.world(); world.resource::<crate::SimRules>().0.skill_level(&world.resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(1)].blueprint.sheet, crate::SKILL_PROWLING) };
            // Level 20 against the bystander's PE 14: 72, + 10 for Intelligence 20 (0x4c6410).
            assert_eq!(level, 20, "Prowling level 20");
            sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: foe, seq: 0 })]);
            sim.update();
            let world = sim.app.world_mut();
            let walks = world.query::<(&NetId, Has<crate::Walking>)>().iter(world).find(|c| *c.0 == neutral).unwrap().1;
            assert_eq!(walks, flees, "prowling {prowl}");
        }
    }

    #[test]
    fn a_cornered_npc_that_would_flee_surrenders() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        std::fs::create_dir_all(dir.path().join("rules")).unwrap();
        std::fs::write(dir.path().join("rules").join("ai_params.mes"), "{0}{50 1000 1000 0 6 20 1001 1000 2 0 0 2001 100 50 20 1 1}").unwrap();
        // Its words (gd_npc 3000-3099), in each speaker file.
        std::fs::create_dir_all(dir.path().join("mes")).unwrap();
        for file in ["gd_npc_m2m", "gd_npc_m2f", "gd_npc_f2m", "gd_npc_f2f"] {
            std::fs::write(dir.path().join("mes").join(format!("{file}.mes")), "{3000}{I give up!}").unwrap();
        }
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        // 4 hit points (no stats), 2 lost: at 50 % it flees, west off the map: nowhere.
        let mut foe = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(64, 70)).field("hp_damage", Value::Int32(2));
        foe.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), foe.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 65, y: 70 })]));
        sim.update();
        sim.update();
        let npc = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::NpcSource)>().iter(world).next().unwrap().0 };
        let pc = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::PlayerControlled)>().iter(world).next().unwrap().0 };
        sim.app.world_mut().resource_mut::<crate::combat::CombatStarts>().0.push((npc, pc));
        sim.update();
        sim.receive([(PlayerId(1), crate::PlayerIntent::EndTurn)]);
        sim.update();
        sim.update();
        assert!(sim.take_deltas().iter().any(|(_, d)| matches!(d, LedgerDelta::FloatText { speaker, text, .. } if *speaker == npc && text == "I give up!")), "it barks (0x413ea0)");
        let world = sim.app.world_mut();
        let (fighting, surrendered) = world.query::<(&NetId, Has<crate::combat::Fighting>, Option<&crate::combat::Surrendered>)>().iter(world).find(|c| *c.0 == npc).map(|c| (c.1, c.2.copied())).unwrap();
        assert_eq!((fighting, surrendered), (false, Some(crate::combat::Surrendered { to: pc })));
        assert!(world.resource::<crate::combat::Combat>().0.is_none(), "no one left fighting: the fight is over");
    }

    #[test]
    fn repair_answers_by_the_items_wear_and_the_chance() {
        use arcanum_formats::object::{Object, TYPE_WEAPON, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        use crate::{CarriedItem, Inventory, InventoryRequest, ItemWear};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut sword = Object::parse_instance(&ObjectBuilder::new(TYPE_WEAPON).field("item_weight", Value::Int32(1)).field("hp_pts", Value::Int32(40)).build()).unwrap();
        sword.id = prototype_id(6049);
        let mut prototypes = Prototypes::default();
        prototypes.insert(sword);
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        let mut start = save(&[(1, "A", TileLoc { x: 71, y: 70 })]);
        let item = |location, damage| CarriedItem { prototype: 6049, location, quantity: 0, keys: vec![], written: 0, wear: ItemWear { damage, lost: 0, broken: false } };
        start.players.get_mut(&PlayerId(1)).unwrap().inventory = Some(Inventory { items: vec![item(0, 20), item(5, 0)], ..Default::default() });
        sim.boot(start);
        sim.update();
        sim.update();
        sim.take_deltas();
        let answer = |sim: &mut SimApp, seq, from| {
            sim.receive([(PlayerId(1), crate::PlayerIntent::Inventory { seq, request: InventoryRequest::Repair { from } })]);
            sim.update();
            sim.take_deltas().into_iter().find_map(|(_, d)| match d { LedgerDelta::SkillResult { message } => Some(message), _ => None })
        };
        assert_eq!(answer(&mut sim, 1, 5), Some(crate::repair::NOT_DAMAGED));
        assert_eq!(answer(&mut sim, 2, 0), Some(crate::repair::REPAIR_FAILED), "no skill: no chance");
        let world = sim.app.world_mut();
        let inv = world.query_filtered::<&crate::Inventory, With<crate::PlayerControlled>>().iter(world).next().unwrap().clone();
        let wear = inv.items.iter().find(|i| i.location == 0).unwrap().wear;
        assert_eq!(wear.damage, 20, "unchanged");
        assert_eq!(wear.lost, 4, "the most falls 10 % all the same (0x4c7daa)");
    }

    #[test]
    fn a_broken_weapon_cannot_be_wielded_and_its_destroy_script_can_keep_it() {
        use crate::{CarriedItem, Inventory, InventoryRequest, ItemWear};
        use crate::script::tests::{always, block};
        use arcanum_formats::object::{Array, Object, TYPE_WEAPON, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        // A sword whose destroy (2) runs script 730.
        let mut data = Vec::new();
        data.extend([0u32, 0, 730].iter().flat_map(|v| v.to_le_bytes()));
        let scripts = Array { elem_size: 12, id: 1, data, bitset: vec![1 << 2, 0] };
        let mut sword = Object::parse_instance(&ObjectBuilder::new(TYPE_WEAPON).field("item_weight", Value::Int32(1)).field("hp_pts", Value::Int32(40)).field("scripts_idx", Value::Array(Some(scripts))).build()).unwrap();
        sword.id = prototype_id(6049);
        let mut prototypes = Prototypes::default();
        prototypes.insert(sword);
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        let mut start = save(&[(1, "A", TileLoc { x: 71, y: 70 })]);
        let broken = ItemWear { broken: true, ..Default::default() };
        let item = |location| CarriedItem { prototype: 6049, location, quantity: 0, keys: vec![], written: 0, wear: broken };
        start.players.get_mut(&PlayerId(1)).unwrap().inventory = Some(Inventory { items: vec![item(0)], ..Default::default() });
        sim.boot(start);
        sim.update();
        sim.update();
        sim.take_deltas();
        sim.receive([(PlayerId(1), crate::PlayerIntent::Inventory { seq: 1, request: InventoryRequest::Move { from: 0, to: crate::combat::WEAPON_SLOT } })]);
        sim.update();
        let carried = |sim: &SimApp| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(1)].inventory.clone().unwrap().items;
        assert_eq!(carried(&sim)[0].location, 0, "not wielded (0x464d9f)");
        assert_eq!(crate::ItemRefusal::Broken.message(), Some(107));
        // Its destroy script answers "skip": kept (0x43cd18); then "run": gone.
        for (answer, kept) in [(1, true), (2, false)] {
            sim.app.world_mut().resource_mut::<crate::Scripts>().0.get_or_insert_with(Default::default).insert(730, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(answer, &[]))] });
            let d = crate::combat::ItemDestroy { item: crate::ScriptObject::Carried { holder: PlayerId(1), prototype: 6049 }, prototype: 6049, location: 0, player: Some(PlayerId(1)) };
            sim.app.world_mut().resource_mut::<crate::combat::CombatEvents>().2.push(d);
            sim.update();
            assert_eq!(!carried(&sim).is_empty(), kept, "answer {answer}");
        }
    }

    #[test]
    fn a_pc_out_of_hit_points_stays_dead_until_resurrected() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let sheet = |sim: &SimApp| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(1)].blueprint.sheet.clone();
        let max = sim.app.world().resource::<crate::SimRules>().0.derived(&sheet(&sim)).hit_points;
        let set = |sim: &mut SimApp, f: &dyn Fn(&mut CharacterSheet)| {
            f(&mut sim.app.world_mut().resource_mut::<WorldState>().0.as_mut().unwrap().players.get_mut(&PlayerId(1)).unwrap().blueprint.sheet);
            sim.app.world_mut().resource_mut::<crate::ScriptSheets>().0.insert(PlayerId(1));
        };
        set(&mut sim, &|s| s.hp_damage = max);
        sim.update();
        sim.update();
        assert!(sheet(&sim).dead, "out of hit points: dead");
        // Healing does not raise the dead; neither does a talk or a walk get through.
        set(&mut sim, &|s| s.hp_damage = 0);
        sim.update();
        sim.update();
        assert!(sheet(&sim).dead);
        let world = sim.app.world_mut();
        assert!(world.query_filtered::<(), (With<crate::PlayerControlled>, With<crate::Unconscious>)>().iter(world).next().is_some(), "lies down");
        // A blessing (cause 3), an item's effect (5) and one of cause 2 held while dead.
        set(&mut sim, &|s| {
            s.add_effect(10, 3);
            s.add_effect(11, 5);
            s.add_effect(12, 2);
            s.poison = 40;
        });
        sim.app.world_mut().resource_mut::<crate::Resurrections>().0 = 1;
        sim.update();
        sim.update();
        let s = sheet(&sim);
        assert!(!s.dead);
        assert_eq!(max - s.hp_damage, crate::resurrected_hp(max).min(max));
        assert_eq!(s.effects, [arcanum_rules::HeldEffect { id: 12, cause: 2 }], "causes 3-9 cleared (0x4a60b5)");
        assert_eq!(s.poison, 0, "poison cleared (owner's call)");
        assert_eq!((crate::resurrected_hp(40), crate::resurrected_hp(5)), (6, 1), "15 %, at least 1");
    }

    #[test]
    fn a_taking_damage_script_that_says_no_keeps_the_npc_whole() {
        use crate::script::tests::{always, block};
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut foe = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70));
        foe.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), foe.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let sector = SectorId::new(1, 1);
        let (npc, index) = {
            let world = sim.app.world_mut();
            let map = world.resource::<LoadedMap>();
            let index = map.mobiles[&sector].iter().position(|o| o.object_type == TYPE_NPC).unwrap() as u32;
            (*world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == index).unwrap().0, index)
        };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(crate::ScriptObject::Mobile { sector, index }, crate::combat::EVENT_TAKING_DAMAGE, crate::Attachment::new(0, 0, 707));
        sim.app.world_mut().resource_mut::<crate::Scripts>().0.get_or_insert_with(Default::default).insert(707, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(1, &[]))] });
        sim.take_deltas();
        let mut hits = 0;
        for seq in 0..60 {
            sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: npc, seq })]);
            sim.update();
            hits += sim.take_deltas().iter().filter(|(_, d)| matches!(d, LedgerDelta::Attacked { attacker, outcome, .. } if *attacker != npc && outcome.hit && outcome.hp > 0)).count();
        }
        assert!(hits > 0, "the PC lands hits");
        let map = sim.app.world().resource::<LoadedMap>();
        assert!(!map.is_dead(sector, index));
        assert_eq!(map.npc_hp(sector, index).map(|(left, max)| left == max), Some(true), "no damage taken");
    }

    #[test]
    fn a_fighting_npc_runs_its_start_combat_script_each_turn() {
        use crate::script::tests::{always, block, n, op};
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut foe = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70)).field("hp_pts", Value::Int32(500));
        foe.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), foe.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let sector = SectorId::new(1, 1);
        let (npc, index) = {
            let world = sim.app.world_mut();
            let map = world.resource::<LoadedMap>();
            let index = map.mobiles[&sector].iter().position(|o| o.object_type == TYPE_NPC).unwrap() as u32;
            (*world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == index).unwrap().0, index)
        };
        // Its script 15 counts in global variable 1.
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(crate::ScriptObject::Mobile { sector, index }, crate::combat::EVENT_START_COMBAT, crate::Attachment::new(0, 0, 708));
        let count = always(block(0xb, &[op(1, 1), op(1, 1), n(1)]));
        sim.app.world_mut().resource_mut::<crate::Scripts>().0.get_or_insert_with(Default::default).insert(708, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![count] });
        let var = |sim: &SimApp| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().scripts.var(1);
        assert_eq!(var(&sim), 0, "not before the fight");
        let mut npc_turns = 0;
        for seq in 0..40 {
            sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: npc, seq })]);
            sim.update();
            npc_turns += sim.take_deltas().iter().filter(|(_, d)| matches!(d, LedgerDelta::Attacked { attacker, .. } if *attacker == npc)).count();
        }
        assert!(npc_turns > 0, "the NPC fights back");
        assert!(var(&sim) > 0, "once a turn of its own");
    }

    #[test]
    fn an_npcs_weapon_runs_its_hit_and_miss_scripts() {
        use crate::script::tests::{always, block, n, op};
        use arcanum_formats::object::{Array, Object, ObjectId, TYPE_WEAPON, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut foe = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70)).field("hp_pts", Value::Int32(500));
        foe.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), foe.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        // A club whose hit (7) and miss (8) run script 720.
        let mut data = Vec::new();
        for v in [0u32, 0, 720, 0, 0, 720] {
            data.extend(v.to_le_bytes());
        }
        let scripts = Array { elem_size: 12, id: 1, data, bitset: vec![1 << 7 | 1 << 8, 0] };
        let mut club = Object::parse_instance(&ObjectBuilder::new(TYPE_WEAPON).field("item_weight", Value::Int32(1)).field("weapon_ammo_type", Value::Int32(crate::combat::NO_AMMO)).field("weapon_speed_factor", Value::Int32(15)).field("scripts_idx", Value::Array(Some(scripts))).build()).unwrap();
        club.id = prototype_id(8001);
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let sector = SectorId::new(1, 1);
        let (npc, index) = {
            let world = sim.app.world_mut();
            let map = world.resource::<LoadedMap>();
            let index = map.mobiles[&sector].iter().position(|o| o.object_type == TYPE_NPC).unwrap() as u32;
            (*world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == index).unwrap().0, index)
        };
        {
            let mut map = sim.app.world_mut().resource_mut::<LoadedMap>();
            let mut prototypes = (*map.prototypes).clone();
            prototypes.insert(club);
            map.prototypes = Arc::new(prototypes);
            let mut carried = map.npc_inventory(sector, index).unwrap_or_default();
            carried.items.push(crate::CarriedItem { prototype: 8001, location: crate::combat::WEAPON_SLOT, quantity: 0, keys: vec![], written: 0, wear: Default::default() });
            map.set_npc_inventory(sector, index, carried);
        }
        // Script 720 counts in global variable 1.
        let count = always(block(0xb, &[op(1, 1), op(1, 1), n(1)]));
        sim.app.world_mut().resource_mut::<crate::Scripts>().0.get_or_insert_with(Default::default).insert(720, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![count] });
        let var = |sim: &SimApp| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().scripts.var(1);
        let mut npc_blows = 0;
        for seq in 0..40 {
            sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: npc, seq })]);
            sim.update();
            npc_blows += sim.take_deltas().iter().filter(|(_, d)| matches!(d, LedgerDelta::Attacked { attacker, .. } if *attacker == npc)).count();
        }
        assert!(npc_blows > 0, "the NPC fights back");
        assert!(var(&sim) >= npc_blows as i32, "each blow runs hit or miss (0x4b3770: no PC test)");
    }

    #[test]
    fn a_pc_raises_another_and_heals_an_npc_by_spell() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut npc = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(74, 70)).field("hp_damage", Value::Int32(2));
        npc.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), npc.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut start = save(&[(1, "Healer", TileLoc { x: 71, y: 70 }), (2, "Fallen", TileLoc { x: 72, y: 70 })]);
        let healer = &mut start.players.get_mut(&PlayerId(1)).unwrap().blueprint.sheet;
        healer.base = [20; arcanum_rules::PRIMARY_STATS];
        healer.spells[crate::RESURRECT as usize / arcanum_rules::SPELLS_PER_COLLEGE] = 5;
        let fallen = &mut start.players.get_mut(&PlayerId(2)).unwrap().blueprint.sheet;
        fallen.dead = true;
        fallen.hp_damage = 1000;
        sim.boot(start);
        sim.update();
        sim.update();
        let id = |sim: &mut SimApp, p: u64| { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::PlayerControlled)>().iter(world).find(|c| c.1.0 == PlayerId(p)).unwrap().0 };
        let fallen_id = id(&mut sim, 2);
        sim.receive([(PlayerId(1), crate::PlayerIntent::CastOn { spell: crate::RESURRECT as usize, target: fallen_id })]);
        sim.update();
        sim.update();
        let sheet = |sim: &SimApp, p: u64| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(p)].blueprint.sheet.clone();
        let raised = sheet(&sim, 2);
        let max = sim.app.world().resource::<crate::SimRules>().0.derived(&raised).hit_points;
        assert!(!raised.dead, "raised");
        assert_eq!(max - raised.hp_damage, crate::resurrected_hp(max));
        let cost = sim.app.world().resource::<crate::SimRules>().0.spells.spells.get(crate::RESURRECT as usize).map_or(0, |s| s.cost);
        assert_eq!(sheet(&sim, 1).fatigue_damage, cost, "paid in fatigue (50 with the game's list)");
        // Minor Healing on the hurt NPC.
        let npc_id = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::NpcSource)>().iter(world).next().unwrap().0 };
        sim.receive([(PlayerId(1), crate::PlayerIntent::CastOn { spell: crate::MINOR_HEALING as usize, target: npc_id })]);
        sim.update();
        let map = sim.app.world().resource::<LoadedMap>();
        let (left, max) = map.npc_hp(SectorId::new(1, 1), 0).unwrap();
        assert_eq!(left, max, "healed whole");
        // Resurrect on the NPC once dead: 15 % too (owner's call).
        sim.app.world_mut().resource_mut::<LoadedMap>().set_npc_poison(SectorId::new(1, 1), 0, 40);
        sim.app.world_mut().resource_mut::<LoadedMap>().kill_npc(None, SectorId::new(1, 1), 0, 7);
        let rest = |sim: &mut SimApp| {
            let world = sim.app.world_mut();
            world.query_filtered::<&mut crate::Sheet, With<crate::PlayerControlled>>().iter_mut(world).for_each(|mut s| s.0.fatigue_damage = 0);
        };
        let cast = |sim: &mut SimApp| {
            sim.receive([(PlayerId(1), crate::PlayerIntent::CastOn { spell: crate::RESURRECT as usize, target: npc_id })]);
            sim.update();
        };
        // Its resurrect script (18) answers "skip": refused (0x4b5a4e).
        let npc = crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc, crate::magick::EVENT_RESURRECT, crate::Attachment::new(0, 0, 740));
        sim.app.world_mut().resource_mut::<crate::Scripts>().0.get_or_insert_with(Default::default).insert(740, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![crate::script::tests::always(crate::script::tests::block(1, &[]))] });
        rest(&mut sim);
        cast(&mut sim);
        assert!(sim.app.world().resource::<LoadedMap>().is_dead(SectorId::new(1, 1), 0), "its script refused");
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc, crate::magick::EVENT_RESURRECT, crate::Attachment::default());
        // Unresurrectable (`critter_flags` 0x1000000, 0x4b5a19): refused.
        assert!(sim.app.world_mut().resource_mut::<LoadedMap>().mobiles.get_mut(&SectorId::new(1, 1)).unwrap()[0].set_int("critter_flags", crate::magick::OCF_UNRESURRECTABLE));
        rest(&mut sim);
        cast(&mut sim);
        assert!(sim.app.world().resource::<LoadedMap>().is_dead(SectorId::new(1, 1), 0), "unresurrectable");
        assert!(sim.app.world_mut().resource_mut::<LoadedMap>().mobiles.get_mut(&SectorId::new(1, 1)).unwrap()[0].set_int("critter_flags", 0));
        let stored = |sim: &SimApp| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(1)].script_state.reactions.get(&crate::met_key(npc)).copied();
        sim.app.world_mut().resource_mut::<WorldState>().0.as_mut().unwrap().players.get_mut(&PlayerId(1)).unwrap().script_state.reactions.insert(crate::met_key(npc), -60);
        rest(&mut sim);
        cast(&mut sim);
        let map = sim.app.world().resource::<LoadedMap>();
        let (left, max) = map.npc_hp(SectorId::new(1, 1), 0).unwrap();
        assert!(!map.is_dead(SectorId::new(1, 1), 0), "raised");
        assert_eq!(left, crate::resurrected_hp(max));
        assert_eq!(map.npc_poison(SectorId::new(1, 1), 0), 0, "poison cleared");
        let (tired, most) = map.npc_fatigue(SectorId::new(1, 1), 0).unwrap();
        assert_eq!(most - tired, crate::magick::RAISED_FATIGUE_DAMAGE, "fatigue damage 10 (0x4b5b28)");
        assert_eq!(stored(&sim), Some(-30), "its reaction to the healer (40: 100 more with Charisma 20) raised to 70 (0x4b5b9e)");
    }

    #[test]
    fn a_player_who_joins_later_learns_the_hurt_npcs_health() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let mut npc = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(74, 70)).field("hp_damage", Value::Int32(2));
        npc.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), npc.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        let told = |sim: &mut SimApp, p: u64| (0..crate::combat::HEALTH_TICKS).any(|_| { sim.update(); sim.take_deltas().iter().any(|(to, d)| *to == PlayerId(p) && matches!(d, LedgerDelta::NpcHealth { .. })) });
        assert!(told(&mut sim, 1), "the hurt NPC's health at first sight");
        assert!(!told(&mut sim, 1), "once");
        sim.app.world_mut().resource_mut::<crate::Session>().players.insert(PlayerId(2), crate::SessionPlayer::InWorld(NetId(999)));
        assert!(told(&mut sim, 2), "the newcomer hears of it");
        assert!(!told(&mut sim, 1), "the others not again");
    }

    #[test]
    fn fists_knock_an_npc_out_and_it_gets_up_later() {
        use arcanum_formats::object::{ObjectId, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        // Hard to kill (60 hit points), easy to tire (4 fatigue: no stats).
        let mut foe = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70)).field("hp_pts", Value::Int32(60));
        foe.id = ObjectId { kind: ObjectId::KIND_GUID, payload: [3; 16] };
        std::fs::write(folder.join("G_0F.mob"), foe.build()).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let sector = SectorId::new(1, 1);
        let npc = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::NpcSource)>().iter(world).next().unwrap().0 };
        let mut ended = false;
        let mut told = None;
        for seq in 0..200 {
            sim.receive([(PlayerId(1), crate::PlayerIntent::Attack { target: npc, seq })]);
            sim.update();
            let deltas = sim.take_deltas();
            told = deltas.iter().rev().find_map(|(_, d)| match d { LedgerDelta::NpcHealth { net_id, fatigue, .. } if *net_id == npc => Some(*fatigue), _ => None }).or(told);
            if deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::Combat { order, .. } if order.is_empty())) {
                ended = true;
                break;
            }
        }
        assert!(ended, "the fight ends with the NPC down");
        for _ in 0..crate::combat::HEALTH_TICKS {
            sim.update();
            told = sim.take_deltas().iter().rev().find_map(|(_, d)| match d { LedgerDelta::NpcHealth { net_id, fatigue, .. } if *net_id == npc => Some(*fatigue), _ => None }).or(told);
        }
        assert!(told.is_some_and(|f| f.current < f.max), "players hear of its fatigue: {told:?}");
        let map = sim.app.world().resource::<LoadedMap>();
        assert!(map.npc_down(sector, 0) && !map.is_dead(sector, 0), "knocked out, not dead");
        assert_eq!(map.npc_fatigue(sector, 0).map(|(left, _)| left < 1), Some(true));
        // Game time passes: its fatigue comes back and it gets up.
        for _ in 0..20 {
            sim.app.world_mut().resource_mut::<crate::GameClock>().now.ms += crate::RECOVERY_MS as u32;
            sim.update();
        }
        assert!(!sim.app.world().resource::<LoadedMap>().npc_down(sector, 0), "up again");
    }

    #[test]
    fn prowling_switches_the_pcs_art_and_everyone_hears_of_it() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        sim.take_deltas();
        let anim = |sim: &mut SimApp| {
            let world = sim.app.world_mut();
            let (look, prowling) = world.query_filtered::<(&crate::Appearance, Has<crate::Prowling>), With<crate::PlayerControlled>>().iter(world).next().map(|(a, p)| (*a, p)).unwrap();
            (arcanum_formats::art_id::CritterArt::from_id(look.art).anim, prowling)
        };
        sim.receive([(PlayerId(1), crate::PlayerIntent::Prowl { on: true })]);
        sim.update();
        assert_eq!(anim(&mut sim), (crate::prowl::ANIM_PROWL, true));
        let net_id = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::PlayerControlled)>().iter(world).next().unwrap().0 };
        assert!(sim.take_deltas().contains(&(PlayerId(1), LedgerDelta::Prowling { net_id, on: true })));
        sim.receive([(PlayerId(1), crate::PlayerIntent::Prowl { on: false })]);
        sim.update();
        assert_eq!(anim(&mut sim), (arcanum_formats::art_id::ANIM_STAND, false));
    }

    #[test]
    fn a_thief_opens_an_npcs_pockets_and_steals_or_fails() {
        use arcanum_formats::object::{Array, Object, ObjectId, TYPE_GOLD, TYPE_WEAPON, Value};
        use arcanum_formats::test_util::{ObjectBuilder, object_id_bytes, prototype_id};
        use crate::InventoryRequest;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let guid = |n: u8| ObjectId { kind: ObjectId::KIND_GUID, payload: [n; 16] };
        let mut dagger = ObjectBuilder::new(TYPE_WEAPON).field("flags", Value::Int32(crate::OF_INVENTORY)).field("item_inv_location", Value::Int32(3));
        dagger.prototype = prototype_id(6049);
        dagger.id = guid(9);
        std::fs::write(folder.join("G_D.mob"), dagger.build()).unwrap();
        let data = object_id_bytes(&guid(9)).to_vec();
        let mark = ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).field("location", at(72, 70)).field("critter_inventory_list_idx", Value::Array(Some(Array { elem_size: 24, id: 0, data, bitset: vec![] })));
        std::fs::write(folder.join("G_0S.mob"), mark.build()).unwrap();
        let proto = |number: u32, kind: u32| {
            let mut o = Object::parse_instance(&ObjectBuilder::new(kind).field("item_weight", Value::Int32(1)).field("item_worth", Value::Int32(10)).build()).unwrap();
            o.id = prototype_id(number);
            o
        };
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut prototypes = Prototypes::default();
        prototypes.insert(proto(crate::GOLD, TYPE_GOLD));
        prototypes.insert(proto(6049, TYPE_WEAPON));
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        sim.boot(save(&[(1, "A", TileLoc { x: 71, y: 70 })]));
        sim.update();
        sim.update();
        let (npc, index) = {
            let world = sim.app.world_mut();
            let map = world.resource::<LoadedMap>();
            let index = map.mobiles[&SectorId::new(1, 1)].iter().position(|o| o.object_type == TYPE_NPC && crate::object_location(o) == Some(TileLoc { x: 72, y: 70 })).unwrap() as u32;
            let npc = *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == index).unwrap().0;
            (npc, index)
        };
        sim.take_deltas();
        sim.receive([(PlayerId(1), crate::PlayerIntent::PickPocket { npc, seq: 0 })]);
        sim.update();
        sim.update();
        let deltas = sim.take_deltas();
        let view = deltas.iter().find_map(|(_, d)| match d { LedgerDelta::Barter { view } => Some(view.clone()), _ => None }).expect("the window opens");
        assert!(view.pickpocket && view.buy.is_empty(), "no prices");
        assert_eq!(view.stock.items.iter().map(|i| i.location).collect::<Vec<_>>(), [3]);
        // Putting an item in is refused; taking rolls.
        let mut stolen = false;
        for seq in 1..40 {
            sim.receive([(PlayerId(1), crate::PlayerIntent::Inventory { seq, request: InventoryRequest::Buy { from: 3, to: None } })]);
            sim.update();
            let deltas = sim.take_deltas();
            let result = deltas.iter().find_map(|(_, d)| match d { LedgerDelta::SkillResult { message } => Some(*message), _ => None });
            if deltas.iter().any(|(_, d)| *d == LedgerDelta::BarterClosed) {
                assert_eq!(result, Some(crate::pickpocket::STEAL_FAILED), "caught on a failure");
                sim.receive([(PlayerId(1), crate::PlayerIntent::PickPocket { npc, seq: 100 + seq })]);
                sim.update();
                sim.update();
                sim.take_deltas();
                continue;
            }
            if result == Some(crate::pickpocket::STEAL_SUCCEEDED) {
                let world = sim.app.world_mut();
                let inventory = world.query::<(&crate::PlayerControlled, &crate::Inventory)>().iter(world).next().unwrap().1.clone();
                assert!(inventory.items.iter().any(|i| i.prototype == 6049), "the dagger is the PC's");
                assert!(sim.app.world().resource::<LoadedMap>().npc_inventory(SectorId::new(1, 1), index).unwrap().items.is_empty());
                stolen = true;
                break;
            }
            assert_eq!(result, Some(crate::pickpocket::STEAL_FAILED));
        }
        assert!(stolen, "an even chance of 47 % comes up in 39 tries");
        // A target flagged against it refuses.
        sim.app.world_mut().resource_mut::<LoadedMap>().mobiles.get_mut(&SectorId::new(1, 1)).unwrap()[index as usize].set_int("critter_flags2", crate::pickpocket::OCF2_NO_PICKPOCKET);
        sim.receive([(PlayerId(1), crate::PlayerIntent::Inventory { seq: 200, request: InventoryRequest::CloseBarter })]);
        sim.receive([(PlayerId(1), crate::PlayerIntent::PickPocket { npc, seq: 300 })]);
        sim.update();
        sim.update();
        assert!(sim.take_deltas().contains(&(PlayerId(1), LedgerDelta::SkillResult { message: crate::CANNOT_STEAL })));
    }

    #[test]
    fn barter_opens_from_the_dialogue_and_trades_for_gold() {
        use arcanum_formats::object::{Array, Object, ObjectId, TYPE_GOLD, TYPE_WEAPON, Value};
        use arcanum_formats::test_util::{ObjectBuilder, object_id_bytes, prototype_id};
        use crate::InventoryRequest;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let guid = |n: u8| ObjectId { kind: ObjectId::KIND_GUID, payload: [n; 16] };
        // A shopkeeper at (72, 70) with a dagger at grid location 3.
        let mut dagger = ObjectBuilder::new(TYPE_WEAPON).field("flags", Value::Int32(crate::OF_INVENTORY)).field("item_inv_location", Value::Int32(3));
        dagger.prototype = prototype_id(6049);
        dagger.id = guid(9);
        std::fs::write(folder.join("G_D.mob"), dagger.build()).unwrap();
        let data = object_id_bytes(&guid(9)).to_vec();
        let shop = ObjectBuilder::new(TYPE_NPC)
            .field("current_aid", Value::Int32(0x2800_2000))
            .field("location", at(72, 70))
            .field("npc_retail_price_multiplier", Value::Int32(100))
            .field("critter_inventory_list_idx", Value::Array(Some(Array { elem_size: 24, id: 0, data, bitset: vec![] })));
        std::fs::write(folder.join("G_0S.mob"), shop.build()).unwrap();
        std::fs::create_dir_all(dir.path().join("mes")).unwrap();
        std::fs::write(dir.path().join("mes/gd_pc2m.mes"), "{300}{Let us trade.}{400}{Bye.}").unwrap();
        std::fs::write(dir.path().join("mes/gd_cls_m2m.mes"), "{2000}{You do not have enough coins.}").unwrap();
        let proto = |number: u32, kind: u32, worth: i32| {
            let mut o = Object::parse_instance(&ObjectBuilder::new(kind).field("item_weight", Value::Int32(1)).field("item_worth", Value::Int32(worth)).build()).unwrap();
            o.id = prototype_id(number);
            o
        };
        let mut s = save(&[(1, "A", TileLoc { x: 71, y: 70 })]);
        let mut gold = crate::Inventory::default();
        gold.change_gold(500);
        s.players.get_mut(&PlayerId(1)).unwrap().inventory = Some(gold);
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut prototypes = Prototypes::default();
        prototypes.insert(proto(crate::GOLD, TYPE_GOLD, 1));
        prototypes.insert(proto(6049, TYPE_WEAPON, 100));
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        sim.boot(s);
        sim.update();
        sim.update();
        let (npc, npc_object) = {
            let world = sim.app.world_mut();
            let map = world.resource::<LoadedMap>();
            let index = map.mobiles[&SectorId::new(1, 1)].iter().position(|o| o.object_type == TYPE_NPC && crate::object_location(o) == Some(TileLoc { x: 72, y: 70 })).unwrap() as u32;
            let npc = *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == index).unwrap().0;
            (npc, crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index })
        };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 700));
        {
            use crate::script::tests::{always, block, n};
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            library.insert(700, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(4, &[n(1)]))] });
            library.insert_dialog(700, arcanum_formats::dialog::DialogFile::parse(b"{1}{Want to trade?}{}{}{}{}{}\n{2}{B:}{}{1}{}{0}{gf 1 1}"));
        }
        sim.take_deltas();
        sim.receive([(PlayerId(1), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.contains(&(PlayerId(1), LedgerDelta::DialogueOptions { npc, options: vec!["Let us trade.".into()] })), "{deltas:?}");
        sim.receive([(PlayerId(1), crate::PlayerIntent::Choose { option: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.contains(&(PlayerId(1), LedgerDelta::DialogueEnded)));
        let view = deltas.iter().find_map(|(_, d)| match d { LedgerDelta::Barter { view } => Some(view.clone()), _ => None }).expect("the window opens");
        assert_eq!(view.stock.items.iter().map(|i| i.location).collect::<Vec<_>>(), [3]);
        let price = view.buy[&3].price;
        assert!(price > 100, "more than its worth: {price}");
        assert!(!sim.app.world().resource::<WorldState>().0.as_ref().unwrap().scripts.flag(1), "B: runs no result");
        // Buy the dagger.
        sim.receive([(PlayerId(1), crate::PlayerIntent::Inventory { seq: 1, request: InventoryRequest::Buy { from: 3, to: None } })]);
        sim.update();
        let deltas = sim.take_deltas();
        let mine = deltas.iter().rev().find_map(|(_, d)| match d { LedgerDelta::Inventory { inventory, ack: 1, .. } => Some(inventory.clone()), _ => None }).unwrap();
        assert_eq!(mine.gold(), 500 - price);
        let dagger_at = mine.items.iter().find(|i| i.prototype == 6049).expect("bought").location;
        let view = deltas.iter().rev().find_map(|(_, d)| match d { LedgerDelta::Barter { view } => Some(view.clone()), _ => None }).unwrap();
        assert_eq!(view.npc_gold, price);
        assert!(view.stock.items.iter().all(|i| i.prototype != 6049));
        let pays = view.sell[&dagger_at].price;
        assert_eq!(pays, crate::barter::sell_price(100, crate::barter::haggle(0, 8), 100, 1, 1));
        // Sell it back.
        sim.receive([(PlayerId(1), crate::PlayerIntent::Inventory { seq: 2, request: InventoryRequest::Sell { from: dagger_at, to: Some(5) } })]);
        sim.update();
        let deltas = sim.take_deltas();
        let mine = deltas.iter().rev().find_map(|(_, d)| match d { LedgerDelta::Inventory { inventory, ack: 2, .. } => Some(inventory.clone()), _ => None }).unwrap();
        assert_eq!(mine.gold(), 500 - price + pays);
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        let shop = &save.sector_changes[&SectorId::new(1, 1)].npc_inventories[&npc_object_index(npc_object)];
        assert!(shop.items.iter().any(|i| i.prototype == 6049 && i.location == 5), "back on the shelf, saved");
        assert_eq!(shop.gold(), price - pays);
        // Too poor: the class line (gd_cls 2000, 0x413910), and nothing moves.
        {
            let world = sim.app.world_mut();
            let mut pc = world.query_filtered::<&mut crate::Inventory, With<PlayerControlled>>().single_mut(world).unwrap();
            let gold = pc.gold();
            pc.change_gold(10 - gold);
        }
        sim.receive([(PlayerId(1), crate::PlayerIntent::Inventory { seq: 3, request: InventoryRequest::Buy { from: 5, to: None } })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.contains(&(PlayerId(1), LedgerDelta::BarterLine { text: "You do not have enough coins.".into() })), "{deltas:?}");
        let mine = deltas.iter().rev().find_map(|(_, d)| match d { LedgerDelta::Inventory { inventory, ack: 3, .. } => Some(inventory.clone()), _ => None }).unwrap();
        assert_eq!(mine.gold(), 10);
        sim.receive([(PlayerId(1), crate::PlayerIntent::Inventory { seq: 4, request: InventoryRequest::CloseBarter })]);
        sim.update();
        assert!(sim.app.world_mut().query::<&crate::barter::BarterOpen>().iter(sim.app.world()).next().is_none(), "closed");
    }

    fn npc_object_index(object: crate::ScriptObject) -> u32 {
        match object {
            crate::ScriptObject::Mobile { index, .. } => index,
            _ => unreachable!(),
        }
    }

    #[test]
    fn containers_open_through_the_host() {
        use arcanum_formats::object::{Array, Object, ObjectId, TYPE_CONTAINER, TYPE_GOLD, TYPE_WEAPON, Value};
        use arcanum_formats::test_util::{ObjectBuilder, object_id_bytes, prototype_id};
        use crate::{GroundItemId, InventoryRequest};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let guid = |n: u8| ObjectId { kind: ObjectId::KIND_GUID, payload: [n; 16] };
        // A dagger stored in the first chest at grid location 3.
        let mut dagger = ObjectBuilder::new(TYPE_WEAPON).field("flags", Value::Int32(crate::OF_INVENTORY)).field("item_inv_location", Value::Int32(3));
        dagger.prototype = prototype_id(6049);
        dagger.id = guid(9);
        std::fs::write(folder.join("G_D.mob"), dagger.build()).unwrap();
        let chest = |name: &str, x: u32, flags: i32, list: &[ObjectId], source: i32| {
            let data = list.iter().flat_map(object_id_bytes).collect();
            let mut b = ObjectBuilder::new(TYPE_CONTAINER)
                .field("current_aid", Value::Int32(0x7000_0000))
                .field("location", at(x, 70))
                .field("container_flags", Value::Int32(flags))
                .field("container_inventory_list_idx", Value::Array(Some(Array { elem_size: 24, id: 0, data, bitset: vec![] })))
                .field("container_inventory_source", Value::Int32(source));
            b.id = guid(x as u8);
            std::fs::write(folder.join(format!("G_{name}.mob")), b.build()).unwrap();
        };
        chest("A", 71, 0, &[guid(9)], 0);
        chest("B", 72, crate::OCOF_LOCKED, &[], 0);
        chest("C", 69, crate::OCOF_INVEN_SPAWN_PENDING, &[], 1);
        std::fs::create_dir_all(dir.path().join("Rules")).unwrap();
        std::fs::write(dir.path().join("Rules/InvenSource.mes"), "{1}{Test: 5,5 100,6029 0,6029}\n").unwrap();
        let proto = |number: u32, kind: u32| {
            let mut o = Object::parse_instance(&ObjectBuilder::new(kind).field("item_weight", Value::Int32(10)).build()).unwrap();
            o.id = prototype_id(number);
            o
        };
        let mut s = save(&[(1, "A", TileLoc { x: 70, y: 70 }), (2, "B", TileLoc { x: 70, y: 72 })]);
        s.players.get_mut(&PlayerId(1)).unwrap().blueprint.sheet.base[0] = 8;
        s.players.get_mut(&PlayerId(1)).unwrap().inventory = Some(crate::Inventory::default());
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut prototypes = Prototypes::default();
        prototypes.insert(proto(crate::GOLD, TYPE_GOLD));
        prototypes.insert(proto(6049, TYPE_WEAPON));
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        sim.boot(s);
        sim.update();
        sim.update();
        sim.take_deltas();
        let sector = TileLoc { x: 71, y: 70 }.sector();
        let find = |sim: &SimApp, x: u32| {
            let map = sim.app.world().resource::<LoadedMap>();
            let index = map.mobiles[&sector].iter().position(|o| crate::object_location(o) == Some(TileLoc { x, y: 70 })).unwrap() as u32;
            GroundItemId::Container { sector, index }
        };
        let (a, b, c) = (find(&sim, 71), find(&sim, 72), find(&sim, 69));
        assert!(sim.app.world().resource::<LoadedMap>().ground_item(a).is_none(), "not an item");

        // The locked chest refuses with "Locked." (inven_ui.mes 1).
        sim.receive([(PlayerId(1), PlayerIntent::PickUp { item: b, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(p, d)| p.0 == 1 && *d == LedgerDelta::OpenRefused { message: 1 }));
        assert!(!deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::PileOpened { .. })));

        // The first opens, shows open for everyone, and holds its stored dagger at 3.
        sim.receive([(PlayerId(1), PlayerIntent::PickUp { item: a, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(p, d)| p.0 == 1 && *d == LedgerDelta::PileOpened { pile: a }));
        assert!(deltas.iter().any(|(p, d)| p.0 == 2 && *d == LedgerDelta::ContainerShown { container: a, open: true }));
        let items = |sim: &SimApp, id| sim.app.world().resource::<LoadedMap>().pile(id).unwrap().inventory.items.iter().map(|i| (i.prototype, i.location, i.quantity)).collect::<Vec<_>>();
        assert_eq!(items(&sim, a), [(6049, 3, 0)]);
        sim.receive([
            (PlayerId(1), PlayerIntent::Inventory { seq: 1, request: InventoryRequest::Take { from: 3, to: None } }),
            (PlayerId(1), PlayerIntent::Inventory { seq: 2, request: InventoryRequest::ClosePile }),
        ]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(p, d)| p.0 == 2 && *d == LedgerDelta::ContainerShown { container: a, open: false }), "shut on close");
        assert!(items(&sim, a).is_empty(), "an empty chest stays");
        let saved = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert!(saved.sector_changes[&sector].containers[&match a { GroundItemId::Container { index, .. } => index, _ => unreachable!() }].items.is_empty(), "kept in the save");

        // The pending source rolls once: 5 gold and the certain dagger, not the 0% one.
        sim.receive([(PlayerId(1), PlayerIntent::PickUp { item: c, seq: 0 })]);
        sim.update();
        sim.take_deltas();
        let rolled = items(&sim, c);
        assert_eq!(rolled.iter().map(|i| (i.0, i.2)).collect::<Vec<_>>(), [(crate::GOLD, 5), (6049, 0)]);
        sim.receive([(PlayerId(1), PlayerIntent::Inventory { seq: 3, request: InventoryRequest::ClosePile })]);
        sim.update();
        sim.receive([(PlayerId(1), PlayerIntent::PickUp { item: c, seq: 0 })]);
        sim.update();
        assert_eq!(items(&sim, c), rolled, "not rolled again");
    }

    #[test]
    fn a_key_taken_from_one_chest_opens_another() {
        use arcanum_formats::object::{Array, Object, ObjectId, TYPE_CONTAINER, TYPE_KEY, TYPE_KEY_RING, Value};
        use arcanum_formats::test_util::{ObjectBuilder, object_id_bytes, prototype_id};
        use crate::{GroundItemId, InventoryRequest};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let folder = dir.path().join("maps").join(MAP);
        let at = |x: u32, y: u32| Value::Int64(Some(i64::from(x) | (i64::from(y) << 32)));
        let guid = |n: u8| ObjectId { kind: ObjectId::KIND_GUID, payload: [n; 16] };
        // Key 14 lies in the first chest; the second is locked to it, the third to key 15.
        let mut key = ObjectBuilder::new(TYPE_KEY).field("flags", Value::Int32(crate::OF_INVENTORY)).field("item_inv_location", Value::Int32(0)).field("key_key_id", Value::Int32(14));
        key.prototype = prototype_id(7000);
        key.id = guid(9);
        std::fs::write(folder.join("G_K.mob"), key.build()).unwrap();
        let chest = |name: &str, x: u32, flags: i32, lock: i32, list: &[ObjectId]| {
            let data = list.iter().flat_map(object_id_bytes).collect();
            let mut b = ObjectBuilder::new(TYPE_CONTAINER)
                .field("current_aid", Value::Int32(0x7000_0000))
                .field("location", at(x, 70))
                .field("container_flags", Value::Int32(flags))
                .field("container_key_id", Value::Int32(lock))
                .field("container_inventory_list_idx", Value::Array(Some(Array { elem_size: 24, id: 0, data, bitset: vec![] })));
            b.id = guid(x as u8);
            std::fs::write(folder.join(format!("G_{name}.mob")), b.build()).unwrap();
        };
        chest("A", 71, 0, 0, &[guid(9)]);
        chest("B", 72, crate::OCOF_LOCKED, 14, &[]);
        chest("C", 69, crate::OCOF_LOCKED, 15, &[]);
        let proto = |number: u32, kind: u32| {
            let mut o = Object::parse_instance(&ObjectBuilder::new(kind).field("item_weight", Value::Int32(1)).build()).unwrap();
            o.id = prototype_id(number);
            o
        };
        let mut s = save(&[(1, "A", TileLoc { x: 70, y: 70 })]);
        s.players.get_mut(&PlayerId(1)).unwrap().blueprint.sheet.base[0] = 8;
        // The PC carries a key ring with key 3 on it.
        let ring = crate::CarriedItem { prototype: 7001, location: 0, quantity: 0, keys: vec![3], written: 0, wear: Default::default() };
        s.players.get_mut(&PlayerId(1)).unwrap().inventory = Some(crate::Inventory { items: vec![ring], ..Default::default() });
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut prototypes = Prototypes::default();
        prototypes.insert(proto(7000, TYPE_KEY));
        prototypes.insert(proto(7001, TYPE_KEY_RING));
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        sim.boot(s);
        sim.update();
        sim.update();
        sim.take_deltas();
        let sector = TileLoc { x: 71, y: 70 }.sector();
        let find = |sim: &SimApp, x: u32| {
            let map = sim.app.world().resource::<LoadedMap>();
            let index = map.mobiles[&sector].iter().position(|o| crate::object_location(o) == Some(TileLoc { x, y: 70 })).unwrap() as u32;
            GroundItemId::Container { sector, index }
        };
        let (a, b, c) = (find(&sim, 71), find(&sim, 72), find(&sim, 69));
        let open = |sim: &mut SimApp, id| {
            sim.receive([(PlayerId(1), PlayerIntent::PickUp { item: id, seq: 0 })]);
            sim.update();
            sim.take_deltas()
        };
        let opened = |deltas: &[(PlayerId, LedgerDelta)], id| deltas.iter().any(|(_, d)| *d == LedgerDelta::PileOpened { pile: id });

        assert!(!opened(&open(&mut sim, b), b), "no key yet");
        // The stored key keeps its number; taken, it goes onto the ring and leaves the grid.
        let deltas = open(&mut sim, a);
        assert!(opened(&deltas, a));
        assert_eq!(sim.app.world().resource::<LoadedMap>().pile(a).unwrap().inventory.items[0].keys, [14]);
        sim.receive([
            (PlayerId(1), PlayerIntent::Inventory { seq: 1, request: InventoryRequest::Take { from: 0, to: Some(55) } }),
            (PlayerId(1), PlayerIntent::Inventory { seq: 2, request: InventoryRequest::ClosePile }),
        ]);
        sim.update();
        sim.take_deltas();
        let inventory = sim.app.world_mut().query_filtered::<&crate::Inventory, With<crate::PlayerControlled>>().single(sim.app.world()).unwrap().clone();
        assert_eq!(inventory.items.iter().map(|i| (i.prototype, i.keys.clone())).collect::<Vec<_>>(), [(7001, vec![3, 14])]);

        // Key 14 opens its chest, which stays locked; key 15's chest still refuses.
        let deltas = open(&mut sim, b);
        assert!(opened(&deltas, b), "{deltas:?}");
        let map = sim.app.world().resource::<LoadedMap>();
        assert_eq!(map.prototypes.int(map.container_object(b).unwrap(), "container_flags"), Some(crate::OCOF_LOCKED));
        assert!(open(&mut sim, c).iter().any(|(_, d)| *d == LedgerDelta::OpenRefused { message: 1 }));

        // A master picks key 15's lock; the chest then opens without its key.
        {
            let world = sim.app.world_mut();
            let mut sheet = world.query_filtered::<&mut crate::Sheet, With<crate::PlayerControlled>>().single_mut(world).unwrap();
            sheet.0.base[1] = 20;
            sheet.0.skills[crate::SKILL_PICK_LOCKS] = 5;
        }
        sim.receive([(PlayerId(1), PlayerIntent::PickLock { tool: None, target: crate::LockTarget::Container(c), seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(_, d)| *d == LedgerDelta::SkillResult { message: crate::PICK_SUCCEEDED }), "{deltas:?}");
        assert!(opened(&open(&mut sim, c), c), "picked");
        let saved = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        let GroundItemId::Container { index, .. } = c else { unreachable!() };
        assert_eq!(saved.sector_changes[&sector].container_flags[&index], 0);
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
    fn stepping_on_a_jump_point_teleports_the_pc() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        // A jump point at (69, 80) to (75, 75) on this map (-1).
        let mut jmp = 1u32.to_le_bytes().to_vec();
        for v in [0u32, 0, 69, 80, u32::MAX, 0, 75, 75] {
            jmp.extend(v.to_le_bytes());
        }
        std::fs::write(dir.path().join("maps").join(MAP).join("map.jmp"), jmp).unwrap();
        let mut sim = booted(dir.path(), save(&[(7, "A", TileLoc { x: 70, y: 80 })]));
        assert_eq!(sim.app.world().resource::<LoadedMap>().jump_points.len(), 1);
        sim.take_deltas();
        send_move(&mut sim, 7, 1, &[(70, 80), (69, 80)]);
        walk_ticks(&mut sim, ticks_for(1) + 1);
        assert_eq!(position(&mut sim, 7), TileLoc { x: 75, y: 75 }, "jumped on arrival");
        assert!(sim.take_deltas().iter().any(|(_, d)| matches!(d, LedgerDelta::Teleported { to, .. } if *to == TileLoc { x: 75, y: 75 })));
        walk_ticks(&mut sim, 3);
        assert_eq!(position(&mut sim, 7), TileLoc { x: 75, y: 75 }, "once");
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
    fn talking_to_an_npc_runs_its_dialogue_script_and_the_conversation_follows_the_choices() {
        use crate::script::tests::{always, block, n};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        sim.take_deltas();
        // NPC "A" (the sector's first mobile object) has script 700 for dialogue: start at line 1.
        let npc_object = crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 700));
        let dialog = arcanum_formats::dialog::DialogFile::parse(
            b"{1}{Hello, @pcname@.}{Hello, lady.}{}{2}{}{lf 0 1}\n{2}{Give me gold.}{}{5}{}{10}{$$ 10}\n{3}{Bye.}{}{5}{}{0}{}\n{4}{Again?}{}{5}{me 1}{0}{}\n{10}{Thanks.}{}{}{}{}{}",
        );
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            library.insert(700, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(4, &[n(1)]))] });
            library.insert_dialog(700, dialog);
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        let float = LedgerDelta::FloatText { speaker: npc, text: "Hello, Virgil.".into(), colour: crate::FloatColour::White, voice: Some("sound/speech/00700/v2_m.mp3".into()) };
        assert!(deltas.contains(&(PlayerId(7), float)), "{deltas:?}");
        assert!(deltas.contains(&(PlayerId(7), LedgerDelta::DialogueOptions { npc, options: vec!["Give me gold.".into(), "Bye.".into()] })), "not met yet: no \"Again?\"");
        sim.receive([(PlayerId(7), crate::PlayerIntent::Choose { option: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::FloatText { text, colour: crate::FloatColour::Green, .. } if text == "Give me gold.")));
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::FloatText { text, .. } if text == "Thanks.")));
        assert!(deltas.contains(&(PlayerId(7), LedgerDelta::DialogueEnded)), "no options after \"Thanks.\": the end");
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::Inventory { inventory, .. } if inventory.items.iter().any(|i| i.prototype == crate::GOLD && i.quantity == 10))));
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.players[&PlayerId(7)].script_state.met.len(), 1, "met");
        assert_eq!(save.sector_changes[&SectorId::new(1, 1)].mobile_scripts[&0][&crate::EVENT_DIALOGUE].flags, 1, "the NPC's flag 0, saved");
        // Again: now "Again?" shows.
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.contains(&(PlayerId(7), LedgerDelta::DialogueOptions { npc, options: vec!["Give me gold.".into(), "Bye.".into(), "Again?".into()] })), "{deltas:?}");
        sim.receive([(PlayerId(7), crate::PlayerIntent::EndTalk)]);
        sim.update();
        assert!(sim.take_deltas().contains(&(PlayerId(7), LedgerDelta::DialogueEnded)));
    }

    #[test]
    fn experience_from_dialogue_is_shared_by_every_pc() {
        use crate::script::tests::{always, block, n};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        std::fs::create_dir_all(dir.path().join("Rules")).unwrap();
        std::fs::write(dir.path().join("Rules/xp_quest.mes"), "{3}{5000}\n").unwrap();
        let rules = RulesData { xp_levels: vec![0, 2100, 4600, 7700], ..Default::default() };
        let mut sim = SimApp::new(rules);
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(7, "Virgil", TileLoc { x: 79, y: 80 }), (8, "Sogg", TileLoc { x: 70, y: 70 })]));
        sim.update();
        sim.update();
        let npc_object = crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 704));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            library.insert(704, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(4, &[n(1)]))] });
            library.insert_dialog(704, arcanum_formats::dialog::DialogFile::parse(b"{1}{Well done.}{}{}{}{}{xp 3}"));
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        sim.take_deltas();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        for p in [7, 8] {
            let sheet = &save.players[&PlayerId(p)].blueprint.sheet;
            assert_eq!((sheet.experience, sheet.level), (2500, 2), "player {p}: half of 5000");
        }
    }

    #[test]
    fn a_pc_with_spot_trap_stops_before_a_trap_object() {
        use arcanum_formats::object::{Object, TYPE_TRAP, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 70, y: 70 })]));
        // A mechanical trap on (73, 70).
        let sector = SectorId::new(1, 1);
        let trap_at = |x: u32| Value::Int64(Some(i64::from(x) | (70i64 << 32)));
        let index = {
            let mut map = sim.app.world_mut().resource_mut::<LoadedMap>();
            let trap = Object::parse_instance(&ObjectBuilder::new(TYPE_TRAP).field("location", trap_at(73)).build()).unwrap();
            let list = map.mobiles.entry(sector).or_default();
            list.push(trap);
            let index = list.len() as u32 - 1;
            map.set_attachment(crate::ScriptObject::Mobile { sector, index }, crate::EVENT_USE, crate::Attachment { flags: 0, counters: [2, 2, 0, 0], number: 30001 });
            index
        };
        let trap = arcanum_formats::script::Script { description: "Mechanical trap".into(), flags: crate::TRAP_MECHANICAL | arcanum_formats::script::RUN_ONCE, lines: vec![] };
        sim.app.world_mut().resource_mut::<crate::Scripts>().0.get_or_insert_with(Default::default).insert(30001, trap);
        {
            let world = sim.app.world_mut();
            let mut sheet = world.query::<&mut crate::Sheet>().single_mut(world).unwrap();
            sheet.0.base = [20; 8];
            sheet.0.skills[crate::SKILL_SPOT_TRAP] = 5;
        }
        send_move(&mut sim, 7, 1, &[(71, 70), (72, 70), (73, 70), (74, 70)]);
        for _ in 0..ticks_for(4) {
            sim.update();
        }
        let flags = sim.app.world().resource::<LoadedMap>().object_flags(sector, index, true).unwrap();
        assert_eq!(flags & crate::OF_TRAP_SPOTTED, crate::OF_TRAP_SPOTTED, "spotted (90 %: level 20 and Intelligence 20)");
        assert_eq!(position(&mut sim, 7), TileLoc { x: 72, y: 70 }, "stopped before it");
    }

    #[test]
    fn a_pc_stepping_onto_a_trap_object_springs_it() {
        use arcanum_formats::object::{Object, TYPE_TRAP, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 70, y: 70 })]));
        // A mechanical trap of 2 damage on (73, 70); the PC has no Spot Trap.
        let sector = SectorId::new(1, 1);
        let index = {
            let mut map = sim.app.world_mut().resource_mut::<LoadedMap>();
            let trap = Object::parse_instance(&ObjectBuilder::new(TYPE_TRAP).field("location", Value::Int64(Some(73 | (70i64 << 32)))).build()).unwrap();
            let list = map.mobiles.entry(sector).or_default();
            list.push(trap);
            let index = list.len() as u32 - 1;
            map.set_attachment(crate::ScriptObject::Mobile { sector, index }, crate::EVENT_USE, crate::Attachment { flags: 0, counters: [2, 2, 0, 0], number: 30001 });
            index
        };
        let trap = arcanum_formats::script::Script { description: "Mechanical trap".into(), flags: crate::TRAP_MECHANICAL | arcanum_formats::script::RUN_ONCE, lines: vec![] };
        sim.app.world_mut().resource_mut::<crate::Scripts>().0.get_or_insert_with(Default::default).insert(30001, trap);
        send_move(&mut sim, 7, 1, &[(71, 70), (72, 70), (73, 70), (74, 70)]);
        for _ in 0..ticks_for(4) {
            sim.update();
        }
        assert_eq!(position(&mut sim, 7), TileLoc { x: 73, y: 70 }, "the walk ends on the trap (0x430cf7)");
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.players[&PlayerId(7)].blueprint.sheet.hp_damage, 2, "sprung on the PC");
        let map = sim.app.world().resource::<LoadedMap>();
        assert_eq!(map.attachment(crate::ScriptObject::Mobile { sector, index }, crate::EVENT_USE).number, 0, "run once");
        assert_ne!(map.object_flags(sector, index, true).unwrap() & crate::OF_DESTROYED, 0, "the trap object destroyed (0x441a80)");
    }

    #[test]
    fn an_npc_stepping_onto_a_trap_object_springs_it() {
        use arcanum_formats::object::{Object, TYPE_TRAP, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 70, y: 70 })]));
        let sector = SectorId::new(1, 1);
        let (entity, npc_index, at) = {
            let world = sim.app.world_mut();
            let (e, s, p) = world.query::<(Entity, &crate::NpcSource, &Position)>().iter(world).find(|(_, s, _)| s.sector == sector).unwrap();
            (e, s.index, p.0)
        };
        let next = TileLoc { x: at.x + 1, y: at.y };
        // A mechanical trap of 2 damage on the tile next to the NPC, which has no leader.
        let index = {
            let mut map = sim.app.world_mut().resource_mut::<LoadedMap>();
            let trap = Object::parse_instance(&ObjectBuilder::new(TYPE_TRAP).field("location", Value::Int64(Some(i64::from(next.x) | (i64::from(next.y) << 32)))).build()).unwrap();
            let list = map.mobiles.entry(sector).or_default();
            list.push(trap);
            let index = list.len() as u32 - 1;
            map.set_attachment(crate::ScriptObject::Mobile { sector, index }, crate::EVENT_USE, crate::Attachment { flags: 0, counters: [2, 2, 0, 0], number: 30001 });
            index
        };
        let trap = arcanum_formats::script::Script { description: "Mechanical trap".into(), flags: crate::TRAP_MECHANICAL | arcanum_formats::script::RUN_ONCE, lines: vec![] };
        sim.app.world_mut().resource_mut::<crate::Scripts>().0.get_or_insert_with(Default::default).insert(30001, trap);
        let max = sim.app.world().resource::<LoadedMap>().npc_hp(sector, npc_index).unwrap().1;
        sim.app.world_mut().entity_mut(entity).insert(crate::Walking { steps: [next, TileLoc { x: next.x + 1, y: next.y }].into(), progress: 0.0 });
        for _ in 0..ticks_for(3) {
            sim.update();
        }
        let map = sim.app.world().resource::<LoadedMap>();
        assert_eq!(map.npc_hp(sector, npc_index).unwrap(), (max - 2, max), "sprung on the NPC: it knows the trap, so does not roll (0x4bbfe0)");
        assert_ne!(map.object_flags(sector, index, true).unwrap() & crate::OF_DESTROYED, 0, "the trap object destroyed");
        assert_eq!(sim.app.world().get::<Position>(entity).unwrap().0, next, "its walk ends on the trap");
    }

    /// An NPC walking towards a trap object with Spot Trap 5 points (level 20) and Intelligence 20
    /// (90 %): `follower` gives it a leader, `pc_set` marks the trap a PC's. Returns the NPC's
    /// tile after the walk, its start, and the trap's flags.
    fn npc_walks_at_a_trap(follower: bool, pc_set: bool) -> (TileLoc, TileLoc, i32) {
        use arcanum_formats::object::{Array, Object, TYPE_NPC, TYPE_TRAP, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 70, y: 70 })]));
        let sector = SectorId::new(1, 1);
        let (entity, npc_index, at) = {
            let world = sim.app.world_mut();
            let (e, s, p) = world.query::<(Entity, &crate::NpcSource, &Position)>().iter(world).find(|(_, s, _)| s.sector == sector).unwrap();
            (e, s.index, p.0)
        };
        let next = TileLoc { x: at.x + 1, y: at.y };
        let tile = |t: TileLoc| Value::Int64(Some(i64::from(t.x) | (i64::from(t.y) << 32)));
        let array = |values: &[i32]| Value::Array(Some(Array { elem_size: 4, id: 1, data: values.iter().flat_map(|x| x.to_le_bytes()).collect(), bitset: vec![(1u32 << values.len()) - 1, 0] }));
        let index = {
            let mut map = sim.app.world_mut().resource_mut::<LoadedMap>();
            let mut skills = [0; 8];
            skills[crate::SKILL_SPOT_TRAP] = 5;
            let mut stats = [0; 5];
            stats[4] = 20;
            let old = &map.mobiles[&sector][npc_index as usize];
            let mut npc = ObjectBuilder::new(TYPE_NPC).field("location", tile(at)).field("critter_basic_skill_idx", array(&skills)).field("critter_stat_base_idx", array(&stats));
            npc.id = old.id;
            map.mobiles.get_mut(&sector).unwrap()[npc_index as usize] = Object::parse_instance(&npc.build()).unwrap();
            let flags = if pc_set { crate::OF_TRAP_PLACED_BY_PC } else { 0 };
            let trap = Object::parse_instance(&ObjectBuilder::new(TYPE_TRAP).field("location", tile(next)).field("flags", Value::Int32(flags)).build()).unwrap();
            let list = map.mobiles.entry(sector).or_default();
            list.push(trap);
            let index = list.len() as u32 - 1;
            map.set_attachment(crate::ScriptObject::Mobile { sector, index }, crate::EVENT_USE, crate::Attachment { flags: 0, counters: [2, 2, 0, 0], number: 30001 });
            index
        };
        let trap = arcanum_formats::script::Script { description: "Mechanical trap".into(), flags: crate::TRAP_MECHANICAL | arcanum_formats::script::RUN_ONCE, lines: vec![] };
        sim.app.world_mut().resource_mut::<crate::Scripts>().0.get_or_insert_with(Default::default).insert(30001, trap);
        let mut npc = sim.app.world_mut().entity_mut(entity);
        npc.insert(crate::Walking { steps: [next, TileLoc { x: next.x + 1, y: next.y }].into(), progress: 0.0 });
        if follower {
            npc.insert(crate::follow::Following { leader: PlayerId(7), spread_out: false });
        }
        for _ in 0..ticks_for(3) {
            sim.update();
        }
        let flags = sim.app.world().resource::<LoadedMap>().object_flags(sector, index, true).unwrap();
        (sim.app.world().get::<Position>(entity).unwrap().0, at, flags)
    }

    #[test]
    fn a_follower_with_spot_trap_stops_before_a_trap_object() {
        let (stood, start, flags) = npc_walks_at_a_trap(true, false);
        assert_eq!(stood, start, "stopped before the step (0x4272b3)");
        assert_eq!(flags & crate::OF_TRAP_SPOTTED, crate::OF_TRAP_SPOTTED, "a follower marks it (0x4bc125)");
    }

    #[test]
    fn a_leaderless_npc_rolls_only_for_a_pcs_trap_and_marks_nothing() {
        let (stood, start, flags) = npc_walks_at_a_trap(false, true);
        assert_eq!(stood, start, "stopped before the step");
        assert_eq!(flags & crate::OF_TRAP_SPOTTED, 0, "not marked (0x4bc11a)");
    }

    #[test]
    fn players_learn_which_npcs_their_pc_has_met() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        let sector = SectorId::new(1, 1);
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0 && s.sector == sector).unwrap().0
        };
        let known = |sim: &mut SimApp| {
            (0..16).flat_map(|_| {
                sim.update();
                sim.take_deltas()
            })
            .filter_map(|(p, d)| match d { LedgerDelta::KnownNpcs { npcs } if p.0 == 7 => Some(npcs), _ => None })
            .flatten()
            .collect::<Vec<_>>()
        };
        assert!(known(&mut sim).is_empty(), "not met");
        let key = crate::met_key(crate::ScriptObject::Mobile { sector, index: 0 });
        sim.app.world_mut().resource_mut::<WorldState>().0.as_mut().unwrap().players.get_mut(&PlayerId(7)).unwrap().script_state.met.insert(key);
        assert_eq!(known(&mut sim), [npc]);
        assert!(known(&mut sim).is_empty(), "once");
    }

    #[test]
    fn quest_experience_from_a_script_is_shared_too() {
        use crate::script::tests::{always, block, n, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        std::fs::create_dir_all(dir.path().join("Rules")).unwrap();
        std::fs::write(dir.path().join("Rules/xp_quest.mes"), "{3}{5000}\n").unwrap();
        let rules = RulesData { xp_levels: vec![0, 2100, 4600, 7700], ..Default::default() };
        let mut sim = SimApp::new(rules);
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(7, "Virgil", TileLoc { x: 79, y: 80 }), (8, "Sogg", TileLoc { x: 70, y: 70 })]));
        sim.update();
        sim.update();
        let npc_object = crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 705));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            // Quest level 3 for the triggerer (set 0); the attachee, an NPC, gets nothing.
            let lines = vec![always(block(0x3f, &[op(0, 0), n(3)])), always(block(0x3f, &[op(1, 0), n(3)])), always(block(1, &[]))];
            library.insert(705, arcanum_formats::script::Script { description: String::new(), flags: 0, lines });
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        sim.update();
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        for p in [7, 8] {
            assert_eq!(save.players[&PlayerId(p)].blueprint.sheet.experience, 2500, "player {p}: half of 5000");
        }
    }

    #[test]
    fn a_script_teleport_waits_for_every_players_yes_and_one_no_cancels_it() {
        use crate::script::tests::{always, block, n, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 79, y: 80 }), (8, "Sogg", TileLoc { x: 70, y: 70 })]));
        // NPC "A"'s dialogue script teleports the triggerer (set 0) to (75, 75) on this map.
        let npc_object = crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 700));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            let teleport = block(0x2c, &[op(0, 0), n(crate::big_move::THIS_MAP), n(75), n(75)]);
            library.insert(700, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(teleport), always(block(1, &[]))] });
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        let position = |sim: &mut SimApp, player: u64| pcs(sim).into_iter().find(|p| p.1 .0 == player).unwrap().2;
        sim.take_deltas();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        let id = deltas.iter().find_map(|(p, d)| match d { LedgerDelta::MoveVote { ask } if p.0 == 8 => Some(ask.id), _ => None }).expect("both players are asked");
        assert!(deltas.iter().any(|(p, d)| p.0 == 7 && matches!(d, LedgerDelta::MoveVote { ask } if ask.causer == "Virgil")));
        sim.receive([(PlayerId(7), crate::PlayerIntent::Vote { id, yes: true })]);
        sim.update();
        assert_eq!(position(&mut sim, 7), TileLoc { x: 79, y: 80 }, "Sogg has not answered");
        sim.receive([(PlayerId(8), crate::PlayerIntent::Vote { id, yes: true })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert_eq!(position(&mut sim, 7), TileLoc { x: 75, y: 75 });
        assert!(deltas.iter().any(|(p, d)| p.0 == 8 && matches!(d, LedgerDelta::Teleported { to, .. } if *to == TileLoc { x: 75, y: 75 })));
        assert!(deltas.iter().any(|(_, d)| *d == LedgerDelta::VoteEnded { id, passed: true }));
        assert_eq!(sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(7)].position, TileLoc { x: 75, y: 75 }, "saved");
        // Again, from (75, 75) the NPC is out of reach: walk back by teleporting the PC there.
        {
            let world = sim.app.world_mut();
            let mut q = world.query::<(&PlayerControlled, &mut Position)>();
            for (p, mut at) in q.iter_mut(world) {
                if p.0 == PlayerId(7) {
                    at.0 = TileLoc { x: 79, y: 80 };
                }
            }
        }
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        let id = deltas.iter().find_map(|(_, d)| match d { LedgerDelta::MoveVote { ask } => Some(ask.id), _ => None }).unwrap();
        sim.receive([(PlayerId(8), crate::PlayerIntent::Vote { id, yes: false })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(_, d)| *d == LedgerDelta::VoteEnded { id, passed: false }), "one no cancels it");
        assert_eq!(position(&mut sim, 7), TileLoc { x: 79, y: 80 });
    }

    #[test]
    fn followers_change_maps_with_their_leader_and_come_home() {
        use crate::follow::{Away, Follower, Following};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let (from, to) = (dir.path().join("maps").join(MAP), dir.path().join("maps").join("Other"));
        std::fs::create_dir_all(&to).unwrap();
        for entry in std::fs::read_dir(&from).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
        std::fs::create_dir_all(dir.path().join("Rules")).unwrap();
        std::fs::write(dir.path().join("Rules/MapList.mes"), format!("{{5000}}{{{MAP}, 70, 70}}\n{{5001}}{{Other, 75, 75}}\n")).unwrap();
        let home = SectorId::new(1, 1);
        let mut s = save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]);
        s.followers.push(Follower { map: MAP.into(), sector: home, index: 0, leader: PlayerId(7), away: None, wait_until: None, warn: 0 });
        let mut sim = booted(dir.path(), s);
        let go = |sim: &mut SimApp, map: i32, at: TileLoc| {
            let teleport = crate::big_move::ScriptTeleport { causer: PlayerId(7), object: crate::ScriptObject::Pc(PlayerId(7)), map, to: at };
            sim.app.world_mut().resource_mut::<crate::big_move::ScriptTeleports>().0.push(teleport);
            sim.update();
            sim.update();
            sim.update();
        };
        let arrival = TileLoc { x: 75, y: 90 };
        go(&mut sim, 5001, arrival);
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.map, "Other");
        assert_eq!(save.followers[0].away, Some(Away { map: "Other".into(), at: arrival, flags: 0 }));
        let guest = {
            let map = sim.app.world().resource::<LoadedMap>();
            assert_eq!(map.guests.len(), 1);
            *map.guests.keys().next().unwrap()
        };
        {
            let world = sim.app.world_mut();
            let found: Vec<(TileLoc, Option<Following>)> = world
                .query::<(&crate::NpcSource, &Position, Option<&Following>)>()
                .iter(world)
                .filter(|(s, ..)| (s.sector, s.index) == guest)
                .map(|(_, p, f)| (p.0, f.copied()))
                .collect();
            assert_eq!(found, [(arrival, Some(Following { leader: PlayerId(7), spread_out: false }))], "the follower arrives with its leader");
        }
        // Back home: the follower takes its home place again, once.
        go(&mut sim, 5000, TileLoc { x: 79, y: 80 });
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!((save.map.as_str(), save.followers[0].away.clone()), (MAP, None));
        assert!(sim.app.world().resource::<LoadedMap>().guests.is_empty());
        let world = sim.app.world_mut();
        let homes = world.query::<(&crate::NpcSource, Option<&Following>)>().iter(world).filter(|(s, _)| s.sector == home && s.index == 0).map(|(_, f)| f.is_some()).collect::<Vec<_>>();
        assert_eq!(homes, [true]);
    }

    #[test]
    fn a_teleport_to_another_map_takes_the_party_and_keeps_each_maps_changes() {
        use crate::script::tests::{always, block, n, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        // "Other": a copy of the test map.
        let (from, to) = (dir.path().join("maps").join(MAP), dir.path().join("maps").join("Other"));
        std::fs::create_dir_all(&to).unwrap();
        for entry in std::fs::read_dir(&from).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
        std::fs::create_dir_all(dir.path().join("Rules")).unwrap();
        std::fs::write(dir.path().join("Rules/MapList.mes"), format!("{{5000}}{{{MAP}, 70, 70}}\n{{5001}}{{Other, 75, 75}}\n")).unwrap();
        let mut s = save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]);
        s.sector_changes.insert(SectorId::new(1, 1), crate::SectorDelta { portals: [(0, true)].into(), ..Default::default() });
        let mut sim = booted(dir.path(), s);
        let teleport_on = |sim: &mut SimApp, map: i32, at: TileLoc| {
            let npc_object = crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 };
            sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 700));
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            let go = block(0x2c, &[op(0, 0), n(map), n(at.x as i32), n(at.y as i32)]);
            library.insert(700, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(go), always(block(1, &[]))] });
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        let npc = teleport_on(&mut sim, 5001, TileLoc { x: 79, y: 81 });
        sim.take_deltas();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.contains(&(PlayerId(7), LedgerDelta::Despawn { net_id: npc })), "the old map's NPCs go");
        assert!(deltas.contains(&(PlayerId(7), LedgerDelta::EnterMap { map: "Other".into() })), "{deltas:?}");
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::Teleported { to, .. } if *to == TileLoc { x: 79, y: 81 })));
        assert!(!deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::MapChanges { .. })), "Other has no changes yet");
        let entered = deltas.iter().position(|(_, d)| matches!(d, LedgerDelta::EnterMap { .. })).unwrap();
        assert!(deltas[entered..].iter().any(|(_, d)| matches!(d, LedgerDelta::Spawn { kind: EntityKind::Npc { .. }, .. })), "Other's NPCs come, after EnterMap");
        {
            let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
            assert_eq!(save.map, "Other");
            assert_eq!(save.players[&PlayerId(7)].position, TileLoc { x: 79, y: 81 });
            assert!(save.sector_changes.is_empty());
            assert!(save.other_maps[MAP][&SectorId::new(1, 1)].portals[&0], "Test's changes kept");
            assert_eq!(sim.app.world().resource::<LoadedMap>().name.as_deref(), Some("Other"));
        }
        // And back: Test's changes come back with it.
        let npc = teleport_on(&mut sim, 5000, TileLoc { x: 79, y: 80 });
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::MapChanges { changes } if changes[&SectorId::new(1, 1)].portals[&0])));
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.map, MAP);
        assert!(save.other_maps.is_empty() || !save.other_maps.contains_key(MAP));
    }

    #[test]
    fn a_lone_player_teleports_at_once_and_other_maps_are_reported() {
        use crate::script::tests::{always, block, n, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        let npc_object = crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 700));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            let here = block(0x2c, &[op(0, 0), n(crate::big_move::THIS_MAP), n(75), n(76)]);
            let elsewhere = block(0x2c, &[op(0, 0), n(5007), n(306), n(303)]);
            library.insert(700, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(here), always(elsewhere), always(block(1, &[]))] });
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        sim.take_deltas();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(!deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::MoveVote { .. })), "nobody to wait for");
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::Teleported { to, .. } if *to == TileLoc { x: 75, y: 76 })));
        let reports = &sim.app.world().resource::<crate::big_move::TeleportReports>().0;
        assert_eq!(reports.len(), 1, "map 5007 is not this one: {reports:?}");
    }

    #[test]
    fn scripts_take_gold_give_items_set_quests_float_lines_and_read_types() {
        use crate::script::tests::{always, block, line, n, op};
        use arcanum_formats::object::{Object, TYPE_GOLD, TYPE_WEAPON, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut s = save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]);
        let mut gold = crate::Inventory::default();
        gold.change_gold(500);
        s.players.get_mut(&PlayerId(7)).unwrap().inventory = Some(gold);
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let proto = |number: u32, kind: u32| {
            let mut o = Object::parse_instance(&ObjectBuilder::new(kind).field("item_weight", Value::Int32(1)).build()).unwrap();
            o.id = prototype_id(number);
            o
        };
        let mut prototypes = Prototypes::default();
        prototypes.insert(proto(crate::GOLD, TYPE_GOLD));
        prototypes.insert(proto(6049, TYPE_WEAPON));
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        sim.boot(s);
        sim.update();
        sim.update();
        let npc_object = crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 700));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            let pc = op(0, 0);
            let npc = op(1, 0);
            let lines = vec![
                line(block(2, &[pc, n(10)]), block(0x23, &[pc, n(-10)]), block(1, &[])),           // 10 gold, or stop
                always(block(0x42, &[n((6049 - crate::BASIC_TO_PROTOTYPE) as i32), pc])),           // a dagger
                always(block(0x10, &[pc, n(1000), n(2)])),                                          // quest 1000: 2
                always(block(0x10, &[pc, n(1000), n(1)])),                                          // never down
                line(block(6, &[pc, n(1000), n(2)]), block(0x17, &[n(1), npc]), block(1, &[])),     // "Thanks."
                always(block(0x22, &[npc, op(2, 0)])),                                              // local 0 = type
                line(block(4, &[op(2, 0), n(TYPE_NPC as i32)]), block(0x64, &[n(5)]), block(0, &[])),
                always(block(1, &[])),
            ];
            library.insert(700, arcanum_formats::script::Script { description: String::new(), flags: 0, lines });
            library.insert_dialog(700, arcanum_formats::dialog::DialogFile::parse(b"{1}{Thanks.}{}{}{}{}{}"));
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        sim.take_deltas();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        let mine = deltas.iter().rev().find_map(|(_, d)| match d { LedgerDelta::Inventory { inventory, .. } => Some(inventory.clone()), _ => None }).expect("the changed inventory is sent");
        assert_eq!(mine.gold(), 490);
        assert!(mine.items.iter().any(|i| i.prototype == 6049), "{mine:?}");
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::FloatText { speaker, text, .. } if *speaker == npc && text == "Thanks.")), "{deltas:?}");
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.players[&PlayerId(7)].script_state.quest(1000), 2, "set up, not down");
        assert!(save.scripts.flag(5), "the NPC's type is 16");
        let world = sim.app.world_mut();
        let carried = world.query::<(&PlayerControlled, &crate::Inventory)>().iter(world).next().unwrap().1.gold();
        assert_eq!(carried, 490, "the PC's own inventory too");
    }

    #[test]
    fn a_script_moving_the_pcs_alignment_makes_its_followers_warn() {
        use crate::follow::{Follower, WARN_EVIL};
        use crate::script::tests::{always, block, n, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let sector = SectorId::new(1, 1);
        let mut start = save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]);
        start.followers.push(Follower { map: MAP.into(), sector, index: 0, leader: PlayerId(7), away: None, wait_until: None, warn: 0 });
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(start);
        sim.update();
        sim.update();
        let npc_object = crate::ScriptObject::Mobile { sector, index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 702));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            // Adjust stat 0x13 (alignment) of the PC by -50 (action 0x89).
            let lines = vec![always(block(0x89, &[n(0x13), op(0, 0), n(-50)])), always(block(1, &[]))];
            library.insert(702, arcanum_formats::script::Script { description: String::new(), flags: 0, lines });
        }
        let npc = { let world = sim.app.world_mut(); *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0 };
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.players[&PlayerId(7)].blueprint.sheet.alignment, -50);
        assert_eq!(save.followers[0].warn & WARN_EVIL, WARN_EVIL, "down: the evil warning (0x4000000)");
    }

    #[test]
    fn scripts_give_and_take_effects_and_the_sheet_follows() {
        use crate::script::tests::{always, block, n, op};
        use arcanum_formats::mes::MesFile;
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let rules = RulesData::from_files(&MesFile::default(), &MesFile::parse(b"{117}{st +2}{177}{pe max 1}").unwrap(), &MesFile::default());
        let mut sim = SimApp::new(rules.clone());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        sim.update();
        sim.update();
        let npc_object = crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 701));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            let (pc, npc) = (op(0, 0), op(1, 0));
            let lines = vec![
                always(block(0x4d, &[pc, n(117), n(5)])),
                always(block(0x4d, &[pc, n(117), n(5)])), // held twice
                always(block(0x4d, &[pc, n(177), n(5)])),
                always(block(0x4d, &[npc, n(117), n(5)])), // NPCs' are not kept
                always(block(0x4e, &[pc, n(117)])),        // one goes
                always(block(0x4e, &[pc, n(999)])),        // not held: nothing
                always(block(1, &[])),
            ];
            library.insert(701, arcanum_formats::script::Script { description: String::new(), flags: 0, lines });
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        sim.take_deltas();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        let sheet = deltas.iter().rev().find_map(|(_, d)| match d { LedgerDelta::SheetUpdate { sheet, .. } => Some(sheet.clone()), _ => None }).expect("the sheet goes to the client");
        let held: Vec<u32> = sheet.effects.iter().map(|e| e.id).collect();
        assert_eq!(held, [117, 177]);
        assert_eq!(sheet.effects[0].cause, 5);
        let stats = rules.stats(&sheet);
        assert_eq!((stats[0], stats[5]), (10, 1), "st 8 + 2; pe at most 1");
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::PcStatus { .. })), "everyone sees the new maxima");
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.players[&PlayerId(7)].blueprint.sheet.effects, sheet.effects, "kept in the save");
        let world = sim.app.world_mut();
        let component = world.query::<(&PlayerControlled, &crate::Sheet)>().iter(world).next().unwrap().1.0.clone();
        assert_eq!(component.effects, sheet.effects, "and on the PC");
    }

    #[test]
    fn a_script_switches_an_npc_off_and_it_goes_for_everyone() {
        use crate::script::tests::{always, block, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        sim.update();
        sim.update();
        let sector = SectorId::new(1, 1);
        let npc_object = crate::ScriptObject::Mobile { sector, index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 702));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            let lines = vec![always(block(0x38, &[op(1, 0)])), always(block(1, &[]))];
            library.insert(702, arcanum_formats::script::Script { description: String::new(), flags: 0, lines });
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        sim.take_deltas();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        sim.update();
        let deltas = sim.take_deltas();
        let flags = deltas.iter().find_map(|(_, d)| match d { LedgerDelta::ObjectFlags { sector: s, index: 0, mobile: true, flags } if *s == sector => Some(*flags), _ => None }).expect("sent");
        assert_eq!(flags & crate::OF_OFF, crate::OF_OFF);
        assert!(deltas.contains(&(PlayerId(7), LedgerDelta::Despawn { net_id: npc })), "{deltas:?}");
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.sector_changes[&sector].mobile_flags[&0], flags, "kept");
        let world = sim.app.world_mut();
        assert_eq!(world.query::<&crate::NpcSource>().iter(world).filter(|s| s.index == 0 && s.sector == sector).count(), 0);
        // Switched on again, it comes back.
        sim.app.world_mut().resource_mut::<LoadedMap>().set_object_flags(sector, 0, true, flags & !crate::OF_OFF);
        sim.app.world_mut().resource_mut::<ToggledNpcs>().0.push((sector, 0));
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(p, d)| p.0 == 7 && matches!(d, LedgerDelta::Spawn { .. })), "{deltas:?}");
    }

    #[test]
    fn an_npc_joins_through_dialogue_then_follows_its_leader() {
        use crate::follow::{Following, LEASH_TILES};
        use crate::script::tests::{always, block, n};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        sim.update();
        sim.update();
        let sector = SectorId::new(1, 1);
        let npc_object = crate::ScriptObject::Mobile { sector, index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 703));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            library.insert(703, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(4, &[n(1)]))] });
            library.insert_dialog(703, arcanum_formats::dialog::DialogFile::parse(b"{1}{Need a hand?}{}{}{}{}{}\n{2}{Come along.}{}{1}{}{0}{jo 0 3}\n{3}{Lead on.}{}{}{}{}{}"));
        }
        let (npc, entity) = {
            let world = sim.app.world_mut();
            let (e, id, _) = world.query::<(Entity, &NetId, &crate::NpcSource)>().iter(world).find(|(_, _, s)| s.index == 0 && s.sector == sector).unwrap();
            (*id, e)
        };
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Choose { option: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::FloatText { text, .. } if text == "Lead on.")), "line 3 after joining: {deltas:?}");
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.followers.len(), 1);
        assert_eq!((save.followers[0].leader, save.followers[0].index), (PlayerId(7), 0));
        sim.update();
        assert_eq!(sim.app.world().get::<Following>(entity), Some(&Following { leader: PlayerId(7), spread_out: false }));
        // Far away: put on the leader's tile.
        let start = sim.app.world().get::<Position>(entity).unwrap().0;
        let far = TileLoc { x: start.x + LEASH_TILES, y: start.y };
        let set_pc = |sim: &mut SimApp, at: TileLoc| {
            let world = sim.app.world_mut();
            let mut q = world.query_filtered::<&mut Position, With<PlayerControlled>>();
            q.single_mut(world).unwrap().0 = at;
        };
        set_pc(&mut sim, far);
        sim.update();
        assert_eq!(sim.app.world().get::<Position>(entity).unwrap().0, far, "leashed");
        // Six tiles off: it walks after the PC until within four.
        let near = TileLoc { x: far.x - 6, y: far.y };
        set_pc(&mut sim, near);
        sim.update();
        let walking = sim.app.world().get::<crate::Walking>(entity).expect("walks after its leader");
        assert_eq!(crate::tile_distance(*walking.steps.back().unwrap(), near), 4, "{walking:?}");
        // A teleport within the map takes it along, onto the same tile.
        let to = TileLoc { x: 75, y: 75 };
        let teleport = crate::big_move::ScriptTeleport { causer: PlayerId(7), object: crate::ScriptObject::Pc(PlayerId(7)), map: crate::big_move::THIS_MAP, to };
        sim.app.world_mut().resource_mut::<crate::big_move::ScriptTeleports>().0.push(teleport);
        sim.update();
        assert_eq!(sim.app.world().get::<Position>(entity).unwrap().0, to);
    }

    #[test]
    fn a_script_makes_an_npc_follow_moves_its_standpoint_and_lets_it_go() {
        use crate::follow::{FORCED, Following};
        use crate::script::tests::{always, block, line, n, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        sim.update();
        sim.update();
        let sector = SectorId::new(1, 1);
        let npc_object = crate::ScriptObject::Mobile { sector, index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 704));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            // Not following yet: follow the PC (set 0 is the triggerer), move the standpoint;
            // following: stop.
            let lines = vec![
                line(block(0x12, &[op(1, 0), op(0, 0)]), block(3, &[n(4)]), block(0, &[])),
                always(block(0x15, &[op(1, 0), op(0, 0)])),
                always(block(0x2d, &[op(1, 0), n(70), n(71)])),
                always(block(1, &[])),
                always(block(0x16, &[op(1, 0)])),
                always(block(1, &[])),
            ];
            library.insert(704, arcanum_formats::script::Script { description: String::new(), flags: 0, lines });
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0 && s.sector == sector).unwrap().0
        };
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        sim.update();
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.followers.len(), 1);
        assert_eq!(save.followers[0].leader, PlayerId(7));
        assert_eq!(save.sector_changes[&sector].npc_flags[&0] & FORCED, FORCED, "forced: not counted");
        assert_eq!(save.sector_changes[&sector].standpoints[&0], TileLoc { x: 70, y: 71 });
        let world = sim.app.world_mut();
        let (standpoints, following) = world.query::<(&crate::NpcSource, &crate::Standpoints, Option<&Following>)>().iter(world).find(|(s, ..)| s.index == 0 && s.sector == sector).map(|(_, p, f)| (*p, f.copied())).unwrap();
        assert_eq!(standpoints.day, TileLoc { x: 70, y: 71 }, "spawned again with it");
        assert_eq!(following, Some(Following { leader: PlayerId(7), spread_out: false }));
        // Talked to again: it stops following.
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0 && s.sector == sector).unwrap().0
        };
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 1 })]);
        sim.update();
        sim.update();
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert!(save.followers.is_empty());
        assert_eq!(save.sector_changes[&sector].npc_flags[&0] & FORCED, 0);
    }

    #[test]
    fn a_script_sets_an_npc_on_the_pc() {
        use crate::script::tests::{always, block, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        sim.update();
        sim.update();
        let sector = SectorId::new(1, 1);
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0 && s.sector == sector).unwrap().0
        };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(crate::ScriptObject::Mobile { sector, index: 0 }, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 706));
        {
            // As script 1000 has it: the attachee (set 1) fights the triggerer (set 0).
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let lines = vec![always(block(0x24, &[op(1, 0), op(0, 0)])), always(block(1, &[]))];
            scripts.0.get_or_insert_with(Default::default).insert(706, arcanum_formats::script::Script { description: String::new(), flags: 0, lines });
        }
        sim.take_deltas();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        sim.update();
        let order = sim.take_deltas().into_iter().find_map(|(_, d)| match d {
            LedgerDelta::Combat { order, .. } if !order.is_empty() => Some(order),
            _ => None,
        });
        assert!(order.is_some_and(|o| o.contains(&npc)), "the fight starts with the NPC in it");
    }

    #[test]
    fn a_script_walks_an_npc_and_renames_it() {
        use crate::script::tests::{always, block, n, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        sim.boot(save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        sim.update();
        sim.update();
        let sector = SectorId::new(1, 1);
        let (npc, at) = {
            let world = sim.app.world_mut();
            world.query::<(&NetId, &crate::NpcSource, &Position)>().iter(world).find(|(_, s, _)| s.index == 0 && s.sector == sector).map(|(n, _, p)| (*n, p.0)).unwrap()
        };
        let to = {
            use crate::Walkable;
            let map = sim.app.world().resource::<LoadedMap>();
            [(2, 0), (0, 2), (-2, 0), (0, -2), (2, 2), (-2, -2), (3, 0), (0, 3)].into_iter().map(|(dx, dy)| TileLoc { x: at.x.wrapping_add_signed(dx), y: at.y.wrapping_add_signed(dy) }).find(|&t| map.walkable(t) && crate::find_path(map, at, t).is_ok()).unwrap()
        };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(crate::ScriptObject::Mobile { sector, index: 0 }, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 705));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let lines = vec![always(block(0x45, &[op(1, 0), n(to.x as i32), n(to.y as i32)])), always(block(0x5c, &[op(1, 0), n(2805)])), always(block(1, &[]))];
            scripts.0.get_or_insert_with(Default::default).insert(705, arcanum_formats::script::Script { description: String::new(), flags: 0, lines });
        }
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        sim.update();
        sim.update();
        {
            let world = sim.app.world_mut();
            let walking = world.query::<(&NetId, &crate::Walking)>().iter(world).find(|(n, _)| **n == npc).map(|(_, w)| w.steps.back().copied());
            assert_eq!(walking, Some(Some(to)), "on its way");
        }
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.sector_changes[&sector].mobile_names[&0], 2805);
        assert_eq!(sim.app.world().resource::<LoadedMap>().mobiles[&sector][0].int("name"), Some(2805));
    }

    #[test]
    fn scripts_hurt_heal_show_plaques_transform_destroy_and_flip_switches() {
        use crate::script::tests::{always, block, line, n, op};
        use arcanum_formats::object::{Object, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        std::fs::create_dir_all(dir.path().join("mes")).unwrap();
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        let mut wolf = Object::parse_instance(&ObjectBuilder::new(TYPE_NPC).field("current_aid", Value::Int32(0x2800_2000)).build()).unwrap();
        wolf.id = prototype_id(9000);
        let mut prototypes = Prototypes::default();
        prototypes.insert(wolf);
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        sim.boot(save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        sim.update();
        sim.update();
        let sector = SectorId::new(1, 1);
        let npc_object = crate::ScriptObject::Mobile { sector, index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 705));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            let (pc, npc) = (op(0, 0), op(1, 0));
            let lines = vec![
                always(block(0x3c, &[pc, n(100), n(0)])),  // more than the 30 hit points
                always(block(0x3c, &[pc, n(7), n(1)])),    // poison
                always(block(0x3c, &[pc, n(3), n(4)])),    // fatigue
                always(block(0x3d, &[n(61), pc])),         // Halt Poison
                always(block(0x3d, &[n(60), pc])),         // Minor Healing: 5
                always(block(0x72, &[n(1000), pc])),
                always(block(0x6e, &[n(7)])),
                line(block(0x2e, &[n(7)]), block(0x64, &[n(6)]), block(0, &[])),
                always(block(0x28, &[n(9000)])),           // the NPC becomes prototype 9000
                always(block(0x44, &[npc])),               // then it is destroyed
                always(block(1, &[])),
            ];
            library.insert(705, arcanum_formats::script::Script { description: String::new(), flags: 0, lines });
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0 && s.sector == sector).unwrap().0
        };
        sim.take_deltas();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        let sheet = &save.players[&PlayerId(7)].blueprint.sheet;
        let max = RulesData::default().derived(sheet).hit_points;
        assert_eq!((sheet.hp_damage, sheet.poison, sheet.fatigue_damage), (max - 5, 0, 3), "down to 0 hit points, then healed 5");
        assert!(deltas.contains(&(PlayerId(7), LedgerDelta::Written { kind: crate::WRITTEN_PLAQUE as u8, line: 1000 })), "the plaque window");
        assert!(save.scripts.switches.contains(&7) && save.scripts.flag(6));
        assert_eq!(save.sector_changes[&sector].mobile_protos[&0], 9000);
        assert_eq!(save.sector_changes[&sector].mobile_flags[&0] & crate::OF_DESTROYED, crate::OF_DESTROYED);
        assert!(deltas.contains(&(PlayerId(7), LedgerDelta::Despawn { net_id: npc })));
        assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::ObjectTransformed { prototype: 9000, .. })));
        let world = sim.app.world_mut();
        assert_eq!(world.query::<&crate::NpcSource>().iter(world).filter(|s| s.sector == sector && s.index == 0).count(), 0, "destroyed: not spawned again");
    }

    #[test]
    fn putting_on_and_taking_off_an_item_runs_its_wield_scripts() {
        use crate::script::tests::{always, block, n, op};
        use arcanum_formats::mes::MesFile;
        use arcanum_formats::object::{Array, Object, TYPE_ARMOR, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let rules = RulesData::from_files(&MesFile::default(), &MesFile::parse(b"{117}{st +2}").unwrap(), &MesFile::default());
        let mut sim = SimApp::new(rules);
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        // A ring: wield on runs script 710, wield off 711 (events 24 and 25).
        let mut data = Vec::new();
        for v in [0u32, 0, 710, 0, 0, 711] {
            data.extend(v.to_le_bytes());
        }
        let scripts = Array { elem_size: 12, id: 1, data, bitset: vec![1 << 24 | 1 << 25, 0] };
        let mut ring = Object::parse_instance(&ObjectBuilder::new(TYPE_ARMOR).field("item_weight", Value::Int32(1)).field("armor_flags", Value::Int32(0)).field("scripts_idx", Value::Array(Some(scripts))).build()).unwrap();
        ring.id = prototype_id(8000);
        let mut prototypes = Prototypes::default();
        prototypes.insert(ring);
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        let mut s = save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]);
        let mut carried = crate::Inventory::default();
        carried.items.push(crate::CarriedItem { prototype: 8000, location: 0, quantity: 0, keys: vec![], written: 0, wear: Default::default() });
        s.players.get_mut(&PlayerId(7)).unwrap().inventory = Some(carried);
        sim.boot(s);
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            library.insert(710, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(0x4d, &[op(0, 0), n(117), n(5)])), always(block(1, &[]))] });
            library.insert(711, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(0x4e, &[op(0, 0), n(117)])), always(block(1, &[]))] });
        }
        sim.update();
        sim.update();
        let effects = |sim: &SimApp| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(7)].blueprint.sheet.effects.iter().map(|e| e.id).collect::<Vec<_>>();
        assert!(effects(&sim).is_empty(), "carried, not worn");
        let move_ring = |sim: &mut SimApp, to: i32| {
            let world = sim.app.world_mut();
            let mut q = world.query::<(&PlayerControlled, &mut crate::Inventory)>();
            let (_, mut inventory) = q.single_mut(world).unwrap();
            inventory.items[0].location = to;
        };
        move_ring(&mut sim, crate::FIRST_SLOT + 5);
        sim.update();
        assert_eq!(effects(&sim), [117], "worn: its effect");
        sim.update();
        assert_eq!(effects(&sim), [117], "once");
        move_ring(&mut sim, 0);
        sim.update();
        assert!(effects(&sim).is_empty(), "taken off");
    }

    #[test]
    fn items_run_their_insert_remove_and_drop_scripts() {
        use crate::script::tests::{always, block, n, op};
        use arcanum_formats::object::{Array, Object, TYPE_ARMOR, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.app.insert_resource(vfs(dir.path()));
        // A ring: drop runs 730 (keeps it), insert 731 (global var 1 + 1), remove 732 (var 2 + 1).
        let mut data = Vec::new();
        for v in [0u32, 0, 730, 0, 0, 731, 0, 0, 732] {
            data.extend(v.to_le_bytes());
        }
        let scripts = Array { elem_size: 12, id: 1, data, bitset: vec![1 << 5 | 1 << 21 | 1 << 28, 0] };
        let mut ring = Object::parse_instance(&ObjectBuilder::new(TYPE_ARMOR).field("item_weight", Value::Int32(1)).field("armor_flags", Value::Int32(0)).field("scripts_idx", Value::Array(Some(scripts))).build()).unwrap();
        ring.id = prototype_id(8000);
        let mut prototypes = Prototypes::default();
        prototypes.insert(ring);
        sim.app.world_mut().resource_mut::<LoadedMap>().prototypes = Arc::new(prototypes);
        let mut s = save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]);
        let mut carried = crate::Inventory::default();
        carried.items.push(crate::CarriedItem { prototype: 8000, location: 0, quantity: 0, keys: vec![], written: 0, wear: Default::default() });
        s.players.get_mut(&PlayerId(7)).unwrap().inventory = Some(carried);
        sim.boot(s);
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            library.insert(730, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(1, &[]))] });
            library.insert(731, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(0xb, &[op(1, 1), op(1, 1), n(1)]))] });
            library.insert(732, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(0xb, &[op(1, 2), op(1, 2), n(1)]))] });
        }
        sim.update();
        sim.update();
        let var = |sim: &SimApp, i: i32| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().scripts.var(i);
        assert_eq!((var(&sim, 1), var(&sim, 2)), (0, 0), "loading runs nothing");
        // Its drop script keeps it.
        sim.receive([(PlayerId(7), crate::PlayerIntent::Inventory { seq: 1, request: crate::InventoryRequest::Drop { from: 0 } })]);
        sim.update();
        let count = |sim: &mut SimApp| {
            let world = sim.app.world_mut();
            world.query::<(&PlayerControlled, &crate::Inventory)>().single(world).unwrap().1.items.len()
        };
        assert_eq!(count(&mut sim), 1, "kept by its drop script");
        // Gone (a script took it, say), then back.
        let set_items = |sim: &mut SimApp, items: Vec<crate::CarriedItem>| {
            let world = sim.app.world_mut();
            let mut q = world.query::<(&PlayerControlled, &mut crate::Inventory)>();
            q.single_mut(world).unwrap().1.items = items;
        };
        let ring = crate::CarriedItem { prototype: 8000, location: 3, quantity: 0, keys: vec![], written: 0, wear: Default::default() };
        set_items(&mut sim, vec![]);
        sim.update();
        assert_eq!((var(&sim, 1), var(&sim, 2)), (0, 1), "removed");
        set_items(&mut sim, vec![ring.clone()]);
        sim.update();
        assert_eq!((var(&sim, 1), var(&sim, 2)), (1, 1), "inserted");
        set_items(&mut sim, vec![crate::CarriedItem { location: 5, ..ring }]);
        sim.update();
        assert_eq!((var(&sim, 1), var(&sim, 2)), (1, 1), "a move inside runs nothing");
    }

    #[test]
    fn heartbeats_run_first_then_on_a_timer_even_for_an_npc_switched_off() {
        use crate::script::tests::{always, block, line, n, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        let sector = SectorId::new(1, 1);
        let npc_object = crate::ScriptObject::Mobile { sector, index: 0 };
        {
            let mut map = sim.app.world_mut().resource_mut::<LoadedMap>();
            map.set_attachment(npc_object, crate::heartbeat::EVENT_FIRST_HEARTBEAT, crate::Attachment::new(0, 0, 720));
            map.set_attachment(npc_object, crate::heartbeat::EVENT_HEARTBEAT, crate::Attachment::new(0, 0, 721));
            let flags = map.object_flags(sector, 0, true).unwrap();
            map.set_object_flags(sector, 0, true, flags | crate::OF_OFF);
        }
        sim.app.world_mut().resource_mut::<crate::world::ToggledNpcs>().0.push((sector, 0));
        sim.app.world_mut().resource_mut::<crate::heartbeat::Heartbeats>().seen.clear();
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            // First heartbeat: count it in global var 1; run the default (event 19 follows).
            library.insert(720, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(0xb, &[op(1, 1), op(1, 1), n(1)])), always(block(2, &[]))] });
            // Heartbeat: count in global var 2; the first time, switch itself on.
            let lines = vec![always(block(0xb, &[op(1, 2), op(1, 2), n(1)])), line(block(4, &[op(1, 2), n(1)]), block(0x38, &[op(1, 0)]), block(0, &[])), always(block(1, &[]))];
            library.insert(721, arcanum_formats::script::Script { description: String::new(), flags: 0, lines });
        }
        let var = |sim: &SimApp, i: i32| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().scripts.var(i);
        sim.update();
        sim.update();
        assert_eq!((var(&sim, 1), var(&sim, 2)), (1, 1), "first heartbeat, then a heartbeat, at once");
        assert_eq!(sim.app.world().resource::<LoadedMap>().object_flags(sector, 0, true).unwrap() & crate::OF_OFF, 0, "it switched itself on");
        // Next beats: at most 250 + 4750 x 1 / 30 + 5000 ms later.
        for _ in 0..(5500 * 30 / 1000) {
            sim.update();
        }
        assert_eq!(var(&sim, 1), 1, "the first heartbeat runs once");
        assert!(var(&sim, 2) >= 2, "{}", var(&sim, 2));
    }

    #[test]
    fn a_killed_npc_lies_dead_and_says_nothing() {
        use crate::script::tests::{always, block, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        let sector = SectorId::new(1, 1);
        let npc_object = crate::ScriptObject::Mobile { sector, index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 722));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            library.insert(722, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(0x3a, &[op(1, 0)])), always(block(1, &[]))] });
        }
        let find = |sim: &mut SimApp| {
            let world = sim.app.world_mut();
            world.query::<(Entity, &NetId, &crate::NpcSource)>().iter(world).find(|(_, _, s)| s.index == 0 && s.sector == sector).map(|(e, n, _)| (e, *n))
        };
        let (_, npc) = find(&mut sim).unwrap();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        sim.update();
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert!(save.sector_changes[&sector].dead.contains(&0));
        let (corpse, again) = find(&mut sim).expect("spawned again, as a corpse");
        assert_ne!(again, npc);
        let art = sim.app.world().get::<crate::Appearance>(corpse).unwrap().art;
        assert_eq!((art.0 >> 6) & 0x1f, crate::script_host::ANIM_DEATH);
        assert!(sim.app.world().get::<crate::Standpoints>(corpse).is_none(), "it does not wander");
        sim.take_deltas();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc: again, seq: 1 })]);
        sim.update();
        assert!(!sim.take_deltas().iter().any(|(_, d)| matches!(d, LedgerDelta::DialogueOptions { .. } | LedgerDelta::FloatText { .. })), "the dead say nothing");
    }

    #[test]
    fn followers_give_up_waiting_and_leave_a_leader_they_dislike() {
        use crate::follow::{Follower, LEFT, WAITING};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let sector = SectorId::new(1, 1);
        let mut s = save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]);
        let now = s.time;
        // NPC 0 waits until a moment from now.
        s.followers.push(Follower { map: MAP.into(), sector, index: 0, leader: PlayerId(7), away: None, wait_until: Some(now.plus_ms(1)), warn: 0 });
        s.sector_changes.insert(sector, crate::SectorDelta { npc_flags: [(0, WAITING)].into(), ..Default::default() });
        let mut sim = booted(dir.path(), s);
        for _ in 0..3 {
            sim.update();
        }
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert!(!save.followers.iter().any(|f| f.sector == sector && f.index == 0), "gave up");
        assert_eq!(save.sector_changes[&sector].npc_flags[&0], LEFT);
        let key = crate::met_key(crate::ScriptObject::Mobile { sector, index: 0 });
        assert_eq!(save.players[&PlayerId(7)].script_state.reactions[&key], crate::BASE_REACTION + 2 * (8 - 20), "charisma 8");
        // A follower who loathes its leader leaves at its next check.
        {
            let mut world = sim.app.world_mut().resource_mut::<WorldState>();
            let save = world.0.as_mut().unwrap();
            save.followers.push(Follower { map: MAP.into(), sector, index: 0, leader: PlayerId(7), away: None, wait_until: None, warn: 0 });
            save.players.get_mut(&PlayerId(7)).unwrap().script_state.reactions.insert(key.clone(), -100);
        }
        sim.app.world_mut().resource_mut::<LoadedMap>().set_npc_flags(sector, 0, 0);
        for _ in 0..(crate::follow::CHECK_TICKS + 2) {
            sim.update();
        }
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert!(!save.followers.iter().any(|f| f.sector == sector && f.index == 0), "left");
        assert_eq!(sim.app.world().resource::<LoadedMap>().npc_flags(sector, 0).unwrap() & LEFT, LEFT);
    }

    #[test]
    fn fate_points_heal_at_once_and_force_the_next_reaction() {
        use crate::script::tests::{always, block, n};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut s = save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]);
        let sheet = &mut s.players.get_mut(&PlayerId(7)).unwrap().blueprint.sheet;
        (sheet.fate_points, sheet.hp_damage) = (2, 5);
        let mut sim = booted(dir.path(), s);
        sim.receive([(PlayerId(7), crate::PlayerIntent::Fate { option: 0, on: true }), (PlayerId(7), crate::PlayerIntent::Fate { option: crate::FATE_GOOD_REACTION, on: true })]);
        sim.update();
        let sheet = |sim: &SimApp| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(7)].blueprint.sheet.clone();
        assert_eq!((sheet(&sim).fate_points, sheet(&sim).hp_damage, sheet(&sim).fate_pending), (0, 0, 1));
        // The next talk starts at reaction 100, and the use is spent.
        let sector = SectorId::new(1, 1);
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(crate::ScriptObject::Mobile { sector, index: 0 }, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 706));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            library.insert(706, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(4, &[n(1)]))] });
            library.insert_dialog(706, arcanum_formats::dialog::DialogFile::parse(b"{1}{Hello.}{}{}{}{}{}"));
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0 && s.sector == sector).unwrap().0
        };
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        assert_eq!(sheet(&sim).fate_pending, 0);
        let key = crate::met_key(crate::ScriptObject::Mobile { sector, index: 0 });
        let stored = sim.app.world().resource::<WorldState>().0.as_ref().unwrap().players[&PlayerId(7)].script_state.reactions[&key];
        assert!(stored >= 100 - 20, "stored so the reaction is 100: {stored}");
    }

    #[test]
    fn a_script_called_in_seconds_runs_once_the_game_clock_gets_there() {
        use crate::script::tests::{always, block, n, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        let npc_object = crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 700));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            // In 1 second (8 game seconds): script 701 from line 0, which sets global flag 3.
            library.insert(700, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(0x36, &[n(701), n(0), op(0, 0), op(1, 0), n(1)])), always(block(1, &[]))] });
            library.insert(701, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(0x64, &[n(3)])), always(block(1, &[]))] });
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let flag = |sim: &SimApp| sim.app.world().resource::<WorldState>().0.as_ref().unwrap().scripts.flag(3);
        assert!(!flag(&sim), "not yet");
        assert_eq!(sim.app.world().resource::<crate::script_host::TimedScripts>().0.len(), 1);
        sim.app.world_mut().resource_mut::<crate::GameClock>().jump(8_000);
        sim.update();
        assert!(flag(&sim), "8 game seconds on");
        assert!(sim.app.world().resource::<crate::script_host::TimedScripts>().0.is_empty());
    }

    #[test]
    fn scripts_set_reactions_and_measure_distances() {
        use crate::script::tests::{always, block, n, op};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        let npc_object = crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 700));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            let lines = vec![
                always(block(0x1e, &[op(1, 0), op(0, 0), n(70)])),     // set to 70
                always(block(0x1f, &[op(1, 0), op(0, 0), n(-5)])),     // then -5
                always(block(0x1d, &[op(1, 0), op(0, 0), op(1, 1)])),  // global var 1 = reaction
                always(block(0x47, &[op(0, 0), op(1, 0), op(1, 2)])),  // global var 2 = distance
                always(block(1, &[])),
            ];
            library.insert(700, arcanum_formats::script::Script { description: String::new(), flags: 0, lines });
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let globals = sim.app.world().resource::<WorldState>().0.as_ref().unwrap().scripts.clone();
        assert_eq!(globals.var(1), 65, "set to 70, then -5");
        assert_eq!(globals.var(2), 1, "the NPC at (80, 80), the PC at (79, 80)");
    }

    #[test]
    fn an_npc_trains_a_skill_to_apprentice_for_gold() {
        use crate::script::tests::{always, block, n};
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        std::fs::create_dir_all(dir.path().join("mes")).unwrap();
        std::fs::write(dir.path().join("mes/gd_cls_m2m.mes"), "{1000}{%d coins.}{3000}{What shall I teach?}{6000}{Done.}").unwrap();
        std::fs::write(dir.path().join("mes/gd_pc2m.mes"), "{1}{Yes.}{100}{No.}{500}{Train me.}{800}{Forget it.}{1000}{Thank you.}").unwrap();
        std::fs::write(dir.path().join("mes/skill.mes"), "{9}{Haggle}").unwrap();
        let mut s = save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]);
        let player = s.players.get_mut(&PlayerId(7)).unwrap();
        player.blueprint.sheet.skills[9] = 1;
        let mut gold = crate::Inventory::default();
        gold.change_gold(500);
        player.inventory = Some(gold);
        let mut sim = booted(dir.path(), s);
        let npc_object = crate::ScriptObject::Mobile { sector: SectorId::new(1, 1), index: 0 };
        sim.app.world_mut().resource_mut::<LoadedMap>().set_attachment(npc_object, crate::EVENT_DIALOGUE, crate::Attachment::new(0, 0, 700));
        {
            let mut scripts = sim.app.world_mut().resource_mut::<crate::Scripts>();
            let library = scripts.0.get_or_insert_with(Default::default);
            library.insert(700, arcanum_formats::script::Script { description: String::new(), flags: 0, lines: vec![always(block(4, &[n(1)]))] });
            library.insert_dialog(700, arcanum_formats::dialog::DialogFile::parse(b"{1}{Hello.}{}{}{}{}{}\n{2}{T:9}{}{1}{}{0}{}"));
        }
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        sim.take_deltas();
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        for (option, expect) in [(0, "Haggle"), (0, "Yes."), (0, "Thank you.")] {
            sim.receive([(PlayerId(7), crate::PlayerIntent::Choose { option })]);
            sim.update();
            let deltas = sim.take_deltas();
            assert!(deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::DialogueOptions { options, .. } if options[0] == expect)), "{expect}: {deltas:?}");
            if expect == "Thank you." {
                let sheet = deltas.iter().find_map(|(_, d)| match d { LedgerDelta::SheetUpdate { sheet, .. } => Some(sheet.clone()), _ => None }).expect("the sheet goes to the client");
                assert_eq!(sheet.training[9], 1, "Apprentice");
                let gold = deltas.iter().find_map(|(_, d)| match d { LedgerDelta::Inventory { inventory, .. } => Some(inventory.gold()), _ => None }).unwrap();
                assert!((300..500).contains(&gold), "{gold}");
            }
        }
        let save = sim.app.world().resource::<WorldState>().0.clone().unwrap();
        assert_eq!(save.players[&PlayerId(7)].blueprint.sheet.training[9], 1, "saved");
    }

    #[test]
    fn an_npc_without_a_dialogue_script_barks_a_greeting() {
        let dir = tempfile::tempdir().unwrap();
        write_map(dir.path());
        std::fs::create_dir_all(dir.path().join("mes")).unwrap();
        for pair in ["m2m", "m2f"] {
            std::fs::write(dir.path().join(format!("mes/gd_cls_{pair}.mes")), "{10000}{Good day.}").unwrap();
        }
        let mut sim = booted(dir.path(), save(&[(7, "Virgil", TileLoc { x: 79, y: 80 })]));
        sim.take_deltas();
        let npc = {
            let world = sim.app.world_mut();
            *world.query::<(&NetId, &crate::NpcSource)>().iter(world).find(|(_, s)| s.index == 0).unwrap().0
        };
        sim.receive([(PlayerId(7), crate::PlayerIntent::Talk { npc, seq: 0 })]);
        sim.update();
        let deltas = sim.take_deltas();
        let float = LedgerDelta::FloatText { speaker: npc, text: "Good day.".into(), colour: crate::FloatColour::White, voice: None };
        assert!(deltas.contains(&(PlayerId(7), float)), "a first, neutral greeting: {deltas:?}");
        assert!(!deltas.iter().any(|(_, d)| matches!(d, LedgerDelta::DialogueOptions { .. } | LedgerDelta::DialogueEnded)), "no conversation");
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
