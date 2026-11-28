//! The name of what the cursor rests on in the world, in the bottom bar's message box
//! (Arcanum.exe 0x553a70 hover → 0x57ccf0 → 0x57cd60 / 0x441b60 / 0x553be0).
//!
//! - **When:** as the object under the cursor changes (0x553a70); nothing under it clears the
//!   box. The bars and open windows cover the world.
//! - **The name** (0x441b60): an item's `description` when identified (no magick or tech
//!   complexity, or flagged identified), else its `item_description_unknown`; a portal's,
//!   container's or scenery's `description`; an NPC's `description` once the player's PC has
//!   met it ([`arcanum_sim::LedgerDelta::KnownNpcs`], 0x45f970), else its
//!   `critter_description_unknown`; a PC's name. Numbers below 30000 are `mes/description.mes`
//!   lines (0x4d4210), others `mes/gamedesc.mes`.
//!
//! - **Under the name** (0x553be0; rows 0x5c70c8, texts `mes/intgame.mes`; [`details`]):
//!   - a door or container: an icon (doors 436, windows 786; containers by basic prototype, 0x553d10), its
//!     lock ("Jammed", "Magically held", "Locked", "Unlocked") and hit points in the last row,
//!     in blue; a container's hit points only when it is not invulnerable (`flags` 0x400000);
//!   - scenery: icon 437 and its hit points when not invulnerable;
//!   - an item: its icon (0x554c20 by type), its name in light red for magick (complexity > 0),
//!     a stats row (weapons D / FT / TH / RNG / PD / FD / ED, armour AC / DR / PR / FR / ER / MR /
//!     NP, each `label:a-b` with the magick part as `(+n)` once identified, 0x555780), quantity,
//!     keys, a tool's bonus; charges ("Uses"); its weight in stone (stacks: × quantity / 4) and,
//!     for weapons, armour and breakable tools, its hit points;
//!   - a critter: its level; hit points and fatigue as gauges (art 463/464, poisoned 665/666;
//!     465/466) with `cur/max` for the player's own PC and a percentage for the others (NPCs'
//!     from the host, [`crate::combat_ui::NpcHealth`]); an NPC's
//!     icon by race (table 0x5c6fa0).
//!
//! Not built: an NPC's reaction or alignment and the prowling state; the to-hit panel in combat;
//! a weapon's speed, magick power and tech aptitude rows (they need the viewer's stats); the
//! description override (`item_description_effects`); special icons (item flag 0x40); PCs'
//! portraits as icons.
//!
//! - **The examine script** (event 0, [`arcanum_sim::examine`]): a hovered object with one on
//!   the client's map, and every NPC (their own scripts are the host's), asks the host first;
//!   the name shows on its answer, or not at all when the script skips it.

use std::collections::HashSet;

use arcanum_formats::mes::MesFile;
use arcanum_formats::object::{Object, Prototypes, TYPE_WALL};
use arcanum_sim::{EntityKind, GroundItemId, NetId};
use bevy::prelude::*;

use crate::actors::Actor;
use crate::main_menu::AppState;
use crate::message_box::{BoxCell, BoxDetails, BoxGauge, BoxIcon, BoxMessage, BoxRow, MessageBox};
use crate::vfs_asset::VfsResource;
use crate::world_render::{ClientMap, MapObject};

/// What a spawned actor is (from its spawn).
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct ActorKind(pub EntityKind);

/// The NPCs the local player's PC has met.
#[derive(Resource, Debug, Default)]
pub struct KnownNpcs(pub HashSet<NetId>);

/// The host's answers to examinations ([`arcanum_sim::LedgerDelta::Examined`]): number, shown.
#[derive(Resource, Debug, Default)]
pub struct ExamineAnswers(pub Vec<(u32, bool)>);

/// The examination waiting for the host: its number and what it would show.
#[derive(Debug, Default)]
struct Examining {
    seq: u32,
    waiting: Option<(u32, BoxMessage)>,
}

/// Asking the host about examine scripts ([`hover_world`]).
type Examine<'w, 's> = (Option<Res<'w, crate::sim_bridge::HostLink>>, ResMut<'w, ExamineAnswers>, Local<'s, Examining>);

/// Description numbers from here on are in `mes/gamedesc.mes` (0x4d4210).
pub const GAME_DESCRIPTIONS_FROM: i32 = 30000;

pub struct WorldHoverPlugin;

impl Plugin for WorldHoverPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KnownNpcs>()
            .init_resource::<ExamineAnswers>()
            .add_systems(OnExit(AppState::InGame), |mut known: ResMut<KnownNpcs>| known.0.clear())
            .add_systems(Update, hover_world.run_if(in_state(AppState::InGame)));
    }
}

