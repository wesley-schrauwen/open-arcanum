//! Turn-based combat (`plans/combat.md`; Arcanum.exe combat module 0x4b3xxx-0x4b8xxx, attack
//! 0x4b3170, damage 0x4b6680 / 0x4b6860 / 0x4b4390).
//!
//! - **The encounter** ([`Combat`]): a PC's attack starts one round it (0x4b71e0): the PCs and
//!   the fighting NPCs within [`ENGAGE_RADIUS`] tiles, PCs first, NPCs after in the order found
//!   (0x4b7eb0 sorts by nothing else). Each round the list is gathered again (0x4b73a0); a round
//!   with no fighting NPC left ends it. PCs outside the radius move freely (owner's rule).
//! - **Turns and action points** (0x4b6e70): `max(5, speed)` at the start of a turn, none kept.
//!   A step costs [`STEP_AP`] (walking, 0x4b7ba0); an attack its weapon's cost
//!   ([`attack_cost`], 0x4b7c30). Out of points, or `PlayerIntent::EndTurn` (0x4b7c90), the next
//!   critter's turn starts. A fight with no NPC fighting or no PC standing ends at once.
//! - **NPC turns** ([`npc_turn`]): at the start, flee by its `ai_params.mes` row
//!   ([`should_flee`]), or give up ([`Surrendered`]) with nowhere to run; else, one turn in 20,
//!   step back to its column 15 distance; else attack its target (or the nearest standing PC of
//!   the fight), walking into reach first, until out of points.
//! - **The attack** ([`resolve`], 0x4b3170): see the function. A hit makes an NPC victim fight
//!   its attacker (0x4a9650) and join the encounter (0x4b7e00).
//! - **Others join** ([`rally`], 0x4a9650 → 0x4a9b80 / 0x4a9e10): followers of either side;
//!   NPCs within [`ENGAGE_RADIUS`] tiles of the victim (radius table 0x5b50c0 `{2, 8, 15, 1}`,
//!   index 2) on a side ([`allied`], 0x4ae3a0) who notice that side's critter; neutrals who notice
//!   the attacker flee. A PC attacker's victim likes it no more than its row's column 10.
//! - **Damage to NPCs** is kept as `hp_damage` in the sector's changes; at 0 hit points they die
//!   as action 0x3a kills (0x45da20), and the experience is shared (team rule).
//!
//! - **PCs hurt:** an NPC's hit goes on the PC's sheet (hit points, poison, fatigue) as script
//!   damage does; a PC with no hit points left takes no turns.
//!
//! Not built yet: stunning, knocking down and knocking out, scars, dropped and damaged items,
//! critical misses' effects, the light, range and
//! cover terms, ammunition, PC death.

use arcanum_formats::map::TileLoc;
use arcanum_formats::object::{Object, Prototypes};
use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{HostRng, LedgerDelta, NetId, PlayerId};

/// Who is gathered into a fight, in tiles (`0x5b57b8`).
pub const ENGAGE_RADIUS: u32 = 15;
/// The fewest action points a turn starts with (0x4b6e70).
pub const MIN_AP: i32 = 5;
/// Action points a walked step costs (0x4b7ba0: path cost × 2).
pub const STEP_AP: i32 = 2;
/// The weapon slot.
pub const WEAPON_SLOT: i32 = 1004;
/// The gloves slot (unarmed bonus damage, field 0x9e).
pub const GLOVES_SLOT: i32 = 1007;
/// Damage types: normal, poison, electrical, fire, fatigue.
pub const DAMAGE_TYPES: usize = 5;
/// The resistance each damage type is cut by (table 0x5b5804: damage, poison, electrical, fire,
/// damage).
const RESISTANCE_OF: [usize; DAMAGE_TYPES] = [0, 3, 2, 1, 0];
/// Skills: bow, dodge, melee, throwing; firearms is tech skill 1 (13).
pub const SKILL_BOW: usize = 0;
pub const SKILL_DODGE: usize = 1;
pub const SKILL_MELEE: usize = 2;
pub const SKILL_THROWING: usize = 3;
pub const SKILL_FIREARMS: usize = 13;
/// Backstab, a basic skill (its table chance is 0: it adds to blows instead, 0x45f9d0).
pub const SKILL_BACKSTAB: usize = 4;
/// The highest chance of a skill flagged 0x800 in 0x5b6f64 (0x4c63b6 / 0x4c6aaf: dodge among
/// the combat skills), and the most a resistance holds.
pub const MAX_CHANCE: i32 = 95;

/// A skill's chance at level `level` (0x4c62e0 / 0x4c69f0, table 0x4c63dc): dodge 2 a level,
/// throwing 7 + 25, bow and melee 5 + 25, firearms (tech) 5 + 25; + 10 at Intelligence 20 or
/// more ([`crate::bright_bonus`], 0x4c6369). Only dodge stops at 95: bow, melee, throwing and
/// firearms lack flag 0x800 (0x5b6f64: 0x58f / 0x58d; the roll 0x4c7259 caps nothing), so a
/// master's chance passes 100 and outweighs the difficulty.
pub fn weapon_chance(skill: usize, level: i32, intelligence: i32) -> i32 {
    chance_against(skill, level, intelligence, false)
}

/// `spell_flags` 0x400000 of a target firearms aim worse at (0x4c6a34-0x4c6a47).
pub const OSF_FIREARM_WARD: i32 = 0x40_0000;

/// [`weapon_chance`] against a target: firearms 20 less at one of [`OSF_FIREARM_WARD`]
/// (0x4c6a47, before the Intelligence bonus and the cap).
pub fn chance_against(skill: usize, level: i32, intelligence: i32, warded: bool) -> i32 {
    let level = level.max(0);
    let chance = match skill {
        SKILL_DODGE => 2 * level,
        SKILL_THROWING => 7 * level + 25,
        // Tech index 1 of 0x4c69f0 (checked), less 20 against a warded target.
        SKILL_FIREARMS => 5 * level + 25 - if warded { 20 } else { 0 },
        _ => 5 * level + 25,
    };
    let chance = (chance + crate::bright_bonus(intelligence)).max(0);
    if skill == SKILL_DODGE { chance.min(MAX_CHANCE) } else { chance }
}

/// An attack's action points for a weapon of speed `speed` (0x4b7c30).
pub fn attack_cost(speed: i32) -> i32 {
    if speed > 20 { if speed <= 24 { 2 } else { 1 } } else { (8 - speed / 3).max(1) }
}

/// What an attacker strikes with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Weapon {
    /// The skill it uses.
    pub skill: usize,
    pub to_hit: i32,
    pub damage: [(i32, i32); DAMAGE_TYPES],
    /// The magick part of each damage, once identified.
    pub magic_damage: [i32; DAMAGE_TYPES],
    pub speed: i32,
    /// In tiles; 1 or less for melee.
    pub range: i32,
    pub min_strength: i32,
    /// `weapon_magic_crit_hit_chance` (field 0x82), `weapon_magic_crit_hit_effect` (0x83) and
    /// `weapon_crit_hit_chart` (0x81).
    pub crit_chance: i32,
    pub crit_effect: i32,
    pub crit_chart: i32,
    /// `weapon_ammo_type` (0 arrows, 1 bullets, 2 charges, 3 fuel; [`NO_AMMO`] none) and
    /// `weapon_ammo_consumption`, rounds a shot.
    pub ammo_type: i32,
    pub ammo_per_shot: i32,
    /// `weapon_crit_miss_chart` (0x84), `weapon_magic_crit_miss_chance` (0x85),
    /// `weapon_magic_crit_miss_effect` (0x86).
    pub miss_chart: i32,
    pub miss_chance: i32,
    pub miss_effect: i32,
    /// Hit points lost in percent (the critical miss's wear term, 0x4c82e0).
    pub worn: i32,
    /// `weapon_flags` [`WEAPON_WEARS_ARMOUR`]: each hit wears the armour it lands on.
    pub wears_armour: bool,
    /// The kind of its wielded art (`item_use_aid_fragment`, field 0x5f, an item art: bits 6-9,
    /// 0x5044f0): Backstab takes 2, and 3, 4 or 7 from Expert (0x45fa27-0x45fa53).
    pub use_kind: u32,
}

/// `weapon_ammo_type` of weapons that use none (0x465ca0).
pub const NO_AMMO: i32 = 10000;
/// Object type of ammunition.
pub const TYPE_AMMO: u32 = 6;

impl Weapon {
    /// Whether it shoots ammunition.
    pub fn uses_ammo(&self) -> bool {
        self.ammo_type != NO_AMMO && self.ammo_per_shot > 0
    }
}

/// Rounds of ammunition type `ammo_type` in `inventory` (0x465820: the stack's `ammo_quantity`).
pub fn ammo_count(inventory: &crate::Inventory, ammo_type: i32, prototypes: &Prototypes) -> i32 {
    inventory.items.iter().filter(|i| is_ammo_of(i.prototype, ammo_type, prototypes)).map(|i| i.quantity).sum()
}

fn is_ammo_of(prototype: u32, ammo_type: i32, prototypes: &Prototypes) -> bool {
    prototypes.get(prototype).is_some_and(|p| p.object_type == TYPE_AMMO && prototypes.int(p, "ammo_type") == Some(ammo_type))
}

/// The weapon an NPC picks for its hands (0x465170, slot 1004): among its carried weapons with
/// rounds enough (0x465820 against `weapon_ammo_consumption`), the best `chance × damage / 100`
/// (the weapon skill's chance, 0x4c62e0; damage the sum of each type's mean, 0x465f70); with a
/// foe `distance` tiles away, one that reaches it first (0x465e30 against 0x441ae0). Its location; `None`: bare hands
/// (0x464c50). Ties go to the later item (builder's call: the original's larger handle).
pub fn choose_weapon(inventory: &crate::Inventory, prototypes: &Prototypes, fighter: &Fighter, distance: Option<u32>) -> Option<i32> {
    let mut best: Option<(i32, i32)> = None;
    let mut reaching: Option<(i32, i32)> = None;
    // A broken weapon cannot be wielded (0x464d9f).
    for item in inventory.items.iter().filter(|i| !i.wear.broken) {
        let Some(o) = prototypes.get(item.prototype).filter(|o| o.object_type == arcanum_formats::object::TYPE_WEAPON) else { continue };
        let weapon = weapon_of(o, prototypes);
        if weapon.uses_ammo() && ammo_count(inventory, weapon.ammo_type, prototypes) < weapon.ammo_per_shot {
            continue;
        }
        let damage: i32 = (0..DAMAGE_TYPES).map(|i| (weapon.damage[i].0 + weapon.damage[i].1) / 2 + weapon.magic_damage[i]).sum();
        let armed = Fighter { weapon: Some(weapon), ..fighter.clone() };
        let skill = armed.skill();
        let score = weapon_chance(skill, armed.level(skill), armed.intelligence) * damage / 100;
        if best.is_none_or(|b| score >= b.0) {
            best = Some((score, item.location));
        }
        if distance.is_some_and(|d| armed.reach() >= d) && reaching.is_none_or(|b| score >= b.0) {
            reaching = Some((score, item.location));
        }
    }
    reaching.or(best).map(|b| b.1)
}

/// `npc_flags` (field 0x118): pick every worn item again (set when an item enters its
/// inventory, 0x46695a / 0x467c50), and pick the weapon again (set when a shot lacks rounds,
/// 0x4ae006); [`rearm_npcs`] clears them (0x4a87f5 / 0x4a8840).
pub const ONF_REPICK_ALL: i32 = 0x40;
pub const ONF_REPICK_WEAPON: i32 = 0x80;

/// NPCs asked to pick their weapon again ([`ONF_REPICK_ALL`], [`ONF_REPICK_WEAPON`]) do so
/// (0x4a8570 at 0x4a87c4 / 0x4a880a → 0x465170 for slot 1004, [`choose_weapon`]) for the foe
/// they fight: the original checks on its AI ticks and attacks (0x4a84f0); here each tick, for
/// NPCs in a fight only (builder's call: 0x4a8570's gates 0x4233d0 / field 0x13 & 0x102 are not
/// decoded, and re-picking at peace would arm shopkeepers with the weapons sold to them; the
/// flag waits for the next fight). Not followers (0x4a8651), nor the dead or down (0x4a8589 /
/// 0x4a8796). With 0x40, every other worn slot is picked again too ([`wear_best`], 0x4654f0).
pub fn rearm_npcs(mut map: ResMut<crate::LoadedMap>, mut world: ResMut<crate::WorldState>, vfs: Option<Res<crate::SimVfs>>, npcs: Query<(&crate::NpcSource, &crate::Position, &Fighting), Without<crate::follow::Following>>, critters: Query<(&NetId, &crate::Position)>) {
    let guard = vfs.as_ref().and_then(|v| v.0.read().ok());
    for (source, at, fighting) in &npcs {
        let Some(flags) = map.npc_flags(source.sector, source.index).filter(|f| f & (ONF_REPICK_ALL | ONF_REPICK_WEAPON) != 0) else { continue };
        if map.is_dead(source.sector, source.index) || map.npc_down(source.sector, source.index) {
            continue;
        }
        let distance = critters.iter().find(|c| *c.0 == fighting.target).map(|c| reach_between(at.0, c.1.0));
        map.set_npc_flags(source.sector, source.index, flags & !(ONF_REPICK_ALL | ONF_REPICK_WEAPON));
        if let (Some(inv), Some(o)) = (map.npc_inventory(source.sector, source.index), map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize))) {
            let fighter = npc_weapon_fighter(o, &map.prototypes);
            let pick = choose_weapon(&inv, &map.prototypes, &fighter, distance);
            let (prototypes, art) = (map.prototypes.clone(), map.art.clone());
            let info = crate::item_info(&prototypes, |id| crate::world::art_size(guard.as_deref(), art.as_deref(), id));
            let mut changed = wield_pick(&inv, pick, &info);
            // Asked for all (0x40): every other worn slot too (0x4654f0).
            if flags & ONF_REPICK_ALL != 0 {
                let wearer = npc_wearer(o, &prototypes);
                let base = changed.clone().unwrap_or_else(|| inv.clone());
                let worn = wear_best(&base, wearer, &info);
                if worn != base {
                    changed = Some(worn);
                }
            }
            if let Some(changed) = changed {
                map.set_npc_inventory(source.sector, source.index, changed);
            }
        }
        crate::ground::save_change(&mut world, &map, source.sector);
    }
}

/// An NPC's race and gender for the fit test, from its art: the body's race (human, dwarf,
/// gnome, half-ogre, elf: table 0x5c5170 read back; builder's call) and the art's sex.
pub fn npc_wearer(o: &Object, prototypes: &Prototypes) -> crate::Wearer {
    let art = prototypes.art_id(o).map(arcanum_formats::art_id::CritterArt::from_id).unwrap_or_default();
    let race = [0u8, 1, 4, 7, 2].get(art.body as usize).copied().unwrap_or(0);
    crate::Wearer { race, gender: if art.male { arcanum_rules::MALE } else { arcanum_rules::FEMALE } }
}

/// What it wears after picking again every slot but the weapon's (0x4654f0: 1000-1008 →
/// 0x465170): in each, the item of the highest `item_worth` (at least 2, 0x461f20 at 0x461f38)
/// that passes the fit test, the worn one first and replaced only by a higher worth (0x46545c);
/// a worn item that no longer fits comes off (0x4651d4). Items already worn elsewhere are left
/// (0x461340).
pub fn wear_best(inv: &crate::Inventory, wearer: crate::Wearer, info: &impl Fn(u32) -> Option<crate::ItemInfo>) -> crate::Inventory {
    let mut inv = inv.clone();
    let slots = (crate::FIRST_SLOT..=crate::LAST_SLOT).filter(|&s| s != WEAPON_SLOT);
    let worth = |p: u32| info(p).map_or(i32::MIN, |i| i.worth.max(2));
    for slot in slots {
        let fits = |inv: &crate::Inventory, p: u32| info(p).is_some_and(|i| {
            let without = crate::Inventory { items: inv.items.iter().filter(|x| x.location != slot).cloned().collect(), ..inv.clone() };
            without.fit_test(&i, slot, wearer, info).is_ok()
        });
        let mut best = i32::MIN;
        if let Some(worn) = inv.items.iter().find(|i| i.location == slot).map(|i| i.prototype) {
            if fits(&inv, worn) {
                best = worth(worn);
            } else {
                let cells = info(worn).map_or((1, 1), |i| i.cells);
                if let Some(off) = crate::Inventory::free_spot(&inv.occupied(info), cells).and_then(|spot| inv.move_item(slot, spot, wearer, info).ok()) {
                    inv = off;
                }
            }
        }
        let pick = inv.items.iter().filter(|i| !(crate::FIRST_SLOT..=crate::LAST_SLOT).contains(&i.location) && inv.in_grid(i.location)).filter(|i| !i.wear.broken && fits(&inv, i.prototype)).fold(None, |b: Option<(i32, i32)>, i| {
            let w = worth(i.prototype);
            if w > b.map_or(best, |b| b.0) { Some((w, i.location)) } else { b }
        });
        if let Some(moved) = pick.and_then(|(_, from)| inv.move_item(from, slot, wearer, info).ok()) {
            inv = moved;
        }
    }
    inv
}

/// What an NPC's weapon choice needs of it: its weapon skills.
fn npc_weapon_fighter(o: &Object, prototypes: &Prototypes) -> Fighter {
    Fighter { levels: [SKILL_BOW, SKILL_DODGE, SKILL_MELEE, SKILL_THROWING, SKILL_FIREARMS].map(|k| (k, npc_skill_level(o, prototypes, k))).to_vec(), ..Default::default() }
}

/// `inv` with `pick` wielded (slot 1004, 0x464ae0), or with its weapon put away for bare hands
/// when `None` (0x464c50); `None` when nothing changes or it cannot be done.
pub fn wield_pick(inv: &crate::Inventory, pick: Option<i32>, info: &impl Fn(u32) -> Option<crate::ItemInfo>) -> Option<crate::Inventory> {
    let wielding = inv.items.iter().find(|i| i.location == WEAPON_SLOT);
    let wearer = crate::Wearer { race: 0, gender: 0 };
    match pick {
        Some(WEAPON_SLOT) => None,
        Some(from) => inv.move_item(from, WEAPON_SLOT, wearer, info).ok(),
        None => {
            let cells = info(wielding?.prototype).map_or((1, 1), |i| i.cells);
            let spot = crate::Inventory::free_spot(&inv.occupied(info), cells)?;
            inv.move_item(WEAPON_SLOT, spot, wearer, info).ok()
        }
    }
}

/// Takes `rounds` of `ammo_type` from `inventory`'s stacks, an emptied stack gone (0x4658e0).
pub fn take_ammo(inventory: &mut crate::Inventory, ammo_type: i32, rounds: i32, prototypes: &Prototypes) {
    let mut left = rounds;
    for item in inventory.items.iter_mut().filter(|i| is_ammo_of(i.prototype, ammo_type, prototypes)) {
        let taken = left.min(item.quantity);
        item.quantity -= taken;
        left -= taken;
    }
    inventory.items.retain(|i| i.quantity > 0 || !is_ammo_of(i.prototype, ammo_type, prototypes));
}

/// One side of an attack.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fighter {
    /// Its skill levels by skill (bow, dodge, melee, throwing, firearms as 13).
    pub levels: Vec<(usize, i32)>,
    pub strength: i32,
    /// Stat 9.
    pub damage_bonus: i32,
    /// `None`: unarmed.
    pub weapon: Option<Weapon>,
    /// Unarmed damage per type (PCs 1-5 normal and fatigue, more with gloves; critters
    /// `npc_damage_idx`).
    pub unarmed: [(i32, i32); DAMAGE_TYPES],
    /// Resistances in percent: damage, fire, electrical, poison, magick.
    pub resistances: [i32; 5],
    /// Its armour class (0x43d880): the `AC` field, its worn armour's and its AC adjustment
    /// (stat 10), at most 95.
    pub armour_class: i32,
    /// How much easier it is to hit, helpless (0x4c8609-0x4c86dc: 50 or 30).
    pub helpless: i32,
    /// What this shot's distance and line add (attackers: [`missile_difficulty`]).
    pub situation: i32,
    /// The skills it is trained in (a weapon's speed + 5, 0x465cf0).
    pub trained: Vec<usize>,
    /// A PC (an NPC's critical hits on it only multiply by 1.5 and never blind it).
    pub pc: bool,
    /// Constitution, against critical injuries (0x45f060).
    pub constitution: i32,
    /// Intelligence: at 20 or more its chances rise by 10 ([`crate::bright_bonus`]).
    pub intelligence: i32,
    /// Its injuries (`critter_flags` 0x80-0x400: [`OCF_BLINDED`] …).
    pub injuries: i32,
    /// It wears a helmet (slot 1000).
    pub helmet: bool,
    /// `critter_flags` 0x4: undead, not injured by critical hits.
    pub undead: bool,
    /// Rounds it carries for its weapon's ammunition.
    pub ammo: i32,
    /// `critter_crit_hit_chart` (0xe3): only 0 can be knocked down.
    pub crit_chart: i32,
    /// Stunned now (`critter_flags` 0x20): not stunned again.
    pub stunned: bool,
    /// Lying from a knock-down ([`KnockedDown`]): neither knocked down again (0x435a90 tests
    /// animation 7) nor stunned.
    pub knocked_down: bool,
    /// Its Backstab level and training rank.
    pub backstab: (i32, u8),
    /// A fate point waits on this blow (attackers only, 0x4b3697-0x4b36f5): a PC's "critical
    /// success on next attack" (option 2) makes it a critical hit whatever the roll and the
    /// dodge; a PC target's "critical failure on next opponent attack" (3) a critical miss.
    pub fate: Fate,
    /// Its level (stat 0x11), against an unnoticed stab's critical (0x4c829c).
    pub level: i32,
    /// This blow's stab ([`stab_of`]; attackers only).
    pub stab: Stab,
    /// Its `critter_flags` (NPCs).
    pub flags: i32,
    /// `spell_flags` [`SPELL_NO_DAMAGE`] (NPCs): it takes no damage.
    pub no_damage: bool,
    /// `spell_flags` [`OSF_FIREARM_WARD`] (NPCs): firearms aim 20 worse at it.
    pub warded: bool,
}

/// `critter_flags` 0x20000000: mechanical (with [`OCF_UNDEAD`], never poisoned: 0x4b441d).
pub const OCF_MECHANICAL: i32 = crate::pickpocket::OCF_MECHANICAL;
/// `spell_flags` (field 0x14) 0x10000: all damage zeroed (0x4b442e).
pub const SPELL_NO_DAMAGE: i32 = 0x10000;

/// Cuts damage `dealt` (by type) as 0x4b4390 does for the target: undead or mechanical
/// critters take no poison (0x4b441d → 0x4b4425); else one of `spell_flags` 0x10000 takes
/// nothing (0x4b442e → 0x4b4435).
pub fn shield_damage(dealt: &mut [i32; DAMAGE_TYPES], critter_flags: i32, no_damage: bool) {
    if critter_flags & (OCF_UNDEAD | OCF_MECHANICAL) != 0 {
        dealt[1] = 0;
    } else if no_damage {
        *dealt = [0; DAMAGE_TYPES];
    }
}

impl Fighter {
    /// The same with the rounds it carries counted.
    fn with_ammo(self, inventory: &crate::Inventory, prototypes: &Prototypes) -> Self {
        let ammo = self.weapon.as_ref().filter(|w| w.uses_ammo()).map_or(0, |w| ammo_count(inventory, w.ammo_type, prototypes));
        // The weapon's wear in percent, for its critical misses.
        let worn = inventory.items.iter().find(|i| i.location == WEAPON_SLOT).map_or(0, |i| {
            let max = crate::repair::item_max_hp(i, prototypes);
            if max > 0 { i.wear.damage.min(max) * 100 / max } else { 0 }
        });
        let weapon = self.weapon.map(|w| Weapon { worn, ..w });
        Self { ammo, weapon, ..self }
    }

    /// Whether its weapon needs ammunition it has not got (0x432cf0).
    pub fn out_of_ammo(&self) -> bool {
        self.weapon.as_ref().is_some_and(|w| w.uses_ammo() && self.ammo < w.ammo_per_shot)
    }
}

/// `critter_flags` 0x4000000: not knocked out by critical hits (0x4b5f40).
pub const OCF_NO_KNOCK_OUT: i32 = 0x0400_0000;

/// `critter_flags`: undead, blinded, one / both arms crippled, legs crippled (0x4b5f40).
pub const OCF_UNDEAD: i32 = 0x4;
pub const OCF_BLINDED: i32 = 0x80;
pub const OCF_CRIPPLED_ARM: i32 = 0x100;
pub const OCF_CRIPPLED_ARMS: i32 = 0x200;
pub const OCF_CRIPPLED_LEGS: i32 = 0x400;
/// All of them.
pub const OCF_INJURIES: i32 = OCF_BLINDED | OCF_CRIPPLED_ARM | OCF_CRIPPLED_ARMS | OCF_CRIPPLED_LEGS;
/// The helmet slot.
pub const HELMET_SLOT: i32 = 1000;

/// Where a critical hit lands (0x4b65a0): torso, head, arms, legs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyPart {
    Torso,
    Head,
    Arms,
    Legs,
}

impl BodyPart {
    /// Its number (0x4b65a0's).
    pub fn number(self) -> u8 {
        match self {
            Self::Torso => 0,
            Self::Head => 1,
            Self::Arms => 2,
            Self::Legs => 3,
        }
    }

    /// The slot of the armour there (0x4b54b0): helmet 1000, body 1006, gloves 1007, boots 1008.
    pub fn slot_of(number: u8) -> i32 {
        match number {
            1 => HELMET_SLOT,
            2 => 1007,
            3 => 1008,
            _ => 1006,
        }
    }

    /// From a 1-100 roll: up to 70 the torso, 85 the legs, 95 the arms, else the head.
    pub fn of_roll(roll: i32) -> Self {
        match roll {
            ..=70 => Self::Torso,
            71..=85 => Self::Legs,
            86..=95 => Self::Arms,
            _ => Self::Head,
        }
    }
}

/// The attacker's difficulty from its injuries (0x4c8430): blinded + 30, one arm crippled + 20,
/// both + 50.
pub fn injury_difficulty(injuries: i32) -> i32 {
    let blind = if injuries & OCF_BLINDED != 0 { 30 } else { 0 };
    let arms = if injuries & OCF_CRIPPLED_ARMS != 0 {
        50
    } else if injuries & OCF_CRIPPLED_ARM != 0 {
        20
    } else {
        0
    };
    blind + arms
}

impl Fighter {
    fn level(&self, skill: usize) -> i32 {
        self.levels.iter().find(|l| l.0 == skill).map_or(0, |l| l.1)
    }

    /// The skill its attacks use.
    pub fn skill(&self) -> usize {
        self.weapon.as_ref().map_or(SKILL_MELEE, |w| w.skill)
    }

    /// Its weapon's speed (0x465cf0): bare hands 10; a weapon's, + 5 when trained in its skill.
    pub fn speed(&self) -> i32 {
        match &self.weapon {
            None => UNARMED_SPEED,
            Some(w) => w.speed + if self.trained.contains(&w.skill) { 5 } else { 0 },
        }
    }

    /// An attack's action points.
    pub fn attack_cost(&self) -> i32 {
        attack_cost(self.speed())
    }

    /// How far it reaches, in tiles.
    pub fn reach(&self) -> u32 {
        self.weapon.as_ref().map_or(1, |w| w.range.max(1) as u32)
    }
}

/// How an attack went.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttackOutcome {
    pub hit: bool,
    pub critical: bool,
    pub dodged: bool,
    /// A critical miss (0x4c82e0); its effects are not built.
    pub fumbled: bool,
    /// Hit points, poison and fatigue taken.
    pub hp: i32,
    pub poison: i32,
    pub fatigue: i32,
    /// The critical multiplier in percent (150, 200, 300), 100 for none.
    pub multiplier: i32,
    /// Injuries this hit gave (`critter_flags` bits it did not have).
    #[serde(default)]
    pub injured: i32,
    /// Where a critical hit landed (0 torso, 1 head, 2 arms, 3 legs, as 0x4b65a0).
    #[serde(default)]
    pub part: u8,
    /// Items it wore or broke ([`ARMOUR_DAMAGED`] …).
    #[serde(default)]
    pub items: u8,
    /// It killed the target (the client plays the death).
    #[serde(default)]
    pub killed: bool,
    /// Critical effects on the target ([`STUNNED`], [`KNOCKED_DOWN`]); on a critical miss, what
    /// befell the attacker ([`WEAPON_DESTROYED`] …).
    #[serde(default)]
    pub effects: u8,
    /// A critical miss's effects of the attacker's own critical on itself, and its injuries.
    #[serde(default)]
    pub fumble_effects: u8,
    #[serde(default)]
    pub fumble_injured: i32,
    /// A critical miss's wear on the attacker's own items, as [`AttackOutcome::items`] (its
    /// critical on itself, 0x4b6410 → 0x4b5f40), where [`AttackOutcome::part`] says.
    #[serde(default)]
    pub fumble_items: u8,
    /// A thrown weapon's blow (the attack animation 14, 0x42bacb).
    #[serde(default)]
    pub thrown: bool,
    /// The attacker's weapon speed (0x465cf0: 10 bare-handed; the weapon's + 5 when trained),
    /// for the swing's frame time ([`swing_frame_millis`]).
    #[serde(default)]
    pub speed: i32,
}

/// [`AttackOutcome::effects`]: stunned (`combat.mes` 16, `critter_flags` 0x20), knocked down
/// (18), the weapon destroyed (22), all its ammunition lost or five shots' worth (9).
pub const STUNNED: u8 = 1;
pub const KNOCKED_DOWN: u8 = 2;
pub const WEAPON_DESTROYED: u8 = 4;
pub const AMMO_LOST_ALL: u8 = 8;
pub const AMMO_LOST: u8 = 16;
/// Knocked out (`combat.mes` 17; 0x4b5f40 flag 0x100): fatigue to its most + 10-20.
pub const KNOCKED_OUT: u8 = 32;

/// A stunned critter (0x4357b0, anim goal 57): it loses its turns, 3 off the count each, until
/// the count is under 1 (0x427000); its count starts at `(20 - CO) / 2`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stunned {
    pub left: i32,
}

/// The stun count for Constitution `co` (0x43581e).
pub fn stun_count(co: i32) -> i32 {
    (20 - co) / 2
}

/// [`AttackOutcome::items`]: the armour where it landed, or the target's weapon, damaged
/// (`combat.mes` 7 / 8) or broken (24 / 23).
pub const ARMOUR_DAMAGED: u8 = 1;
pub const WEAPON_DAMAGED: u8 = 2;
pub const ARMOUR_BROKEN: u8 = 4;
pub const WEAPON_BROKEN: u8 = 8;
/// A critical hit knocked the target's weapon from its hands (0x4b5f40 flag 0x2000; dropped at
/// 0x4b4fdf → 0x461a90, "Weapon dropped").
pub const WEAPON_DROPPED: u8 = 16;

