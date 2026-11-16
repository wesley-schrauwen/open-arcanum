//! Operations and queries on a [`CharacterSheet`], all on [`RulesData`].
//!
//! Every operation takes the current sheet and returns a new one (or a [`Refusal`]), so the same
//! call can preview a change on the client and apply it on the host.

use crate::data::{Background, Effect, RACE_EFFECT_BASE, RESISTANCES, RulesData, race_gender_code};
use crate::sheet::*;

/// The highest value primary stat `stat` can reach for `race` (Arcanum.exe 0x4b0fd0; 20 unless
/// the race raises it).
pub fn race_max(race: u8, stat: usize) -> i32 {
    // Indices: 0 ST, 1 DX, 2 CN, 3 BE, 4 IN, 5 PE, 6 WP, 7 CH.
    match (race, stat) {
        (1, 0) | (1, 2) => 21,          // dwarf: ST, CN
        (2, 1) | (2, 3) | (2, 6) => 21, // elf: DX, BE, WP
        (3, 1) => 21,                   // half-elf: DX
        (4, 6) => 22,                   // gnome: WP
        (5, 1) => 22,                   // halfling: DX
        (6, 0) | (6, 2) => 21,          // half-orc: ST, CN
        (7, 0) => 24,                   // half-ogre: ST
        _ => 20,
    }
}

/// Reaction modifier for a Beauty value (table 0x5b5330).
pub fn reaction_modifier(beauty: i32) -> i32 {
    const UP: [i32; 10] = [0, 3, 7, 12, 18, 25, 33, 42, 52, 65];
    match beauty {
        b if b >= 20 => 10 * b - 100,
        b if b >= 10 => UP[(b - 10) as usize],
        b if b >= 1 => -UP[(10 - b) as usize],
        _ => -UP[9],
    }
}

/// Effect every female character carries (`rules/effect.mes` 330).
pub const FEMALE_EFFECT: u32 = 330;

/// Experience needed for level 2 (`Rules/xp_level.mes` 2).
pub const XP_FOR_LEVEL_2: i32 = 2100;

/// The derived stats the character editor shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Derived {
    pub carry_weight: i32,
    pub damage_bonus: i32,
    pub ac_adjustment: i32,
    pub speed: i32,
    pub heal_rate: i32,
    pub poison_recovery: i32,
    pub reaction: i32,
    pub max_followers: i32,
    pub hit_points: i32,
    pub fatigue: i32,
}

impl Derived {
    /// Stats 8-15 in `mes/stat.mes` order.
    pub fn by_stat_id(&self) -> [i32; 8] {
        [
            self.carry_weight,
            self.damage_bonus,
            self.ac_adjustment,
            self.speed,
            self.heal_rate,
            self.poison_recovery,
            self.reaction,
            self.max_followers,
        ]
    }
}

/// Steps `index` through `count` choices with wrap.
fn wrap(index: usize, count: usize, delta: i32) -> usize {
    if count == 0 { 0 } else { (index as i64 + i64::from(delta)).rem_euclid(count as i64) as usize }
}

impl RulesData {
    /// A new character as the "create your own" template: human male, first background and
    /// portrait, base 8 in every stat, 5 points.
    pub fn new_sheet(&self) -> CharacterSheet {
        let s = CharacterSheet::default();
        CharacterSheet { background: self.first_background(s.race, s.gender), ..s }
    }

    fn first_background(&self, race: u8, gender: u8) -> u32 {
        self.backgrounds_for(race, gender).first().map_or(0, |b| b.id)
    }

    /// Backgrounds legal for this race and gender, in file order (the first is "no background").
    pub fn backgrounds_for(&self, race: u8, gender: u8) -> Vec<&Background> {
        self.backgrounds.iter().filter(|b| b.allows(race, gender)).collect()
    }

    fn portrait_entries(&self, race: u8, gender: u8) -> Vec<&(u32, String)> {
        let code = race_gender_code(race, gender);
        self.portraits
            .iter()
            .filter(|(_, stem)| stem.len() > code.len() && stem.starts_with(&code) && stem[code.len()..].chars().all(|c| c.is_ascii_digit()))
            .collect()
    }

    /// Portrait file stems for this race and gender (`DWM1`, `DWM2`, ...).
    pub fn portraits_for(&self, race: u8, gender: u8) -> Vec<&str> {
        self.portrait_entries(race, gender).into_iter().map(|(_, stem)| stem.as_str()).collect()
    }

