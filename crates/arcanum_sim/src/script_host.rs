//! The host's side of the script engine: which object a script runs for, where its attachment
//! is kept, and the world it sees ([`crate::script`] is the runner).
//!
//! - **Attachments** come from the object's `scripts_idx` (field 0x20, its own or its
//!   prototype's): a sparse array whose bitset marks the events present, one 12-byte
//!   `{flags, counters, number}` entry for each, in order ([`attachments`]). Once a script
//!   changes one (its flags, counters or number), the new one is kept in the sector's changes
//!   and saved ([`crate::SectorDelta::object_scripts`] / `mobile_scripts`).
//! - **The world:** global state is the save's [`crate::ScriptGlobals`]; a PC's variables and
//!   flags are its [`crate::PlayerSave::script_state`]; random numbers come from the
//!   [`crate::HostRng`]; day is 6:00 to 17:59. Lines a script shows (action 0x18) go to the
//!   PC's player as [`crate::LedgerDelta::ScriptMessage`], in the text for the PC's gender.
//! - **Events** wired so far: [`EVENT_USE`] on doors ([`crate::use_portals`]) and containers
//!   ([`crate::pick_up_items`]), before their default (0x428c17 runs the hook before the use).
//! - Unsupported condition and action types met in the game are kept in [`ScriptReports`].

use std::collections::BTreeMap;
use std::sync::Arc;

use arcanum_formats::object::{Object, Prototypes, Value};
use arcanum_formats::script::Script;
use bevy_ecs::prelude::*;
use bevy_ecs::system::SystemParam;

use crate::{Attachment, GroundItemId, HookResult, Invocation, LoadedMap, MapObjectId, PcScriptState, PlayerId, ScriptContext, ScriptGlobals, ScriptLibrary, WorldState};

/// The "use" event (attach point 1).
pub const EVENT_USE: u32 = 1;
/// Reports kept, newest last.
pub const REPORTS_KEPT: usize = 64;

/// An object a script runs for, or that it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptObject {
    /// A player's PC.
    Pc(PlayerId),
    /// An object of a sector file (doors, scenery).
    Map(MapObjectId),
    /// One of the map's mobile objects (containers, items on the ground, NPCs).
    Mobile { sector: arcanum_formats::map::SectorId, index: u32 },
    /// An item a PC carries, by its prototype (its scripts are the prototype's).
    Carried { holder: PlayerId, prototype: u32 },
    /// An item an NPC carries, by its prototype (its scripts are the prototype's).
    NpcCarried { sector: arcanum_formats::map::SectorId, index: u32, prototype: u32 },
}

impl ScriptObject {
    /// A container or a map item on the ground.
    pub fn ground(id: GroundItemId) -> Option<Self> {
        match id {
            GroundItemId::Container { sector, index } | GroundItemId::Mobile { sector, index } => Some(Self::Mobile { sector, index }),
            _ => None,
        }
    }
}

/// `object`'s attachments by event: its own `scripts_idx`, else its prototype's.
pub fn attachments(object: &Object, prototypes: &Prototypes) -> BTreeMap<u32, Attachment> {
    let array = |o: &Object| match o.field("scripts_idx") {
        Some(Value::Array(Some(a))) => Some(a.clone()),
        _ => None,
    };
    let Some(array) = array(object).or_else(|| prototypes.of(object).and_then(array)) else { return BTreeMap::new() };
    if array.elem_size != 12 {
        return BTreeMap::new();
    }
    let events = array.bitset.iter().enumerate().flat_map(|(w, &word)| (0..32).filter(move |b| word >> b & 1 != 0).map(move |b| w as u32 * 32 + b));
    let entries = array.data.as_chunks::<12>().0.iter();
    events
        .zip(entries)
        .map(|(event, e)| {
            let u = |o: usize| u32::from_le_bytes(e[o..o + 4].try_into().expect("4 bytes"));
            (event, Attachment::new(u(0), u(4), u(8)))
        })
        .collect()
}

impl LoadedMap {
    /// The attachment `object` has for `event` now: changed by a script, else from the data.
    pub fn attachment(&self, object: ScriptObject, event: u32) -> Attachment {
        let (sector, index, data) = match object {
            ScriptObject::Pc(_) | ScriptObject::Carried { .. } | ScriptObject::NpcCarried { .. } => return Attachment::default(),
            ScriptObject::Map(id) => (id.sector, id.index, self.map_object(id)),
            ScriptObject::Mobile { sector, index } => (sector, index, self.mobiles.get(&sector).and_then(|m| m.get(index as usize))),
        };
        let changed = self.changes.get(&sector).and_then(|c| match object {
            ScriptObject::Map(_) => c.object_scripts.get(&index),
            _ => c.mobile_scripts.get(&index),
        });
        if let Some(a) = changed.and_then(|c| c.get(&event)) {
            return *a;
        }
        data.and_then(|o| attachments(o, &self.prototypes).get(&event).copied()).unwrap_or_default()
    }

    /// Keeps a changed attachment in the sector's changes.
    pub fn set_attachment(&mut self, object: ScriptObject, event: u32, attachment: Attachment) {
        let (sector, index) = match object {
            ScriptObject::Pc(_) | ScriptObject::Carried { .. } | ScriptObject::NpcCarried { .. } => return,
            ScriptObject::Map(id) => (id.sector, id.index),
            ScriptObject::Mobile { sector, index } => (sector, index),
        };
        let changes = self.changes.entry(sector).or_default();
        let by_object = match object {
            ScriptObject::Map(_) => &mut changes.object_scripts,
            _ => &mut changes.mobile_scripts,
        };
        by_object.entry(index).or_default().insert(event, attachment);
    }
}

/// The script library, built from the game data on first use.
#[derive(Resource, Debug, Default)]
pub struct Scripts(pub Option<ScriptLibrary>);

/// Scripts that met a type the runner does not have yet: the script and what it met.
#[derive(Resource, Debug, Default)]
pub struct ScriptReports(pub Vec<(u32, crate::Unsupported)>);

/// What the host needs to run a script.
#[derive(SystemParam)]
pub struct ScriptHost<'w, 's> {
    scripts: ResMut<'w, Scripts>,
    started: ResMut<'w, crate::StartedDialogues>,
    critters: Query<'w, 's, CritterQuery<'static>>,
    talks: Query<'w, 's, (&'static crate::PlayerControlled, &'static crate::Conversation)>,
    rules: Res<'w, crate::SimRules>,
    reputations: ResMut<'w, crate::ReputationTable>,
    timed: ResMut<'w, TimedScripts>,
    areas: ResMut<'w, crate::areas::AreaTable>,
    changed_inventories: ResMut<'w, ScriptInventories>,
    changed_sheets: ResMut<'w, ScriptSheets>,
    toggled_npcs: ResMut<'w, crate::world::ToggledNpcs>,
    teleports: ResMut<'w, crate::big_move::ScriptTeleports>,
    orders: ResMut<'w, ScriptOrders>,
    reports: ResMut<'w, ScriptReports>,
    rng: ResMut<'w, crate::HostRng>,
    clock: Res<'w, crate::GameClock>,
    session: Res<'w, crate::Session>,
    awards: ResMut<'w, crate::ExperienceAwards>,
    vfs: Option<Res<'w, crate::SimVfs>>,
}

/// How far the vicinity reaches (sets 8 and 9), in tiles (builder's call: the original takes a
/// view-sized rectangle round the attachee, 0x440ff0).
pub const VICINITY: u32 = 10;

/// What the script host needs of the critters: their ids, tiles, and who they are.
type CritterQuery<'a> = (&'a crate::NetId, &'a crate::Position, Option<&'a crate::PlayerControlled>, Option<&'a crate::NpcSource>, Option<&'a crate::follow::Following>, Has<crate::Prowling>);

/// The world as a running script sees it.
struct HostWorld<'a> {
    library: &'a mut ScriptLibrary,
    map: &'a mut LoadedMap,
    /// The acting player (set 0x15).
    player: PlayerId,
    /// The critters: object, tile, id.
    critters: Vec<(ScriptObject, arcanum_formats::map::TileLoc, crate::NetId)>,
    /// The NPCs following, and whom.
    following: Vec<(ScriptObject, PlayerId)>,
    /// Who is talking: PCs and the NPCs they talk to.
    talking: Vec<ScriptObject>,
    /// PCs prowling ([`crate::prowl`]).
    prowling: Vec<ScriptObject>,
    /// NPCs made to follow (a leader) or to stop (`None`).
    follow_changes: Vec<((arcanum_formats::map::SectorId, u32), Option<PlayerId>)>,
    /// PCs whose alignment a script moved, and whether up (their followers warn).
    alignment_moves: Vec<(PlayerId, bool)>,
    /// Lines floated over objects.
    floats: Vec<(crate::NetId, String)>,
    /// PCs whose inventory a script changed.
    inventories: std::collections::BTreeSet<PlayerId>,
    /// PCs whose sheet a script changed.
    sheets: std::collections::BTreeSet<PlayerId>,
    /// Objects switched off or on, for every player.
    flag_changes: Vec<crate::LedgerDelta>,
    /// Mobile objects switched, for NPCs to go or come.
    npcs_toggled: Vec<(arcanum_formats::map::SectorId, u32)>,
    /// Sectors whose changes a script touched.
    sectors: std::collections::BTreeSet<arcanum_formats::map::SectorId>,
    rules: &'a arcanum_rules::RulesData,
    reputations: &'a crate::ReputationEffects,
    areas: &'a crate::areas::AreaTable,
    /// Scripts to run later (action 0x36).
    scheduled: Vec<TimedScript>,
    /// Scheduled runs cancelled (action 0x7d): script and object.
    cancelled: Vec<(u32, ScriptObject)>,
    vfs: Option<&'a arcanum_vfs::Vfs>,
    globals: &'a mut ScriptGlobals,
    players: &'a mut BTreeMap<PlayerId, crate::PlayerSave>,
    rng: &'a mut crate::HostRng,
    now: crate::GameTime,
    /// Lines for players' message boxes.
    messages: Vec<(PlayerId, String)>,
    /// Written windows to open: player, kind, line.
    written: Vec<(PlayerId, u8, i32)>,
    /// Items made near objects (action 0x34): prototype, tile.
    created: Vec<(u32, arcanum_formats::map::TileLoc)>,
    /// Experience given (action 0x3f), shared as dialogue's is.
    experience: Vec<i32>,
    /// Conversations scripts started.
    dialogs: Vec<crate::StartedDialogue>,
    /// Teleports asked for: object, map, tile.
    teleports: Vec<(ScriptObject, i32, arcanum_formats::map::TileLoc)>,
    /// Walks and skill uses a script ordered (actions 0x45, 0x4f).
    orders: ScriptOrders,
}