/// The two description files and the interface's texts.
#[derive(Default)]
struct Descriptions {
    loaded: bool,
    description: MesFile,
    game: MesFile,
    intgame: MesFile,
}

impl Descriptions {
    fn text(&self, n: i32) -> Option<String> {
        let file = if n < GAME_DESCRIPTIONS_FROM { &self.description } else { &self.game };
        file.get(u32::try_from(n).ok()?).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    }
}

/// An item's name number: its description when identified, else its unknown one (0x441b60).
pub fn item_name(object: &Object, prototypes: &Prototypes) -> Option<i32> {
    let complexity = prototypes.int(object, "item_magic_tech_complexity").unwrap_or(0);
    let identified = complexity < 1 || prototypes.int(object, "item_flags").unwrap_or(0) & 1 != 0;
    prototypes.int(object, if identified { "description" } else { "item_description_unknown" })
}

/// A critter's name number: known, its description; else its unknown one (0x45f970).
pub fn critter_name(object: &Object, prototypes: &Prototypes, known: bool) -> Option<i32> {
    prototypes.int(object, if known { "description" } else { "critter_description_unknown" })
}

/// Detail texts' colours (0x64c498 white, 0x64c49c blue, 0x64c500 red) and the names' of magick
/// items (0x64c470).
pub const WHITE: [u8; 3] = [255, 255, 255];
pub const BLUE: [u8; 3] = [0, 0, 255];
pub const RED: [u8; 3] = [255, 0, 0];
pub const MAGICK_NAME: [u8; 3] = [255, 100, 100];
/// `item_flags`: identified.
const OIF_IDENTIFIED: i32 = 0x1;
/// `flags`: invulnerable (no hit points shown).
pub const OF_INVULNERABLE: i32 = 0x400000;
/// Interface art: a door's icon (portal art bit 0x400, 0x504260) and a window's, scenery's, a
/// container's by default.
pub const DOOR_ICON: u32 = 436;
pub const WINDOW_ICON: u32 = 786;
pub const SCENERY_ICON: u32 = 437;
pub const CONTAINER_ICON: u32 = 435;
/// Containers' icons by basic prototype (0x553d10).
pub const CONTAINER_ICONS: [(u32, u32); 9] = [(3023, 832), (3033, 834), (3034, 834), (3055, 835), (3056, 833), (3057, 837), (3058, 838), (3059, 839), (3060, 836)];
/// NPCs' icons by race (table 0x5c6fa0).
pub const RACE_ICONS: [u32; 11] = [375, 378, 376, 377, 379, 380, 381, 382, 376, 382, 381];
/// Weapons' icons by the art's sub-type (0x5c6fe4); ammo by type (0x5c7020); armour by sub-type
/// (0x5c7030) or, with a coverage, by that (0x5c7054); written items by sub-type (0x5c7070).
pub const WEAPON_ICONS: [u32; 15] = [391, 391, 391, 391, 391, 391, 392, 391, 395, 391, 392, 391, 391, 391, 391];
pub const AMMO_ICONS: [u32; 4] = [398, 399, 400, 401];
pub const ARMOUR_ICONS: [u32; 9] = [402, 402, 405, 405, 405, 408, 405, 405, 402];
pub const COVERAGE_ICONS: [u32; 7] = [0, 405, 405, 405, 405, 411, 414];
pub const WRITTEN_ICONS: [u32; 8] = [428, 429, 431, 430, 428, 462, 428, 438];
/// Gauges: hit points (poisoned) and fatigue, fill and empty (0x554640).
pub const HP_GAUGE: (u32, u32) = (463, 464);
pub const POISONED_GAUGE: (u32, u32) = (665, 666);
pub const FATIGUE_GAUGE: (u32, u32) = (465, 466);

/// An object's hit points: `hp_pts + hp_adj` less `hp_damage`, and the most.
pub fn hit_points(o: &Object, prototypes: &Prototypes) -> (i32, i32) {
    let max = prototypes.int(o, "hp_pts").unwrap_or(0) + prototypes.int(o, "hp_adj").unwrap_or(0);
    (max - prototypes.int(o, "hp_damage").unwrap_or(0), max)
}

