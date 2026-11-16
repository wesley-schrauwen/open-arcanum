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
    /// Move the player's PC to a tile.
    MoveTo(TileLoc),
}