/// One attack (0x4b3170 → 0x4c7160 → 0x4b6680 → 0x4b6860 → 0x4b4390):
/// - **To hit:** 1-100 plus the difficulty at or under the weapon skill's chance; the difficulty
///   is −(the weapon's to-hit) plus 5 a point of Strength short of the weapon's. A hit is critical
///   when a second roll is at or under half the chance; a miss fumbles when one is at or under
///   `max((100 − chance) / 2, 2)` (as Pick Locks').
/// - **Dodge:** a hit is dodged when the defender's Dodge chance (2 a level) makes its roll.
/// - **Damage:** each type rolled between its bounds (the weapon's plus its magick part; unarmed,
///   the attacker's own); melee adds the damage bonus to normal and fatigue damage; each type
///   less its resistance in percent (fatigue three quarters of it). A critical multiplies the
///   hit points: × 3 one time in ten, × 2 two in ten, else × 1.5 (0x4b5f40 with no chart
///   bonus).
pub fn resolve(attacker: &Fighter, defender: &Fighter, rng: &mut HostRng) -> AttackOutcome {
    let skill = attacker.skill();
    let chance = chance_against(skill, attacker.level(skill), attacker.intelligence, defender.warded);
    let weapon = attacker.weapon.clone().unwrap_or_default();
    let short = (weapon.min_strength - attacker.strength).max(0);
    // The defender's armour: half its class, in the attacker's chance (0x4c875c-0x4c878b).
    // An unnoticed stab by one trained in Backstab skips it (0x4c8743-0x4c875a).
    let armour = if attacker.stab == Stab::Unnoticed && attacker.backstab.1 >= 1 { 0 } else { defender.armour_class.max(0) / 2 * chance / 100 };
    let difficulty = -weapon.to_hit + 5 * short + injury_difficulty(attacker.injuries) + armour - defender.helpless + attacker.situation;
    let mut out = AttackOutcome { multiplier: 100, ..Default::default() };
    let missed = match attacker.fate {
        Fate::CriticalHit => false,
        Fate::CriticalMiss => true,
        Fate::None => rng.range(1, 100) + difficulty > chance,
    };
    if missed {
        if attacker.fate == Fate::CriticalMiss {
            out.fumbled = true;
            if rng.range(1, 100) <= 50 {
                fumble(attacker, &weapon, &mut out, rng);
            }
            return out;
        }
        out.fumbled = rng.range(1, 100) <= fumble_chance(chance, attacker.weapon.as_ref());
        if out.fumbled && rng.range(1, 100) <= 50 {
            fumble(attacker, &weapon, &mut out, rng);
        }
        return out;
    }
    // A combat skill's critical chance is a twentieth of it (0x4c81a0: skill flag 0x400), plus
    // the weapon's magick part (unscaled: 0x461590 not read).
    out.critical = rng.range(1, 100) <= chance / 20 + weapon.crit_chance + stab_critical(attacker.stab, attacker.backstab, defender.level);
    if attacker.fate == Fate::CriticalHit {
        out.critical = true;
    }
    let dodge = weapon_chance(SKILL_DODGE, defender.level(SKILL_DODGE), defender.intelligence);
    if attacker.fate != Fate::CriticalHit && dodge > 0 && rng.range(1, 100) <= dodge {
        out.dodged = true;
        return out;
    }
    out.hit = true;
    let melee = weapon.range <= 1;
    let mut dealt = [0; DAMAGE_TYPES];
    for (i, dealt) in dealt.iter_mut().enumerate() {
        let (mut lo, mut hi) = match &attacker.weapon {
            Some(w) => (w.damage[i].0 + w.magic_damage[i], w.damage[i].1 + w.magic_damage[i]),
            None => attacker.unarmed[i],
        };
        if melee && (i == 0 || i == 4) && hi > 0 {
            lo = (lo + attacker.damage_bonus).max(1);
            hi = (hi + attacker.damage_bonus).max(1);
        }
        if hi <= 0 {
            continue;
        }
        let rolled = rng.range(lo.max(0).min(hi), hi) + if i == 0 { stab_damage(attacker.stab, attacker.backstab.0) } else { 0 };
        let r = defender.resistances[RESISTANCE_OF[i]];
        let r = if i == 4 { r * 3 / 4 } else { r };
        *dealt = if r > 0 { rolled - rolled * r.min(100) / 100 } else { rolled };
    }
    if out.critical {
        out.multiplier = critical_multiplier(attacker, defender, &weapon, rng);
        let part = BodyPart::of_roll(rng.range(1, 100));
        out.part = part.number();
        out.effects = critical_effects(attacker, defender, &weapon, part, rng);
        out.injured = critical_injuries(attacker, defender, &weapon, part, rng);
        // Item damage (0x4b5f40 flags 0x100000 / 0x4000): each on a roll up to the base + 10;
        // the host finds the items ([`crate::repair::wear_item`]).
        let x = critical_base(attacker, &weapon);
        if rng.range(1, 100) <= x + 10 {
            out.items |= ARMOUR_DAMAGED;
        }
        if rng.range(1, 100) <= x + 10 {
            out.items |= WEAPON_DAMAGED;
        }
        // Knocked from its hands (0x4b6389, the same threshold), never an NPC's hit on a PC.
        if defender.weapon.is_some() && !(defender.pc && !attacker.pc) && rng.range(1, 100) <= x + 10 {
            out.items |= WEAPON_DROPPED;
        }
    }
    shield_damage(&mut dealt, defender.flags, defender.no_damage);
    // Any hit with fire damage (0x4b454f) or from a weapon of flag 0x100 (0x4b4592) wears the
    // armour where it lands (0x4b4d5e → 0x4b54b0), by 10 as a critical's.
    if dealt[3] > 0 || weapon.wears_armour {
        out.items |= ARMOUR_DAMAGED;
    }
    out.hp = (dealt[0] + dealt[2] + dealt[3]) * out.multiplier / 100;
    out.poison = dealt[1];
    out.fatigue = dealt[4];
    out
}

/// A weapon item's fighting stats (fields 0x72-0x7d) and its skill (0x465d80): thrown with
/// `weapon_flags` 0x40; firearms when tech, of range 3 or more, firing bullets, charges or fuel
/// (ammunition 1-3); a bow when it shoots arrows (0); else melee.
/// The chance in percent that a miss is critical (0x4c82e0): `(100 - chance) / 7` for the combat
/// skills (flag 0x400; checked at 0x4c8309), + the weapon's wear the same way, + its
/// `weapon_magic_crit_miss_chance`; at least 2.
pub fn fumble_chance(chance: i32, weapon: Option<&Weapon>) -> i32 {
    let mut c = (100 - chance) / 7;
    if let Some(w) = weapon {
        c += w.worn / 7 + w.miss_chance;
    }
    c.max(2)
}

/// The critical miss charts whose weapons can blow up (0x4b650f).
pub const EXPLODING_CHARTS: std::ops::RangeInclusive<i32> = 6..=8;

/// What half the critical misses do to the attacker (0x4b6410), `Y` its weapon's critical miss
/// effect: the weapon destroyed (chart not 5, a roll up to `Y + 1`); a shooter's ammunition, all
/// of it (up to `Y + 1`) or five shots' worth (else up to `Y + 11`); then a critical of its own on
/// itself (0x4b5f40: stun, knock down, injuries).
fn fumble(attacker: &Fighter, weapon: &Weapon, out: &mut AttackOutcome, rng: &mut HostRng) {
    let armed = attacker.weapon.is_some();
    let y = if armed { weapon.miss_effect } else { 0 };
    let chart = if armed { weapon.miss_chart } else { 5 };
    if chart != 5 && rng.range(1, 100) <= y + 1 {
        out.effects |= WEAPON_DESTROYED;
    }
    // Charts 6-8 (the grenade launcher, flame throwers, the Inferno Hammer, tesla weapons) blow
    // up on another roll up to `Y + 1` (0x4b650f-0x4b6531, flag 0x10000): destroyed too
    // (0x4b4f28 adds 0x8000), with effect 0x37 on the attacker (0x44c820, not built).
    if EXPLODING_CHARTS.contains(&chart) && rng.range(1, 100) <= y + 1 {
        out.effects |= WEAPON_DESTROYED;
    }
    if armed && weapon.uses_ammo() {
        if rng.range(1, 100) <= y + 1 {
            out.effects |= AMMO_LOST_ALL;
        } else if rng.range(1, 100) <= y + 11 {
            out.effects |= AMMO_LOST;
        }
    }
    let part = BodyPart::of_roll(rng.range(1, 100));
    out.part = part.number();
    out.fumble_effects = critical_effects(attacker, attacker, weapon, part, rng);
    out.fumble_injured = critical_injuries(attacker, attacker, weapon, part, rng);
    // Its critical on itself rolls the item effects too (0x4b636e; nothing skips them for a
    // critical miss): its weapon worn and knocked away (gone if destroyed above: 0x8000 is
    // handled first), the armour where it struck worn; each up to the base + 10.
    let x = critical_base(attacker, weapon);
    let kept = armed && out.effects & WEAPON_DESTROYED == 0;
    if kept && rng.range(1, 100) <= x + 10 {
        out.fumble_items |= WEAPON_DAMAGED;
    }
    if kept && rng.range(1, 100) <= x + 10 {
        out.fumble_items |= WEAPON_DROPPED;
    }
    if rng.range(1, 100) <= x + 10 {
        out.fumble_items |= ARMOUR_DAMAGED;
    }
}

/// A critical hit's stun and knock down (0x4b5f40), `E` as for injuries
/// ([`critical_injuries`]): stunned on a roll up to `E` and a failed Constitution save at − 5,
/// unless stunned already or knocked down; knocked down (critter chart 0 only, not the undead)
/// on a roll up to the base + 5, + 10 with weapon chart 1, + 10 on the legs. Knocking out
/// (0x100) is not built.
pub fn critical_effects(attacker: &Fighter, defender: &Fighter, weapon: &Weapon, part: BodyPart, rng: &mut HostRng) -> u8 {
    let x = critical_base(attacker, weapon);
    let chart = if attacker.weapon.is_none() || weapon.range > 1 { 1 } else { weapon.crit_chart };
    let mut e = x + 5;
    if matches!(chart, 1 | 3) {
        e += 10;
    }
    if part == BodyPart::Head && !defender.helmet {
        e += 10;
    }
    let mut effects = 0;
    let stun = rng.range(1, 100) <= e && rng.range(1, 100) > defender.constitution - 5;
    if !defender.undead && defender.crit_chart == 0 && !defender.knocked_down {
        let k = x + 5 + if chart == 1 { 10 } else { 0 } + if part == BodyPart::Legs { 10 } else { 0 };
        if rng.range(1, 100) <= k {
            effects |= KNOCKED_DOWN;
        }
    }
    if stun && !defender.stunned && !defender.knocked_down && effects & KNOCKED_DOWN == 0 {
        effects |= STUNNED;
        // Knocked out on top (0x100): an NPC, not of flag 0x4000000, another roll and save.
        if !defender.pc && !defender.undead && defender.flags & OCF_NO_KNOCK_OUT == 0 && rng.range(1, 100) <= e && rng.range(1, 100) > defender.constitution - 5 {
            effects |= KNOCKED_OUT;
        }
    }
    effects
}

/// The base of a critical hit's rolls (0x4b5f40): the weapon's critical effect, none for ranged
/// weapons.
fn critical_base(attacker: &Fighter, weapon: &Weapon) -> i32 {
    if attacker.weapon.is_some() && weapon.range <= 1 { weapon.crit_effect } else { 0 }
}

/// The multiplier of a critical hit (0x4b5f40): × 3 on a roll up to the base + 10, else × 2 on a
/// second up to + 30, else × 1.5; an NPC's on a PC × 1.5.
pub fn critical_multiplier(attacker: &Fighter, defender: &Fighter, weapon: &Weapon, rng: &mut HostRng) -> i32 {
    if !attacker.pc && defender.pc {
        return 150;
    }
    let x = critical_base(attacker, weapon);
    if rng.range(1, 100) <= x + 10 {
        300
    } else if rng.range(1, 100) <= x + 30 {
        200
    } else {
        150
    }
}

/// The injuries a critical hit gives (0x4b5f40), none to the undead: by the body part `part`
/// ([`BodyPart::of_roll`]), a roll up to `E` (the base + 5, + 10 more for critical chart 1 or 3
/// (unarmed and ranged count as 1), + 10 on a bare head) and a failed Constitution save (a roll
/// above CO − 5, CO for the head): the head blinds (not with chart 1, nor an NPC a PC), an arm is
/// crippled (both if one was, half the time), the legs are crippled. The original's rolls for
/// the three injuries differ a little (builder's call: `E` for all).
pub fn critical_injuries(attacker: &Fighter, defender: &Fighter, weapon: &Weapon, part: BodyPart, rng: &mut HostRng) -> i32 {
    if defender.undead {
        return 0;
    }
    let chart = if attacker.weapon.is_none() || weapon.range > 1 { 1 } else { weapon.crit_chart };
    let mut e = critical_base(attacker, weapon) + 5;
    if matches!(chart, 1 | 3) {
        e += 10;
    }
    if part == BodyPart::Head && !defender.helmet {
        e += 10;
    }
    let mut fails = |modifier: i32| rng.range(1, 100) <= e && rng.range(1, 100) > defender.constitution + modifier;
    let had = defender.injuries;
    let got = match part {
        BodyPart::Torso => 0,
        BodyPart::Head if chart == 1 || (!attacker.pc && defender.pc) => 0,
        BodyPart::Head => if fails(0) { OCF_BLINDED } else { 0 },
        BodyPart::Arms if fails(-5) => {
            if had & OCF_CRIPPLED_ARM != 0 && rng.range(1, 100) <= 50 { OCF_CRIPPLED_ARMS } else { OCF_CRIPPLED_ARM }
        }
        BodyPart::Arms => 0,
        BodyPart::Legs => if fails(-5) { OCF_CRIPPLED_LEGS } else { 0 },
    };
    got & !had
}

pub fn weapon_of(item: &Object, prototypes: &Prototypes) -> Weapon {
    let int = |f: &str| prototypes.int(item, f).unwrap_or(0);
    let at = |f: &str, i: usize| item.ints(f).or_else(|| prototypes.of(item)?.ints(f)).and_then(|v| v.get(i).copied()).unwrap_or(0);
    let range = int("weapon_range");
    let ammo = int("weapon_ammo_type");
    let skill = if int("weapon_flags") & WEAPON_THROWN != 0 {
        SKILL_THROWING
    } else if int("item_magic_tech_complexity") < 0 && range >= 3 && (1..=3).contains(&ammo) {
        SKILL_FIREARMS
    } else if ammo == 0 {
        SKILL_BOW
    } else {
        SKILL_MELEE
    };
    let mut damage = [(0, 0); DAMAGE_TYPES];
    let mut magic = [0; DAMAGE_TYPES];
    for i in 0..DAMAGE_TYPES {
        damage[i] = (at("weapon_damage_lower_idx", i), at("weapon_damage_upper_idx", i));
        magic[i] = at("weapon_magic_damage_adj_idx", i);
    }
    Weapon {
        skill,
        to_hit: int("weapon_bonus_to_hit") + int("weapon_magic_hit_adj"),
        damage,
        magic_damage: magic,
        speed: int("weapon_speed_factor") + int("weapon_magic_speed_adj"),
        range: if range == 1 { 0 } else { range + int("weapon_magic_range_adj") },
        min_strength: int("weapon_min_strength") + int("weapon_magic_min_strength_adj"),
        crit_chance: int("weapon_magic_crit_hit_chance"),
        crit_effect: int("weapon_magic_crit_hit_effect"),
        crit_chart: int("weapon_crit_hit_chart"),
        ammo_type: prototypes.int(item, "weapon_ammo_type").unwrap_or(NO_AMMO),
        ammo_per_shot: int("weapon_ammo_consumption"),
        miss_chart: int("weapon_crit_miss_chart"),
        miss_chance: int("weapon_magic_crit_miss_chance"),
        miss_effect: int("weapon_magic_crit_miss_effect"),
        worn: 0,
        wears_armour: int("weapon_flags") & WEAPON_WEARS_ARMOUR != 0,
        use_kind: {
            let aid = arcanum_formats::art_id::ArtId(int("item_use_aid_fragment") as u32);
            if aid.art_type() == 6 { (aid.0 >> 6) & 0xf } else { 0 }
        },
    }
}

/// `weapon_flags`: a throwing weapon (0x465d80).
pub const WEAPON_THROWN: i32 = 0x40;
/// `weapon_flags`: its hits wear the armour they land on (0x4b4585, tested at 0x4b458d).
pub const WEAPON_WEARS_ARMOUR: i32 = 0x100;
/// A bare hand's speed (0x465cf0 with no weapon).
pub const UNARMED_SPEED: i32 = 10;

/// An unarmed PC's damage (0x465f70 with no weapon): normal and fatigue `max(g − 25, 1)` to
/// `g + 5`, `g` the gloves' `armor_unarmed_bonus_damage`.
pub fn pc_unarmed(gloves: i32) -> [(i32, i32); DAMAGE_TYPES] {
    let hi = gloves + 5;
    let lo = (hi - 30).max(1);
    [(lo, hi), (0, 0), (0, 0), (0, 0), (lo, hi)]
}

/// A critter's own damage (`npc_damage_idx`, pairs by type).
pub fn npc_unarmed(object: &Object, prototypes: &Prototypes) -> [(i32, i32); DAMAGE_TYPES] {
    let list = object.ints("npc_damage_idx").or_else(|| prototypes.of(object)?.ints("npc_damage_idx")).unwrap_or_default();
    let mut out = [(0, 0); DAMAGE_TYPES];
    for (i, o) in out.iter_mut().enumerate() {
        *o = (list.get(2 * i).copied().unwrap_or(0), list.get(2 * i + 1).copied().unwrap_or(0));
    }
    if out.iter().all(|d| d.1 <= 0) { pc_unarmed(0) } else { out }
}

/// Resistances from worn armour: each `armor_resistance_adj_idx` plus its magick part, summed,
/// at most 95 (builder's call: 0x43d6d0 was not read).
pub fn armour_resistances(worn: &[&Object], prototypes: &Prototypes) -> [i32; 5] {
    let mut out = [0; 5];
    for item in worn {
        let at = |f: &str, i: usize| item.ints(f).or_else(|| prototypes.of(item)?.ints(f)).and_then(|v| v.get(i).copied()).unwrap_or(0);
        for (i, r) in out.iter_mut().enumerate() {
            *r += at("armor_resistance_adj_idx", i) + at("armor_magic_resistance_adj_idx", i);
        }
    }
    out.map(|r| r.min(MAX_CHANCE))
}

/// The armour class worn items give (0x465ba0 over slots 1000-1008: `armor_ac_adj` field 0x98
/// and its magick part 0x99, unscaled: 0x461590 not read).
pub fn worn_armour_class(worn: &[&Object], prototypes: &Prototypes) -> i32 {
    worn.iter().map(|o| prototypes.int(o, "armor_ac_adj").unwrap_or(0) + prototypes.int(o, "armor_magic_ac_adj").unwrap_or(0)).sum()
}

/// The highest armour class (0x43d923: 0x5f).
pub const MAX_ARMOUR_CLASS: i32 = 95;

/// The resistances of the armour worn in `inv` (as [`armour_resistances`]).
pub fn worn_resistances(inv: &crate::Inventory, prototypes: &Prototypes) -> [i32; 5] {
    let worn: Vec<&Object> = inv.items.iter().filter(|i| (crate::FIRST_SLOT..=crate::LAST_SLOT).contains(&i.location)).filter_map(|i| prototypes.get(i.prototype)).filter(|o| o.object_type == arcanum_formats::object::TYPE_ARMOR).collect();
    armour_resistances(&worn, prototypes)
}

/// An NPC's stat (its `critter_stat_base_idx`).
pub fn npc_stat(object: &Object, prototypes: &Prototypes, stat: usize) -> i32 {
    object.ints("critter_stat_base_idx").or_else(|| prototypes.of(object)?.ints("critter_stat_base_idx")).and_then(|s| s.get(stat).copied()).unwrap_or(0)
}

/// An NPC's skill level: points × 4, at most 20.
/// NPC skill `k`'s level and training rank (bits 6-7, as dialogue reads them).
pub fn npc_skill(object: &Object, prototypes: &Prototypes, k: usize) -> (i32, u8) {
    let (field, i) = if k < 12 { ("critter_basic_skill_idx", k) } else { ("critter_tech_skill_idx", k - 12) };
    let raw = object.ints(field).or_else(|| prototypes.of(object)?.ints(field)).and_then(|v| v.get(i).copied()).unwrap_or(0);
    (((raw & 0x3f) * arcanum_rules::LEVELS_PER_POINT).min(arcanum_rules::MAX_SKILL_LEVEL), ((raw >> 6) & 3) as u8)
}

pub fn npc_skill_level(object: &Object, prototypes: &Prototypes, k: usize) -> i32 {
    let (field, i) = if k < 12 { ("critter_basic_skill_idx", k) } else { ("critter_tech_skill_idx", k - 12) };
    let raw = object.ints(field).or_else(|| prototypes.of(object)?.ints(field)).and_then(|v| v.get(i).copied()).unwrap_or(0);
    ((raw & 0x3f) * arcanum_rules::LEVELS_PER_POINT).min(arcanum_rules::MAX_SKILL_LEVEL)
}

/// An NPC's most hit points (0x43d5a0): `4 × hp_pts + hp_adj + WP + 2 × (level + ST) + 4`.
pub fn npc_max_hp(object: &Object, prototypes: &Prototypes) -> i32 {
    let stat = |i| npc_stat(object, prototypes, i);
    4 * prototypes.int(object, "hp_pts").unwrap_or(0) + prototypes.int(object, "hp_adj").unwrap_or(0) + stat(6) + 2 * (stat(17) + stat(0)) + 4
}

/// An NPC's speed (stat 11 from Dexterity, as a PC's; 5 less with crippled legs, 0x4b052a).
pub fn npc_speed(object: &Object, prototypes: &Prototypes) -> i32 {
    let dx = npc_stat(object, prototypes, 1);
    let lame = prototypes.int(object, "critter_flags").unwrap_or(0) & OCF_CRIPPLED_LEGS != 0;
    dx + if dx >= 20 { 5 } else { 0 } - if lame { arcanum_rules::CRIPPLED_LEGS_SPEED } else { 0 }
}

/// An NPC's damage bonus (stat 9 from Strength, as a PC's).
pub fn damage_bonus(st: i32) -> i32 {
    let d = st - 10;
    let d = if d < 0 { d / 2 } else { d };
    if st >= 20 { d * 2 } else { d }
}

/// The share of an NPC's experience a hit earns (0x4b4390): its percent of the most hit points
/// of `worth`, at most what is left of the pool.
pub fn hit_experience(damage: i32, max_hp: i32, worth: i32, pool: i32) -> i32 {
    if max_hp <= 0 || pool <= 0 {
        return 0;
    }
    (damage.max(0) * 100 / max_hp * worth / 100).min(pool)
}

/// An NPC fighting someone (0x118 bit 1, its target in field 0x11b).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fighting {
    pub target: NetId,
}

/// The running fight, if any.
#[derive(Resource, Debug, Default)]
pub struct Combat(pub Option<Encounter>);

/// Who fights, in turn order, whose turn it is and its points left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encounter {
    pub order: Vec<NetId>,
    pub turn: usize,
    pub ap: i32,
    pub round: u32,
    /// The turn has just started (an NPC decides whether to flee).
    pub fresh: bool,
}

impl Encounter {
    pub fn current(&self) -> Option<NetId> {
        self.order.get(self.turn).copied()
    }

    /// What clients see of it.
    pub fn delta(&self) -> LedgerDelta {
        LedgerDelta::Combat { order: self.order.clone(), turn: self.current(), ap: self.ap }
    }
}

/// Attack and end-turn requests taken from the inbox.
#[derive(Resource, Debug, Default)]
pub struct CombatRequests {
    pub attacks: Vec<(PlayerId, NetId, u32)>,
    pub end_turn: Vec<PlayerId>,
}

/// Fights started elsewhere (dialogue `co`, script action 0x24): the NPC and whom it attacks.
#[derive(Resource, Debug, Default)]
pub struct CombatStarts(pub Vec<(NetId, NetId)>);

/// Script events the fight raises (0x4aabe0, 0x45da20, 0x4b3770 …): the attachee, the event, the
/// player whose PC is involved, the triggerer. An item a PC carries runs its prototype's script.
/// Their answers are ignored, as the original's. Second, the carried items' events (weapons'
/// hits and misses, items' busts); third, carried items to destroy.
#[derive(Resource, Debug, Default)]
pub struct CombatEvents(pub Vec<(crate::ScriptObject, u32, PlayerId, crate::ScriptObject)>, pub Vec<WeaponEvent>, pub Vec<ItemDestroy>);

/// A carried item the fight destroys (0x43cca0: a weapon a critical miss blew up, 0x4b4f48; a
/// broken item worn to nothing, 0x43f44a): its destroy script (2) runs first, the item its
/// triggerer and attachee (0x43cd0e), and a 0 answer keeps it (0x43cd18). Without a player to
/// run it for, it is destroyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemDestroy {
    pub item: crate::ScriptObject,
    pub prototype: u32,
    pub location: i32,
    pub player: Option<PlayerId>,
}

/// Object `flags` 0x400000 on a weapon: it is never worn, destroyed or blown up (0x4b4e81);
/// armour has no such test. No prototype has it.
pub const OF_WEAR_IMMUNE: i32 = 0x40_0000;

/// Whether weapon `o` is of object `flags` [`OF_WEAR_IMMUNE`].
pub fn immune_to_wear(o: &Object) -> bool {
    o.int("flags").unwrap_or(0) & OF_WEAR_IMMUNE != 0
}

/// Item `event` 30: a carried item broke (0x43f1c0 → 0x441980; the attacker the triggerer).
pub const EVENT_BUST: u32 = 30;

/// A carried item's script event: a weapon's hit or miss (0x4b3770, 0x4b2f60: the weapon the
/// attachee, the attacker the triggerer, the victim the extra), or an item's bust (no extra);
/// for `player`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeaponEvent {
    pub item: crate::ScriptObject,
    pub prototype: u32,
    pub event: u32,
    pub player: PlayerId,
    pub attacker: crate::ScriptObject,
    pub victim: Option<crate::ScriptObject>,
}

/// Script events of attacks: hit and miss (on the weapon, the attacker the triggerer; 0x4b37a3 /
/// 0x4b3972), critical hit and miss after them (0x4b37cd / 0x4b399c), taking damage (on the
/// victim, the attacker the triggerer, 0x4b446b: a 0 answer cancels the damage), critter hits
/// (on the attacker, the victim the triggerer, 0x4b37fc), will kill on sight (on the NPC, its
/// target the triggerer, 0x4ae179: a 0 answer leaves it peaceful), leader killing (on each
/// follower of the killer, the dead the triggerer, 0x45db95).
pub const EVENT_HIT: u32 = 7;
pub const EVENT_MISS: u32 = 8;
pub const EVENT_LEADER_KILLING: u32 = 20;
pub const EVENT_KILL_ON_SIGHT: u32 = 22;
pub const EVENT_TAKING_DAMAGE: u32 = 23;
pub const EVENT_CRITTER_HITS: u32 = 26;
pub const EVENT_CRITICAL_HIT: u32 = 34;
pub const EVENT_CRITICAL_MISS: u32 = 35;

/// A hit on an NPC whose taking-damage script is asked first (0x4b446b): the damage waits for its
/// answer, a tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldHit {
    pub attacker_object: crate::ScriptObject,
    /// The attacking PC (its team gets the experience).
    pub attacker_player: Option<PlayerId>,
    /// The player the script runs for.
    pub player: Option<PlayerId>,
    pub victim: NetId,
    pub victim_object: crate::ScriptObject,
    pub outcome: AttackOutcome,
}

/// Script answers the fight waits on: held hits (answered: whether the damage goes ahead), and
/// NPCs about to turn on a PC (`(npc, pc, npc object, player)`; event 22).
#[derive(Resource, Debug, Default)]
pub struct ScriptChecks {
    pub hits: Vec<HeldHit>,
    pub answered: Vec<(HeldHit, bool)>,
    pub hostile: Vec<(NetId, NetId, crate::ScriptObject, PlayerId)>,
    /// Lock witnesses with a "catching thief" script to ask (witness, PC, its object, player,
    /// lock opened), and those whose script let them react ([`catch_lock_pickers`]).
    pub witnesses: Vec<(NetId, NetId, crate::ScriptObject, PlayerId, bool)>,
    pub witnessed: Vec<(NetId, NetId, PlayerId, bool)>,
}

/// Script events: an NPC dying (killer the triggerer), starting and stopping a fight.
pub const EVENT_DYING: u32 = 12;
pub const EVENT_ENTER_COMBAT: u32 = 13;
pub const EVENT_EXIT_COMBAT: u32 = 14;
/// Start / end combat (0x4a8570): each think of a leaderless NPC in a fight.
pub const EVENT_START_COMBAT: u32 = 15;
pub const EVENT_END_COMBAT: u32 = 16;

/// Runs the fight's script events with the host.
/// Runs the fight's script events with the host; asks the held hits' and would-be foes' scripts.
/// Takes a destroyed item from its holder's inventory, if it is still where it was.
fn destroy_carried(world: &mut crate::WorldState, map: &mut crate::LoadedMap, inventories: &mut crate::script_host::ScriptInventories, d: ItemDestroy) {
    let gone = |inv: &mut crate::Inventory| match inv.items.iter().position(|i| i.location == d.location && i.prototype == d.prototype) {
        Some(i) => {
            inv.items.remove(i);
            true
        }
        None => false,
    };
    match d.item {
        crate::ScriptObject::Carried { holder, .. } => {
            if world.0.as_mut().and_then(|w| w.players.get_mut(&holder)).and_then(|s| s.inventory.as_mut()).is_some_and(gone) {
                inventories.0.insert(holder);
            }
        }
        crate::ScriptObject::NpcCarried { sector, index, .. } => {
            if let Some(mut inv) = map.npc_inventory(sector, index)
                && gone(&mut inv)
            {
                map.set_npc_inventory(sector, index, inv);
                crate::ground::save_change(world, map, sector);
            }
        }
        _ => {}
    }
}

pub fn run_combat_events(mut events: ResMut<CombatEvents>, mut checks: ResMut<ScriptChecks>, mut starts: ResMut<CombatStarts>, mut host: crate::ScriptHost, mut world: ResMut<crate::WorldState>, mut map: ResMut<crate::LoadedMap>, mut outbox: ResMut<crate::Outbox>) {
    for (object, event, player, triggerer) in std::mem::take(&mut events.0) {
        host.hook_by(&mut world, &mut map, &mut outbox, object, event, player, triggerer);
    }
    for e in std::mem::take(&mut events.1) {
        host.carried_hook(&mut world, &mut map, &mut outbox, e.item, e.prototype, e.event, e.player, e.attacker, e.victim);
    }
    for d in std::mem::take(&mut events.2) {
        let kept = d.player.is_some_and(|player| host.carried_hook(&mut world, &mut map, &mut outbox, d.item, d.prototype, crate::EVENT_DESTROY, player, d.item, None) == crate::HookResult::SkipDefault);
        if !kept {
            destroy_carried(&mut world, &mut map, host.changed_inventories(), d);
        }
    }
    for hit in std::mem::take(&mut checks.hits) {
        let allowed = match hit.player {
            Some(player) => host.hook_by(&mut world, &mut map, &mut outbox, hit.victim_object, EVENT_TAKING_DAMAGE, player, hit.attacker_object) == crate::HookResult::RunDefault,
            None => true,
        };
        checks.answered.push((hit, allowed));
    }
    for (npc, pc, object, player) in std::mem::take(&mut checks.hostile) {
        if host.hook(&mut world, &mut map, &mut outbox, object, EVENT_KILL_ON_SIGHT, player) == crate::HookResult::RunDefault {
            starts.0.push((npc, pc));
        }
    }
    for (npc, pc, object, player, opened) in std::mem::take(&mut checks.witnesses) {
        if host.hook(&mut world, &mut map, &mut outbox, object, crate::pickpocket::EVENT_CATCHING_THIEF, player) == crate::HookResult::RunDefault {
            checks.witnessed.push((npc, pc, player, opened));
        }
    }
}

/// A PC's attack waiting for its walk to end next to the target.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingAttack {
    pub target: NetId,
}