/// The lock's text in `mes/intgame.mes` (0x555ec0): jammed 26, magically held 27, locked 9,
/// unlocked 10. Day and night locks count as their bit.
pub fn lock_text(flags: i32) -> u32 {
    if flags & 2 != 0 {
        26
    } else if flags & 4 != 0 {
        27
    } else if flags & 1 != 0 {
        9
    } else {
        10
    }
}

fn cell(text: impl Into<String>, colour: [u8; 3]) -> Option<BoxCell> {
    Some(BoxCell::new(text, colour))
}

/// One stats segment (0x555780): `label:a-b`, `label:a` (`+a` when signed), then `(+c)`.
/// Nothing when all are 0.
pub fn segment(label: &str, a: i32, b: i32, c: i32, signed: bool) -> String {
    if a == 0 && b == 0 && c == 0 {
        return String::new();
    }
    let mut s = format!("{label}:");
    if b != 0 {
        s += &format!("{a}-{b}");
    } else if a != 0 {
        s += &if signed { format!("{a:+}") } else { format!("{a}") };
    }
    if c != 0 {
        s += &format!("({c:+})");
    }
    s + "  "
}

/// What the box shows of a map object or item besides its name: the icon and rows.
pub fn details(o: &Object, prototypes: &Prototypes, intgame: &dyn Fn(u32) -> String) -> (Option<u32>, BoxDetails) {
    let int = |f: &str| prototypes.int(o, f).unwrap_or(0);
    let at = |i: usize, f: &str| o.ints(f).or_else(|| prototypes.of(o)?.ints(f)).and_then(|v| v.get(i).copied()).unwrap_or(0);
    let (hp, max) = hit_points(o, prototypes);
    let hp_text = format!("{hp}/{max}");
    let vulnerable = int("flags") & OF_INVULNERABLE == 0;
    let mut d = BoxDetails::default();
    let row = |row: usize, left: Option<BoxCell>, right: Option<BoxCell>| BoxRow { row, left, right };
    let icon = match o.object_type {
        arcanum_formats::object::TYPE_PORTAL => {
            d.rows.push(row(3, cell(intgame(lock_text(int("portal_flags"))), BLUE), cell(hp_text, BLUE)));
            let art = int("current_aid") as u32;
            Some(if art >> 28 == 3 && art & 0x400 != 0 { DOOR_ICON } else { WINDOW_ICON })
        }
        arcanum_formats::object::TYPE_CONTAINER => {
            d.rows.push(row(3, cell(intgame(lock_text(int("container_flags"))), BLUE), if vulnerable { cell(hp_text, BLUE) } else { None }));
            let basic = o.prototype.prototype().map(|p| p.saturating_sub(20));
            Some(CONTAINER_ICONS.iter().find(|c| Some(c.0) == basic).map_or(CONTAINER_ICON, |c| c.1))
        }
        arcanum_formats::object::TYPE_SCENERY => {
            if vulnerable {
                d.rows.push(row(3, None, cell(hp_text, BLUE)));
            }
            Some(SCENERY_ICON)
        }
        kind @ 5..=14 => {
            let complexity = int("item_magic_tech_complexity");
            let identified = complexity < 1 || int("item_flags") & OIF_IDENTIFIED != 0;
            let magick = |f: &str| if identified && complexity > 0 { int(f) } else { 0 };
            let magick_at = |i: usize, f: &str| if identified && complexity > 0 { at(i, f) } else { 0 };
            if complexity > 0 {
                d.title_colour = Some(MAGICK_NAME);
            }
            let sub_type = |f: &str| {
                let art = int(f) as u32;
                if art >> 28 == 6 { ((art >> 6) & 0xf) as usize } else { 0 }
            };
            let by_complexity = |n: u32| if complexity > 0 { n + 1 } else if complexity < 0 { n + 2 } else { n };
            let label = |k: u32| intgame(k);
            let stats = match kind {
                5 => {
                    let damage = |i: usize, k: u32| segment(&label(k), at(i, "weapon_damage_lower_idx"), at(i, "weapon_damage_upper_idx"), magick_at(i, "weapon_magic_damage_adj_idx"), false);
                    let range = int("weapon_range");
                    [damage(0, 43), damage(4, 44), segment(&label(45), int("weapon_bonus_to_hit"), 0, magick("weapon_magic_hit_adj"), true), segment(&label(46), if range == 1 { 0 } else { range }, 0, magick("weapon_magic_range_adj"), false), damage(1, 47), damage(3, 48), damage(2, 49)].concat()
                }
                7 => {
                    let resist = |i: usize, k: u32| segment(&label(k), at(i, "armor_resistance_adj_idx"), 0, magick_at(i, "armor_magic_resistance_adj_idx"), true);
                    [segment(&label(50), int("armor_ac_adj"), 0, magick("armor_magic_ac_adj"), false), resist(0, 51), resist(3, 52), resist(1, 53), resist(2, 54), resist(4, 55), segment(&label(56), int("armor_silent_move_adj"), 0, magick("armor_magic_silent_move_adj"), true)].concat()
                }
                6 => format!("{}: {}", label(6), int("ammo_quantity")),
                8 => format!("{}: {}", label(6), int("gold_quantity")),
                12 => format!("{}: {}", label(7), o.ints("key_ring_list_idx").or_else(|| prototypes.of(o)?.ints("key_ring_list_idx")).map_or(0, |k| k.len())),
                14 if int("generic_flags") & 2 != 0 => format!("{}: {:+}%", label(71), int("generic_usage_bonus")),
                14 if int("generic_flags") & 8 != 0 => format!("{}: {:+}%", label(72), int("generic_usage_bonus")),
                _ => String::new(),
            };
            let uses = if complexity > 0 && int("item_spell_mana_store") != 0 {
                let n = int("item_spell_mana_store");
                Some(if !identified { format!("{}: ??", label(19)) } else { format!("{}: {}", label(19), n.max(0)) })
            } else if kind == 14 && int("generic_flags") & 8 != 0 {
                Some(format!("{}: {}", label(19), int("generic_usage_count_remaining")))
            } else {
                None
            };
            d.rows.push(row(1, (!stats.is_empty()).then(|| BoxCell::new(stats.trim_end(), WHITE)), None));
            if let Some(uses) = uses {
                d.rows.push(row(2, None, cell(uses, WHITE)));
            }
            let quantity = match kind {
                6 => Some(int("ammo_quantity")),
                _ => None,
            };
            let weight = if kind == 8 { 0 } else { quantity.map_or(int("item_weight"), |q| int("item_weight") * q / 4) };
            let broken = int("current_aid") as u32 & 0x400 != 0;
            let shows_hp = !broken && (kind == 5 || kind == 7 || (kind == 14 && int("generic_flags") & 0x20 != 0));
            d.rows.push(row(3, cell(format!("{}: {} {}", label(4), weight, label(5)), BLUE), if shows_hp { cell(hp_text, BLUE) } else { None }));
            Some(match kind {
                5 => by_complexity(WEAPON_ICONS.get(sub_type("item_use_aid_fragment")).copied().unwrap_or(391)),
                6 => AMMO_ICONS.get(int("ammo_type").max(0) as usize).copied().unwrap_or(398),
                7 => {
                    let coverage = {
                        let art = int("item_inv_aid") as u32;
                        if art >> 28 == 6 { ((art >> 14) & 7) as usize } else { 0 }
                    };
                    by_complexity(if coverage == 0 { ARMOUR_ICONS.get(sub_type("item_use_aid_fragment")).copied().unwrap_or(402) } else { COVERAGE_ICONS.get(coverage).copied().unwrap_or(405) })
                }
                8 => 417,
                9 if complexity > 0 => 418,
                9 if complexity < 0 => match int("item_discipline") {
                    7 => 419,
                    0 => 420,
                    1 => 421,
                    _ => 423,
                },
                9 => 423,
                10 => 425,
                11 => 426,
                12 => 427,
                13 => WRITTEN_ICONS.get(int("written_subtype").max(0) as usize).copied().unwrap_or(428),
                _ => by_complexity(432),
            })
        }
        _ => None,
    };
    (icon, d)
}

