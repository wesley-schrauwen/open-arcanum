//! Casting spells (Arcanum.exe 0x57bc70 / 0x57c110 / 0x4564e0 / 0x4508a0 / 0x450420): for now
//! the Unlocking Cantrip on a door or container, and the healing spells and Resurrect on a
//! critter ([`cast_on_critters`]).
//!
//! - **On critters** (`rules/spelllist.mes`): Minor Healing (60, 5-30 by aptitude, a hurt
//!   living target), Halt Poison (61, 50 off the level, [`crate::poison`]), Major Healing (62, 40-80, and scars and crippling),
//!   Resurrect (64, a dead target not the caster, `Full | Resurrect`; a PC or NPC gets up with
//!   15 %, owner's call).
//!
//! - **Who may cast** (0x57bc70, 0x4564e0): a PC who knows the spell (its college's count,
//!   [`arcanum_rules::CharacterSheet::spells`], is past the spell's place in it) with an
//!   Intelligence of at least [`MIN_INTELLIGENCE`] (0x4502e0 returns 5 for every spell; the
//!   list's 6-18 by level is a comment only).
//! - **Cost** (0x4508a0 → 0x450420): the spell's `Cost:` in fatigue, refused with "Not enough
//!   Energy." (`mes/spell.mes` 600) when it would take the PC below [`LOWEST_FATIGUE`]. Spent as
//!   fatigue damage ([`arcanum_rules::CharacterSheet::fatigue_damage`]), saved and shown on the
//!   vial.
//! - **Range**: the list's default is "max visible"; [`CAST_RANGE`] stands in for it.
//! - **The Unlocking Cantrip** (spell 1, `rules/spelllist.mes` 1050-1057): any portal or
//!   container, locked or not; clears the locked bit (`OPF_LOCKED` / `OCOF_LOCKED`), leaves a
//!   jam and does not open it.
//! - **Recovery** (0x45d5a0 schedules a time event 80 000 game ms after fatigue is spent; its
//!   handler 0x45e610 heals): every [`RECOVERY_MS`] the fatigue damage drops by the heal rate
//!   (stat 12, `(CN + 1) / 3`), down to 0. (0x45e910's three times the rate is resting,
//!   [`crate::rest`].)
//! - **Unconscious** (0x45d800): at fatigue 0 or below the PC casts nothing
//!   ([`crate::unconscious`]).
//!
//! Not built: the cast animation and its eye candy and sounds (`SpellEyeCandy.mes`: cast art
//! 10, sound 9020; destination art 16, sound 9025); the charge waiting for the animation; mana
//! stored in worn items, drawn first; line of sight ("You cannot see the target.", 604); the
//! tech-aptitude roll (read as dead code); other spells; NPCs' poison; mechanical targets.

use arcanum_rules::{CharacterSheet, SPELLS_PER_COLLEGE};
use bevy_ecs::prelude::{Commands, Component, Entity, Query, Res, ResMut, Resource};

use crate::{LedgerDelta, LoadedMap, LockTarget, NetId, Outbox, PlayerControlled, PlayerId, Position, Session, Sheet, SimRules, WorldState, pc_status};

/// The Unlocking Cantrip.
pub const SPELL_UNLOCKING_CANTRIP: usize = 1;
/// Intelligence needed for any spell (0x4502e0).
pub const MIN_INTELLIGENCE: i32 = 5;
/// The lowest fatigue a cast may leave (0x450420 refuses below -14).
pub const LOWEST_FATIGUE: i32 = -14;
/// How far a spell reaches, in tiles: the list's "max visible" (builder's call, about the
/// 800x400 view's half width).
pub const CAST_RANGE: u32 = 15;
/// `mes/spell.mes`: "Not enough Energy."
pub const NOT_ENOUGH_ENERGY: u32 = 600;
/// Game milliseconds between fatigue recoveries (0x45e820's 80 000).
pub const RECOVERY_MS: u64 = 80_000;

