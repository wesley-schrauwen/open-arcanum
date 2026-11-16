//! A player's identity in a session.
//!
//! Until Steam ids exist, a player is identified by a random `u64` stored in their config.
//! The host keys session characters by it.

use serde::{Deserialize, Serialize};

/// Identifies a player across the wire. Stable for the duration of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub u64);
