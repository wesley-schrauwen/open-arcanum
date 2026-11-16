//! The client's side of character creation: a preview of the host's copy.
//!
//! Screens never change a sheet themselves. They send [`CharacterAction`]s here and draw what
//! [`CharacterHost::sheet`] holds. Each change is applied at once to a local preview, with the
//! same rules the host uses, and queued as a numbered intent for the host
//! ([`crate::sim_bridge`] sends it). The host answers each one with its copy and the number;
//! the preview becomes that copy with the still-unanswered changes replayed on top. Where the
//! two agree, as they do with the same rules, nothing visible happens.

use std::collections::VecDeque;

use arcanum_rules::{CharacterAction, CharacterBlueprint, CharacterSheet, Refusal, RulesData};
use arcanum_sim::{CharacterRequest, Creation, LedgerDelta, PlayerIntent};
use bevy::prelude::*;

#[derive(Resource, Debug, Clone, Default)]
pub struct CharacterHost {
    /// The host's copy, as of the last answer.
    confirmed: Creation,
    /// `confirmed` with `pending` applied: what the screens show.
    preview: Creation,
    /// Sent but not answered yet.
    pending: VecDeque<(u32, CharacterRequest)>,
    last_seq: u32,
    /// Waiting for the bridge to send.
    outgoing: Vec<PlayerIntent>,
    /// Set once the character is finished (Next on the editor or the picker).
    pub finished: Option<CharacterBlueprint>,
    /// Set when the host accepts the finished character.
    pub accepted: bool,
}

impl CharacterHost {
    /// A fresh "create your own" character, also started on the host.
    pub fn new(rules: &RulesData) -> Self {
        let mut host = Self::default();
        host.reset(rules);
        host
    }

    /// Starts again from the template, here and on the host.
    pub fn reset(&mut self, rules: &RulesData) {
        self.finished = None;
        self.accepted = false;
        let _ = self.request(rules, CharacterRequest::New);
    }

    pub fn sheet(&self) -> &CharacterSheet {
        &self.preview.sheet
    }

    /// Applies one action with the shared rules; on refusal nothing changes or is sent.
    pub fn apply(&mut self, rules: &RulesData, action: CharacterAction) -> Result<(), Refusal> {
        self.request(rules, CharacterRequest::Action(action)).map(|_| ())
    }

    /// Accepts the character and keeps its blueprint.
    pub fn finish(&mut self, rules: &RulesData) -> Result<&CharacterBlueprint, Refusal> {
        let blueprint = self.request(rules, CharacterRequest::Accept)?.expect("Accept gives a blueprint");
        Ok(self.finished.insert(blueprint))
    }

    fn request(&mut self, rules: &RulesData, request: CharacterRequest) -> Result<Option<CharacterBlueprint>, Refusal> {
        let result = self.preview.request(rules, &request)?;
        self.last_seq += 1;
        self.pending.push_back((self.last_seq, request.clone()));
        self.outgoing.push(PlayerIntent::Character { seq: self.last_seq, request });
        Ok(result)
    }

    /// Intents for the host, oldest first.
    pub fn take_outgoing(&mut self) -> Vec<PlayerIntent> {
        std::mem::take(&mut self.outgoing)
    }

    /// Takes a delta from the host. Others' and non-character deltas are ignored.
    pub fn receive(&mut self, rules: &RulesData, delta: &LedgerDelta) {
        match delta {
            LedgerDelta::CharacterUpdate { ack, creation, .. } => {
                self.confirmed = creation.clone();
                while self.pending.front().is_some_and(|(seq, _)| seq <= ack) {
                    self.pending.pop_front();
                }
                self.preview = self.confirmed.clone();
                for (_, request) in &self.pending {
                    let _ = self.preview.request(rules, request);
                }
            }
            LedgerDelta::CharacterAccepted { .. } => self.accepted = true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use arcanum_sim::PlayerId;

    use super::*;

    fn update(ack: u32, creation: Creation) -> LedgerDelta {
        LedgerDelta::CharacterUpdate { player: PlayerId(1), ack, creation }
    }

    #[test]
    fn changes_show_at_once_and_are_sent_numbered() {
        let rules = RulesData::default();
        let mut host = CharacterHost::new(&rules);
        host.apply(&rules, CharacterAction::SetName("Virgil".into())).unwrap();
        assert_eq!(host.sheet().name, "Virgil");
        let sent = host.take_outgoing();
        assert_eq!(sent, vec![
            PlayerIntent::Character { seq: 1, request: CharacterRequest::New },
            PlayerIntent::Character { seq: 2, request: CharacterRequest::Action(CharacterAction::SetName("Virgil".into())) },
        ]);
        assert!(host.take_outgoing().is_empty());
    }

    #[test]
    fn refusals_are_not_sent() {
        let rules = RulesData::default();
        let mut host = CharacterHost::new(&rules);
        host.take_outgoing();
        assert_eq!(host.apply(&rules, CharacterAction::LowerStat(0)), Err(Refusal::BelowAccepted));
        assert_eq!(host.finish(&rules).err(), Some(Refusal::NoName));
        assert!(host.take_outgoing().is_empty());
        assert!(host.finished.is_none());
    }

    #[test]
    fn unanswered_changes_are_replayed_on_the_hosts_copy() {
        let rules = RulesData::default();
        let mut host = CharacterHost::new(&rules); // seq 1
        host.apply(&rules, CharacterAction::SetName("Virgil".into())).unwrap(); // seq 2
        host.apply(&rules, CharacterAction::StepPortrait(1)).unwrap(); // seq 3
        // The host answered seq 2 with a different name (as if it disagreed).
        let mut theirs = Creation::new(&rules);
        theirs.sheet.name = "Host".into();
        host.receive(&rules, &update(2, theirs.clone()));
        let mut expected = theirs.clone();
        expected.apply(&rules, &CharacterAction::StepPortrait(1)).unwrap();
        assert_eq!(host.sheet(), &expected.sheet, "the host's name, the portrait step replayed");
        // Answering seq 3 leaves nothing to replay.
        host.receive(&rules, &update(3, theirs.clone()));
        assert_eq!(host.sheet(), &theirs.sheet);
    }

    #[test]
    fn acceptance_is_recorded_and_reset_clears_it() {
        let rules = RulesData::default();
        let mut host = CharacterHost::new(&rules);
        host.apply(&rules, CharacterAction::SetName("Virgil".into())).unwrap();
        host.finish(&rules).unwrap();
        host.receive(&rules, &LedgerDelta::CharacterAccepted { player: PlayerId(1) });
        assert!(host.accepted && host.finished.is_some());
        host.reset(&rules);
        assert!(!host.accepted && host.finished.is_none());
        assert!(host.sheet().name.is_empty());
    }
}
