//! Followers: NPCs who join a PC through dialogue, follow it and come along on its moves.
//!
//! - **Who leads** (field 0x119 `npc_leader`, getter 0x45de00): kept here as
//!   [`crate::WorldSave::followers`], one [`Follower`] per NPC with its leader's player. A
//!   waiting follower keeps its leader (0x4aa7a0) but is out of the leader's list (0xf1), so it
//!   neither counts nor follows.
//! - **NPC flags** (`npc_flags`, field 0x118): [`WAITING`] 0x8, [`SPREAD_OUT`] 0x10 (clear: stay
//!   close), [`LEFT`] 0x20 (left or refused, the `wt` test), [`FORCED`] 0x400 (joined with
//!   `jo 1`: not counted, never refused). Changes are kept in the sector's changes
//!   ([`crate::SectorDelta::npc_flags`]).
//! - **Joining** (`jo force line`, 0x41624d → [`can_join`] 0x4ad950 → 0x45de90): refused with a
//!   `gd_npc` line by reason ([`refusal_line`], 0x413a90); else the NPC follows and the talk goes
//!   on at NPC line `line`.
//! - **Following** (0x4ac380 → 0x4346a0): farther than 4 tiles (7 spread out) from its leader it
//!   walks to it; 31 tiles or more away it is put on the leader's tile (0x4ad4d0).
//! - **Moves:** a teleport within the map takes the followers onto the same tile (0x4d3760 /
//!   0x4d39a0).
//! - **Death** (0x45da20 at 0x45dbb9-0x45dbf4): a dead follower is out of its leader's list (it
//!   neither follows nor counts; a forced joining, 0x400, is cleared) but keeps its leader;
//!   raised, it joins again if [`rejoins`] lets it, else it is dropped.

use arcanum_formats::map::SectorId;
use arcanum_formats::mes::MesFile;
use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::PlayerId;

/// `npc_flags` 0x8: told to wait (`wa`).
pub const WAITING: i32 = 0x8;
/// `npc_flags` 0x10: spread out (`so`); clear, stay close (`sc`).
pub const SPREAD_OUT: i32 = 0x10;
/// `npc_flags` 0x20: left the party or was refused (`wt`).
pub const LEFT: i32 = 0x20;
/// `npc_flags` 0x400: joined by force (`jo 1`): not counted against the limit.
pub const FORCED: i32 = 0x400;
/// How near a follower keeps to its leader, in tiles (0x4346a0).
pub const STAY_CLOSE_TILES: u32 = 4;
pub const SPREAD_OUT_TILES: u32 = 7;
/// From this far a follower is put on its leader's tile (0x4ad4d0: 0x1f).
pub const LEASH_TILES: u32 = 31;
/// The AI packets (table 0x5f5cb0).
pub const GAME_AI: &str = "Rules/gameai.mes";
/// The packet used when an NPC's is not in the table (builder's call: "generic AI").
pub const GENERIC_AI: u32 = 100;
/// Persuasion's skill number: a Master (rank 3) is never refused for who they are (0x4ad950).
pub const PERSUASION: usize = 11;

/// One NPC following a PC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Follower {
    /// The NPC's home: its map (folder name), sector and place among that map's mobile objects
    /// ([`crate::NpcSource`] there).
    pub map: String,
    pub sector: SectorId,
    pub index: u32,
    pub leader: PlayerId,
    /// Where it is when it went with its leader to another map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away: Option<Away>,
    /// Told to wait: when it gives up and leaves for good (0x4aa7a0 / 0x4aa990).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_until: Option<crate::GameTime>,
    /// Why to warn at the next check (0x4adb50): [`WARN_REACTION`], [`WARN_GOOD`],
    /// [`WARN_EVIL`] (npc_flags 0x10000 with critter bits 0x1000000 / 0x2000000 / 0x4000000).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub warn: u8,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

/// The leader's reaction from the follower dropped.
pub const WARN_REACTION: u8 = 1;
/// The leader's alignment rose / fell.
pub const WARN_GOOD: u8 = 2;
pub const WARN_EVIL: u8 = 4;