/// What scripts ordered critters to do, for [`apply_script_orders`].
#[derive(Resource, Debug, Default)]
pub struct ScriptOrders {
    /// Critters to walk to a tile (action 0x45).
    pub walks: Vec<(crate::NetId, arcanum_formats::map::TileLoc)>,
    /// PCs to pick a lock (action 0x4f with Pick Locks), with the tool's prototype.
    pub picks: Vec<(PlayerId, crate::LockTarget, Option<u32>)>,
    /// NPCs to fight a critter (action 0x24).
    pub fights: Vec<(crate::NetId, crate::NetId)>,
}

type OrderedQuery<'a> = (Entity, &'a crate::NetId, &'a crate::Position, Option<&'a crate::PlayerControlled>, Option<&'a crate::Inventory>);

/// Starts the walks and picks scripts ordered: a walk on a path from where the critter stands
/// (none found: it stays); a pick as the player's own, with the PC's first item of the tool's
/// prototype as the tool.
pub fn apply_script_orders(
    mut commands: Commands,
    mut orders: ResMut<ScriptOrders>,
    map: Res<LoadedMap>,
    mut picks: ResMut<crate::PickLockRequests>,
    mut fights: ResMut<crate::combat::CombatStarts>,
    critters: Query<OrderedQuery>,
) {
    fights.0.append(&mut orders.fights);
    for (net_id, to) in std::mem::take(&mut orders.walks) {
        let Some((entity, _, position, ..)) = critters.iter().find(|c| *c.1 == net_id) else { continue };
        match crate::find_path(&*map, position.0, to) {
            Ok(path) if !path.is_empty() => {
                commands.entity(entity).insert(crate::Walking { steps: path.into(), progress: 0.0 });
            }
            _ => {}
        }
    }
    for (player, target, tool) in std::mem::take(&mut orders.picks) {
        let inventory = critters.iter().find(|c| c.3.is_some_and(|pc| pc.0 == player)).and_then(|c| c.4);
        let tool = tool.and_then(|t| inventory?.items.iter().find(|i| i.prototype == t).map(|i| i.location));
        picks.0.push((player, target, tool, 0));
    }
}

