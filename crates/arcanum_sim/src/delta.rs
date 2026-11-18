//! What the host tells clients.
//!
//! Every change to shared state reaches clients as a [`LedgerDelta`]. Clients apply deltas in
//! the order received; they never change shared state on their own.

use arcanum_formats::map::TileLoc;
use serde::{Deserialize, Serialize};

use crate::{Creation, NetId, PlayerId};

/// A host simulation step number.
pub type Tick = u64;

/// What kind of entity a [`LedgerDelta::Spawn`] creates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityKind {
    /// A player character, controlled by `player`.
    Pc { player: PlayerId },
}

/// A change to shared state, sent from the host to clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LedgerDelta {
    /// The host's copy of the player's character after the character request numbered `ack`
    /// (applied or refused). The client takes it and replays its later requests on top.
    CharacterUpdate { player: PlayerId, ack: u32, creation: Creation },
    /// The player's character was accepted; the world is booting.
    CharacterAccepted { player: PlayerId },
    /// The player is now on `map` (a folder under `maps/`). Sent before the player's PC spawns:
    /// the client reads the map's unchanged content from its own copy of the game data.
    EnterMap { map: String },
    /// An entity appeared. `art` is its art id (critters: standing, facing the camera) and
    /// `speed` its speed stat, from which clients time its walk ([`Gait`](crate::Gait)).
    Spawn { net_id: NetId, kind: EntityKind, position: TileLoc, art: u32, speed: i32 },
    /// An entity started along a path.
    Move { net_id: NetId, path: Vec<TileLoc> },
    /// The entity stopped at `position`: the move numbered `seq` (its player's numbering) was
    /// refused, or a step on its path was blocked. Sent to every player.
    MoveCorrection { net_id: NetId, seq: u32, position: TileLoc },
    /// An entity is gone.
    Despawn { net_id: NetId },
    /// The host finished this tick.
    Tick(Tick),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltas_serialise_round_trip() {
        let deltas = [
            LedgerDelta::CharacterUpdate { player: PlayerId(7), ack: 3, creation: Creation::default() },
            LedgerDelta::Spawn { net_id: NetId(1), kind: EntityKind::Pc { player: PlayerId(7) }, position: TileLoc { x: 3, y: 4 }, art: 0x2800_2000, speed: 8 },
            LedgerDelta::Move { net_id: NetId(1), path: vec![TileLoc { x: 3, y: 5 }, TileLoc { x: 4, y: 6 }] },
            LedgerDelta::Tick(12),
        ];
        for delta in deltas {
            let json = serde_json::to_string(&delta).unwrap();
            assert_eq!(serde_json::from_str::<LedgerDelta>(&json).unwrap(), delta);
        }
    }
}
