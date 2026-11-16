//! The players in a session and their characters, and the system that answers their intents.
//!
//! The host reads intents from the [`Inbox`] and writes deltas to the [`Outbox`]; whoever owns
//! the [`SimApp`](crate::SimApp) moves them to and from a transport. That keeps this crate free
//! of any transport.

use std::collections::HashMap;

use arcanum_rules::{CharacterBlueprint, RulesData};
use bevy_ecs::prelude::*;

use crate::{Creation, LedgerDelta, PlayerId, PlayerIntent};

/// The rules the host applies.
#[derive(Resource, Debug, Clone, Default)]
pub struct SimRules(pub RulesData);

/// Intents received, waiting for the next tick.
#[derive(Resource, Debug, Default)]
pub struct Inbox(pub Vec<(PlayerId, PlayerIntent)>);

/// Deltas to send, in order.
#[derive(Resource, Debug, Default)]
pub struct Outbox(pub Vec<(PlayerId, LedgerDelta)>);

/// Where a player is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPlayer {
    /// Making a character.
    Creating(Creation),
    /// The character is finished; the world has not taken it yet.
    Ready(CharacterBlueprint),
}

/// Every player who has talked to this host.
#[derive(Resource, Debug, Default)]
pub struct Session {
    pub players: HashMap<PlayerId, SessionPlayer>,
}

/// Answers the intents in the [`Inbox`].
///
/// Every character request gets a `CharacterUpdate` with the host's copy and the request's
/// `seq`, refused or not. A finished character also gets `CharacterAccepted`. A player who is
/// `Ready` can only start a new character.
pub fn handle_intents(mut inbox: ResMut<Inbox>, mut outbox: ResMut<Outbox>, mut session: ResMut<Session>, rules: Res<SimRules>) {
    for (player, intent) in inbox.0.drain(..) {
        match intent {
            PlayerIntent::Character { seq, request } => {
                let entry = session.players.entry(player).or_insert_with(|| SessionPlayer::Creating(Creation::new(&rules.0)));
                if matches!(request, crate::CharacterRequest::New) {
                    *entry = SessionPlayer::Creating(Creation::new(&rules.0));
                }
                let SessionPlayer::Creating(creation) = entry else { continue };
                let result = creation.request(&rules.0, &request);
                let creation = creation.clone();
                outbox.0.push((player, LedgerDelta::CharacterUpdate { player, ack: seq, creation }));
                if let Ok(Some(blueprint)) = result {
                    *entry = SessionPlayer::Ready(blueprint);
                    outbox.0.push((player, LedgerDelta::CharacterAccepted { player }));
                }
            }
            // Movement arrives with the world.
            PlayerIntent::MoveTo(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use arcanum_rules::CharacterAction;

    use super::*;
    use crate::{CharacterRequest, SimApp};

    const P: PlayerId = PlayerId(7);

    fn ticked(sim: &mut SimApp, intents: Vec<PlayerIntent>) -> Vec<LedgerDelta> {
        sim.receive(intents.into_iter().map(|i| (P, i)));
        sim.update();
        sim.take_deltas().into_iter().map(|(to, d)| {
            assert_eq!(to, P);
            d
        }).collect()
    }

    fn character(seq: u32, request: CharacterRequest) -> PlayerIntent {
        PlayerIntent::Character { seq, request }
    }

    fn started() -> SimApp {
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.update(); // Starts the clock; no tick yet.
        sim
    }

    #[test]
    fn named_character_is_accepted() {
        let mut sim = started();
        let deltas = ticked(&mut sim, vec![
            character(1, CharacterRequest::Action(CharacterAction::SetName("Virgil".into()))),
            character(2, CharacterRequest::Accept),
        ]);
        assert_eq!(deltas.len(), 3, "{deltas:?}");
        assert!(matches!(&deltas[0], LedgerDelta::CharacterUpdate { ack: 1, creation, .. } if creation.sheet.name == "Virgil"));
        assert!(matches!(&deltas[1], LedgerDelta::CharacterUpdate { ack: 2, .. }));
        assert_eq!(deltas[2], LedgerDelta::CharacterAccepted { player: P });
        let session = sim.app.world().resource::<Session>();
        assert!(matches!(&session.players[&P], SessionPlayer::Ready(b) if b.sheet.name == "Virgil"));
    }

    #[test]
    fn refusals_are_answered_with_the_unchanged_character() {
        let mut sim = started();
        let deltas = ticked(&mut sim, vec![character(1, CharacterRequest::Accept)]);
        assert_eq!(deltas, vec![LedgerDelta::CharacterUpdate { player: P, ack: 1, creation: Creation::new(&RulesData::default()) }]);
    }

    #[test]
    fn a_ready_player_can_only_start_again() {
        let mut sim = started();
        ticked(&mut sim, vec![
            character(1, CharacterRequest::Action(CharacterAction::SetName("Virgil".into()))),
            character(2, CharacterRequest::Accept),
        ]);
        let ignored = ticked(&mut sim, vec![character(3, CharacterRequest::Action(CharacterAction::SetName("Other".into())))]);
        assert!(ignored.is_empty());
        let restarted = ticked(&mut sim, vec![character(4, CharacterRequest::New)]);
        assert!(matches!(&restarted[..], [LedgerDelta::CharacterUpdate { ack: 4, creation, .. }] if creation.sheet.name.is_empty()));
    }

    #[test]
    fn intents_wait_for_a_tick() {
        let mut sim = SimApp::new(RulesData::default());
        sim.use_manual_clock();
        sim.receive([(P, character(1, CharacterRequest::New))]);
        sim.update(); // The first frame runs no tick.
        assert!(sim.take_deltas().is_empty());
        sim.update();
        assert_eq!(sim.take_deltas().len(), 1);
    }
}
