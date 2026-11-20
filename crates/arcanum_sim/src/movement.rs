//! PC movement on the host.
//!
//! Out of combat a client paths and walks its own PC at once, and sends the path as
//! [`PlayerIntent::Move`](crate::PlayerIntent::Move). The host checks it with [`check_path`]
//! (the same step rules as [`find_path`](crate::find_path)), then walks the PC along it at its
//! [`Gait`]. A path it refuses stops the PC where the host has it.
//!
//! Moves arrive unreliably: one may be lost, repeated or overtaken. The host takes a move only
//! if its `seq` is newer than the last it took from that player ([`LastMove`]).
//!
//! Players learn where the PCs are from [`sync_state`]: a [`StateSync`](LedgerDelta::StateSync)
//! with every PC's tile, remaining path and last move taken, at once after anything changes
//! and then every [`SYNC_TICKS_MOVING`] ticks while a PC walks, or every [`SYNC_TICKS_IDLE`].
//! A refused move shows as a PC whose path no longer leads where its client is going; that is
//! the correction.
//!
//! Every step is checked again just before it is taken, against the map: doors close while a
//! path is walked. The host never plans a PC's path: on a blocked step the PC stops, and the
//! client plans again from there.
//!
//! Out of combat, actors (PCs and NPCs) do not block PCs: they walk through each other and may
//! share a tile, so a client is never snapped back because another actor moved. In combat they
//! will block ([`Occupied`]), and the snap-back is enforced; there is no combat yet.
//!
//! The host walks the PC itself, so a client cannot go faster than the host allows. The client
//! walks at once and the host only once the move arrives, so with latency the client is ahead:
//! a new path may also start anywhere on the route the host is still walking. The host walks on
//! to that tile and continues along the new path, at its own pace. A path starting anywhere
//! else, not on or next to the host's tile, is corrected.

use std::collections::{HashMap, HashSet, VecDeque};

use arcanum_formats::map::TileLoc;
use bevy_ecs::prelude::*;

use crate::{Gait, LedgerDelta, LoadedMap, NetId, Outbox, ActorState, PlayerControlled, PlayerId, Position, Session, SessionPlayer, TICK_HZ, Walkable, WorldState, adjacent, step_allowed, walk_fps};

/// Ticks between state syncs while any PC walks (10 a second).
pub const SYNC_TICKS_MOVING: u32 = 3;
/// Ticks between state syncs while every PC stands (one a second).
pub const SYNC_TICKS_IDLE: u32 = 30;

/// The `seq` of the last move request the host took for this PC.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LastMove(pub u32);

/// A client this many syncs behind gets a [`Resync`](LedgerDelta::Resync) (2 s while walking).
pub const RESYNC_BEHIND: u32 = 20;
/// Ticks between keyframes: every player gets a [`Resync`](LedgerDelta::Resync) this often
/// regardless (5 s).
pub const KEYFRAME_TICKS: u32 = 150;

/// When the next state sync goes out, and how far behind each player is.
#[derive(Resource, Debug, Default)]
pub struct SyncClock {
    /// Something changed: sync at the end of this tick.
    pub due: bool,
    /// Ticks since the last sync.
    pub since: u32,
    /// Ticks since the last keyframe.
    pub since_keyframe: u32,
    /// The number of the last sync sent.
    pub number: u32,
    /// Per player: the last sync it reported applying (or the first sent to it), and the
    /// number of the last resync sent to it.
    pub players: HashMap<PlayerId, (u32, Option<u32>)>,
}

impl SyncClock {
    /// `player` reports it applied sync `number`.
    pub fn acked(&mut self, player: PlayerId, number: u32) {
        if let Some((last, _)) = self.players.get_mut(&player)
            && crate::sync_is_newer(number, *last)
            && !crate::sync_is_newer(number, self.number)
        {
            *last = number;
        }
    }

    /// Whether `player` should get a resync now: it is [`RESYNC_BEHIND`] syncs behind, and no
    /// resync was sent in the last [`RESYNC_BEHIND`] syncs.
    fn needs_resync(&self, player: PlayerId) -> bool {
        let Some(&(last, resent)) = self.players.get(&player) else { return false };
        crate::sync_distance(last, self.number) >= RESYNC_BEHIND && resent.is_none_or(|r| crate::sync_distance(r, self.number) >= RESYNC_BEHIND)
    }
}

/// The longest path the host takes in one move.
pub const MAX_PATH_STEPS: usize = 256;

