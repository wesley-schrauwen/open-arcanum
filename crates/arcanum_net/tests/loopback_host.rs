//! A client creates a character through an in-process host.

use arcanum_net::{ClientTransport, LocalHost};
use arcanum_rules::{CharacterAction, RulesData};
use arcanum_sim::{CharacterRequest, LedgerDelta, PlayerId, PlayerIntent};

#[test]
fn character_is_accepted_through_the_loopback() {
    let mut host = LocalHost::new(RulesData::default());
    host.sim.use_manual_clock();
    let client = host.connect(PlayerId(7));
    host.update(); // Starts the clock.

    let send = |seq, request| client.send_intent(PlayerIntent::Character { seq, request }).unwrap();
    send(1, CharacterRequest::Action(CharacterAction::SetName("Virgil".into())));
    send(2, CharacterRequest::Accept);
    host.update();

    let deltas = client.recv_deltas();
    assert!(matches!(&deltas[0], LedgerDelta::CharacterUpdate { ack: 1, creation, .. } if creation.sheet.name == "Virgil"));
    assert_eq!(deltas.last(), Some(&LedgerDelta::CharacterAccepted { player: PlayerId(7) }));
}