/// The larger axis difference between two tiles (0x4b96f0).
pub fn reach_between(a: TileLoc, b: TileLoc) -> u32 {
    a.x.abs_diff(b.x).max(a.y.abs_diff(b.y))
}

impl crate::LoadedMap {
    /// NPC `index`'s hit points: what is left, and the most.
    pub fn npc_hp(&self, sector: arcanum_formats::map::SectorId, index: u32) -> Option<(i32, i32)> {
        let o = self.mobiles.get(&sector)?.get(index as usize)?;
        let max = npc_max_hp(o, &self.prototypes);
        Some((max - self.prototypes.int(o, "hp_damage").unwrap_or(0), max))
    }

    /// Adds `amount` to NPC `index`'s `hp_damage` (a negative amount heals, to 0), kept in
    /// [`crate::LoadedMap::changes`].
    pub fn hurt_npc(&mut self, sector: arcanum_formats::map::SectorId, index: u32, amount: i32) {
        let Some(o) = self.mobiles.get_mut(&sector).and_then(|m| m.get_mut(index as usize)) else { return };
        let now = (o.int("hp_damage").unwrap_or(0) + amount).max(0);
        o.set_int("hp_damage", now);
        self.changes.entry(sector).or_default().npc_damage.insert(index, now);
    }

    /// Adds injuries to NPC `index`'s `critter_flags`, kept the same way.
    pub fn injure_npc(&mut self, sector: arcanum_formats::map::SectorId, index: u32, injuries: i32) {
        let Some(o) = self.mobiles.get_mut(&sector).and_then(|m| m.get_mut(index as usize)) else { return };
        let flags = self.prototypes.int(o, "critter_flags").unwrap_or(0) | injuries;
        o.set_int("critter_flags", flags);
        self.changes.entry(sector).or_default().npc_critter_flags.insert(index, flags);
    }

    /// NPC `index`'s fatigue: what is left, and the most (0x45d670: `critter_fatigue_pts × 4 +
    /// critter_fatigue_adj + 4 + (level + CO) × 2 + WP`).
    pub fn npc_fatigue(&self, sector: arcanum_formats::map::SectorId, index: u32) -> Option<(i32, i32)> {
        let o = self.mobiles.get(&sector)?.get(index as usize)?;
        let int = |f: &str| self.prototypes.int(o, f).unwrap_or(0);
        let max = int("critter_fatigue_pts") * 4 + int("critter_fatigue_adj") + 4 + (npc_stat(o, &self.prototypes, STAT_LEVEL) + npc_stat(o, &self.prototypes, 2)) * 2 + npc_stat(o, &self.prototypes, 6);
        Some((max - int("critter_fatigue_damage"), max))
    }

    /// Whether NPC `index` is out of the fight: dead, or knocked out (fatigue below 1, 0x45d800;
    /// never the undead, `critter_flags` 0x4 at 0x45d847).
    pub fn npc_down(&self, sector: arcanum_formats::map::SectorId, index: u32) -> bool {
        let undead = self.mobiles.get(&sector).and_then(|m| m.get(index as usize)).is_some_and(|o| self.prototypes.int(o, "critter_flags").unwrap_or(0) & OCF_UNDEAD != 0);
        self.is_dead(sector, index) || (!undead && self.npc_fatigue(sector, index).is_some_and(|(left, _)| left < 1))
    }

    /// Sets NPC `index`'s fatigue damage, kept in [`crate::LoadedMap::changes`].
    /// NPC `index`'s poison level (stat 0x18): as kept in [`crate::SectorDelta::npc_poison`],
    /// else its own.
    pub fn npc_poison(&self, sector: arcanum_formats::map::SectorId, index: u32) -> i32 {
        if let Some(&p) = self.changes.get(&sector).and_then(|c| c.npc_poison.get(&index)) {
            return p;
        }
        self.mobiles.get(&sector).and_then(|m| m.get(index as usize)).map_or(0, |o| npc_stat(o, &self.prototypes, crate::poison::STAT_POISON))
    }

    /// Sets NPC `index`'s poison level, at least 0.
    pub fn set_npc_poison(&mut self, sector: arcanum_formats::map::SectorId, index: u32, level: i32) {
        self.changes.entry(sector).or_default().npc_poison.insert(index, level.max(0));
    }

    pub fn set_npc_fatigue_damage(&mut self, sector: arcanum_formats::map::SectorId, index: u32, damage: i32) {
        let Some(o) = self.mobiles.get_mut(&sector).and_then(|m| m.get_mut(index as usize)) else { return };
        o.set_int("critter_fatigue_damage", damage.max(0));
        self.changes.entry(sector).or_default().npc_fatigue.insert(index, damage.max(0));
    }

    /// Lays NPC `index` down on the last frame of animation 7 (knocked down, 0x435a90), or stands
    /// it up again (animation 0, frame 0).
    pub fn lay_npc_down(&mut self, vfs: Option<&arcanum_vfs::Vfs>, sector: arcanum_formats::map::SectorId, index: u32, down: bool) {
        let Some(art) = self.object_at(sector, index, true).and_then(|o| self.prototypes.art_id(o)) else { return };
        let art = if down { crate::death_frame(vfs, self.art.as_deref(), art).0 } else { art.0 & !(0x1f << 6 | 0x1f << 14) };
        self.set_object_art(sector, index, true, art);
    }

    /// Mends NPC `index`'s injuries (`critter_flags` 0x80-0x400).
    pub fn cure_npc_injuries(&mut self, sector: arcanum_formats::map::SectorId, index: u32) {
        let Some(o) = self.mobiles.get_mut(&sector).and_then(|m| m.get_mut(index as usize)) else { return };
        let flags = self.prototypes.int(o, "critter_flags").unwrap_or(0) & !OCF_INJURIES;
        o.set_int("critter_flags", flags);
        self.changes.entry(sector).or_default().npc_critter_flags.insert(index, flags);
    }

    /// Raises dead NPC `index` with [`crate::resurrected_hp`] of its hit points, as a PC (owner's
    /// call: one rule for both; the list's `Full` would raise it whole), standing (animation 0,
    /// frame 0).
    pub fn revive_npc(&mut self, sector: arcanum_formats::map::SectorId, index: u32) {
        if let Some(changes) = self.changes.get_mut(&sector) {
            changes.dead.remove(&index);
        }
        let Some(max) = self.npc_hp(sector, index).map(|h| h.1) else { return };
        let damage = (max - crate::resurrected_hp(max)).max(0);
        if let Some(o) = self.mobiles.get_mut(&sector).and_then(|m| m.get_mut(index as usize)) {
            o.set_int("hp_damage", damage);
        }
        self.changes.entry(sector).or_default().npc_damage.insert(index, damage);
        // Its poison gone (owner's call; the original keeps it, 0x4a609a).
        self.set_npc_poison(sector, index, 0);
        if let Some(art) = self.object_at(sector, index, true).and_then(|o| self.prototypes.art_id(o)) {
            self.set_object_art(sector, index, true, art.0 & !(0x1f << 6 | 0x1f << 14));
        }
    }

    /// Takes `amount` from NPC `index`'s experience pool (field 0x11e), kept the same way.
    pub fn spend_npc_pool(&mut self, sector: arcanum_formats::map::SectorId, index: u32, amount: i32) {
        let Some(o) = self.mobiles.get_mut(&sector).and_then(|m| m.get_mut(index as usize)) else { return };
        let pool = self.prototypes.int(o, "npc_experience_pool").unwrap_or(0) - amount;
        o.set_int("npc_experience_pool", pool);
        self.changes.entry(sector).or_default().npc_pools.insert(index, pool);
    }

    /// Kills NPC `index` (action 0x3a's way): dead, in the last frame of death animation `anim`.
    pub fn kill_npc(&mut self, vfs: Option<&arcanum_vfs::Vfs>, sector: arcanum_formats::map::SectorId, index: u32, anim: u32) {
        if self.is_dead(sector, index) {
            return;
        }
        self.changes.entry(sector).or_default().dead.insert(index);
        if let Some(art) = self.object_at(sector, index, true).and_then(|o| self.prototypes.art_id(o)) {
            let dying = crate::death_frame_in(vfs, self.art.as_deref(), art, anim);
            self.set_object_art(sector, index, true, dying.0);
        }
        // Out of its leader's list (0x45dbd5 → 0x45dfc0 → 0x45e040), its leader kept
        // (0x45dbef): a joining by force no longer stands (0x45e142 clears 0x400).
        let flags = self.npc_flags(sector, index).unwrap_or(0);
        if flags & crate::follow::FORCED != 0 {
            if self.guests.contains_key(&(sector, index)) {
                if let Some(o) = self.mobiles.get_mut(&sector).and_then(|m| m.get_mut(index as usize)) {
                    o.set_int("npc_flags", flags & !crate::follow::FORCED);
                }
            } else {
                self.set_npc_flags(sector, index, flags & !crate::follow::FORCED);
            }
        }
    }
}

/// What the combat systems need of the world.
#[derive(bevy_ecs::system::SystemParam)]
pub struct CombatWorld<'w, 's> {
    pub combat: ResMut<'w, Combat>,
    pub requests: ResMut<'w, CombatRequests>,
    pub map: ResMut<'w, crate::LoadedMap>,
    pub world: ResMut<'w, crate::WorldState>,
    pub session: Res<'w, crate::Session>,
    pub outbox: ResMut<'w, crate::Outbox>,
    pub rules: Res<'w, crate::SimRules>,
    pub rng: ResMut<'w, HostRng>,
    pub awards: ResMut<'w, crate::ExperienceAwards>,
    pub toggled: ResMut<'w, crate::world::ToggledNpcs>,
    pub vfs: Option<Res<'w, crate::SimVfs>>,
    pub generic: ResMut<'w, crate::GenericLines>,
    pub sheets: ResMut<'w, crate::ScriptSheets>,
    pub ai: ResMut<'w, AiParams>,
    pub starts: ResMut<'w, CombatStarts>,
    pub events: ResMut<'w, CombatEvents>,
    pub checks: ResMut<'w, ScriptChecks>,
    pub reputations: ResMut<'w, crate::ReputationTable>,
    pub inventories: ResMut<'w, crate::script_host::ScriptInventories>,
    /// The time of day, for the light (0x4c8a06).
    pub clock: Res<'w, crate::GameClock>,
    /// NPCs made to fight this tick (their `Fighting` lands after the system).
    pub joined: Local<'s, Vec<NetId>>,
}

/// A critter as the combat systems see it.
type CombatantQuery<'a> = (Entity, &'a NetId, &'a crate::Position, Option<&'a crate::PlayerControlled>, Option<&'a crate::NpcSource>, Option<&'a crate::Sheet>, Option<&'a crate::Inventory>, Option<&'a Fighting>, Has<crate::Walking>, Option<&'a PendingAttack>, Option<&'a crate::follow::Following>, Option<&'a Surrendered>, Option<&'a Stunned>, (Option<&'a crate::Facing>, Option<&'a crate::Appearance>, Has<crate::Prowling>, Has<KnockedDown>));

/// One critter of the query, copied out.
#[derive(Clone)]
struct Seen {
    entity: Entity,
    net_id: NetId,
    at: TileLoc,
    player: Option<PlayerId>,
    npc: Option<crate::NpcSource>,
    sheet: Option<arcanum_rules::CharacterSheet>,
    inventory: Option<crate::Inventory>,
    fighting: bool,
    /// Whom it fights.
    target: Option<NetId>,
    walking: bool,
    pending: Option<PendingAttack>,
    /// The PC it follows.
    leader: Option<PlayerId>,
    /// Whom it gave up to.
    surrendered: Option<NetId>,
    stunned: Option<Stunned>,
    rotation: usize,
    /// A PC prowling ([`crate::Prowling`]).
    prowling: bool,
    /// Lying from a knock-down ([`KnockedDown`]).
    knocked_down: bool,
}

impl Seen {
    /// An NPC fighting for no PC.
    fn foe(&self) -> bool {
        self.npc.is_some() && self.leader.is_none()
    }
}

fn seen(q: &Query<CombatantQuery>) -> Vec<Seen> {
    q.iter()
        .map(|(entity, &net_id, at, pc, npc, sheet, inventory, fighting, walking, pending, following, surrendered, stunned, (facing, look, prowling, knocked_down))| Seen {
            prowling,
            knocked_down,
            rotation: facing_of(facing, look),
            stunned: stunned.copied(),
            surrendered: surrendered.map(|s| s.to),
            leader: following.map(|f| f.leader),
            target: fighting.map(|f| f.target),
            entity,
            net_id,
            at: at.0,
            player: pc.map(|p| p.0),
            npc: npc.copied(),
            sheet: sheet.map(|s| s.0.clone()),
            inventory: inventory.cloned(),
            fighting: fighting.is_some(),
            walking,
            pending: pending.copied(),
        })
        .collect()
}

impl CombatWorld<'_, '_> {
    /// A critter's fighting side.
    fn fighter(&self, c: &Seen) -> Fighter {
        let prototypes = &self.map.prototypes;
        let item = |inv: &crate::Inventory, slot: i32| inv.items.iter().find(|i| i.location == slot).and_then(|i| prototypes.get(i.prototype));
        if let (Some(sheet), Some(inv)) = (&c.sheet, &c.inventory) {
            let rules = &self.rules.0;
            let levels = [SKILL_BOW, SKILL_DODGE, SKILL_MELEE, SKILL_THROWING, SKILL_FIREARMS].map(|k| (k, rules.skill_level(sheet, k))).to_vec();
            let worn: Vec<&Object> = (crate::FIRST_SLOT..=crate::LAST_SLOT).filter_map(|s| item(inv, s)).filter(|o| o.object_type == arcanum_formats::object::TYPE_ARMOR).collect();
            let gloves = item(inv, GLOVES_SLOT).and_then(|g| prototypes.int(g, "armor_unarmed_bonus_damage")).unwrap_or(0);
            return Fighter {
                levels,
                strength: rules.stats(sheet)[0],
                damage_bonus: rules.derived(sheet).damage_bonus,
                weapon: item(inv, WEAPON_SLOT).filter(|w| w.object_type == arcanum_formats::object::TYPE_WEAPON).map(|w| weapon_of(w, prototypes)),
                unarmed: pc_unarmed(gloves),
                resistances: armour_resistances(&worn, prototypes),
                armour_class: (rules.derived(sheet).ac_adjustment + worn_armour_class(&worn, prototypes)).clamp(0, MAX_ARMOUR_CLASS),
                helpless: 0,
                situation: 0,
                trained: [SKILL_BOW, SKILL_MELEE, SKILL_THROWING, SKILL_FIREARMS].into_iter().filter(|&k| sheet.training[k] > 0).collect(),
                pc: c.player.is_some(),
                constitution: rules.stats(sheet)[2],
                intelligence: rules.stats(sheet)[4],
                backstab: (rules.skill_level(sheet, SKILL_BACKSTAB), sheet.training[SKILL_BACKSTAB]),
                level: sheet.level,
                stab: Stab::None,
                fate: Fate::None,
                injuries: sheet.injuries,
                helmet: item(inv, HELMET_SLOT).is_some(),
                undead: false,
                ammo: 0,
                crit_chart: 0,
                stunned: c.stunned.is_some(),
                knocked_down: c.knocked_down,
                flags: 0,
                no_damage: false,
                warded: false,
            }
            .with_ammo(inv, prototypes);
        }
        let Some(source) = c.npc else { return Fighter::default() };
        let Some(o) = self.map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)) else { return Fighter::default() };
        let inv = self.map.npc_inventory(source.sector, source.index).unwrap_or_default();
        let worn: Vec<&Object> = (crate::FIRST_SLOT..=crate::LAST_SLOT).filter_map(|s| item(&inv, s)).filter(|o| o.object_type == arcanum_formats::object::TYPE_ARMOR).collect();
        Fighter {
            levels: [SKILL_BOW, SKILL_DODGE, SKILL_MELEE, SKILL_THROWING, SKILL_FIREARMS].map(|k| (k, npc_skill_level(o, prototypes, k))).to_vec(),
            strength: npc_stat(o, prototypes, 0),
            damage_bonus: damage_bonus(npc_stat(o, prototypes, 0)),
            weapon: item(&inv, WEAPON_SLOT).filter(|w| w.object_type == arcanum_formats::object::TYPE_WEAPON).map(|w| weapon_of(w, prototypes)),
            unarmed: npc_unarmed(o, prototypes),
            resistances: armour_resistances(&worn, prototypes),
            // Stat 10 as a PC's (DX - 10, builder's call for NPCs).
            armour_class: (prototypes.int(o, "AC").unwrap_or(0) + npc_stat(o, prototypes, 1) - 10 + worn_armour_class(&worn, prototypes)).clamp(0, MAX_ARMOUR_CLASS),
            helpless: 0,
            situation: 0,
            trained: Vec::new(),
            pc: false,
            constitution: npc_stat(o, prototypes, 2),
            intelligence: npc_stat(o, prototypes, 4),
            backstab: npc_skill(o, prototypes, SKILL_BACKSTAB),
            level: npc_stat(o, prototypes, 0x11),
            stab: Stab::None,
            fate: Fate::None,
            injuries: prototypes.int(o, "critter_flags").unwrap_or(0) & OCF_INJURIES,
            helmet: item(&inv, HELMET_SLOT).is_some(),
            undead: prototypes.int(o, "critter_flags").unwrap_or(0) & OCF_UNDEAD != 0,
            ammo: 0,
            crit_chart: prototypes.int(o, "critter_crit_hit_chart").unwrap_or(0),
            stunned: c.stunned.is_some(),
            knocked_down: c.knocked_down,
            flags: prototypes.int(o, "critter_flags").unwrap_or(0),
            no_damage: prototypes.int(o, "spell_flags").unwrap_or(0) & SPELL_NO_DAMAGE != 0,
            warded: prototypes.int(o, "spell_flags").unwrap_or(0) & OSF_FIREARM_WARD != 0,
        }
        .with_ammo(&inv, prototypes)
    }

    /// A turn's action points: `max(5, speed)` (0x4b6e70).
    fn turn_ap(&self, c: &Seen) -> i32 {
        let speed = match (&c.sheet, c.npc) {
            (Some(sheet), _) => self.rules.0.derived(sheet).speed,
            (None, Some(n)) => self.map.mobiles.get(&n.sector).and_then(|m| m.get(n.index as usize)).map_or(0, |o| npc_speed(o, &self.map.prototypes)),
            _ => 0,
        };
        speed.max(MIN_AP)
    }

    /// A PC with no hit points left (it takes no turns; death is step 4).
    fn pc_down(&self, c: &Seen) -> bool {
        let Some(p) = c.player else { return false };
        self.world.0.as_ref().and_then(|w| w.players.get(&p)).is_some_and(|s| s.blueprint.sheet.hp_damage >= self.rules.0.derived(&s.blueprint.sheet).hit_points)
    }

    /// A PC out of fatigue (0x45d800: fatigue below 1): its turns pass (0x4b7580 → 0x45d790;
    /// one knocked out on its own turn ends it, 0x45d5a0 → 0x4b7c90).
    fn pc_out(&self, c: &Seen) -> bool {
        let Some(p) = c.player else { return false };
        self.world.0.as_ref().and_then(|w| w.players.get(&p)).is_some_and(|s| crate::is_unconscious(self.rules.0.derived(&s.blueprint.sheet).fatigue - s.blueprint.sheet.fatigue_damage))
    }

    /// A PC takes an attack's damage on its sheet (0x4b4390): hit points (at most all of them),
    /// poison, fatigue; the sheet is sent on ([`crate::sync_script_sheets`]).
    fn hurt_pc(&mut self, player: PlayerId, outcome: &AttackOutcome) {
        let rules = &self.rules.0;
        let Some(saved) = self.world.0.as_mut().and_then(|w| w.players.get_mut(&player)) else { return };
        let sheet = &mut saved.blueprint.sheet;
        let max = rules.derived(sheet).hit_points;
        sheet.hp_damage = (sheet.hp_damage + outcome.hp.max(0)).min(max);
        sheet.poison += outcome.poison.max(0);
        sheet.fatigue_damage += outcome.fatigue.max(0);
        sheet.injuries |= outcome.injured;
        self.sheets.0.insert(player);
    }

    /// An NPC's `ai_params.mes` row (by `npc_ai_data`, 0x4aaa60), the file read on first use.
    fn ai_row(&mut self, source: crate::NpcSource) -> Option<[i32; 17]> {
        if self.ai.0.is_none() {
            let vfs = self.vfs.as_ref().map(|v| v.0.clone());
            let guard = vfs.as_ref().and_then(|v| v.read().ok());
            let mes = guard.as_deref().and_then(|v| v.read(AI_PARAMS).ok()).and_then(|d| arcanum_formats::mes::MesFile::parse(&d).ok()).unwrap_or_default();
            self.ai.0 = Some(parse_ai_params(&mes));
        }
        let row = self.npc_int(source, "npc_ai_data").max(0) as usize;
        self.ai.0.as_ref()?.get(row).copied()
    }

    /// Uses up PC `player`'s waiting fate `option` (0x4f5270), its sheet sent on.
    fn consume_fate(&mut self, player: PlayerId, option: u8) -> bool {
        let Some(saved) = self.world.0.as_mut().and_then(|w| w.players.get_mut(&player)) else { return false };
        let used = crate::consume_fate(&mut saved.blueprint.sheet, option);
        if used {
            self.sheets.0.insert(player);
        }
        used
    }

    /// What the dark adds to `attacker`'s blow at `victim` with `skill` ([`dark_to_hit`]): the
    /// light on the target's tile (builder's call for 0x4dce10, the target's own lit colour).
    fn dark_difficulty(&self, attacker: &Seen, victim: &Seen, skill: usize) -> i32 {
        let hour = self.clock.now.ms / crate::HOUR_MS;
        let Some(light) = self.map.light_at(victim.at, hour) else { return 0 };
        let (rank, flags, dark_sight) = match (&attacker.sheet, attacker.npc) {
            // A PC's light from what it wears (0x467e80); an NPC's flags, or its worn light.
            (Some(sheet), _) => (sheet.training.get(skill).copied().unwrap_or(0), attacker.inventory.as_ref().map_or(0, |i| i.borne_light(&self.map.prototypes)), sheet.background == DARK_SIGHT_BACKGROUND),
            (None, Some(n)) => {
                let Some(o) = self.map.mobiles.get(&n.sector).and_then(|m| m.get(n.index as usize)) else { return 0 };
                let worn = self.map.npc_inventory(n.sector, n.index).map_or(0, |i| i.borne_light(&self.map.prototypes));
                (npc_skill(o, &self.map.prototypes, skill).1, self.npc_int(n, "critter_flags") | worn, self.npc_int(n, "critter_flags2") & OCF2_DARK_SIGHT != 0)
            }
            _ => return 0,
        };
        dark_to_hit(skill, rank, flags, dark_sight, crate::light::luminance(light))
    }

    /// Whether `watcher` sees `other` or hears it at noise 0 (0x4af260 / 0x4af470 at 0x4b2319):
    /// NPCs by their state ([`can_look`], [`ears`]); PCs by their Perception.
    fn notices_quietly(&self, watcher: &Seen, other: &Seen) -> bool {
        let dark_sight = match (&watcher.sheet, watcher.npc) {
            (Some(sheet), _) => sheet.background == DARK_SIGHT_BACKGROUND,
            (None, Some(n)) => self.npc_int(n, "critter_flags2") & OCF2_DARK_SIGHT != 0,
            _ => false,
        };
        let prowler = other.sheet.as_ref().filter(|_| other.prowling).map(|s| crate::prowl::Prowler::of(&self.rules.0, s, &self.map, other.at, self.clock.now.ms / crate::HOUR_MS, other.inventory.as_ref().map_or(0, |i| i.borne_light(&self.map.prototypes)), other.inventory.as_ref()).for_watcher(dark_sight));
        let (eyes, ears_pe) = match (&watcher.sheet, watcher.npc) {
            (Some(sheet), _) => {
                let pe = self.rules.0.stats(sheet)[5];
                (Some(pe), Some(pe))
            }
            (None, Some(n)) => match self.map.mobiles.get(&n.sector).and_then(|m| m.get(n.index as usize)) {
                Some(o) => {
                    let target = other.npc.and_then(|a| self.map.mobiles.get(&a.sector)?.get(a.index as usize));
                    (can_look(o, &self.map.prototypes, watcher.stunned.is_some(), target).then(|| npc_stat(o, &self.map.prototypes, 5)), ears(o, &self.map.prototypes, watcher.stunned.is_some()))
                }
                None => (None, None),
            },
            _ => (None, None),
        };
        let view = crate::MissileView(&self.map);
        eyes.is_some_and(|pe| sees(&view, watcher.at, watcher.rotation, pe, other.at, prowler)) || ears_pe.is_some_and(|pe| hears_noise(&view, watcher.at, pe, other.at, prowler, crate::prowl::NOISE_QUIET))
    }

    /// [`missile_difficulty`] for `attacker`'s shot with `skill` at `victim`: the line's objects
    /// ([`crate::blocking::missile_cost`]) and the living critters standing on it
    /// ([`Self::critters_cost`]).
    fn shot_difficulty(&self, all: &[Seen], attacker: &Seen, victim: &Seen, skill: usize) -> i32 {
        let (perception, master) = match (&attacker.sheet, attacker.npc) {
            (Some(sheet), _) => (self.rules.0.stats(sheet)[5], sheet.training.get(skill).is_some_and(|&t| t >= 3)),
            (None, Some(n)) => (self.npc_int_stat(n, 5), false),
            _ => (0, false),
        };
        let distance = crate::tile_distance(attacker.at, victim.at);
        let line = line_between(attacker.at, victim.at);
        let objects: i32 = line.iter().map(|t| self.map.missiles.costs.get(t).copied().unwrap_or(0)).sum();
        missile_difficulty(distance, perception, master, objects + self.critters_cost(all, attacker, victim, &line))
    }

    /// What the living critters on `line` cost a shot (0x44064d-0x44066a: types 0xf / 0x10, the
    /// dead skipped by 0x45d8d0, then their `flags` through [`crate::blocking::cover_cost`]).
    /// NPCs by their own flags; PCs by their prototypes' 0x4030 (shoot-through, seen through,
    /// cover: 20, all 13 alike). A PC's own followers, which its shot passes (owner's rule),
    /// still cost (builder's call).
    fn critters_cost(&self, all: &[Seen], attacker: &Seen, victim: &Seen, line: &[TileLoc]) -> i32 {
        let living = all.iter().filter(|c| c.net_id != attacker.net_id && c.net_id != victim.net_id).filter_map(|c| match (c.player, c.npc) {
            (Some(_), _) if self.pc_down(c) => None,
            (Some(_), _) => Some((c.at, PC_FLAGS)),
            (None, Some(n)) if self.map.is_dead(n.sector, n.index) => None,
            (None, Some(n)) => Some((c.at, self.npc_int(n, "flags"))),
            _ => None,
        });
        line_critters_cost(line, living)
    }

    /// Whether NPC `n`'s re-pick for a foe `distance` tiles away would change its hands.
    fn can_rearm(&self, n: crate::NpcSource, distance: u32) -> bool {
        let (Some(inv), Some(o)) = (self.map.npc_inventory(n.sector, n.index), self.map.mobiles.get(&n.sector).and_then(|m| m.get(n.index as usize))) else { return false };
        let fighter = npc_weapon_fighter(o, &self.map.prototypes);
        let pick = choose_weapon(&inv, &self.map.prototypes, &fighter, Some(distance));
        let vfs = self.vfs.as_ref().map(|v| v.0.clone());
        let guard = vfs.as_ref().and_then(|v| v.read().ok());
        let info = crate::item_info(&self.map.prototypes, |id| crate::world::art_size(guard.as_deref(), self.map.art.as_deref(), id));
        wield_pick(&inv, pick, &info).is_some()
    }

    /// Drops `c`'s wielded weapon on its tile (0x461a90); whether it had one.
    fn drop_weapon(&mut self, c: &Seen) -> bool {
        self.drop_weapon_at(c, c.at)
    }

    /// Puts `c`'s wielded weapon on tile `at`; whether it had one.
    fn drop_weapon_at(&mut self, c: &Seen, at: TileLoc) -> bool {
        self.drop_item_at(c, WEAPON_SLOT, at)
    }

    /// Puts `c`'s item at `location` on tile `at`; whether it had one.
    fn drop_item_at(&mut self, c: &Seen, location: i32, at: TileLoc) -> bool {
        let take = |inv: &mut crate::Inventory| {
            let at = inv.items.iter().position(|i| i.location == location)?;
            Some(inv.items.remove(at))
        };
        let item = match (c.player, c.npc) {
            (Some(p), _) => {
                let item = self.world.0.as_mut().and_then(|w| w.players.get_mut(&p)).and_then(|s| s.inventory.as_mut()).and_then(take);
                if item.is_some() {
                    self.inventories.0.insert(p);
                }
                item
            }
            (None, Some(n)) => {
                let mut inv = self.map.npc_inventory(n.sector, n.index);
                let item = inv.as_mut().and_then(take);
                if let (Some(inv), Some(_)) = (inv, &item) {
                    self.map.set_npc_inventory(n.sector, n.index, inv);
                }
                item
            }
            _ => None,
        };
        let Some(item) = item else { return false };
        let (prototypes, art) = (self.map.prototypes.clone(), self.map.art.clone());
        let vfs = self.vfs.as_ref().map(|v| v.0.clone());
        let guard = vfs.as_ref().and_then(|v| v.read().ok());
        let info = crate::item_info(&prototypes, |id| crate::world::art_size(guard.as_deref(), art.as_deref(), id));
        crate::ground::place_on_ground(item, at, &info, &mut self.map, &mut self.world, &self.session, &mut self.outbox);
        if let Some(n) = c.npc {
            crate::ground::save_change(&mut self.world, &self.map, n.sector);
        }
        true
    }

    /// Whether `critter`'s wielded weapon is of object `flags` [`OF_WEAR_IMMUNE`]: never worn,
    /// destroyed or blown up (0x4b4e81 clears 0x4000 / 0x8000 / 0x10000).
    fn weapon_wear_immune(&self, critter: &Seen) -> bool {
        self.wielded(critter).and_then(|p| self.map.prototypes.get(p)).is_some_and(immune_to_wear)
    }

    /// The prototype of the item in `critter`'s weapon slot.
    fn wielded(&self, critter: &Seen) -> Option<u32> {
        let found = |inv: &crate::Inventory| inv.items.iter().find(|i| i.location == WEAPON_SLOT).map(|i| i.prototype);
        match (critter.player, critter.npc) {
            (Some(_), _) => critter.inventory.as_ref().and_then(found),
            (None, Some(n)) => self.map.npc_inventory(n.sector, n.index).as_ref().and_then(found),
            _ => None,
        }
    }

    /// A carried item of `holder` at `location`: its script object and prototype.
    fn carried_at(&self, holder: &Seen, location: i32) -> Option<(crate::ScriptObject, u32)> {
        let inv = match (holder.player, holder.npc) {
            (Some(p), _) => self.world.0.as_ref().and_then(|w| w.players.get(&p)).and_then(|s| s.inventory.clone()),
            (None, Some(n)) => self.map.npc_inventory(n.sector, n.index),
            _ => None,
        }?;
        let prototype = inv.items.iter().find(|i| i.location == location)?.prototype;
        let item = match (holder.player, holder.npc) {
            (Some(p), _) => crate::ScriptObject::Carried { holder: p, prototype },
            (None, Some(n)) => crate::ScriptObject::NpcCarried { sector: n.sector, index: n.index, prototype },
            _ => return None,
        };
        Some((item, prototype))
    }

    /// Destroys `holder`'s item at `location` (0x43cca0): after its destroy script
    /// ([`ItemDestroy`]).
    fn destroy_item(&mut self, holder: &Seen, location: i32, player: Option<PlayerId>) {
        if let Some((item, prototype)) = self.carried_at(holder, location) {
            self.events.2.push(ItemDestroy { item, prototype, location, player });
        }
    }

    /// Destroys the attacker's wielded weapon (0x4b4f48 → 0x43cca0).
    fn destroy_weapon(&mut self, attacker: &Seen) {
        let player = attacker.player.or(attacker.leader);
        self.destroy_item(attacker, WEAPON_SLOT, player);
    }

    /// Takes a shot's rounds from the attacker's inventory (builder's call: the original's
    /// decrement was not found; its check 0x432cf0 counts `weapon_ammo_consumption`).
    fn spend_ammo(&mut self, attacker: &Seen, ammo_type: i32, rounds: i32) {
        match (attacker.player, attacker.npc) {
            (Some(p), _) => {
                let prototypes = self.map.prototypes.clone();
                if let Some(inv) = self.world.0.as_mut().and_then(|w| w.players.get_mut(&p)).and_then(|s| s.inventory.as_mut()) {
                    take_ammo(inv, ammo_type, rounds, &prototypes);
                    self.inventories.0.insert(p);
                }
            }
            (None, Some(n)) => {
                if let Some(mut inv) = self.map.npc_inventory(n.sector, n.index) {
                    take_ammo(&mut inv, ammo_type, rounds, &self.map.prototypes);
                    self.map.set_npc_inventory(n.sector, n.index, inv);
                    crate::ground::save_change(&mut self.world, &self.map, n.sector);
                }
            }
            _ => {}
        }
    }

    /// Wears the victim's items a hit struck (0x4b4390): the armour where it landed and
    /// its weapon, if it has them; what happened ([`AttackOutcome::items`]). An item worn to
    /// nothing breaks (0x43f4cd): it comes off into the pack (0x464c80), or falls to the ground
    /// without room (0x461a70), and its bust script runs, `by` the triggerer; a broken one is
    /// destroyed ([`crate::repair::Wear`]).
    fn wear_items(&mut self, by: &Seen, victim: &Seen, outcome: &AttackOutcome) -> u8 {
        use crate::repair::Wear;
        let mut inventory = match (victim.player, victim.npc) {
            (Some(p), _) => self.world.0.as_ref().and_then(|w| w.players.get(&p)).and_then(|s| s.inventory.clone()),
            (None, Some(n)) => self.map.npc_inventory(n.sector, n.index),
            _ => None,
        };
        let Some(inv) = inventory.as_mut() else { return 0 };
        let player = by.player.or(victim.player).or(by.leader).or(victim.leader);
        let info = self.item_facts();
        let mut done = 0;
        let (mut busted, mut dropped, mut gone) = (Vec::new(), Vec::new(), Vec::new());
        for (bit, slot, broke) in [(ARMOUR_DAMAGED, BodyPart::slot_of(outcome.part), ARMOUR_BROKEN), (WEAPON_DAMAGED, WEAPON_SLOT, WEAPON_BROKEN)] {
            if outcome.items & bit == 0 {
                continue;
            }
            match crate::repair::wear_item(inv, slot, &self.map.prototypes) {
                None => {}
                Some(Wear::Worn) => done |= bit,
                Some(Wear::Broke) => {
                    done |= broke;
                    let Some(prototype) = inv.items.iter().find(|i| i.location == slot).map(|i| i.prototype) else { continue };
                    busted.push(prototype);
                    let cells = info(prototype).map_or((1, 1), |i| i.cells);
                    match crate::Inventory::free_spot(&inv.occupied(&info), cells).and_then(|spot| inv.move_item(slot, spot, crate::Wearer { race: 0, gender: 0 }, &info).ok()) {
                        Some(off) => *inv = off,
                        None => dropped.push(slot),
                    }
                }
                Some(Wear::Gone) => {
                    done |= broke;
                    gone.push(slot);
                }
            }
        }
        if done == 0 {
            return 0;
        }
        match (victim.player, victim.npc) {
            (Some(p), _) => {
                if let Some(saved) = self.world.0.as_mut().and_then(|w| w.players.get_mut(&p)) {
                    saved.inventory = inventory;
                }
                self.inventories.0.insert(p);
            }
            (None, Some(n)) => {
                if let Some(inv) = inventory {
                    self.map.set_npc_inventory(n.sector, n.index, inv);
                }
            }
            _ => {}
        }
        for slot in dropped {
            self.drop_item_at(victim, slot, victim.at);
        }
        for slot in gone {
            self.destroy_item(victim, slot, player);
        }
        if let Some(player) = player {
            for prototype in busted {
                let item = match (victim.player, victim.npc) {
                    (Some(p), _) => crate::ScriptObject::Carried { holder: p, prototype },
                    (None, Some(n)) => crate::ScriptObject::NpcCarried { sector: n.sector, index: n.index, prototype },
                    _ => continue,
                };
                self.events.1.push(WeaponEvent { item, prototype, event: EVENT_BUST, player, attacker: object_of(by), victim: None });
            }
        }
        done
    }

    /// The facts of items by prototype ([`crate::item_info`]).
    fn item_facts(&self) -> impl Fn(u32) -> Option<crate::ItemInfo> + use<> {
        let (prototypes, art) = (self.map.prototypes.clone(), self.map.art.clone());
        let vfs = self.vfs.as_ref().map(|v| v.0.clone());
        move |id| {
            let guard = vfs.as_ref().and_then(|v| v.read().ok());
            crate::item_info(&prototypes, |a| crate::world::art_size(guard.as_deref(), art.as_deref(), a))(id)
        }
    }

    /// A critter's hit points left in percent (0x4ab400).
    fn hp_percent(&self, c: &Seen) -> i32 {
        let (left, max) = match (c.player, c.npc) {
            (Some(p), _) => {
                let Some(saved) = self.world.0.as_ref().and_then(|w| w.players.get(&p)) else { return 100 };
                let max = self.rules.0.derived(&saved.blueprint.sheet).hit_points;
                (max - saved.blueprint.sheet.hp_damage, max)
            }
            (None, Some(n)) => self.map.npc_hp(n.sector, n.index).unwrap_or((1, 1)),
            _ => return 100,
        };
        if max > 0 { left * 100 / max } else { 100 }
    }

    /// The side a critter stands with (0x4ae0a0): a PC and the NPCs following it; an NPC alone.
    fn party(&self, c: &Seen, all: &[Seen]) -> Party {
        let level = |m: &Seen| match (&m.sheet, m.npc) {
            (Some(sheet), _) => sheet.level,
            (None, Some(n)) => self.map.mobiles.get(&n.sector).and_then(|v| v.get(n.index as usize)).map_or(0, |o| npc_stat(o, &self.map.prototypes, STAT_LEVEL)),
            _ => 0,
        };
        match c.player.or(c.leader) {
            Some(p) => {
                let members: Vec<&Seen> = all.iter().filter(|m| m.player == Some(p) || m.leader == Some(p)).collect();
                Party { count: members.len() as i32, levels: members.iter().map(|m| level(m)).sum() }
            }
            None => Party { count: 1, levels: level(c) },
        }
    }

    /// An NPC's field, its prototype's where it has none.
    fn npc_int(&self, source: crate::NpcSource, field: &str) -> i32 {
        self.map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)).and_then(|o| self.map.prototypes.int(o, field)).unwrap_or(0)
    }

    /// Whether a reputation PC `player` holds lists `faction` (0x4c21e0: the three faction
    /// slots of its `rules/gamerep.mes` line).
    fn pc_kin(&mut self, player: PlayerId, faction: i32) -> bool {
        if faction == 0 {
            return false;
        }
        self.load_reputations();
        let (Some(table), Some(saved)) = (self.reputations.0.as_ref(), self.world.0.as_ref().and_then(|w| w.players.get(&player))) else { return false };
        table.lists_faction(saved.script_state.reputations.iter().copied(), faction)
    }

    fn load_reputations(&mut self) {
        if self.reputations.0.is_none() {
            let vfs = self.vfs.as_ref().map(|v| v.0.clone());
            let guard = vfs.as_ref().and_then(|v| v.read().ok());
            self.reputations.0 = Some(guard.as_deref().and_then(|v| v.read(crate::GAME_REP).ok()).and_then(|d| arcanum_formats::mes::MesFile::parse(&d).ok()).map(|m| crate::ReputationEffects::parse(&m)).unwrap_or_default());
        }
    }

    /// NPC `source`'s reaction to PC `player`, everything counted.
    fn reaction(&mut self, source: crate::NpcSource, player: PlayerId) -> Option<i32> {
        self.load_reputations();
        let o = self.map.mobiles.get(&source.sector)?.get(source.index as usize)?;
        let saved = self.world.0.as_ref()?.players.get(&player)?;
        Some(crate::reaction_to(&saved.script_state, &crate::met_key(source.object()), &crate::NpcReaction::of(o, &self.map.prototypes), &self.rules.0, &saved.blueprint.sheet, self.reputations.0.as_ref()?))
    }

    /// Why follower `source` of PC `leader` will not fight `foe` (0x4adcc0, non-zero codes):
    /// 4 a follower of the same leader (0x4add66); 1 the follower good (alignment, stat 0x13,
    /// above 0) and the foe's alignment at least its row's column 12 (0x4adda4); 2 the same
    /// non-zero faction (0x45d320); 3 allied to it (0x45d110). `spell_flags` 0x8000000 (a bound
    /// follower, 0x459040) always fights.
    fn follower_refusal(&mut self, source: crate::NpcSource, leader: PlayerId, foe: &Seen) -> Option<u8> {
        if self.npc_int(source, "spell_flags") & OSF_BOUND != 0 {
            return None;
        }
        if foe.leader == Some(leader) {
            return Some(4);
        }
        let alignment = self.npc_int_stat(source, 0x13);
        let foe_alignment = match (foe.player, foe.npc) {
            (Some(p), _) => self.world.0.as_ref().and_then(|w| w.players.get(&p)).map_or(0, |s| s.blueprint.sheet.alignment),
            (None, Some(n)) => self.npc_int_stat(n, 0x13),
            _ => 0,
        };
        if alignment > 0 && self.ai_row(source).is_some_and(|row| foe_alignment >= row[12]) {
            return Some(1);
        }
        let own = self.npc_int(source, "npc_faction");
        if let Some(n) = foe.npc {
            let theirs = self.npc_int(n, "npc_faction");
            if own != 0 && own == theirs {
                return Some(2);
            }
        }
        let foe_faction = foe.npc.map(|n| self.npc_int(n, "npc_faction"));
        let kin = foe.player.is_some_and(|p| self.pc_kin(p, own));
        allied(Some(leader), own, foe.player, foe.leader, foe_faction, kin).then_some(3)
    }

    /// Follower `source` (entity `net_id`) tells leader `leader` why it will not fight (0x414370
    /// at 0x4a9d0a, `gd_npc` by refusal code: 1 → 4900, 2 → 5000, 3 → 5100, 4 → 4800, table
    /// 0x414478).
    fn refusal_bark(&mut self, source: crate::NpcSource, net_id: NetId, leader: PlayerId, code: u8) {
        let base = match code {
            1 => 4900,
            2 => 5000,
            3 => 5100,
            _ => 4800,
        };
        self.bark(source, net_id, leader, base);
    }

    /// NPC `source` (entity `net_id`) floats a `gd_npc` line of block `base` to `leader`'s PC,
    /// as the callback 0x5f8488 (0x568430) shows it.
    fn bark(&mut self, source: crate::NpcSource, net_id: NetId, leader: PlayerId, base: u32) {
        let Some(sheet) = self.world.0.as_ref().and_then(|w| w.players.get(&leader)).map(|s| s.blueprint.sheet.clone()) else { return };
        let vfs = self.vfs.as_ref().map(|v| v.0.clone());
        let guard = vfs.as_ref().and_then(|v| v.read().ok());
        let info = crate::npc_details(&self.map, source.object(), &mut self.generic, guard.as_deref());
        let who = crate::Speaker { npc: &info, pc_name: &sheet.name, pc_female: sheet.gender == arcanum_rules::FEMALE, pc_race: sheet.race };
        if let Some((text, voice)) = crate::npc_line(&mut self.generic, guard.as_deref(), &mut self.rng, &who, crate::Block::Npc, base) {
            crate::ground::broadcast(&self.session, &mut self.outbox, LedgerDelta::FloatText { speaker: net_id, text, colour: crate::FloatColour::White, voice });
        }
    }

    /// [`Self::bark`] unless the speaker is barred from it ([`bark_vetoed`]): the flee, surrender
    /// and begging lines (0x413ea0) and the followers' remarks (0x413f50) ask 0x4ad800 first.
    fn checked_bark(&mut self, speaker: &Seen, to: PlayerId, base: u32) {
        let Some(source) = speaker.npc else { return };
        let gone = self.map.is_dead(source.sector, source.index) || self.map.npc_down(source.sector, source.index);
        let (flags, critter, spell) = (self.npc_int(source, "flags"), self.npc_int(source, "critter_flags"), self.npc_int(source, "spell_flags"));
        if !bark_vetoed(flags, critter | if speaker.stunned.is_some() { OCF_STUNNED } else { 0 }, spell, gone, speaker.leader, to) {
            self.bark(source, speaker.net_id, to, base);
        }
    }

    fn npc_int_stat(&self, source: crate::NpcSource, stat: usize) -> i32 {
        self.map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)).map_or(0, |o| npc_stat(o, &self.map.prototypes, stat))
    }

    /// Adds `delta` to the reaction NPC `source` keeps for PC `player` (as script action 0x2b).
    fn change_reaction(&mut self, source: crate::NpcSource, player: PlayerId, delta: i32) {
        let Some(base) = self.map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)).map(|o| crate::NpcReaction::of(o, &self.map.prototypes).base) else { return };
        let Some(saved) = self.world.0.as_mut().and_then(|w| w.players.get_mut(&player)) else { return };
        let key = crate::met_key(source.object());
        let stored = saved.script_state.reactions.get(&key).copied().unwrap_or(base);
        saved.script_state.reactions.insert(key, stored + delta);
    }

    fn tell(&mut self) {
        let delta = match &self.combat.0 {
            Some(e) => e.delta(),
            None => LedgerDelta::Combat { order: vec![], turn: None, ap: 0 },
        };
        crate::ground::broadcast(&self.session, &mut self.outbox, delta);
    }
}