/// A map with some tiles taken by actors, which cannot be walked through. For combat, where
/// actors block (see the module docs); not used yet.
pub struct Occupied<'a, M> {
    pub map: &'a M,
    pub occupied: &'a HashSet<TileLoc>,
}

impl<M: Walkable> Walkable for Occupied<'_, M> {
    fn walkable(&self, at: TileLoc) -> bool {
        !self.occupied.contains(&at) && self.map.walkable(at)
    }
}

/// Why a path was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveRefusal {
    /// No tiles, or more than [`MAX_PATH_STEPS`].
    BadLength,
    /// The path starts neither on the PC's tile nor next to it.
    TooFar,
    /// A step is not allowed: the tiles do not touch, the tile is not walkable, or a diagonal
    /// cuts past an unwalkable tile. Holds the step's destination.
    Blocked(TileLoc),
}

/// Checks a client's path for a PC standing at `from`, and returns the tiles to step on.
///
/// The path may start on `from` (that tile is dropped) or one step from it: the client walks at
/// once and may be a tile ahead of the host.
pub fn check_path(map: &impl Walkable, from: TileLoc, path: &[TileLoc]) -> Result<Vec<TileLoc>, MoveRefusal> {
    if path.is_empty() || path.len() > MAX_PATH_STEPS {
        return Err(MoveRefusal::BadLength);
    }
    if path[0] != from && !adjacent(from, path[0]) {
        return Err(MoveRefusal::TooFar);
    }
    let steps: Vec<TileLoc> = if path[0] == from { path[1..].to_vec() } else { path.to_vec() };
    let mut prev = from;
    for &to in &steps {
        if !step_allowed(map, prev, to) {
            return Err(MoveRefusal::Blocked(to));
        }
        prev = to;
    }
    Ok(steps)
}

/// A path that starts on `route` (the steps a walking PC still has): the route up to that tile,
/// then the path's checked steps. `None` if it does not start on the route or is refused.
pub fn continue_route(map: &impl Walkable, route: Option<&VecDeque<TileLoc>>, path: &[TileLoc]) -> Option<Vec<TileLoc>> {
    let route = route?;
    let i = route.iter().position(|&t| Some(&t) == path.first())?;
    let rest = check_path(map, route[i], path).ok()?;
    Some(route.iter().take(i + 1).copied().chain(rest).collect())
}

/// A PC walking a checked path.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct Walking {
    /// Tiles still to step on.
    pub steps: VecDeque<TileLoc>,
    /// Seconds spent on the step to the next tile.
    pub progress: f64,
}

/// Move requests taken from the inbox by `handle_intents`, for [`apply_moves`].
#[derive(Resource, Debug, Default)]
pub struct MoveRequests(pub Vec<(PlayerId, u32, Vec<TileLoc>)>);

type MoverQuery<'a> = (Entity, &'a NetId, &'a Position, &'a PlayerControlled, Option<&'a Walking>, Option<&'a mut LastMove>, Option<&'a crate::PendingUse>);
/// PCs and NPCs.
type Actor = Or<(With<PlayerControlled>, With<crate::Npc>)>;
type SyncedQuery<'a> = (Entity, &'a NetId, &'a Position, Option<&'a Walking>, Option<&'a LastMove>, Has<PlayerControlled>, Has<JustStopped>);

