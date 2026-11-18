//! Critters on the map: their sprites, stand and walk animations, and walking between tiles.
//!
//! The host announces each PC with a `Spawn` (its tile, art id and speed stat) and moves it with
//! `Move` (other players' PCs) and `MoveCorrection`. The local PC walks on the client at once
//! ([`WalkLocalPc`]): the client sends the path to the host and walks it without waiting.
//!
//! A walker steps from tile centre to tile centre, each step taking as long as its
//! [`Gait`] says, which is also the host's timing. Its sprite plays the walk art in the step's
//! direction at the gait's frame time, and the stand art (same direction) once it stops. The
//! sprite sits at the tile centre on its frame's hotspot; it is drawn after the objects on its
//! tile, at the depth of whichever of the two tiles of its step is lower on screen.

use std::collections::{HashMap, VecDeque};

use arcanum_formats::art_id::{ANIM_STAND, ArtId, CritterArt};
use arcanum_formats::map::TileLoc;
use arcanum_sim::{Gait, LedgerDelta, NetId, PlayerIntent, find_path, rotation_of_step, walk_art, with_anim};
use bevy::prelude::*;

use crate::animation::ArtAnimation;
use crate::art_loader::ArtFrames;
use crate::isometric::tile_to_world;
use crate::main_menu::AppState;
use crate::sim_bridge::{HostLink, LocalPc};
use crate::vfs_asset::VfsResource;
use crate::world_render::{ClientMap, DEPTH_TILE_ORDER, object_depth};

pub struct ActorsPlugin;

impl Plugin for ActorsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActorDeltas>().init_resource::<WalkLocalPc>().add_systems(OnExit(AppState::InGame), forget_actors).add_systems(
            Update,
            (apply_actor_deltas, start_local_walk, walk_actors, place_actors).chain().run_if(in_state(AppState::InGame)),
        );
    }
}

/// Deltas about actors, kept from the bridge until the world is up to apply them.
#[derive(Resource, Debug, Default)]
pub struct ActorDeltas(pub Vec<LedgerDelta>);

impl ActorDeltas {
    /// Whether this module handles `delta`.
    pub fn wants(delta: &LedgerDelta) -> bool {
        matches!(delta, LedgerDelta::Spawn { .. } | LedgerDelta::Move { .. } | LedgerDelta::MoveCorrection { .. } | LedgerDelta::Despawn { .. })
    }
}

/// A tile for the local PC to walk to: the client plans the path, walks it and tells the host.
#[derive(Resource, Debug, Default)]
pub struct WalkLocalPc {
    pub goal: Option<TileLoc>,
    /// Numbering of the moves sent.
    seq: u32,
}

/// A critter the host told us about.
#[derive(Component, Debug, Clone)]
pub struct Actor {
    pub net_id: NetId,
    /// Standing, rotation 0; the rotation in use is [`Actor::rotation`].
    pub art: ArtId,
    pub gait: Gait,
    pub at: TileLoc,
    pub rotation: usize,
    pub steps: VecDeque<TileLoc>,
    /// Seconds into the step to `steps[0]`.
    pub progress: f64,
}

impl Actor {
    pub fn walking(&self) -> bool {
        !self.steps.is_empty()
    }

    /// How far through the current step, 0 to 1.
    pub fn step_fraction(&self) -> f32 {
        let Some(&next) = self.steps.front() else { return 0.0 };
        let total = self.gait.step_seconds(self.at, next);
        if total <= 0.0 { 0.0 } else { (self.progress / total).clamp(0.0, 1.0) as f32 }
    }

    /// Walks on by `dt` seconds. Returns true if it changed tile.
    pub fn advance(&mut self, dt: f64) -> bool {
        if self.steps.is_empty() {
            self.progress = 0.0;
            return false;
        }
        self.progress += dt;
        let mut moved = false;
        while let Some(&next) = self.steps.front() {
            let needed = self.gait.step_seconds(self.at, next);
            if self.progress < needed {
                break;
            }
            self.progress -= needed;
            self.at = next;
            self.steps.pop_front();
            moved = true;
        }
        if let Some(rotation) = self.steps.front().and_then(|&next| rotation_of_step(self.at, next)) {
            self.rotation = rotation;
        }
        if self.steps.is_empty() {
            self.progress = 0.0;
        }
        moved
    }

