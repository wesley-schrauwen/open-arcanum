//! The authoritative game simulation (the host).
//!
//! `arcanum_sim` owns the canonical game state: characters, the loaded map, entity positions,
//! quest flags and combat. It runs in its own headless world with no rendering or audio.
//! In single player the host lives in the client process; in multiplayer it may be a separate
//! process or the same one.
//!
//! Communication with clients goes through the `arcanum_net` transports: the host receives
//! [`PlayerIntent`]s and sends [`LedgerDelta`]s. The client never changes shared state
//! directly.

pub mod delta;
pub mod intent;
pub mod net_id;
pub mod player;
pub mod plugin;

pub use delta::*;
pub use intent::*;
pub use net_id::*;
pub use player::*;
pub use plugin::*;