/// How long a follower told to wait waits before leaving, by its `npc_wait` (field 0x12c, 2 by
/// default, 0x49b5e7) plus one for a leader trained in Persuasion (table 0x5b5088: days and
/// milliseconds). Six or more: it waits for ever.
pub const WAIT_DELAYS_MS: [u64; 6] = [0, 900_000, 7_200_000, 86_400_000, 7 * 86_400_000, 30 * 86_400_000];
/// The default `npc_wait`.
pub const DEFAULT_NPC_WAIT: i32 = 2;
/// A leader this charismatic is never left (0x4b0ee0 stat 7 at its maximum, 20).
pub const LOYAL_CHARISMA: i32 = 20;

/// How long a follower waits (0x4aa7a0), `None` for ever.
pub fn wait_delay_ms(npc_wait: i32, leader_persuasion_rank: u8, leader_charisma: i32) -> Option<u64> {
    if leader_charisma >= LOYAL_CHARISMA {
        return None;
    }
    let index = usize::try_from(npc_wait + i32::from(leader_persuasion_rank > 0)).ok()?;
    WAIT_DELAYS_MS.get(index).copied()
}

/// What a follower warns of (0x4adb50), in order: reaction near the limit (`gd_npc` 5400,
/// within 20 of D6), alignment too good (5200, within 100 of D7) or too evil (5300, within
/// 100 of D8). A Master of Persuasion and forced followers get no warnings.
pub fn warning(c: &JoinCheck, warn: u8) -> Option<u32> {
    if c.pc_persuasion_rank >= 3 || c.forced {
        return None;
    }
    if warn & WARN_REACTION != 0 && c.reaction <= c.ai.min_reaction + 20 {
        return Some(5400);
    }
    let (npc, pc) = (c.npc_alignment, c.pc_alignment);
    if pc > npc && warn & WARN_GOOD != 0 && pc - npc > c.ai.above - 100 {
        return Some(5200);
    }
    if pc <= npc && warn & WARN_EVIL != 0 && npc - pc > c.ai.below - 100 {
        return Some(5300);
    }
    None
}

/// A follower away from its home map (owner's rule: followers change maps with their leader,
/// as PCs do; the original takes them along too, 0x4d3760).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Away {
    /// The map it is on, and the tile it arrived on.
    pub map: String,
    pub at: arcanum_formats::map::TileLoc,
    /// Its `npc_flags` there (at home they are in the home map's changes).
    pub flags: i32,
}

/// An NPC's home: map, sector, place.
pub type NpcHome = (String, SectorId, u32);

impl Follower {
    pub fn home(&self) -> NpcHome {
        (self.map.clone(), self.sector, self.index)
    }

    pub fn is(&self, home: &NpcHome) -> bool {
        self.map == home.0 && self.sector == home.1 && self.index == home.2
    }
}

impl crate::LoadedMap {
    /// The home of the NPC at `sector`, `index` of this map: its own place, or a guest's home.
    pub fn home_of(&self, sector: SectorId, index: u32) -> NpcHome {
        self.guests.get(&(sector, index)).cloned().unwrap_or_else(|| (self.name.clone().unwrap_or_default(), sector, index))
    }

    /// Where the NPC from `home` is on this map, if it is here.
    pub fn place_of(&self, home: &NpcHome) -> Option<(SectorId, u32)> {
        if let Some((&place, _)) = self.guests.iter().find(|(_, h)| *h == home) {
            return Some(place);
        }
        (self.name.as_deref() == Some(home.0.as_str()) && !self.away.contains(&(home.1, home.2))).then_some((home.1, home.2))
    }

    /// Puts this map's followers in place, after [`crate::LoadedMap::open`]: those from here who
    /// are away are not spawned at home; those away here are read from their home map's
    /// `.mob` files and added to the mobile objects on their tile, as guests.
    pub fn place_followers(&mut self, vfs: Option<&arcanum_vfs::Vfs>, followers: &[Follower]) {
        let Some(name) = self.name.clone() else { return };
        for f in followers {
            if f.map == name && f.away.is_some() {
                self.away.insert((f.sector, f.index));
            }
            let Some(away) = f.away.as_ref().filter(|a| a.map == name) else { continue };
            let Some(mut object) = self.home_mobile(vfs, &f.map, f.sector, f.index) else { continue };
            set_location(&mut object, away.at);
            object.set_int("npc_flags", away.flags);
            let list = self.mobiles.entry(away.at.sector()).or_default();
            list.push(object);
            self.guests.insert((away.at.sector(), list.len() as u32 - 1), f.home());
        }
    }