/// Who fights around `centre` (0x4b71e0 / 0x4b73a0): the PCs within the radius first, then
/// their fighting followers (0x4b7eb0), then the other fighting NPCs, each in the order given;
/// `first` leads.
fn gather(all: &[Seen], centre: TileLoc, first: Option<NetId>) -> Vec<NetId> {
    let near = |c: &&Seen| crate::within_radius(c.at, centre, ENGAGE_RADIUS);
    let mut order: Vec<NetId> = first.into_iter().collect();
    order.extend(all.iter().filter(near).filter(|c| c.player.is_some()).map(|c| c.net_id).filter(|n| Some(*n) != first));
    order.extend(all.iter().filter(near).filter(|c| c.leader.is_some() && c.fighting).map(|c| c.net_id).filter(|n| Some(*n) != first));
    order.extend(all.iter().filter(near).filter(|c| c.foe() && c.fighting).map(|c| c.net_id).filter(|n| Some(*n) != first));
    order
}

/// Runs the fight: attacks and turn ends asked for, walks ending in attacks, NPC turns (passed
/// for now), points running out, rounds and the end. See the module docs.
pub fn run_combat(mut commands: Commands, mut w: CombatWorld, critters: Query<CombatantQuery>) {
    let all = seen(&critters);
    let find = |id: NetId| all.iter().find(|c| c.net_id == id).cloned();
    let mut changed = false;
    w.joined.clear();
    // Hits whose taking-damage scripts answered: the damage goes ahead unless one said no.
    for (hit, allowed) in std::mem::take(&mut w.checks.answered) {
        if allowed && let Some(victim) = find(hit.victim) {
            damage_npc(&mut commands, &mut w, hit.attacker_player, &victim, &hit.outcome);
            changed = true;
        }
    }
    // Fights started by dialogue or scripts: the NPC fights its target (0x4a9650).
    for (npc, target) in std::mem::take(&mut w.starts.0) {
        let (Some(attacker), Some(victim)) = (find(npc), find(target)) else { continue };
        let Some(source) = attacker.npc else { continue };
        if w.map.npc_down(source.sector, source.index) {
            continue;
        }
        commands.entity(attacker.entity).insert(Fighting { target });
        w.joined.push(npc);
        if !attacker.fighting && let Some(player) = victim.player {
            w.events.0.push((source.object(), EVENT_ENTER_COMBAT, player, crate::ScriptObject::Pc(player)));
        }
        match w.combat.0.as_mut() {
            None => {
                let mut order = gather(&all, victim.at, None);
                for id in [target, npc] {
                    if !order.contains(&id) {
                        order.push(id);
                    }
                }
                let first = order.first().and_then(|id| find(*id));
                let ap = first.map_or(MIN_AP, |c| w.turn_ap(&c));
                w.combat.0 = Some(Encounter { order, turn: 0, ap, round: 1, fresh: true });
            }
            Some(e) => {
                if !e.order.contains(&npc) {
                    e.order.push(npc);
                }
            }
        }
        changed = true;
    }
    // Attacks asked for: the first starts the fight (0x4b3f20 → 0x4b71e0).
    for (player, target, _seq) in std::mem::take(&mut w.requests.attacks) {
        let (Some(pc), Some(victim)) = (all.iter().find(|c| c.player == Some(player)).cloned(), find(target)) else { continue };
        let Some(source) = victim.npc else { continue };
        if w.map.is_dead(source.sector, source.index) {
            continue;
        }
        if !victim.fighting {
            commands.entity(victim.entity).insert(Fighting { target: pc.net_id });
            w.joined.push(target);
            w.events.0.push((source.object(), EVENT_ENTER_COMBAT, player, crate::ScriptObject::Pc(player)));
        }
        if w.combat.0.is_none() {
            let mut order = gather(&all, pc.at, Some(pc.net_id));
            if !order.contains(&target) {
                order.push(target);
            }
            let ap = w.turn_ap(&pc);
            w.combat.0 = Some(Encounter { order, turn: 0, ap, round: 1, fresh: true });
            changed = true;
        }
        let Some(e) = w.combat.0.as_mut() else { continue };
        if !e.order.contains(&target) {
            e.order.push(target);
            changed = true;
        }
        if e.current() == Some(pc.net_id) {
            changed |= attack_or_approach(&mut commands, &mut w, &all, &pc, &victim);
        }
    }
    for player in std::mem::take(&mut w.requests.end_turn) {
        let Some(pc) = all.iter().find(|c| c.player == Some(player)) else { continue };
        if w.combat.0.as_ref().and_then(Encounter::current) == Some(pc.net_id) {
            next_turn(&mut w, &all);
            changed = true;
        }
    }
    // A walk to the target over: the attack.
    if let Some(current) = w.combat.0.as_ref().and_then(Encounter::current)
        && let Some(c) = find(current)
        && let (Some(p), false) = (c.pending, c.walking)
    {
        commands.entity(c.entity).remove::<PendingAttack>();
        if let Some(victim) = find(p.target) {
            changed |= attack_or_approach(&mut commands, &mut w, &all, &c, &victim);
        }
    }
    // An NPC's turn: flee or fight; a PC out of points, down or gone ends its turn.
    if let Some(e) = w.combat.0.clone() {
        let current = e.current().and_then(find);
        let done = match &current {
            None => true,
            // A stunned critter loses its turn, 3 off its count (0x4b7580 → 0x427000).
            Some(c) if c.stunned.is_some() && e.fresh => {
                let left = c.stunned.map_or(0, |s| s.left) - 3;
                if left < 1 {
                    commands.entity(c.entity).remove::<Stunned>();
                } else {
                    commands.entity(c.entity).insert(Stunned { left });
                }
                true
            }
            Some(c) if c.walking || c.pending.is_some() => false,
            // Down, or stunned or paralyzed by its flags: it cannot act (0x4b7580 → 0x45d790,
            // `critter_flags` 0x60), and its turn passes.
            Some(c) if c.npc.is_some_and(|n| w.map.npc_down(n.sector, n.index) || w.npc_int(n, "critter_flags") & OCF_HELD != 0) => true,
            Some(c) if c.npc.is_some() => {
                let acted = npc_turn(&mut commands, &mut w, c, &all, e.fresh);
                if let Some(e) = w.combat.0.as_mut() {
                    e.fresh = false;
                }
                changed |= acted;
                !acted
            }
            Some(c) => e.ap <= 0 || w.pc_down(c) || w.pc_out(c),
        };
        if done {
            next_turn(&mut w, &all);
            changed = true;
        }
    }
    // No NPC left fighting, or no PC standing: the fight is over at once (0x4b7580 finds no
    // active critter).
    if let Some(e) = &w.combat.0 {
        let fights = e.order.iter().filter_map(|id| find(*id)).any(|c| c.foe() && c.npc.is_some_and(|n| !w.map.npc_down(n.sector, n.index)) && (c.fighting || w.joined.contains(&c.net_id)));
        let stands = e.order.iter().filter_map(|id| find(*id)).any(|c| c.player.is_some() && !w.pc_down(&c));
        if !fights || !stands {
            w.combat.0 = None;
            changed = true;
        }
    }
    // Out of a fight the stun wears off (builder's call: the original counts it down by its
    // animation goal's ticks, 0x426f60, at a rate not traced).
    if w.combat.0.is_none() {
        for c in all.iter().filter(|c| c.stunned.is_some()) {
            commands.entity(c.entity).remove::<Stunned>();
        }
    }
    if changed {
        w.tell();
    }
}

/// The next critter's turn (0x4b7690); past the last, a new round gathered again (0x4b73a0),
/// or the end when no NPC fights (0x4b7330).
fn next_turn(w: &mut CombatWorld, all: &[Seen]) {
    let Some((turn, len, centre)) = w.combat.0.as_mut().map(|e| {
        e.turn += 1;
        let centre = e.order.iter().find_map(|id| all.iter().find(|c| c.net_id == *id && c.player.is_some())).map(|c| c.at);
        (e.turn, e.order.len(), centre)
    }) else {
        return;
    };
    if turn >= len {
        let alive: Vec<Seen> = all.iter().filter(|c| c.npc.is_none_or(|n| !w.map.npc_down(n.sector, n.index))).cloned().collect();
        let order = centre.map(|at| gather(&alive, at, None)).unwrap_or_default();
        let fights = order.iter().any(|id| alive.iter().any(|c| c.net_id == *id && c.foe()));
        let stands = order.iter().any(|id| alive.iter().any(|c| c.net_id == *id && c.player.is_some() && !w.pc_down(c)));
        if !fights || !stands {
            w.combat.0 = None;
            return;
        }
        if let Some(e) = w.combat.0.as_mut() {
            e.order = order;
            e.turn = 0;
            e.round += 1;
        }
    }
    let next = w.combat.0.as_ref().and_then(Encounter::current).and_then(|id| all.iter().find(|c| c.net_id == id)).cloned();
    let ap = next.map_or(0, |c| w.turn_ap(&c));
    if let Some(e) = w.combat.0.as_mut() {
        e.ap = ap;
        e.fresh = true;
    }
}

/// The attacker strikes if the target is in reach and the points last; out of reach it walks
/// towards it first until in reach, as far as its points go (0x4b78d0). True if something
/// changed.
fn attack_or_approach(commands: &mut Commands, w: &mut CombatWorld, all: &[Seen], attacker: &Seen, victim: &Seen) -> bool {
    let mut fighter = w.fighter(attacker);
    // Out of ammunition (0x4b3219 → 0x4b65d0 fails): no shot. A PC cannot shoot; an NPC asks
    // for its weapon to be picked again (0x4b3284 → 0x4adff0: `npc_flags` 0x80) and acts on
    // once [`rearm_npcs`] has. One that can change nothing (no room to put the weapon away)
    // fights bare-handed with it in hand (builder's call).
    if fighter.out_of_ammo() {
        let Some(n) = attacker.npc else { return false };
        if w.can_rearm(n, reach_between(attacker.at, victim.at)) {
            let flags = w.map.npc_flags(n.sector, n.index).unwrap_or(0);
            w.map.set_npc_flags(n.sector, n.index, flags | ONF_REPICK_WEAPON);
            crate::ground::save_change(&mut w.world, &w.map, n.sector);
            return true;
        }
        fighter.weapon = None;
    }
    let cost = fighter.attack_cost();
    let ap = w.combat.0.as_ref().map_or(0, |e| e.ap);
    if reach_between(attacker.at, victim.at) <= fighter.reach() {
        // A PC with points left may act past them (0x4b7790); its turn ends after.
        if ap < cost && !(ap > 0 && attacker.player.is_some()) {
            return false;
        }
        if let Some(e) = w.combat.0.as_mut() {
            e.ap = (e.ap - cost).max(0);
        }
        // A throw strikes the first critter on its way (0x4b3c00 sets 0x200: 0x4b33cf →
        // 0x4ade00, the target's own tile not counted), rolled for afresh (0x4b3405).
        // Throws, arrows and bullets strike what is in their way (owner's call: the original
        // redirects throws only, 0x4b3c00's flag 0x200).
        let thrown = fighter.weapon.as_ref().is_some_and(|wp| wp.skill == SKILL_THROWING);
        let missile = fighter.weapon.as_ref().is_some_and(|wp| wp.range > 1);
        let way = if missile { in_the_way(w, all, attacker, victim) } else { InTheWay::Clear };
        let lands = match way {
            InTheWay::Critter(other) => {
                let outcome = strike(commands, w, all, attacker, &other, &fighter, Aim::Stray);
                stray_remark(w, all, attacker, victim, &outcome);
                other.at
            }
            // Stopped by a wall or object: it strikes that, and nobody (0x4b34a6: an object is
            // hit outright; objects take no damage here).
            InTheWay::Stopped(at) => {
                crate::ground::broadcast(&w.session, &mut w.outbox, LedgerDelta::Attacked { attacker: attacker.net_id, target: victim.net_id, outcome: AttackOutcome { multiplier: 100, ..Default::default() } });
                at
            }
            InTheWay::Clear => {
                let _ = strike(commands, w, all, attacker, victim, &fighter, if thrown { Aim::Thrown } else { Aim::Meant });
                victim.at
            }
        };
        // The thrown weapon lands where its flight ended, hit or miss (0x4b2690 with the
        // projectile's 0x40: `projectile_parent_weapon` to its tile, 0x4b271d-0x4b2746), the
        // whole item (no stack split found).
        if thrown {
            w.drop_weapon_at(attacker, lands);
        }
        return true;
    }
    // A path towards the target, to the first tile in reach (next to it for melee), as many
    // steps as the points pay for.
    let goals = neighbours(victim.at);
    let path = goals.into_iter().filter_map(|g| crate::find_path(&*w.map, attacker.at, g).ok()).min_by_key(Vec::len);
    let Some(mut path) = path else { return false };
    if let Some(i) = path.iter().position(|&t| reach_between(t, victim.at) <= fighter.reach()) {
        path.truncate(i + 1);
    }
    let steps = (ap / STEP_AP).max(0) as usize;
    path.truncate(steps);
    if path.is_empty() {
        return false;
    }
    if let Some(e) = w.combat.0.as_mut() {
        e.ap -= path.len() as i32 * STEP_AP;
    }
    commands.entity(attacker.entity).insert((crate::Walking { steps: path.into(), progress: 0.0 }, PendingAttack { target: victim.net_id }));
    true
}

/// `rules/ai_params.mes`: 17 numbers a row, the row by `npc_ai_data` (0x4aaa60).
#[derive(Resource, Debug, Default)]
pub struct AiParams(pub Option<Vec<[i32; 17]>>);

/// The AI rows of `mes`.
pub fn parse_ai_params(mes: &arcanum_formats::mes::MesFile) -> Vec<[i32; 17]> {
    let mut rows = Vec::new();
    for n in 0.. {
        let Some(line) = mes.get(n) else { break };
        let mut row = [0; 17];
        for (r, v) in row.iter_mut().zip(line.split_whitespace()) {
            *r = v.parse().unwrap_or(0);
        }
        rows.push(row);
    }
    rows
}

/// `critter_flags`: never flees (0x4ab2f0).
pub const OCF_NO_FLEE: i32 = 0x0800_0000;
/// How far a fleeing NPC runs in one turn at most, in tiles (builder's call).
pub const FLEE_TILES: u32 = 8;

/// A side's weight for fleeing (0x4ae0a0): how many, and their levels summed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Party {
    pub count: i32,
    pub levels: i32,
}

/// Whether an NPC flees its target (0x4ab2f0), never with `OCF_NO_FLEE`: not from a target below
/// its row's column 3 hit point percent; at or below its own column 0 percent; else when the
/// target's side is column 1 larger, or (when not) its levels column 2 higher.
pub fn should_flee(hp_percent: i32, target_percent: i32, own: Party, theirs: Party, row: &[i32; 17], critter_flags: i32) -> bool {
    if critter_flags & OCF_NO_FLEE != 0 || target_percent < row[3] {
        return false;
    }
    if hp_percent <= row[0] {
        return true;
    }
    if theirs.count - own.count < row[1] { row[2] <= theirs.levels - own.levels } else { true }
}

/// Fights no more: it gave up to `to` (`critter_flags` 0x4000, 0x4aabe0 state 3).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surrendered {
    pub to: NetId,
}

/// The hit point percent at which a surrendered NPC may take heart (0x4a945a), and the chance a
/// tick (1 in 500).
pub const RECOVER_PERCENT: i32 = 80;
pub const RECOVER_ODDS: i32 = 500;

