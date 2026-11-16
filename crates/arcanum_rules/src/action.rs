//! One enum for every player change to a character, applied by one function. The client applies
//! an action to preview it; the host applies the same action and its result counts.

use crate::*;

/// A player's change to a character.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CharacterAction {
    StepRace(i32),
    ToggleGender,
    StepPortrait(i32),
    StepBackground(i32),
    SetName(String),
    RaiseStat(usize),
    LowerStat(usize),
    AdjustPool(Pool, i32),
    /// Take a pre-made character (prototype number) instead.
    PickPremade(u32),
    Accept,
}

impl RulesData {
    pub fn apply(&self, s: &CharacterSheet, action: &CharacterAction) -> Result<CharacterSheet, Refusal> {
        match action {
            CharacterAction::StepRace(d) => Ok(self.step_race(s, *d)),
            CharacterAction::ToggleGender => self.toggle_gender(s),
            CharacterAction::StepPortrait(d) => Ok(self.step_portrait(s, *d)),
            CharacterAction::StepBackground(d) => Ok(self.step_background(s, *d)),
            CharacterAction::SetName(name) => Ok(self.set_name(s, name)),
            CharacterAction::RaiseStat(i) if *i < PRIMARY_STATS => self.raise_stat(s, *i),
            CharacterAction::LowerStat(i) if *i < PRIMARY_STATS => self.lower_stat(s, *i),
            CharacterAction::RaiseStat(_) | CharacterAction::LowerStat(_) => Err(Refusal::NotAllowed),
            CharacterAction::AdjustPool(pool, d) => self.adjust_pool(s, *pool, *d),
            CharacterAction::PickPremade(p) => {
                self.premades.iter().find(|x| x.prototype == *p).map(|x| x.sheet.clone()).ok_or(Refusal::UnknownPremade)
            }
            CharacterAction::Accept => Ok(self.accept(s)),
        }
    }
}
