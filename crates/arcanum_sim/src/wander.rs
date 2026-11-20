//! NPCs out of combat: keeping to their standpoint, and wandering round it. Decided by the host.
//!
//! As in the original's idle AI (heartbeat 0x4ad200, standpoint and wander logic 0x4acdb0,
//! `ag_wander` 0x425930):
//! - Each NPC thinks on a heartbeat, sooner the nearer a PC is ([`crate::ai_delay_ms`]), and
//!   only while [`Active`](crate::Active) (within 30 tiles of a PC). NPCs with a leader (none
//!   yet) follow it instead.
//! - Its anchor is its day standpoint from 6:00 to 20:59, else its night one (0x4acd90). At
//!   exactly hours 6 and 21 it thinks only one time in a thousand (the original's throttle).
//! - Its range is [`WANDER_RANGE`] if it wanders (`npc_flags` 0x1000, or 0x2000 for wandering in
//!   the dark), else 1. Further from its anchor than that, it walks back to within 1 tile.
//! - A wanderer within range picks a tile within [`WANDER_RANGE`] of its anchor at random and
//!   walks there if a path of 1 to [`WANDER_RANGE`] steps exists; if not, it tries again 300 ms
//!   later.
//! - An NPC no PC is near that is away from its anchor is put back on it (mode 1, 0x43e770):
//!   nobody sees it, and it is home when someone comes.
//!
//! Distances are a radius (owner's rule; the original uses a square: a 9 x 9 area for wandering).
//!
//! Not reproduced: waypoint patrols (0x4acbb0), seeking darkness (no light model: dark
//! wanderers wander as others do), fidgets and spreading out of a shared tile (0x4364d0), and
//! the marker object at a standpoint (0x4ad140).

use arcanum_formats::map::TileLoc;
use arcanum_formats::object::Value;
use bevy_ecs::prelude::*;

use crate::{Active, GameClock, LoadedMap, Npc, PlayerControlled, Position, Walking, Walkable, ai_delay_ms, find_path, tile_distance, within_radius};

/// `npc_flags`: wanders round its standpoint.
pub const ONF_WANDERS: i32 = 0x1000;
/// `npc_flags`: wanders round its standpoint, keeping to the dark.
pub const ONF_WANDERS_IN_DARK: i32 = 0x2000;
/// How far a wanderer goes from its standpoint, in tiles (0x4acf23).
pub const WANDER_RANGE: u32 = 4;
/// Milliseconds before a wanderer that found no tile tries again.
pub const RETRY_MS: u32 = 300;

/// Where an NPC keeps to and how.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standpoints {
    pub day: TileLoc,
    pub night: TileLoc,
    pub wanders: bool,
    /// Milliseconds to its next think.
    pub next_think_ms: u32,
}

impl Standpoints {
    /// From an NPC's `npc_standpoint_day`/`_night` and `npc_flags` (its prototype's where it has
    /// none); a missing standpoint is where it stands, as the original sets it on placing.
    pub fn of(object: &arcanum_formats::object::Object, prototypes: &arcanum_formats::object::Prototypes, at: TileLoc) -> Self {
        let point = |name| match object.field(name) {
            Some(Value::Int64(Some(packed))) if *packed != 0 => TileLoc::from_packed(*packed),
            _ => at,
        };
        let flags = prototypes.int(object, "npc_flags").unwrap_or(0);
        Self { day: point("npc_standpoint_day"), night: point("npc_standpoint_night"), wanders: flags & (ONF_WANDERS | ONF_WANDERS_IN_DARK) != 0, next_think_ms: 0 }
    }

    /// The anchor at `hour`: the day standpoint from 6:00 to 20:59.
    pub fn anchor(&self, hour: u32) -> TileLoc {
        if (6..21).contains(&hour) { self.day } else { self.night }
    }

    /// How far from its anchor it may be before walking back.
    pub fn range(&self) -> u32 {
        if self.wanders { WANDER_RANGE } else { 1 }
    }
}

/// The host's random numbers (a xorshift; seeded the same each run, for repeatable tests).
#[derive(Resource, Debug, Clone)]
pub struct HostRng(pub u64);

impl Default for HostRng {
    fn default() -> Self {
        Self(0x9e37_79b9_7f4a_7c15)
    }
}

impl HostRng {
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A whole number from `low` to `high`, both included (the original's 0x4cef00).
    pub fn range(&mut self, low: i32, high: i32) -> i32 {
        low + (self.next_u64() % (high - low + 1) as u64) as i32
    }
}

/// What an NPC thinking at `at` decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Thought {
    /// Nothing to do.
    Stay,
    /// Walk this path.
    Walk(Vec<TileLoc>),
    /// Found nowhere to wander: think again soon.
    Retry,
}

