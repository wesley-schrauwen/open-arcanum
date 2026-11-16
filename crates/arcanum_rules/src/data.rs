//! Rules tables read once from the game data.
//!
//! - Backgrounds: `Rules/backgrnd.mes`, blocks of 10 (description id in `mes/gameback.mes`,
//!   effect id in `rules/effect.mes`, legal race-gender codes, money, items). Blank or `ANY`
//!   allows everyone; `NPC` marks the pre-made characters' own backgrounds, never offered at
//!   creation.
//! - Effects: `rules/effect.mes`, e.g. `st +1, cn +1, resistpoison +10`; race effects are
//!   64 + race.
//! - Portraits: `portrait/gameport.mes` entries named after the race-gender code (`DWM1`, ...).

use arcanum_formats::mes::MesFile;
use arcanum_vfs::Vfs;

use crate::sheet::{FEMALE, PRIMARY_STATS, RACE_COUNT};
use crate::tables::{Schematic, SkillTable, SpellTable, TechTable, parse_schematics};

/// Race-gender codes as used in `gameport.mes` and `backgrnd.mes`.
const RACE_CODES: [&str; RACE_COUNT] = ["HU", "DW", "EL", "HE", "GN", "HA", "HO", "HG"];
const STAT_KEYS: [&str; PRIMARY_STATS] = ["st", "dx", "cn", "be", "in", "pe", "wp", "ch"];
/// First race effect in `rules/effect.mes`.
pub const RACE_EFFECT_BASE: u32 = 64;
/// Resistance kinds, in the character editor's order.
pub const RESISTANCES: usize = 5;
const RESIST_KEYS: [&str; RESISTANCES] = ["resistdamage", "resistmagic", "resistfire", "resistpoison", "resistelectrical"];

/// One background block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Background {
    /// Block number in `Rules/backgrnd.mes` (the block starts at `id * 10`).
    pub id: u32,
    /// `mes/gameback.mes` id: title line, then the story.
    pub description: u32,
    /// `rules/effect.mes` id.
    pub effect: Option<u32>,
    /// Legal race-gender codes; empty = everyone.
    pub codes: Vec<String>,
    /// A pre-made character's own background (`NPC`): kept, never offered at creation.
    pub npc: bool,
    /// Gold the background gives (line 4).
    pub money: i32,
    /// Item prototype numbers the background gives (line 5; bad tokens skipped).
    pub items: Vec<u32>,
}

impl Background {
    pub fn allows(&self, race: u8, gender: u8) -> bool {
        !self.npc && (self.codes.is_empty() || self.codes.iter().any(|c| c == "ANY" || *c == race_gender_code(race, gender)))
    }
}

/// `DWM`, `HUF`, ...
pub fn race_gender_code(race: u8, gender: u8) -> String {
    format!("{}{}", RACE_CODES[race as usize % RACE_COUNT], if gender == FEMALE { 'F' } else { 'M' })
}

/// What an effect line does to the primary stats and resistances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Effect {
    pub stats: [i32; PRIMARY_STATS],
    /// Damage, Magick, Fire, Poison, Electrical (`mes/charedit.mes` 12-16 order).
    pub resist: [i32; RESISTANCES],
}

/// Everything the character rules need from the game data.
#[derive(Debug, Clone, Default)]
pub struct RulesData {
    pub backgrounds: Vec<Background>,
    /// `(portrait id, file stem)` in id order.
    pub portraits: Vec<(u32, String)>,
    /// Stat and resistance changes per effect id.
    pub effects: Vec<(u32, Effect)>,
    pub skills: SkillTable,
    pub tech: TechTable,
    pub spells: SpellTable,
    pub schematics: Vec<Schematic>,
    /// The pre-made characters, built from their PC prototypes.
    pub premades: Vec<crate::blueprint::Premade>,
}

pub(crate) fn read_mes(vfs: &Vfs, path: &str) -> Result<MesFile, String> {
    let data = vfs.read(path).map_err(|e| format!("{path}: {e}"))?;
    MesFile::parse(&data).map_err(|e| format!("{path}: {e}"))
}