/// Checks each move request against the player's PC. Accepted: the PC walks the path.
/// Refused: the PC stops where it is. Either way the move is the PC's [`LastMove`] and a sync
/// is due. Requests from players with no PC in the world, and moves no newer than the last
/// taken, are dropped.
#[allow(clippy::too_many_arguments)]
pub fn apply_moves(
    mut commands: Commands,
    mut requests: ResMut<MoveRequests>,
    mut map: ResMut<LoadedMap>,
    vfs: Option<Res<crate::SimVfs>>,
    session: Res<Session>,
    mut clock: ResMut<SyncClock>,
    mut world: ResMut<WorldState>,
    mut pcs: Query<MoverQuery>,
) {
    for (player, seq, path) in requests.0.drain(..) {
        let Some(SessionPlayer::InWorld(net_id)) = session.players.get(&player) else { continue };
        let Some((entity, _, &Position(at), _, walking, last, pending)) = pcs.iter_mut().find(|pc| pc.1 == net_id) else { continue };
        if last.as_ref().is_some_and(|l| seq <= l.0) {
            continue;
        }
        match last {
            Some(mut last) => last.0 = seq,
            None => {
                commands.entity(entity).insert(LastMove(seq));
            }
        }
        clock.due = true;
        // A door use waiting for an older move is dropped: the player went elsewhere.
        if pending.is_some_and(|p| p.seq < seq) {
            commands.entity(entity).remove::<crate::PendingUse>();
        }
        // The path may lead into sectors not loaded yet: load them to check it.
        let vfs_guard = vfs.as_ref().and_then(|v| v.0.read().ok());
        let mut sectors: Vec<_> = path.iter().map(|t| t.sector()).collect();
        sectors.dedup();
        for id in sectors {
            map.load_sector(vfs_guard.as_deref(), id);
        }
        drop(vfs_guard);
        match check_path(&*map, at, &path).or_else(|refusal| continue_route(&*map, walking.map(|w| &w.steps), &path).ok_or(refusal)) {
            Ok(steps) if !steps.is_empty() => {
                // The step under way goes on where it was if the new path takes it too.
                let progress = walking.filter(|w| w.steps.front() == steps.first()).map_or(0.0, |w| w.progress);
                commands.entity(entity).insert(Walking { steps: steps.into(), progress });
            }
            // Standing still, or refused.
            _ => {
                commands.entity(entity).remove::<Walking>();
                record_position(&mut world, player, at);
            }
        }
    }
}

type WalkerQuery<'a> = (Entity, &'a mut Position, Option<&'a PlayerControlled>, Option<&'a Gait>, Option<&'a mut Walking>);

/// An NPC that stopped walking since the last state sync: the next sync includes it once.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct JustStopped;

/// Steps walking actors (PCs and NPCs) along their paths, checking each step against the map
/// just before it is taken (see the module docs). One that reaches the end or cannot go on
/// stops and a sync is due; a PC's position goes into the save. PCs out of combat move at the
/// party's pace.
pub fn walk(mut commands: Commands, map: Res<LoadedMap>, pace: Res<crate::PartyPace>, mut clock: ResMut<SyncClock>, mut world: ResMut<WorldState>, mut pcs: Query<WalkerQuery>) {
    for (entity, mut position, player, gait, walking) in &mut pcs {
        let Some(mut walking) = walking else { continue };
        let own = gait.copied().unwrap_or(Gait::human(walk_fps(8, 0)));
        // Out of combat the party moves at its leader's pace (there is no combat yet).
        let gait = match (player, pace.gait) {
            (Some(_), Some(p)) => own.paced(&p),
            _ => own,
        };
        walking.progress += 1.0 / TICK_HZ;
        let mut stopped = false;
        while let Some(&next) = walking.steps.front() {
            let at = position.0;
            let needed = gait.step_seconds(at, next);
            if walking.progress < needed {
                break;
            }
            if !step_allowed(&*map, at, next) {
                stopped = true;
                break;
            }
            walking.steps.pop_front();
            position.0 = next;
            walking.progress -= needed;
        }
        if stopped || walking.steps.is_empty() {
            commands.entity(entity).remove::<Walking>();
            match player {
                Some(player) => record_position(&mut world, player.0, position.0),
                None => {
                    commands.entity(entity).insert(JustStopped);
                }
            }
            clock.due = true;
        }
    }
}

