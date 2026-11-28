//! Traps: spotting them and disarming them (Arcanum.exe 0x4bbe40, 0x4bc7b0, 0x4bc220; the trap
//! itself is sprung by the script runner, [`crate::script`]).
//!
//! - **A trap** is a trap script ([`crate::is_trap`]) attached to an object's use (event 1).
//! - **Spotting** (0x4bbe40, Spot Trap, skill 7, 0x4c7160 case 7): when a PC uses an object
//!   whose trap is not spotted. The chance is the skill's ([`spot_chance`], 0x4c62e0: 4 a level);
//!   a Master who fails rolls again; a success below Expert on a magical trap fails. Spotted,
//!   the object gets flag [`OF_TRAP_SPOTTED`] (0x4bc090) and the PC is told `mes/trap.mes` 0.
//! - **Disarming** (Disarm Traps, skill 15, 0x4c7160 case 0xf → 0x4bc7b0): only a spotted trap.
//!   Its chance is [`crate::tech_chance`]'s (4 a level + 20, none at level 0).
//!   The roll is the lock pick's ([`crate::roll_pick`]; a door's or container's trap has no
//!   difficulty, 0x4c8430 adds `trap_difficulty` only for trap objects); a Master who fails
//!   without a critical failure rolls again. Success removes the trap (0x4bc220: the
//!   attachment's script number becomes 0); a critical failure springs it on the PC (0x441980
//!   event 1). `mes/skill_ui.mes` 560 / 561 tell the result.
//!
//! - **Spotting while walking** (0x427110, each step): a walker about to step onto a tile with
//!   trap objects (type 17, list mask 0x20000) rolls Spot Trap for each it does not know
//!   (0x4bbfe0), in turn, once a tile; the first spotted stops the walk before the step
//!   (0x4272b3). A PC or a follower marks it spotted (0x4bc090: a leaderless NPC does not), and a
//!   PC is told. Leaderless NPCs know every trap a PC did not set, so roll only for those.
//!   Doors and containers are not checked this way.
//!
//! - **Stepping onto a trap object** (0x4305d0, 0x430ca8-0x430d0f): the walk ends on its tile
//!   and the walker, PC or NPC, uses it (event 1) ([`spring_stepped_traps`]); not one of
//!   `spell_flags` 0x2 / 0x4. NPCs without a leader know every trap a PC did not set, so spring
//!   them; followers roll their own Spot Trap first.
//!
//! Not built: the recovered trap
//! parts on a critical success for an Expert or Master (0x4bc9c0, table
//! 0x5b5f80; the critical success roll 0x4c81a0 is not decoded); setting traps (0x4bc480);
//! the eye candy (`rules/TrapEyeCandy.mes` N × 10 + 2 on a spotted trap); scenery and trap
//! objects as targets (doors and containers only).

use bevy_ecs::prelude::*;

use crate::{LedgerDelta, LoadedMap, LockTarget, Outbox, PlayerControlled, PlayerId, Position, Sheet, SimRules, WorldState};

/// `flags`: the object's trap has been spotted (0x4bc090).
pub const OF_TRAP_SPOTTED: i32 = 0x200_0000;
/// `flags`: a trap a PC set (0x4bc480): PCs know it.
pub const OF_TRAP_PLACED_BY_PC: i32 = 0x100_0000;
/// The Spot Trap and Disarm Traps skills (`mes/skill.mes` order).
pub const SKILL_SPOT_TRAP: usize = 7;
pub const SKILL_DISARM_TRAPS: usize = 15;
/// The trap texts.
pub const TRAP_MES: &str = "mes/trap.mes";
/// `mes/skill_ui.mes`: disarmed, failed.
pub const DISARM_SUCCEEDED: u32 = 560;
pub const DISARM_FAILED: u32 = 561;
const EXPERT: u8 = 2;
const MASTER: u8 = 3;

/// Spot Trap's chance (0x4c62e0, a basic skill: 0x4c71b9 sends skills under 12 there): 4 a
/// level (0x4c6350, table 0x4c63dc entry 7), + 10 at Intelligence 20, at most 95 (flag 0x884
/// holds 0x800).
pub fn spot_chance(level: i32, intelligence: i32) -> i32 {
    (level.max(0) * 4 + crate::bright_bonus(intelligence)).clamp(0, crate::MAX_PICK_CHANCE)
}

