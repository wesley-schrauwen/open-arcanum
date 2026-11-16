//! What a player asks the host to do.
//!
//! The client builds a [`PlayerIntent`] and sends it through the transport. The host validates
//! it, applies it and sends back one or more [`LedgerDelta`]s.

use arcanum_formats::map::TileLoc;
use arcanum_rules::CharacterAction;
use serde::{Deserialize, Serialize};

/// A player's request to the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlayerIntent {
    /// A character-creation or level-up action, applied with `arcanum_rules`.
    CharacterAction(CharacterAction),
    /// Accept the finished character and start the game.
    AcceptCharacter,
    /// Move the player's PC to a tile.
    MoveTo(TileLoc),
}
