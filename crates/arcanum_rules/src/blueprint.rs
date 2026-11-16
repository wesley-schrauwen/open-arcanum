//! What the host holds once a character is made: everything needed to spawn the PC.

use crate::*;

/// A pre-made character, built from its PC prototype (16087-16098).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Premade {
    pub prototype: u32,
    pub sheet: CharacterSheet,
    pub gold: i32,
    /// Item prototype numbers.
    pub items: Vec<u32>,
}

/// Everything needed to spawn a PC: its sheet, gold and starting items (as prototype numbers;
/// the world turns them into objects).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterBlueprint {
    pub sheet: CharacterSheet,
    pub gold: i32,
    pub items: Vec<u32>,
    /// The pre-made's prototype, if picked rather than created.
    pub prototype: Option<u32>,
}

/// Folder of the loose prototypes (mounted from `<game>/data`).
pub const PROTO_DIR: &str = "proto";
/// Prototype numbers of the pre-made characters, in the original's order (016086 is the empty
/// "create your own" template).
pub const FIRST_PREMADE: u32 = 16087;
pub const LAST_PREMADE: u32 = 16098;

// `critter_stat_base_idx` entries beyond the primary stats.
const STAT_LEVEL: usize = 17;
const STAT_UNSPENT: usize = 21;
const STAT_AGE: usize = 25;
const STAT_GENDER: usize = 26;
const STAT_RACE: usize = 27;

/// Where a critter's starting inventory comes from (`Rules/InvenSource.mes`): `Name: min,max`
/// gold, then `chance,prototype` per item. A prototype's `critter_inventory_source` picks one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventorySource {
    pub gold: (i32, i32),
    /// `(percent chance, item prototype number)`.
    pub items: Vec<(u32, u32)>,
}

impl InventorySource {
    /// The items that are always given (100%).
    pub fn certain_items(&self) -> Vec<u32> {
        self.items.iter().filter(|(chance, _)| *chance >= 100).map(|(_, p)| *p).collect()
    }
}

/// Parses one `InvenSource.mes` entry; tolerates the stray commas and spaces in the real file.
pub fn parse_inventory_source(text: &str) -> Option<InventorySource> {
    let (_, rest) = text.split_once(':')?;
    // Tokens are `a,b`; `8170,` has a stray comma, and `80,` followed by `6167` is one pair.
    let mut tokens = Vec::new();
    let mut words = rest.split_whitespace();
    while let Some(word) = words.next() {
        if word.ends_with(',') && word.matches(',').count() == 1 && !word.starts_with(',') {
            match words.next() {
                Some(next) => tokens.push(format!("{word}{next}")),
                None => tokens.push(word.to_string()),
            }
        } else {
            tokens.push(word.to_string());
        }
    }
    let mut pairs = tokens.into_iter().filter_map(|token| {
        let (a, b) = token.trim_matches(',').split_once(',')?;
        Some((a.trim().parse::<i64>().ok()?, b.trim().parse::<i64>().ok()?))
    });
    let (min, max) = pairs.next()?;
    let items = pairs.filter_map(|(c, p)| Some((u32::try_from(c).ok()?, u32::try_from(p).ok()?))).collect();
    Some(InventorySource { gold: (min as i32, max as i32), items })
}

impl RulesData {
    /// A pre-made character's sheet from its PC prototype: stored base stats, level, unspent
    /// points, age, gender, race, name, background and portrait. Nothing is in a draft.
    pub fn premade_from_prototype(&self, object: &arcanum_formats::object::Object) -> CharacterSheet {
        let base = object.ints("critter_stat_base_idx").unwrap_or_default();
        let stat = |i: usize| base.get(i).copied().unwrap_or(0);
        CharacterSheet {
            name: object.string("pc_player_name").unwrap_or("").to_string(),
            race: stat(STAT_RACE).clamp(0, RACE_COUNT as i32 - 1) as u8,
            gender: if stat(STAT_GENDER) == 0 { FEMALE } else { MALE },
            age: stat(STAT_AGE),
            level: stat(STAT_LEVEL).max(1),
            background: object.int("pc_background").and_then(|b| u32::try_from(b).ok()).unwrap_or(0),
            portrait: object.int("critter_portrait").and_then(|p| u32::try_from(p).ok()),
            base: std::array::from_fn(|i| stat(i)),
            hp_points: 0,
            fatigue_points: 0,
            unspent: stat(STAT_UNSPENT),
            draft: Draft::default(),
        }
    }

    /// Reads the pre-made characters' prototypes (missing or broken ones are skipped). Gold and
    /// items come from their `critter_inventory_source` in `Rules/InvenSource.mes` (all PC
    /// sources have fixed gold and only certain items; Arcanum.exe gives them, not the
    /// background).
    pub fn load_premades(&mut self, vfs: &arcanum_vfs::Vfs) {
        let sources = crate::data::read_mes(vfs, "Rules/InvenSource.mes").ok();
        let files = vfs.list_dir(PROTO_DIR).unwrap_or_default();
        self.premades = (FIRST_PREMADE..=LAST_PREMADE)
            .filter_map(|number| {
                let prefix = format!("{PROTO_DIR}/{number:06} - ").to_ascii_lowercase();
                let path = files.iter().find(|f| f.to_ascii_lowercase().starts_with(&prefix))?;
                let object = arcanum_formats::object::Object::parse_prototype(&vfs.read(path).ok()?).ok()?;
                let sheet = self.premade_from_prototype(&object);
                let source = object
                    .int("critter_inventory_source")
                    .and_then(|id| u32::try_from(id).ok())
                    .and_then(|id| sources.as_ref()?.get(id))
                    .and_then(parse_inventory_source);
                let (gold, items) = source.map_or((0, Vec::new()), |s| (s.gold.0, s.certain_items()));
                Some(Premade { prototype: number, sheet, gold, items })
            })
            .collect();
    }

