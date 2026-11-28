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

pub mod activity;
pub mod big_move;
pub mod areas;
pub mod blocking;
pub mod clock;
pub mod combat;
pub mod container;
pub mod creation;
pub mod delta;
pub mod examine;
pub mod dialogue;
pub mod doors;
pub mod follow;
pub mod heartbeat;
pub mod gait;
pub mod barter;
pub mod ground;
pub mod inventory;
pub mod keys;
pub mod light;
pub mod lockpick;
pub mod pickpocket;
pub mod poison;
pub mod prowl;
pub mod trap;
pub mod newspaper;
pub mod magick;
pub mod intent;
pub mod movement;
pub mod net_id;
pub mod pathfinding;
pub mod player;
pub mod plugin;
pub mod save;
pub mod script;
pub mod script_host;
pub mod reaction;
pub mod repair;
pub mod rest;
pub mod session;
pub mod unconscious;
pub mod sheet;
pub mod wander;
pub mod world;

pub use activity::*;
pub use blocking::*;
pub use clock::*;
pub use container::*;
pub use creation::*;
pub use delta::*;
pub use dialogue::*;
pub use doors::*;
pub use gait::*;
pub use ground::*;
pub use inventory::*;
pub use keys::*;
pub use lockpick::*;
pub use pickpocket::{SKILL_PICK_POCKET, CANNOT_STEAL};
pub use prowl::{Prowling, SKILL_PROWLING};
pub use trap::*;
pub use magick::*;
pub use unconscious::*;
pub use intent::*;
pub use movement::*;
pub use net_id::*;
pub use pathfinding::*;
pub use player::*;
pub use plugin::*;
pub use save::*;
pub use script::*;
pub use script_host::*;
pub use reaction::*;
pub use rest::*;
pub use session::*;
pub use sheet::*;
pub use wander::*;
pub use world::*;