    /// Starts along `path` (the tiles after the current one).
    pub fn walk(&mut self, path: impl IntoIterator<Item = TileLoc>) {
        self.steps = path.into_iter().collect();
        self.progress = 0.0;
        if let Some(rotation) = self.steps.front().and_then(|&next| rotation_of_step(self.at, next)) {
            self.rotation = rotation;
        }
    }

    /// The art to show now: walking or standing, in the current rotation.
    pub fn current_art(&self) -> ArtId {
        let anim = if self.walking() { walk_art(self.art) } else { with_anim(self.art, ANIM_STAND) };
        CritterArt { rotation: self.rotation as u32, ..CritterArt::from_id(anim) }.id()
    }
}

fn forget_actors(mut deltas: ResMut<ActorDeltas>, mut walk: ResMut<WalkLocalPc>) {
    deltas.0.clear();
    walk.goal = None;
}

#[allow(clippy::too_many_arguments)]
fn apply_actor_deltas(
    mut commands: Commands,
    mut deltas: ResMut<ActorDeltas>,
    assets: Res<AssetServer>,
    vfs: Option<Res<VfsResource>>,
    pc: Res<LocalPc>,
    mut actors: Query<(Entity, &mut Actor)>,
) {
    if deltas.0.is_empty() {
        return;
    }
    let local = pc.0.map(|(id, _)| id);
    let mut by_id: HashMap<NetId, Entity> = actors.iter().map(|(e, a)| (a.net_id, e)).collect();
    for delta in deltas.0.drain(..) {
        match delta {
            LedgerDelta::Spawn { net_id, position, art, speed, .. } => {
                if by_id.contains_key(&net_id) {
                    continue;
                }
                let art = ArtId(art);
                let gait = match vfs.as_ref().and_then(|v| v.0.read().ok()) {
                    Some(vfs) => Gait::load(art, speed, |p| vfs.read(p).ok()),
                    None => Gait::load(art, speed, |_| None),
                };
                let rotation = CritterArt::from_id(art).rotation as usize;
                let actor = Actor { net_id, art, gait, at: position, rotation, steps: VecDeque::new(), progress: 0.0 };
                let path = CritterArt::from_id(actor.current_art()).path();
                let handle: Handle<ArtFrames> = assets.load(path);
                let entity = commands
                    .spawn((
                        ArtAnimation::new(handle, rotation),
                        actor,
                        Sprite::default(),
                        bevy::sprite::Anchor::CENTER,
                        Transform::default(),
                        DespawnOnExit(AppState::InGame),
                    ))
                    .id();
                by_id.insert(net_id, entity);
            }
            LedgerDelta::Move { net_id, path } if Some(net_id) != local => {
                if let Some(Ok((_, mut a))) = by_id.get(&net_id).map(|&e| actors.get_mut(e)) {
                    // The path may start on the actor's tile or the next one along.
                    let skip = usize::from(path.first() == Some(&a.at));
                    a.walk(path.into_iter().skip(skip));
                }
            }
            LedgerDelta::MoveCorrection { net_id, position, seq } => {
                if Some(net_id) == local {
                    info!("the host stopped the PC at {position:?} (move {seq})");
                }
                if let Some(Ok((_, mut a))) = by_id.get(&net_id).map(|&e| actors.get_mut(e)) {
                    a.at = position;
                    a.walk([]);
                }
            }
            LedgerDelta::Despawn { net_id } => {
                if let Some(e) = by_id.remove(&net_id) {
                    commands.entity(e).despawn();
                }
            }
            _ => {}
        }
    }
}