/// The idle decision of an NPC at `at` with `standpoints` at `hour` (0x4acdb0).
pub fn think(map: &impl Walkable, rng: &mut HostRng, standpoints: &Standpoints, at: TileLoc, hour: u32) -> Thought {
    if matches!(hour, 6 | 21) && rng.range(1, 1000) != 1 {
        return Thought::Stay;
    }
    let anchor = standpoints.anchor(hour);
    if !within_radius(at, anchor, standpoints.range()) {
        // Back to within 1 of the anchor (ag_move_near_tile).
        return match find_path(map, at, anchor) {
            Ok(path) => {
                let end = path.iter().position(|&t| within_radius(t, anchor, 1)).map_or(path.len(), |i| i + 1);
                Thought::Walk(path[..end].to_vec())
            }
            Err(_) => Thought::Stay,
        };
    }
    if !standpoints.wanders {
        return Thought::Stay;
    }
    let r = WANDER_RANGE as i32;
    let (dx, dy) = (rng.range(-r, r), rng.range(-r, r));
    let Some(target) = at_offset(anchor, dx, dy).filter(|&t| within_radius(t, anchor, WANDER_RANGE)) else { return Thought::Retry };
    match find_path(map, at, target) {
        Ok(path) if !path.is_empty() && path.len() <= WANDER_RANGE as usize => Thought::Walk(path),
        _ => Thought::Retry,
    }
}

fn at_offset(at: TileLoc, dx: i32, dy: i32) -> Option<TileLoc> {
    Some(TileLoc { x: at.x.checked_add_signed(dx)?, y: at.y.checked_add_signed(dy)? })
}

type ThinkerQuery<'a> = (Entity, &'a mut Position, &'a mut Standpoints, Has<Active>, Has<Walking>);

/// Runs each NPC's heartbeat (one tick of time), as described in the module docs.
pub fn npc_idle(
    mut commands: Commands,
    map: Res<LoadedMap>,
    clock: Res<GameClock>,
    mut rng: ResMut<HostRng>,
    pcs: Query<&Position, (With<PlayerControlled>, Without<Npc>)>,
    mut npcs: Query<ThinkerQuery, With<Npc>>,
) {
    let tick_ms = (1000.0 / crate::TICK_HZ) as u32;
    let hour = clock.now.hour();
    let pcs: Vec<TileLoc> = pcs.iter().map(|p| p.0).collect();
    for (entity, mut position, mut standpoints, active, walking) in &mut npcs {
        if !active {
            // Nobody near: home at once if away (0x43e770), once its sector is loaded.
            let anchor = standpoints.anchor(hour);
            if !walking && !within_radius(position.0, anchor, standpoints.range()) && map.sectors.contains_key(&anchor.sector()) {
                position.0 = anchor;
                commands.entity(entity).insert(crate::JustStopped);
            }
            continue;
        }
        standpoints.next_think_ms = standpoints.next_think_ms.saturating_sub(tick_ms);
        if standpoints.next_think_ms > 0 || walking {
            continue;
        }
        let nearest = pcs.iter().map(|&pc| tile_distance(pc, position.0)).min().unwrap_or(u32::MAX);
        standpoints.next_think_ms = ai_delay_ms(nearest);
        match think(&*map, &mut rng, &standpoints, position.0, hour) {
            Thought::Stay => {}
            Thought::Walk(path) => {
                commands.entity(entity).insert(Walking { steps: path.into(), progress: 0.0 });
            }
            Thought::Retry => standpoints.next_think_ms = RETRY_MS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathfinding::tests::{Grid, t};

    fn wanderer(at: TileLoc) -> Standpoints {
        Standpoints { day: at, night: TileLoc { x: at.x + 10, y: at.y }, wanders: true, next_think_ms: 0 }
    }

    #[test]
    fn wanderers_stay_within_range_of_their_standpoint() {
        let map = Grid::parse(&["...................."; 20]);
        let mut rng = HostRng::default();
        let s = wanderer(t(8, 8));
        let mut walked = 0;
        for _ in 0..200 {
            match think(&map, &mut rng, &s, t(8, 8), 12) {
                Thought::Walk(path) => {
                    walked += 1;
                    assert!(path.len() <= WANDER_RANGE as usize);
                    assert!(within_radius(*path.last().unwrap(), t(8, 8), WANDER_RANGE), "{path:?}");
                }
                Thought::Retry | Thought::Stay => {}
            }
        }
        assert!(walked > 100, "{walked}");
    }

    #[test]
    fn npcs_go_back_to_the_standpoint_of_the_hour() {
        let map = Grid::parse(&["...................."; 20]);
        let mut rng = HostRng::default();
        let s = wanderer(t(2, 8));
        // By night the anchor is (12, 8): from (2, 8) it walks back to within 1 of it.
        let Thought::Walk(path) = think(&map, &mut rng, &s, t(2, 8), 23) else { panic!() };
        assert!(within_radius(*path.last().unwrap(), t(12, 8), 1), "{path:?}");
        // One who does not wander stays put within 1 tile.
        let still = Standpoints { wanders: false, ..s };
        assert_eq!(think(&map, &mut rng, &still, t(3, 8), 12), Thought::Stay);
        assert!(matches!(think(&map, &mut rng, &still, t(5, 8), 12), Thought::Walk(_)), "2 tiles off: back");
    }

    #[test]
    fn at_the_turn_of_day_they_mostly_hold_still() {
        let map = Grid::parse(&["...................."; 20]);
        let mut rng = HostRng::default();
        let s = wanderer(t(8, 8));
        let moved = (0..1000).filter(|_| think(&map, &mut rng, &s, t(8, 8), 21) != Thought::Stay).count();
        assert!(moved < 10, "{moved}");
    }
}