/// A PC's level and gauges: `cur/max` for the player's own PC, else percentages.
pub fn pc_details(status: &arcanum_sim::PcStatus, own: bool, intgame: &dyn Fn(u32) -> String) -> BoxDetails {
    let level = BoxRow { row: 1, left: None, right: cell(format!("{}: {}", intgame(0), status.level), WHITE) };
    BoxDetails { title_colour: None, rows: vec![level], gauges: vec![], to_hit: None }.with_gauges(status.hit_points, status.fatigue, own, false)
}

/// An NPC's icon (by race), level and gauges (full: NPCs take no damage yet).
pub fn npc_details(o: &Object, prototypes: &Prototypes, intgame: &dyn Fn(u32) -> String, health: Option<(arcanum_sim::Pool, arcanum_sim::Pool)>) -> (Option<u32>, BoxDetails) {
    let stats = o.ints("critter_stat_base_idx").or_else(|| prototypes.of(o)?.ints("critter_stat_base_idx")).unwrap_or_default();
    let stat = |i: usize| stats.get(i).copied().unwrap_or(0);
    let icon = RACE_ICONS.get(stat(27).max(0) as usize).copied();
    let level = BoxRow { row: 1, left: None, right: cell(format!("{}: {}", intgame(0), stat(17)), WHITE) };
    let full = arcanum_sim::Pool::full(1);
    let (hp, fatigue) = health.unwrap_or((full, full));
    (icon, BoxDetails { title_colour: None, rows: vec![level], gauges: vec![], to_hit: None }.with_gauges(hp, fatigue, false, false))
}