/// A fighting NPC's turn: at its start, flee if hurt enough (it walks away and stops fighting);
/// else attack its target, or a standing PC of the fight when that one is gone, walking next to
/// it first. False when it has nothing more to do.
fn npc_turn(commands: &mut Commands, w: &mut CombatWorld, npc: &Seen, all: &[Seen], fresh: bool) -> bool {
    let Some(source) = npc.npc else { return false };
    if !npc.fighting {
        return false;
    }
    // A leaderless NPC's think in a fight runs its start-combat script (15), triggerer its
    // target, the answer ignored (0x4a8570 at 0x4a86ac); here once at the start of each of its
    // turns (builder's call: the original thinks more often). End combat (16, 0x4a866b) runs
    // only in real time.
    if fresh && npc.leader.is_none()
        && let Some(target) = npc.target.and_then(|t| all.iter().find(|c| c.net_id == t))
        && let Some(player) = target.player.or(target.leader)
    {
        w.events.0.push((source.object(), EVENT_START_COMBAT, player, object_of(target)));
    }
    let row = w.ai_row(source);
    if fresh {
        let flags = w.npc_int(source, "critter_flags");
        let from_critter = npc.target.and_then(|t| all.iter().find(|c| c.net_id == t));
        let percent = w.hp_percent(npc);
        let flees = match (row, from_critter) {
            (Some(row), Some(t)) => should_flee(percent, w.hp_percent(t), w.party(npc, all), w.party(t, all), &row, flags),
            (Some(row), None) => should_flee(percent, 100, Party::default(), Party::default(), &row, flags),
            _ => false,
        };
        if flees {
            let from = from_critter.map_or(npc.at, |c| c.at);
            commands.entity(npc.entity).remove::<Fighting>();
            // Fleeing or giving up, it barks (0x4aabe0 states 2 / 3 → 0x413ea0 at 0x4aad4b,
            // `gd_npc` 3000-3099), its words for the PC it ran from.
            if let Some(player) = from_critter.and_then(|c| c.player) {
                w.checked_bark(npc, player, FLEE_BARK);
            }
            if let Some(player) = from_critter.and_then(|c| c.player) {
                w.events.0.push((source.object(), EVENT_EXIT_COMBAT, player, crate::ScriptObject::Pc(player)));
            }
            let away = flee_goal(npc.at, from, FLEE_TILES);
            let ap = w.combat.0.as_ref().map_or(0, |e| e.ap);
            let path = crate::find_path(&*w.map, npc.at, away).ok().map(|mut p| {
                p.truncate((ap / STEP_AP).max(0) as usize);
                p
            });
            match path.filter(|p| !p.is_empty()) {
                Some(path) => {
                    commands.entity(npc.entity).insert(crate::Walking { steps: path.into(), progress: 0.0 });
                }
                // Nowhere to run: it gives up (0x4ac180 → 0x4ac620).
                None => {
                    commands.entity(npc.entity).insert(Surrendered { to: from_critter.map_or(npc.net_id, |c| c.net_id) });
                }
            }
            if let Some(e) = w.combat.0.as_mut() {
                e.ap = 0;
            }
            return false;
        }
    }
    let in_fight = |c: &&Seen| w.combat.0.as_ref().is_some_and(|e| e.order.contains(&c.net_id));
    let standing = |c: &Seen| match (c.player, c.npc) {
        (Some(_), _) => !w.pc_down(c),
        (None, Some(n)) => !w.map.npc_down(n.sector, n.index),
        _ => false,
    };
    let target = npc.target.and_then(|t| all.iter().find(|c| c.net_id == t)).filter(|c| standing(c)).cloned();
    // Else the nearest of the other side: a follower's foes are the fighting NPCs, a foe's the
    // PCs; unconscious PCs last (0x4ab460 adds 1000 to a downed critter's score).
    let other_side = |c: &&Seen| if npc.leader.is_some() { c.foe() && c.fighting } else { c.player.is_some() };
    let target = target.or_else(|| all.iter().filter(in_fight).filter(other_side).filter(|c| standing(c)).min_by_key(|c| (w.pc_out(c), crate::tile_distance(c.at, npc.at))).cloned());
    let Some(target) = target else { return false };
    if npc.target != Some(target.net_id) {
        commands.entity(npc.entity).insert(Fighting { target: target.net_id });
    }
    // Keeping away (0x4ac6e0 → 0x4ac7b0): one in 20 turns, a fighter with a column 15 distance
    // nearer than it steps back to it.
    if fresh
        && let Some(keep) = row.map(|r| r[15]).filter(|&k| k > 1)
        && reach_between(npc.at, target.at) < keep as u32
        && w.rng.range(1, 20) == 1
        && let Some(path) = back_off(w, npc.at, target.at, keep as u32)
    {
        if let Some(e) = w.combat.0.as_mut() {
            e.ap -= path.len() as i32 * STEP_AP;
        }
        commands.entity(npc.entity).insert(crate::Walking { steps: path.into(), progress: 0.0 });
        return true;
    }
    attack_or_approach(commands, w, all, npc, &target)
}

/// `rules/ai_params.mes`.
pub const AI_PARAMS: &str = "rules/ai_params.mes";

/// `npc_flags`: kill on sight (0x4ae120), and its "made peaceful" bit.
pub const ONF_KOS: i32 = 0x100;
pub const ONF_KOS_OVERRIDE: i32 = 0x800;
/// How often NPCs look for foes, in ticks (builder's call: the original on each heartbeat).
pub const NOTICE_TICKS: u64 = 15;

/// Whether an NPC is hostile to a PC (0x4ae120): kill on sight; or its reaction at or below its
/// row's column 10; or their alignments at least column 11 apart.
pub fn hostile(npc_flags: i32, reaction: i32, alignment_gap: i32, row: &[i32; 17]) -> bool {
    (npc_flags & ONF_KOS != 0 && npc_flags & ONF_KOS_OVERRIDE == 0) || reaction <= row[10] || alignment_gap >= row[11]
}

/// How far an NPC of Perception `pe` notices a PC (0x4ab460 / 0x4af260: `clamp(PE, 1, 10)`
/// tiles, Perception first cut by a prowler's value), or hears it (0x4af470).
pub fn notice_reach(pe: i32, prowler: Option<crate::prowl::Prowler>) -> u32 {
    sight_reach(pe, prowler).max(crate::prowl::hearing_reach(pe, prowler) as u32)
}

/// How far a watcher of Perception `pe` sees (0x4af260 → 0x4af240): `clamp(PE, 1, 10)` tiles, its
/// Perception first cut by a prowler's value in percent.
pub fn sight_reach(pe: i32, prowler: Option<crate::prowl::Prowler>) -> u32 {
    let seen = pe - prowler.map_or(0, |p| pe * p.seen_against(pe) / 100);
    seen.clamp(1, 10) as u32
}

/// Whether a watcher at `from` facing `rotation` has `to` in view (0x45eff0): the direction to it
/// within two rotations of its facing (5 of the 8; behind it, 3, not).
pub fn in_view(rotation: usize, from: TileLoc, to: TileLoc) -> bool {
    let step = TileLoc { x: (i64::from(from.x) + (i64::from(to.x) - i64::from(from.x)).signum()) as u32, y: (i64::from(from.y) + (i64::from(to.y) - i64::from(from.y)).signum()) as u32 };
    let Some(dir) = crate::rotation_of_step(from, step) else { return true };
    matches!((dir + 8 - rotation % 8) % 8, 0 | 1 | 2 | 6 | 7)
}

/// The tiles on the straight line between `from` and `to`, both left out.
pub fn line_between(from: TileLoc, to: TileLoc) -> Vec<TileLoc> {
    let (x0, y0, x1, y1) = (i64::from(from.x), i64::from(from.y), i64::from(to.x), i64::from(to.y));
    let n = (x1 - x0).abs().max((y1 - y0).abs());
    (1..n).map(|i| TileLoc { x: (x0 + (x1 - x0) * i / n) as u32, y: (y0 + (y1 - y0) * i / n) as u32 }).collect()
}

/// Whether a watcher sees `to` (0x4af260 returning 0): in reach ([`sight_reach`]), in view
/// ([`in_view`]) and with nothing between that stops a missile (0x4ade00 with the missile's
/// flags 0xc at 0x4ade93: a blocker adds 1 past the reach, 0x4af45a, so it hides even a near
/// target; its line cost is dropped, 0x4af45e). `map` is the missiles' view
/// ([`crate::MissileView`]); critters on the line are left out (all shoot-through in the data).
pub fn sees(map: &impl crate::Walkable, from: TileLoc, rotation: usize, pe: i32, to: TileLoc, prowler: Option<crate::prowl::Prowler>) -> bool {
    // A prowler's cover on the line (0x4ade00 at 0x4c8bc6): its shoot-through objects' cost.
    let cover = prowler.map_or(0, |_| line_between(from, to).into_iter().map(|t| map.line_cost(t)).sum());
    crate::tile_distance(from, to) <= sight_reach(pe, prowler.map(|p| p.with_cover(cover))) && in_view(rotation, from, to) && first_on_line(from, to, std::iter::empty(), |a, b| crate::crossing_clear(map, a, b), |t| map.walkable(t)).is_none()
}

/// Whether a watcher notices `to`: sees it (when `sight`: [`can_look`]), or hears it (0x4af470:
/// within its hearing reach less what the sound pays on its way, [`crate::sound_cost`] at
/// 0x4af60c).
/// `eyes` and `ears` are the Perception it sees ([`can_look`]) and hears ([`ears`]) with; `None`:
/// not at all.
pub fn notices(map: &impl crate::Walkable, eyes: Option<i32>, from: TileLoc, rotation: usize, ears: Option<i32>, to: TileLoc, prowler: Option<crate::prowl::Prowler>) -> bool {
    eyes.is_some_and(|pe| sees(map, from, rotation, pe, to, prowler)) || ears.is_some_and(|pe| hears(map, from, pe, to, prowler))
}

/// `critter_flags` 0x1000: asleep (0x4af518), hearing with half its Perception.
pub const OCF_SLEEPING: i32 = 0x1000;
/// `critter_flags` 0x20: stunned (0x4af49b, 0x4c8684).
pub const OCF_STUNNED: i32 = 0x20;

/// The Perception NPC `listener` hears with (0x4af470): none when stunned (`critter_flags`
/// 0x20 at 0x4af49b, or [`Stunned`]) or down (0x45d800 at 0x4af4b1: callers leave the down
/// out); half asleep ([`OCF_SLEEPING`], 0x4af518-0x4af526, before a prowler's cut).
pub fn ears(listener: &Object, prototypes: &Prototypes, stunned: bool) -> Option<i32> {
    let flags = prototypes.int(listener, "critter_flags").unwrap_or(0);
    let pe = npc_stat(listener, prototypes, 5);
    if stunned || flags & OCF_STUNNED != 0 {
        None
    } else if flags & OCF_SLEEPING != 0 {
        Some(pe / 2)
    } else {
        Some(pe)
    }
}

/// Whether a listener at `from` hears `to` (0x4af470): the distance at most its reach
/// ([`crate::prowl::hearing_reach`]) less the sound's cost on the line (0x4af640).
pub fn hears(map: &impl crate::Walkable, from: TileLoc, pe: i32, to: TileLoc, prowler: Option<crate::prowl::Prowler>) -> bool {
    hears_noise(map, from, pe, to, prowler, crate::prowl::NOISE_STEP)
}

/// [`hears`] for a sound of `noise` ([`crate::prowl::noise_reach`]).
pub fn hears_noise(map: &impl crate::Walkable, from: TileLoc, pe: i32, to: TileLoc, prowler: Option<crate::prowl::Prowler>, noise: usize) -> bool {
    let reach = crate::prowl::noise_reach(pe, prowler, noise);
    let distance = i64::from(crate::tile_distance(from, to));
    distance <= reach && distance <= reach - i64::from(crate::sound_cost(map, from, to))
}

/// `spell_flags` 0x8000000: a follower bound to its leader (0x459040's table): it always
/// fights for it and never leaves.
pub const OSF_BOUND: i32 = 0x0800_0000;
/// The reaction a follower refusing its leader's fight costs (0x4a9d26: −5).
pub const REFUSAL_REACTION: i32 = -5;

/// `critter_flags` of a watcher that sees nothing (0x4af287: 0x10a0, stunned 0x20 and blinded
/// 0x80 among them).
pub const OCF_SIGHTLESS: i32 = 0x10a0;
/// `flags` (field 0x13) 0x200 and `spell_flags` (0x14) 0x1: invisible (0x4af2d6 / 0x4af2e7).
pub const OF_INVISIBLE: i32 = 0x200;
pub const OSF_INVISIBLE: i32 = 0x1;
/// `spell_flags` 0x200 of a watcher that sees the invisible (0x4af307).
pub const OSF_SEE_INVISIBLE: i32 = 0x200;

/// Whether NPC `watcher` can look at all at a target (0x4af260 before the cone and reach): not
/// sightless ([`OCF_SIGHTLESS`]) or stunned; and an invisible target ([`OF_INVISIBLE`],
/// [`OSF_INVISIBLE`]; `None`, a PC: never, builder's call) only with Perception 20 (0x4b0ee0
/// with stat 5: at least 20) or [`OSF_SEE_INVISIBLE`].
pub fn can_look(watcher: &Object, prototypes: &Prototypes, stunned: bool, target: Option<&Object>) -> bool {
    let int = |o: &Object, f: &str| prototypes.int(o, f).unwrap_or(0);
    if stunned || int(watcher, "critter_flags") & OCF_SIGHTLESS != 0 {
        return false;
    }
    let invisible = target.is_some_and(|t| int(t, "flags") & OF_INVISIBLE != 0 || int(t, "spell_flags") & OSF_INVISIBLE != 0);
    !invisible || npc_stat(watcher, prototypes, 5) >= 20 || int(watcher, "spell_flags") & OSF_SEE_INVISIBLE != 0
}

/// A critter's facing: its last step's, else its art's rotation (bits 11-13).
fn facing_of(facing: Option<&crate::Facing>, look: Option<&crate::Appearance>) -> usize {
    facing.map(|f| f.0).or_else(|| look.map(|l| ((l.art.0 >> 11) & 7) as usize)).unwrap_or(4)
}

type FoeQuery<'a> = (&'a NetId, &'a crate::Position, &'a crate::NpcSource, Option<&'a crate::Facing>, Option<&'a crate::Appearance>);
/// NPCs not fighting, following or surrendered.
type AtPeace = (Without<Fighting>, Without<crate::follow::Following>, Without<Surrendered>);
type QuarryQuery<'a> = (&'a NetId, &'a crate::Position, &'a crate::PlayerControlled, &'a crate::Sheet, (Has<crate::Prowling>, Option<&'a crate::Inventory>));

/// NPCs that are not fighting look for PCs they are hostile to within reach (0x4aa0d0 →
/// 0x4ab460 → 0x4ae120); one found starts a fight ([`CombatStarts`]). Followers, the dead and
/// those talking keep still. Sight lines and facing are not counted.
#[allow(clippy::too_many_arguments)]
pub fn notice_foes(
    tick: Res<crate::SimTick>,
    map: Res<crate::LoadedMap>,
    world: Res<crate::WorldState>,
    rules: Res<crate::SimRules>,
    (mut ai, mut reputations, vfs): (ResMut<AiParams>, ResMut<crate::ReputationTable>, Option<Res<crate::SimVfs>>),
    (mut starts, mut checks, clock): (ResMut<CombatStarts>, ResMut<ScriptChecks>, Res<crate::GameClock>),
    npcs: Query<FoeQuery, AtPeace>,
    pcs: Query<QuarryQuery>,
    talking: Query<&crate::Conversation>,
) {
    if !tick.0.is_multiple_of(NOTICE_TICKS) || pcs.is_empty() {
        return;
    }
    let Some(save) = world.0.as_ref() else { return };
    let guard = vfs.as_ref().and_then(|v| v.0.read().ok());
    if ai.0.is_none() {
        let mes = guard.as_deref().and_then(|v| v.read(AI_PARAMS).ok()).and_then(|d| arcanum_formats::mes::MesFile::parse(&d).ok()).unwrap_or_default();
        ai.0 = Some(parse_ai_params(&mes));
    }
    let reputations = &*reputations.0.get_or_insert_with(|| guard.as_deref().and_then(|v| v.read(crate::GAME_REP).ok()).and_then(|d| arcanum_formats::mes::MesFile::parse(&d).ok()).map(|m| crate::ReputationEffects::parse(&m)).unwrap_or_default());
    let rows = ai.0.as_deref().unwrap_or_default();
    let busy: Vec<NetId> = talking.iter().map(|c| c.npc).collect();
    for (&npc, at, source, facing, look) in &npcs {
        let rotation = facing_of(facing, look);
        if map.npc_down(source.sector, source.index) || busy.contains(&npc) || map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)).is_some_and(|o| map.prototypes.int(o, "critter_flags").unwrap_or(0) & OCF_HELD != 0) {
            continue;
        }
        let Some(o) = map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)) else { continue };
        let Some(row) = rows.get(map.prototypes.int(o, "npc_ai_data").unwrap_or(0).max(0) as usize) else { continue };
        let flags = map.prototypes.int(o, "npc_flags").unwrap_or(0);
        let pe = npc_stat(o, &map.prototypes, 5);
        let reaction = crate::NpcReaction::of(o, &map.prototypes);
        let key = crate::met_key(source.object());
        let alignment = npc_stat(o, &map.prototypes, 0x13);
        for (&pc, pc_at, player, sheet, (prowling, inventory)) in &pcs {
            let Some(saved) = save.players.get(&player.0) else { continue };
            let prowler = prowling.then(|| crate::prowl::Prowler::of(&rules.0, &sheet.0, &map, pc_at.0, clock.now.ms / crate::HOUR_MS, inventory.map_or(0, |i| i.borne_light(&map.prototypes)), inventory).for_watcher(npc_dark_sight(o, &map.prototypes)).watched_by(npc_watcher_helpless(o, &map.prototypes)));
            if !notices(&crate::MissileView(&map), can_look(o, &map.prototypes, false, None).then_some(pe), at.0, rotation, ears(o, &map.prototypes, false), pc_at.0, prowler) {
                continue;
            }
            let towards = crate::reaction_to(&saved.script_state, &key, &reaction, &rules.0, &sheet.0, reputations);
            let gap = (alignment - sheet.0.alignment).abs();
            if hostile(flags, towards, gap, row) {
                // Its kill-on-sight script has the last word (0x4ae179).
                if map.attachment(source.object(), EVENT_KILL_ON_SIGHT).number != 0 {
                    checks.hostile.push((npc, pc, source.object(), player.0));
                } else {
                    starts.0.push((npc, pc));
                }
                break;
            }
        }
    }
}

/// How far from the lock witnesses are looked for (0x4aee50 → 0x4ae4e0 with 10).
pub const WITNESS_TILES: u32 = 10;
/// `npc_social_class` (field 0x128) of guards: they watch every lock (0x45d010 == 5).
pub const SOCIAL_CLASS_GUARD: i32 = 5;
/// The reaction a failed pick costs with a witness (0x4af00f: −20, through 0x4c0de0).
pub const WITNESS_REACTION: i32 = -20;

/// Witnesses of a pick on an owned lock (0x4aee50): NPCs within [`WITNESS_TILES`] of the lock,
/// standing, at peace, of the owner's `name` number or guards, who see the PC (0x4af260; the
/// hearing check's reach is 0 for these calls, table 0x5b50c0 index 0). One with a "catching
/// thief" script (event 11, 0x4aefcd: triggerer the PC, attachee the witness) is asked first
/// ([`ScriptChecks::witnesses`], answered the next tick): "skip" leaves it out (0x4aefd5). A
/// lock opened: they fight the PC (0x4a9650 at once, `softFlag` 0). A failed pick: each likes
/// the PC 20 less (0x4af017) and fights when its reaction is then at or under its row's column
/// 10 (0x4af046, dword 10 of the 0x4aaa60 copy), then barks a `gd_npc` 4000 line ("Stop
/// that!": 0x414130 → 0x418780 lines 4000-4099, shown through 0x5f8488 = 0x568430) when it can
/// act (0x45d790). Not built: 0x45d870's skip, the PC's followers' picks counting for their
/// leader (0x45dda0), the dialogue override's lines.
#[allow(clippy::too_many_arguments)]
pub fn catch_lock_pickers(
    mut witnessing: ResMut<crate::lockpick::LockWitnessing>,
    map: Res<crate::LoadedMap>,
    mut world: ResMut<crate::WorldState>,
    rules: Res<crate::SimRules>,
    (mut ai, mut reputations, vfs): (ResMut<AiParams>, ResMut<crate::ReputationTable>, Option<Res<crate::SimVfs>>),
    (mut starts, mut checks): (ResMut<CombatStarts>, ResMut<ScriptChecks>),
    (mut generic, mut rng, session, mut outbox): (ResMut<crate::GenericLines>, ResMut<HostRng>, Res<crate::Session>, ResMut<crate::Outbox>),
    clock: Res<crate::GameClock>,
    npcs: Query<FoeQuery, AtPeace>,
    pcs: Query<QuarryQuery>,
) {
    if witnessing.0.is_empty() && checks.witnessed.is_empty() {
        return;
    }
    // Who reacts: the scripts' answers from last tick, then this tick's witnesses.
    let mut reacting: Vec<(NetId, NetId, PlayerId, bool)> = std::mem::take(&mut checks.witnessed);
    for (player, lock, owner, opened) in std::mem::take(&mut witnessing.0) {
        let Some((&pc, pc_at, _, sheet, (prowling, inventory))) = pcs.iter().find(|p| p.2.0 == player) else { continue };
        let prowler = prowling.then(|| crate::prowl::Prowler::of(&rules.0, &sheet.0, &map, pc_at.0, clock.now.ms / crate::HOUR_MS, inventory.map_or(0, |i| i.borne_light(&map.prototypes)), inventory));
        for (&npc, at, source, facing, look) in &npcs {
            if !crate::within_radius(at.0, lock, WITNESS_TILES) || map.npc_down(source.sector, source.index) || map.is_dead(source.sector, source.index) {
                continue;
            }
            let Some(o) = map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)) else { continue };
            let int = |f: &str| map.prototypes.int(o, f).unwrap_or(0);
            if int("name") != owner && int("npc_social_class") != SOCIAL_CLASS_GUARD {
                continue;
            }
            if !can_look(o, &map.prototypes, false, None) || !sees(&crate::MissileView(&map), at.0, facing_of(facing, look), npc_stat(o, &map.prototypes, 5), pc_at.0, prowler.map(|p| p.for_watcher(npc_dark_sight(o, &map.prototypes)).watched_by(npc_watcher_helpless(o, &map.prototypes)))) {
                continue;
            }
            if map.attachment(source.object(), crate::pickpocket::EVENT_CATCHING_THIEF).number != 0 {
                checks.witnesses.push((npc, pc, source.object(), player, opened));
            } else {
                reacting.push((npc, pc, player, opened));
            }
        }
    }
    if reacting.is_empty() {
        return;
    }
    let guard = vfs.as_ref().and_then(|v| v.0.read().ok());
    if ai.0.is_none() {
        let mes = guard.as_deref().and_then(|v| v.read(AI_PARAMS).ok()).and_then(|d| arcanum_formats::mes::MesFile::parse(&d).ok()).unwrap_or_default();
        ai.0 = Some(parse_ai_params(&mes));
    }
    let reputations = &*reputations.0.get_or_insert_with(|| guard.as_deref().and_then(|v| v.read(crate::GAME_REP).ok()).and_then(|d| arcanum_formats::mes::MesFile::parse(&d).ok()).map(|m| crate::ReputationEffects::parse(&m)).unwrap_or_default());
    let rows = ai.0.as_deref().unwrap_or_default();
    for (npc, pc, player, opened) in reacting {
        if opened {
            starts.0.push((npc, pc));
            continue;
        }
        let Some((_, _, source, _, _)) = npcs.iter().find(|n| *n.0 == npc) else { continue };
        let Some(o) = map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)) else { continue };
        let Some(sheet) = pcs.iter().find(|p| *p.0 == pc).map(|p| p.3.0.clone()) else { continue };
        let Some(saved) = world.0.as_mut().and_then(|w| w.players.get_mut(&player)) else { continue };
        let reaction = crate::NpcReaction::of(o, &map.prototypes);
        let key = crate::met_key(source.object());
        let stored = saved.script_state.reactions.get(&key).copied().unwrap_or(reaction.base);
        saved.script_state.reactions.insert(key.clone(), stored + WITNESS_REACTION);
        let towards = crate::reaction_to(&saved.script_state, &key, &reaction, &rules.0, &sheet, reputations);
        if rows.get(map.prototypes.int(o, "npc_ai_data").unwrap_or(0).max(0) as usize).is_some_and(|row| towards <= row[10]) {
            starts.0.push((npc, pc));
        }
        // The bark, fight or not (0x4af06f).
        let info = crate::npc_details(&map, source.object(), &mut generic, guard.as_deref());
        let who = crate::Speaker { npc: &info, pc_name: &sheet.name, pc_female: sheet.gender == arcanum_rules::FEMALE, pc_race: sheet.race };
        if let Some((text, voice)) = crate::npc_line(&mut generic, guard.as_deref(), &mut rng, &who, crate::Block::Npc, WITNESS_BARK) {
            crate::ground::broadcast(&session, &mut outbox, LedgerDelta::FloatText { speaker: npc, text, colour: crate::FloatColour::White, voice });
        }
    }
}

/// A PC's critical hit, one of its followers may remark on (0x4b374b → 0x4b6b90 → 0x4ae9e0):
/// a hit (flag 0x2, 0x4b6bee) and critical (0x4, 0x4b6c0b) on the target it meant, not on one of
/// its own followers (0x4b6be4); half the time (0x4ae9fb: random(1, 2) == 1 says nothing) a
/// follower picked at random (0x4aea3e) that can act (0x45d790) floats a `gd_npc` 2400 line
/// (0x413f50 at 0x4aea84). Not built: the 2500 lines for a hit on another target than the one
/// meant (flag 0x8, 0x4b6c29: no stray hits here).
fn praise_critical(w: &mut CombatWorld, all: &[Seen], attacker: &Seen, victim: &Seen, outcome: &AttackOutcome) {
    let Some(player) = attacker.player else { return };
    if outcome.hit && outcome.critical && victim.leader != Some(player) {
        follower_remark(w, all, player, CRITICAL_BARK);
    }
}

/// A PC's blow that hit another than its target (0x4b6b90's flag 0x8 at 0x4b6c29: a hit, flag 0x2,
/// on a critter other than the one meant, not one of its own followers, 0x4b6be4): one of its
/// followers remarks on it (0x4ae9e0 with mode 1, `gd_npc` 2500-2599 at 0x413fce).
fn stray_remark(w: &mut CombatWorld, all: &[Seen], attacker: &Seen, meant: &Seen, outcome: &AttackOutcome) {
    if let Some(player) = attacker.player.filter(|&p| stray_remarked(outcome.hit, meant.leader, p)) {
        follower_remark(w, all, player, STRAY_BARK);
    }
}

/// Whether PC `player`'s stray blow draws a remark: it hit, and the critter meant is not its
/// own follower (0x4b6be4).
pub fn stray_remarked(hit: bool, meant_leader: Option<PlayerId>, player: PlayerId) -> bool {
    hit && meant_leader != Some(player)
}

/// Half the time (0x4ae9fb) one of `player`'s followers picked at random (0x4aea3e), not joined
/// by force ([`crate::follow::FORCED`]: 0x45e3f0 counts them out at 0x45e43b), that can act (0x45d790) floats a `gd_npc` line of
/// block `base` (0x413f50 → 0x4aea97).
fn follower_remark(w: &mut CombatWorld, all: &[Seen], player: PlayerId, base: u32) {
    if w.rng.range(1, 2) == 1 {
        return;
    }
    let followers: Vec<&Seen> = all.iter().filter(|c| c.leader == Some(player) && c.npc.is_some_and(|n| w.npc_int(n, "npc_flags") & crate::follow::FORCED == 0)).collect();
    if followers.is_empty() {
        return;
    }
    let pick = followers[if followers.len() > 1 { w.rng.range(0, followers.len() as i32 - 1) as usize } else { 0 }];
    let Some(source) = pick.npc else { return };
    if pick.stunned.is_some() || w.map.npc_down(source.sector, source.index) || w.map.is_dead(source.sector, source.index) {
        return;
    }
    w.checked_bark(pick, player, base);
}

/// Why follower `npc` would leave leader `pc` (0x4ad950 with `p5` 1): 0 it stays; 3 its
/// reaction at or under its row's column 5 (0x4ada57); 1 the leader's alignment above its own
/// by more than column 6 (0x4ada95), 2 below by more than column 7 (0x4adaac); 7 its level at
/// least the leader's + column 8 (0x4adae6). It stays when bound (`spell_flags` 0x8000000), when
/// the leader is a Master of Persuasion (0x4c60c0 skill 0xb rank 3, 0x4ad9c9) or when it joined
/// by force (`npc_flags` 0x400, 0x4ada32).
pub fn leave_reason(row: &[i32; 17], reaction: i32, alignment: (i32, i32), level: (i32, i32), stays: bool) -> u8 {
    let ((npc_align, pc_align), (npc_level, pc_level)) = (alignment, level);
    if stays {
        0
    } else if reaction <= row[5] {
        3
    } else if pc_align > npc_align && pc_align - npc_align > row[6] {
        1
    } else if npc_align > pc_align && npc_align - pc_align > row[7] {
        2
    } else if pc_level + row[8] <= npc_level {
        7
    } else {
        0
    }
}

/// How much a follower's reaction to its leader changes when the leader's blow strikes it
/// (0x4a9650): its row's column 9 (0x4a996b); nothing for a stray hit (the owner's rule, as the
/// original: the indirect flag jumps past 0x4c0de0 at 0x4a9969), nor a loss to a leader of
/// Charisma 20 or more (0x4c0e8e).
pub fn leader_hit_reaction(column_9: i32, charisma: i32, aim: Aim) -> Option<i32> {
    if aim == Aim::Stray || (column_9 < 0 && charisma >= 20) {
        return None;
    }
    Some(column_9)
}

/// A PC hit its own follower (0x4a9650, the victim's leader the attacker, 0x4a9951): it likes
/// the PC by its row's column 9 (0x4a996b → 0x4c0de0; negative in the data); then, with a
/// [`leave_reason`], it leaves the party (0x45dfc0) and says why (0x413a90 at 0x4a99f3: `gd_npc`
/// 1600 / 1700 / 1800 for 1-3, 2900 for 7), else one time in three it complains (0x413df0 at
/// 0x4a9a4d: `gd_npc` 2300); staying, it warns at its next check ([`crate::follow::warning`]).
fn leader_hits_follower(commands: &mut Commands, w: &mut CombatWorld, player: PlayerId, source: crate::NpcSource, victim: &Seen, aim: Aim) {
    let Some(row) = w.ai_row(source) else { return };
    let Some(sheet) = w.world.0.as_ref().and_then(|s| s.players.get(&player)).map(|s| s.blueprint.sheet.clone()) else { return };
    // 0x4c0de0: a leader of Charisma 20 or more loses nothing by its follower (0x4c0e8e:
    // 0x4b0ee0 with stat 7); else a loss makes the follower warn at its next check (`npc_flags`
    // 0x10000 with 0x1000000, 0x4c0f06 / 0x4c0f18: [`crate::follow::WARN_REACTION`]).
    if let Some(change) = leader_hit_reaction(row[9], w.rules.0.stats(&sheet)[7], aim) {
        w.change_reaction(source, player, change);
        if change < 0 {
            let home = w.map.home_of(source.sector, source.index);
            if let Some(f) = w.world.0.as_mut().and_then(|s| s.followers.iter_mut().find(|f| f.is(&home))) {
                f.warn |= crate::follow::WARN_REACTION;
            }
        }
    }
    let Some(reaction) = w.reaction(source, player) else { return };
    // Persuasion is basic skill 11 (0xb); training 3 is Master.
    let stays = w.npc_int(source, "spell_flags") & OSF_BOUND != 0 || sheet.training[11] >= 3 || w.npc_int(source, "npc_flags") & crate::follow::FORCED != 0;
    let code = leave_reason(&row, reaction, (w.npc_int_stat(source, 0x13), sheet.alignment), (w.npc_int_stat(source, STAT_LEVEL), sheet.level), stays);
    let line = match code {
        0 => {
            if w.rng.range(1, 3) == 1 {
                w.bark(source, victim.net_id, player, 2300);
            }
            return;
        }
        1 => 1600,
        2 => 1700,
        3 => 1800,
        _ => 2900,
    };
    let left = match w.world.0.as_mut() {
        Some(save) => crate::follow::dismiss(&mut w.map, save, source.sector, source.index),
        None => false,
    };
    if left {
        commands.entity(victim.entity).remove::<crate::follow::Following>();
        w.bark(source, victim.net_id, player, line);
    }
}

/// A follower's remark on its leader's critical hit: `gd_npc` 2400-2499 (0x413f50, arg 0).
pub const CRITICAL_BARK: u32 = 2400;

/// Under this percent of its hit points a PC's follower calls to it (0x4b50d3 / 0x4b50e4: 0x14).
pub const LOW_HEALTH_PERCENT: i32 = 20;
/// Its words: `gd_npc` 5500-5599 (0x414490 at 0x4aeae4).
pub const LOW_HEALTH_BARK: u32 = 5500;

/// `critter_flags` 0x2000 of an NPC that says nothing (0x4ad84b, with 0x20 and 0x40).
pub const OCF_MUTE: i32 = 0x2000;