    /// The race or gender changed: back to the first background and portrait.
    fn reset_choices(&self, s: CharacterSheet) -> CharacterSheet {
        CharacterSheet { background: self.first_background(s.race, s.gender), portrait: None, ..s }
    }

    /// Steps the race by `delta`, skipping races that do not allow the current gender. Resets
    /// portrait and background; spent points stay.
    pub fn step_race(&self, s: &CharacterSheet, delta: i32) -> CharacterSheet {
        let mut race = s.race as i32;
        for _ in 0..RACE_COUNT {
            race = (race + delta).rem_euclid(RACE_COUNT as i32);
            if s.gender == MALE || FEMALE_ALLOWED[race as usize] {
                break;
            }
        }
        self.reset_choices(CharacterSheet { race: race as u8, ..s.clone() })
    }

    /// Switches gender if the race allows it.
    pub fn toggle_gender(&self, s: &CharacterSheet) -> Result<CharacterSheet, Refusal> {
        let gender = if s.gender == MALE { FEMALE } else { MALE };
        if gender == FEMALE && !FEMALE_ALLOWED[s.race as usize % RACE_COUNT] {
            return Err(Refusal::NotAllowed);
        }
        Ok(self.reset_choices(CharacterSheet { gender, ..s.clone() }))
    }

    pub fn step_portrait(&self, s: &CharacterSheet, delta: i32) -> CharacterSheet {
        let list = self.portrait_entries(s.race, s.gender);
        let at = s.portrait.and_then(|id| list.iter().position(|(p, _)| *p == id)).unwrap_or(0);
        let portrait = list.get(wrap(at, list.len(), delta)).map(|(id, _)| *id);
        CharacterSheet { portrait, ..s.clone() }
    }

    pub fn step_background(&self, s: &CharacterSheet, delta: i32) -> CharacterSheet {
        let list = self.backgrounds_for(s.race, s.gender);
        let at = list.iter().position(|b| b.id == s.background).unwrap_or(0);
        let background = list.get(wrap(at, list.len(), delta)).map_or(s.background, |b| b.id);
        CharacterSheet { background, ..s.clone() }
    }

    /// Sets the name, keeping at most [`MAX_NAME`] characters the game's fonts can show.
    pub fn set_name(&self, s: &CharacterSheet, name: &str) -> CharacterSheet {
        let name = name
            .chars()
            .filter(|&c| !c.is_control() && arcanum_formats::cp1252::encode_char(c).is_some())
            .take(MAX_NAME)
            .collect();
        CharacterSheet { name, ..s.clone() }
    }

    /// The sheet's background: one legal for its race and gender, or a pre-made's own (`NPC`).
    pub fn background(&self, s: &CharacterSheet) -> Option<&Background> {
        self.backgrounds.iter().find(|b| b.id == s.background && (b.npc || b.allows(s.race, s.gender)))
    }

    pub fn portrait(&self, s: &CharacterSheet) -> Option<&str> {
        let list = self.portrait_entries(s.race, s.gender);
        let entry = match s.portrait {
            Some(id) => list.into_iter().find(|(p, _)| *p == id),
            None => list.into_iter().next(),
        };
        entry.map(|(_, stem)| stem.as_str())
    }

    /// The race, gender and background effects together. Race is effect 64 + race; women carry
    /// effect 330 (`rules/effect.mes` "all females"), applied by the gender setter
    /// (Arcanum.exe 0x4b0980 case 0x1a, cause 9).
    fn character_effects(&self, s: &CharacterSheet) -> Effect {
        let race = self.effect(RACE_EFFECT_BASE + u32::from(s.race));
        let gender = if s.gender == FEMALE { self.effect(FEMALE_EFFECT) } else { Effect::default() };
        let background = self.background(s).and_then(|b| b.effect).map(|e| self.effect(e)).unwrap_or_default();
        Effect {
            stats: std::array::from_fn(|i| race.stats[i] + gender.stats[i] + background.stats[i]),
            resist: std::array::from_fn(|i| race.resist[i] + gender.resist[i] + background.resist[i]),
        }
    }

    /// Current primary stats: base, plus the draft, plus race and background effects.
    pub fn stats(&self, s: &CharacterSheet) -> [i32; PRIMARY_STATS] {
        let e = self.character_effects(s);
        std::array::from_fn(|i| s.base[i] + s.draft.stats[i] + e.stats[i])
    }

    /// Resistances from race and background, in percent.
    pub fn resistances(&self, s: &CharacterSheet) -> [i32; RESISTANCES] {
        self.character_effects(s).resist
    }