trait Gauges {
    fn with_gauges(self, hp: arcanum_sim::Pool, fatigue: arcanum_sim::Pool, own: bool, poisoned: bool) -> Self;
}

impl Gauges for BoxDetails {
    /// Hit points in row 2, fatigue in row 3 (0x553f70).
    fn with_gauges(mut self, hp: arcanum_sim::Pool, fatigue: arcanum_sim::Pool, own: bool, poisoned: bool) -> Self {
        for (row, pool, (fill, empty), colour) in [(2, hp, if poisoned { POISONED_GAUGE } else { HP_GAUGE }, BLUE), (3, fatigue, FATIGUE_GAUGE, RED)] {
            let percent = (pool.fraction() * 100.0).round() as u32;
            self.gauges.push(BoxGauge { row, fill, empty, percent });
            let text = if own { format!("{}/{}", pool.current, pool.max) } else { format!("{percent}%") };
            self.rows.push(BoxRow { row, left: None, right: cell(text, colour) });
        }
        self
    }
}

/// The player's chance to hit (0x554830, simplified): the weapon skill's chance less the
/// difficulty (the weapon's to-hit, the Strength short), at least 0; the defender's dodge and
/// the original's 0x461620 term are not counted.
pub fn to_hit(sheet: &arcanum_rules::CharacterSheet, rules: &arcanum_rules::RulesData, weapon: Option<&Object>, prototypes: &Prototypes) -> i32 {
    use arcanum_sim::combat::{SKILL_MELEE, weapon_chance, weapon_of};
    let weapon = weapon.filter(|w| w.object_type == arcanum_formats::object::TYPE_WEAPON).map(|w| weapon_of(w, prototypes));
    let skill = weapon.as_ref().map_or(SKILL_MELEE, |w| w.skill);
    let chance = weapon_chance(skill, rules.skill_level(sheet, skill), rules.stats(sheet)[4]);
    let (bonus, min_st) = weapon.as_ref().map_or((0, 0), |w| (w.to_hit, w.min_strength));
    let short = (min_st - rules.stats(sheet)[0]).max(0);
    (chance + bonus - 5 * short).clamp(0, 100)
}

/// The fight and what the to-hit panel needs of the player's PC.
type Fighting<'w> = (Option<Res<'w, crate::combat_ui::CombatView>>, Res<'w, crate::pc_sheet::SheetHost>, Option<Res<'w, crate::create_character::CreationData>>, Res<'w, crate::inventory_screen::PcInventory>);

/// The windows that cover the world.
type Covering<'w> = (Option<Res<'w, crate::character_editor::CharacterPanel>>, Option<Res<'w, crate::inventory_screen::InventoryPanel>>, Option<Res<'w, crate::written_window::WrittenPanel>>, Option<Res<'w, crate::dialogue_ui::DialogueState>>);

type HoverActor<'a> = (Entity, &'a Sprite, &'a bevy::sprite::Anchor, &'a GlobalTransform, &'a Actor, Option<&'a ActorKind>);
type HoverObject<'a> = (Entity, &'a Sprite, &'a bevy::sprite::Anchor, &'a GlobalTransform, &'a MapObject);