/// Whether an NPC may not bark at PC `to` (0x4ad800 with mode 1, a nonzero reason): destroyed
/// or off (`flags` 0x3, 0x4ad817); stunned, paralyzed or mute (`critter_flags` 0x2060,
/// 0x4ad84b) or asleep (0x1000, 0x4ad85b); `spell_flags` 0x10000 (0x4ad890); dead or down
/// (`gone`: 0x45d8d0 / 0x45d800); or another's follower (0x4c1110 at 0x4ad923-0x4ad937). Not
/// built: condition 0x3a (0x459380), the dead's `spell_flags` 0x2000000 exception.
pub fn bark_vetoed(flags: i32, critter_flags: i32, spell_flags: i32, gone: bool, leader: Option<PlayerId>, to: PlayerId) -> bool {
    flags & (crate::blocking::OF_DESTROYED | crate::blocking::OF_OFF) != 0
        || critter_flags & (OCF_STUNNED | OCF_PARALYZED | OCF_MUTE | OCF_SLEEPING) != 0
        || spell_flags & SPELL_NO_DAMAGE != 0
        || gone
        || leader.is_some_and(|l| l != to)
}

/// The remark on a PC's hit on another than meant: `gd_npc` lines 2500-2599 (0x413fce: 0x9c4-0xa27).
pub const STRAY_BARK: u32 = 2500;

/// The flee and surrender bark: `gd_npc` lines 3000-3099 (0x413f13 / 0x413f1c: 0xbb8-0xc1b).
pub const FLEE_BARK: u32 = 3000;

/// The witness's bark: `gd_npc` lines 4000-4099 (0x4141b3: 0xfa0-0x1003).
pub const WITNESS_BARK: u32 = 4000;

/// A tile up to `tiles` away from `from`, straight on past `at`.
fn flee_goal(at: TileLoc, from: TileLoc, tiles: u32) -> TileLoc {
    let dx = i64::from(at.x) - i64::from(from.x);
    let dy = i64::from(at.y) - i64::from(from.y);
    let (sx, sy) = (dx.signum(), dy.signum());
    let (sx, sy) = if (sx, sy) == (0, 0) { (1, 0) } else { (sx, sy) };
    let x = (i64::from(at.x) + sx * i64::from(tiles)).max(0) as u32;
    let y = (i64::from(at.y) + sy * i64::from(tiles)).max(0) as u32;
    TileLoc { x, y }
}

/// The level stat (0x11).
pub const STAT_LEVEL: usize = 0x11;

/// A path from `at` away from `from` that ends at least `keep` tiles from it (0x4ac7b0), cut to
/// the turn's points; none if it cannot get farther.
fn back_off(w: &CombatWorld, at: TileLoc, from: TileLoc, keep: u32) -> Option<Vec<TileLoc>> {
    let ap = w.combat.0.as_ref().map_or(0, |e| e.ap);
    let mut path = crate::find_path(&*w.map, at, flee_goal(at, from, keep)).ok()?;
    if let Some(i) = path.iter().position(|&t| reach_between(t, from) >= keep) {
        path.truncate(i + 1);
    }
    path.truncate((ap / STEP_AP).max(0) as usize);
    (!path.is_empty()).then_some(path)
}

/// How often NPCs' health is compared, in ticks.
pub const HEALTH_TICKS: u64 = 5;

/// Tells every player an NPC's hit points and fatigue when they change from what was last told
/// (or, at first sight, are not full): the hover's gauges show the host's numbers. A player new
/// in the world first learns every NPC told so far that is not whole.
pub fn send_npc_health(tick: Res<crate::SimTick>, map: Res<crate::LoadedMap>, session: Res<crate::Session>, mut outbox: ResMut<crate::Outbox>, npcs: Query<(&NetId, &crate::NpcSource)>, mut told: Local<std::collections::HashMap<NetId, (crate::Pool, crate::Pool)>>, mut players: Local<std::collections::HashSet<crate::PlayerId>>) {
    if !tick.0.is_multiple_of(HEALTH_TICKS) {
        return;
    }
    let here: std::collections::HashSet<crate::PlayerId> = session.players.iter().filter(|p| matches!(p.1, crate::SessionPlayer::InWorld(_))).map(|p| *p.0).collect();
    let mut known: Vec<(&NetId, &(crate::Pool, crate::Pool))> = told.iter().filter(|t| t.1.0.current < t.1.0.max || t.1.1.current < t.1.1.max).collect();
    known.sort_by_key(|t| *t.0);
    for &player in here.difference(&players) {
        for &(&net_id, &(hit_points, fatigue)) in &known {
            outbox.0.push((player, LedgerDelta::NpcHealth { net_id, hit_points, fatigue }));
        }
    }
    *players = here;
    for (&net_id, source) in &npcs {
        let (Some(hp), Some(fatigue)) = (map.npc_hp(source.sector, source.index), map.npc_fatigue(source.sector, source.index)) else { continue };
        let now = (crate::Pool { current: hp.0, max: hp.1 }, crate::Pool { current: fatigue.0, max: fatigue.1 });
        let before = told.insert(net_id, now);
        let full = now.0.current >= now.0.max && now.1.current >= now.1.max;
        if before == Some(now) || (before.is_none() && full) {
            continue;
        }
        crate::ground::broadcast(&session, &mut outbox, LedgerDelta::NpcHealth { net_id, hit_points: now.0, fatigue: now.1 });
    }
}

/// Walks NPCs at their own speed ([`npc_speed`]: Dexterity, 5 less with crippled legs,
/// 0x4b052a): every [`HEALTH_TICKS`] an NPC whose speed changed (first at its spawn, or a
/// crippling critical) gets its gait again, and every player is told (`LedgerDelta::Speed`).
pub fn update_npc_paces(tick: Res<crate::SimTick>, map: Res<crate::LoadedMap>, session: Res<crate::Session>, mut outbox: ResMut<crate::Outbox>, vfs: Option<Res<crate::SimVfs>>, mut npcs: Query<(&NetId, &crate::NpcSource, &mut crate::Appearance, &mut crate::gait::Gait)>) {
    if !tick.0.is_multiple_of(HEALTH_TICKS) {
        return;
    }
    let guard = vfs.as_ref().and_then(|v| v.0.read().ok());
    for (&net_id, source, mut look, mut gait) in &mut npcs {
        let Some(o) = map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)) else { continue };
        let speed = npc_speed(o, &map.prototypes);
        if speed == look.speed {
            continue;
        }
        look.speed = speed;
        *gait = crate::gait::Gait::load(look.art, speed, |p| guard.as_deref().and_then(|v| v.read(p).ok()));
        crate::ground::broadcast(&session, &mut outbox, LedgerDelta::Speed { net_id, speed });
    }
}

type WeaponArtQuery<'a> = (&'a NetId, &'a mut crate::Appearance, Option<&'a crate::Inventory>, Option<&'a crate::NpcSource>, Has<Fighting>);

/// The shield slot (0x465090: slot 0x3ed, an armour item, sets art bit 19).
pub const SHIELD_SLOT: i32 = 1005;

/// The weapon and shield a critter's art shows (0x465020): in combat mode (0x4b3d50:
/// `critter_flags` 0x40000; here in a fight), its wielded weapon's kind (`item_use_aid_fragment`
/// bits 6-9, 0x5044f0; 1 bare-handed or a kind 0, 0x465074) and a shield when slot 1005 holds
/// armour (0x46509d); out of it 0 and none.
pub fn armed_art(fighting: bool, inventory: Option<&crate::Inventory>, prototypes: &Prototypes) -> (u32, bool) {
    if !fighting {
        return (0, false);
    }
    let item = |slot: i32| inventory.and_then(|inv| inv.items.iter().find(|i| i.location == slot)).and_then(|i| prototypes.get(i.prototype));
    let kind = item(WEAPON_SLOT).filter(|w| w.object_type == arcanum_formats::object::TYPE_WEAPON).map_or(0, |w| weapon_of(w, prototypes).use_kind);
    let shield = item(SHIELD_SLOT).is_some_and(|o| o.object_type == arcanum_formats::object::TYPE_ARMOR);
    (if kind == 0 { 1 } else { kind }, shield)
}

/// Sets the weapon and shield bits of a critter's art ([`armed_art`]) as it enters or leaves a
/// fight (an NPC fighting, or anyone in the fight's order; the original's combat mode toggle),
/// and while it fights when its arms change; every player is told (`LedgerDelta::Armed`). A
/// critter never in a fight keeps the art it was placed with (0x465020 runs only on wielding
/// and combat mode).
pub fn update_weapon_art(map: Res<crate::LoadedMap>, combat: Res<Combat>, session: Res<crate::Session>, mut outbox: ResMut<crate::Outbox>, mut fought: Local<std::collections::HashSet<NetId>>, mut critters: Query<WeaponArtQuery>) {
    let order: &[NetId] = combat.0.as_ref().map_or(&[], |e| &e.order);
    for (&net_id, mut look, inventory, source, fighting) in &mut critters {
        let now = fighting || order.contains(&net_id);
        if !now && !fought.remove(&net_id) {
            continue;
        }
        if now {
            fought.insert(net_id);
        }
        let npc_inventory = source.and_then(|s| map.npc_inventory(s.sector, s.index));
        let inventory = inventory.or(npc_inventory.as_ref());
        let (weapon, shield) = armed_art(now, inventory, &map.prototypes);
        let art = arcanum_formats::art_id::CritterArt::from_id(look.art);
        if look.art.art_type() != 2 || (art.weapon == weapon && art.flag_s == shield) {
            continue;
        }
        look.art = arcanum_formats::art_id::CritterArt { weapon, flag_s: shield, ..art }.id();
        crate::ground::broadcast(&session, &mut outbox, LedgerDelta::Armed { net_id, weapon, shield });
    }
}

/// NPCs' fatigue recovers as PCs' does ([`crate::magick::recover_fatigue`], 0x45e610): every
/// [`crate::RECOVERY_MS`] of game time by their heal rate `(CO + 1) / 3`; one back above 0 gets
/// up (0x434de0).
pub fn recover_npc_fatigue(clock: Res<crate::GameClock>, mut last: Local<Option<u64>>, mut map: ResMut<crate::LoadedMap>, mut world: ResMut<crate::WorldState>, mut toggled: ResMut<crate::world::ToggledNpcs>, vfs: Option<Res<crate::SimVfs>>) {
    let now = clock.now.days as u64 * crate::DAY_MS as u64 + clock.now.ms as u64;
    let from = *last.get_or_insert(now);
    let steps = now.saturating_sub(from) / crate::RECOVERY_MS;
    if steps == 0 {
        return;
    }
    *last = Some(from + steps * crate::RECOVERY_MS);
    let tired: Vec<(arcanum_formats::map::SectorId, u32)> = map.changes.iter().flat_map(|(&s, c)| c.npc_fatigue.iter().filter(|(_, d)| **d > 0).map(move |(&i, _)| (s, i))).collect();
    let guard = vfs.as_ref().and_then(|v| v.0.read().ok());
    for (sector, index) in tired {
        if map.is_dead(sector, index) {
            continue;
        }
        let Some((left, most)) = map.npc_fatigue(sector, index) else { continue };
        let co = map.mobiles.get(&sector).and_then(|m| m.get(index as usize)).map_or(0, |o| npc_stat(o, &map.prototypes, 2));
        let rate = ((co + 1) / 3).max(1);
        let damage = crate::recovered(most - left, rate, steps);
        map.set_npc_fatigue_damage(sector, index, damage);
        if left < 1 && most - damage >= 1 {
            map.lay_npc_down(guard.as_deref(), sector, index, false);
            toggled.0.push((sector, index));
        }
        crate::ground::save_change(&mut world, &map, sector);
    }
}

/// Surrendered NPCs take heart (0x4ac660, 0x4a945a): when whom they gave up to is gone, dead or
/// down, or at [`RECOVER_PERCENT`] hit points or more one tick in [`RECOVER_ODDS`].
pub fn recover_surrendered(mut commands: Commands, map: Res<crate::LoadedMap>, world: Res<crate::WorldState>, rules: Res<crate::SimRules>, mut rng: ResMut<HostRng>, npcs: Query<(Entity, &Surrendered, &crate::NpcSource)>, critters: Query<(&NetId, Option<&crate::PlayerControlled>, Option<&crate::NpcSource>)>) {
    for (entity, s, source) in &npcs {
        let to = critters.iter().find(|c| *c.0 == s.to);
        let gone = match to {
            None => true,
            Some((_, Some(pc), _)) => world.0.as_ref().and_then(|w| w.players.get(&pc.0)).is_none_or(|p| p.blueprint.sheet.hp_damage >= rules.0.derived(&p.blueprint.sheet).hit_points),
            Some((_, None, Some(n))) => map.npc_down(n.sector, n.index),
            Some(_) => true,
        };
        let heart = map.npc_hp(source.sector, source.index).is_some_and(|(left, max)| max > 0 && left * 100 / max >= RECOVER_PERCENT) && rng.range(1, RECOVER_ODDS) == 1;
        if gone || heart {
            commands.entity(entity).remove::<Surrendered>();
        }
    }
}

/// The eight tiles round `at`.
pub fn neighbours(at: TileLoc) -> Vec<TileLoc> {
    let mut out = Vec::new();
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            if (dx, dy) != (0, 0) {
                let (x, y) = (i64::from(at.x) + dx, i64::from(at.y) + dy);
                if let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) {
                    out.push(TileLoc { x, y });
                }
            }
        }
    }
    out
}

/// One attack and what it does: an NPC victim fights back; its damage, death and the
/// experience; everyone is told.
/// Whom a blow struck: the target meant, by a throw ([`Aim::Thrown`]: the original's thrown
/// attacks skip 0x4a9650, 0x4b3334 with `ebx` 0), or another in the way (a stray throw).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aim {
    Meant,
    Thrown,
    Stray,
}

/// What a missile meets on its way.
enum InTheWay {
    Clear,
    Critter(Box<Seen>),
    /// Stopped while on this tile.
    Stopped(TileLoc),
}

/// What a missile from `attacker` to `victim` meets first in flight (0x4ade00 → 0x43fe00):
/// along the straight line (0x4201c0), each step's crossed edges, then what stands on the next
/// tile, the target's own left out (flag 0x10 on the last step): a living critter not
/// shoot-through (0x440415; the dead let it by), or an object that stops missiles
/// ([`crate::blocking::missile_blocks`]).
fn in_the_way(w: &CombatWorld, all: &[Seen], attacker: &Seen, victim: &Seen) -> InTheWay {
    let shoot_through = |c: &Seen| missile_passes(attacker.player, c.player, c.leader, c.npc.is_some_and(|n| w.npc_int(n, "flags") & crate::blocking::OF_SHOOT_THROUGH != 0));
    let alive = |c: &&Seen| c.net_id != attacker.net_id && c.net_id != victim.net_id && c.npc.is_none_or(|n| !w.map.is_dead(n.sector, n.index)) && !(c.player.is_some() && w.pc_down(c)) && !shoot_through(c);
    let others: Vec<(NetId, TileLoc)> = all.iter().filter(alive).map(|c| (c.net_id, c.at)).collect();
    let view = crate::MissileView(&w.map);
    match first_on_line(attacker.at, victim.at, others.iter().copied(), |a, b| crate::crossing_clear(&view, a, b), |t| crate::Walkable::walkable(&view, t)) {
        Some(Met::Critter(id)) => all.iter().find(|c| c.net_id == id).cloned().map_or(InTheWay::Clear, |c| InTheWay::Critter(Box::new(c))),
        Some(Met::Blocked(at)) => InTheWay::Stopped(at),
        None => InTheWay::Clear,
    }
}

/// Whether a missile passes a critter standing in its way. Every critter prototype of the game
/// is shoot-through (PCs' too), so by the data: NPCs by their own `flags` (only one placed
/// without 0x20 stops it), PCs always. A PC's own shot (owner's rule): its own followers let it
/// by, whatever their flags; other PCs stop it.
pub fn missile_passes(shooter: Option<PlayerId>, player: Option<PlayerId>, leader: Option<PlayerId>, shoot_through: bool) -> bool {
    match (shooter, player) {
        (Some(_), Some(_)) => false,
        (None, Some(_)) => true,
        (Some(me), None) if leader == Some(me) => true,
        _ => shoot_through,
    }
}

/// What a line meets first ([`first_on_line`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Met {
    Critter(NetId),
    /// Stopped while on this tile.
    Blocked(TileLoc),
}

/// What a missile from `from` to `to` meets first: stepping along the straight line, a step
/// whose edges are not `clear`, then on each tile before `to` the first of `others` standing
/// there, or a tile not `open`.
pub fn first_on_line(from: TileLoc, to: TileLoc, others: impl Iterator<Item = (NetId, TileLoc)> + Clone, clear: impl Fn(TileLoc, TileLoc) -> bool, open: impl Fn(TileLoc) -> bool) -> Option<Met> {
    let mut at = from;
    for next in line_between(from, to).into_iter().chain(std::iter::once(to)) {
        if !clear(at, next) {
            return Some(Met::Blocked(at));
        }
        if next == to {
            return None;
        }
        if let Some(o) = others.clone().find(|o| o.1 == next) {
            return Some(Met::Critter(o.0));
        }
        if !open(next) {
            return Some(Met::Blocked(at));
        }
        at = next;
    }
    None
}

/// How much easier `victim` is to hit by `attacker` (0x4c8609-0x4c86dc, one amount): by its
/// state first ([`helpless_by_state`]); else 30 when, not in a fight (0x4b3d50), it neither sees
/// nor hears the attacker (0x4af260 / 0x4af470: a blow from behind).
fn helpless(w: &CombatWorld, attacker: &Seen, victim: &Seen) -> i32 {
    let (down, flags) = match (victim.player, victim.npc) {
        (Some(_), _) => (w.pc_down(victim) || w.pc_out(victim), 0),
        (None, Some(n)) => (w.map.npc_down(n.sector, n.index), w.npc_int(n, "critter_flags")),
        _ => (false, 0),
    };
    if let Some(amount) = helpless_by_state(flags, down, victim.knocked_down, victim.stunned.is_some()) {
        return amount;
    }
    // Not fighting yet: the blow that starts its fight counts (builder's call for when 0x40000
    // is set).
    if let (false, Some(n)) = (victim.fighting, victim.npc)
        && let Some(o) = w.map.mobiles.get(&n.sector).and_then(|m| m.get(n.index as usize))
    {
        let prowler = attacker.sheet.as_ref().filter(|_| attacker.prowling).map(|s| crate::prowl::Prowler::of(&w.rules.0, s, &w.map, attacker.at, w.clock.now.ms / crate::HOUR_MS, attacker.inventory.as_ref().map_or(0, |i| i.borne_light(&w.map.prototypes)), attacker.inventory.as_ref()).for_watcher(npc_dark_sight(o, &w.map.prototypes)).watched_by(npc_watcher_helpless(o, &w.map.prototypes)));
        let target = attacker.npc.and_then(|a| w.map.mobiles.get(&a.sector)?.get(a.index as usize));
        if !notices(&crate::MissileView(&w.map), can_look(o, &w.map.prototypes, victim.stunned.is_some(), target).then(|| npc_stat(o, &w.map.prototypes, 5)), victim.at, victim.rotation, ears(o, &w.map.prototypes, victim.stunned.is_some()), attacker.at, prowler) {
            return HELPLESS_UNAWARE;
        }
    }
    0
}

/// What a missile's way adds to the shot (0x4c8430, ranged skills: 0x4c886f-0x4c890e): 5 a tile
/// past half the shooter's Perception (0x4c888e-0x4c88c1; not for a Master of the skill,
/// 0x4c8883), and the cost of the shoot-through objects on its line (0x4ade00 at 0x4c88d7, added
/// raw: [`crate::blocking::missile_cost`]), the target's tile left out.
pub fn missile_difficulty(distance: u32, perception: i32, master: bool, line_cost: i32) -> i32 {
    let far = if master { 0 } else { (5 * (distance as i32 - perception / 2)).max(0) };
    far + line_cost
}

/// What the living critters (tile, `flags`) standing on `line` cost a shot
/// ([`crate::blocking::cover_cost`] each).
pub fn line_critters_cost(line: &[TileLoc], living: impl Iterator<Item = (TileLoc, i32)>) -> i32 {
    living.filter(|(at, _)| line.contains(at)).map(|(_, flags)| crate::blocking::cover_cost(flags)).sum()
}

/// The `flags` of every PC prototype (13 of 13): shoot-through, seen through, cover.
pub const PC_FLAGS: i32 = 0x4030;

/// A critter lying from a knock-down (goal 62, 0x435a90; animation 7). The goal's handler
/// (0x431b20) queues no get-up: only 0x434de0 does, from the fatigue recovery tick (0x45e610 at
/// 0x45e75e) once fatigue is above 0 and the art lies on animation 7. Here it gets up one
/// recovery period ([`crate::RECOVERY_MS`]) after it fell (builder's call: the original's
/// per-critter tick may come sooner). While it lies it is 30 easier to hit (0x4c867b) and
/// cannot be stunned (0x4b5f40).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnockedDown {
    /// The game time it gets up, set when first seen ([`get_up_knocked_down`]).
    pub until: Option<u64>,
}

type LyingQuery<'a> = (Entity, &'a NetId, &'a mut KnockedDown, Option<&'a mut crate::Appearance>, Has<crate::Prowling>);

/// Art animation 7: knocked down, lying on its last frame.
pub const ANIM_LYING: u32 = 7;

/// Gets up the knocked-down critters whose time has come; every player is told
/// (`LedgerDelta::GotUp`). While one lies its art is animation 7, so players who join meanwhile
/// spawn it lying; up, it is back to standing (or prowling, animation 5).
pub fn get_up_knocked_down(mut commands: Commands, clock: Res<crate::GameClock>, session: Res<crate::Session>, mut outbox: ResMut<crate::Outbox>, mut lying: Query<LyingQuery>) {
    let now = clock.now.days as u64 * crate::DAY_MS as u64 + clock.now.ms as u64;
    for (entity, &net_id, mut down, look, prowling) in &mut lying {
        match down.until {
            None => {
                down.until = Some(now + crate::RECOVERY_MS);
                if let Some(mut look) = look {
                    look.art = crate::gait::with_anim(look.art, ANIM_LYING);
                }
            }
            Some(t) if now >= t => {
                commands.entity(entity).remove::<KnockedDown>();
                if let Some(mut look) = look {
                    look.art = crate::gait::with_anim(look.art, if prowling { crate::prowl::ANIM_PROWL } else { arcanum_formats::art_id::ANIM_STAND });
                }
                crate::ground::broadcast(&session, &mut outbox, LedgerDelta::GotUp { net_id });
            }
            Some(_) => {}
        }
    }
}

/// `critter_flags` 0x40: paralyzed (0x4c8642).
pub const OCF_PARALYZED: i32 = 0x40;
/// `critter_flags` of a critter that cannot act (0x45d790: 0x60, stunned and paralyzed).
pub const OCF_HELD: i32 = OCF_STUNNED | OCF_PARALYZED;

/// The helpless amount a target's state gives, in 0x4c8430's order: paralyzed
/// ([`OCF_PARALYZED`], 0x4c8642) or down (0x45d800 at 0x4c8654) 50; knocked down (animation 7,
/// 0x4c867b) or stunned (0x20 or [`Stunned`], 0x4c8684) 30; asleep ([`OCF_SLEEPING`], 0x4c8688)
/// 50. PCs have no such flags here; knocked down is not kept on the host (always false).
pub fn helpless_by_state(flags: i32, down: bool, knocked_down: bool, stunned: bool) -> Option<i32> {
    if flags & OCF_PARALYZED != 0 || down {
        Some(HELPLESS_DOWN)
    } else if knocked_down || stunned || flags & OCF_STUNNED != 0 {
        Some(HELPLESS_UNAWARE)
    } else if flags & OCF_SLEEPING != 0 {
        Some(HELPLESS_DOWN)
    } else {
        None
    }
}

/// A melee blow's stab (0x4b22da-0x4b232a): none, from behind (0x4000: [`can_backstab`]), or
/// unnoticed besides (0x8000: the target neither sees nor hears the attacker).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Stab {
    #[default]
    None,
    Behind,
    Unnoticed,
}

/// Whether an attacker of Backstab `level` and `rank` wielding a weapon of `use_kind` may stab
/// a target that does not have it in view (0x45f9d0: a level, 0x45eff0 false, a dagger, or from
/// Expert a sword, axe or great sword).
pub fn can_backstab(level: i32, rank: u8, use_kind: Option<u32>, in_view: bool) -> bool {
    level > 0 && !in_view && use_kind.is_some_and(|k| k == KIND_DAGGER || (rank >= 2 && matches!(k, KIND_SWORD | KIND_AXE | KIND_GREAT_SWORD)))
}

/// Wielded-art kinds ([`Weapon::use_kind`]), named from the game's weapon prototypes: 2 the
/// daggers (Dagger, Stiletto, Railroad Spike), 3 swords (Sword, Rapier, Falchion), 4 axes, 7
/// great swords (Claymore, Broadsword, Flamberge); 5 maces and hammers, 1 boomerangs and
/// chakrams, 6 pistols, 8 bows, 10 rifles, 13 staves.
pub const KIND_DAGGER: u32 = 2;
pub const KIND_SWORD: u32 = 3;
pub const KIND_AXE: u32 = 4;
pub const KIND_GREAT_SWORD: u32 = 7;

/// What a stab adds to a hit's normal damage (0x4b6680 at 0x4b67a5-0x4b67d9): the Backstab
/// level from behind, five times it unnoticed; added to the roll before resistance.
pub fn stab_damage(stab: Stab, level: i32) -> i32 {
    match stab {
        Stab::None => 0,
        Stab::Behind => level,
        Stab::Unnoticed => 5 * level,
    }
}

/// `attacker`'s stab at `victim` (0x4b22da: a melee blow, skill 2, at a critter that does not
/// have the attacker in view, [`in_view`] as 0x45eff0).
/// Unnoticed besides (`unnoticed`, 0x4b2327: the target neither sees it nor hears it at noise
/// 0, [`CombatWorld::notices_quietly`]).
fn stab_of(fighter: &Fighter, attacker: &Seen, victim: &Seen, unnoticed: bool) -> Stab {
    let melee = fighter.skill() == SKILL_MELEE;
    let (level, rank) = fighter.backstab;
    if !(melee && can_backstab(level, rank, fighter.weapon.as_ref().map(|w| w.use_kind), in_view(victim.rotation, victim.at, attacker.at))) {
        return Stab::None;
    }
    if unnoticed { Stab::Unnoticed } else { Stab::Behind }
}

/// What an unnoticed stab adds to the critical chance (0x4c827e-0x4c82b6): twice the Backstab
/// level less the target's level, and 20 more for a Master.
pub fn stab_critical(stab: Stab, (level, rank): (i32, u8), target_level: i32) -> i32 {
    if stab != Stab::Unnoticed {
        return 0;
    }
    2 * level - target_level + if rank >= 3 { 20 } else { 0 }
}

/// How helpless NPC watcher `o` is against a prowler (0x4c8603: its `critter_flags` as a
/// target's, [`helpless_by_state`]; the down and stunned see nothing anyway).
pub fn npc_watcher_helpless(o: &Object, prototypes: &Prototypes) -> i32 {
    helpless_by_state(prototypes.int(o, "critter_flags").unwrap_or(0), false, false, false).unwrap_or(0)
}

/// Whether NPC `o` sees in the dark ([`OCF2_DARK_SIGHT`], 0x45fbdf).
pub fn npc_dark_sight(o: &Object, prototypes: &Prototypes) -> bool {
    prototypes.int(o, "critter_flags2").unwrap_or(0) & OCF2_DARK_SIGHT != 0
}

/// The "Dark Sight" background (block 78 of `rules/backgrnd.mes`, `mes/gameback.mes` 1076):
/// 0x45fb90 tests 0x4c2690 for 0x4e.
pub const DARK_SIGHT_BACKGROUND: u32 = 78;
/// `critter_flags2` 0x800: a critter that sees in the dark (0x45fbdf).
pub const OCF2_DARK_SIGHT: i32 = 0x800;

/// What the dark adds to a weapon skill's difficulty at light `l` (0x4c8923-0x4c8ac7): none for
/// an undead attacker (`critter_flags` 0x4) or melee from Expert (0x4c8954); else
/// [`crate::light::dark_difficulty`], cut by the light the attacker bears
/// ([`crate::light::borne_scale`], 0x4c8a2f-0x4c8a7c); for one
/// that sees in the dark (0x45fb90) 30 less it (0x4c8aa4 / 0x4c8ac1).
pub fn dark_to_hit(skill: usize, rank: u8, critter_flags: i32, dark_sight: bool, l: i32) -> i32 {
    if critter_flags & OCF_UNDEAD != 0 || (skill == SKILL_MELEE && rank >= 2) {
        return 0;
    }
    let p = crate::light::borne_scale(crate::light::dark_difficulty(l), critter_flags);
    let p = if dark_sight { 30 - p } else { p };
    p.max(0)
}

/// A swing's time a frame in milliseconds (0x432700, 0x43285e-0x4328c2): the art's
/// `1000 / rate`, 10 ms less a point of weapon speed over 10, kept within 30-800.
pub fn swing_frame_millis(frame_rate: u32, speed: i32) -> u32 {
    let base = 1000 / frame_rate.max(1) as i32;
    (base - 10 * (speed - 10)).clamp(30, 800) as u32
}

/// A fate point waiting on a blow ([`Fighter::fate`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Fate {
    #[default]
    None,
    CriticalHit,
    CriticalMiss,
}

/// The fate options of `mes/fate_ui.mes` a blow uses (0x4b36a2 / 0x4b36e0).
pub const FATE_NEXT_ATTACK: u8 = 2;
pub const FATE_OPPONENT_FAILS: u8 = 3;

/// The two helpless amounts (0x4c864a / 0x4c86d9: 0x32, 0x1e).
pub const HELPLESS_DOWN: i32 = 50;
pub const HELPLESS_UNAWARE: i32 = 30;