    /// Character points left after the draft.
    pub fn unspent(&self, s: &CharacterSheet) -> i32 {
        s.unspent - s.draft.spent()
    }

    /// Raises primary stat `stat` by one, spending a point (Arcanum.exe 0x55a80a).
    pub fn raise_stat(&self, s: &CharacterSheet, stat: usize) -> Result<CharacterSheet, Refusal> {
        if self.stats(s)[stat] >= race_max(s.race, stat) {
            return Err(Refusal::AtMaximum);
        }
        if self.unspent(s) < STAT_COST {
            return Err(Refusal::NoPoints);
        }
        let mut out = s.clone();
        out.draft.stats[stat] += 1;
        Ok(out)
    }

    /// Lowers primary stat `stat` by one, refunding a point; not below its accepted value
    /// (0x55a997).
    pub fn lower_stat(&self, s: &CharacterSheet, stat: usize) -> Result<CharacterSheet, Refusal> {
        if s.draft.stats[stat] == 0 {
            return Err(if self.stats(s)[stat] <= MIN_STAT { Refusal::AtMinimum } else { Refusal::BelowAccepted });
        }
        let mut out = s.clone();
        out.draft.stats[stat] -= 1;
        Ok(out)
    }

    /// Buys (`delta > 0`) or sells a step of hit points or fatigue.
    pub fn adjust_pool(&self, s: &CharacterSheet, pool: Pool, delta: i32) -> Result<CharacterSheet, Refusal> {
        let unspent = self.unspent(s);
        let mut out = s.clone();
        let points = match pool {
            Pool::HitPoints => &mut out.draft.hit_points,
            Pool::Fatigue => &mut out.draft.fatigue,
        };
        if delta > 0 {
            if unspent < STAT_COST {
                return Err(Refusal::NoPoints);
            }
            *points += 1;
        } else {
            if *points == 0 {
                return Err(Refusal::BelowAccepted);
            }
            *points -= 1;
        }
        Ok(out)
    }

    /// Makes the draft permanent: it can no longer be sold back.
    pub fn accept(&self, s: &CharacterSheet) -> CharacterSheet {
        let mut out = s.clone();
        out.unspent = self.unspent(s);
        for i in 0..PRIMARY_STATS {
            out.base[i] += s.draft.stats[i];
        }
        out.hp_points += s.draft.hit_points;
        out.fatigue_points += s.draft.fatigue;
        out.draft = Draft::default();
        out
    }

