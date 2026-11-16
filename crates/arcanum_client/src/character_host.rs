//! Stand-in for the host until netcode exists: the one owner of the character sheet that counts.
//!
//! Screens never change a sheet themselves. They send [`CharacterAction`]s here and draw what
//! [`CharacterHost::sheet`] holds. When `arcanum_sim` exists, these calls move behind it and the
//! screens stay as they are.

use arcanum_rules::{CharacterAction, CharacterBlueprint, CharacterSheet, Refusal, RulesData};
use bevy::prelude::*;

#[derive(Resource, Debug, Clone, Default)]
pub struct CharacterHost {
    sheet: CharacterSheet,
    /// The pre-made picked last, if no change was made after it.
    premade: Option<u32>,
    /// Set once the character is finished (Next on the editor or the picker).
    pub finished: Option<CharacterBlueprint>,
}

impl CharacterHost {
    /// A fresh "create your own" character.
    pub fn new(rules: &RulesData) -> Self {
        Self { sheet: rules.new_sheet(), ..Self::default() }
    }

    pub fn sheet(&self) -> &CharacterSheet {
        &self.sheet
    }

    /// Applies one action with the shared rules; on refusal nothing changes.
    pub fn apply(&mut self, rules: &RulesData, action: CharacterAction) -> Result<(), Refusal> {
        self.sheet = rules.apply(&self.sheet, &action)?;
        self.premade = match action {
            CharacterAction::PickPremade(p) => Some(p),
            CharacterAction::Accept => self.premade,
            _ => None,
        };
        Ok(())
    }

    /// Accepts the character and keeps its blueprint.
    pub fn finish(&mut self, rules: &RulesData) -> Result<&CharacterBlueprint, Refusal> {
        let blueprint = match self.premade {
            Some(p) => rules.premade_blueprint(p)?,
            None => rules.blueprint(&self.sheet)?,
        };
        self.sheet = blueprint.sheet.clone();
        Ok(self.finished.insert(blueprint))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arcanum_rules::Premade;

    #[test]
    fn refused_actions_change_nothing() {
        let rules = RulesData::default();
        let mut host = CharacterHost::new(&rules);
        let before = host.sheet().clone();
        assert_eq!(host.apply(&rules, CharacterAction::LowerStat(0)), Err(Refusal::BelowAccepted));
        assert_eq!(host.sheet(), &before);
        assert_eq!(host.finish(&rules).err(), Some(Refusal::NoName));
        assert!(host.finished.is_none());
    }

    #[test]
    fn a_picked_premade_finishes_as_itself_until_changed() {
        let mut rules = RulesData::default();
        let sheet = CharacterSheet { name: "Lugard".into(), race: 1, ..CharacterSheet::default() };
        rules.premades.push(Premade { prototype: 16094, sheet, gold: 7, items: vec![1] });
        let mut host = CharacterHost::new(&rules);
        host.apply(&rules, CharacterAction::PickPremade(16094)).unwrap();
        assert_eq!(host.finish(&rules).unwrap().prototype, Some(16094));
        host.apply(&rules, CharacterAction::RaiseStat(0)).unwrap();
        assert_eq!(host.finish(&rules).unwrap().prototype, None, "a changed pre-made is the player's own");
    }
}