impl RulesData {
    /// Reads `Rules/backgrnd.mes`, `rules/effect.mes`, `portrait/gameport.mes` and the skill,
    /// tech, spell and schematic tables.
    pub fn load(vfs: &Vfs) -> Result<Self, String> {
        let mut data = Self::from_files(
            &read_mes(vfs, "Rules/backgrnd.mes")?,
            &read_mes(vfs, "rules/effect.mes")?,
            &read_mes(vfs, "portrait/gameport.mes")?,
        );
        // No screen needs these yet: a missing or broken table is left empty rather than losing
        // character creation.
        let optional = |path: &str| read_mes(vfs, path).ok();
        if let Some(m) = optional("mes/skill.mes") {
            data.skills = SkillTable::from_mes(&m);
        }
        if let Some(m) = optional("mes/tech.mes") {
            data.tech = TechTable::from_mes(&m);
        }
        if let Some(m) = optional("mes/spell.mes") {
            data.spells = SpellTable::from_mes(&m);
        }
        if let Some(m) = optional("rules/schematic.mes") {
            data.schematics = parse_schematics(&m);
        }
        data.load_premades(vfs);
        Ok(data)
    }

    pub fn from_files(backgrounds: &MesFile, effects: &MesFile, portraits: &MesFile) -> Self {
        let mut list = Vec::new();
        for id in 0..1000u32 {
            let Some(description) = backgrounds.get(id * 10).and_then(|s| s.trim().parse::<u32>().ok()) else { continue };
            let effect = backgrounds.get(id * 10 + 1).and_then(|s| s.trim().parse().ok());
            let codes: Vec<String> = backgrounds.get(id * 10 + 2).unwrap_or("").split_whitespace().map(str::to_ascii_uppercase).collect();
            let npc = codes.iter().any(|c| c == "NPC");
            let money = backgrounds.get(id * 10 + 3).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let items = backgrounds.get(id * 10 + 4).unwrap_or("").split_whitespace().filter_map(|t| t.parse().ok()).collect();
            list.push(Background { id, description, effect, codes, npc, money, items });
        }
        let mut stat_effects = Vec::new();
        for id in 0..2000u32 {
            if let Some(text) = effects.get(id) {
                let effect = parse_effect(text);
                if effect != Effect::default() {
                    stat_effects.push((id, effect));
                }
            }
        }
        let mut portrait_list = Vec::new();
        for id in 1000..2000u32 {
            if let Some(stem) = portraits.get(id) {
                portrait_list.push((id, stem.trim().to_string()));
            }
        }
        Self { backgrounds: list, portraits: portrait_list, effects: stat_effects, ..Self::default() }
    }

    pub(crate) fn effect(&self, id: u32) -> Effect {
        self.effects.iter().find(|(e, _)| *e == id).map(|(_, m)| *m).unwrap_or_default()
    }
}

/// The primary stat changes in an effect line such as `st +1, cn +1, ch -1, repair +2`.
pub fn parse_stat_effect(text: &str) -> [i32; PRIMARY_STATS] {
    parse_effect(text).stats
}

/// The stat and resistance changes in an effect line.
pub fn parse_effect(text: &str) -> Effect {
    let mut out = Effect::default();
    for part in text.split(',') {
        let mut words = part.split_whitespace();
        let (Some(key), Some(value)) = (words.next(), words.next()) else { continue };
        let Ok(v) = value.trim_start_matches('+').parse::<i32>() else { continue };
        if let Some(i) = STAT_KEYS.iter().position(|k| key.eq_ignore_ascii_case(k)) {
            out.stats[i] += v;
        } else if let Some(i) = RESIST_KEYS.iter().position(|k| key.eq_ignore_ascii_case(k)) {
            out.resist[i] += v;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tables_do_not_stop_character_creation() {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in [
            ("Rules/backgrnd.mes", "{0}{1000}{1}{86}{2}{}"),
            ("rules/effect.mes", "{65}{st +1}"),
            ("portrait/gameport.mes", "{1005}{HUM1}"),
        ] {
            let file = dir.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        let mut vfs = Vfs::new();
        vfs.mount_dir(dir.path());
        let data = RulesData::load(&vfs).expect("skill/tech/spell/schematic tables are optional for now");
        assert_eq!((data.backgrounds.len(), data.portraits.len()), (1, 1));
        assert!(data.skills.skills.is_empty() && data.schematics.is_empty() && data.premades.is_empty());
    }

    #[test]
    fn effects_carry_resistances() {
        let e = parse_effect("st +1, cn +1, melee +2, resistpoison +10, be -2");
        assert_eq!(e.stats, [1, 0, 1, -2, 0, 0, 0, 0]);
        assert_eq!(e.resist, [0, 0, 0, 10, 0]);
    }
}