/// A Spot Trap roll (0x4bbe40 → 0x4c7160 case 7).
/// `light`: the luminance where the trap is (the dark adds to the roll below Apprentice,
/// [`crate::light::skill_dark`]).
pub fn roll_spot(rules: &arcanum_rules::RulesData, sheet: &arcanum_rules::CharacterSheet, magical: bool, light: Option<i32>, rng: &mut crate::HostRng) -> bool {
    roll_spot_at(rules.skill_level(sheet, SKILL_SPOT_TRAP), rules.stats(sheet)[4], sheet.training[SKILL_SPOT_TRAP], magical, light, rng)
}

/// The same for a skill `level`, `intelligence` and `training` (an NPC's).
pub fn roll_spot_at(level: i32, intelligence: i32, training: u8, magical: bool, light: Option<i32>, rng: &mut crate::HostRng) -> bool {
    let chance = spot_chance(level, intelligence);
    let dark = crate::light::skill_dark(training, 1, light);
    let mut spotted = rng.range(1, 100) + dark <= chance;
    if !spotted && training >= MASTER {
        spotted = rng.range(1, 100) <= chance;
    }
    spotted && !(magical && training < EXPERT)
}

/// A Disarm Traps roll (0x4c7160 case 0xf): success, failure, or a critical failure
/// ([`crate::PickOutcome::Jammed`]).
/// `light`: the luminance where the trap is ([`crate::light::skill_dark`], below Apprentice).
pub fn roll_disarm(chance: i32, training: u8, light: Option<i32>, rng: &mut crate::HostRng) -> crate::PickOutcome {
    let dark = crate::light::skill_dark(training, 1, light);
    let outcome = crate::roll_pick(chance, dark, rng);
    if outcome == crate::PickOutcome::Failure && training >= MASTER { crate::roll_pick(chance, dark, rng) } else { outcome }
}

/// The tile a walker last rolled Spot Trap for.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpotChecked(pub arcanum_formats::map::TileLoc);

/// Who rolls Spot Trap while walking: a PC, a follower (an NPC with a leader) or another NPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spotter {
    Pc,
    Follower,
    Npc,
}

/// Whether `who` knows a trap with object `flags` (0x4bbfe0): a PC or a follower once it is
/// spotted or a PC set it; another NPC unless a PC set it.
fn knows(who: Spotter, flags: i32) -> bool {
    match who {
        Spotter::Pc | Spotter::Follower => flags & (OF_TRAP_SPOTTED | OF_TRAP_PLACED_BY_PC) != 0,
        Spotter::Npc => flags & OF_TRAP_PLACED_BY_PC == 0,
    }
}

/// The trap objects (type 17) on `at` that `who` does not know, in the list's order: sector,
/// index, magical.
fn unknown_traps(map: &LoadedMap, host: &mut crate::script_host::ScriptHost, at: arcanum_formats::map::TileLoc, who: Spotter) -> Vec<(arcanum_formats::map::SectorId, u32, bool)> {
    let sector = at.sector();
    let candidates: Vec<(u32, u32)> = map
        .mobiles
        .get(&sector)
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, o)| o.object_type == arcanum_formats::object::TYPE_TRAP && crate::object_location(o) == Some(at))
        .filter(|(_, o)| {
            let flags = map.prototypes.int(o, "flags").unwrap_or(0);
            flags & (crate::OF_OFF | crate::OF_DESTROYED) == 0 && !knows(who, flags)
        })
        .map(|(i, _)| (i as u32, map.attachment(crate::ScriptObject::Mobile { sector, index: i as u32 }, crate::EVENT_USE).number))
        .collect();
    candidates
        .into_iter()
        .filter_map(|(index, number)| {
            let flags = host.script_flags(number)?;
            crate::is_trap(number, &arcanum_formats::script::Script { description: String::new(), flags, lines: vec![] }).then_some((sector, index, flags & crate::TRAP_MAGICAL != 0))
        })
        .collect()
}

