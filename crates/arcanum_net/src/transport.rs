//! The two ends of a host-client connection.

use std::fmt;

use arcanum_sim::{LedgerDelta, PlayerId, PlayerIntent};

/// Why a send failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    /// The other end is gone.
    Disconnected,
    /// The host has no connection for this player.
    UnknownPlayer(PlayerId),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => write!(f, "the other end disconnected"),
            Self::UnknownPlayer(p) => write!(f, "no connection for player {}", p.0),
        }
    }
}

impl std::error::Error for TransportError {}

/// The host's end: intents from every player in, deltas to one player out.
pub trait HostTransport: Send + Sync + 'static {
    fn send_delta(&self, player: PlayerId, delta: LedgerDelta) -> Result<(), TransportError>;
    /// Everything received since the last call, in arrival order. Never blocks.
    fn recv_intents(&self) -> Vec<(PlayerId, PlayerIntent)>;
}

/// A client's end: its player's intents out, deltas in.
pub trait ClientTransport: Send + Sync + 'static {
    fn player(&self) -> PlayerId;
    fn send_intent(&self, intent: PlayerIntent) -> Result<(), TransportError>;
    /// Everything received since the last call, in arrival order. Never blocks.
    fn recv_deltas(&self) -> Vec<LedgerDelta>;
}