    /// A created character's blueprint: the sheet accepted, gold and items from its background.
    /// Refused without a name (`MainMenu.mes` 506).
    pub fn blueprint(&self, s: &CharacterSheet) -> Result<CharacterBlueprint, Refusal> {
        if !s.has_name() {
            return Err(Refusal::NoName);
        }
        let (gold, items) = self.background(s).map_or((0, Vec::new()), |b| (b.money, b.items.clone()));
        Ok(CharacterBlueprint { sheet: self.accept(s), gold, items, prototype: None })
    }

    /// A pre-made character's blueprint.
    pub fn premade_blueprint(&self, prototype: u32) -> Result<CharacterBlueprint, Refusal> {
        let p = self.premades.iter().find(|p| p.prototype == prototype).ok_or(Refusal::UnknownPremade)?;
        Ok(CharacterBlueprint { sheet: p.sheet.clone(), gold: p.gold, items: p.items.clone(), prototype: Some(prototype) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arcanum_formats::mes::MesFile;

    fn mes(text: &str) -> MesFile {
        MesFile::parse(text.as_bytes()).unwrap()
    }

    fn rules() -> RulesData {
        RulesData::from_files(
            &mes("{0}{1000}{1}{86}{2}{}{3}{400}{4}{}{10}{1001}{11}{87}{12}{}{13}{250}{14}{ 8054  x 8055 }{390}{1039}{392}{NPC}{393}{100}{394}{9000}"),
            &mes("{87}{pe +1}"),
            &mes("{1005}{HUM1}"),
        )
    }

    #[test]
    fn background_money_and_items_are_read() {
        let r = rules();
        let b = r.backgrounds.iter().find(|b| b.id == 1).unwrap();
        assert_eq!((b.money, b.items.clone()), (250, vec![8054, 8055]));
        let none = r.backgrounds.iter().find(|b| b.id == 0).unwrap();
        assert_eq!((none.money, none.items.is_empty()), (400, true));
    }

    #[test]
    fn apply_matches_the_operations() {
        let r = rules();
        let s = r.new_sheet();
        assert_eq!(r.apply(&s, &CharacterAction::RaiseStat(0)), r.raise_stat(&s, 0));
        let raised = r.raise_stat(&s, 0).unwrap();
        assert_eq!(r.apply(&raised, &CharacterAction::Accept), Ok(r.accept(&raised)));
        assert_eq!(r.apply(&s, &CharacterAction::SetName("Virgil".into())).unwrap().name, "Virgil");
        assert_eq!(r.apply(&s, &CharacterAction::AdjustPool(Pool::Fatigue, -1)), Err(Refusal::BelowAccepted));
    }

    #[test]
    fn unknown_premade_is_refused() {
        let r = rules();
        assert_eq!(r.apply(&r.new_sheet(), &CharacterAction::PickPremade(99999)), Err(Refusal::UnknownPremade));
    }

    #[test]
    fn picking_a_premade_takes_its_sheet() {
        let mut r = rules();
        let sheet = CharacterSheet { name: "Lugard".into(), race: 1, background: 39, ..CharacterSheet::default() };
        r.premades.push(Premade { prototype: 16094, sheet: sheet.clone(), gold: 0, items: vec![] });
        assert_eq!(r.apply(&r.new_sheet(), &CharacterAction::PickPremade(16094)), Ok(sheet));
        let bp = r.premade_blueprint(16094).unwrap();
        assert_eq!((bp.prototype, bp.sheet.name.as_str()), (Some(16094), "Lugard"));
    }

    #[test]
    fn inventory_sources_are_parsed() {
        let s = parse_inventory_source("General(sm)PC: 385,385 100,6029 100,8168 100,8065").unwrap();
        assert_eq!((s.gold, s.items.clone()), ((385, 385), vec![(100, 6029), (100, 8168), (100, 8065)]));
        // Stray commas and a space after the comma, as in the real file.
        let s = parse_inventory_source("Adventure: 75,75 100,6029 100,8170, 80, 6167 x,1").unwrap();
        assert_eq!(s.items, [(100, 6029), (100, 8170), (80, 6167)]);
        assert_eq!(s.certain_items(), [6029, 8170]);
        assert!(parse_inventory_source("no colon here").is_none());
    }

    #[test]
    fn blueprint_needs_a_name() {
        let r = rules();
        assert_eq!(r.blueprint(&r.new_sheet()), Err(Refusal::NoName));
        let s = r.set_name(&r.step_background(&r.new_sheet(), 1), "Virgil");
        let s = r.raise_stat(&s, 0).unwrap();
        let bp = r.blueprint(&s).unwrap();
        assert_eq!((bp.gold, bp.items.clone(), bp.prototype), (250, vec![8054, 8055], None));
        assert_eq!((bp.sheet.draft.clone(), bp.sheet.base[0]), (Draft::default(), 9), "accepted on finish");
    }
}