/// Fatigue damage after `steps` recoveries at `heal_rate` (0x45e610: the rate each).
pub fn recovered(damage: i32, heal_rate: i32, steps: u64) -> i32 {
    (damage as i64 - heal_rate as i64 * steps as i64).max(0) as i32
}

/// When a PC's next fatigue recovery is due, in game milliseconds.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FatigueRecovery {
    pub due: u64,
}

pub fn game_ms(time: crate::GameTime) -> u64 {
    time.days as u64 * crate::DAY_MS as u64 + time.ms as u64
}

/// Heals the PCs' spent fatigue as game time passes: see the module docs.
#[allow(clippy::too_many_arguments)]
pub fn recover_fatigue(
    mut commands: Commands,
    clock: Res<crate::GameClock>,
    mut world: ResMut<WorldState>,
    session: Res<Session>,
    rules: Res<SimRules>,
    mut outbox: ResMut<Outbox>,
    mut pcs: Query<(Entity, &NetId, &PlayerControlled, &mut Sheet, Option<&FatigueRecovery>)>,
) {
    let now = game_ms(clock.now);
    for (entity, &net_id, player, mut sheet, recovery) in &mut pcs {
        if sheet.0.fatigue_damage <= 0 {
            if recovery.is_some() {
                commands.entity(entity).remove::<FatigueRecovery>();
            }
            continue;
        }
        let Some(&FatigueRecovery { due }) = recovery else {
            commands.entity(entity).insert(FatigueRecovery { due: now + RECOVERY_MS });
            continue;
        };
        if now < due {
            continue;
        }
        let steps = 1 + (now - due) / RECOVERY_MS;
        sheet.0.fatigue_damage = recovered(sheet.0.fatigue_damage, rules.0.derived(&sheet.0).heal_rate, steps);
        commands.entity(entity).insert(FatigueRecovery { due: due + steps * RECOVERY_MS });
        if let Some(saved) = world.0.as_mut().and_then(|w| w.players.get_mut(&player.0)) {
            saved.blueprint.sheet = sheet.0.clone();
        }
        crate::ground::broadcast(&session, &mut outbox, LedgerDelta::PcStatus { net_id, status: pc_status(&rules.0, &sheet.0) });
    }
}

/// Whether `sheet` knows `spell`: spells are known in order within a college.
pub fn knows_spell(sheet: &CharacterSheet, spell: usize) -> bool {
    sheet.spells.get(spell / SPELLS_PER_COLLEGE).is_some_and(|&known| known > (spell % SPELLS_PER_COLLEGE) as i32)
}

/// Whether fatigue `current` pays `cost` (0x450420).
pub fn affords(current: i32, cost: i32) -> bool {
    current - cost >= LOWEST_FATIGUE
}

/// Why a cast did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastRefusal {
    /// Not known, too dim, unconscious, no such target or out of range: nothing is said.
    Silent,
    /// "Not enough Energy."
    NoEnergy,
}

/// Checks a cast of `spell` by `sheet` with fatigue `current`; `Ok` is the cost.
pub fn cast_check(rules: &arcanum_rules::RulesData, sheet: &CharacterSheet, current: i32, spell: usize) -> Result<i32, CastRefusal> {
    if !knows_spell(sheet, spell) || rules.stats(sheet)[4] < MIN_INTELLIGENCE || current < 1 {
        return Err(CastRefusal::Silent);
    }
    let cost = rules.spells.spells.get(spell).map_or(0, |s| s.cost);
    if !affords(current, cost) {
        return Err(CastRefusal::NoEnergy);
    }
    Ok(cost)
}

/// Casts taken from the inbox by `handle_intents`: the player, the spell and its target (a door
/// or container; `critters`: a critter).
#[derive(Resource, Debug, Default)]
pub struct CastRequests(pub Vec<(PlayerId, usize, LockTarget)>, pub Vec<(PlayerId, usize, NetId)>);

impl CastRequests {
    pub fn critters(&mut self) -> &mut Vec<(PlayerId, usize, NetId)> {
        &mut self.1
    }
}