    /// NPC `index` of sector `sector` of map `map`, as its `.mob` file has it.
    fn home_mobile(&self, vfs: Option<&arcanum_vfs::Vfs>, map: &str, sector: SectorId, index: u32) -> Option<arcanum_formats::object::Object> {
        let mut home = crate::LoadedMap { prototypes: self.prototypes.clone(), ..Default::default() };
        home.read_mobiles(vfs?, map);
        home.mobiles.get(&sector)?.get(index as usize).cloned()
    }
}

/// Moves an object to `at` (field `location`).
fn set_location(object: &mut arcanum_formats::object::Object, at: arcanum_formats::map::TileLoc) {
    let packed = i64::from(at.x) | (i64::from(at.y) << 32);
    if let Some((_, v)) = object.fields.iter_mut().find(|(d, _)| d.name == "location") {
        *v = arcanum_formats::object::Value::Int64(Some(packed));
    }
}

/// Records who the NPC at `place` of the loaded map follows now (`leader`, `None` for no one)
/// and its `npc_flags`: the follower list (the save's), and the flags on a guest's record or in this map's
/// changes. `waits` is a wait just told (`Some(None)`: for ever); without one a waiting
/// follower keeps its time. A guest that leaves goes home at the next load. Returns the sector
/// whose changes were touched.
pub fn set_follower(followers: &mut Vec<Follower>, map: &mut crate::LoadedMap, place: (SectorId, u32), leader: Option<PlayerId>, flags: i32, waits: Option<Option<u64>>, now: crate::GameTime) -> Option<SectorId> {
    let (sector, index) = place;
    let home = map.home_of(sector, index);
    let before = followers.iter().find(|f| f.is(&home)).cloned();
    followers.retain(|f| !f.is(&home));
    if let Some(leader) = leader {
        let away = before.as_ref().and_then(|f| f.away.clone()).map(|a| Away { flags, ..a });
        // Told to wait: until when (0x4aa7a0); rejoined: no more.
        let wait_until = match waits {
            Some(delay) => delay.map(|ms| now.plus_ms(ms)),
            None if flags & WAITING != 0 => before.as_ref().and_then(|f| f.wait_until),
            None => None,
        };
        let warn = before.as_ref().map_or(0, |f| f.warn);
        followers.push(Follower { map: home.0.clone(), sector: home.1, index: home.2, leader, away, wait_until, warn });
    }
    if map.guests.contains_key(&(sector, index)) {
        if let Some(object) = map.mobiles.get_mut(&sector).and_then(|m| m.get_mut(index as usize)) {
            object.set_int("npc_flags", flags);
        }
        None
    } else {
        map.set_npc_flags(sector, index, flags);
        Some(sector)
    }
}

/// A follower's `npc_flags`, wherever it is: away, on its record; at home on the loaded map,
/// there; at home on another map, in that map's kept changes.
pub fn follower_flags(f: &Follower, map: &crate::LoadedMap, other_maps: &std::collections::BTreeMap<String, std::collections::BTreeMap<SectorId, crate::SectorDelta>>) -> i32 {
    if let Some(a) = &f.away {
        return a.flags;
    }
    if map.name.as_deref() == Some(f.map.as_str()) {
        return map.npc_flags(f.sector, f.index).unwrap_or(0);
    }
    other_maps.get(&f.map).and_then(|c| c.get(&f.sector)).and_then(|d| d.npc_flags.get(&f.index)).copied().unwrap_or(0)
}

/// An NPC following a PC now (not waiting), on the NPC's entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Following {
    pub leader: PlayerId,
    pub spread_out: bool,
}

/// The follow columns of an AI packet (`Rules/gameai.mes` D6-D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiFollow {
    /// D6: at or below this reaction the NPC will not follow.
    pub min_reaction: i32,
    /// D7 / D8: how far the PC's alignment may be above / below the NPC's.
    pub above: i32,
    pub below: i32,
    /// D9: how many levels the NPC may be above the PC.
    pub levels: i32,
}

impl Default for AiFollow {
    /// The generic packet 100: `20 1001 1000 2`.
    fn default() -> Self {
        Self { min_reaction: 20, above: 1001, below: 1000, levels: 2 }
    }
}

/// Packet `packet`'s follow columns, else the generic packet's, else the defaults.
pub fn ai_follow(mes: Option<&MesFile>, packet: i32) -> AiFollow {
    let row = |id: u32| -> Option<AiFollow> {
        let numbers: Vec<i32> = mes?.get(id)?.split_whitespace().filter_map(|n| n.parse().ok()).collect();
        Some(AiFollow { min_reaction: *numbers.get(5)?, above: *numbers.get(6)?, below: *numbers.get(7)?, levels: *numbers.get(8)? })
    };
    u32::try_from(packet).ok().and_then(row).or_else(|| row(GENERIC_AI)).unwrap_or_default()
}