type SpotterQuery<'a> = (
    Entity,
    Option<&'a PlayerControlled>,
    Option<&'a crate::NpcSource>,
    Option<&'a crate::follow::Following>,
    &'a mut crate::Walking,
    Option<&'a Sheet>,
    Option<&'a SpotChecked>,
);

/// A walker (a PC or an NPC) rolls Spot Trap for each trap object it does not know on the
/// tile its next step enters, once a tile, until one is spotted (0x427110). The spotted trap is
/// marked for a PC or a follower (0x4bc090), the PC is told, and the walk stops where the
/// walker stands.
#[allow(clippy::too_many_arguments)]
pub fn spot_traps_walking(
    mut commands: Commands,
    mut map: ResMut<LoadedMap>,
    mut world: ResMut<WorldState>,
    mut host: crate::script_host::ScriptHost,
    rules: Res<SimRules>,
    session: Res<crate::Session>,
    mut outbox: ResMut<Outbox>,
    mut walkers: Query<SpotterQuery>,
    idle: Query<Entity, (With<SpotChecked>, Without<crate::Walking>)>,
    clock: Res<crate::GameClock>,
) {
    // A walker that stopped rolls again on its next walk.
    for entity in &idle {
        commands.entity(entity).remove::<SpotChecked>();
    }
    for (entity, controlled, npc, following, mut walking, sheet, checked) in &mut walkers {
        let who = match (controlled, npc, following) {
            (Some(_), _, _) if sheet.is_some() => Spotter::Pc,
            (None, Some(_), Some(_)) => Spotter::Follower,
            (None, Some(_), None) => Spotter::Npc,
            _ => continue,
        };
        let Some(&next) = walking.steps.front() else { continue };
        if checked.is_some_and(|c| c.0 == next) {
            continue;
        }
        commands.entity(entity).insert(SpotChecked(next));
        let light = map.luminance_at(next, clock.now);
        // An NPC's own Spot Trap and Intelligence (0x4bbe40 takes types 15 and 16).
        let npc_skill = npc.and_then(|n| map.mobiles.get(&n.sector)?.get(n.index as usize)).map(|o| {
            let (level, training) = crate::combat::npc_skill(o, &map.prototypes, SKILL_SPOT_TRAP);
            (level, crate::combat::npc_stat(o, &map.prototypes, 4), training)
        });
        let spotted = unknown_traps(&map, &mut host, next, who).into_iter().find(|&(_, _, magical)| match (sheet, npc_skill) {
            (Some(sheet), _) if who == Spotter::Pc => roll_spot(&rules.0, &sheet.0, magical, light, host.rng()),
            (_, Some((level, intelligence, training))) => roll_spot_at(level, intelligence, training, magical, light, host.rng()),
            _ => false,
        });
        let Some((sector, index, _)) = spotted else { continue };
        walking.steps.clear();
        if who == Spotter::Npc {
            continue;
        }
        let flags = map.object_flags(sector, index, true).unwrap_or(0) | OF_TRAP_SPOTTED;
        map.set_object_flags(sector, index, true, flags);
        crate::ground::save_change(&mut world, &map, sector);
        crate::ground::broadcast(&session, &mut outbox, LedgerDelta::ObjectFlags { sector, index, mobile: true, flags });
        if let (Some(controlled), Some(text)) = (controlled, host.trap_line(crate::TRAP_LINE_SPOTTED)) {
            outbox.0.push((controlled.0, LedgerDelta::ScriptMessage { text }));
        }
    }
}

/// Walkers that stepped onto a trap object this tick, and the tile ([`crate::walk`] stops them
/// there).
#[derive(Resource, Debug, Default)]
pub struct SteppedOnTraps(pub Vec<(crate::ScriptObject, arcanum_formats::map::TileLoc)>);

/// `spell_flags` 0x2 / 0x4: a walker of either springs no trap object it steps on (0x430ca8;
/// the spells not identified).
pub const OSF_SPARES_TRAPS: i32 = 0x6;

