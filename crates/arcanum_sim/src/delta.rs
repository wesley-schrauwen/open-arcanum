//! What the host tells clients.
//!
//! Every change to shared state reaches clients as a [`LedgerDelta`]. Clients never change
//! shared state on their own.
//!
//! Most deltas must arrive once and in order ([`LedgerDelta::reliable`]). The exception is
//! [`LedgerDelta::StateSync`], the host's regular account of where the PCs are and where they
//! are walking: it may be lost or arrive late, and the next one replaces it.

use arcanum_formats::map::TileLoc;
use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use arcanum_formats::map::SectorId;

use crate::{Creation, MapObjectId, NetId, PlayerId, SectorDelta};

/// A host simulation step number.
pub type Tick = u64;

/// What kind of entity a [`LedgerDelta::Spawn`] creates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityKind {
    /// A player character, controlled by `player`.
    Pc { player: PlayerId },
    /// A non-player character from the map, made from prototype `prototype`.
    Npc { prototype: u32 },
}

/// State sync numbers roll over here.
pub const SYNC_NUMBERS: u32 = 10_000;

/// Whether sync `number` came after `last`, across the roll-over: it is up to half the range
/// ahead.
pub fn sync_is_newer(number: u32, last: u32) -> bool {
    let ahead = sync_distance(last, number);
    ahead != 0 && ahead < SYNC_NUMBERS / 2
}

/// How many syncs `to` is after `from`, across the roll-over.
pub fn sync_distance(from: u32, to: u32) -> u32 {
    (to % SYNC_NUMBERS + SYNC_NUMBERS - from % SYNC_NUMBERS) % SYNC_NUMBERS
}

/// One PC in a [`LedgerDelta::StateSync`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorState {
    pub net_id: NetId,
    /// The tile it is on.
    pub position: TileLoc,
    /// The tiles still to step on; empty when standing.
    pub path: Vec<TileLoc>,
    /// The number of the last move request from its player that the host took (accepted or
    /// refused); 0 before any.
    pub seq: u32,
}

impl ActorState {
    /// Where the PC will stop: the end of its path, or its tile.
    pub fn destination(&self) -> TileLoc {
        self.path.last().copied().unwrap_or(self.position)
    }
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
    /// Where every PC on the map is and where it is walking. Sent unreliably and often
    /// ([`crate::sync_state`]); `number` counts syncs, rolling over at [`SYNC_NUMBERS`], and a
    /// client keeps the newest ([`sync_is_newer`]) and drops older ones. It also answers moves:
    /// a PC's `seq` is the last move of its player the host took. `time` is the game clock.
    StateSync { number: u32, time: crate::GameTime, actors: Vec<ActorState> },
    /// The same as a `StateSync`, but sent reliably, to a client that reported it is far
    /// behind ([`PlayerIntent::SyncAck`](crate::PlayerIntent::SyncAck)).
    Resync { number: u32, time: crate::GameTime, actors: Vec<ActorState> },
    /// Every change to the map's objects so far (doors opened or closed), sent to a player
    /// after `EnterMap`: the client applies them to its own copy of the map.
    MapChanges { changes: BTreeMap<SectorId, SectorDelta> },
    /// A door opened or closed ([`crate::doors`]).
    PortalChanged { door: MapObjectId, open: bool },
    /// This PC leads the party: out of combat every member moves at its speed, capped
    /// ([`PartyPace`](crate::PartyPace)). Sent to each player once it is known.
    PartyLeader { net_id: NetId },
    /// An entity is gone.
    Despawn { net_id: NetId },
    /// The host finished this tick.
    Tick(Tick),
}

impl LedgerDelta {
    /// Whether the delta must arrive, once and in order. Only state syncs may be lost.
    pub fn reliable(&self) -> bool {
        !matches!(self, Self::StateSync { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_numbers_compare_across_the_roll_over() {
        assert!(sync_is_newer(5, 4));
        assert!(!sync_is_newer(4, 4));
        assert!(!sync_is_newer(3, 4));
        assert!(sync_is_newer(2, 9_998), "after the roll-over");
        assert!(!sync_is_newer(9_998, 2));
        assert_eq!(sync_distance(9_998, 2), 4);
        assert!(!LedgerDelta::StateSync { number: 1, time: Default::default(), actors: vec![] }.reliable());
        assert!(LedgerDelta::Resync { number: 1, time: Default::default(), actors: vec![] }.reliable());
    }

    #[test]
    fn deltas_serialise_round_trip() {
        let deltas = [
            LedgerDelta::CharacterUpdate { player: PlayerId(7), ack: 3, creation: Creation::default() },
            LedgerDelta::Spawn { net_id: NetId(1), kind: EntityKind::Pc { player: PlayerId(7) }, position: TileLoc { x: 3, y: 4 }, art: 0x2800_2000, speed: 8 },
            LedgerDelta::StateSync { number: 9, time: crate::GameTime::at_hour(15), actors: vec![ActorState { net_id: NetId(1), position: TileLoc { x: 3, y: 4 }, path: vec![TileLoc { x: 3, y: 5 }, TileLoc { x: 4, y: 6 }], seq: 2 }] },
            LedgerDelta::Resync { number: 9_999, time: crate::GameTime { days: 3, ms: 7 }, actors: vec![] },
            LedgerDelta::Tick(12),
        ];
        for delta in deltas {
            let json = serde_json::to_string(&delta).unwrap();
            assert_eq!(serde_json::from_str::<LedgerDelta>(&json).unwrap(), delta);
        }
    }
}
