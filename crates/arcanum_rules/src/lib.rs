//! Character rules from the original game, without Bevy.
//!
//! The client and (later) the host call the same functions: [`RulesData`] holds the tables read
//! once from the game data, a [`CharacterSheet`] is one character, and every change is an
//! operation on `RulesData` that returns a new sheet or a [`Refusal`].

pub mod action;
pub mod blueprint;
pub mod data;
pub mod ops;
pub mod sheet;
pub mod tables;

pub use action::*;
pub use blueprint::*;
pub use data::*;
pub use ops::*;
pub use sheet::*;
pub use tables::*;