impl ScriptContext for HostWorld<'_> {
    type Object = ScriptObject;

    fn script(&mut self, number: u32) -> Option<Arc<Script>> {
        self.library.get(self.vfs?, number)
    }

    fn globals(&mut self) -> &mut ScriptGlobals {
        self.globals
    }

    fn pc_state(&mut self, object: ScriptObject) -> Option<&mut PcScriptState> {
        let ScriptObject::Pc(player) = object else { return None };
        self.players.get_mut(&player).map(|p| &mut p.script_state)
    }

    fn random(&mut self, low: i32, high: i32) -> i32 {
        self.rng.range(low, high)
    }

    fn now(&self) -> crate::GameTime {
        self.now
    }

    fn dialog_text(&mut self, script: u32, line: i32, reader: ScriptObject) -> Option<String> {
        let female = match reader {
            ScriptObject::Pc(player) => self.players.get(&player).is_some_and(|p| p.blueprint.sheet.gender == arcanum_rules::FEMALE),
            _ => false,
        };
        let dialog = self.library.dialog(self.vfs?, script)?;
        dialog.text(line, female).map(str::to_string)
    }

    fn show_message(&mut self, to: ScriptObject, text: String) {
        if let ScriptObject::Pc(player) = to {
            self.messages.push((player, text));
        }
    }

    fn object_int(&mut self, object: ScriptObject, field: &str) -> Option<i32> {
        let data = match object {
            ScriptObject::Pc(_) => None,
            ScriptObject::Carried { prototype, .. } | ScriptObject::NpcCarried { prototype, .. } => return self.map.prototypes.get(prototype)?.int(field),
            ScriptObject::Map(id) => self.map.map_object(id),
            ScriptObject::Mobile { sector, index } => self.map.mobiles.get(&sector).and_then(|m| m.get(index as usize)),
        }?;
        self.map.prototypes.int(data, field)
    }

    fn is_npc(&mut self, object: ScriptObject) -> bool {
        matches!(object, ScriptObject::Mobile { sector, index } if self.map.mobiles.get(&sector).and_then(|m| m.get(index as usize)).is_some_and(|o| o.object_type == arcanum_formats::object::TYPE_NPC))
    }

    fn start_dialog(&mut self, attachee: ScriptObject, triggerer: ScriptObject, dialog: u32, line: i32) {
        // A PC talks to the other party.
        let (pc, npc) = match (attachee, triggerer) {
            (_, ScriptObject::Pc(p)) => (p, attachee),
            (ScriptObject::Pc(p), _) => (p, triggerer),
            _ => return,
        };
        self.dialogs.push(crate::StartedDialogue { player: pc, npc, dialog, line });
    }

    fn teleport(&mut self, object: ScriptObject, map: i32, to: arcanum_formats::map::TileLoc) {
        self.teleports.push((object, map, to));
    }

    fn gold(&mut self, object: ScriptObject) -> i32 {
        self.inventory(object).map_or(0, |i| i.gold())
    }

    fn change_gold(&mut self, object: ScriptObject, n: i32) {
        let Some(mut inventory) = self.inventory(object) else { return };
        if n < 0 && inventory.gold() < -n {
            return;
        }
        inventory.change_gold(n);
        self.set_inventory(object, inventory);
    }

    fn has_item_named(&mut self, object: ScriptObject, name: i32) -> bool {
        let Some(inventory) = self.inventory(object) else { return false };
        inventory.items.iter().any(|i| self.item_name(i.prototype) == Some(name))
    }

    fn transfer_item_named(&mut self, name: i32, from: ScriptObject, to: ScriptObject) {
        let (Some(source), Some(target)) = (self.inventory(from), self.inventory(to)) else { return };
        let Some(item) = source.items.iter().find(|i| self.item_name(i.prototype) == Some(name)).cloned() else { return };
        let Some((rest, item)) = source.take_out(item.location) else { return };
        let mut target = target;
        let placed = target.insert_item(crate::CarriedItem { location: 0, ..item }, self.item_facts(), i32::MAX, 0).is_ok();
        if !placed {
            return;
        }
        self.set_inventory(from, rest);
        self.set_inventory(to, target);
    }

    fn create_item(&mut self, basic: i32, into: ScriptObject) {
        let Some(mut target) = self.inventory(into) else { return };
        let Ok(basic) = u32::try_from(basic) else { return };
        let placed = target.insert(basic + crate::BASIC_TO_PROTOTYPE, 1, self.item_facts(), i32::MAX, 0).is_ok();
        if placed {
            self.set_inventory(into, target);
        }
    }

    fn object_type(&mut self, object: ScriptObject) -> i32 {
        match object {
            ScriptObject::Pc(_) => arcanum_formats::object::TYPE_PC as i32,
            ScriptObject::Carried { prototype, .. } | ScriptObject::NpcCarried { prototype, .. } => self.map.prototypes.get(prototype).map_or(0, |p| p.object_type as i32),
            ScriptObject::Map(id) => self.map.map_object(id).map_or(0, |o| o.object_type as i32),
            ScriptObject::Mobile { sector, index } => self.map.mobiles.get(&sector).and_then(|m| m.get(index as usize)).map_or(0, |o| o.object_type as i32),
        }
    }

    fn float_text(&mut self, object: ScriptObject, text: String) {
        if let Some(&(_, _, net_id)) = self.critters.iter().find(|c| c.0 == object) {
            self.floats.push((net_id, text));
        }
    }

    fn attachment(&mut self, object: ScriptObject, event: u32) -> crate::Attachment {
        self.map.attachment(object, event)
    }

    fn set_attachment(&mut self, object: ScriptObject, event: u32, attachment: crate::Attachment) {
        if self.map.attachment(object, event) == attachment {
            return;
        }
        self.map.set_attachment(object, event, attachment);
        match object {
            ScriptObject::Map(id) => {
                self.sectors.insert(id.sector);
            }
            ScriptObject::Mobile { sector, .. } => {
                self.sectors.insert(sector);
            }
            ScriptObject::Pc(_) | ScriptObject::Carried { .. } | ScriptObject::NpcCarried { .. } => {}
        }
    }

    fn near(&mut self, centre: ScriptObject, kind: crate::NearKind) -> Vec<ScriptObject> {
        use arcanum_formats::object::{TYPE_CONTAINER, TYPE_PORTAL, TYPE_SCENERY};
        let Some(at) = self.tile_of(centre) else { return Vec::new() };
        let close = |there: Option<arcanum_formats::map::TileLoc>| there.is_some_and(|t| crate::within_radius(at, t, VICINITY));
        match kind {
            crate::NearKind::Critters => self.critters.iter().filter(|c| close(Some(c.1))).map(|c| c.0).collect(),
            crate::NearKind::Containers | crate::NearKind::Items => {
                let item = |t: u32| (5..=14).contains(&t);
                let mut out = Vec::new();
                for (&sector, objects) in &self.map.mobiles {
                    for (index, o) in objects.iter().enumerate() {
                        let wanted = if kind == crate::NearKind::Containers { o.object_type == TYPE_CONTAINER } else { item(o.object_type) };
                        if wanted && close(crate::object_location(o)) {
                            out.push(ScriptObject::Mobile { sector, index: index as u32 });
                        }
                    }
                }
                out
            }
            crate::NearKind::Scenery | crate::NearKind::Portals => {
                let wanted = if kind == crate::NearKind::Scenery { TYPE_SCENERY } else { TYPE_PORTAL };
                let mut out = Vec::new();
                for (&sector, loaded) in &self.map.sectors {
                    for (index, o) in loaded.objects.iter().enumerate() {
                        if o.object_type == wanted && close(crate::object_location(o)) {
                            out.push(ScriptObject::Map(MapObjectId { sector, index: index as u32 }));
                        }
                    }
                }
                out
            }
        }
    }

    /// PCs take it on their sheet (hit points at most to 0: death is not built); NPCs' damage
    /// is not kept (builder's call: they have no hit point state yet). Armour and resistances
    /// are not applied.
    fn damage(&mut self, object: ScriptObject, amount: i32, kind: i32) {
        if let ScriptObject::Mobile { sector, index } = object {
            self.damage_npc(sector, index, amount.max(0), kind);
            return;
        }
        let ScriptObject::Pc(p) = object else { return };
        let Some(saved) = self.players.get_mut(&p) else { return };
        let sheet = &mut saved.blueprint.sheet;
        let amount = amount.max(0);
        match kind {
            1 => sheet.poison += amount,
            4 => sheet.fatigue_damage += amount,
            _ => {
                let max = self.rules.derived(sheet).hit_points;
                sheet.hp_damage = (sheet.hp_damage + amount).min(max);
            }
        }
        self.sheets.insert(p);
    }

    /// The healing spells on a PC, with no caster's aptitude: Minor / Major Healing heal 5 / 40,
    /// Halt Poison takes 50 poison. Other spells cast this way are not built (the script goes
    /// on).
    fn cast_on(&mut self, spell: i32, target: ScriptObject) {
        let ScriptObject::Pc(p) = target else { return };
        let Some(saved) = self.players.get_mut(&p) else { return };
        let sheet = &mut saved.blueprint.sheet;
        match u32::try_from(spell).unwrap_or(u32::MAX) {
            crate::HALT_POISON => sheet.poison = (sheet.poison - 50).max(0),
            s @ (crate::MINOR_HEALING | crate::MAJOR_HEALING) => sheet.hp_damage = (sheet.hp_damage - crate::spell_heal(s, 0)).max(0),
            _ => return,
        }
        self.sheets.insert(p);
    }

    /// Objects of the map only: kept in the sector's changes, sent to every player; an NPC is
    /// spawned again as the new prototype, unhurt (its damage is not kept).
    fn transform(&mut self, object: ScriptObject, prototype: i32) {
        let (Some((sector, index, mobile)), Ok(prototype)) = (map_place(object), u32::try_from(prototype)) else { return };
        if self.map.transform_object(sector, index, mobile, prototype) {
            self.sectors.insert(sector);
            self.flag_changes.push(crate::LedgerDelta::ObjectTransformed { sector, index, mobile, prototype });
            if mobile {
                self.npcs_toggled.push((sector, index));
            }
        }
    }

    /// The player's written window opens on it ([`crate::LedgerDelta::Written`]); the client reads
    /// the text.
    fn show_text(&mut self, kind: i32, line: i32, to: ScriptObject) {
        let (ScriptObject::Pc(p), Ok(kind)) = (to, u8::try_from(kind)) else { return };
        self.written.push((p, kind, line));
    }

    /// Objects of the map: the number field of the art id by its type (0x502780), frame 0
    /// (0x502c40); kept only when that art exists (0x502220), and sent to every player.
    fn set_art_number(&mut self, object: ScriptObject, number: i32) {
        let Some((sector, index, mobile)) = map_place(object) else { return };
        let Some(old) = self.map.object_at(sector, index, mobile).and_then(|o| self.map.prototypes.art_id(o)) else { return };
        let Some(art) = with_art_number(old, number) else { return };
        let art = crate::with_frame(art, 0);
        let exists = self.map.art.as_deref().and_then(|a| a.path(art)).is_some_and(|p| self.vfs.is_some_and(|v| v.exists(&p)));
        if !exists {
            return;
        }
        self.map.set_object_art(sector, index, mobile, art.0);
        self.sectors.insert(sector);
        self.flag_changes.push(crate::LedgerDelta::ObjectArt { sector, index, mobile, art: art.0 });
        if mobile {
            self.npcs_toggled.push((sector, index));
        }
    }

    /// PCs: the rules' level; NPCs: points × 4, at most 20 (their stats do not cap it here).
    fn skill_level(&mut self, object: ScriptObject, k: i32) -> i32 {
        let Ok(k) = usize::try_from(k) else { return 0 };
        if k >= arcanum_rules::SKILLS {
            return 0;
        }
        match object {
            ScriptObject::Pc(p) => self.players.get(&p).map_or(0, |s| self.rules.skill_level(&s.blueprint.sheet, k)),
            ScriptObject::Mobile { sector, index } => {
                let Some(o) = self.map.mobiles.get(&sector).and_then(|m| m.get(index as usize)) else { return 0 };
                let (field, i) = if k < 12 { ("critter_basic_skill_idx", k) } else { ("critter_tech_skill_idx", k - 12) };
                let raw = o.ints(field).or_else(|| self.map.prototypes.of(o)?.ints(field)).and_then(|v| v.get(i).copied()).unwrap_or(0);
                ((raw & 0x3f) * arcanum_rules::LEVELS_PER_POINT).min(arcanum_rules::MAX_SKILL_LEVEL)
            }
            ScriptObject::Map(_) | ScriptObject::Carried { .. } | ScriptObject::NpcCarried { .. } => 0,
        }
    }

    /// A PC with no hit points left; an NPC killed.
    fn is_dead(&mut self, object: ScriptObject) -> bool {
        match object {
            ScriptObject::Pc(p) => self.players.get(&p).is_some_and(|s| s.blueprint.sheet.hp_damage >= self.rules.derived(&s.blueprint.sheet).hit_points),
            ScriptObject::Mobile { sector, index } => self.map.is_dead(sector, index),
            _ => false,
        }
    }

    /// A PC loses all its hit points (death is not built). An NPC is kept dead and shown on the
    /// last frame of its death animation (7), as 0x45d900 leaves it (when 0x5e863c is 1, not
    /// traced); it is spawned again as a corpse. Its hit points are not kept.
    fn kill(&mut self, object: ScriptObject) {
        match object {
            ScriptObject::Pc(p) => {
                if let Some(saved) = self.players.get_mut(&p) {
                    let sheet = &mut saved.blueprint.sheet;
                    sheet.hp_damage = self.rules.derived(sheet).hit_points;
                    self.sheets.insert(p);
                }
            }
            ScriptObject::Mobile { sector, index } => {
                if self.map.is_dead(sector, index) {
                    return;
                }
                self.map.changes.entry(sector).or_default().dead.insert(index);
                if let Some(art) = self.map.object_at(sector, index, true).and_then(|o| self.map.prototypes.art_id(o)) {
                    let dying = death_frame(self.vfs, self.map.art.as_deref(), art);
                    self.map.set_object_art(sector, index, true, dying.0);
                }
                self.sectors.insert(sector);
                self.npcs_toggled.push((sector, index));
            }
            _ => {}
        }
    }

    /// Objects of the map: flagged destroyed (0x1), so hidden, not blocking, NPCs gone (its
    /// destroy event ran first, in the action).
    fn destroy(&mut self, object: ScriptObject) {
        let Some((sector, index, mobile)) = map_place(object) else { return };
        let Some(flags) = self.map.object_flags(sector, index, mobile) else { return };
        self.toggle_flags(object, !flags & crate::OF_DESTROYED);
    }

    fn destroyed(&mut self, object: ScriptObject) -> bool {
        map_place(object).and_then(|(sector, index, mobile)| self.map.object_flags(sector, index, mobile)).is_some_and(|f| f & crate::OF_DESTROYED != 0)
    }

    fn wears_item_named(&mut self, object: ScriptObject, name: i32) -> bool {
        let Some(inventory) = self.inventory(object) else { return false };
        inventory.items.iter().any(|i| (crate::FIRST_SLOT..=crate::LAST_SLOT).contains(&i.location) && self.item_name(i.prototype) == Some(name))
    }

    /// Objects of the map: kept in the sector's changes and sent to every player; NPCs go or
    /// come. PCs are not switched (builder's call: no script switches one).
    fn toggle_flags(&mut self, object: ScriptObject, mask: i32) {
        let (sector, index, mobile) = match object {
            ScriptObject::Map(id) => (id.sector, id.index, false),
            ScriptObject::Mobile { sector, index } => (sector, index, true),
            ScriptObject::Pc(_) | ScriptObject::Carried { .. } | ScriptObject::NpcCarried { .. } => return,
        };
        let Some(flags) = self.map.object_flags(sector, index, mobile) else { return };
        let flags = flags ^ mask;
        self.map.set_object_flags(sector, index, mobile, flags);
        self.sectors.insert(sector);
        self.flag_changes.push(crate::LedgerDelta::ObjectFlags { sector, index, mobile, flags });
        if mobile {
            self.npcs_toggled.push((sector, index));
        }
    }

    /// PCs keep their effects on the sheet. NPCs' are not kept yet (builder's call: their
    /// stats are read from their prototype's base stats, without effects).
    fn give_effect(&mut self, object: ScriptObject, id: i32, cause: i32) {
        if let (ScriptObject::Pc(p), Ok(id)) = (object, u32::try_from(id))
            && let Some(saved) = self.players.get_mut(&p)
        {
            saved.blueprint.sheet.add_effect(id, u32::try_from(cause).unwrap_or(0));
            self.sheets.insert(p);
        }
    }

    fn leader(&mut self, object: ScriptObject) -> Option<ScriptObject> {
        self.following.iter().find(|f| f.0 == object).map(|f| ScriptObject::Pc(f.1))
    }

    /// The NPCs on this map following the PC now.
    fn followers(&mut self, leader: ScriptObject) -> Vec<ScriptObject> {
        let ScriptObject::Pc(p) = leader else { return Vec::new() };
        self.following.iter().filter(|f| f.1 == p).map(|f| f.0).collect()
    }

    /// A PC's party is every PC in the world (owner's rule: the players are one team; the
    /// original's 0x441310 lists the multiplayer party, alone just the PC).
    fn party(&mut self, object: ScriptObject) -> Vec<ScriptObject> {
        match object {
            ScriptObject::Pc(_) => self.players.keys().map(|&p| ScriptObject::Pc(p)).collect(),
            other => vec![other],
        }
    }

    fn set_leader(&mut self, npc: ScriptObject, leader: Option<ScriptObject>) {
        let ScriptObject::Mobile { sector, index } = npc else { return };
        let leader = match leader {
            Some(ScriptObject::Pc(p)) => Some(p),
            Some(_) => return,
            None => None,
        };
        self.follow_changes.push(((sector, index), leader));
        // Seen at once by the rest of the run.
        self.following.retain(|f| f.0 != npc);
        if let Some(p) = leader {
            self.following.push((npc, p));
        }
    }

    /// Line `id × 10 + 1` of `mes/gamebless.mes` or `mes/gamecurse.mes` (0x4c4170: ids from 50).
    fn blessing_effect(&mut self, curse: bool, id: i32) -> Option<i32> {
        let id = u32::try_from(id).ok().filter(|&id| id > 49)?;
        let file = if curse { "mes/gamecurse.mes" } else { "mes/gamebless.mes" };
        let mes = arcanum_formats::mes::MesFile::parse(&self.vfs?.read(file).ok()?).ok()?;
        Some(arcanum_formats::dialog::atoi(mes.get(id * 10 + 1)?.as_bytes()))
    }

    /// PCs: their sheet's damage goes down, to 0 (0x4b58c0 / 0x4ed720). NPCs' damage is not
    /// kept, so there is nothing to heal.
    fn heal(&mut self, object: ScriptObject, amount: i32, fatigue: bool) {
        let ScriptObject::Pc(p) = object else { return };
        let Some(saved) = self.players.get_mut(&p) else { return };
        let sheet = &mut saved.blueprint.sheet;
        // The dead are not healed back to life ([`crate::unconscious`]).
        if sheet.dead && !fatigue {
            return;
        }
        let damage = if fatigue { &mut sheet.fatigue_damage } else { &mut sheet.hp_damage };
        let healed = (*damage - amount.max(0)).max(0);
        if healed != *damage {
            *damage = healed;
            self.sheets.insert(p);
        }
    }

    /// PCs from their sheet. NPCs from their data: hit points `hp_pts + hp_adj` (the critter
    /// part, 0x43d630, and effects are not added), fatigue `pts × 4 + adj + 4 + (level + CN) × 2
    /// + WP` (0x45d670); less the data's damage.
    fn pool(&mut self, object: ScriptObject, fatigue: bool) -> (i32, i32) {
        match object {
            ScriptObject::Pc(p) => {
                let Some(sheet) = self.players.get(&p).map(|s| &s.blueprint.sheet) else { return (0, 0) };
                let d = self.rules.derived(sheet);
                if fatigue { (d.fatigue - sheet.fatigue_damage, d.fatigue) } else { (d.hit_points - sheet.hp_damage, d.hit_points) }
            }
            ScriptObject::Mobile { .. } if matches!(self.object_type(object), 15 | 16) => {
                let mut int = |f: &str| self.object_int(object, f).unwrap_or(0);
                if fatigue {
                    let (pts, adj, damage) = (int("critter_fatigue_pts"), int("critter_fatigue_adj"), int("critter_fatigue_damage"));
                    let most = pts * 4 + adj + 4 + (self.stat(object, 0x11) + self.stat(object, 2)) * 2 + self.stat(object, 6);
                    (most - damage, most)
                } else {
                    let most = int("hp_pts") + int("hp_adj");
                    (most - int("hp_damage"), most)
                }
            }
            _ => (0, 0),
        }
    }

    /// PCs only: primary stats (their base), alignment, fate points and poison, within the
    /// limits of tables 0x5b5194 / 0x5b5204; other stats, and NPCs (whose changes are not kept),
    /// are not changed.
    fn adjust_stat(&mut self, object: ScriptObject, stat: i32, delta: i32) {
        let ScriptObject::Pc(p) = object else { return };
        let Some(saved) = self.players.get_mut(&p) else { return };
        let sheet = &mut saved.blueprint.sheet;
        let (value, low, high) = match stat {
            0..=7 => (&mut sheet.base[stat as usize], 1, 20),
            0x13 => (&mut sheet.alignment, -1000, 1000),
            0x14 => (&mut sheet.fate_points, 0, 100),
            0x18 => (&mut sheet.poison, 0, 1000),
            _ => return,
        };
        let new = (*value + delta).clamp(low, high);
        if new != *value {
            if stat == 0x13 {
                self.alignment_moves.push((p, new >= *value));
            }
            *value = new;
            self.sheets.insert(p);
        }
    }

    fn location(&mut self, object: ScriptObject) -> Option<arcanum_formats::map::TileLoc> {
        self.tile_of(object)
    }

    /// Living, and within its perception (1-10, 0x4af240) in tiles, the larger distance
    /// (builder's call: the original's light, invisibility and wall checks, 0x4af260, are not
    /// copied).
    fn can_see(&mut self, viewer: ScriptObject, target: ScriptObject) -> bool {
        if self.is_dead(viewer) {
            return false;
        }
        let (Some(a), Some(b)) = (self.tile_of(viewer), self.tile_of(target)) else { return false };
        let reach = i64::from(self.stat(viewer, 5).clamp(1, 10));
        (i64::from(a.x) - i64::from(b.x)).abs().max((i64::from(a.y) - i64::from(b.y)).abs()) <= reach
    }

    fn in_dialog(&mut self, object: ScriptObject) -> bool {
        self.talking.contains(&object)
    }

    /// Spells kept on a critter are not built: never.
    fn under_spell(&mut self, _object: ScriptObject, _spell: i32) -> bool {
        false
    }

    /// Kept in the sector's changes; the NPC is spawned again to take it up.
    fn set_standpoint(&mut self, npc: ScriptObject, to: arcanum_formats::map::TileLoc, night: bool) {
        let ScriptObject::Mobile { sector, index } = npc else { return };
        if self.map.set_standpoint(sector, index, to, night) {
            self.sectors.insert(sector);
            self.npcs_toggled.push((sector, index));
        }
    }

    /// Items only, on the object's tile as a drop would put them (builder's call: the original
    /// tries up to ten random tiles 1-3 away, 0x4f4e40, and makes critters and scenery too).
    fn create_near(&mut self, basic: i32, near: ScriptObject) {
        let (Ok(basic), Some(at)) = (u32::try_from(basic), self.tile_of(near)) else { return };
        let prototype = basic + crate::BASIC_TO_PROTOTYPE;
        if self.map.prototypes.get(prototype).is_some_and(|p| (5..=14).contains(&p.object_type)) {
            self.created.push((prototype, at));
        }
    }

    fn newspaper_text(&mut self, key: i32) -> Option<String> {
        let mes = arcanum_formats::mes::MesFile::parse(&self.vfs?.read(crate::newspaper::NEWSPAPER_MES).ok()?).ok()?;
        mes.get(u32::try_from(key).ok()?).map(str::to_string)
    }

    /// Carried items only: the first such item the holder has (items are kept by prototype
    /// here); a map item's field is not kept.
    fn set_written_line(&mut self, item: ScriptObject, line: i32) {
        let ScriptObject::Carried { holder, prototype } = item else { return };
        let Some(inventory) = self.players.get_mut(&holder).and_then(|s| s.inventory.as_mut()) else { return };
        if let Some(i) = inventory.items.iter_mut().find(|i| i.prototype == prototype) {
            i.written = line;
            self.inventories.insert(holder);
        }
    }

    fn give_experience(&mut self, pc: ScriptObject, amount: i32) {
        if matches!(pc, ScriptObject::Pc(_)) {
            self.experience.push(amount);
        }
    }

    fn quest_experience(&mut self, level: i32) -> i32 {
        let Ok(level) = u32::try_from(level) else { return 0 };
        let Some(mes) = self.vfs.and_then(|v| v.read(crate::XP_QUEST).ok()).and_then(|d| arcanum_formats::mes::MesFile::parse(&d).ok()) else { return 0 };
        mes.get(level).map_or(0, |t| arcanum_formats::dialog::atoi(t.as_bytes()))
    }

    /// PCs: their save's inventory (NPCs' slots are not read: none).
    fn in_slot(&mut self, holder: ScriptObject, slot: i32) -> Option<ScriptObject> {
        let ScriptObject::Pc(p) = holder else { return None };
        let item = self.players.get(&p)?.inventory.as_ref()?.items.iter().find(|i| i.location == slot)?.prototype;
        Some(ScriptObject::Carried { holder: p, prototype: item })
    }

    /// Living, and within `(clamp(PE - 4, 1, 10) + 6) / 2` tiles less what the sound pays on its
    /// way (0x4af470 → 0x4af640, [`crate::combat::hears`]), the larger distance; an NPC listener
    /// by its state ([`crate::combat::ears`]).
    fn can_hear(&mut self, listener: ScriptObject, target: ScriptObject) -> bool {
        if self.is_dead(listener) {
            return false;
        }
        let (Some(a), Some(b)) = (self.tile_of(listener), self.tile_of(target)) else { return false };
        let prowler = match target {
            ScriptObject::Pc(p) if self.prowling.contains(&target) => self.players.get(&p).map(|s| crate::prowl::Prowler { level: self.rules.skill_level(&s.blueprint.sheet, crate::SKILL_PROWLING), intelligence: self.rules.stats(&s.blueprint.sheet)[4], injuries: s.blueprint.sheet.injuries, silence: s.inventory.as_ref().map_or(0, |i| crate::prowl::armour_silence(i, &self.map.prototypes, s.blueprint.sheet.training[crate::SKILL_PROWLING])), ..Default::default() }),
            _ => None,
        };
        // An NPC hears nothing stunned or down, half asleep ([`crate::combat::ears`]).
        let pe = match listener {
            ScriptObject::Mobile { sector, index } if self.map.npc_down(sector, index) => return false,
            ScriptObject::Mobile { sector, index } => match self.map.mobiles.get(&sector).and_then(|m| m.get(index as usize)).filter(|o| o.object_type == arcanum_formats::object::TYPE_NPC) {
                Some(o) => match crate::combat::ears(o, &self.map.prototypes, false) {
                    Some(pe) => pe,
                    None => return false,
                },
                None => self.stat(listener, 5),
            },
            _ => self.stat(listener, 5),
        };
        crate::combat::hears(&crate::MissileView(self.map), a, pe, b, prowler)
    }

    fn is_open(&mut self, object: ScriptObject) -> bool {
        match object {
            ScriptObject::Map(id) => self.map.map_object(id).filter(|o| o.object_type == arcanum_formats::object::TYPE_PORTAL).and_then(|o| self.map.prototypes.art_id(o)).is_some_and(crate::portal_open),
            ScriptObject::Mobile { sector, index } => self.map.container_object(GroundItemId::Container { sector, index }).is_some() && self.map.container_open(GroundItemId::Container { sector, index }),
            _ => false,
        }
    }

    fn trap_flags(&mut self, object: ScriptObject) -> i32 {
        map_place(object).and_then(|(sector, index, mobile)| self.map.object_flags(sector, index, mobile)).unwrap_or(0)
    }

    fn spot_trap(&mut self, critter: ScriptObject, magical: bool) -> bool {
        // The light where the critter stands (builder's call: the trapped object's own not
        // looked up).
        let light = self.tile_of(critter).and_then(|at| self.map.luminance_at(at, self.now));
        match critter {
            ScriptObject::Pc(p) => {
                let Some(sheet) = self.players.get(&p).map(|s| s.blueprint.sheet.clone()) else { return false };
                crate::trap::roll_spot(self.rules, &sheet, magical, light, self.rng)
            }
            // An NPC with its own skill (0x4bbe40 takes types 15 and 16).
            ScriptObject::Mobile { sector, index } => {
                let Some(o) = self.map.mobiles.get(&sector).and_then(|m| m.get(index as usize)) else { return false };
                if o.object_type != arcanum_formats::object::TYPE_NPC {
                    return false;
                }
                let (level, training) = crate::combat::npc_skill(o, &self.map.prototypes, crate::SKILL_SPOT_TRAP);
                let intelligence = crate::combat::npc_stat(o, &self.map.prototypes, 4);
                crate::trap::roll_spot_at(level, intelligence, training, magical, light, self.rng)
            }
            _ => false,
        }
    }

    /// The object's flag is kept and sent to every player (the original keeps it per PC in
    /// multiplayer, 0x4bd1e0; here the players are one team); the spotter is told.
    fn mark_trap_spotted(&mut self, object: ScriptObject, by: ScriptObject) {
        let Some((sector, index, mobile)) = map_place(object) else { return };
        let Some(flags) = self.map.object_flags(sector, index, mobile) else { return };
        let flags = flags | crate::OF_TRAP_SPOTTED;
        self.map.set_object_flags(sector, index, mobile, flags);
        self.sectors.insert(sector);
        self.flag_changes.push(crate::LedgerDelta::ObjectFlags { sector, index, mobile, flags });
        if let (ScriptObject::Pc(p), Some(text)) = (by, self.trap_text(crate::TRAP_LINE_SPOTTED)) {
            self.messages.push((p, text));
        }
    }

    fn trap_text(&mut self, line: u32) -> Option<String> {
        let mes = arcanum_formats::mes::MesFile::parse(&self.vfs?.read(crate::trap::TRAP_MES).ok()?).ok()?;
        mes.get(line).map(str::to_string)
    }

    /// NPCs only; the fight starts at the next combat step.
    fn start_fight(&mut self, npc: ScriptObject, target: ScriptObject) {
        let net = |o: ScriptObject| self.critters.iter().find(|c| c.0 == o).map(|c| c.2);
        if let (ScriptObject::Mobile { .. }, Some(a), Some(b)) = (npc, net(npc), net(target)) {
            self.orders.fights.push((a, b));
        }
    }

    /// Critters only (PCs as well as NPCs); the path is found when it starts.
    fn walk_to(&mut self, object: ScriptObject, to: arcanum_formats::map::TileLoc) {
        if let Some(c) = self.critters.iter().find(|c| c.0 == object) {
            self.orders.walks.push((c.2, to));
        }
    }

    /// Pick Locks by a PC on a door or container (the item scripts 29009 / 29010 use it); the
    /// other skills do nothing yet (builder's call).
    fn use_skill(&mut self, actor: ScriptObject, tool: Option<ScriptObject>, target: ScriptObject, skill: i32) {
        let ScriptObject::Pc(player) = actor else { return };
        if skill != crate::lockpick::SKILL_PICK_LOCKS as i32 {
            return;
        }
        let kind = self.object_type(target);
        let lock = match target {
            ScriptObject::Map(id) if kind == arcanum_formats::object::TYPE_PORTAL as i32 => crate::LockTarget::Door(id),
            ScriptObject::Mobile { sector, index } if kind == arcanum_formats::object::TYPE_CONTAINER as i32 => crate::LockTarget::Container(GroundItemId::Container { sector, index }),
            _ => return,
        };
        let tool = match tool {
            Some(ScriptObject::Carried { prototype, .. } | ScriptObject::NpcCarried { prototype, .. }) => Some(prototype),
            _ => None,
        };
        self.orders.picks.push((player, lock, tool));
    }

    /// Objects of the map, kept in the sector's changes; a carried item's name is not kept
    /// (items are kept by prototype).
    fn rename(&mut self, object: ScriptObject, name: i32) {
        let Some((sector, index, mobile)) = map_place(object) else { return };
        if self.map.set_object_name(sector, index, mobile, name) {
            self.sectors.insert(sector);
        }
    }

    fn critters_within(&mut self, centre: ScriptObject, radius: u32) -> Vec<ScriptObject> {
        let Some(at) = self.tile_of(centre) else { return Vec::new() };
        self.critters.iter().filter(|c| c.1.x.abs_diff(at.x).max(c.1.y.abs_diff(at.y)) <= radius).map(|c| c.0).collect()
    }

    fn remove_effect(&mut self, object: ScriptObject, id: i32) {
        if let (ScriptObject::Pc(p), Ok(id)) = (object, u32::try_from(id))
            && let Some(saved) = self.players.get_mut(&p)
            && saved.blueprint.sheet.remove_effect(id)
        {
            self.sheets.insert(p);
        }
    }

    fn stat(&mut self, object: ScriptObject, stat: i32) -> i32 {
        match object {
            ScriptObject::Pc(p) => {
                let Some(sheet) = self.players.get(&p).map(|s| s.blueprint.sheet.clone()) else { return 0 };
                pc_stat(self.rules, &sheet, stat)
            }
            ScriptObject::Mobile { sector, index } => {
                let Some(o) = self.map.mobiles.get(&sector).and_then(|m| m.get(index as usize)) else { return 0 };
                let stats = o.ints("critter_stat_base_idx").or_else(|| self.map.prototypes.of(o)?.ints("critter_stat_base_idx"));
                usize::try_from(stat).ok().and_then(|i| stats?.get(i).copied()).unwrap_or(0)
            }
            ScriptObject::Map(_) | ScriptObject::Carried { .. } | ScriptObject::NpcCarried { .. } => 0,
        }
    }

    fn reaction(&mut self, npc: ScriptObject, pc: ScriptObject) -> i32 {
        let (ScriptObject::Mobile { sector, index }, ScriptObject::Pc(p)) = (npc, pc) else { return 50 };
        let Some(o) = self.map.mobiles.get(&sector).and_then(|m| m.get(index as usize)).filter(|o| o.object_type == arcanum_formats::object::TYPE_NPC) else { return 50 };
        let Some(saved) = self.players.get(&p) else { return 50 };
        crate::reaction_to(&saved.script_state, &crate::met_key(npc), &crate::NpcReaction::of(o, &self.map.prototypes), self.rules, &saved.blueprint.sheet, self.reputations)
    }

    fn change_reaction(&mut self, npc: ScriptObject, pc: ScriptObject, delta: i32) {
        let (ScriptObject::Mobile { .. }, ScriptObject::Pc(p)) = (npc, pc) else { return };
        if delta == 0 || !self.is_npc(npc) {
            return;
        }
        let ScriptObject::Mobile { sector, index } = npc else { return };
        let Some(base) = self.map.mobiles.get(&sector).and_then(|m| m.get(index as usize)).map(|o| crate::NpcReaction::of(o, &self.map.prototypes).base) else { return };
        let key = crate::met_key(npc);
        if let Some(saved) = self.players.get_mut(&p) {
            let stored = saved.script_state.reactions.get(&key).copied().unwrap_or(base);
            saved.script_state.reactions.insert(key, stored + delta);
        }
    }

    fn distance(&mut self, a: ScriptObject, b: ScriptObject) -> i32 {
        let (Some(a), Some(b)) = (self.tile_of(a), self.tile_of(b)) else { return 0 };
        (f64::from(a.x) - f64::from(b.x)).hypot(f64::from(a.y) - f64::from(b.y)) as i32
    }

    fn has_met(&mut self, npc: ScriptObject, pc: ScriptObject) -> bool {
        if npc == pc {
            return true;
        }
        let ScriptObject::Pc(p) = pc else { return false };
        let key = crate::met_key(npc);
        self.players.get(&p).is_some_and(|s| s.script_state.met.contains(&key) || s.script_state.reactions.contains_key(&key))
    }

    fn knows_spell(&mut self, object: ScriptObject, spell: i32) -> bool {
        let ScriptObject::Pc(p) = object else { return false };
        let Some(sheet) = self.players.get(&p).map(|s| &s.blueprint.sheet) else { return false };
        (0..80).contains(&spell) && sheet.spells.get(spell as usize / 5).is_some_and(|&level| level > spell % 5)
    }

    fn schedule(&mut self, script: u32, line: i32, triggerer: ScriptObject, attachee: ScriptObject, delay_ms: u64) {
        self.scheduled.push(TimedScript { at: self.now.plus_ms(delay_ms), script, line, triggerer, attachee });
    }

    /// Which of the record's objects the original compares was not read (0x44aff0): here
    /// either (builder's call). Runs scheduled in this same run are dropped too.
    fn cancel_scheduled(&mut self, script: u32, object: ScriptObject) {
        let matches = |t: &TimedScript| t.script == script && (t.triggerer == object || t.attachee == object);
        self.scheduled.retain(|t| !matches(t));
        self.cancelled.push((script, object));
    }

    fn area_of(&mut self, object: ScriptObject) -> i32 {
        let (Some(at), Some(map)) = (self.tile_of(object), self.map.name.clone()) else { return 0 };
        self.areas.area_at(&map, at) as i32
    }

    fn player(&mut self) -> Option<ScriptObject> {
        Some(ScriptObject::Pc(self.player))
    }
}

