//! PC movement on the host.
//!
//! Out of combat a client paths and walks its own PC at once, and sends the path as
//! [`PlayerIntent::Move`](crate::PlayerIntent::Move). The host checks it with [`check_path`]
//! (the same step rules as [`find_path`](crate::find_path)), then walks the PC along it at
//! [`WALK_TILES_PER_SECOND`] and tells the other players with a `Move`. A path it refuses gets
//! a `MoveCorrection` with the PC's position on the host, and the PC stops there.
//!
//! Every step is checked again just before it is taken, against the map: doors close while a
//! path is walked. The host never plans a PC's path: on a blocked step the PC stops and every
//! player gets a `MoveCorrection`, and the client replans from there.
//!
//! Out of combat, actors (PCs and NPCs) do not block PCs: they walk through each other and may
//! share a tile, so a client is never snapped back because another actor moved. In combat they
//! will block ([`Occupied`]), and the snap-back is enforced; there is no combat yet.
//!
//! The host walks the PC itself, so a client cannot go faster than the host allows: a client
//! that runs ahead sends a path that starts too far from where the host has the PC, and is
//! corrected.

use std::collections::{HashSet, VecDeque};

use arcanum_formats::map::TileLoc;
use bevy_ecs::prelude::*;

use crate::{Gait, LedgerDelta, LoadedMap, NetId, Outbox, PlayerControlled, PlayerId, Position, Session, SessionPlayer, TICK_HZ, Walkable, WorldState, adjacent, step_allowed, walk_fps};

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

/// A PC walking a checked path.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct Walking {
    /// The `seq` of the move this path came from.
    pub seq: u32,
    /// Tiles still to step on.
    pub steps: VecDeque<TileLoc>,
    /// Seconds spent on the step to the next tile.
    pub progress: f64,
}

/// Move requests taken from the inbox by `handle_intents`, for [`apply_moves`].
#[derive(Resource, Debug, Default)]
pub struct MoveRequests(pub Vec<(PlayerId, u32, Vec<TileLoc>)>);

/// Checks each move request against the player's PC. Accepted: the PC walks the path and the
/// other players get a `Move`. Refused: the PC stops where it is and every player gets a
/// `MoveCorrection` (the others may have its last path). Requests from players with no PC in
/// the world are dropped.
pub fn apply_moves(
    mut commands: Commands,
    mut requests: ResMut<MoveRequests>,
    map: Res<LoadedMap>,
    session: Res<Session>,
    mut outbox: ResMut<Outbox>,
    mut world: ResMut<WorldState>,
    pcs: Query<(Entity, &NetId, &Position, &PlayerControlled)>,
) {
    for (player, seq, path) in requests.0.drain(..) {
        let Some(SessionPlayer::InWorld(net_id)) = session.players.get(&player) else { continue };
        let Some((entity, &net_id, &Position(at), _)) = pcs.iter().find(|(_, id, _, _)| **id == *net_id) else { continue };
        match check_path(&*map, at, &path) {
            Ok(steps) => {
                if steps.is_empty() {
                    commands.entity(entity).remove::<Walking>();
                    continue;
                }
                let relay = LedgerDelta::Move { net_id, path: steps.clone() };
                for &other in session.players.keys().filter(|&&p| p != player) {
                    outbox.0.push((other, relay.clone()));
                }
                commands.entity(entity).insert(Walking { seq, steps: steps.into(), progress: 0.0 });
            }
            Err(_) => {
                commands.entity(entity).remove::<Walking>();
                record_position(&mut world, player, at);
                let correction = LedgerDelta::MoveCorrection { net_id, seq, position: at };
                outbox.0.extend(session.players.keys().map(|&p| (p, correction.clone())));
            }
        }
    }
}

type WalkerQuery<'a> = (Entity, &'a NetId, &'a mut Position, &'a PlayerControlled, Option<&'a Gait>, Option<&'a mut Walking>);

/// Steps walking PCs along their paths, in net id order, checking each step against the map
/// just before it is taken (see the module docs). A PC that reaches the end or cannot go on stops, and its
/// position goes into the save.
pub fn walk(
    mut commands: Commands,
    map: Res<LoadedMap>,
    session: Res<Session>,
    mut outbox: ResMut<Outbox>,
    mut world: ResMut<WorldState>,
    mut pcs: Query<WalkerQuery>,
) {
    let mut order: Vec<(NetId, Entity)> = pcs.iter().filter(|pc| pc.5.is_some()).map(|(e, id, ..)| (*id, e)).collect();
    order.sort();
    for (net_id, entity) in order {
        let Ok((_, _, mut position, player, gait, Some(mut walking))) = pcs.get_mut(entity) else { continue };
        let gait = gait.copied().unwrap_or(Gait::human(walk_fps(8, 0)));
        walking.progress += 1.0 / TICK_HZ;
        let mut stopped = false;
        while let Some(&next) = walking.steps.front() {
            let at = position.0;
            let needed = gait.step_seconds(at, next);
            if walking.progress < needed {
                break;
            }
            if !step_allowed(&*map, at, next) {
                let correction = LedgerDelta::MoveCorrection { net_id, seq: walking.seq, position: at };
                outbox.0.extend(session.players.keys().map(|&p| (p, correction.clone())));
                stopped = true;
                break;
            }
            walking.steps.pop_front();
            position.0 = next;
            walking.progress -= needed;
        }
        if stopped || walking.steps.is_empty() {
            commands.entity(entity).remove::<Walking>();
            record_position(&mut world, player.0, position.0);
        }
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
        assert_eq!(check_path(&map, t(1, 2), &[t(2, 2), t(3, 1)]), Err(MoveRefusal::Blocked(t(3, 1))), "cuts past (2, 1)");
        assert_eq!(check_path(&map, t(1, 1), &[t(1, 2), t(2, 2), t(3, 2), t(3, 1)]), Ok(vec![t(1, 2), t(2, 2), t(3, 2), t(3, 1)]));
        assert_eq!(check_path(&map, t(1, 2), &[t(2, 1)]), Err(MoveRefusal::Blocked(t(2, 1))));
        assert_eq!(check_path(&map, t(1, 1), &[t(2, 2)]), Err(MoveRefusal::Blocked(t(2, 2))), "cuts past (2, 1)");
    }
}