fn strike(commands: &mut Commands, w: &mut CombatWorld, all: &[Seen], attacker: &Seen, victim: &Seen, fighter: &Fighter, aim: Aim) -> AttackOutcome {
    let defender = Fighter { helpless: helpless(w, attacker, victim), ..w.fighter(victim) };
    let ranged = fighter.weapon.as_ref().is_some_and(|wp| wp.range > 1);
    // The weapon whose scripts the blow runs, before a critical miss can take it.
    let weapon = fighter.weapon.as_ref().and_then(|_| w.wielded(attacker));
    let situation = if ranged { w.shot_difficulty(all, attacker, victim, fighter.skill()) } else { 0 } + w.dark_difficulty(attacker, victim, fighter.skill());
    let stab = stab_of(fighter, attacker, victim, !w.notices_quietly(victim, attacker));
    // A waiting fate use (0x4b3697): the PC attacker's own, else a PC target's against an NPC.
    let fate = match (attacker.player, victim.player) {
        (Some(p), _) if w.consume_fate(p, FATE_NEXT_ATTACK) => Fate::CriticalHit,
        (None, Some(p)) if w.consume_fate(p, FATE_OPPONENT_FAILS) => Fate::CriticalMiss,
        _ => Fate::None,
    };
    let fighter = &Fighter { situation, stab, fate, ..fighter.clone() };
    let mut outcome = resolve(fighter, &defender, &mut w.rng);
    outcome.thrown = fighter.skill() == SKILL_THROWING;
    // A weapon immune to wear is never worn, destroyed or blown up (0x4b4e81), its floats too.
    if w.weapon_wear_immune(attacker) {
        outcome.effects &= !WEAPON_DESTROYED;
        outcome.fumble_items &= !WEAPON_DAMAGED;
    }
    if w.weapon_wear_immune(victim) {
        outcome.items &= !WEAPON_DAMAGED;
    }
    outcome.speed = fighter.speed();
    if let Some(weapon) = fighter.weapon.as_ref().filter(|w| w.uses_ammo()) {
        w.spend_ammo(attacker, weapon.ammo_type, weapon.ammo_per_shot);
    }
    // A critical miss's wear on the attacker's own items, then its weapon knocked away.
    if outcome.fumble_items != 0 {
        let dropped = outcome.fumble_items & WEAPON_DROPPED;
        outcome.fumble_items = w.wear_items(attacker, attacker, &AttackOutcome { items: outcome.fumble_items, ..outcome });
        if dropped != 0 && outcome.fumble_items & WEAPON_BROKEN == 0 && w.drop_weapon(attacker) {
            outcome.fumble_items |= WEAPON_DROPPED;
        }
    }
    if outcome.items != 0 {
        let dropped = outcome.items & WEAPON_DROPPED;
        outcome.items = w.wear_items(attacker, victim, &outcome);
        // A weapon broken by the same hit is gone (0x4b4f21 runs first).
        if dropped != 0 && outcome.items & WEAPON_BROKEN == 0 && w.drop_weapon(victim) {
            outcome.items |= WEAPON_DROPPED;
        }
    }
    outcome.killed = outcome.hit && outcome.hp > 0 && victim.npc.is_some_and(|n| w.map.npc_hp(n.sector, n.index).is_some_and(|(left, _)| left - outcome.hp < 1));
    crate::ground::broadcast(&w.session, &mut w.outbox, LedgerDelta::Attacked { attacker: attacker.net_id, target: victim.net_id, outcome });
    attack_events(w, attacker, victim, &outcome, weapon, ranged);
    // The remark needs the target meant (0x4b6bfc); 0x4a9650's part only blows not thrown.
    if aim != Aim::Stray {
        praise_critical(w, all, attacker, victim, &outcome);
    }
    if let (Some(player), Some(source)) = (attacker.player, victim.npc)
        && victim.leader == Some(player)
        && aim == Aim::Meant
    {
        leader_hits_follower(commands, w, player, source, victim, aim);
    }
    apply_effects(commands, w, attacker, victim, fighter, &defender, &outcome);
    // Throws draw no one in, and stir no one's reaction (0x4b3334: 0x4a9650 skipped).
    if aim == Aim::Meant {
        rally(commands, w, all, attacker, victim);
    }
    if let Some(player) = victim.player {
        if outcome.hit {
            w.hurt_pc(player, &outcome);
        }
        return outcome;
    }
    let Some(source) = victim.npc else { return outcome };
    // One who gave up fights again only when whom it gave up to attacks it (0x4aa620 case 3,
    // 0x4aa71b → 0x4ab2a0); attacked by another, it begs that one (0x413ea0 at 0x4aa759,
    // `gd_npc` 3000-3099) when it can act (0x45d790), from 0x4a9650: blows not thrown.
    match victim.surrendered {
        Some(to) if to != attacker.net_id => {
            let acts = aim == Aim::Meant && !outcome.killed && victim.stunned.is_none() && !w.map.npc_down(source.sector, source.index);
            if let Some(player) = attacker.player.filter(|_| acts) {
                w.checked_bark(victim, player, FLEE_BARK);
            }
            // Then it flees that one (0x434980 at 0x4aa779: goal 0x19, `ag_flee`, when it can
            // act, 0x4348e0), as far as a fleeing fighter (builder's call: the goal's handler
            // not read).
            if acts {
                let away = flee_goal(victim.at, attacker.at, FLEE_TILES);
                if let Ok(path) = crate::find_path(&*w.map, victim.at, away)
                    && !path.is_empty()
                {
                    commands.entity(victim.entity).insert(crate::Walking { steps: path.into(), progress: 0.0 });
                }
            }
        }
        Some(_) => {
            commands.entity(victim.entity).remove::<Surrendered>().insert(Fighting { target: attacker.net_id });
        }
        None => {
            commands.entity(victim.entity).insert(Fighting { target: attacker.net_id });
        }
    }
    if !outcome.hit || (outcome.hp <= 0 && outcome.fatigue <= 0 && outcome.effects & KNOCKED_OUT == 0) {
        return outcome;
    }
    // Its taking-damage script is asked first: the damage waits for the answer.
    if w.map.attachment(source.object(), EVENT_TAKING_DAMAGE).number != 0 {
        let hit = HeldHit { attacker_object: object_of(attacker), attacker_player: attacker.player, player: attacker.player.or(attacker.leader).or(victim.leader), victim: victim.net_id, victim_object: source.object(), outcome };
        w.checks.hits.push(hit);
        return outcome;
    }
    damage_npc(commands, w, attacker.player, victim, &outcome);
    outcome
}

/// A hit's and a critical miss's effects: stuns, the attacker's weapon destroyed or ammunition
/// lost, its injuries from its own critical (0x4b4390).
fn apply_effects(commands: &mut Commands, w: &mut CombatWorld, attacker: &Seen, victim: &Seen, fighter: &Fighter, defender: &Fighter, outcome: &AttackOutcome) {
    if outcome.effects & STUNNED != 0 {
        commands.entity(victim.entity).insert(Stunned { left: stun_count(defender.constitution) });
    }
    // Knocked down (0x435a90, goal 62): it lies until its fatigue recovery gets it up.
    if outcome.effects & KNOCKED_DOWN != 0 {
        commands.entity(victim.entity).insert(KnockedDown { until: None });
    }
    if outcome.fumble_effects & KNOCKED_DOWN != 0 {
        commands.entity(attacker.entity).insert(KnockedDown { until: None });
    }
    if outcome.fumble_effects & STUNNED != 0 && !fighter.stunned {
        commands.entity(attacker.entity).insert(Stunned { left: stun_count(fighter.constitution) });
    }
    let weapon = fighter.weapon.as_ref();
    let lost = match weapon {
        Some(_) if outcome.effects & AMMO_LOST_ALL != 0 => fighter.ammo.max(0),
        Some(wp) if outcome.effects & AMMO_LOST != 0 && fighter.ammo >= wp.ammo_per_shot * 5 => wp.ammo_per_shot * 5,
        _ => 0,
    };
    if let Some(wp) = weapon.filter(|_| lost > 0) {
        w.spend_ammo(attacker, wp.ammo_type, lost);
    }
    if outcome.effects & WEAPON_DESTROYED != 0 {
        w.destroy_weapon(attacker);
    }
    if outcome.fumble_injured != 0 {
        match (attacker.player, attacker.npc) {
            (Some(p), _) => {
                if let Some(saved) = w.world.0.as_mut().and_then(|s| s.players.get_mut(&p)) {
                    saved.blueprint.sheet.injuries |= outcome.fumble_injured;
                    w.sheets.0.insert(p);
                }
            }
            (None, Some(n)) => w.map.injure_npc(n.sector, n.index, outcome.fumble_injured),
            _ => {}
        }
    }
}

/// The script object of a critter of the fight.
fn object_of(c: &Seen) -> crate::ScriptObject {
    match (c.player, c.npc) {
        (Some(p), _) => crate::ScriptObject::Pc(p),
        (None, Some(n)) => n.object(),
        _ => crate::ScriptObject::Pc(PlayerId(0)),
    }
}

/// The script events of an attack (answers ignored): the weapon's hit or miss, then its
/// critical hit or miss (a missile's only for a weapon object, type 5: 0x4b2fb3 / 0x4b3142),
/// PCs' and NPCs' alike (0x4b3770 and 0x4b2f60 test no type); the attacker's critter-hits on a
/// hit. `weapon`: the wielded item's prototype as the blow began (an unarmed blow runs none).
fn attack_events(w: &mut CombatWorld, attacker: &Seen, victim: &Seen, outcome: &AttackOutcome, weapon: Option<u32>, ranged: bool) {
    let Some(player) = attacker.player.or(victim.player).or(attacker.leader).or(victim.leader) else { return };
    let item = match (attacker.player, attacker.npc, weapon) {
        (Some(holder), _, Some(prototype)) => Some(crate::ScriptObject::Carried { holder, prototype }),
        (None, Some(n), Some(prototype)) => Some(crate::ScriptObject::NpcCarried { sector: n.sector, index: n.index, prototype }),
        _ => None,
    };
    if let (Some(item), Some(prototype)) = (item, weapon) {
        let (first, critical) = if outcome.hit || outcome.dodged { (EVENT_HIT, outcome.critical) } else { (EVENT_MISS, outcome.fumbled) };
        let event = |event| WeaponEvent { item, prototype, event, player, attacker: object_of(attacker), victim: Some(object_of(victim)) };
        w.events.1.push(event(first));
        let weapon_object = w.map.prototypes.get(prototype).is_some_and(|p| p.object_type == arcanum_formats::object::TYPE_WEAPON);
        if critical && (!ranged || weapon_object) {
            w.events.1.push(event(if first == EVENT_HIT { EVENT_CRITICAL_HIT } else { EVENT_CRITICAL_MISS }));
        }
    }
    if outcome.hit && attacker.npc.is_some() {
        w.events.0.push((object_of(attacker), EVENT_CRITTER_HITS, player, object_of(victim)));
    }
}

/// An NPC takes a hit's damage: injuries, hit points, the experience, and its death (events 12
/// and 20).
fn damage_npc(commands: &mut Commands, w: &mut CombatWorld, attacker_player: Option<PlayerId>, victim: &Seen, outcome: &AttackOutcome) {
    let Some(source) = victim.npc else { return };
    if w.map.is_dead(source.sector, source.index) {
        return;
    }
    if outcome.injured != 0 {
        w.map.injure_npc(source.sector, source.index, outcome.injured);
    }
    // Fatigue (0x4b4390, field 0xe2): a knock-out takes it to its most + 10-20 (0x4b5f40 /
    // 0x4cef00(10, 20)); out of it the NPC lies down (0x45d5a0 → 0x435a90).
    if let Some((tired, most)) = w.map.npc_fatigue(source.sector, source.index) {
        let damage = most - tired;
        let new = if outcome.effects & KNOCKED_OUT != 0 && tired >= 1 { most + w.rng.range(10, 20) } else { damage + outcome.fatigue.max(0) };
        if new != damage {
            w.map.set_npc_fatigue_damage(source.sector, source.index, new);
            if tired >= 1 && most - new < 1 {
                let vfs = w.vfs.as_ref().map(|v| v.0.clone());
                let guard = vfs.as_ref().and_then(|v| v.read().ok());
                w.map.lay_npc_down(guard.as_deref(), source.sector, source.index, true);
                commands.entity(victim.entity).remove::<Fighting>();
                w.toggled.0.push((source.sector, source.index));
            }
        }
    }
    // Poison adds to its level (0x4b51a4), as a PC's.
    if outcome.poison > 0 {
        let level = w.map.npc_poison(source.sector, source.index);
        w.map.set_npc_poison(source.sector, source.index, level + outcome.poison);
    }
    let Some((left, max)) = w.map.npc_hp(source.sector, source.index) else { return };
    let dealt = outcome.hp.min(left.max(0));
    w.map.hurt_npc(source.sector, source.index, outcome.hp);
    // A follower of a PC brought under 20 % calls to its leader (0x4b50bb-0x4b50f1 → 0x4aeab0:
    // `gd_npc` 5500, 0x414490), once as it crosses (before at least 20, 0x4b50d3; after under,
    // 0x4b50e4), dying or not.
    if let Some(leader) = victim.leader
        && outcome.hp > 0
        && max > 0
        && left * 100 / max >= LOW_HEALTH_PERCENT
        && (left - outcome.hp).max(0) * 100 / max < LOW_HEALTH_PERCENT
    {
        w.bark(source, victim.net_id, leader, LOW_HEALTH_BARK);
    }
    // Experience for the damage (0x4b4390) and the kill (0x45da20: a fifth of its worth).
    let (worth, pool) = w.map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)).map_or((0, 0), |o| (w.map.prototypes.int(o, "npc_experience_worth").unwrap_or(0), w.map.prototypes.int(o, "npc_experience_pool").unwrap_or(0)));
    let mut xp = 0;
    if attacker_player.is_some() {
        xp = hit_experience(dealt, max, worth, pool);
        if xp > 0 {
            w.map.spend_npc_pool(source.sector, source.index, xp);
        }
    }
    if left - outcome.hp < 1 {
        let vfs = w.vfs.as_ref().map(|v| v.0.clone());
        let guard = vfs.as_ref().and_then(|v| v.read().ok());
        w.map.kill_npc(guard.as_deref(), source.sector, source.index, crate::death_anim(outcome.hp, outcome.part));
        commands.entity(victim.entity).remove::<Fighting>();
        w.toggled.0.push((source.sector, source.index));
        if let Some(player) = attacker_player {
            w.events.0.push((source.object(), EVENT_DYING, player, crate::ScriptObject::Pc(player)));
            // The killer's followers hear of it (0x45db95).
            let homes: Vec<crate::follow::NpcHome> = w.world.0.as_ref().map(|s| s.followers.iter().filter(|f| f.leader == player).map(|f| f.home()).collect()).unwrap_or_default();
            for home in homes {
                if let Some((sector, index)) = w.map.place_of(&home) {
                    w.events.0.push((crate::ScriptObject::Mobile { sector, index }, EVENT_LEADER_KILLING, player, source.object()));
                }
            }
            xp += worth * 20 / 100;
        }
    }
    if xp > 0 {
        w.awards.0.push(xp);
    }
    crate::ground::save_change(&mut w.world, &w.map, source.sector);
}

/// Whether NPC `w` takes `x`'s side (0x4ae3a0 / 0x45d110): `x` itself; a PC it follows, or a PC
/// whose reputations include its (non-zero) faction (`kin`: 0x4c21e0 at 0x45d26d); an NPC
/// following the same PC; an NPC of its own (non-zero) `npc_faction` (0x45d080). Not counted:
/// leaders among NPCs.
pub fn allied(w_leader: Option<PlayerId>, w_faction: i32, x_player: Option<PlayerId>, x_leader: Option<PlayerId>, x_faction: Option<i32>, kin: bool) -> bool {
    match (x_player, x_faction) {
        (Some(p), _) => w_leader == Some(p) || (w_faction != 0 && kin),
        (None, Some(f)) => (w_leader.is_some() && w_leader == x_leader) || (w_leader.is_none() && x_leader.is_none() && f != 0 && f == w_faction),
        (None, None) => false,
    }
}