/// Critter art's death animation.
pub const ANIM_DEATH: u32 = 7;

/// The default death animation for the default death: `art` on the last frame of
/// [`ANIM_DEATH`] ([`death_frame_in`]).
pub fn death_frame(vfs: Option<&arcanum_vfs::Vfs>, index: Option<&arcanum_formats::art_id::ArtIndex>, art: arcanum_formats::art_id::ArtId) -> arcanum_formats::art_id::ArtId {
    death_frame_in(vfs, index, art, ANIM_DEATH)
}

/// The violent deaths (0x4b5306): by the head, the body, the legs.
pub const ANIM_DEATH_HEAD: u32 = 17;
pub const ANIM_DEATH_BODY: u32 = 18;
pub const ANIM_DEATH_LEGS: u32 = 19;

/// `art` on the last frame of death animation `anim` (critters, monsters, unique NPCs share the
/// animation bits 6-10 and frame bits 14-18), or of 18 when it has no art for `anim` (0x42bce9),
/// or of 7; other art as it is.
pub fn death_frame_in(vfs: Option<&arcanum_vfs::Vfs>, index: Option<&arcanum_formats::art_id::ArtIndex>, art: arcanum_formats::art_id::ArtId, anim: u32) -> arcanum_formats::art_id::ArtId {
    use arcanum_formats::art_id::{ArtId, ArtKind};
    if !matches!(art.kind(), ArtKind::Critter | ArtKind::Monster | ArtKind::UniqueNpc) {
        return art;
    }
    let with = |anim: u32| ArtId((art.0 & !(0x1f << 6 | 0x1f << 14)) | anim << 6);
    let frames_of = |id: ArtId| index.and_then(|i| i.path(id)).and_then(|p| vfs?.read(&p).ok()).and_then(|d| arcanum_formats::art::ArtFile::parse(&d).ok()).map(|f| f.frame_count);
    let (dying, frames) = [anim, ANIM_DEATH_BODY, ANIM_DEATH].into_iter().find_map(|a| Some((with(a), frames_of(with(a))?))).unwrap_or((with(ANIM_DEATH), 1));
    ArtId(dying.0 | ((frames.saturating_sub(1) as u32 & 0x1f) << 14))
}