/// What is under the cursor, as text for the box.
#[allow(clippy::too_many_arguments)]
fn hover_world(
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    client: Option<Res<ClientMap>>,
    actors: Query<HoverActor>,
    objects: Query<HoverObject>,
    images: Res<Assets<Image>>,
    (party, known, vfs, local): (Res<crate::actors::Party>, Res<KnownNpcs>, Option<Res<VfsResource>>, Res<crate::sim_bridge::LocalPc>),
    (panel, inventory, written, dialogue): Covering,
    mut messages: ResMut<MessageBox>,
    time: Option<Res<Time<Real>>>,
    (combat, sheet_host, data, pc_inventory): Fighting,
    health: Option<Res<crate::combat_ui::NpcHealth>>,
    mut texts: Local<Descriptions>,
    mut shown: Local<Option<Entity>>,
    (link, mut answers, mut examining): Examine,
) {
    // The host's answer to the examination waiting: shown, or nothing (0x57cd9b).
    for (seq, yes) in std::mem::take(&mut answers.0) {
        if let Some((waiting, message)) = examining.waiting.take() {
            if waiting != seq {
                examining.waiting = Some((waiting, message));
            } else if yes {
                messages.show_help(message, time.as_ref().map_or(0.0, |t| t.elapsed_secs_f64()));
            }
        }
    }
    let covered = panel.is_some_and(|p| p.open) || inventory.is_some_and(|p| p.open) || written.is_some_and(|w| w.open.is_some()) || dialogue.is_some_and(|d| d.talking());
    let cursor = windows.iter().find_map(|w| w.cursor_position().map(|c| (c, w.size())));
    let hit = match (cursor, client.as_ref()) {
        (Some((at, size)), Some(_)) if !covered && !crate::hud::covers(size, at, false) => cameras.iter().find_map(|(camera, t)| camera.viewport_to_world_2d(t, at).ok()).and_then(|world| {
            let critters = actors.iter().map(|(e, s, a, t, ..)| (e, s, a, t));
            crate::picking::pick(world, critters, &images).or_else(|| crate::picking::pick(world, objects.iter().filter(|o| o.4.object_type != TYPE_WALL).map(|(e, s, a, t, _)| (e, s, a, t)), &images))
        }),
        _ => None,
    };
    if hit == *shown {
        return;
    }
    let was = *shown;
    *shown = hit;
    let Some(entity) = hit else {
        examining.waiting = None;
        if was.is_some() {
            messages.clear();
        }
        return;
    };
    if !texts.loaded {
        texts.loaded = true;
        let guard = vfs.as_ref().and_then(|v| v.0.read().ok());
        let read = |p: &str| guard.as_ref().and_then(|v| v.read(p).ok()).and_then(|d| MesFile::parse(&d).ok()).unwrap_or_default();
        texts.description = read("mes/description.mes");
        texts.game = read("mes/gamedesc.mes");
        texts.intgame = read("mes/intgame.mes");
    }
    let Some(client) = client else { return };
    let map = &client.map;
    let intgame = |k: u32| texts.intgame.get(k).map(|t| t.trim().to_string()).unwrap_or_default();
    // What to ask the host about first: an NPC, or an object with an examine script here.
    let mut examine = None;
    let shown = if let Ok((.., actor, kind)) = actors.get(entity) {
        if matches!(kind.map(|k| &k.0), Some(EntityKind::Npc { .. })) {
            examine = Some(arcanum_sim::examine::Examined::Critter(actor.net_id));
        }
        match kind.map(|k| &k.0) {
            Some(EntityKind::Pc { .. }) => party.status.get(&actor.net_id).map(|s| (s.name.clone(), None, pc_details(s, local.0.is_some_and(|(id, _)| id == actor.net_id), &intgame))),
            Some(&EntityKind::Npc { prototype }) => map.prototypes.get(prototype).and_then(|p| {
                let name = critter_name(p, &map.prototypes, known.0.contains(&actor.net_id)).and_then(|n| texts.text(n))?;
                let (icon, mut details) = npc_details(p, &map.prototypes, &intgame, health.as_ref().and_then(|h| h.0.get(&actor.net_id).copied()));
                let me = local.0.map(|(id, _)| id);
                if combat.as_ref().is_some_and(|c| c.attacks(me))
                    && let (Some(sheet), Some(rules)) = (sheet_host.sheet(), data.as_ref().map(|d| &d.rules))
                {
                    let weapon = pc_inventory.shown().and_then(|inv| inv.items.iter().find(|i| i.location == arcanum_sim::combat::WEAPON_SLOT)).and_then(|i| map.prototypes.get(i.prototype));
                    details.to_hit = Some((intgame(20), format!("{}%", to_hit(sheet, rules, weapon, &map.prototypes))));
                }
                Some((name, icon, details))
            }),
            None => None,
        }
    } else if let Ok((.., object)) = objects.get(entity) {
        let item = |o: &Object| if (5..=14).contains(&o.object_type) { item_name(o, &map.prototypes) } else { map.prototypes.int(o, "description") };
        let scripted = |o: arcanum_sim::ScriptObject| map.attachment(o, arcanum_sim::examine::EVENT_EXAMINE).number != 0;
        examine = match (object.id, object.ground) {
            (Some(id), _) if scripted(arcanum_sim::ScriptObject::Map(id)) => Some(arcanum_sim::examine::Examined::Object(id)),
            (_, Some(id)) if arcanum_sim::ScriptObject::ground(id).is_some_and(scripted) => Some(arcanum_sim::examine::Examined::Ground(id)),
            _ => None,
        };
        let found: Option<Object> = match (object.id, object.ground) {
            (Some(id), _) => map.map_object(id).cloned(),
            (_, Some(id @ GroundItemId::Container { .. })) => map.container_object(id).cloned(),
            (_, Some(id)) => map.ground_item(id).and_then(|g| map.prototypes.get(g.prototype).cloned()),
            _ => None,
        };
        found.and_then(|o| {
            let name = item(&o).and_then(|n| texts.text(n))?;
            let (icon, d) = details(&o, &map.prototypes, &intgame);
            Some((name, icon, d))
        })
    } else {
        None
    };
    examining.waiting = None;
    match shown {
        Some((name, icon, details)) => {
            let message = BoxMessage { icon: icon.map_or(BoxIcon::Arcanum, BoxIcon::Interface), title: Some(name), body: String::new(), extra: None, details: Some(details) };
            match (examine, link.as_ref()) {
                (Some(target), Some(link)) => {
                    examining.seq += 1;
                    let seq = examining.seq;
                    let _ = link.0.send_intent(arcanum_sim::PlayerIntent::Examine { target, seq });
                    examining.waiting = Some((seq, message));
                    messages.clear();
                }
                _ => {
                    messages.show_help(message, time.map_or(0.0, |t| t.elapsed_secs_f64()));
                }
            }
        }
        None => messages.clear(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arcanum_formats::object::{TYPE_NPC, TYPE_WEAPON, Value};
    use arcanum_formats::test_util::ObjectBuilder;

    #[test]
    fn unidentified_items_and_unmet_npcs_show_their_unknown_names() {
        let prototypes = Prototypes::default();
        let sword = |complexity: i32, flags: i32| Object::parse_instance(&ObjectBuilder::new(TYPE_WEAPON).field("description", Value::Int32(100)).field("item_description_unknown", Value::Int32(101)).field("item_magic_tech_complexity", Value::Int32(complexity)).field("item_flags", Value::Int32(flags)).build()).unwrap();
        assert_eq!(item_name(&sword(0, 0), &prototypes), Some(100), "plain: known");
        assert_eq!(item_name(&sword(40, 0), &prototypes), Some(101), "magick, unidentified");
        assert_eq!(item_name(&sword(40, 1), &prototypes), Some(100), "identified");
        let npc = Object::parse_instance(&ObjectBuilder::new(TYPE_NPC).field("description", Value::Int32(200)).field("critter_description_unknown", Value::Int32(201)).build()).unwrap();
        assert_eq!((critter_name(&npc, &prototypes, true), critter_name(&npc, &prototypes, false)), (Some(200), Some(201)));
    }

    /// An int array value: dense, every element present.
    fn ints(v: &[i32]) -> Value {
        let data = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        let bits = (0..v.len()).fold(vec![0u32; v.len().div_ceil(32)], |mut b, i| {
            b[i / 32] |= 1 << (i % 32);
            b
        });
        Value::Array(Some(arcanum_formats::object::Array { elem_size: 4, id: 0, data, bitset: bits }))
    }

    fn intgame(k: u32) -> String {
        match k {
            4 => "Weight", 5 => "stone", 9 => "Locked", 10 => "Unlocked", 26 => "Jammed", 0 => "Level", 43 => "D", 45 => "TH", 46 => "RNG", 50 => "AC", 51 => "DR", 6 => "Quantity",
            _ => "?",
        }
        .into()
    }

    #[test]
    fn segments_read_as_the_original_writes_them() {
        assert_eq!(segment("D", 2, 8, 0, false), "D:2-8  ");
        assert_eq!(segment("TH", 5, 0, 0, true), "TH:+5  ");
        assert_eq!(segment("AC", 10, 0, 3, false), "AC:10(+3)  ");
        assert_eq!(segment("FT", 0, 0, 0, false), "", "all 0: nothing");
        assert_eq!((lock_text(1), lock_text(0), lock_text(3), lock_text(4)), (9, 10, 26, 27));
    }

    #[test]
    fn doors_weapons_and_npcs_show_their_rows() {
        use arcanum_formats::object::TYPE_PORTAL;
        let prototypes = Prototypes::default();
        let door = Object::parse_instance(&ObjectBuilder::new(TYPE_PORTAL).field("portal_flags", Value::Int32(1)).field("hp_pts", Value::Int32(50)).field("hp_damage", Value::Int32(10)).build()).unwrap();
        let (icon, d) = details(&door, &prototypes, &intgame);
        assert_eq!(icon, Some(WINDOW_ICON), "no door art bit");
        assert_eq!(d.rows, vec![BoxRow { row: 3, left: Some(BoxCell::new("Locked", BLUE)), right: Some(BoxCell::new("40/50", BLUE)) }]);
        let sword = Object::parse_instance(
            &ObjectBuilder::new(TYPE_WEAPON)
                .field("weapon_damage_lower_idx", ints(&[2, 0, 0, 0, 0]))
                .field("weapon_damage_upper_idx", ints(&[8, 0, 0, 0, 0]))
                .field("weapon_bonus_to_hit", Value::Int32(5))
                .field("weapon_range", Value::Int32(1))
                .field("item_weight", Value::Int32(40))
                .field("hp_pts", Value::Int32(100))
                .build(),
        )
        .unwrap();
        let (icon, d) = details(&sword, &prototypes, &intgame);
        assert_eq!(icon, Some(391));
        assert_eq!(d.title_colour, None, "not magick");
        assert_eq!(d.rows[0].left, Some(BoxCell::new("D:2-8  TH:+5", WHITE)), "a range of 1 is none");
        assert_eq!(d.rows.last().unwrap().left, Some(BoxCell::new("Weight: 40 stone", BLUE)));
        assert_eq!(d.rows.last().unwrap().right, Some(BoxCell::new("100/100", BLUE)));
        let npc = Object::parse_instance(&ObjectBuilder::new(TYPE_NPC).field("critter_stat_base_idx", ints(&(0..28).map(|i| if i == 17 { 7 } else if i == 27 { 2 } else { 0 }).collect::<Vec<i32>>())).build()).unwrap();
        let (icon, d) = npc_details(&npc, &prototypes, &intgame, None);
        assert_eq!(icon, Some(376), "an elf");
        assert_eq!(d.rows[0].right, Some(BoxCell::new("Level: 7", WHITE)));
        assert_eq!(d.gauges.iter().map(|g| (g.row, g.fill, g.percent)).collect::<Vec<_>>(), [(2, 463, 100), (3, 465, 100)]);
        assert_eq!(d.rows[1].right, Some(BoxCell::new("100%", BLUE)));
        let hurt = (arcanum_sim::Pool { current: 10, max: 40 }, arcanum_sim::Pool { current: 0, max: 20 });
        let (_, d) = npc_details(&npc, &prototypes, &intgame, Some(hurt));
        assert_eq!(d.gauges.iter().map(|g| g.percent).collect::<Vec<_>>(), [25, 0], "the host's numbers");
    }

    #[test]
    fn the_to_hit_panel_shows_the_weapon_chance() {
        let rules = arcanum_rules::RulesData::default();
        let sheet = arcanum_rules::CharacterSheet::default();
        let prototypes = Prototypes::default();
        assert_eq!(to_hit(&sheet, &rules, None, &prototypes), 25, "bare hands: melee 25");
        let heavy = Object::parse_instance(&ObjectBuilder::new(TYPE_WEAPON).field("weapon_bonus_to_hit", Value::Int32(10)).field("weapon_min_strength", Value::Int32(60)).field("weapon_ammo_type", Value::Int32(10000)).build()).unwrap();
        assert_eq!(to_hit(&sheet, &rules, Some(&heavy), &prototypes), 0, "far too heavy");
    }

    #[test]
    fn high_numbers_read_the_game_descriptions() {
        let texts = Descriptions { loaded: true, intgame: MesFile::default(), description: MesFile::parse(b"{100}{Sword}").unwrap(), game: MesFile::parse(b"{30001}{Virgil}").unwrap() };
        assert_eq!((texts.text(100), texts.text(30001), texts.text(5)), (Some("Sword".into()), Some("Virgil".into()), None));
    }
}
