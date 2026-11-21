//! The players in a session and their characters, and the system that answers their intents.
//!
//! The host reads intents from the [`Inbox`] and writes deltas to the [`Outbox`]; whoever owns
//! the [`SimApp`](crate::SimApp) moves them to and from a transport. That keeps this crate free
//! of any transport.

use std::collections::HashMap;

use arcanum_rules::{CharacterBlueprint, RulesData};
use bevy_ecs::prelude::*;

use crate::{Creation, LedgerDelta, MoveRequests, NetId, PlayerId, PlayerIntent, StartMap, WorldState};

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
    /// The player's PC is in the world.
    InWorld(NetId),
}

/// Every player who has talked to this host.
#[derive(Resource, Debug, Default)]
pub struct Session {
    pub players: HashMap<PlayerId, SessionPlayer>,
}

/// Answers the intents in the [`Inbox`]. Moves are passed on to
/// [`apply_moves`](crate::apply_moves) as [`MoveRequests`].
///
/// Every character request gets a `CharacterUpdate` with the host's copy and the request's
/// `seq`, refused or not. A finished character gets `CharacterAccepted` first, so a client
/// that sees the update for its `Accept` without it knows the host refused. The accepted
/// character joins the world at the start position. A player who is `Ready` can only start a
/// new character; one who is `InWorld` can make no character requests.
#[allow(clippy::too_many_arguments)]
pub fn handle_intents(
    mut inbox: ResMut<Inbox>,
    mut outbox: ResMut<Outbox>,
    mut session: ResMut<Session>,
    rules: Res<SimRules>,
    start: Option<Res<StartMap>>,
    mut world: ResMut<WorldState>,
    mut moves: ResMut<MoveRequests>,
    mut clock: ResMut<crate::SyncClock>,
    mut uses: ResMut<crate::UseRequests>,
    mut sheets: ResMut<crate::SheetRequests>,
    mut items: ResMut<crate::InventoryRequests>,
) {
    for (player, intent) in inbox.0.drain(..) {
        match intent {
            PlayerIntent::Character { seq, request } => {
                let entry = session.players.entry(player).or_insert_with(|| SessionPlayer::Creating(Creation::new(&rules.0)));
                if matches!(entry, SessionPlayer::InWorld(_)) {
                    continue;
                }
                if matches!(request, crate::CharacterRequest::New) {
                    *entry = SessionPlayer::Creating(Creation::new(&rules.0));
                }
                let SessionPlayer::Creating(creation) = entry else { continue };
                let result = creation.request(&rules.0, &request);
                let update = LedgerDelta::CharacterUpdate { player, ack: seq, creation: creation.clone() };
                if let Ok(Some(blueprint)) = result {
                    world.add_player(start.as_deref(), player, blueprint.clone());
                    *entry = SessionPlayer::Ready(blueprint);
                    outbox.0.push((player, LedgerDelta::CharacterAccepted { player }));
                }
                outbox.0.push((player, update));
            }
            PlayerIntent::Move { seq, path } => moves.0.push((player, seq, path)),
            PlayerIntent::SyncAck { number } => clock.acked(player, number),
            PlayerIntent::UsePortal { door, seq } => uses.0.push((player, door, seq)),
            PlayerIntent::Sheet { seq, request } => sheets.0.push((player, seq, request)),
            PlayerIntent::Inventory { seq, request } => items.0.push((player, seq, request)),
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
        assert_eq!(deltas[1], LedgerDelta::CharacterAccepted { player: P }, "before the update it answers");
        assert!(matches!(&deltas[2], LedgerDelta::CharacterUpdate { ack: 2, .. }));
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
    fn an_accepted_character_is_saved_on_the_start_map() {
        use arcanum_formats::map::TileLoc;
        let dir = tempfile::tempdir().unwrap();
        let file = crate::SaveFile::in_dir(dir.path(), 5);
        let start = StartMap { map: "Arcanum1-024-fixed".into(), position: TileLoc { x: 92958, y: 82592 }, clock: Default::default() };
        let mut sim = started();
        sim.app.insert_resource(start.clone()).insert_resource(file.clone());
        ticked(&mut sim, vec![
            character(1, CharacterRequest::Action(CharacterAction::SetName("Virgil".into()))),
            character(2, CharacterRequest::Accept),
        ]);
        let saved = crate::WorldSave::read(&file.0).unwrap();
        assert_eq!(saved.map, start.map);
        assert_eq!(saved.players[&P].position, start.position);
        assert_eq!(saved.players[&P].blueprint.sheet.name, "Virgil");
        assert_eq!(saved.tick, 1, "saved at the end of the tick that accepted");
        // Nothing changed since: no second write.
        ticked(&mut sim, vec![]);
        assert_eq!(sim.app.world().resource::<crate::SaveStatus>().writes, 1);
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
