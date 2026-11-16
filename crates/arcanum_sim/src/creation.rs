//! A character being made: its sheet and the pre-made it came from.
//!
//! The host and the client run the same code on a [`Creation`]: the host to decide, the client
//! to preview. Because both apply the same requests with the same rules, they agree; the host's
//! copy wins when they do not.

use arcanum_rules::{CharacterAction, CharacterBlueprint, CharacterSheet, Refusal, RulesData};
use serde::{Deserialize, Serialize};

/// One request about the player's character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CharacterRequest {
    /// Start again from the "create your own" template.
    New,
    /// A change applied with `arcanum_rules`.
    Action(CharacterAction),
    /// Finish the character.
    Accept,
}

/// A character in creation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Creation {
    pub sheet: CharacterSheet,
    /// The pre-made picked last, if no change was made after it.
    pub premade: Option<u32>,
}

impl Creation {
    /// A fresh "create your own" character.
    pub fn new(rules: &RulesData) -> Self {
        Self { sheet: rules.new_sheet(), premade: None }
    }

    /// Applies one action; on refusal nothing changes.
    pub fn apply(&mut self, rules: &RulesData, action: &CharacterAction) -> Result<(), Refusal> {
        self.sheet = rules.apply(&self.sheet, action)?;
        self.premade = match action {
            CharacterAction::PickPremade(p) => Some(*p),
            CharacterAction::Accept => self.premade,
            _ => None,
        };
        Ok(())
    }

    /// The finished character. The sheet becomes the blueprint's (accepted) sheet.
    pub fn finish(&mut self, rules: &RulesData) -> Result<CharacterBlueprint, Refusal> {
        let blueprint = match self.premade {
            Some(p) => rules.premade_blueprint(p)?,
            None => rules.blueprint(&self.sheet)?,
        };
        self.sheet = blueprint.sheet.clone();
        Ok(blueprint)
    }

    /// Applies one request. `Accept` returns the blueprint.
    pub fn request(&mut self, rules: &RulesData, request: &CharacterRequest) -> Result<Option<CharacterBlueprint>, Refusal> {
        match request {
            CharacterRequest::New => {
                *self = Self::new(rules);
                Ok(None)
            }
            CharacterRequest::Action(action) => self.apply(rules, action).map(|()| None),
            CharacterRequest::Accept => self.finish(rules).map(Some),
        }
    }
}

#[cfg(test)]
mod tests {
    use arcanum_rules::Premade;

    use super::*;

    #[test]
    fn refused_actions_change_nothing() {
        let rules = RulesData::default();
        let mut c = Creation::new(&rules);
        let before = c.clone();
        assert_eq!(c.apply(&rules, &CharacterAction::LowerStat(0)), Err(Refusal::BelowAccepted));
        assert_eq!(c.finish(&rules).err(), Some(Refusal::NoName));
        assert_eq!(c, before);
    }

    #[test]
    fn a_picked_premade_finishes_as_itself_until_changed() {
        let mut rules = RulesData::default();
        let sheet = CharacterSheet { name: "Lugard".into(), race: 1, ..CharacterSheet::default() };
        rules.premades.push(Premade { prototype: 16094, sheet, gold: 7, items: vec![1] });
        let mut c = Creation::new(&rules);
        c.apply(&rules, &CharacterAction::PickPremade(16094)).unwrap();
        assert_eq!(c.finish(&rules).unwrap().prototype, Some(16094));
        c.apply(&rules, &CharacterAction::RaiseStat(0)).unwrap();
        assert_eq!(c.finish(&rules).unwrap().prototype, None, "a changed pre-made is the player's own");
    }

    #[test]
    fn new_resets() {
        let rules = RulesData::default();
        let mut c = Creation::new(&rules);
        c.apply(&rules, &CharacterAction::SetName("Virgil".into())).unwrap();
        c.request(&rules, &CharacterRequest::New).unwrap();
        assert_eq!(c, Creation::new(&rules));
    }
}