/// The first trap object (type 17) on tile `at`, not off or destroyed (0x430cba: list mask
/// 0x20000, its first object only).
pub fn trap_object_at(map: &LoadedMap, at: arcanum_formats::map::TileLoc) -> Option<(arcanum_formats::map::SectorId, u32)> {
    let sector = at.sector();
    let index = map.mobiles.get(&sector)?.iter().position(|o| {
        o.object_type == arcanum_formats::object::TYPE_TRAP && crate::object_location(o) == Some(at) && map.prototypes.int(o, "flags").unwrap_or(0) & (crate::OF_OFF | crate::OF_DESTROYED) == 0
    })?;
    Some((sector, index as u32))
}

/// A walker that stepped onto a trap object uses it (0x4305d0 at 0x430ce3: event 1, the walker
/// the triggerer; its answer ignored): the trap's script rolls the walker's Spot Trap if it does
/// not know the trap, a known trap or a failed roll springs it, and a sprung trap object is then
/// destroyed ([`crate::run_hook`]). An NPC's runs for the first player (scripts run on the host).
pub fn spring_stepped_traps(mut stepped: ResMut<SteppedOnTraps>, mut map: ResMut<LoadedMap>, mut world: ResMut<WorldState>, mut host: crate::script_host::ScriptHost, mut outbox: ResMut<Outbox>) {
    for (walker, at) in std::mem::take(&mut stepped.0) {
        let player = match walker {
            crate::ScriptObject::Pc(p) => Some(p),
            _ => world.0.as_ref().and_then(|w| w.players.keys().next().copied()),
        };
        if let (Some(player), Some((sector, index))) = (player, trap_object_at(&map, at)) {
            host.hook_by(&mut world, &mut map, &mut outbox, crate::ScriptObject::Mobile { sector, index }, crate::EVENT_USE, player, walker);
        }
    }
}

/// Players' requests to disarm a trap: the target and the move sent with it.
#[derive(Resource, Debug, Default)]
pub struct DisarmRequests(pub Vec<(PlayerId, LockTarget, u32)>);

/// A PC's disarm waiting for it to walk in range.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingDisarm {
    pub target: LockTarget,
    pub seq: u32,
}

/// The script object a door or container is.
pub fn target_object(target: LockTarget) -> Option<crate::ScriptObject> {
    match target {
        LockTarget::Door(id) => Some(crate::ScriptObject::Map(id)),
        LockTarget::Container(id) => crate::ScriptObject::ground(id),
    }
}

type DisarmerQuery<'a> = (Entity, &'a PlayerControlled, &'a Position, Option<&'a crate::Walking>, Option<&'a PendingDisarm>, &'a Sheet);