/// The death animation of a fatal hit (0x4b5306): more than 20 damage dies by where it landed
/// (head 17, legs 19, else 18); else the default 7.
pub fn death_anim(damage: i32, part: u8) -> u32 {
    match (damage > 20, part) {
        (false, _) => ANIM_DEATH,
        (true, 1) => ANIM_DEATH_HEAD,
        (true, 3) => ANIM_DEATH_LEGS,
        (true, _) => ANIM_DEATH_BODY,
    }
}

/// A map object's place: sector, index, whether it is a mobile one.
fn map_place(object: ScriptObject) -> Option<(arcanum_formats::map::SectorId, u32, bool)> {
    match object {
        ScriptObject::Map(id) => Some((id.sector, id.index, false)),
        ScriptObject::Mobile { sector, index } => Some((sector, index, true)),
        ScriptObject::Pc(_) | ScriptObject::Carried { .. } | ScriptObject::NpcCarried { .. } => None,
    }
}

/// `art` with its number set to `n` by the art's type (0x502780): unique NPCs 8 bits at 20,
/// items 11 at 17, interface 12 at 16, the others 9 at 19; tiles, walls, critters and monsters
/// have none. Out of range: no change.
pub fn with_art_number(art: arcanum_formats::art_id::ArtId, n: i32) -> Option<arcanum_formats::art_id::ArtId> {
    let (limit, mask, shift) = match art.art_type() {
        0 | 1 | 2 | 0xc => return None,
        0xd => (0x100, 0x0ff0_0000u32, 20),
        6 => (0x800, 0x0ffe_0000, 17),
        5 => (0x1000, 0x0fff_0000, 16),
        _ => (0x200, 0x0ff8_0000, 19),
    };
    let n = u32::try_from(n).ok().filter(|&n| n < limit)?;
    Some(arcanum_formats::art_id::ArtId((art.0 & !mask) | (n << shift)))
}

