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

/// A pool with a current and a highest value (hit points, fatigue).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pool {
    pub current: i32,
    pub max: i32,
}

impl Pool {
    pub fn full(max: i32) -> Self {
        Self { current: max, max }
    }

    /// How full, from 0 to 1 (0 when the highest value is not positive).
    pub fn fraction(self) -> f32 {
        if self.max <= 0 { 0.0 } else { (self.current as f32 / self.max as f32).clamp(0.0, 1.0) }
    }
}

/// What the in-game interface shows of a PC.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PcStatus {
    pub name: String,
    /// The portrait's file stem under `portrait/` (`portrait/gameport.mes`), if it has one.
    pub portrait: Option<String>,
    pub hit_points: Pool,
    pub fatigue: Pool,
    /// Its level (the hover text shows it).
    #[serde(default)]
    pub level: i32,
    /// Dead until resurrected ([`crate::unconscious`]).
    #[serde(default)]
    pub dead: bool,
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
    /// A lock was picked or jammed: the door's or container's new flags ([`crate::lockpick`]).
    LockChanged { target: crate::LockTarget, flags: i32 },
    /// A script switched an object off or on (actions 0x38 / 0x39): its new `flags`. `mobile`:
    /// `index` is in the sector's mobile objects, else in its own list.
    ObjectFlags { sector: SectorId, index: u32, mobile: bool, flags: i32 },
    /// A script changed an object's art (action 0x3b).
    ObjectArt { sector: SectorId, index: u32, mobile: bool, art: u32 },
    /// A script made an object into prototype `prototype` (action 0x28).
    ObjectTransformed { sector: SectorId, index: u32, mobile: bool, prototype: u32 },
    /// How the player's skill use went: a `mes/skill_ui.mes` line for the message box.
    SkillResult { message: u32 },
    /// Why the player's spell was not cast: a `mes/spell.mes` line for the message box.
    SpellResult { message: u32 },
    /// A line floating over `speaker`'s head ([`crate::dialogue`]): an NPC's (white) or a PC's
    /// chosen option (green), with the voice file to play, if any.
    FloatText { speaker: NetId, text: String, colour: crate::FloatColour, voice: Option<String> },
    /// The player's conversation with `npc` now offers these options.
    DialogueOptions { npc: NetId, options: Vec<String> },
    /// The player's conversation is over.
    DialogueEnded,
    /// A line a script shows in the player's message box ([`crate::script`], action 0x18).
    ScriptMessage { text: String },
    /// The answer to examination `seq` ([`crate::examine`]): whether its name shows (the
    /// examine script did not skip it).
    Examined { seq: u32, shown: bool },
    /// NPCs this player's PC has met, newly in sight ([`crate::dialogue::send_known_npcs`]):
    /// their real names show on hover.
    KnownNpcs { npcs: Vec<NetId> },
    /// Open the written window (0x56bc00) on `kind` ([`crate::WRITTEN_BOOK`] and the rest) from
    /// `line` of its `.mes` (an image: `line` is its interface art).
    Written { kind: u8, line: i32 },
    /// The player may rest: the sleep window opens, sleeping or waiting ([`crate::rest`]).
    RestWindow { mode: crate::RestMode },
    /// Why the player may not rest: a `mes/SleepUI.mes` line for the message box.
    RestRefused { message: u32 },
    /// The player's PC stopped resting.
    RestEnded,
    /// This PC leads the party: out of combat every member moves at its speed, capped
    /// ([`PartyPace`](crate::PartyPace)). Sent to each player once it is known.
    PartyLeader { net_id: NetId },
    /// What a PC's portrait and gauges show (the in-game interface): sent to every player with
    /// the PC's `Spawn`, and again when it changes. Nothing changes it yet: no damage or fatigue
    /// is taken, so the gauges are full.
    PcStatus { net_id: NetId, status: PcStatus },
    /// The host's copy of the player's own PC's sheet after the sheet request numbered `ack`
    /// (applied or refused). The client takes it and replays its later requests on top.
    SheetUpdate { ack: u32, sheet: arcanum_rules::CharacterSheet },
    /// What the player's own PC carries: sent with its spawn (`ack` 0), and after each inventory
    /// request numbered `ack`, taken or refused. The client replays its later requests on top.
    Inventory { net_id: NetId, ack: u32, inventory: crate::Inventory },
    /// An item was dropped on the ground ([`crate::ground`]).
    GroundItemDropped { item: crate::GroundItemId, ground: crate::GroundItem },
    /// An item was picked up off the ground.
    GroundItemTaken { item: crate::GroundItemId },
    /// A pile changed, appeared (`Some`) or went (`None`).
    PileChanged { pile: crate::GroundItemId, contents: Option<crate::Pile> },
    /// The player's PC reached the pile it was sent to: the loot window opens.
    PileOpened { pile: crate::GroundItemId },
    /// One of the map's containers shows open (frame 1) or shut ([`crate::container`]).
    ContainerShown { container: crate::GroundItemId, open: bool },
    /// The player's PC could not open a container: `mes/inven_ui.mes` `message` (locked, jammed,
    /// stuck).
    OpenRefused { message: u32 },
    /// The player's PC could not take an item: `mes/item.mes` `message` (too heavy, no room).
    ItemRefused { message: u32 },
    /// The player's barter window opened or changed ([`crate::barter`]).
    Barter { view: crate::barter::BarterView },
    /// The barter NPC refuses a trade, in the window's text line.
    BarterLine { text: String },
    /// The host closed the player's barter or pick-pocket window (a thief caught,
    /// [`crate::pickpocket`]).
    BarterClosed,
    /// A big move waits for every player's answer ([`crate::big_move`]).
    MoveVote { ask: crate::big_move::VoteAsk },
    /// A vote ended: the move happened (`passed`) or someone declined.
    VoteEnded { id: u32, passed: bool },
    /// A PC jumped to `to` (a teleport): clients put it there at once.
    Teleported { net_id: NetId, to: TileLoc },
    /// A PC's speed stat changed (it raised its dexterity): clients time its walk from it.
    Speed { net_id: NetId, speed: i32 },
    /// A critter's art shows weapon `weapon` (art bits 0-3) and a shield or not (bit 19)
    /// ([`crate::combat::update_weapon_art`]): clients set those bits of its art.
    Armed { net_id: NetId, weapon: u32, shield: bool },
    /// A knocked-down critter got up ([`crate::combat::KnockedDown`]): clients play its get-up.
    GotUp { net_id: NetId },
    /// The fight as it now stands ([`crate::combat`]): who fights in turn order, whose turn it
    /// is and its action points; an empty order: the fight is over.
    Combat { order: Vec<NetId>, turn: Option<NetId>, ap: i32 },
    /// `attacker` attacked `target`: how it went.
    Attacked { attacker: NetId, target: NetId, outcome: crate::combat::AttackOutcome },
    /// `net_id` lost `hit_points` to poison ([`crate::poison`]): clients float "HT n".
    Hurt { net_id: NetId, hit_points: i32 },
    /// An NPC's hit points and fatigue now ([`crate::combat::send_npc_health`]): for the hover's
    /// gauges.
    NpcHealth { net_id: NetId, hit_points: Pool, fatigue: Pool },
    /// A PC started or stopped prowling ([`crate::prowl`]): clients show it in animation 5.
    Prowling { net_id: NetId, on: bool },
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