/// What a spell does to a critter (`rules/spelllist.mes` lines `1000 + 50 × spell` …; 0x4521a0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CritterSpell {
    /// Hit points healed on a living, hurt target; Major Healing also mends scars and crippling
    /// (`Dmg_Flags: Scars | Crippling`).
    Heal { amount: i32, injuries: bool },
    /// Halt Poison: 50 poison taken away.
    CurePoison(i32),
    /// Resurrect (`Full | Resurrect`): a dead target, not the caster, gets up.
    Resurrect,
}

/// The critter spell `spell` cast at magick aptitude `aptitude`.
pub fn critter_spell(spell: usize, aptitude: i32) -> Option<CritterSpell> {
    let spell32 = spell as u32;
    Some(match spell32 {
        crate::MINOR_HEALING => CritterSpell::Heal { amount: crate::spell_heal(spell32, aptitude), injuries: false },
        crate::MAJOR_HEALING => CritterSpell::Heal { amount: crate::spell_heal(spell32, aptitude), injuries: true },
        crate::HALT_POISON => CritterSpell::CurePoison(50),
        crate::RESURRECT => CritterSpell::Resurrect,
        _ => return None,
    })
}

/// The critters the spells may land on: PCs and NPCs.
type SpellTargetQuery<'a> = (&'a NetId, &'a Position, Option<&'a PlayerControlled>, Option<&'a crate::NpcSource>);

/// Casts the healing spells and Resurrect on critters (`PlayerIntent::CastOn`): in range
/// ([`CAST_RANGE`]), paid as any cast ([`cast_check`]); a target the spell does not take (a dead
/// one to heal, a living one to raise, the caster to raise, the undead) costs nothing. A PC is
/// raised with 15 % of its hit points (owner's call), an NPC with all of them (`Full`).
#[allow(clippy::too_many_arguments)]
pub fn cast_on_critters(
    mut requests: ResMut<CastRequests>,
    mut map: ResMut<LoadedMap>,
    mut world: ResMut<WorldState>,
    session: Res<Session>,
    rules: Res<SimRules>,
    mut outbox: ResMut<Outbox>,
    (mut sheets, mut raising): (ResMut<crate::ScriptSheets>, ResMut<Resurrecting>),
    mut casters: Query<(&NetId, &PlayerControlled, &Position, &mut Sheet)>,
    targets: Query<SpellTargetQuery>,
) {
    for (player, spell, target) in std::mem::take(requests.critters()) {
        let Some((&net_id, _, position, mut sheet)) = casters.iter_mut().find(|pc| pc.1.0 == player) else { continue };
        let Some(effect) = critter_spell(spell, rules.0.aptitude(&sheet.0)) else { continue };
        let Some((_, at, pc, npc)) = targets.iter().find(|t| *t.0 == target) else { continue };
        if !crate::within_radius(position.0, at.0, CAST_RANGE) {
            continue;
        }
        // Whether the target takes it (the list's `Tgt_` flags).
        let takes = match (pc.map(|p| p.0), npc) {
            (Some(p), _) => world.0.as_ref().and_then(|w| w.players.get(&p)).is_some_and(|s| {
                let t = &s.blueprint.sheet;
                match effect {
                    CritterSpell::Heal { .. } => !t.dead && t.hp_damage > 0,
                    CritterSpell::CurePoison(_) => !t.dead,
                    CritterSpell::Resurrect => t.dead && p != player,
                }
            }),
            (None, Some(n)) => {
                let dead = map.is_dead(n.sector, n.index);
                let undead = map.mobiles.get(&n.sector).and_then(|m| m.get(n.index as usize)).and_then(|o| map.prototypes.int(o, "critter_flags")).unwrap_or(0) & crate::combat::OCF_UNDEAD != 0;
                match effect {
                    CritterSpell::Heal { .. } => !dead && map.npc_hp(n.sector, n.index).is_some_and(|(left, max)| left < max),
                    CritterSpell::CurePoison(_) => !dead,
                    CritterSpell::Resurrect => dead && !undead,
                }
            }
            _ => false,
        };
        if !takes {
            continue;
        }
        let current = pc_status(&rules.0, &sheet.0).fatigue.current;
        let cost = match cast_check(&rules.0, &sheet.0, current, spell) {
            Ok(cost) => cost,
            Err(CastRefusal::NoEnergy) => {
                outbox.0.push((player, LedgerDelta::SpellResult { message: NOT_ENOUGH_ENERGY }));
                continue;
            }
            Err(CastRefusal::Silent) => continue,
        };
        sheet.0.fatigue_damage += cost;
        if let Some(saved) = world.0.as_mut().and_then(|w| w.players.get_mut(&player)) {
            saved.blueprint.sheet = sheet.0.clone();
        }
        crate::ground::broadcast(&session, &mut outbox, LedgerDelta::PcStatus { net_id, status: pc_status(&rules.0, &sheet.0) });
        match (pc.map(|p| p.0), npc) {
            (Some(p), _) => {
                let Some(t) = world.0.as_mut().and_then(|w| w.players.get_mut(&p)).map(|s| &mut s.blueprint.sheet) else { continue };
                match effect {
                    CritterSpell::Heal { amount, injuries } => {
                        t.hp_damage = (t.hp_damage - amount).max(0);
                        if injuries {
                            t.injuries = 0;
                        }
                    }
                    CritterSpell::CurePoison(n) => t.poison = (t.poison - n).max(0),
                    CritterSpell::Resurrect => crate::unconscious::resurrect(&rules.0, t),
                }
                sheets.0.insert(p);
            }
            (None, Some(n)) => {
                match effect {
                    CritterSpell::Heal { amount, injuries } => {
                        map.hurt_npc(n.sector, n.index, -amount);
                        if injuries {
                            map.cure_npc_injuries(n.sector, n.index);
                        }
                    }
                    CritterSpell::CurePoison(amount) => {
                        let level = map.npc_poison(n.sector, n.index);
                        map.set_npc_poison(n.sector, n.index, level - amount);
                    }
                    // Raised after its resurrect script answers ([`resurrect_npcs`]).
                    CritterSpell::Resurrect => raising.0.push((*n, player)),
                }
                crate::ground::save_change(&mut world, &map, n.sector);
            }
            _ => {}
        }
    }
}