    /// The derived stats (Arcanum.exe 0x4b0740, 0x43d5a0, 0x45d670).
    pub fn derived(&self, s: &CharacterSheet) -> Derived {
        let [st, dx, cn, be, _in, _pe, wp, ch] = self.stats(s);
        let level = s.level;
        let damage_bonus = {
            let d = st - 10;
            let d = if d < 0 { d / 2 } else { d };
            if st >= 20 { d * 2 } else { d }
        };
        Derived {
            carry_weight: 500 * st,
            damage_bonus,
            ac_adjustment: dx - 10,
            speed: dx + if dx >= 20 { 5 } else { 0 },
            heal_rate: (cn + 1) / 3,
            poison_recovery: cn,
            reaction: reaction_modifier(be),
            max_followers: ch / 4,
            hit_points: 4 * (s.hp_points + s.draft.hit_points) + wp + 2 * (level + st) + 4,
            fatigue: 4 * (s.fatigue_points + s.draft.fatigue) + wp + 2 * (level + cn) + 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    use arcanum_formats::mes::MesFile;

    fn mes(text: &str) -> MesFile {
        MesFile::parse(text.as_bytes()).unwrap()
    }

    fn rules() -> RulesData {
        RulesData::from_files(
            &mes("{0}{1000}{1}{86}{2}{}{10}{1001}{11}{87}{12}{}{30}{1003}{31}{89}{32}{HUF HUM}{390}{1039}{392}{NPC}{40}{1004}{41}{90}{42}{DWM}"),
            &mes("{65}{st +1, cn +1, ch -1, dx -1, repair +2}{87}{resistpoison +20, be -1}{89}{repair -1}{90}{pe +1}"),
            &mes("{1000}{ELM1}{1001}{DWM1}{1005}{HUM1}{1018}{HUM2}{1020}{DWM2}{1008}{HUF1}{1035}{NPCDoc}"),
        )
    }

    #[test]
    fn backgrounds_follow_the_legal_codes_and_skip_npc_ones() {
        let r = rules();
        let ids = |race, gender| r.backgrounds_for(race, gender).iter().map(|b| b.id).collect::<Vec<_>>();
        assert_eq!(ids(0, MALE), [0, 1, 3], "human male: no restriction, HUM");
        assert_eq!(ids(1, MALE), [0, 1, 4], "dwarf male: DWM only");
    }

    #[test]
    fn npc_backgrounds_are_kept_but_never_offered() {
        let r = rules();
        assert!(r.backgrounds.iter().any(|b| b.id == 39 && b.npc));
        assert!(!r.backgrounds_for(0, MALE).iter().any(|b| b.npc));
    }

    #[test]
    fn portraits_match_the_race_and_gender_code() {
        let r = rules();
        assert_eq!(r.portraits_for(0, MALE), ["HUM1", "HUM2"]);
        assert_eq!(r.portraits_for(1, MALE), ["DWM1", "DWM2"]);
        assert_eq!(r.portraits_for(0, FEMALE), ["HUF1"]);
    }

    #[test]
    fn stats_add_race_and_background_effects() {
        let r = rules();
        let s = r.step_race(&r.new_sheet(), 1);
        assert_eq!(r.stats(&s), [9, 7, 9, 8, 8, 8, 8, 7], "dwarf: st +1 cn +1 ch -1 dx -1");
        let s = CharacterSheet { background: 4, ..s }; // DWM-only background, pe +1
        assert_eq!(r.background(&s).unwrap().id, 4);
        assert_eq!(r.stats(&s)[5], 9);
        assert_eq!(parse_stat_effect("wp +1, dx +1, be +1, cn -2, st -1, magicpts +3"), [-1, 1, -2, 1, 0, 0, 1, 0]);
    }

    #[test]
    fn race_skips_male_only_races_for_women_and_resets_choices() {
        let r = rules();
        let s = CharacterSheet { gender: FEMALE, portrait: Some(1008), background: 3, ..r.new_sheet() };
        let s = r.step_race(&s, 1);
        assert_eq!(s.race, 2, "dwarf has no women: straight to elf");
        assert_eq!((s.portrait, s.background), (None, 0));
        let s = r.step_race(&s, 1);
        assert_eq!(s.race, 3);
        let s = r.step_race(&s, 1);
        assert_eq!(s.race, 6, "half-orc after half-elf");
        let s = r.step_race(&s, 1);
        assert_eq!(s.race, 0, "wraps past half-ogre");
        assert_eq!(r.step_race(&s, -1).race, 6);
    }

    #[test]
    fn gender_only_switches_where_the_race_allows_it() {
        let r = rules();
        let dwarf = r.step_race(&r.new_sheet(), 1);
        assert_eq!(r.toggle_gender(&dwarf), Err(Refusal::NotAllowed), "dwarves are male only");
        let elf = CharacterSheet { race: 2, ..r.new_sheet() };
        assert_eq!(r.toggle_gender(&elf).unwrap().gender, FEMALE);
    }

    #[test]
    fn portraits_and_backgrounds_wrap() {
        let r = rules();
        let s = r.step_portrait(&r.new_sheet(), -1);
        assert_eq!(r.portrait(&s), Some("HUM2"));
        let s = r.step_portrait(&s, 1);
        assert_eq!(r.portrait(&s), Some("HUM1"));
        let s = r.step_background(&s, -1);
        assert_eq!(r.background(&s).unwrap().id, 3);
    }

    #[test]
    fn names_are_limited_and_printable() {
        let r = rules();
        let s = r.new_sheet();
        assert!(!s.has_name());
        let s = r.set_name(&s, "Virgil the very long-named hero");
        assert_eq!(s.name.chars().count(), MAX_NAME);
        let d = r.set_name(&r.new_sheet(), "\u{8}日 ");
        assert!(!d.has_name(), "control, unsupported and blank input do not make a name");
    }

    #[test]
    fn spending_points_follows_the_original_rules() {
        let r = rules();
        let s = r.new_sheet();
        assert_eq!(r.unspent(&s), STARTING_POINTS);
        let s = r.raise_stat(&s, 0).unwrap();
        assert_eq!((r.stats(&s)[0], r.unspent(&s)), (9, 4));
        assert_eq!(r.lower_stat(&s, 1), Err(Refusal::BelowAccepted), "not below the starting value");
        let mut s = r.lower_stat(&s, 0).unwrap();
        assert_eq!(r.unspent(&s), STARTING_POINTS);
        for _ in 0..5 {
            s = r.raise_stat(&s, 7).unwrap();
        }
        assert_eq!(r.raise_stat(&s, 7), Err(Refusal::NoPoints));
        assert_eq!(r.adjust_pool(&s, Pool::HitPoints, 1), Err(Refusal::NoPoints));
        // The race maximum stops a stat even with points left.
        let d = CharacterSheet { base: [20, 8, 8, 8, 8, 8, 8, 8], ..r.new_sheet() };
        assert_eq!(r.raise_stat(&d, 0), Err(Refusal::AtMaximum));
        assert_eq!(race_max(7, 0), 24, "half-ogres reach 24 strength");
    }

    #[test]
    fn derived_stats_use_the_original_formulas() {
        let r = rules();
        let s = r.new_sheet();
        let d = r.derived(&s);
        assert_eq!((d.carry_weight, d.damage_bonus, d.ac_adjustment, d.speed), (4000, -1, -2, 8));
        assert_eq!((d.heal_rate, d.poison_recovery, d.reaction, d.max_followers), (3, 8, -7, 2));
        // HP = 4 x bought + WP + 2 x (level + ST) + 4 = 0 + 8 + 18 + 4.
        assert_eq!((d.hit_points, d.fatigue), (30, 30));
        let e = r.adjust_pool(&s, Pool::HitPoints, 1).unwrap();
        assert_eq!(r.derived(&e).hit_points, 34);
        assert_eq!(r.derived(&r.accept(&e)).hit_points, 34, "accepted pool points still count");
        assert_eq!(reaction_modifier(10), 0);
        assert_eq!(reaction_modifier(15), 25);
        assert_eq!(reaction_modifier(1), -65);
        assert_eq!(reaction_modifier(22), 120);
    }

    #[test]
    fn accept_folds_the_draft_and_blocks_lowering_below_it() {
        let r = rules();
        let s = r.raise_stat(&r.new_sheet(), 0).unwrap();
        let s = r.accept(&s);
        assert_eq!((s.base[0], s.unspent, s.draft.stats[0]), (9, 4, 0));
        assert_eq!(r.lower_stat(&s, 0), Err(Refusal::BelowAccepted));
        let s = r.adjust_pool(&s, Pool::Fatigue, 1).unwrap();
        let s = r.accept(&s);
        assert_eq!((s.fatigue_points, s.unspent), (1, 3));
        assert_eq!(r.adjust_pool(&s, Pool::Fatigue, -1), Err(Refusal::BelowAccepted));
    }

    #[test]
    fn empty_rules_still_make_a_sheet() {
        let r = RulesData::default();
        let s = r.new_sheet();
        assert_eq!((s.portrait, r.portrait(&s), r.unspent(&s)), (None, None, STARTING_POINTS));
        let s = r.step_portrait(&s, 1);
        let s = r.step_background(&s, -1);
        let s = r.step_race(&s, 1);
        assert_eq!(r.background(&s), None);
    }

    #[test]
    fn race_change_after_spending_keeps_the_draft_within_bounds() {
        let r = rules();
        let mut s = r.new_sheet();
        for _ in 0..5 {
            s = r.raise_stat(&s, 0).unwrap();
        }
        let s = r.step_race(&s, 1);
        assert_eq!((s.race, s.background, r.unspent(&s)), (1, 0, 0));
        assert_eq!(r.stats(&s)[0], 14, "8 + 5 bought + dwarf +1");
    }

    #[test]
    fn women_get_the_female_effect() {
        let r = RulesData::from_files(&mes("{0}{1000}{1}{86}{2}{}"), &mes("{330}{st -1, cn +1}"), &mes("{1008}{HUF1}"));
        let woman = r.toggle_gender(&r.new_sheet()).unwrap();
        assert_eq!(r.stats(&woman), [7, 8, 9, 8, 8, 8, 8, 8], "effect 330: st -1, cn +1 for all females");
        assert_eq!(r.stats(&r.new_sheet())[0], 8, "men unchanged");
    }

    #[test]
    fn refusals_name_their_messages() {
        assert_eq!(Refusal::AtMaximum.message(), Some(Message::Charedit(17)));
        assert_eq!(Refusal::BelowAccepted.message(), Some(Message::Charedit(19)));
        assert_eq!(Refusal::NoName.message(), Some(Message::MainMenu(506)));
        assert_eq!(Refusal::NotAllowed.message(), None);
    }
}
