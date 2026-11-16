//! In-process transport over channels, for single player and tests.
//!
//! [`LoopbackHost::new`] makes the host end; [`LoopbackHost::connect`] adds a client end for one
//! player. Several clients may connect to one host.

use std::collections::HashMap;
use std::sync::RwLock;

use arcanum_sim::{LedgerDelta, PlayerId, PlayerIntent};
use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::{ClientTransport, HostTransport, TransportError};

/// The host end of a loopback connection.
pub struct LoopbackHost {
    intents_tx: Sender<(PlayerId, PlayerIntent)>,
    intents_rx: Receiver<(PlayerId, PlayerIntent)>,
    clients: RwLock<HashMap<PlayerId, Sender<LedgerDelta>>>,
}

/// A client end of a loopback connection.
pub struct LoopbackClient {
    player: PlayerId,
    intents: Sender<(PlayerId, PlayerIntent)>,
    deltas: Receiver<LedgerDelta>,
}

impl LoopbackHost {
    pub fn new() -> Self {
        let (intents_tx, intents_rx) = unbounded();
        Self { intents_tx, intents_rx, clients: RwLock::default() }
    }

    /// A client end for `player`. Connecting the same player again replaces the old end, which
    /// then stops receiving deltas.
    pub fn connect(&self, player: PlayerId) -> LoopbackClient {
        let (deltas_tx, deltas_rx) = unbounded();
        self.clients.write().unwrap().insert(player, deltas_tx);
        LoopbackClient { player, intents: self.intents_tx.clone(), deltas: deltas_rx }
    }
}

impl Default for LoopbackHost {
    fn default() -> Self {
        Self::new()
    }
}

impl HostTransport for LoopbackHost {
    fn send_delta(&self, player: PlayerId, delta: LedgerDelta) -> Result<(), TransportError> {
        let clients = self.clients.read().unwrap();
        let tx = clients.get(&player).ok_or(TransportError::UnknownPlayer(player))?;
        tx.send(delta).map_err(|_| TransportError::Disconnected)
    }

    fn recv_intents(&self) -> Vec<(PlayerId, PlayerIntent)> {
        self.intents_rx.try_iter().collect()
    }
}

impl ClientTransport for LoopbackClient {
    fn player(&self) -> PlayerId {
        self.player
    }

    fn send_intent(&self, intent: PlayerIntent) -> Result<(), TransportError> {
        self.intents.send((self.player, intent)).map_err(|_| TransportError::Disconnected)
    }

    fn recv_deltas(&self) -> Vec<LedgerDelta> {
        self.deltas.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use arcanum_rules::CharacterAction;
    use arcanum_sim::{CharacterRequest, Creation};

    use super::*;

    fn character(seq: u32, request: CharacterRequest) -> PlayerIntent {
        PlayerIntent::Character { seq, request }
    }

    #[test]
    fn intent_reaches_the_host_tagged_with_the_player() {
        let host = LoopbackHost::new();
        let client = host.connect(PlayerId(7));
        let name = character(1, CharacterRequest::Action(CharacterAction::SetName("Virgil".into())));
        client.send_intent(name.clone()).unwrap();
        client.send_intent(character(2, CharacterRequest::Accept)).unwrap();
        let got = host.recv_intents();
        assert_eq!(got, vec![(PlayerId(7), name), (PlayerId(7), character(2, CharacterRequest::Accept))]);
        assert!(host.recv_intents().is_empty(), "drained");
    }

    #[test]
    fn deltas_go_only_to_their_player() {
        let host = LoopbackHost::new();
        let a = host.connect(PlayerId(1));
        let b = host.connect(PlayerId(2));
        let delta = LedgerDelta::CharacterUpdate { player: PlayerId(1), ack: 1, creation: Creation::default() };
        host.send_delta(PlayerId(1), delta.clone()).unwrap();
        assert_eq!(a.recv_deltas(), vec![delta]);
        assert!(b.recv_deltas().is_empty());
    }

    #[test]
    fn unknown_and_dropped_clients_are_errors() {
        let host = LoopbackHost::new();
        assert_eq!(host.send_delta(PlayerId(9), LedgerDelta::Tick(1)), Err(TransportError::UnknownPlayer(PlayerId(9))));
        drop(host.connect(PlayerId(3)));
        assert_eq!(host.send_delta(PlayerId(3), LedgerDelta::Tick(1)), Err(TransportError::Disconnected));
    }

    #[test]
    fn client_send_fails_after_host_drops() {
        let host = LoopbackHost::new();
        let client = host.connect(PlayerId(1));
        drop(host);
        // The client holds a sender clone, but the receiver went with the host.
        assert_eq!(client.send_intent(character(1, CharacterRequest::Accept)).unwrap_err(), TransportError::Disconnected);
    }
}