/// Dead NPCs a Resurrect spell reached, and the caster.
#[derive(Resource, Debug, Default)]
pub struct Resurrecting(pub Vec<(crate::NpcSource, crate::PlayerId)>);

/// `critter_flags` 0x1000000: no Resurrect raises the critter (0x4b5a19); 0x800000 bars a
/// Reanimate (`Dmg_Flags` 0x400000, 0x4b5a03; no PC spell has it).
pub const OCF_UNRESURRECTABLE: i32 = 0x100_0000;

/// A raised critter's fatigue damage (0x4b5b16-0x4b5b28).
pub const RAISED_FATIGUE_DAMAGE: i32 = 10;

/// Script event 18, resurrect: on the dead critter, the healer the triggerer (0x4b5a46).
pub const EVENT_RESURRECT: u32 = 18;

/// The reaction to the PC healer a raised NPC is brought up to (0x4b5b3d: `0x46 − r`).
pub const RAISED_REACTION: i32 = 70;

/// A Resurrect reaching a dead NPC (0x4b58c0 with `Dmg_Flags` Resurrect, bit 2): its resurrect
/// script runs first (0x4b5a46), and a 0 answer or `critter_flags` [`OCF_UNRESURRECTABLE`]
/// refuses it (0x4b5a4e → 0x45a540, whose eye candy is not built); otherwise it gets up
/// (0x4b5a71): its fatigue damage 10 (0x4b5b28; the spell's heal gives no fatigue back,
/// 0x4b5c08), its injuries mended (`Full`, 0x4b5c3d: bits 0x80-0x400), and a reaction to the PC
/// healer above 0 and below [`RAISED_REACTION`] raised by the difference (0x4b5b3d-0x4b5b9e); a
/// follower joins its leader again if the join check passes, else it is dropped
/// ([`crate::follow::rejoins`]).
#[allow(clippy::too_many_arguments)]
pub fn resurrect_npcs(mut raising: ResMut<Resurrecting>, mut host: crate::ScriptHost, mut map: ResMut<LoadedMap>, mut world: ResMut<WorldState>, mut outbox: ResMut<Outbox>, rules: Res<SimRules>, mut generic: ResMut<crate::GenericLines>, vfs: Option<Res<crate::SimVfs>>, clock: Res<crate::GameClock>) {
    for (n, caster) in std::mem::take(&mut raising.0) {
        if !map.is_dead(n.sector, n.index) {
            continue;
        }
        let answer = host.hook_by(&mut world, &mut map, &mut outbox, n.object(), EVENT_RESURRECT, caster, crate::ScriptObject::Pc(caster));
        let barred = map.mobiles.get(&n.sector).and_then(|m| m.get(n.index as usize)).and_then(|o| map.prototypes.int(o, "critter_flags")).unwrap_or(0) & OCF_UNRESURRECTABLE != 0;
        if answer == crate::HookResult::SkipDefault || barred {
            continue;
        }
        map.revive_npc(n.sector, n.index);
        map.set_npc_fatigue_damage(n.sector, n.index, RAISED_FATIGUE_DAMAGE);
        map.cure_npc_injuries(n.sector, n.index);
        // 0x4c0de0's delta is scaled by the PC (0x4eaab0, not decoded): here as it is.
        if let Some(r) = host.reaction_of(&world, &map, n, caster).filter(|r| (1..RAISED_REACTION).contains(r))
            && let Some(saved) = world.0.as_mut().and_then(|w| w.players.get_mut(&caster))
        {
            let key = crate::met_key(n.object());
            let base = map.mobiles.get(&n.sector).and_then(|m| m.get(n.index as usize)).map_or(0, |o| crate::NpcReaction::of(o, &map.prototypes).base);
            let stored = saved.script_state.reactions.get(&key).copied().unwrap_or(base);
            saved.script_state.reactions.insert(key, stored + RAISED_REACTION - r);
        }
        // A follower joins its leader again, or is dropped (0x4b5ba1-0x4b5be3: 0x45de90 with
        // the join check, else 0x45de50 clears the leader; no line).
        let home = map.home_of(n.sector, n.index);
        if let Some(leader) = world.0.as_ref().and_then(|s| s.followers.iter().find(|f| f.is(&home))).map(|f| f.leader) {
            let reaction = host.reaction_of(&world, &map, n, leader).unwrap_or(0);
            let guard = vfs.as_ref().and_then(|v| v.0.read().ok());
            let packet = map.mobiles.get(&n.sector).and_then(|m| m.get(n.index as usize)).and_then(|o| map.prototypes.int(o, "npc_ai_data")).unwrap_or(0);
            let ai = crate::follow::ai_follow(generic.file(guard.as_deref(), crate::follow::GAME_AI), packet);
            let refused = world.0.as_ref().is_some_and(|s| crate::follow::rejoins(s, &map, &rules.0, ai, (n.sector, n.index), leader, reaction).is_err());
            if refused && let Some(save) = world.0.as_mut() {
                let flags = map.npc_flags(n.sector, n.index).unwrap_or(0);
                let now = clock.now;
                crate::follow::set_follower(&mut save.followers, &mut map, (n.sector, n.index), None, flags, None, now);
            }
        }
        host.toggled_npcs().0.push((n.sector, n.index));
        crate::ground::save_change(&mut world, &map, n.sector);
    }
}

