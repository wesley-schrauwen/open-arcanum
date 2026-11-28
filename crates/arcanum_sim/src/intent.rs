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
    /// Open or close a door, or use scenery (a bed opens the sleep window on it,
    /// [`crate::rest`]). The PC must be within [`USE_RANGE`](crate::USE_RANGE) of it: the
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
    /// Pick up an item off the ground once the PC is within [`crate::USE_RANGE`] of it. `seq` is
    /// the move sent with it (a newer move cancels it).
    PickUp { item: crate::GroundItemId, seq: u32 },
    /// Pick the lock of a door or container ([`crate::lockpick`]) once the PC is within
    /// [`crate::USE_RANGE`] of it, with the lockpick at inventory location `tool` (if any).
    /// `seq` is the move sent with it (a newer move cancels it).
    PickLock { target: crate::LockTarget, tool: Option<i32>, seq: u32 },
    /// Disarm the spotted trap on a door or container ([`crate::trap`]) once the PC is within
    /// [`crate::USE_RANGE`] of it. `seq` is the move sent with it.
    DisarmTrap { target: crate::LockTarget, seq: u32 },
    /// Pick NPC `npc`'s pocket ([`crate::pickpocket`]) once the PC is within
    /// [`crate::USE_RANGE`] of it: the host opens the window. `seq` is the move sent with it.
    PickPocket { npc: crate::NetId, seq: u32 },
    /// The PC's cursor rests on an object with an examine script ([`crate::examine`]): the host
    /// runs it and answers with [`Examined`](crate::LedgerDelta::Examined) carrying `seq`.
    Examine { target: crate::examine::Examined, seq: u32 },
    /// Start or stop prowling ([`crate::prowl`]).
    Prowl { on: bool },
    /// Attack critter `target` ([`crate::combat`]): starts a fight if none runs; on the PC's
    /// turn it strikes, walking next to it first for melee. `seq` is the move sent with it.
    Attack { target: crate::NetId, seq: u32 },
    /// End the PC's turn in a fight.
    EndTurn,
    /// Cast `spell` on a door or container from where the PC stands ([`crate::magick`]).
    Cast { spell: usize, target: crate::LockTarget },
    /// Cast `spell` on critter `target` from where the PC stands ([`crate::magick`]: the healing
    /// spells and Resurrect).
    CastOn { spell: usize, target: crate::NetId },
    /// Open the sleep window ([`crate::rest`]): the host answers with
    /// [`RestWindow`](crate::LedgerDelta::RestWindow) or [`RestRefused`](crate::LedgerDelta::RestRefused).
    RestWindow,
    /// Spends a fate point on option `option` of `mes/fate_ui.mes` (`on`), or takes a waiting
    /// one back (0x4f50c0 / 0x4f51b0; multiplayer packet 0x24).
    Fate { option: u8, on: bool },
    /// Rest for `option`; the host sends [`RestEnded`](crate::LedgerDelta::RestEnded) when done.
    Rest { option: crate::RestOption },
    /// Stop resting.
    StopRest,
    /// Talk to NPC `npc` once the PC is within [`crate::USE_RANGE`] of it ([`crate::dialogue`]).
    /// `seq` is the move sent with it (a newer move cancels it).
    Talk { npc: crate::NetId, seq: u32 },
    /// Choose option `option` (0-based) of the conversation's options.
    Choose { option: usize },
    /// Leave the conversation.
    EndTalk,
    /// Answer vote `id` on a big move ([`crate::big_move`]): go along, or not.
    Vote { id: u32, yes: bool },
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