/// What deciding a join reads (0x4ad950).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JoinCheck {
    pub pc_persuasion_rank: u8,
    pub pc_charisma: i32,
    pub pc_alignment: i32,
    pub pc_level: i32,
    /// Stat 15 and the followers counted now (0x45e3f0: neither waiting nor forced).
    pub max_followers: i32,
    pub followers: i32,
    /// The charisma of another PC the NPC already follows.
    pub other_leader_charisma: Option<i32>,
    /// `npc_flags` has [`FORCED`].
    pub forced: bool,
    pub reaction: i32,
    pub npc_alignment: i32,
    pub npc_level: i32,
    pub ai: AiFollow,
}

/// Whether NPC `place` of the loaded map, raised from the dead, joins its leader again (0x4b5ba1:
/// 0x45de90 with flag 0, so 0x4ad950 with the count): bound to it, it does (0x4ad973);
/// `reaction` is its reaction to the leader. The count leaves it out (dead, it left the list).
#[allow(clippy::too_many_arguments)]
pub fn rejoins(save: &crate::WorldSave, map: &crate::LoadedMap, rules: &arcanum_rules::RulesData, ai: AiFollow, place: (SectorId, u32), leader: PlayerId, reaction: i32) -> Result<(), Refusal> {
    let Some(object) = map.mobiles.get(&place.0).and_then(|m| m.get(place.1 as usize)) else { return Ok(()) };
    if map.prototypes.int(object, "spell_flags").unwrap_or(0) & crate::combat::OSF_BOUND != 0 {
        return Ok(());
    }
    let Some(saved) = save.players.get(&leader) else { return Ok(()) };
    let sheet = &saved.blueprint.sheet;
    let home = map.home_of(place.0, place.1);
    let followers = save
        .followers
        .iter()
        .filter(|f| f.leader == leader && !f.is(&home) && follower_flags(f, map, &save.other_maps) & (WAITING | FORCED) == 0)
        .filter(|f| map.place_of(&f.home()).is_none_or(|(s, i)| !map.is_dead(s, i)))
        .count();
    let stat = |i: usize| object.ints("critter_stat_base_idx").or_else(|| map.prototypes.of(object)?.ints("critter_stat_base_idx")).and_then(|v| v.get(i).copied()).unwrap_or(0);
    let check = JoinCheck {
        pc_persuasion_rank: sheet.training[PERSUASION],
        pc_charisma: rules.stats(sheet)[7],
        pc_alignment: rules.alignment(sheet),
        pc_level: sheet.level,
        max_followers: rules.derived(sheet).max_followers,
        followers: followers as i32,
        forced: map.npc_flags(place.0, place.1).unwrap_or(0) & FORCED != 0,
        reaction,
        npc_alignment: stat(0x13),
        npc_level: stat(0x11),
        ai,
        ..Default::default()
    };
    can_join(&check, false)
}

/// Why an NPC will not join (0x4ad950's codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    TooGood = 1,
    TooEvil = 2,
    Dislikes = 3,
    HasLeader = 4,
    TooMany = 5,
    NoFollowers = 6,
    TooHighLevel = 7,
}

/// The `gd_npc` block an NPC refuses with (0x413a90, table 0x413bc0).
pub fn refusal_line(r: Refusal) -> u32 {
    match r {
        Refusal::TooGood => 1600,
        Refusal::TooEvil => 1700,
        Refusal::Dislikes => 1800,
        Refusal::HasLeader => 2600,
        Refusal::TooMany => 2700,
        Refusal::NoFollowers => 2800,
        Refusal::TooHighLevel => 2900,
    }
}