/// Casts the requested spells from where the PCs stand.
#[allow(clippy::too_many_arguments)]
pub fn cast_spells(
    mut requests: ResMut<CastRequests>,
    mut map: ResMut<LoadedMap>,
    mut world: ResMut<WorldState>,
    session: Res<Session>,
    rules: Res<SimRules>,
    mut outbox: ResMut<Outbox>,
    mut pcs: Query<(&NetId, &PlayerControlled, &Position, &mut Sheet)>,
) {
    for (player, spell, target) in requests.0.drain(..) {
        let Some((&net_id, _, position, mut sheet)) = pcs.iter_mut().find(|pc| pc.1.0 == player) else { continue };
        if spell != SPELL_UNLOCKING_CANTRIP {
            continue;
        }
        let Some(location) = map.lock_location(target) else { continue };
        if !crate::within_radius(position.0, location, CAST_RANGE) {
            continue;
        }
        let current = pc_status(&rules.0, &sheet.0).fatigue.current;
        let cost = match cast_check(&rules.0, &sheet.0, current, spell) {
            Ok(cost) => cost,
            Err(CastRefusal::NoEnergy) => {
                outbox.0.push((player, LedgerDelta::SpellResult { message: NOT_ENOUGH_ENERGY }));
                continue;
            }
            Err(CastRefusal::Silent) => continue,
        };
        let Some(flags) = map.lock_flags(target) else { continue };
        sheet.0.fatigue_damage += cost;
        if let Some(saved) = world.0.as_mut().and_then(|w| w.players.get_mut(&player)) {
            saved.blueprint.sheet = sheet.0.clone();
        }
        let status = pc_status(&rules.0, &sheet.0);
        crate::ground::broadcast(&session, &mut outbox, LedgerDelta::PcStatus { net_id, status });
        let changed = flags & !crate::OPF_LOCKED;
        if changed != flags {
            map.set_lock_flags(target, changed);
            let sector = match target {
                LockTarget::Door(id) => id.sector,
                LockTarget::Container(id) => id.sector(),
            };
            crate::ground::save_change(&mut world, &map, sector);
            crate::ground::broadcast(&session, &mut outbox, LedgerDelta::LockChanged { target, flags: changed });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fatigue_recovers_the_heal_rate_a_step() {
        assert_eq!(recovered(20, 3, 1), 17);
        assert_eq!(recovered(20, 3, 7), 0, "not below 0");
        assert_eq!(recovered(5, 0, 4), 5, "no heal rate");
    }

    #[test]
    fn spells_are_known_in_order_within_a_college() {
        let mut sheet = CharacterSheet::default();
        assert!(!knows_spell(&sheet, SPELL_UNLOCKING_CANTRIP));
        sheet.spells[0] = 1;
        assert!(knows_spell(&sheet, 0) && !knows_spell(&sheet, 1));
        sheet.spells[0] = 2;
        assert!(knows_spell(&sheet, SPELL_UNLOCKING_CANTRIP));
        sheet.spells[3] = 5;
        assert!(knows_spell(&sheet, 19) && !knows_spell(&sheet, 20));
        assert!(!knows_spell(&sheet, 500));
    }

    #[test]
    fn a_cast_may_leave_fatigue_down_to_minus_14() {
        assert!(affords(10, 10));
        assert!(affords(0, 14));
        assert!(!affords(0, 15));
        let mut rules = arcanum_rules::RulesData::default();
        rules.spells.spells = (0..arcanum_rules::SPELLS).map(|i| arcanum_rules::Spell { name: String::new(), description: String::new(), college: i / 5, cost: 10 }).collect();
        let mut sheet = CharacterSheet::default();
        assert_eq!(cast_check(&rules, &sheet, 30, 1), Err(CastRefusal::Silent), "not known");
        sheet.spells[0] = 2;
        assert_eq!(cast_check(&rules, &sheet, 30, 1), Ok(10));
        assert_eq!(cast_check(&rules, &sheet, 3, 1), Ok(10), "down to -7");
        assert_eq!(cast_check(&rules, &sheet, 0, 1), Err(CastRefusal::Silent), "unconscious");
    }
}