impl HostWorld<'_> {
    /// The same on an NPC (a sprung trap's, action 0x3c's): poison, fatigue, or hit points;
    /// with none left it dies as action 0x3a kills (builder's call: without experience or its
    /// dying script, 12).
    fn damage_npc(&mut self, sector: arcanum_formats::map::SectorId, index: u32, amount: i32, kind: i32) {
        if !self.map.mobiles.get(&sector).and_then(|m| m.get(index as usize)).is_some_and(|o| o.object_type == arcanum_formats::object::TYPE_NPC) || self.map.is_dead(sector, index) {
            return;
        }
        match kind {
            1 => {
                let level = self.map.npc_poison(sector, index) + amount;
                self.map.set_npc_poison(sector, index, level);
            }
            4 => {
                let Some((left, max)) = self.map.npc_fatigue(sector, index) else { return };
                self.map.set_npc_fatigue_damage(sector, index, max - left + amount);
            }
            _ => {
                let Some((left, _)) = self.map.npc_hp(sector, index) else { return };
                self.map.hurt_npc(sector, index, amount.min(left.max(0)));
                if left - amount <= 0 {
                    ScriptContext::kill(self, ScriptObject::Mobile { sector, index });
                }
            }
        }
        self.sectors.insert(sector);
    }

    /// Where an object stands.
    fn tile_of(&self, object: ScriptObject) -> Option<arcanum_formats::map::TileLoc> {
        if let Some(c) = self.critters.iter().find(|c| c.0 == object) {
            return Some(c.1);
        }
        match object {
            ScriptObject::Map(id) => crate::object_location(self.map.map_object(id)?),
            ScriptObject::Mobile { sector, index } => crate::object_location(self.map.mobiles.get(&sector)?.get(index as usize)?),
            ScriptObject::Pc(_) | ScriptObject::Carried { .. } | ScriptObject::NpcCarried { .. } => None,
        }
    }

    /// An object's inventory: a PC's (its save's), an NPC's, or a container's.
    fn inventory(&self, object: ScriptObject) -> Option<crate::Inventory> {
        match object {
            ScriptObject::Pc(p) => self.players.get(&p)?.inventory.clone(),
            ScriptObject::Mobile { sector, index } => {
                let o = self.map.mobiles.get(&sector)?.get(index as usize)?;
                match o.object_type {
                    arcanum_formats::object::TYPE_NPC => self.map.npc_inventory(sector, index),
                    arcanum_formats::object::TYPE_CONTAINER => self.map.pile(GroundItemId::Container { sector, index }).map(|p| p.inventory),
                    _ => None,
                }
            }
            ScriptObject::Map(_) | ScriptObject::Carried { .. } | ScriptObject::NpcCarried { .. } => None,
        }
    }

    fn set_inventory(&mut self, object: ScriptObject, inventory: crate::Inventory) {
        match object {
            ScriptObject::Pc(p) => {
                if let Some(saved) = self.players.get_mut(&p) {
                    saved.inventory = Some(inventory);
                    self.inventories.insert(p);
                }
            }
            ScriptObject::Mobile { sector, index } => {
                let kind = self.map.mobiles.get(&sector).and_then(|m| m.get(index as usize)).map(|o| o.object_type);
                if kind == Some(arcanum_formats::object::TYPE_NPC) {
                    self.map.set_npc_inventory(sector, index, inventory);
                } else if let Some(mut pile) = self.map.pile(GroundItemId::Container { sector, index }) {
                    pile.inventory = inventory;
                    self.map.set_pile(GroundItemId::Container { sector, index }, Some(pile));
                }
                self.sectors.insert(sector);
            }
            ScriptObject::Map(_) | ScriptObject::Carried { .. } | ScriptObject::NpcCarried { .. } => {}
        }
    }

    /// An item prototype's `name` (obj_f_name, field 0x16).
    fn item_name(&self, prototype: u32) -> Option<i32> {
        self.map.prototypes.get(prototype)?.int("name")
    }

    /// Item facts for placing items, with the art's sizes.
    fn item_facts(&self) -> impl Fn(u32) -> Option<crate::ItemInfo> + '_ {
        let (prototypes, art) = (self.map.prototypes.clone(), self.map.art.clone());
        let vfs = self.vfs;
        move |n| prototypes.get(n).map(|p| crate::ItemInfo::of(p, |id| crate::world::art_size(vfs, art.as_deref(), id)))
    }
}

/// A PC's effective stat (0x4b0490 order): 0-7 primary, 8-15 derived, 0x10 the aptitude, 0x11
/// the level, 0x12 experience, 0x13 alignment, 0x14 fate points, 0x15 unspent points, 0x18
/// poison, 0x19 age, 0x1a gender, 0x1b race; magick and tech points are 0.
pub fn pc_stat(rules: &arcanum_rules::RulesData, sheet: &arcanum_rules::CharacterSheet, stat: i32) -> i32 {
    match stat {
        0..=7 => rules.stats(sheet)[stat as usize],
        8..=15 => rules.derived(sheet).by_stat_id()[stat as usize - 8],
        0x10 => rules.aptitude(sheet),
        0x11 => sheet.level,
        0x12 => sheet.experience,
        0x13 => rules.alignment(sheet),
        0x14 => sheet.fate_points,
        0x15 => sheet.unspent,
        0x18 => sheet.poison,
        0x19 => sheet.age,
        0x1a => i32::from(sheet.gender),
        0x1b => i32::from(sheet.race),
        _ => 0,
    }
}

/// A script run waiting for its time (action 0x36).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedScript {
    pub at: crate::GameTime,
    pub script: u32,
    pub line: i32,
    pub triggerer: ScriptObject,
    pub attachee: ScriptObject,
}

/// Script runs waiting for their time (not in the save yet).
#[derive(Resource, Debug, Default)]
pub struct TimedScripts(pub Vec<TimedScript>);

/// Runs the scripts whose time has come ([`ScriptHost::run_due`]).
pub fn run_timed_scripts(mut scripts: ScriptHost, mut world: ResMut<WorldState>, mut map: ResMut<LoadedMap>, mut outbox: ResMut<crate::Outbox>) {
    if scripts.timed.0.is_empty() {
        return;
    }
    scripts.run_due(&mut world, &mut map, &mut outbox);
}

/// PCs whose inventory a script changed: [`sync_script_inventories`] sends them.
#[derive(Resource, Debug, Default)]
pub struct ScriptInventories(pub std::collections::BTreeSet<PlayerId>);

/// Copies inventories scripts changed from the save to the PCs and their players.
pub fn sync_script_inventories(mut changed: ResMut<ScriptInventories>, world: Res<WorldState>, acks: Res<crate::InventoryAcks>, mut outbox: ResMut<crate::Outbox>, mut pcs: Query<(&crate::NetId, &crate::PlayerControlled, &mut crate::Inventory)>) {
    if changed.0.is_empty() {
        return;
    }
    for player in std::mem::take(&mut changed.0) {
        let Some(saved) = world.0.as_ref().and_then(|w| w.players.get(&player)).and_then(|p| p.inventory.clone()) else { continue };
        let Some((&net_id, _, mut inventory)) = pcs.iter_mut().find(|pc| pc.1.0 == player) else { continue };
        *inventory = saved.clone();
        let ack = acks.0.get(&player).copied().unwrap_or(0);
        outbox.0.push((player, crate::LedgerDelta::Inventory { net_id, ack, inventory: saved }));
    }
}