/// Whether the NPC joins (0x4ad950). A Master of Persuasion skips the NPC's own conditions; a
/// forced follower skips them and the count; `ignore_count` skips the count.
pub fn can_join(c: &JoinCheck, ignore_count: bool) -> Result<(), Refusal> {
    if c.pc_persuasion_rank < 3 {
        if c.other_leader_charisma.is_some_and(|other| c.pc_charisma <= other) {
            return Err(Refusal::HasLeader);
        }
        if c.forced {
            return Ok(());
        }
        if c.reaction <= c.ai.min_reaction {
            return Err(Refusal::Dislikes);
        }
        let (npc, pc) = (c.npc_alignment, c.pc_alignment);
        if npc < pc {
            if c.ai.above < pc - npc {
                return Err(Refusal::TooGood);
            }
        } else if c.ai.below < npc - pc {
            return Err(Refusal::TooEvil);
        }
        if c.pc_level + c.ai.levels <= c.npc_level {
            return Err(Refusal::TooHighLevel);
        }
    }
    if !ignore_count {
        if c.max_followers == 0 {
            return Err(Refusal::NoFollowers);
        }
        if c.max_followers <= c.followers {
            return Err(Refusal::TooMany);
        }
    }
    Ok(())
}

/// What a conversation knows and changes about the NPC following the talking PC.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FollowTalk {
    /// The NPC follows this PC (waiting or not).
    pub mine: bool,
    /// The charisma of another PC the NPC follows.
    pub other_leader_charisma: Option<i32>,
    /// This PC's followers counted now (neither waiting nor forced).
    pub followers: i32,
    pub ai: AiFollow,
    /// The NPC's alignment and level (stats 0x13 and 0x11 of its base stats).
    pub npc_alignment: i32,
    pub npc_level: i32,
    /// Something changed: the leader or the NPC's flags.
    pub changed: bool,
    /// Joined: the NPC line the talk goes on at (`jo` / `uw`'s second number).
    pub goto: Option<i32>,
    /// Would not join: why.
    pub refused: Option<Refusal>,
    /// The NPC's `npc_wait` (field 0x12c), for `wa`.
    pub npc_wait: i32,
    /// Told to wait this step: for how long (`None`: for ever).
    pub waits: Option<Option<u64>>,
    /// The reaction to this PC dropped this step (a warning at the next check).
    pub reaction_dropped: bool,
    /// The gold of this PC's followers here (not waiting), by their place, as it is now in the
    /// step: `$$` tests count it and payments take from it after the PC's (0x415335 / 0x415cc6).
    pub followers_gold: Vec<((SectorId, u32), i32)>,
}

/// How far a follower lets its leader get before walking after it.
pub fn follow_distance(spread_out: bool) -> u32 {
    if spread_out { SPREAD_OUT_TILES } else { STAY_CLOSE_TILES }
}

/// Puts [`Following`] on the NPCs that follow a PC on this map now (not waiting, not dead), and
/// takes it off the others.
pub fn sync_followers(
    mut commands: Commands,
    world: Res<crate::WorldState>,
    map: Res<crate::LoadedMap>,
    npcs: Query<(Entity, &crate::NpcSource, Option<&Following>), With<crate::Npc>>,
) {
    let (Some(save), Some(here)) = (world.0.as_ref(), map.name.as_deref()) else { return };
    let _ = here;
    for (entity, source, now) in &npcs {
        let home = map.home_of(source.sector, source.index);
        let record = save.followers.iter().find(|f| f.is(&home));
        let flags = map.npc_flags(source.sector, source.index).unwrap_or(0);
        // A dead follower is out of its leader's list (0x45dbd5) but keeps its leader.
        let dead = map.is_dead(source.sector, source.index);
        let want = record.filter(|_| flags & WAITING == 0 && !dead).map(|f| Following { leader: f.leader, spread_out: flags & SPREAD_OUT != 0 });
        match (want, now) {
            (Some(w), Some(n)) if w == *n => {}
            (Some(w), _) => {
                commands.entity(entity).insert(w);
            }
            (None, Some(_)) => {
                commands.entity(entity).remove::<Following>();
            }
            (None, None) => {}
        }
    }
}

/// Followers out of a fight.
type FollowerFilter = (With<crate::Npc>, Without<crate::combat::Fighting>);

