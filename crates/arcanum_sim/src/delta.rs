//! What the host tells clients.
//!
//! Every change to shared state reaches clients as a [`LedgerDelta`]. Clients apply deltas in
//! the order received; they never change shared state on their own.

use arcanum_formats::map::TileLoc;
use arcanum_rules::CharacterSheet;
use serde::{Deserialize, Serialize};

use crate::{NetId, PlayerId};

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
    /// The player's character sheet after an action (creation preview, stat change). The
    /// client replaces its preview with it.
    CharacterUpdate { player: PlayerId, sheet: CharacterSheet },
    /// The player's character was accepted; the world is booting.
    CharacterAccepted { player: PlayerId },
    /// An entity appeared.
    Spawn { net_id: NetId, kind: EntityKind, position: TileLoc },
    /// An entity started along a path.
    Move { net_id: NetId, path: Vec<TileLoc> },
    /// A move was rejected; the entity is at `position`.
    MoveCorrection { net_id: NetId, position: TileLoc },
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
            LedgerDelta::CharacterUpdate { player: PlayerId(7), sheet: CharacterSheet::default() },
            LedgerDelta::Spawn { net_id: NetId(1), kind: EntityKind::Pc { player: PlayerId(7) }, position: TileLoc { x: 3, y: 4 } },
            LedgerDelta::Move { net_id: NetId(1), path: vec![TileLoc { x: 3, y: 5 }, TileLoc { x: 4, y: 6 }] },
            LedgerDelta::Tick(12),
        ];
        for delta in deltas {
            let json = serde_json::to_string(&delta).unwrap();
            assert_eq!(serde_json::from_str::<LedgerDelta>(&json).unwrap(), delta);
        }
    }
}
