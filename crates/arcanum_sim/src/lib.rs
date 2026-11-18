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

pub mod creation;
pub mod delta;
pub mod gait;
pub mod intent;
pub mod movement;
pub mod net_id;
pub mod pathfinding;
pub mod player;
pub mod plugin;
pub mod save;
pub mod session;
pub mod world;

pub use creation::*;
pub use delta::*;
pub use gait::*;
pub use intent::*;
pub use movement::*;
pub use net_id::*;
pub use pathfinding::*;
pub use player::*;
pub use plugin::*;
pub use save::*;
pub use session::*;
pub use world::*;