/// Sends every player in the world a [`StateSync`](LedgerDelta::StateSync) when one is due:
/// at the end of a tick in which something changed, else every [`SYNC_TICKS_MOVING`] ticks while
/// a PC walks and every [`SYNC_TICKS_IDLE`] when none does. Every [`KEYFRAME_TICKS`] the sync
/// is a keyframe: every player gets it as a reliable [`Resync`](LedgerDelta::Resync). Between
/// keyframes, a player whose acknowledgements ([`SyncClock::acked`]) are [`RESYNC_BEHIND`]
/// syncs behind gets one too.
pub fn sync_state(
    mut clock: ResMut<SyncClock>,
    session: Res<Session>,
    mut outbox: ResMut<Outbox>,
    mut commands: Commands,
    pcs: Query<SyncedQuery, Actor>,
    clock_time: Res<crate::GameClock>,
) {
    clock.since += 1;
    clock.since_keyframe += 1;
    let moving = pcs.iter().any(|pc| pc.3.is_some());
    let period = if moving { SYNC_TICKS_MOVING } else { SYNC_TICKS_IDLE };
    let keyframe = clock.since_keyframe >= KEYFRAME_TICKS;
    if !clock.due && !keyframe && clock.since < period {
        return;
    }
    clock.due = false;
    clock.since = 0;
    if keyframe {
        clock.since_keyframe = 0;
    }
    // Every PC; NPCs while they walk and once more when they stop, and all of them in a
    // keyframe, so a client that missed a stop catches up.
    let mut states: Vec<ActorState> = pcs
        .iter()
        .filter(|(_, _, _, walking, _, pc, stopped)| *pc || keyframe || walking.is_some() || *stopped)
        .map(|(entity, &net_id, &Position(position), walking, last, _, stopped)| {
            if stopped {
                commands.entity(entity).remove::<JustStopped>();
            }
            ActorState { net_id, position, path: walking.map(|w| w.steps.iter().copied().collect()).unwrap_or_default(), seq: last.map_or(0, |l| l.0) }
        })
        .collect();
    if states.is_empty() {
        return;
    }
    states.sort_by_key(|s| s.net_id);
    let number = (clock.number + 1) % crate::SYNC_NUMBERS;
    clock.number = number;
    let mut players: Vec<PlayerId> = session.players.iter().filter(|(_, s)| matches!(s, SessionPlayer::InWorld(_))).map(|(&p, _)| p).collect();
    players.sort();
    for player in players {
        clock.players.entry(player).or_insert((number, None));
        let sync = if keyframe || clock.needs_resync(player) {
            clock.players.get_mut(&player).expect("inserted above").1 = Some(number);
            LedgerDelta::Resync { number, time: clock_time.now, actors: states.clone() }
        } else {
            LedgerDelta::StateSync { number, time: clock_time.now, actors: states.clone() }
        };
        outbox.0.push((player, sync));
    }
}

/// Puts a stopped PC's position into the save, which then writes it. An unchanged position
/// does not mark the save changed.
fn record_position(world: &mut ResMut<WorldState>, player: PlayerId, at: TileLoc) {
    let moved = world.0.as_ref().and_then(|s| s.players.get(&player)).is_some_and(|saved| saved.position != at);
    if moved && let Some(saved) = world.0.as_mut().and_then(|s| s.players.get_mut(&player)) {
        saved.position = at;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathfinding::tests::{Grid, t};

    #[test]
    fn a_path_from_the_pc_is_accepted() {
        let map = Grid::parse(&["....", "....", "...."]);
        assert_eq!(check_path(&map, t(0, 0), &[t(0, 0), t(1, 1), t(2, 1)]), Ok(vec![t(1, 1), t(2, 1)]));
        assert_eq!(check_path(&map, t(0, 0), &[t(1, 0), t(2, 0)]), Ok(vec![t(1, 0), t(2, 0)]), "a tile ahead");
        assert_eq!(check_path(&map, t(0, 0), &[t(0, 0)]), Ok(vec![]), "standing still");
    }

    #[test]
    fn bad_paths_are_refused() {
        let map = Grid::parse(&["..#.", "..#.", "...."]);
        assert_eq!(check_path(&map, t(0, 0), &[]), Err(MoveRefusal::BadLength));
        assert_eq!(check_path(&map, t(0, 0), &vec![t(0, 0); MAX_PATH_STEPS + 1]), Err(MoveRefusal::BadLength));
        assert_eq!(check_path(&map, t(0, 0), &[t(2, 2), t(3, 2)]), Err(MoveRefusal::TooFar));
        assert_eq!(check_path(&map, t(0, 0), &[t(1, 0), t(2, 0)]), Err(MoveRefusal::Blocked(t(2, 0))));
        assert_eq!(check_path(&map, t(0, 0), &[t(1, 0), t(3, 0)]), Err(MoveRefusal::Blocked(t(3, 0))), "a jump");
        assert_eq!(check_path(&map, t(1, 2), &[t(2, 2), t(3, 1)]), Ok(vec![t(2, 2), t(3, 1)]), "past (2, 1): only edges stop diagonals");
        assert_eq!(check_path(&map, t(1, 1), &[t(1, 2), t(2, 2), t(3, 2), t(3, 1)]), Ok(vec![t(1, 2), t(2, 2), t(3, 2), t(3, 1)]));
        assert_eq!(check_path(&map, t(1, 2), &[t(2, 1)]), Err(MoveRefusal::Blocked(t(2, 1))));
        assert_eq!(check_path(&map, t(1, 1), &[t(2, 2)]), Ok(vec![t(2, 2)]), "past (2, 1)");
    }
}