/// Plans the local PC's path to [`WalkLocalPc::goal`] on the client's map, walks it and sends it
/// to the host.
fn start_local_walk(mut walk: ResMut<WalkLocalPc>, client: Option<Res<ClientMap>>, pc: Res<LocalPc>, link: Option<Res<HostLink>>, mut actors: Query<&mut Actor>) {
    let Some(goal) = walk.goal else { return };
    let (Some(client), Some((net_id, _))) = (client, pc.0) else { return };
    if client.map.sectors.is_empty() {
        return;
    }
    let Some(mut actor) = actors.iter_mut().find(|a| a.net_id == net_id) else { return };
    walk.goal = None;
    // Mid-step, the path starts from the tile being stepped onto.
    let from = actor.steps.front().copied().unwrap_or(actor.at);
    let path = match find_path(&client.map, from, goal) {
        Ok(path) => path,
        Err(e) => {
            warn!("no path to {goal:?}: {e:?}");
            return;
        }
    };
    walk.seq += 1;
    let mut sent = vec![from];
    sent.extend(&path);
    if let Some(link) = link
        && let Err(e) = link.0.send_intent(PlayerIntent::Move { seq: walk.seq, path: sent })
    {
        warn!("cannot reach the host: {e}");
    }
    if actor.steps.front() == Some(&from) {
        // Finish the step under way, then follow the new path.
        actor.steps.truncate(1);
        actor.steps.extend(path);
    } else {
        actor.walk(path);
    }
}

/// Walks every actor on and keeps [`LocalPc`] on the local PC's tile.
fn walk_actors(time: Res<Time>, mut pc: ResMut<LocalPc>, mut actors: Query<&mut Actor>) {
    let dt = f64::from(time.delta_secs());
    for mut actor in &mut actors {
        if actor.advance(dt)
            && let Some((id, at)) = pc.0
            && id == actor.net_id
            && at != actor.at
        {
            pc.0 = Some((id, actor.at));
        }
    }
}

/// Puts each actor's sprite between its tiles and gives it the right art.
fn place_actors(client: Option<Res<ClientMap>>, assets: Res<AssetServer>, mut actors: Query<(&Actor, &mut ArtAnimation, &mut Transform)>) {
    let Some(origin) = client.and_then(|c| c.origin) else { return };
    for (actor, mut animation, mut transform) in &mut actors {
        let here = tile_to_world(actor.at, origin);
        let (pos, depth_tile) = match actor.steps.front() {
            Some(&next) => {
                let there = tile_to_world(next, origin);
                let lower = if there.y < here.y { next } else { actor.at };
                (here.lerp(there, actor.step_fraction()), lower)
            }
            None => (here, actor.at),
        };
        transform.translation = pos.extend(object_depth(depth_tile, origin, false, DEPTH_TILE_ORDER - 1));
        let path = CritterArt::from_id(actor.current_art()).path();
        let walking = actor.walking();
        let handle = assets.get_handle::<ArtFrames>(&path).unwrap_or_else(|| assets.load(path));
        animation.set(&handle, actor.rotation);
        animation.frame_millis = walking.then_some(actor.gait.frame_millis);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(x: u32, y: u32) -> TileLoc {
        TileLoc { x, y }
    }

    fn actor() -> Actor {
        let art = arcanum_sim::pc_art(0, true);
        Actor { net_id: NetId(0), art, gait: Gait::human(17), at: t(10, 10), rotation: 4, steps: VecDeque::new(), progress: 0.0 }
    }

    #[test]
    fn an_actor_walks_at_its_gait_and_faces_its_steps() {
        let mut a = actor();
        a.walk([t(9, 10), t(8, 11)]);
        assert_eq!(a.rotation, 1, "up-right");
        assert_eq!(CritterArt::from_id(a.current_art()).path(), "art/critter/hmm/hmmuwxab.art");
        let first = a.gait.step_seconds(t(10, 10), t(9, 10));
        assert!(!a.advance(first * 0.5));
        assert!((a.step_fraction() - 0.5).abs() < 1e-4);
        assert!(a.advance(first * 0.5 + 1e-9));
        assert_eq!((a.at, a.rotation), (t(9, 10), 2), "next step goes right");
        a.advance(10.0);
        assert_eq!(a.at, t(8, 11));
        assert!(!a.walking());
        assert_eq!(a.rotation, 2, "keeps facing the last step");
        assert_eq!(CritterArt::from_id(a.current_art()).path(), "art/critter/hmm/hmmuwxaa.art", "standing");
    }

    #[test]
    fn a_correction_stops_the_actor_where_the_host_says() {
        let mut a = actor();
        a.walk([t(11, 10), t(12, 10)]);
        a.advance(0.1);
        a.at = t(10, 11);
        a.walk([]);
        assert!(!a.walking());
        assert_eq!(a.step_fraction(), 0.0);
    }
}