/// Item events (0x441980's list): an item worn (24) and taken off (25).
pub const EVENT_WIELD_ON: u32 = 24;
pub const EVENT_WIELD_OFF: u32 = 25;
/// An object's lock is tried (0x4aeb70 doors, 0x4aed80 containers), before the lock check: a
/// "skip the default" answer denies the opening.
pub const EVENT_UNLOCK: u32 = 3;
/// An item is picked up off the ground (0x461810), before: "skip the default" cancels it.
pub const EVENT_GET: u32 = 4;
/// An item is dropped (0x461ab0), before: "skip the default" cancels it.
pub const EVENT_DROP: u32 = 5;
/// An item went into an inventory (0x466640) / came out of one (0x467860), after; the answer is
/// ignored.
pub const EVENT_INSERT_ITEM: u32 = 21;
pub const EVENT_REMOVE_ITEM: u32 = 28;

/// What each PC carried at the last look, by prototype and count (host only, not saved).
#[derive(Resource, Debug, Default)]
pub struct CarriedSeen(pub BTreeMap<PlayerId, BTreeMap<u32, usize>>);

/// Runs the insert (21) and remove (28) scripts of the items each PC gained or lost since the
/// last look, whatever moved them (pick-ups, loot, trade, scripts), with the PC as triggerer;
/// lost first. Items are counted by prototype: a move inside the inventory runs nothing. A PC
/// seen for the first time runs nothing (loading a game runs nothing). Builder's call: the
/// original fires them inside each move (0x466640 / 0x467860); NPCs' and containers' are not
/// run.
pub fn run_carry_scripts(mut host: ScriptHost, mut seen: ResMut<CarriedSeen>, mut world: ResMut<WorldState>, mut map: ResMut<LoadedMap>, mut outbox: ResMut<crate::Outbox>, pcs: Query<(&crate::PlayerControlled, &crate::Inventory)>) {
    let mut changes = Vec::new();
    for (controlled, inventory) in &pcs {
        let mut now = BTreeMap::new();
        for item in &inventory.items {
            *now.entry(item.prototype).or_insert(0usize) += 1;
        }
        let Some(before) = seen.0.insert(controlled.0, now.clone()) else { continue };
        for (&prototype, &n) in &before {
            for _ in now.get(&prototype).copied().unwrap_or(0)..n {
                changes.push((controlled.0, prototype, EVENT_REMOVE_ITEM));
            }
        }
        for (&prototype, &n) in &now {
            for _ in before.get(&prototype).copied().unwrap_or(0)..n {
                changes.push((controlled.0, prototype, EVENT_INSERT_ITEM));
            }
        }
    }
    changes.sort_by_key(|c| c.2 != EVENT_REMOVE_ITEM);
    for (player, prototype, event) in changes {
        host.item_hook(&mut world, &mut map, &mut outbox, player, prototype, event);
    }
}

/// Runs the wield scripts of the items each PC puts on or takes off (0x466640 → 0x4677b0,
/// 0x467860 → 0x467cb0), after the change, by comparing what it wears with what it wore (kept
/// in the save, [`crate::PlayerSave::worn`]). Taken off first, then put on. A PC seen for the
/// first time (a new character) puts its starting gear on; loading a game runs nothing, as in
/// the original.
pub fn run_wield_scripts(mut host: ScriptHost, mut world: ResMut<WorldState>, mut map: ResMut<LoadedMap>, mut outbox: ResMut<crate::Outbox>, pcs: Query<(&crate::PlayerControlled, &crate::Inventory)>) {
    // Read first: the save is only touched when something changed.
    let mut changes = Vec::new();
    let mut now_worn = Vec::new();
    {
        let Some(save) = world.0.as_ref() else { return };
        for (controlled, inventory) in &pcs {
            let Some(saved) = save.players.get(&controlled.0) else { continue };
            let now: BTreeMap<i32, u32> = inventory.items.iter().filter(|i| (crate::FIRST_SLOT..=crate::LAST_SLOT).contains(&i.location)).map(|i| (i.location, i.prototype)).collect();
            let before = saved.worn.clone().unwrap_or_default();
            if before == now {
                continue;
            }
            for (slot, &prototype) in &before {
                if now.get(slot) != Some(&prototype) {
                    changes.push((controlled.0, prototype, EVENT_WIELD_OFF));
                }
            }
            for (slot, &prototype) in &now {
                if before.get(slot) != Some(&prototype) {
                    changes.push((controlled.0, prototype, EVENT_WIELD_ON));
                }
            }
            now_worn.push((controlled.0, now));
        }
    }
    if now_worn.is_empty() {
        return;
    }
    if let Some(save) = world.0.as_mut() {
        for (player, now) in now_worn {
            if let Some(saved) = save.players.get_mut(&player) {
                saved.worn = Some(now);
            }
        }
    }
    for (player, prototype, event) in changes {
        host.item_hook(&mut world, &mut map, &mut outbox, player, prototype, event);
    }
}

/// PCs whose sheet a script changed: [`sync_script_sheets`] sends them.
#[derive(Resource, Debug, Default)]
pub struct ScriptSheets(pub std::collections::BTreeSet<PlayerId>);

/// Copies sheets scripts changed from the save to the PCs and their players; everyone gets the
/// PC's status, as effects move its maxima.
pub fn sync_script_sheets(
    mut changed: ResMut<ScriptSheets>,
    world: Res<WorldState>,
    rules: Res<crate::SimRules>,
    session: Res<crate::Session>,
    mut outbox: ResMut<crate::Outbox>,
    mut pcs: Query<(&crate::NetId, &crate::PlayerControlled, &mut crate::Sheet)>,
) {
    for player in std::mem::take(&mut changed.0) {
        let Some(saved) = world.0.as_ref().and_then(|w| w.players.get(&player)).map(|p| p.blueprint.sheet.clone()) else { continue };
        if let Some((&net_id, _, mut sheet)) = pcs.iter_mut().find(|pc| pc.1.0 == player) {
            sheet.0 = saved.clone();
            crate::ground::broadcast(&session, &mut outbox, crate::LedgerDelta::PcStatus { net_id, status: crate::pc_status(&rules.0, &saved) });
        }
        outbox.0.push((player, crate::LedgerDelta::SheetUpdate { ack: 0, sheet: saved }));
    }
}

impl ScriptHost<'_, '_> {
    /// Runs `object`'s script for `event`, triggered by `player`'s PC (0x441980). A changed
    /// attachment is kept and saved; the answer says whether the default still happens.
    /// Lines it shows go to their players through `outbox`.
    pub fn hook(&mut self, world: &mut WorldState, map: &mut LoadedMap, outbox: &mut crate::Outbox, object: ScriptObject, event: u32, player: PlayerId) -> HookResult {
        self.hook_by(world, map, outbox, object, event, player, ScriptObject::Pc(player))
    }

    /// The same with another triggerer (a heartbeat's is the NPC itself, 0x4ad200).
    #[allow(clippy::too_many_arguments)]
    pub fn hook_by(&mut self, world: &mut WorldState, map: &mut LoadedMap, outbox: &mut crate::Outbox, object: ScriptObject, event: u32, player: PlayerId, triggerer: ScriptObject) -> HookResult {
        let mut attachment = map.attachment(object, event);
        if attachment.number == 0 {
            return HookResult::RunDefault;
        }
        let before = attachment;
        let invocation = Invocation { attachee: object, triggerer, other: None, event, start_line: 0 };
        let Some(result) = self.execute(world, map, outbox, &mut attachment, invocation, player) else { return HookResult::RunDefault };
        if attachment != before {
            map.set_attachment(object, event, attachment);
            let sector = match object {
                ScriptObject::Map(id) => id.sector,
                ScriptObject::Mobile { sector, .. } => sector,
                ScriptObject::Pc(_) | ScriptObject::Carried { .. } | ScriptObject::NpcCarried { .. } => return result,
            };
            crate::ground::save_change(world, map, sector);
        }
        result
    }

    /// Runs the script of an item `player`'s PC carries, for `event` (wield 24, unwield 25):
    /// the item is the script's own object, the PC the triggerer (0x4677b0 / 0x467cb0). The
    /// answer is ignored, as the original's is; the item's script counters are not kept.
    /// The sheets changed this tick, to send on ([`ScriptSheets`]).
    pub fn changed_sheets(&mut self) -> &mut ScriptSheets {
        &mut self.changed_sheets
    }

    pub fn item_hook(&mut self, world: &mut WorldState, map: &mut LoadedMap, outbox: &mut crate::Outbox, player: PlayerId, prototype: u32, event: u32) -> HookResult {
        self.carried_hook(world, map, outbox, ScriptObject::Carried { holder: player, prototype }, prototype, event, player, ScriptObject::Pc(player), None)
    }

    /// Runs the script of a carried `item` (a PC's or an NPC's) of `prototype` for `event`, with
    /// `triggerer` and `other`, `player` acting.
    #[allow(clippy::too_many_arguments)]
    pub fn carried_hook(&mut self, world: &mut WorldState, map: &mut LoadedMap, outbox: &mut crate::Outbox, item: ScriptObject, prototype: u32, event: u32, player: PlayerId, triggerer: ScriptObject, other: Option<ScriptObject>) -> HookResult {
        let Some(mut attachment) = map.prototypes.get(prototype).and_then(|p| attachments(p, &map.prototypes).get(&event).copied()) else { return HookResult::RunDefault };
        if attachment.number == 0 {
            return HookResult::RunDefault;
        }
        let invocation = Invocation { attachee: item, triggerer, other, event, start_line: 0 };
        self.execute(world, map, outbox, &mut attachment, invocation, player).unwrap_or(HookResult::RunDefault)
    }

    /// Runs the scripts whose time has come (action 0x36): each with a record of its own, for
    /// its triggerer's player (or the first player when the triggerer is no PC).
    pub fn run_due(&mut self, world: &mut WorldState, map: &mut LoadedMap, outbox: &mut crate::Outbox) {
        let now = self.clock.now;
        let (due, waiting): (Vec<TimedScript>, Vec<TimedScript>) = std::mem::take(&mut self.timed.0).into_iter().partition(|t| t.at <= now);
        self.timed.0 = waiting;
        for timed in due {
            let player = match timed.triggerer {
                ScriptObject::Pc(p) => Some(p),
                _ => world.0.as_ref().and_then(|w| w.players.keys().next().copied()),
            };
            let Some(player) = player else { continue };
            let invocation = Invocation { attachee: timed.attachee, triggerer: timed.triggerer, other: None, event: 0, start_line: timed.line.max(0) as usize };
            self.execute(world, map, outbox, &mut crate::Attachment::new(0, 0, timed.script), invocation, player);
        }
    }