/// Followers keep up with their leaders (0x4ac380): farther than [`follow_distance`] they walk
/// until within it; [`LEASH_TILES`] or more away they are put on the leader's tile (0x4ad4d0).
pub fn follow_leaders(
    mut commands: Commands,
    map: Res<crate::LoadedMap>,
    pcs: Query<(&crate::PlayerControlled, &crate::Position), Without<crate::Npc>>,
    mut followers: Query<(Entity, &Following, &mut crate::Position, Has<crate::Walking>), FollowerFilter>,
) {
    for (entity, following, mut position, walking) in &mut followers {
        let Some(leader) = pcs.iter().find(|pc| pc.0.0 == following.leader).map(|pc| pc.1.0) else { continue };
        let distance = crate::tile_distance(position.0, leader);
        if distance >= LEASH_TILES {
            position.0 = leader;
            commands.entity(entity).remove::<crate::Walking>().insert(crate::JustStopped);
            continue;
        }
        let near = follow_distance(following.spread_out);
        if walking || distance <= near {
            continue;
        }
        let Ok(mut path) = crate::pathfinding::find_path(&*map, position.0, leader) else { continue };
        if let Some(stop) = path.iter().position(|&t| crate::tile_distance(t, leader) <= near) {
            path.truncate(stop + 1);
        }
        if !path.is_empty() {
            commands.entity(entity).insert(crate::Walking { steps: path.into(), progress: 0.0 });
        }
    }
}

/// Sets a follower's `npc_flags` wherever it is kept: on its record when away, on the loaded
/// map (and the save) at home, else in its home map's kept changes.
fn set_follower_flags(f: &mut Follower, map: &mut crate::LoadedMap, save: &mut crate::WorldSave, flags: i32) {
    if let Some(a) = &mut f.away {
        a.flags = flags;
        if let Some((s, i)) = map.place_of(&f.home())
            && let Some(o) = map.mobiles.get_mut(&s).and_then(|m| m.get_mut(i as usize))
        {
            o.set_int("npc_flags", flags);
        }
        return;
    }
    if map.name.as_deref() == Some(f.map.as_str()) {
        map.set_npc_flags(f.sector, f.index, flags);
        if let Some(delta) = map.changes.get(&f.sector).cloned() {
            save.sector_changes.insert(f.sector, delta);
        }
    } else {
        save.other_maps.entry(f.map.clone()).or_default().entry(f.sector).or_default().npc_flags.insert(f.index, flags);
    }
}

/// Followers told to wait give up when their time is up (0x4aa990): they leave for good
/// (flag [`LEFT`]), and their reaction to the leader drops by `2 × (charisma - 20)` (0x4b0fd0's
/// maximum for stat 7 is 20).
pub fn expire_waits(mut world: ResMut<crate::WorldState>, mut map: ResMut<crate::LoadedMap>, clock: Res<crate::GameClock>, rules: Res<crate::SimRules>) {
    let now = clock.now;
    let Some(save) = world.0.as_ref() else { return };
    if !save.followers.iter().any(|f| f.wait_until.is_some_and(|t| t <= now)) {
        return;
    }
    let Some(save) = world.0.as_mut() else { return };
    let (gone, kept): (Vec<Follower>, Vec<Follower>) = std::mem::take(&mut save.followers).into_iter().partition(|f| f.wait_until.is_some_and(|t| t <= now));
    save.followers = kept;
    for mut f in gone {
        let flags = follower_flags(&f, &map, &save.other_maps);
        f.away = None;
        set_follower_flags(&mut f, &mut map, save, (flags & !(WAITING | FORCED)) | LEFT);
        if let Some(leader) = save.players.get_mut(&f.leader) {
            let charisma = rules.0.stats(&leader.blueprint.sheet)[7];
            let key = crate::met_key(crate::ScriptObject::Mobile { sector: f.sector, index: f.index });
            let stored = leader.script_state.reactions.get(&key).copied().unwrap_or(crate::BASE_REACTION);
            leader.script_state.reactions.insert(key, stored + 2 * (charisma - LOYAL_CHARISMA));
        }
    }
}

/// How often a follower checks whether it still follows (builder's call: every half second;
/// the original's AI tick, 0x4ac380, comes every 250 ms to 5 s by distance).
pub const CHECK_TICKS: u64 = 15;

