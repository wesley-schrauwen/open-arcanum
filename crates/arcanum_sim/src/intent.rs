//! What a player asks the host to do.
//!
//! The client builds a [`PlayerIntent`] and sends it through the transport. The host validates
//! it, applies it and sends back one or more [`LedgerDelta`](crate::LedgerDelta)s.

use arcanum_formats::map::TileLoc;
use serde::{Deserialize, Serialize};

use crate::CharacterRequest;

/// A player's request to the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerIntent {
    /// A character creation request. `seq` counts up per client; the host echoes the last one
    /// it applied so the client can replay the requests still in flight on top.
    Character { seq: u32, request: CharacterRequest },
    /// The path the client is walking its PC along, from its current tile (or one step ahead
    /// of it, or a tile on the route the host is walking) to the goal. `seq` counts up per
    /// client; the host takes only a move newer than the last it took, and reports that number
    /// in each [`StateSync`](crate::LedgerDelta::StateSync). Sent unreliably: the client sends
    /// it again until a sync shows it was taken.
    Move { seq: u32, path: Vec<TileLoc> },
    /// Open or close a door. The PC must be within [`USE_RANGE`](crate::USE_RANGE) of it: the
    /// host waits while the PC walks there on move `seq` (or the last one) and drops the request
    /// if it stops short or a newer move comes.
    UsePortal { door: crate::MapObjectId, seq: u32 },
    /// A change to the player's own PC's sheet from the character screen ([`crate::sheet`]).
    /// `seq` counts up per client; the host answers each with a
    /// [`SheetUpdate`](crate::LedgerDelta::SheetUpdate) carrying it.
    Sheet { seq: u32, request: crate::SheetRequest },
    /// A change to the player's own PC's inventory (the inventory screen). `seq` counts up per
    /// client; the host answers each with an [`Inventory`](crate::LedgerDelta::Inventory)
    /// carrying it.
    Inventory { seq: u32, request: crate::InventoryRequest },
    /// The number of the last state sync the client applied. Sent unreliably, about twice a
    /// second; a client far behind gets a [`Resync`](crate::LedgerDelta::Resync).
    SyncAck { number: u32 },
}

impl PlayerIntent {
    /// Whether the intent must arrive, once and in order. Moves and sync acks may be lost.
    pub fn reliable(&self) -> bool {
        !matches!(self, Self::Move { .. } | Self::SyncAck { .. })
    }
}