/// Who else an attack draws in (0x4a9650, the hit's radius index 2: [`ENGAGE_RADIUS`] tiles,
/// table 0x5b50c0). Each NPC near the victim that is not fighting yet, alive and not the attacker
/// or victim:
/// - a follower of either side, or an NPC allied to the victim that notices it ([`notice_reach`]:
///   sight or hearing), fights the other side (0x4a9b80 / 0x4a9f10 → 0x4aa620) and joins the
///   encounter;
/// - an NPC of neither side that notices the attacker, idle and not `OCF_NO_FLEE`, flees
///   ([`FLEE_TILES`] away from it, out of the fight).
///
/// A PC attacker's NPC victim then likes it no more than its row's column 10 (0x4a9650: lowered
/// to it, never raised; 0x4aabe0 again). Not built: the bark, a leader hitting its follower
/// (column 9, rebellion 0x4ad950), indirect hits (-10), followers who like the victim staying out
/// (0x4adcc0).
fn rally(commands: &mut Commands, w: &mut CombatWorld, all: &[Seen], attacker: &Seen, victim: &Seen) {
    let faction = |w: &CombatWorld, c: &Seen| c.npc.map(|n| w.npc_int(n, "npc_faction"));
    let (a_faction, v_faction) = (faction(w, attacker), faction(w, victim));
    let player = attacker.player.or(victim.player);
    for c in all {
        let Some(source) = c.npc else { continue };
        if c.net_id == attacker.net_id || c.net_id == victim.net_id || c.fighting || c.surrendered.is_some() || w.joined.contains(&c.net_id) || w.map.npc_down(source.sector, source.index) || !crate::within_radius(c.at, victim.at, ENGAGE_RADIUS) {
            continue;
        }
        let own = w.npc_int(source, "npc_faction");
        let (v_kin, a_kin) = (victim.player.is_some_and(|p| w.pc_kin(p, own)), attacker.player.is_some_and(|p| w.pc_kin(p, own)));
        let Some(o) = w.map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)) else { continue };
        let pe = npc_stat(o, &w.map.prototypes, 5);
        // A prowling PC is noticed less (0x4a9f10 → 0x4af260 / 0x4af470 with its stealth).
        let notices = |x: &Seen| {
            let target = x.npc.and_then(|n| w.map.mobiles.get(&n.sector)?.get(n.index as usize));
            let prowler = x.sheet.as_ref().filter(|_| x.prowling).map(|s| crate::prowl::Prowler::of(&w.rules.0, s, &w.map, x.at, w.clock.now.ms / crate::HOUR_MS, x.inventory.as_ref().map_or(0, |i| i.borne_light(&w.map.prototypes)), x.inventory.as_ref()).for_watcher(npc_dark_sight(o, &w.map.prototypes)).watched_by(npc_watcher_helpless(o, &w.map.prototypes)));
            notices(&crate::MissileView(&w.map), can_look(o, &w.map.prototypes, c.stunned.is_some(), target).then_some(pe), c.at, c.rotation, ears(o, &w.map.prototypes, c.stunned.is_some()), x.at, prowler)
        };
        let for_victim = allied(c.leader, own, victim.player, victim.leader, v_faction, v_kin) && (c.leader.is_some() || notices(victim));
        let for_attacker = allied(c.leader, own, attacker.player, attacker.leader, a_faction, a_kin) && (c.leader.is_some() || notices(attacker));
        let foe = match (for_victim, for_attacker) {
            (true, false) => attacker,
            (false, true) => victim,
            (false, false) => {
                // Guards stand (0x4aa055: social class 5); the others flee (state 2, 0x4aa0be)
                // and bark as they go (0x4aabe0 → 0x413ea0: `gd_npc` 3000).
                if c.leader.is_none() && !c.walking && w.npc_int(source, "critter_flags") & OCF_NO_FLEE == 0 && w.npc_int(source, "npc_social_class") != SOCIAL_CLASS_GUARD && notices(attacker) {
                    let away = flee_goal(c.at, attacker.at, FLEE_TILES);
                    if let Ok(path) = crate::find_path(&*w.map, c.at, away)
                        && !path.is_empty()
                    {
                        commands.entity(c.entity).insert(crate::Walking { steps: path.into(), progress: 0.0 });
                        if let Some(player) = player {
                            w.checked_bark(c, player, FLEE_BARK);
                        }
                    }
                }
                continue;
            }
            (true, true) => continue,
        };
        // A follower may refuse (0x4a9c00 → 0x4adcc0); its leader's side then likes it 5 less
        // (0x4a9d26), the victim's side nothing.
        if let Some(leader) = c.leader
            && let Some(code) = w.follower_refusal(source, leader, foe)
        {
            if for_attacker && attacker.player == Some(leader) {
                w.refusal_bark(source, c.net_id, leader, code);
                w.change_reaction(source, leader, REFUSAL_REACTION);
            }
            continue;
        }
        commands.entity(c.entity).insert(Fighting { target: foe.net_id });
        w.joined.push(c.net_id);
        if let Some(e) = w.combat.0.as_mut()
            && !e.order.contains(&c.net_id)
        {
            e.order.push(c.net_id);
        }
        if let Some(player) = player {
            w.events.0.push((source.object(), EVENT_ENTER_COMBAT, player, crate::ScriptObject::Pc(player)));
        }
    }
    // The victim's reaction to a PC attacker falls to its row's column 10 (not its own
    // follower's: that is column 9's change, 0x4a9951).
    if let (Some(player), Some(source)) = (attacker.player, victim.npc)
        && victim.leader != Some(player)
        && let Some(row) = w.ai_row(source)
        && let Some(now) = w.reaction(source, player)
        && now > row[10]
    {
        w.change_reaction(source, player, row[10] - now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chances_and_costs_follow_the_original() {
        assert_eq!((weapon_chance(SKILL_MELEE, 0, 8), weapon_chance(SKILL_MELEE, 4, 8), weapon_chance(SKILL_MELEE, 20, 8)), (25, 45, 125), "no cap: 0x5b6f64 0x58d");
        assert_eq!(weapon_chance(SKILL_DODGE, 50, 8), 95, "dodge: 0x800, at most 95");
        assert_eq!((weapon_chance(SKILL_MELEE, 4, 20), weapon_chance(SKILL_DODGE, 0, 20), weapon_chance(SKILL_MELEE, 15, 20)), (55, 10, 110), "Intelligence 20: + 10");
        assert_eq!((weapon_chance(SKILL_DODGE, 5, 8), weapon_chance(SKILL_THROWING, 2, 8)), (10, 39));
        assert_eq!((chance_against(SKILL_FIREARMS, 4, 8, true), chance_against(SKILL_FIREARMS, 4, 8, false), chance_against(SKILL_BOW, 4, 8, true)), (25, 45, 45), "a ward: firearms only");
        assert_eq!((attack_cost(0), attack_cost(9), attack_cost(21), attack_cost(30)), (8, 5, 2, 1));
        assert_eq!(Fighter::default().attack_cost(), 5, "bare hands: speed 10");
        assert_eq!(pc_unarmed(0), [(1, 5), (0, 0), (0, 0), (0, 0), (1, 5)]);
        assert_eq!(pc_unarmed(40)[0], (15, 45));
        assert_eq!(hit_experience(10, 20, 100, 100), 50);
        assert_eq!(hit_experience(10, 20, 100, 30), 30, "what is left");
    }

    #[test]
    fn hostility_and_reach_follow_the_ai_row() {
        let row = [0, 1000, 1000, 20, 6, 20, 1001, 1000, 2, 0, 0, 2001, 100, 50, 20, 1, 1];
        assert!(hostile(ONF_KOS, 60, 0, &row), "kill on sight");
        assert!(!hostile(ONF_KOS | ONF_KOS_OVERRIDE, 60, 0, &row), "made peaceful");
        assert!(hostile(0, 0, 0, &row) && !hostile(0, 1, 0, &row), "reaction at or below column 10");
        assert!(!hostile(0, 50, 1000, &row), "alignment: column 11 (2001) is out of reach");
        assert_eq!((notice_reach(8, None), notice_reach(20, None), notice_reach(8, Some(crate::prowl::Prowler { level: 20, intelligence: 8, rank: 2, ..Default::default() }))), (8, 10, 3), "a prowler 10 over: 100");
    }

    #[test]
    fn npcs_flee_by_their_ai_row() {
        let rows = parse_ai_params(&arcanum_formats::mes::MesFile::parse(b"{0}{0 1000 1000 20 6 20 1001 1000 2 0 0 2001 100 50 20 1 1}{1}{50 20 3 20 20 20 20 20 2 0 0 2001 100 15 100 0 1}").unwrap());
        assert_eq!(rows.len(), 2);
        let one = |levels| Party { count: 1, levels };
        assert!(!should_flee(10, 100, one(1), one(1), &rows[0], 0), "row 0: only from a side 1000 larger");
        assert!(should_flee(50, 100, one(1), one(1), &rows[1], 0) && !should_flee(51, 100, one(1), one(1), &rows[1], 0));
        assert!(!should_flee(10, 100, one(1), one(1), &rows[1], OCF_NO_FLEE));
        assert!(!should_flee(10, 19, one(1), one(1), &rows[1], 0), "not from a target below column 3");
        assert!(should_flee(60, 100, one(1), one(4), &rows[1], 0), "a side 3 levels higher");
        assert!(!should_flee(60, 100, one(1), one(3), &rows[1], 0));
        assert!(should_flee(60, 100, one(1), Party { count: 21, levels: 0 }, &rows[1], 0), "a side 20 larger");
        assert_eq!(flee_goal(TileLoc { x: 10, y: 10 }, TileLoc { x: 9, y: 10 }, 8), TileLoc { x: 18, y: 10 });
    }

    #[test]
    fn npcs_pick_the_best_weapon_they_can_use_that_reaches() {
        use arcanum_formats::object::{Array, Object, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let ints = |v: [i32; 5]| Value::Array(Some(Array { elem_size: 4, id: 1, data: v.iter().flat_map(|x| x.to_le_bytes()).collect(), bitset: vec![0x1f, 0] }));
        let mut prototypes = Prototypes::default();
        // 200 a dagger (2-4), 201 a sword (6-10), 202 a bow (8-12, arrows, 3 a shot), 203 arrows.
        let weapon = |n: u32, lo: i32, hi: i32, ammo: i32, range: i32| {
            let b = ObjectBuilder::new(arcanum_formats::object::TYPE_WEAPON).field("weapon_damage_lower_idx", ints([lo, 0, 0, 0, 0])).field("weapon_damage_upper_idx", ints([hi, 0, 0, 0, 0])).field("weapon_ammo_type", Value::Int32(ammo)).field("weapon_ammo_consumption", Value::Int32(if ammo == NO_AMMO { 0 } else { 3 })).field("weapon_range", Value::Int32(range));
            let mut o = Object::parse_instance(&b.build()).unwrap();
            o.id = prototype_id(n);
            o
        };
        for o in [weapon(200, 2, 4, NO_AMMO, 1), weapon(201, 6, 10, NO_AMMO, 1), weapon(202, 8, 12, 0, 10)] {
            prototypes.insert(o);
        }
        let mut arrows = Object::parse_instance(&ObjectBuilder::new(TYPE_AMMO).field("ammo_type", Value::Int32(0)).build()).unwrap();
        arrows.id = prototype_id(203);
        prototypes.insert(arrows);
        let item = |prototype, location, quantity| crate::CarriedItem { prototype, location, quantity, keys: vec![], written: 0, wear: Default::default() };
        let skilled = Fighter { levels: vec![(SKILL_MELEE, 10), (SKILL_BOW, 10)], ..Default::default() };
        let inv = |items| crate::Inventory { items, ..Default::default() };
        let melee = inv(vec![item(200, WEAPON_SLOT, 1), item(201, 0, 1)]);
        assert_eq!(choose_weapon(&melee, &prototypes, &skilled, Some(1)), Some(0), "the sword over the dagger");
        let archer = inv(vec![item(201, WEAPON_SLOT, 1), item(202, 0, 1), item(203, 1, 2)]);
        assert_eq!(choose_weapon(&archer, &prototypes, &skilled, Some(5)), Some(WEAPON_SLOT), "2 arrows: the bow cannot shoot");
        let archer = inv(vec![item(201, WEAPON_SLOT, 1), item(202, 0, 1), item(203, 1, 9)]);
        assert_eq!(choose_weapon(&archer, &prototypes, &skilled, Some(5)), Some(0), "the bow reaches");
        assert_eq!(choose_weapon(&inv(vec![item(202, WEAPON_SLOT, 1)]), &prototypes, &skilled, Some(5)), None, "no arrows: bare hands");
        assert_eq!(choose_weapon(&archer, &prototypes, &skilled, None), Some(0), "no foe: the best");
        // Wielding the pick: the sword swaps into the hands; bare hands put the bow away.
        let info = crate::item_info(&prototypes, |_| Some((32, 32)));
        let armed = wield_pick(&melee, Some(0), &info).unwrap();
        assert_eq!(armed.items.iter().find(|i| i.location == WEAPON_SLOT).map(|i| i.prototype), Some(201));
        assert!(armed.items.iter().any(|i| i.prototype == 200 && crate::in_grid(i.location)));
        assert_eq!(wield_pick(&melee, Some(WEAPON_SLOT), &info), None, "already wielded");
        let bare = wield_pick(&inv(vec![item(202, WEAPON_SLOT, 1)]), None, &info).unwrap();
        assert!(bare.items.iter().all(|i| i.location != WEAPON_SLOT));
        // An item new to an NPC asks it to pick again (0x46695a); a move does not.
        let mut map = crate::LoadedMap { prototypes: prototypes.clone().into(), ..Default::default() };
        let sector = arcanum_formats::map::SectorId::new(1, 1);
        map.mobiles.insert(sector, vec![Object::parse_instance(&ObjectBuilder::new(arcanum_formats::object::TYPE_NPC).build()).unwrap()]);
        map.set_npc_inventory(sector, 0, melee.clone());
        assert_eq!(map.npc_flags(sector, 0).unwrap() & ONF_REPICK_ALL, ONF_REPICK_ALL);
        map.set_npc_flags(sector, 0, 0);
        map.set_npc_inventory(sector, 0, armed);
        assert_eq!(map.npc_flags(sector, 0), Some(0), "the same items");
    }

    #[test]
    fn npcs_wear_their_most_valuable_armour_that_fits() {
        use arcanum_formats::object::TYPE_ARMOR;
        // 1 plain body armour (worth 10), 2 fine (worth 50), 3 a large-only suit (worth 900),
        // 4 a helmet (worth 5).
        let info = |p: u32| {
            let (coverage, worth, flags) = match p {
                1 => (0, 10, 7),
                2 => (0, 50, 7),
                3 => (0, 900, 4),
                4 => (2, 5, 7),
                _ => return None,
            };
            Some(crate::ItemInfo { kind: TYPE_ARMOR, inventory_art: coverage << 14, cells: (1, 1), worth, armor_flags: flags, ..Default::default() })
        };
        let item = |prototype, location| crate::CarriedItem { prototype, location, quantity: 1, keys: vec![], written: 0, wear: Default::default() };
        let human = crate::Wearer { race: 0, gender: arcanum_rules::MALE };
        let inv = crate::Inventory { items: vec![item(1, crate::Slot::Armor as i32), item(2, 0), item(3, 1), item(4, 2)], ..Default::default() };
        let worn = wear_best(&inv, human, &info);
        let at = |p| worn.items.iter().find(|i| i.prototype == p).unwrap().location;
        assert_eq!((at(2), at(4)), (crate::Slot::Armor as i32, crate::Slot::Helmet as i32), "the finer suit, the helmet put on");
        assert!(crate::in_grid(at(1)) && crate::in_grid(at(3)), "the plain one off, the large suit never");
        assert_eq!(wear_best(&worn, human, &info), worn, "nothing better: as it was");
        let ogre = crate::Wearer { race: 7, gender: arcanum_rules::MALE };
        assert_eq!(wear_best(&worn, ogre, &info).items.iter().find(|i| i.location == crate::Slot::Armor as i32).map(|i| i.prototype), Some(3), "a half-ogre: the large suit");
    }

    #[test]
    fn the_blinded_see_nothing_and_the_invisible_go_unseen() {
        use arcanum_formats::object::{Array, Object, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let prototypes = Prototypes::default();
        let stats = |pe: i32| {
            let mut v = [0i32; 6];
            v[5] = pe;
            Value::Array(Some(Array { elem_size: 4, id: 1, data: v.iter().flat_map(|x| x.to_le_bytes()).collect(), bitset: vec![0x3f, 0] }))
        };
        fn npc(fields: &[(&'static str, Value)]) -> Object {
            let mut b = ObjectBuilder::new(arcanum_formats::object::TYPE_NPC);
            for (f, v) in fields {
                b = b.field(f, v.clone());
            }
            Object::parse_instance(&b.build()).unwrap()
        }
        let watcher = npc(&[("critter_stat_base_idx", stats(8))]);
        // The listener's state (0x4af49b / 0x4af518): stunned deaf, asleep half.
        let listener = |flags: i32| npc(&[("critter_flags", Value::Int32(flags)), ("critter_stat_base_idx", stats(8))]);
        assert_eq!((ears(&listener(0), &prototypes, false), ears(&listener(OCF_STUNNED), &prototypes, false), ears(&listener(0), &prototypes, true), ears(&listener(OCF_SLEEPING), &prototypes, false)), (Some(8), None, None, Some(4)));
        assert!(can_look(&watcher, &prototypes, false, None));
        assert!(!can_look(&watcher, &prototypes, true, None), "stunned");
        let blind = npc(&[("critter_flags", Value::Int32(OCF_BLINDED)), ("critter_stat_base_idx", stats(8))]);
        assert!(!can_look(&blind, &prototypes, false, None), "blinded (0x80 of 0x10a0)");
        let ghost = npc(&[("flags", Value::Int32(OF_INVISIBLE))]);
        let spelled = npc(&[("spell_flags", Value::Int32(OSF_INVISIBLE))]);
        assert!(!can_look(&watcher, &prototypes, false, Some(&ghost)) && !can_look(&watcher, &prototypes, false, Some(&spelled)));
        let keen = npc(&[("critter_stat_base_idx", stats(20))]);
        assert!(can_look(&keen, &prototypes, false, Some(&ghost)), "Perception 20");
        let seer = npc(&[("spell_flags", Value::Int32(OSF_SEE_INVISIBLE)), ("critter_stat_base_idx", stats(8))]);
        assert!(can_look(&seer, &prototypes, false, Some(&spelled)), "sees the invisible");
    }

    #[test]
    fn a_pcs_shots_pass_its_followers_but_not_other_pcs() {
        let (me, other) = (Some(PlayerId(1)), Some(PlayerId(2)));
        assert!(missile_passes(me, None, me, false), "its own follower, even placed solid");
        assert!(!missile_passes(me, other, None, true), "another PC");
        assert!(!missile_passes(me, None, other, false), "another PC's follower placed solid: by its flag");
        assert!(missile_passes(me, None, other, true));
        assert!(missile_passes(None, other, None, true), "an NPC's shot: PCs pass, as the data");
        assert!(!missile_passes(None, None, None, false), "an NPC placed solid");
    }

    #[test]
    fn a_throw_strikes_the_first_critter_on_its_way() {
        let t = |x, y| TileLoc { x, y };
        let others = [(NetId(5), t(14, 10)), (NetId(6), t(12, 10)), (NetId(7), t(12, 11))];
        let (clear, open) = (|_, _| true, |_| true);
        assert_eq!(first_on_line(t(10, 10), t(16, 10), others.iter().copied(), clear, open), Some(Met::Critter(NetId(6))), "the nearer of two on the line");
        assert_eq!(first_on_line(t(10, 10), t(12, 10), others.iter().copied(), clear, open), None, "the target's own tile does not count");
        assert_eq!(first_on_line(t(10, 12), t(16, 12), others.iter().copied(), clear, open), None, "nobody between");
        let boulder = |p: TileLoc| p != t(13, 12);
        assert_eq!(first_on_line(t(10, 12), t(16, 12), others.iter().copied(), clear, boulder), Some(Met::Blocked(t(12, 12))), "a big stone stops it, the throw left on the tile before");
        assert_eq!(first_on_line(t(10, 10), t(16, 10), others.iter().copied(), clear, |p| p != t(11, 10)), Some(Met::Blocked(t(10, 10))), "before the critter");
        let wall = |a: TileLoc, b: TileLoc| !(a == t(11, 12) && b == t(12, 12));
        assert_eq!(first_on_line(t(10, 12), t(16, 12), others.iter().copied(), wall, open), Some(Met::Blocked(t(11, 12))), "a wall on the edge crossed");
        assert_eq!(first_on_line(t(10, 12), t(11, 12), others.iter().copied(), |_, _| false, open), Some(Met::Blocked(t(10, 12))), "even next to the target");
    }

    #[test]
    fn stray_hits_leave_a_followers_reaction_alone() {
        assert_eq!(leader_hit_reaction(-20, 10, Aim::Meant), Some(-20));
        assert_eq!(leader_hit_reaction(-20, 10, Aim::Stray), None, "a stray hit: no loss, no warning");
        assert_eq!(leader_hit_reaction(-20, 20, Aim::Meant), None, "charisma 20");
    }

    #[test]
    fn a_knocked_down_critter_is_neither_knocked_down_again_nor_stunned() {
        let sword = Weapon::default();
        let pc = Fighter { pc: true, ..Default::default() };
        let lying = Fighter { knocked_down: true, constitution: -100, ..Default::default() };
        let mut rng = HostRng::default();
        assert!((0..300).all(|_| critical_effects(&pc, &lying, &sword, BodyPart::Legs, &mut rng) & (KNOCKED_DOWN | STUNNED) == 0));
        assert_eq!(helpless_by_state(0, false, true, false), Some(30), "30 easier to hit while it lies");
    }

    #[test]
    fn backstabs_need_a_blade_behind_the_back() {
        assert!(can_backstab(4, 0, Some(2), false), "kind 2, unseen");
        assert!(!can_backstab(4, 0, Some(2), true), "in view");
        assert!(!can_backstab(0, 0, Some(2), false), "no skill");
        assert!(!can_backstab(4, 0, None, false), "bare hands");
        assert!(!can_backstab(4, 1, Some(3), false) && can_backstab(4, 2, Some(3), false), "kind 3 from Expert");
        assert!(!can_backstab(4, 3, Some(5), false), "kind 5 never");
        assert_eq!((stab_damage(Stab::None, 8), stab_damage(Stab::Behind, 8), stab_damage(Stab::Unnoticed, 8)), (0, 8, 40));
        // A stab's damage lands on the normal roll.
        let knife = Weapon { skill: SKILL_MELEE, damage: [(1, 1), (0, 0), (0, 0), (0, 0), (0, 0)], use_kind: 2, ..Default::default() };
        let stabber = Fighter { levels: vec![(SKILL_MELEE, 20)], weapon: Some(knife), backstab: (8, 0), stab: Stab::Behind, ..Default::default() };
        let mut rng = HostRng::default();
        let hit = (0..50).map(|_| resolve(&stabber, &Fighter::default(), &mut rng)).find(|o| o.hit && !o.critical).unwrap();
        assert_eq!(hit.hp, 9, "1 + 8");
        // Unnoticed: five times it, the armour skipped by the trained, a critical bonus.
        assert_eq!((stab_critical(Stab::Unnoticed, (8, 0), 5), stab_critical(Stab::Unnoticed, (8, 3), 5), stab_critical(Stab::Behind, (8, 3), 5)), (11, 31, 0));
        let unseen = Fighter { stab: Stab::Unnoticed, backstab: (8, 1), ..stabber };
        let plated = Fighter { armour_class: 95, ..Default::default() };
        let hits = (0..400).filter(|_| resolve(&unseen, &plated, &mut rng).hit).count();
        assert!(hits > 390, "armour skipped: {hits}");
        assert_eq!(crate::prowl::noise_reach(10, None, crate::prowl::NOISE_QUIET), 3, "a quiet blow: (6 + 0) / 2");
    }

    #[test]
    fn the_dark_makes_blows_harder() {
        assert_eq!(dark_to_hit(SKILL_BOW, 0, 0, false, 79), 20, "the default night: (255 - 79) × 30 / 255");
        assert_eq!(dark_to_hit(SKILL_BOW, 0, 0, false, 254), 0, "noon");
        assert_eq!((dark_to_hit(SKILL_MELEE, 2, 0, false, 0), dark_to_hit(SKILL_MELEE, 1, 0, false, 0)), (0, 30), "melee Experts are spared");
        assert_eq!(dark_to_hit(SKILL_BOW, 0, OCF_UNDEAD, false, 0), 0, "the undead");
        assert_eq!((dark_to_hit(SKILL_BOW, 0, 0x40_0000, false, 0), dark_to_hit(SKILL_BOW, 0, 0x8_0000, false, 0)), (7, 22));
        assert_eq!((dark_to_hit(SKILL_BOW, 0, 0, true, 0), dark_to_hit(SKILL_BOW, 0, 0, true, 254)), (0, 30), "dark sight: the light hinders");
    }

    #[test]
    fn arms_show_in_a_fight() {
        use arcanum_formats::object::{Object, TYPE_ARMOR, TYPE_WEAPON, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let mut prototypes = Prototypes::default();
        let mut sword = Object::parse_instance(&ObjectBuilder::new(TYPE_WEAPON).field("item_use_aid_fragment", Value::Int32((6 << 28) | (3 << 6))).build()).unwrap();
        sword.id = prototype_id(9200);
        let mut shield = Object::parse_instance(&ObjectBuilder::new(TYPE_ARMOR).build()).unwrap();
        shield.id = prototype_id(9201);
        prototypes.insert(sword);
        prototypes.insert(shield);
        let held = |p: u32, at: i32| crate::CarriedItem { prototype: p, location: at, quantity: 0, keys: vec![], written: 0, wear: Default::default() };
        let inv = crate::Inventory { items: vec![held(9200, WEAPON_SLOT), held(9201, SHIELD_SLOT)], ..Default::default() };
        assert_eq!(armed_art(true, Some(&inv), &prototypes), (3, true), "a sword (kind 3) and a shield");
        assert_eq!(armed_art(true, None, &prototypes), (1, false), "bare hands");
        assert_eq!(armed_art(false, Some(&inv), &prototypes), (0, false), "out of combat mode");
    }

    #[test]
    fn fate_makes_a_blow_critical_or_a_critical_miss() {
        let mut rng = HostRng::default();
        let novice = Fighter { levels: vec![(SKILL_MELEE, 0)], ..Default::default() };
        let nimble = Fighter { levels: vec![(SKILL_DODGE, 20)], armour_class: 95, ..Default::default() };
        let fated = Fighter { fate: Fate::CriticalHit, ..novice.clone() };
        assert!((0..200).all(|_| { let o = resolve(&fated, &nimble, &mut rng); o.hit && o.critical && !o.dodged }), "hit, critical, never dodged");
        let doomed = Fighter { levels: vec![(SKILL_MELEE, 20)], fate: Fate::CriticalMiss, ..Default::default() };
        assert!((0..200).all(|_| { let o = resolve(&doomed, &Fighter::default(), &mut rng); !o.hit && o.fumbled }), "a critical miss");
    }

    #[test]
    fn a_quick_weapon_swings_faster() {
        assert_eq!((swing_frame_millis(10, 10), swing_frame_millis(10, 15), swing_frame_millis(10, 0), swing_frame_millis(10, 30), swing_frame_millis(1, 0)), (100, 50, 200, 30, 800));
    }

    #[test]
    fn the_undead_never_tire_out() {
        use arcanum_formats::object::Value;
        let tired = |flags: i32| {
            let o = Object::parse_instance(&arcanum_formats::test_util::ObjectBuilder::new(arcanum_formats::object::TYPE_NPC).field("critter_flags", Value::Int32(flags)).field("critter_fatigue_damage", Value::Int32(500)).build()).unwrap();
            let mut map = crate::LoadedMap::default();
            map.mobiles.insert(arcanum_formats::map::SectorId::new(0, 0), vec![o]);
            map.npc_down(arcanum_formats::map::SectorId::new(0, 0), 0)
        };
        assert!(tired(0), "out of fatigue: down");
        assert!(!tired(OCF_UNDEAD), "undead: never down by fatigue");
    }

    #[test]
    fn some_npcs_may_not_bark() {
        let (pc, other) = (PlayerId(1), PlayerId(2));
        assert!(!bark_vetoed(0, 0, 0, false, None, pc));
        assert!(!bark_vetoed(0, 0, 0, false, Some(pc), pc), "its own leader");
        assert!(bark_vetoed(0, 0, 0, false, Some(other), pc), "another's follower");
        assert!(bark_vetoed(crate::blocking::OF_OFF, 0, 0, false, None, pc));
        for flags in [OCF_STUNNED, OCF_PARALYZED, OCF_MUTE, OCF_SLEEPING] {
            assert!(bark_vetoed(0, flags, 0, false, None, pc), "{flags:#x}");
        }
        assert!(bark_vetoed(0, 0, SPELL_NO_DAMAGE, false, None, pc) && bark_vetoed(0, 0, 0, true, None, pc));
    }

    #[test]
    fn a_stray_hit_draws_a_remark_unless_the_meant_was_a_follower() {
        let pc = PlayerId(1);
        assert!(stray_remarked(true, None, pc), "a hit on another than the foe meant");
        assert!(!stray_remarked(false, None, pc), "a miss");
        assert!(!stray_remarked(true, Some(pc), pc), "it meant its own follower");
        assert!(stray_remarked(true, Some(PlayerId(2)), pc), "another PC's follower");
    }

    #[test]
    fn a_follower_hit_by_its_leader_leaves_for_its_reasons() {
        // Columns 5-9: reaction floor 0, alignment gaps 40 / 40, level lead 5, the hit's -20.
        let row = [0, 0, 0, 0, 0, 0, 40, 40, 5, -20, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(leave_reason(&row, 10, (0, 0), (3, 3), false), 0, "stays");
        assert_eq!(leave_reason(&row, 0, (0, 0), (3, 3), false), 3, "loathes it");
        assert_eq!(leave_reason(&row, 10, (-50, 0), (3, 3), false), 1, "the leader far better");
        assert_eq!(leave_reason(&row, 10, (50, 0), (3, 3), false), 2, "the leader far worse");
        assert_eq!(leave_reason(&row, 10, (0, 0), (8, 3), false), 7, "it outranks the leader");
        assert_eq!(leave_reason(&row, -100, (0, 0), (3, 3), true), 0, "bound, forced or a master persuader");
    }

    #[test]
    fn shots_need_and_spend_their_rounds() {
        use arcanum_formats::object::{Object, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let mut prototypes = Prototypes::default();
        for (n, kind) in [(100u32, 0), (101, 1)] {
            let mut o = Object::parse_instance(&ObjectBuilder::new(TYPE_AMMO).field("ammo_type", Value::Int32(kind)).build()).unwrap();
            o.id = prototype_id(n);
            prototypes.insert(o);
        }
        let stack = |prototype, location, quantity| crate::CarriedItem { prototype, location, quantity, keys: vec![], written: 0, wear: Default::default() };
        let mut inv = crate::Inventory { items: vec![stack(100, 0, 3), stack(101, 1, 9), stack(100, 2, 4)], ..Default::default() };
        assert_eq!((ammo_count(&inv, 0, &prototypes), ammo_count(&inv, 1, &prototypes)), (7, 9));
        take_ammo(&mut inv, 0, 5, &prototypes);
        assert_eq!(inv.items.iter().map(|i| (i.location, i.quantity)).collect::<Vec<_>>(), [(1, 9), (2, 2)], "the first stack emptied and gone");
        let bow = Weapon { ammo_type: 0, ammo_per_shot: 3, range: 10, ..Default::default() };
        let archer = Fighter { weapon: Some(bow), ammo: 2, ..Default::default() };
        assert!(archer.out_of_ammo());
        assert!(!Fighter { ammo: 3, ..archer.clone() }.out_of_ammo());
        let sword = Weapon { ammo_type: NO_AMMO, ..Default::default() };
        assert!(!Fighter { weapon: Some(sword), ..Default::default() }.out_of_ammo());
    }

    #[test]
    fn a_weapon_of_flags_0x400000_is_immune_to_wear() {
        use arcanum_formats::object::{TYPE_WEAPON, Value};
        use arcanum_formats::test_util::ObjectBuilder;
        let weapon = |flags| Object::parse_instance(&ObjectBuilder::new(TYPE_WEAPON).field("flags", Value::Int32(flags)).build()).unwrap();
        assert!(immune_to_wear(&weapon(OF_WEAR_IMMUNE)));
        assert!(!immune_to_wear(&weapon(0)));
    }

    #[test]
    fn critical_misses_and_stuns_follow_the_original() {
        assert_eq!((stun_count(8), stun_count(20), stun_count(5)), (6, 0, 7));
        assert_eq!(fumble_chance(95, None), 2, "at least 2");
        assert_eq!(fumble_chance(30, None), 10, "70 / 7");
        let worn = Weapon { worn: 70, miss_chance: 3, ..Default::default() };
        assert_eq!(fumble_chance(30, Some(&worn)), 23);
        // A base of 100 always stuns a CO 0 critter (unless knocked down) and knocks down chart 0.
        let sword = Weapon { skill: SKILL_MELEE, crit_effect: 100, crit_chart: 2, ..Default::default() };
        let pc = Fighter { weapon: Some(sword.clone()), pc: true, ..Default::default() };
        let mut rng = HostRng::default();
        let npc = Fighter { crit_chart: 5, ..Default::default() };
        assert!((0..50).all(|_| critical_effects(&pc, &npc, &sword, BodyPart::Torso, &mut rng) == STUNNED | KNOCKED_OUT), "not chart 0: never knocked down; CO 0: knocked out too");
        let tough = Fighter { flags: OCF_NO_KNOCK_OUT, ..npc.clone() };
        assert!((0..20).all(|_| critical_effects(&pc, &tough, &sword, BodyPart::Torso, &mut rng) == STUNNED), "flag 0x4000000: never knocked out");
        let down = critical_effects(&pc, &Fighter::default(), &sword, BodyPart::Legs, &mut rng);
        assert_eq!(down, KNOCKED_DOWN, "knocked down: not stunned too");
        let stunned = Fighter { stunned: true, crit_chart: 5, ..Default::default() };
        assert_eq!(critical_effects(&pc, &stunned, &sword, BodyPart::Torso, &mut rng), 0);
        // A critical miss with a breakable weapon of effect 100: destroyed, and the ammunition.
        let gun = Weapon { miss_chart: 1, miss_effect: 100, ammo_type: 1, ammo_per_shot: 1, range: 10, ..Default::default() };
        let shooter = Fighter { weapon: Some(gun.clone()), ammo: 20, crit_chart: 5, ..Default::default() };
        let mut out = AttackOutcome::default();
        fumble(&shooter, &gun, &mut out, &mut rng);
        assert_eq!(out.effects & (WEAPON_DESTROYED | AMMO_LOST_ALL), WEAPON_DESTROYED | AMMO_LOST_ALL);
        assert_eq!(out.fumble_items & (WEAPON_DAMAGED | WEAPON_DROPPED), 0, "a destroyed weapon is not worn or dropped");
        // Charts 6-8 roll twice to be destroyed (Y 0: about 2 in 100, against 1 in 100).
        let destroyed = |chart: i32, rng: &mut HostRng| {
            let w = Weapon { miss_chart: chart, ..Default::default() };
            let f = Fighter { weapon: Some(w.clone()), crit_chart: 5, ..Default::default() };
            (0..40_000).filter(|_| {
                let mut out = AttackOutcome::default();
                fumble(&f, &w, &mut out, rng);
                out.effects & WEAPON_DESTROYED != 0
            }).count()
        };
        let (plain, flame) = (destroyed(1, &mut rng), destroyed(7, &mut rng));
        assert!((300..500).contains(&plain) && (650..950).contains(&flame), "{plain} / {flame}");
        // A melee base of 100 on itself: its weapon worn and knocked away, its armour worn.
        let mut out = AttackOutcome::default();
        fumble(&pc, &sword, &mut out, &mut rng);
        assert_eq!(out.fumble_items, WEAPON_DAMAGED | WEAPON_DROPPED | ARMOUR_DAMAGED);
    }

    #[test]
    fn watchers_see_ahead_in_reach_and_past_nothing() {
        use crate::pathfinding::tests::{Grid, t};
        // Facing rotation 5 (x + 1): ahead and two to each side are in view, behind is not.
        assert!(in_view(5, t(10, 10), t(15, 10)) && in_view(5, t(10, 10), t(15, 14)) && in_view(5, t(10, 10), t(10, 15)));
        assert!(!in_view(5, t(10, 10), t(5, 10)) && !in_view(5, t(10, 10), t(5, 5)));
        assert_eq!(line_between(t(0, 0), t(4, 2)), [t(1, 0), t(2, 1), t(3, 1)]);
        let open = Grid::parse(&["...................."; 20]);
        assert!(sees(&open, t(2, 5), 5, 8, t(9, 5), None));
        assert!(!sees(&open, t(2, 5), 5, 8, t(11, 5), None), "9 tiles: past PE 8");
        assert!(!sees(&open, t(2, 5), 5, 8, t(9, 5), Some(crate::prowl::Prowler { level: 9, intelligence: 8, rank: 2, ..Default::default() })), "an Expert prowler of 9 against PE 8 (52) halves it");
        let mut rows = vec!["...................."; 20];
        rows[5] = ".....#..............";
        let walled = Grid::parse(&rows);
        assert!(!sees(&walled, t(2, 5), 5, 8, t(9, 5), None), "a wall between");
        assert!(notices(&walled, Some(8), t(2, 5), 1, Some(8), t(4, 5), None), "heard 2 tiles off, the line short of the block");
        assert!(!notices(&walled, Some(8), t(4, 5), 1, Some(8), t(6, 5), None), "past blocking ground: 8 off the reach of 5");
        // A wall on the edge crossed hides even a target next to it (0x4af45a).
        let mut edged = Grid::parse(&["...................."; 20]);
        edged.3.edges.insert(t(5, 5), 1 << 5);
        assert!(!sees(&edged, t(5, 5), 5, 8, t(6, 5), None), "the wall faces the step");
        assert!(sees(&edged, t(5, 4), 5, 8, t(6, 4), None), "the row beside it");
        // Hearing (0x4af470): PE 10 hears 6 tiles; a wall crossed costs 2 (0x4400ba).
        assert!(hears(&edged, t(1, 8), 10, t(7, 8), None) && !hears(&edged, t(1, 8), 10, t(8, 8), None), "6 tiles in the open");
        assert!(hears(&edged, t(2, 5), 10, t(6, 5), None) && !hears(&edged, t(1, 5), 10, t(6, 5), None), "4 past the wall");
        assert_eq!(crate::sound_cost(&edged, t(5, 6), t(6, 4)), 0, "round the wall's corner: the cheaper route");
    }

    #[test]
    fn violent_deaths_go_by_where_the_hit_landed() {
        assert_eq!([crate::death_anim(20, 1), crate::death_anim(21, 1), crate::death_anim(21, 3), crate::death_anim(21, 0), crate::death_anim(30, 2)], [7, 17, 19, 18, 18]);
    }

    #[test]
    fn critical_hits_injure_by_body_part() {
        assert_eq!([1, 70, 71, 85, 86, 95, 96, 100].map(BodyPart::of_roll), [BodyPart::Torso, BodyPart::Torso, BodyPart::Legs, BodyPart::Legs, BodyPart::Arms, BodyPart::Arms, BodyPart::Head, BodyPart::Head]);
        assert_eq!((injury_difficulty(OCF_BLINDED), injury_difficulty(OCF_CRIPPLED_ARM), injury_difficulty(OCF_CRIPPLED_ARM | OCF_CRIPPLED_ARMS | OCF_BLINDED)), (30, 20, 80));
        let sword = Weapon { skill: SKILL_MELEE, crit_effect: 100, crit_chart: 2, ..Default::default() };
        let pc = Fighter { weapon: Some(sword.clone()), pc: true, ..Default::default() };
        let npc = Fighter::default();
        let mut rng = HostRng::default();
        // A base of 100: every multiplier roll and injury roll passes; CO 0 never saves.
        assert!((0..50).all(|_| critical_multiplier(&pc, &npc, &sword, &mut rng) == 300));
        assert!((0..50).all(|_| critical_multiplier(&npc, &pc, &sword, &mut rng) == 150), "an NPC's on a PC");
        let mut seen = 0;
        for _ in 0..400 {
            let part = BodyPart::of_roll(rng.range(1, 100));
            seen |= critical_injuries(&pc, &npc, &sword, part, &mut rng);
        }
        assert_eq!(seen, OCF_BLINDED | OCF_CRIPPLED_ARM | OCF_CRIPPLED_LEGS, "both arms only after one");
        let one_arm = Fighter { injuries: OCF_CRIPPLED_ARM, ..npc.clone() };
        assert!((0..400).any(|_| critical_injuries(&pc, &one_arm, &sword, BodyPart::Arms, &mut rng) == OCF_CRIPPLED_ARMS));
        let undead = Fighter { undead: true, ..npc.clone() };
        assert!((0..100).all(|_| critical_injuries(&pc, &undead, &sword, BodyPart::Legs, &mut rng) == 0));
        let tough = Fighter { constitution: 200, ..npc };
        assert!((0..100).all(|_| critical_injuries(&pc, &tough, &sword, BodyPart::Legs, &mut rng) == 0), "always saves");
    }

    #[test]
    fn criticals_knock_weapons_from_hands_but_never_a_pcs_by_an_npc() {
        let sword = Weapon { skill: SKILL_MELEE, crit_effect: 100, damage: [(1, 1), (0, 0), (0, 0), (0, 0), (0, 0)], ..Default::default() };
        let armed = |pc| Fighter { levels: vec![(SKILL_MELEE, 20)], weapon: Some(sword.clone()), pc, ..Default::default() };
        let mut rng = HostRng::default();
        let crits = |a: &Fighter, d: &Fighter, rng: &mut HostRng| (0..400).map(|_| resolve(a, d, rng)).filter(|o| o.critical).collect::<Vec<_>>();
        let pc_on_npc = crits(&armed(true), &armed(false), &mut rng);
        assert!(!pc_on_npc.is_empty() && pc_on_npc.iter().all(|o| o.items & WEAPON_DROPPED != 0), "a base of 100: always");
        assert!(crits(&armed(false), &armed(true), &mut rng).iter().all(|o| o.items & WEAPON_DROPPED == 0), "an NPC's on a PC");
        assert!(crits(&armed(true), &Fighter::default(), &mut rng).iter().all(|o| o.items & WEAPON_DROPPED == 0), "bare hands");
    }

    #[test]
    fn attacks_hit_miss_and_resistance_cuts_damage() {
        let sword = Weapon { skill: SKILL_MELEE, damage: [(4, 4), (0, 0), (0, 0), (0, 0), (0, 0)], ..Default::default() };
        let master = Fighter { levels: vec![(SKILL_MELEE, 20)], weapon: Some(sword), ..Default::default() };
        let armoured = Fighter { resistances: [50, 0, 0, 0, 0], ..Default::default() };
        let mut rng = HostRng::default();
        let mut hits = 0;
        for _ in 0..200 {
            let out = resolve(&master, &armoured, &mut rng);
            if out.hit {
                hits += 1;
                assert!(out.hp == 2 || out.critical, "4 less half: {out:?}");
            }
        }
        assert!(hits > 160, "95 %: {hits}");
        let novice = Fighter { levels: vec![(SKILL_MELEE, 0)], ..Default::default() };
        let misses = (0..200).filter(|_| !resolve(&novice, &armoured, &mut rng).hit).count();
        assert!(misses > 100, "25 %: {misses}");
    }

    #[test]
    fn undead_and_machines_take_no_poison_and_the_spell_no_damage() {
        let hit = || [4, 6, 1, 2, 3];
        let mut d = hit();
        shield_damage(&mut d, OCF_UNDEAD, false);
        assert_eq!(d, [4, 0, 1, 2, 3]);
        let mut d = hit();
        shield_damage(&mut d, OCF_MECHANICAL, true);
        assert_eq!(d, [4, 0, 1, 2, 3], "the poison test skips the 0x10000 one (0x4b442c)");
        let mut d = hit();
        shield_damage(&mut d, 0, true);
        assert_eq!(d, [0; DAMAGE_TYPES]);
    }

    #[test]
    fn armour_class_makes_hits_harder() {
        let master = Fighter { levels: vec![(SKILL_MELEE, 20)], ..Default::default() };
        let mut rng = HostRng::default();
        let hits = |d: &Fighter, rng: &mut HostRng| (0..2000).filter(|_| { let o = resolve(&master, d, rng); o.hit || o.dodged }).count();
        let bare = hits(&Fighter::default(), &mut rng);
        let plated = hits(&Fighter { armour_class: 90, ..Default::default() }, &mut rng);
        // A chance of 125 (no cap): always against nothing; AC 90: 45 × 125 / 100 = 56 harder, 69 %.
        assert!(bare == 2000 && (1280..1500).contains(&plated), "{bare} {plated}");
    }

    #[test]
    fn far_and_covered_shots_are_harder() {
        assert_eq!(missile_difficulty(4, 8, false, 0), 0, "within half its Perception");
        assert_eq!(missile_difficulty(10, 8, false, 0), 30, "6 tiles past: 5 each");
        assert_eq!(missile_difficulty(10, 8, true, 0), 0, "a Master");
        assert_eq!(missile_difficulty(4, 8, false, 70), 70, "a hedge (50) and a low wall (20)");
        // Critters on the line: the game's 0x4030 (cover, 20); one not seen through 50.
        let line = line_between(TileLoc { x: 0, y: 0 }, TileLoc { x: 6, y: 0 });
        let living = [(TileLoc { x: 2, y: 0 }, PC_FLAGS), (TileLoc { x: 4, y: 0 }, crate::blocking::OF_SHOOT_THROUGH), (TileLoc { x: 4, y: 1 }, PC_FLAGS), (TileLoc { x: 6, y: 0 }, PC_FLAGS)];
        assert_eq!(line_critters_cost(&line, living.into_iter()), 70, "off the line and the target's tile left out");
    }

    #[test]
    fn helpless_targets_are_easier_to_hit() {
        assert_eq!(helpless_by_state(0, false, false, false), None);
        assert_eq!(helpless_by_state(OCF_PARALYZED, false, false, false), Some(50), "paralyzed");
        assert_eq!(helpless_by_state(0, true, false, false), Some(50), "down");
        assert_eq!(helpless_by_state(OCF_STUNNED, false, false, false), Some(30), "stunned by flag");
        assert_eq!(helpless_by_state(0, false, false, true), Some(30), "stunned by a blow");
        assert_eq!(helpless_by_state(OCF_STUNNED | OCF_SLEEPING, false, false, false), Some(30), "stunned is tested before asleep");
        assert_eq!(helpless_by_state(OCF_SLEEPING, false, false, false), Some(50), "asleep");
        let novice = Fighter { levels: vec![(SKILL_MELEE, 0)], ..Default::default() };
        let mut rng = HostRng::default();
        let hits = |d: &Fighter, rng: &mut HostRng| (0..2000).filter(|_| { let o = resolve(&novice, d, rng); o.hit || o.dodged }).count();
        let (awake, unaware, down) = (hits(&Fighter::default(), &mut rng), hits(&Fighter { helpless: HELPLESS_UNAWARE, ..Default::default() }, &mut rng), hits(&Fighter { helpless: HELPLESS_DOWN, ..Default::default() }, &mut rng));
        // 25 %, 55 %, 75 %.
        assert!((400..600).contains(&awake) && (1000..1200).contains(&unaware) && (1400..1600).contains(&down), "{awake} {unaware} {down}");
    }

    #[test]
    fn fire_and_flagged_weapons_wear_armour_on_every_hit() {
        let master = |w: Weapon| Fighter { levels: vec![(SKILL_MELEE, 20)], weapon: Some(Weapon { skill: SKILL_MELEE, ..w }), ..Default::default() };
        let target = Fighter::default();
        let mut rng = HostRng::default();
        let hits = |f: &Fighter, rng: &mut HostRng| (0..200).map(|_| resolve(f, &target, rng)).filter(|o| o.hit && !o.critical).collect::<Vec<_>>();
        let plain = master(Weapon { damage: [(4, 4), (0, 0), (0, 0), (0, 0), (0, 0)], ..Default::default() });
        assert!(hits(&plain, &mut rng).iter().all(|o| o.items & ARMOUR_DAMAGED == 0), "an ordinary hit");
        let torch = master(Weapon { damage: [(0, 0), (0, 0), (0, 0), (2, 2), (0, 0)], ..Default::default() });
        assert!(hits(&torch, &mut rng).iter().all(|o| o.items & ARMOUR_DAMAGED != 0), "fire damage");
        let flagged = master(Weapon { damage: [(4, 4), (0, 0), (0, 0), (0, 0), (0, 0)], wears_armour: true, ..Default::default() });
        assert!(hits(&flagged, &mut rng).iter().all(|o| o.items & ARMOUR_DAMAGED != 0), "weapon_flags 0x100");
    }
}