/// Followers check whether they still follow (0x4ac380): one that would no longer join
/// (0x4ad950, the count left out) leaves with its refusal line, unless its leader's charisma
/// is 20 or more (0x45dfc0); otherwise, if something worried it, it may warn (0x4adb50).
#[allow(clippy::too_many_arguments)]
pub fn recheck_followers(
    mut world: ResMut<crate::WorldState>,
    mut map: ResMut<crate::LoadedMap>,
    rules: Res<crate::SimRules>,
    session: Res<crate::Session>,
    mut generic: ResMut<crate::GenericLines>,
    mut scripts: ResMut<crate::Scripts>,
    reputations: Res<crate::ReputationTable>,
    vfs: Option<Res<crate::SimVfs>>,
    mut rng: ResMut<crate::HostRng>,
    mut outbox: ResMut<crate::Outbox>,
    tick: Res<crate::SimTick>,
    npcs: Query<(&crate::NetId, &crate::NpcSource), With<Following>>,
) {
    if !tick.0.is_multiple_of(CHECK_TICKS) || npcs.is_empty() {
        return;
    }
    let vfs_guard = vfs.as_ref().and_then(|v| v.0.read().ok());
    let vfs = vfs_guard.as_deref();
    let no_reputations = crate::ReputationEffects::default();
    let reputations = reputations.0.as_ref().unwrap_or(&no_reputations);
    let Some(save) = world.0.as_mut() else { return };
    let mut leaving = Vec::new();
    for (&net_id, source) in &npcs {
        let home = map.home_of(source.sector, source.index);
        let Some(pos) = save.followers.iter().position(|f| f.is(&home)) else { continue };
        let f = save.followers[pos].clone();
        let Some(leader) = save.players.get(&f.leader) else { continue };
        let Some(object) = map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)) else { continue };
        let sheet = &leader.blueprint.sheet;
        let stat = |i: usize| object.ints("critter_stat_base_idx").or_else(|| map.prototypes.of(object)?.ints("critter_stat_base_idx")).and_then(|v| v.get(i).copied()).unwrap_or(0);
        let key = crate::met_key(crate::ScriptObject::Mobile { sector: source.sector, index: source.index });
        let reaction = crate::reaction_to(&leader.script_state, &key, &crate::NpcReaction::of(object, &map.prototypes), &rules.0, sheet, reputations);
        let packet = map.prototypes.int(object, "npc_ai_data").unwrap_or(0);
        let flags = map.prototypes.int(object, "npc_flags").unwrap_or(0);
        let check = JoinCheck {
            pc_persuasion_rank: sheet.training[PERSUASION],
            pc_charisma: rules.0.stats(sheet)[7],
            pc_alignment: rules.0.alignment(sheet),
            pc_level: sheet.level,
            forced: flags & FORCED != 0,
            reaction,
            npc_alignment: stat(0x13),
            npc_level: stat(0x11),
            ai: ai_follow(generic.file(vfs, GAME_AI), packet),
            ..Default::default()
        };
        let line = match can_join(&check, true) {
            Err(r) if check.pc_charisma < LOYAL_CHARISMA => {
                leaving.push(pos);
                Some(refusal_line(r))
            }
            _ if f.warn != 0 => {
                save.followers[pos].warn = 0;
                warning(&check, f.warn)
            }
            _ => None,
        };
        let Some(base) = line else { continue };
        let mut info = crate::npc_details(&map, crate::ScriptObject::Mobile { sector: source.sector, index: source.index }, &mut generic, vfs);
        let overrides = map.attachment(crate::ScriptObject::Mobile { sector: source.sector, index: source.index }, crate::EVENT_OVERRIDE).number;
        if overrides != 0 {
            let library = scripts.0.get_or_insert_with(|| vfs.map(crate::ScriptLibrary::new).unwrap_or_default());
            info.overrides = vfs.and_then(|v| library.dialog(v, overrides)).map(|d| (overrides, d));
        }
        let who = crate::Speaker { npc: &info, pc_name: &sheet.name, pc_female: sheet.gender == arcanum_rules::FEMALE, pc_race: sheet.race };
        if let Some((text, voice)) = crate::npc_line(&mut generic, vfs, &mut rng, &who, crate::Block::Npc, base) {
            crate::ground::broadcast(&session, &mut outbox, crate::LedgerDelta::FloatText { speaker: net_id, text, colour: crate::FloatColour::White, voice });
        }
    }
    // Those who leave: no leader, flag LEFT (0x45dfc0 / 0x45e040), where they stand.
    leaving.sort_unstable();
    for pos in leaving.into_iter().rev() {
        let mut f = save.followers.remove(pos);
        let flags = follower_flags(&f, &map, &save.other_maps);
        set_follower_flags(&mut f, &mut map, save, (flags & !(WAITING | FORCED)) | LEFT);
    }
}