    /// Runs `attachment`'s script for `invocation` with `player` acting, and delivers what it
    /// did. `None` without a world.
    fn execute(&mut self, world: &mut WorldState, map: &mut LoadedMap, outbox: &mut crate::Outbox, attachment: &mut crate::Attachment, invocation: Invocation<ScriptObject>, player: PlayerId) -> Option<HookResult> {
        let save = world.0.as_mut()?;
        let vfs_guard = self.vfs.as_ref().and_then(|v| v.0.read().ok());
        let vfs = vfs_guard.as_deref();
        let library = self.scripts.0.get_or_insert_with(|| vfs.map(ScriptLibrary::new).unwrap_or_default());
        let number = attachment.number;
        let following = self.critters.iter().filter_map(|(_, _, _, npc, f, _)| Some((npc?.object(), f?.leader))).collect();
        let prowling = self.critters.iter().filter_map(|(_, _, pc, _, _, p)| p.then_some(ScriptObject::Pc(pc?.0))).collect();
        let talking = self.talks.iter().flat_map(|(pc, talk)| [ScriptObject::Pc(pc.0), talk.npc_object]).collect();
        let critters = self
            .critters
            .iter()
            .filter_map(|(&net_id, position, pc, npc, _, _)| {
                let object = match (pc, npc) {
                    (Some(pc), _) => ScriptObject::Pc(pc.0),
                    (_, Some(npc)) => npc.object(),
                    _ => return None,
                };
                Some((object, position.0, net_id))
            })
            .collect();
        let reputations = &*self.reputations.0.get_or_insert_with(|| {
            vfs.and_then(|v| v.read(crate::GAME_REP).ok()).and_then(|d| arcanum_formats::mes::MesFile::parse(&d).ok()).map(|m| crate::ReputationEffects::parse(&m)).unwrap_or_default()
        });
        let areas = self.areas.load(vfs);
        let mut ctx = HostWorld {
            areas,
            rules: &self.rules.0,
            reputations,
            scheduled: Vec::new(),
            cancelled: Vec::new(),
            library,
            map,
            player,
            critters,
            following,
            talking,
            prowling,
            follow_changes: Vec::new(),
            alignment_moves: Vec::new(),
            floats: Vec::new(),
            inventories: Default::default(),
            sheets: Default::default(),
            flag_changes: Vec::new(),
            npcs_toggled: Vec::new(),
            sectors: Default::default(),
            vfs,
            globals: &mut save.scripts,
            players: &mut save.players,
            rng: &mut self.rng,
            now: self.clock.now,
            messages: Vec::new(),
            written: Vec::new(),
            created: Vec::new(),
            experience: Vec::new(),
            dialogs: Vec::new(),
            teleports: Vec::new(),
            orders: ScriptOrders::default(),
        };
        let run = crate::run_hook(&mut ctx, attachment, invocation);
        for (to, text) in std::mem::take(&mut ctx.messages) {
            outbox.0.push((to, crate::LedgerDelta::ScriptMessage { text }));
        }
        for (to, kind, line) in std::mem::take(&mut ctx.written) {
            outbox.0.push((to, crate::LedgerDelta::Written { kind, line }));
        }
        self.awards.0.append(&mut ctx.experience);
        let created = std::mem::take(&mut ctx.created);
        self.started.0.append(&mut ctx.dialogs);
        // Floats go to everyone in the world (the original to those who can see it).
        let players: Vec<PlayerId> = ctx.players.keys().copied().collect();
        for (speaker, text) in std::mem::take(&mut ctx.floats) {
            for &to in &players {
                outbox.0.push((to, crate::LedgerDelta::FloatText { speaker, text: text.clone(), colour: crate::FloatColour::White, voice: None }));
            }
        }
        self.changed_inventories.0.append(&mut ctx.inventories);
        self.changed_sheets.0.append(&mut ctx.sheets);
        for delta in std::mem::take(&mut ctx.flag_changes) {
            for &to in &players {
                outbox.0.push((to, delta.clone()));
            }
        }
        self.toggled_npcs.0.append(&mut ctx.npcs_toggled);
        for (script, object) in std::mem::take(&mut ctx.cancelled) {
            self.timed.0.retain(|t| !(t.script == script && (t.triggerer == object || t.attachee == object)));
        }
        self.timed.0.append(&mut ctx.scheduled);
        let mut touched = std::mem::take(&mut ctx.sectors);
        let follow_changes = std::mem::take(&mut ctx.follow_changes);
        let alignment_moves = std::mem::take(&mut ctx.alignment_moves);
        for (object, map, to) in std::mem::take(&mut ctx.teleports) {
            self.teleports.0.push(crate::big_move::ScriptTeleport { causer: player, object, map, to });
        }
        self.orders.walks.append(&mut ctx.orders.walks);
        self.orders.picks.append(&mut ctx.orders.picks);
        self.orders.fights.append(&mut ctx.orders.fights);
        for unsupported in run.unsupported {
            self.reports.0.push((number, unsupported));
        }
        let excess = self.reports.0.len().saturating_sub(REPORTS_KEPT);
        self.reports.0.drain(..excess);
        // Joins and leaves by script: forced joins, not counted (0x45de90 sets 0x400, clears
        // 0x20); leaving clears forced and waiting, as the `lv` result does.
        if let Some(save) = world.0.as_mut() {
            // A PC's alignment moved: its followers warn at their next check (0x4b0980 stat
            // 0x13 → 0x4b0db8: 0x2000000 up, 0x4000000 down, compare at 0x4b0dd4).
            for (player, up) in alignment_moves {
                let bit = if up { crate::follow::WARN_GOOD } else { crate::follow::WARN_EVIL };
                for f in save.followers.iter_mut().filter(|f| f.leader == player) {
                    f.warn |= bit;
                }
            }
            for ((sector, index), leader) in follow_changes {
                use crate::follow::{FORCED, LEFT, WAITING};
                let flags = map.npc_flags(sector, index).unwrap_or(0);
                let flags = if leader.is_some() { (flags & !(LEFT | WAITING)) | FORCED } else { flags & !(FORCED | WAITING) };
                touched.extend(crate::follow::set_follower(&mut save.followers, map, (sector, index), leader, flags, None, self.clock.now));
            }
        }
        for sector in touched {
            crate::ground::save_change(world, map, sector);
        }
        if !created.is_empty() {
            let (prototypes, art) = (map.prototypes.clone(), map.art.clone());
            let info = crate::item_info(&prototypes, |id| crate::world::art_size(vfs, art.as_deref(), id));
            for (prototype, at) in created {
                let item = crate::CarriedItem { prototype, location: 0, quantity: 0, keys: vec![], written: 0, wear: Default::default() };
                crate::ground::place_on_ground(item, at, &info, map, world, &self.session, outbox);
            }
        }
        Some(run.result)
    }

    /// Line `line` of `mes/trap.mes`.
    pub fn trap_line(&self, line: u32) -> Option<String> {
        let guard = self.vfs.as_ref().and_then(|v| v.0.read().ok());
        let mes = arcanum_formats::mes::MesFile::parse(&guard.as_deref()?.read(crate::trap::TRAP_MES).ok()?).ok()?;
        mes.get(line).map(str::to_string)
    }

    /// Script `number`'s header flags, if it exists.
    pub fn script_flags(&mut self, number: u32) -> Option<u32> {
        let vfs_guard = self.vfs.as_ref().and_then(|v| v.0.read().ok());
        let vfs = vfs_guard.as_deref();
        let library = self.scripts.0.get_or_insert_with(|| vfs.map(ScriptLibrary::new).unwrap_or_default());
        let script = match vfs {
            Some(vfs) => library.get(vfs, number),
            None => library.get_loaded(number),
        };
        script.map(|s| s.flags)
    }

    /// Whether script `number` is a trap ([`crate::is_trap`]).
    pub fn is_trap(&mut self, number: u32) -> bool {
        let vfs_guard = self.vfs.as_ref().and_then(|v| v.0.read().ok());
        let vfs = vfs_guard.as_deref();
        let library = self.scripts.0.get_or_insert_with(|| vfs.map(ScriptLibrary::new).unwrap_or_default());
        let script = match vfs {
            Some(vfs) => library.get(vfs, number),
            None => library.get_loaded(number),
        };
        script.is_some_and(|s| crate::is_trap(number, &s))
    }

    /// The host's random numbers.
    pub fn rng(&mut self) -> &mut crate::HostRng {
        &mut self.rng
    }

    /// Puts a script in the library by hand (tests).
    pub fn insert(&mut self, number: u32, script: Script) {
        self.scripts.0.get_or_insert_with(ScriptLibrary::default).insert(number, script);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arcanum_formats::object::{Array, TYPE_PORTAL};
    use arcanum_formats::test_util::ObjectBuilder;

    trait Instance {
        fn build_instance(&self) -> Object;
    }

    impl Instance for ObjectBuilder {
        fn build_instance(&self) -> Object {
            Object::parse_instance(&self.build()).unwrap()
        }
    }

    #[test]
    fn art_numbers_sit_where_the_art_type_keeps_them() {
        use arcanum_formats::art_id::ArtId;
        let scenery = ArtId(0x4000_0000 | 5 << 19 | 3 << 11);
        assert_eq!(with_art_number(scenery, 73), Some(ArtId(0x4000_0000 | 73 << 19 | 3 << 11)), "rotation kept");
        assert_eq!(with_art_number(scenery, 512), None, "9 bits");
        assert_eq!(with_art_number(ArtId(0x6000_0000), 1500), Some(ArtId(0x6000_0000 | 1500 << 17)), "items: 11 bits at 17");
        assert_eq!(with_art_number(ArtId(0x2000_0000), 1), None, "critters have no number");
    }

    #[test]
    fn attachments_are_a_sparse_array_by_event() {
        // Events 1 and 9: script 1274 (flags 3, counters 1, 2, 0, 0) and script 1823.
        let mut data = Vec::new();
        for v in [3u32, 0x0201, 1274, 0, 0, 1823] {
            data.extend(v.to_le_bytes());
        }
        let array = Array { elem_size: 12, id: 1, data, bitset: vec![1 << 1 | 1 << 9, 0] };
        let object = ObjectBuilder::new(TYPE_PORTAL).field("scripts_idx", Value::Array(Some(array))).build_instance();
        let found = attachments(&object, &Prototypes::default());
        assert_eq!(found.len(), 2);
        assert_eq!(found[&1], Attachment { flags: 3, counters: [1, 2, 0, 0], number: 1274 });
        assert_eq!(found[&9].number, 1823);
        assert!(attachments(&ObjectBuilder::new(TYPE_PORTAL).build_instance(), &Prototypes::default()).is_empty());
    }
}