/// Takes disarm requests (each becomes the PC's [`PendingDisarm`]), then disarms the traps whose
/// PC is within [`crate::USE_RANGE`]; a request whose PC stopped out of range is dropped.
#[allow(clippy::too_many_arguments)]
pub fn disarm_traps(
    mut commands: Commands,
    mut requests: ResMut<DisarmRequests>,
    mut map: ResMut<LoadedMap>,
    mut world: ResMut<WorldState>,
    mut host: crate::script_host::ScriptHost,
    rules: Res<SimRules>,
    mut outbox: ResMut<Outbox>,
    pcs: Query<DisarmerQuery>,
    clock: Res<crate::GameClock>,
) {
    let mut fresh = std::collections::HashMap::new();
    for (player, target, seq) in requests.0.drain(..) {
        if let Some((entity, ..)) = pcs.iter().find(|pc| pc.1.0 == player) {
            fresh.insert(entity, PendingDisarm { target, seq });
        }
    }
    for (entity, controlled, position, walking, pending, sheet) in &pcs {
        let Some(want) = fresh.get(&entity).copied().or(pending.copied()) else { continue };
        let (Some(location), Some(object)) = (map.lock_location(want.target), target_object(want.target)) else {
            commands.entity(entity).remove::<PendingDisarm>();
            continue;
        };
        if !crate::within_radius(position.0, location, crate::USE_RANGE) {
            if walking.is_some() {
                commands.entity(entity).insert(want);
            } else {
                commands.entity(entity).remove::<PendingDisarm>();
            }
            continue;
        }
        commands.entity(entity).remove::<PendingDisarm>();
        let player = controlled.0;
        let attachment = map.attachment(object, crate::EVENT_USE);
        let place = match object {
            crate::ScriptObject::Map(id) => Some((id.sector, id.index, false)),
            crate::ScriptObject::Mobile { sector, index } => Some((sector, index, true)),
            _ => None,
        };
        let spotted = place.and_then(|(s, i, m)| map.object_flags(s, i, m)).is_some_and(|f| f & OF_TRAP_SPOTTED != 0);
        if !spotted || !host.is_trap(attachment.number) {
            outbox.0.push((player, LedgerDelta::SkillResult { message: DISARM_FAILED }));
            continue;
        }
        let sheet = &sheet.0;
        let chance = crate::tech_chance(crate::TECH_DISARM_TRAPS, rules.0.skill_level(sheet, SKILL_DISARM_TRAPS), rules.0.stats(sheet)[4]);
        let mut outcome = roll_disarm(chance, sheet.training[SKILL_DISARM_TRAPS], map.luminance_at(location, clock.now), host.rng());
        // A waiting "critical success at Disarm traps" (0x4c737c) makes it succeed.
        if crate::take_fate(&mut world, host.changed_sheets(), player, crate::FATE_DISARM) {
            outcome = crate::PickOutcome::Success;
        }
        match outcome {
            crate::PickOutcome::Success => {
                map.set_attachment(object, crate::EVENT_USE, crate::Attachment { number: 0, ..attachment });
                if let Some((sector, ..)) = place {
                    crate::ground::save_change(&mut world, &map, sector);
                }
                outbox.0.push((player, LedgerDelta::SkillResult { message: DISARM_SUCCEEDED }));
            }
            crate::PickOutcome::Failure => outbox.0.push((player, LedgerDelta::SkillResult { message: DISARM_FAILED })),
            crate::PickOutcome::Jammed => {
                outbox.0.push((player, LedgerDelta::SkillResult { message: DISARM_FAILED }));
                host.hook(&mut world, &mut map, &mut outbox, object, crate::EVENT_USE, player);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(level_points: i32, training: u8, skill: usize) -> arcanum_rules::CharacterSheet {
        let mut s = arcanum_rules::RulesData::default().new_sheet();
        s.skills[skill] = level_points;
        s.training[skill] = training;
        s.base = [20, 20, 20, 20, 8, 20, 20, 20];
        s
    }

    #[test]
    fn spotting_follows_the_skill_and_magic_needs_an_expert() {
        let rules = arcanum_rules::RulesData::default();
        let mut rng = crate::HostRng::default();
        let blind = sheet(0, 0, SKILL_SPOT_TRAP);
        assert!((0..200).all(|_| !roll_spot(&rules, &blind, false, None, &mut rng)), "no skill: never");
        let keen = sheet(5, 0, SKILL_SPOT_TRAP);
        let seen = (0..1000).filter(|_| roll_spot(&rules, &keen, false, None, &mut rng)).count();
        assert!((740..860).contains(&seen), "level 20: 4 a level, 80 %, {seen}");
        assert_eq!((spot_chance(10, 8), spot_chance(10, 20), spot_chance(25, 20)), (40, 50, 95));
        assert!((0..200).all(|_| !roll_spot(&rules, &keen, true, None, &mut rng)), "magical: not below Expert");
        let expert = sheet(5, EXPERT, SKILL_SPOT_TRAP);
        assert!((0..200).any(|_| roll_spot(&rules, &expert, true, None, &mut rng)));
    }

    #[test]
    fn a_master_disarms_more_often() {
        let mut rng = crate::HostRng::default();
        let won = |training, rng: &mut crate::HostRng| (0..2000).filter(|_| roll_disarm(50, training, None, rng) == crate::PickOutcome::Success).count();
        let (plain, master) = (won(0, &mut rng), won(MASTER, &mut rng));
        assert!((900..1100).contains(&plain), "{plain}");
        assert!(master > plain + 300, "{master} vs {plain}");
    }
}