/// Follower `(sector, index)` of this map leaves its leader (0x45dfc0 / 0x45e040): no leader,
/// flag [`LEFT`], where it stands. False when it follows no one.
pub fn dismiss(map: &mut crate::LoadedMap, save: &mut crate::WorldSave, sector: SectorId, index: u32) -> bool {
    let Some(pos) = save.followers.iter().position(|f| f.away.is_none() && map.name.as_deref() == Some(f.map.as_str()) && f.sector == sector && f.index == index) else { return false };
    let mut f = save.followers.remove(pos);
    let flags = follower_flags(&f, map, &save.other_maps);
    set_follower_flags(&mut f, map, save, (flags & !(WAITING | FORCED)) | LEFT);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check() -> JoinCheck {
        JoinCheck { pc_charisma: 8, pc_level: 1, max_followers: 2, reaction: 50, npc_level: 1, ai: AiFollow::default(), ..Default::default() }
    }

    #[test]
    fn the_generic_packet_reads_and_joins_follow_its_columns() {
        let mes = MesFile::parse(b"{100}{0 1000 1000 101 1 20 1001 1000 2 0 0 1001 50 100}{101}{0 0 0 0 0 40 10 10 0 0}").unwrap();
        assert_eq!(ai_follow(Some(&mes), 0), AiFollow::default(), "no packet 0: generic");
        let strict = ai_follow(Some(&mes), 101);
        assert_eq!(strict, AiFollow { min_reaction: 40, above: 10, below: 10, levels: 0 });
        assert_eq!(can_join(&check(), false), Ok(()));
        assert_eq!(can_join(&JoinCheck { reaction: 20, ..check() }, false), Err(Refusal::Dislikes), "at D6");
        let c = JoinCheck { ai: strict, reaction: 50, ..check() };
        assert_eq!(can_join(&JoinCheck { pc_alignment: 11, ..c }, false), Err(Refusal::TooGood));
        assert_eq!(can_join(&JoinCheck { pc_alignment: -11, ..c }, false), Err(Refusal::TooEvil));
        assert_eq!(can_join(&JoinCheck { npc_level: 1, ..c }, false), Err(Refusal::TooHighLevel), "level 1 + 0 <= 1");
        assert_eq!(can_join(&JoinCheck { followers: 2, ..check() }, false), Err(Refusal::TooMany));
        assert_eq!(can_join(&JoinCheck { followers: 2, ..check() }, true), Ok(()), "jo 1: the count is not asked");
        assert_eq!(can_join(&JoinCheck { max_followers: 0, ..check() }, false), Err(Refusal::NoFollowers));
        assert_eq!(can_join(&JoinCheck { other_leader_charisma: Some(8), ..check() }, false), Err(Refusal::HasLeader));
        assert_eq!(can_join(&JoinCheck { other_leader_charisma: Some(7), ..check() }, false), Ok(()), "more charismatic");
        assert_eq!(can_join(&JoinCheck { pc_persuasion_rank: 3, reaction: 0, ..check() }, false), Ok(()), "a Master of Persuasion");
        assert_eq!(refusal_line(Refusal::TooMany), 2700);
    }

    #[test]
    fn waits_last_by_npc_wait_and_persuasion_and_warnings_come_before_leaving() {
        assert_eq!(wait_delay_ms(DEFAULT_NPC_WAIT, 0, 8), Some(7_200_000), "2 h");
        assert_eq!(wait_delay_ms(DEFAULT_NPC_WAIT, 1, 8), Some(86_400_000), "trained: a day");
        assert_eq!(wait_delay_ms(5, 1, 8), None, "past the table: for ever");
        assert_eq!(wait_delay_ms(2, 0, 20), None, "a leader of charisma 20 is waited for");
        let c = JoinCheck { reaction: 35, ai: AiFollow { min_reaction: 20, above: 300, below: 300, levels: 2 }, ..Default::default() };
        assert_eq!(warning(&c, WARN_REACTION), Some(5400), "35 is within 20 of 20");
        assert_eq!(warning(&JoinCheck { reaction: 41, ..c }, WARN_REACTION), None);
        assert_eq!(warning(&JoinCheck { pc_alignment: 250, ..c }, WARN_GOOD), Some(5200), "250 above, within 100 of 300");
        assert_eq!(warning(&JoinCheck { pc_alignment: -250, ..c }, WARN_EVIL), Some(5300));
        assert_eq!(warning(&JoinCheck { pc_alignment: -250, ..c }, WARN_GOOD), None, "only what happened");
        assert_eq!(warning(&JoinCheck { pc_persuasion_rank: 3, ..c }, WARN_REACTION), None, "a Master");
    }
}
