//! Critters on the map: their sprites, stand and walk animations, and walking between tiles.
//!
//! The host announces each PC with a `Spawn` (its tile, art id and speed stat), reliably. Where
//! the PCs are and where they walk comes in state syncs (`StateSync`, unreliable and frequent;
//! `Resync`, reliable: keyframes and catch-ups). The client applies only a sync newer than the
//! last it applied ([`SyncState`]) and reports that number to the host twice a second.
//!
//! The local PC walks on the client at once ([`WalkLocalPc`], set by a left click on the map):
//! the client sends the path to the host and walks it without waiting. Moves are unreliable, so
//! the newest is sent again every [`MOVE_RESEND_SECS`] until a sync shows the host took it.
//!
//! A sync about the local PC that answers the newest move is a correction when the host's PC
//! will stop somewhere else than the client's ([`Actor::sync_local`]). Other PCs follow the
//! sync unless they are within a step of it on the same route ([`Actor::follow`]). A corrected
//! actor's sprite slides from where it was drawn, at its walking speed, instead of jumping.
//! After a correction of the local PC the client plans once more to its goal.
//!
//! A walker steps from tile centre to tile centre, each step taking as long as its
//! [`Gait`] says, which is also the host's timing. Its sprite plays the walk art in the step's
//! direction at the gait's frame time, and the stand art (same direction) once it stops. The
//! sprite sits at the tile centre on its frame's hotspot; it is drawn after the objects on its
//! tile, at the depth of whichever of the two tiles of its step is lower on screen.

use std::collections::{HashMap, VecDeque};

use arcanum_formats::art_id::{ANIM_WALK, ArtId, ArtKind, CritterArt};
use arcanum_formats::map::TileLoc;
use arcanum_formats::object::{TYPE_PORTAL, TYPE_WALL};
use arcanum_sim::{Gait, GameTime, LedgerDelta, MapObjectId, NetId, ActorState, PlayerIntent, USE_RANGE, Walkable, find_path, rotation_of_step, sync_is_newer, walk_art};
use bevy::prelude::*;

use crate::animation::ArtAnimation;
use crate::art_loader::ArtFrames;
use crate::isometric::{tile_to_world, world_to_tile};
use crate::main_menu::AppState;
use crate::sim_bridge::{HostLink, LocalPc};
use crate::vfs_asset::VfsResource;
use crate::world_render::{ClientMap, DEPTH_TILE_ORDER, MapObject, object_depth};

pub struct ActorsPlugin;

impl Plugin for ActorsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActorDeltas>()
            .init_resource::<WalkLocalPc>()
            .init_resource::<SyncState>()
            .init_resource::<WorldClock>()
            .init_resource::<Party>()
            .add_systems(OnExit(AppState::InGame), forget_actors)
            .add_systems(
                Update,
                (click_to_move, apply_actor_deltas, pace_party, run_clock, start_local_walk, resend_move, ack_syncs, walk_actors, place_actors).chain().run_if(in_state(AppState::InGame)),
            );
    }
}

/// Deltas about actors, kept from the bridge until the world is up to apply them.
#[derive(Resource, Debug, Default)]
pub struct ActorDeltas(pub Vec<LedgerDelta>);

impl ActorDeltas {
    /// Whether this module handles `delta`.
    pub fn wants(delta: &LedgerDelta) -> bool {
        matches!(delta, LedgerDelta::Spawn { .. } | LedgerDelta::StateSync { .. } | LedgerDelta::Resync { .. } | LedgerDelta::Despawn { .. } | LedgerDelta::PartyLeader { .. })
    }
}

/// The party as this client knows it (owner's rules, [`arcanum_sim::PartyPace`]): out of combat
/// every PC moves at the leader's speed, capped; each keeps its own walk frames.
#[derive(Resource, Debug, Default)]
pub struct Party {
    pub leader: Option<NetId>,
    /// The leader's walk at the party's speed.
    pub pace: Option<Gait>,
    /// Art and speed stat of every actor spawned, to work out the pace from.
    known: HashMap<NetId, (ArtId, i32)>,
}

/// Puts every PC on the party's pace (out of combat; there is no combat yet).
fn pace_party(party: Res<Party>, mut actors: Query<&mut Actor>) {
    let Some(pace) = party.pace else { return };
    for mut actor in &mut actors {
        if actor.pc {
            let paced = actor.own_gait.paced(&pace);
            if actor.gait != paced {
                actor.gait = paced;
            }
        }
    }
}

/// Seconds between sync acknowledgements.
pub const SYNC_ACK_SECS: f32 = 0.5;
/// Seconds before a move the host has not taken is sent again.
pub const MOVE_RESEND_SECS: f32 = 0.2;

/// The game clock as this client knows it: the time from the newest sync, run on at the game's
/// speed until the next. It decides which day and night locks hold on the client's map.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
pub struct WorldClock {
    pub now: Option<GameTime>,
    /// Game milliseconds not yet added.
    carry: f64,
}

impl WorldClock {
    /// Runs on by `dt` real seconds.
    pub fn advance(&mut self, dt: f64) {
        let Some(now) = self.now.as_mut() else { return };
        self.carry += dt * 1000.0 * f64::from(arcanum_sim::GAME_SPEED);
        let whole = self.carry.floor();
        self.carry -= whole;
        *now = now.plus_ms(whole as u64);
    }

    /// Takes the host's time.
    pub fn set(&mut self, time: GameTime) {
        self.now = Some(time);
        self.carry = 0.0;
    }
}

/// Runs the client's clock on and has its map's day and night locks follow it.
fn run_clock(time: Res<Time>, mut clock: ResMut<WorldClock>, client: Option<ResMut<ClientMap>>) {
    clock.advance(f64::from(time.delta_secs()));
    if let (Some(now), Some(mut client)) = (clock.now, client) {
        let day = arcanum_sim::lock_day(now.hour());
        if client.map.lock_day != day {
            client.map.set_lock_day(day);
        }
    }
}

/// The newest state sync applied, and when it was last reported.
#[derive(Resource, Debug, Default)]
pub struct SyncState {
    pub last: Option<u32>,
    since_ack: f32,
}

impl SyncState {
    /// Takes sync `number` if it is newer than the last applied. Returns whether to apply it.
    pub fn take(&mut self, number: u32) -> bool {
        if self.last.is_some_and(|last| !sync_is_newer(number, last)) {
            return false;
        }
        self.last = Some(number);
        true
    }
}

/// A tile for the local PC to walk to: the client plans the path, walks it and tells the host.
#[derive(Resource, Debug, Default)]
pub struct WalkLocalPc {
    pub goal: Option<TileLoc>,
    /// Numbering of the moves sent.
    seq: u32,
    /// The goal of the last move sent, and whether it was already planned again after a
    /// correction.
    last_goal: Option<(TileLoc, bool)>,
    /// The newest move, until a sync shows the host took it, and seconds since it was sent.
    pending: Option<(PlayerIntent, f32)>,
    /// A door to use, and its tile: the PC walks to within [`USE_RANGE`] of it first.
    pub door: Option<(MapObjectId, TileLoc)>,
}

impl WalkLocalPc {
    /// Takes the host's account of move `seq`. Returns false if it answers an older move than
    /// the last one sent, so is to be ignored; otherwise the host has the newest move, which
    /// need not be sent again.
    pub fn answered(&mut self, seq: u32) -> bool {
        if seq < self.seq {
            return false;
        }
        self.pending = None;
        true
    }

    /// The host stopped the local PC at `position`, short of where the client is going. The
    /// first correction after a move plans again to its goal, from `position`.
    pub fn corrected(&mut self, position: TileLoc) {
        if let Some((goal, false)) = self.last_goal
            && goal != position
        {
            self.goal = Some(goal);
            self.last_goal = Some((goal, true));
        }
    }

    /// Records that move `seq + 1` to `goal` is being sent and returns its number.
    fn sending(&mut self, goal: TileLoc) -> u32 {
        let again = matches!(self.last_goal, Some((g, true)) if g == goal);
        self.seq += 1;
        self.last_goal = Some((goal, again));
        self.seq
    }

    /// A new goal from the player.
    fn click(&mut self, goal: TileLoc) {
        self.goal = Some(goal);
        self.last_goal = None;
        self.door = None;
    }

    /// The player clicked door `door` on tile `at`.
    fn click_door(&mut self, door: MapObjectId, at: TileLoc) {
        self.goal = None;
        self.last_goal = None;
        self.door = Some((door, at));
    }
}

/// A critter the host told us about.
#[derive(Component, Debug, Clone)]
pub struct Actor {
    pub net_id: NetId,
    /// Standing, rotation 0; the rotation in use is [`Actor::rotation`].
    pub art: ArtId,
    /// The walk it moves at now: its own, or (a PC out of combat) the party's pace.
    pub gait: Gait,
    /// Its own walk, from its art and speed stat.
    pub own_gait: Gait,
    /// A player's PC (part of the party), not an NPC.
    pub pc: bool,
    pub at: TileLoc,
    pub rotation: usize,
    pub steps: VecDeque<TileLoc>,
    /// Seconds into the step to `steps[0]`.
    pub progress: f64,
    /// Where the sprite is drawn relative to its place on the path, in world units: left by a
    /// correction, and shrinking to nothing at walking speed.
    pub slide: Vec2,
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

    /// Where it will stop: the end of its steps, or its tile.
    pub fn destination(&self) -> TileLoc {
        self.steps.back().copied().unwrap_or(self.at)
    }

    /// Follows a host sync for a PC this client does not control. Within a step of the host on
    /// the same route it walks on as it is; otherwise it takes the host's tile and path,
    /// sliding there. Returns true if it was moved.
    pub fn follow(&mut self, host: &ActorState) -> bool {
        if host.destination() == self.destination() {
            let next = self.steps.front().copied();
            let close = self.at == host.position || next == Some(host.position) || host.path.first() == Some(&self.at);
            if close {
                return false;
            }
        }
        if self.steps.front() == Some(&host.position) {
            // Still stepping onto the host's tile: finish that step, then the host's path.
            self.steps.truncate(1);
            self.steps.extend(host.path.iter().copied());
            return false;
        }
        self.correct(host.position);
        self.walk(host.path.iter().copied());
        true
    }

    /// Checks a host sync for the local PC against where the client is taking it. Returns
    /// true, having moved to the host's tile and path, if the host's PC will stop somewhere
    /// else: the move was refused or a step was blocked.
    pub fn sync_local(&mut self, host: &ActorState) -> bool {
        if host.destination() == self.destination() {
            return false;
        }
        self.correct(host.position);
        self.walk(host.path.iter().copied());
        true
    }

    /// Where the actor is drawn relative to the centre of its tile, in world units: between its
    /// tile and the next.
    pub fn drawn_offset(&self) -> Vec2 {
        let base = match self.steps.front() {
            Some(&next) => tile_to_world(next, self.at) * self.step_fraction(),
            None => Vec2::ZERO,
        };
        base + self.slide
    }

    /// Puts the actor on `position`, standing, its sprite sliding there from where it is drawn.
    pub fn correct(&mut self, position: TileLoc) {
        self.slide = self.drawn_offset() + tile_to_world(self.at, position);
        self.at = position;
        self.walk([]);
    }

    /// Walking speed in world units a second (the mean over the rotations).
    fn speed(&self) -> f32 {
        let per_frame = self.gait.pixels_per_frame.iter().sum::<f64>() / 8.0;
        (per_frame * 1000.0 / f64::from(self.gait.frame_millis.max(1))) as f32
    }

    /// Shrinks [`Actor::slide`] by `dt` seconds of walking.
    fn settle(&mut self, dt: f32) {
        let left = self.slide.length() - self.speed() * dt;
        self.slide = if left <= 0.0 { Vec2::ZERO } else { self.slide.normalize() * left };
    }

    /// The art to show now: the walk animation while walking, else the art it was spawned with
    /// (an NPC may stand in another animation, such as lying wounded), in the current rotation.
    /// Critters, monsters and unique NPCs share the animation and rotation bits; other art (an
    /// NPC drawn as scenery) is shown as it is.
    pub fn current_art(&self) -> ArtId {
        match self.art.kind() {
            ArtKind::Critter => {
                let art = if self.walking() { walk_art(self.art) } else { self.art };
                CritterArt { rotation: self.rotation as u32, ..CritterArt::from_id(art) }.id()
            }
            ArtKind::Monster | ArtKind::UniqueNpc => {
                let anim = if self.walking() { ANIM_WALK } else { (self.art.0 >> 6) & 0x1f };
                with_anim_bits(self.art, anim, self.rotation as u32)
            }
            _ => self.art,
        }
    }

    /// The frame to hold while standing: the one its art id names.
    pub fn standing_frame(&self) -> usize {
        ((self.art.0 >> 14) & 0x1f) as usize
    }
}

/// `art` with animation `anim`, rotation `rotation` and frame 0.
fn with_anim_bits(art: ArtId, anim: u32, rotation: u32) -> ArtId {
    ArtId((art.0 & !(0x1f << 6 | 7 << 11 | 0x1f << 14)) | anim << 6 | (rotation & 7) << 11)
}

fn forget_actors(mut deltas: ResMut<ActorDeltas>, mut walk: ResMut<WalkLocalPc>, mut sync: ResMut<SyncState>, mut clock: ResMut<WorldClock>, mut party: ResMut<Party>) {
    deltas.0.clear();
    *party = Party::default();
    *clock = WorldClock::default();
    *walk = WalkLocalPc::default();
    *sync = SyncState::default();
}

#[allow(clippy::too_many_arguments)]
fn apply_actor_deltas(
    mut commands: Commands,
    mut deltas: ResMut<ActorDeltas>,
    vfs: Option<Res<VfsResource>>,
    pc: Res<LocalPc>,
    mut walk: ResMut<WalkLocalPc>,
    mut sync: ResMut<SyncState>,
    mut clock: ResMut<WorldClock>,
    mut party: ResMut<Party>,
    mut actors: Query<(Entity, &mut Actor)>,
) {
    if deltas.0.is_empty() {
        return;
    }
    let local = pc.0.map(|(id, _)| id);
    let mut by_id: HashMap<NetId, Entity> = actors.iter().map(|(e, a)| (a.net_id, e)).collect();
    for delta in deltas.0.drain(..) {
        match delta {
            LedgerDelta::Spawn { net_id, position, art, speed, kind } => {
                if by_id.contains_key(&net_id) {
                    continue;
                }
                let art = ArtId(art);
                party.known.insert(net_id, (art, speed));
                let gait = match vfs.as_ref().and_then(|v| v.0.read().ok()) {
                    Some(vfs) => Gait::load(art, speed, |p| vfs.read(p).ok()),
                    None => Gait::load(art, speed, |_| None),
                };
                let rotation = CritterArt::from_id(art).rotation as usize;
                let pc = matches!(kind, arcanum_sim::EntityKind::Pc { .. });
                let actor = Actor { net_id, art, gait, own_gait: gait, pc, at: position, rotation, steps: VecDeque::new(), progress: 0.0, slide: Vec2::ZERO };
                // The art is set once the art index is in (`place_actors`).
                let entity = commands
                    .spawn((
                        ArtAnimation::new(Handle::default(), rotation),
                        actor,
                        Sprite::default(),
                        bevy::sprite::Anchor::CENTER,
                        Transform::default(),
                        DespawnOnExit(AppState::InGame),
                    ))
                    .id();
                by_id.insert(net_id, entity);
            }
            LedgerDelta::PartyLeader { net_id } => {
                let Some(&(art, speed)) = party.known.get(&net_id) else { continue };
                let pace = match vfs.as_ref().and_then(|v| v.0.read().ok()) {
                    Some(vfs) => Gait::load(art, arcanum_sim::party_speed(speed), |p| vfs.read(p).ok()),
                    None => Gait::load(art, arcanum_sim::party_speed(speed), |_| None),
                };
                party.leader = Some(net_id);
                party.pace = Some(pace);
            }
            LedgerDelta::StateSync { number, time, actors: states } | LedgerDelta::Resync { number, time, actors: states } => {
                if !sync.take(number) {
                    continue;
                }
                clock.set(time);
                for host in &states {
                    let Some(Ok((_, mut a))) = by_id.get(&host.net_id).map(|&e| actors.get_mut(e)) else { continue };
                    if Some(host.net_id) != local {
                        a.follow(host);
                    } else if walk.answered(host.seq) && a.sync_local(host) {
                        info!("the host has the PC at {:?}, going to {:?} (move {})", host.position, host.destination(), host.seq);
                        if host.path.is_empty() {
                            walk.corrected(host.position);
                        }
                    }
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

/// A click to make once the world is up, at this window position (logical pixels): for
/// screenshot checks, which take no input. It waits `frames` frames after the sectors are in (90 from `DEBUG_ARCANUM_CLICK`),
/// for the camera to reach the PC.
#[derive(Resource, Debug, Clone, Copy)]
pub struct DebugClick {
    pub at: Vec2,
    pub frames: u32,
}

/// A left click on the map walks the local PC to the tile under the cursor (0x54de50, the game
/// window's mouse handler). With Shift held the click does nothing, as in the original.
#[allow(clippy::too_many_arguments)]
fn click_to_move(
    mut commands: Commands,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    debug: Option<ResMut<DebugClick>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    client: Option<Res<ClientMap>>,
    mut walk: ResMut<WalkLocalPc>,
    objects: Query<(Entity, &Sprite, &bevy::sprite::Anchor, &GlobalTransform, &MapObject)>,
    images: Res<Assets<Image>>,
) {
    let Some(origin) = client.filter(|c| !c.map.sectors.is_empty()).and_then(|c| c.origin) else { return };
    let cursor = match debug {
        Some(mut click) if click.frames > 0 => {
            click.frames -= 1;
            return;
        }
        Some(click) => {
            commands.remove_resource::<DebugClick>();
            click.at
        }
        None if buttons.just_pressed(MouseButton::Left) && !keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) => {
            let Some(cursor) = windows.iter().find_map(Window::cursor_position) else { return };
            cursor
        }
        None => return,
    };
    let Some(world) = cameras.iter().find_map(|(camera, at)| camera.viewport_to_world_2d(at, cursor).ok()) else { return };
    // An object under the cursor comes first (0x4f28a0); only doors can be used so far, and a
    // click on anything else is a click on the ground. Walls are passed over: a wall beside a
    // door is drawn over part of it, and walls take no clicks (not traced in the original).
    let candidates = objects.iter().filter(|o| o.4.object_type != TYPE_WALL).map(|(e, s, a, t, _)| (e, s, a, t));
    let hit = crate::picking::pick(world, candidates, &images);
    if let Some((_, _, _, _, object)) = hit.and_then(|e| objects.get(e).ok())
        && object.object_type == TYPE_PORTAL
        && let Some(door) = object.id
    {
        debug!("click at {cursor} is on door {door:?}");
        walk.click_door(door, object.location);
        return;
    }
    if let Some(goal) = world_to_tile(world, origin) {
        debug!("click at {cursor} goes to {goal:?}");
        walk.click(goal);
    }
}

/// Plans the local PC's path to [`WalkLocalPc::goal`] on the client's map, walks it and sends it
/// to the host.
fn start_local_walk(mut walk: ResMut<WalkLocalPc>, client: Option<Res<ClientMap>>, pc: Res<LocalPc>, link: Option<Res<HostLink>>, mut actors: Query<&mut Actor>) {
    if walk.goal.is_none() && walk.door.is_none() {
        return;
    }
    let (Some(client), Some((net_id, _))) = (client, pc.0) else { return };
    if client.map.sectors.is_empty() {
        return;
    }
    let Some(mut actor) = actors.iter_mut().find(|a| a.net_id == net_id) else { return };
    let goal = walk.goal.take();
    let door = walk.door.take();
    // Mid-step, the path starts from the tile being stepped onto.
    let from = actor.steps.front().copied().unwrap_or(actor.at);
    let send = |intent: PlayerIntent| {
        if let Some(link) = &link
            && let Err(e) = link.0.send_intent(intent)
        {
            warn!("cannot reach the host: {e}");
        }
    };
    let path = match (goal, door) {
        (_, Some((door, at))) if within(from, at, USE_RANGE) => {
            // Already near enough: use it from here.
            send(PlayerIntent::UsePortal { door, seq: walk.seq });
            return;
        }
        (_, Some((_, at))) => match path_near(&client.map, from, at) {
            Some(path) => path,
            None => {
                warn!("no way to the door at {at:?}");
                return;
            }
        },
        (Some(goal), None) => match find_path(&client.map, from, goal) {
            Ok(path) => path,
            Err(e) => {
                warn!("no path to {goal:?}: {e:?}");
                return;
            }
        },
        (None, None) => return,
    };
    let goal = path.last().copied().unwrap_or(from);
    let seq = walk.sending(goal);
    let mut sent = vec![from];
    sent.extend(&path);
    let intent = PlayerIntent::Move { seq, path: sent };
    walk.pending = Some((intent.clone(), 0.0));
    send(intent);
    if let Some((door, _)) = door {
        send(PlayerIntent::UsePortal { door, seq });
    }
    if actor.steps.front() == Some(&from) {
        // Finish the step under way, then follow the new path.
        actor.steps.truncate(1);
        actor.steps.extend(path);
    } else {
        actor.walk(path);
    }
}

/// Whether `a` and `b` are within `range` tiles of each other (a radius, as the host measures).
fn within(a: TileLoc, b: TileLoc, range: u32) -> bool {
    arcanum_sim::within_radius(a, b, range)
}

/// A path from `from` that ends within [`USE_RANGE`] of `target` (`ag_move_near_obj`): the path
/// to `target` cut at its first tile in range, or else the path to the nearest free tile in
/// range.
fn path_near(map: &impl Walkable, from: TileLoc, target: TileLoc) -> Option<Vec<TileLoc>> {
    if within(from, target, USE_RANGE) {
        return Some(Vec::new());
    }
    let cut = |path: Vec<TileLoc>| {
        let end = path.iter().position(|&t| within(t, target, USE_RANGE)).map_or(path.len(), |i| i + 1);
        path[..end].to_vec()
    };
    if let Ok(path) = find_path(map, from, target) {
        return Some(cut(path));
    }
    let r = USE_RANGE as i32;
    let mut near: Vec<TileLoc> = (-r..=r)
        .flat_map(|dy| (-r..=r).map(move |dx| (dx, dy)))
        .filter_map(|(dx, dy)| Some(TileLoc { x: target.x.checked_add_signed(dx)?, y: target.y.checked_add_signed(dy)? }))
        .filter(|&t| map.walkable(t) && within(t, target, USE_RANGE))
        .collect();
    near.sort_by_key(|t| (t.x.abs_diff(from.x).max(t.y.abs_diff(from.y)), t.y, t.x));
    near.into_iter().find_map(|t| find_path(map, from, t).ok())
}

/// Sends the newest move again every [`MOVE_RESEND_SECS`] until a sync shows the host took it.
fn resend_move(time: Res<Time>, mut walk: ResMut<WalkLocalPc>, link: Option<Res<HostLink>>) {
    let Some((intent, since)) = walk.pending.as_mut() else { return };
    *since += time.delta_secs();
    if *since < MOVE_RESEND_SECS {
        return;
    }
    *since = 0.0;
    if let Some(link) = link {
        let _ = link.0.send_intent(intent.clone());
    }
}

/// Tells the host the newest sync applied, every [`SYNC_ACK_SECS`].
fn ack_syncs(time: Res<Time>, mut sync: ResMut<SyncState>, link: Option<Res<HostLink>>) {
    let Some(number) = sync.last else { return };
    sync.since_ack += time.delta_secs();
    if sync.since_ack < SYNC_ACK_SECS {
        return;
    }
    sync.since_ack = 0.0;
    if let Some(link) = link {
        let _ = link.0.send_intent(PlayerIntent::SyncAck { number });
    }
}

/// Walks every actor on and keeps [`LocalPc`] on the local PC's tile.
fn walk_actors(time: Res<Time>, mut pc: ResMut<LocalPc>, mut actors: Query<&mut Actor>) {
    let dt = f64::from(time.delta_secs());
    for mut actor in &mut actors {
        if actor.slide != Vec2::ZERO {
            actor.settle(time.delta_secs());
        }
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
fn place_actors(
    client: Option<Res<ClientMap>>,
    assets: Res<AssetServer>,
    vfs: Option<Res<VfsResource>>,
    mut exists: Local<HashMap<String, bool>>,
    mut actors: Query<(&Actor, &mut ArtAnimation, &mut Transform)>,
) {
    let Some(client) = client else { return };
    let Some(origin) = client.origin else { return };
    let depth_origin = client.depth_origin.unwrap_or(origin);
    for (actor, mut animation, mut transform) in &mut actors {
        let here = tile_to_world(actor.at, origin);
        let pos = here + actor.drawn_offset();
        let depth_tile = match actor.steps.front() {
            Some(&next) if tile_to_world(next, origin).y < here.y => next,
            _ => actor.at,
        };
        transform.translation = pos.extend(object_depth(depth_tile, depth_origin, false, DEPTH_TILE_ORDER - 1));
        let art = actor.current_art();
        let Some(mut path) = client.art_path(art) else { continue };
        // A body with no art for this animation (two have no standing art) shows its walk.
        let mut there = |p: &String| *exists.entry(p.clone()).or_insert_with(|| vfs.as_ref().and_then(|v| v.0.read().ok()).is_none_or(|v| v.exists(p)));
        if !there(&path)
            && matches!(art.kind(), ArtKind::Monster | ArtKind::UniqueNpc)
            && let Some(walk) = client.art_path(with_anim_bits(art, ANIM_WALK, actor.rotation as u32))
            && there(&walk)
        {
            path = walk;
        }
        let walking = actor.walking();
        let handle = assets.get_handle::<ArtFrames>(&path).unwrap_or_else(|| assets.load(path));
        let changed = animation.art != handle;
        animation.set(&handle, actor.rotation);
        animation.frame_millis = walking.then_some(actor.gait.frame_millis);
        // Standing holds the frame the art id names; walking plays.
        animation.playing = walking;
        if !walking && changed {
            animation.clock.frame = actor.standing_frame();
        }
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
        Actor { net_id: NetId(0), art, gait: Gait::human(17), own_gait: Gait::human(17), pc: true, at: t(10, 10), rotation: 4, steps: VecDeque::new(), progress: 0.0, slide: Vec2::ZERO }
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
        let step = a.gait.step_seconds(t(10, 10), t(11, 10));
        a.advance(step / 2.0);
        let drawn = tile_to_world(t(10, 10), t(0, 0)) + a.drawn_offset();
        a.correct(t(10, 11));
        assert!(!a.walking());
        assert_eq!(a.step_fraction(), 0.0);
        assert_eq!(tile_to_world(t(10, 11), t(0, 0)) + a.drawn_offset(), drawn, "drawn where it was");
        let distance = a.slide.length();
        a.settle(0.1);
        assert!(a.slide.length() < distance, "slides towards the host's tile");
        a.settle(10.0);
        assert_eq!(a.slide, Vec2::ZERO, "and stops there");
    }

    #[test]
    fn only_the_newest_move_is_answered_and_planned_once_more() {
        let mut w = WalkLocalPc::default();
        w.click(t(20, 20));
        w.goal = None;
        assert_eq!(w.sending(t(20, 20)), 1);
        assert_eq!(w.sending(t(20, 20)), 2, "a second click before the answer");
        w.pending = Some((PlayerIntent::Move { seq: 2, path: vec![] }, 0.0));
        assert!(!w.answered(1), "answers the older move");
        assert!(w.pending.is_some(), "still to be sent again");
        assert!(w.answered(2));
        assert!(w.pending.is_none());
        w.corrected(t(11, 10));
        assert_eq!(w.goal, Some(t(20, 20)), "plans again from the host's tile");
        w.goal = None;
        assert_eq!(w.sending(t(20, 20)), 3);
        w.corrected(t(12, 10));
        assert_eq!(w.goal, None, "only once");
        w.click(t(30, 30));
        w.goal = None;
        w.sending(t(30, 30));
        w.corrected(t(30, 30));
        assert_eq!(w.goal, None, "already at the goal");
    }

    #[test]
    fn syncs_apply_only_when_newer_across_the_roll_over() {
        let mut s = SyncState::default();
        assert!(s.take(9_998));
        assert!(!s.take(9_998), "the same");
        assert!(!s.take(9_990), "older");
        assert!(s.take(3), "after the roll-over");
        assert_eq!(s.last, Some(3));
    }

    fn host(position: TileLoc, path: &[TileLoc], seq: u32) -> ActorState {
        ActorState { net_id: NetId(0), position, path: path.to_vec(), seq }
    }

    #[test]
    fn the_local_pc_is_corrected_only_when_the_host_stops_it_elsewhere() {
        let mut a = actor();
        a.walk([t(11, 10), t(12, 10), t(13, 10)]);
        a.advance(a.gait.step_seconds(t(10, 10), t(11, 10)) * 1.5);
        assert_eq!(a.at, t(11, 10));
        // The host is behind on the same route: fine.
        assert!(!a.sync_local(&host(t(10, 10), &[t(11, 10), t(12, 10), t(13, 10)], 1)));
        assert_eq!(a.at, t(11, 10));
        // The host stopped it at (11, 10): a correction.
        assert!(a.sync_local(&host(t(11, 10), &[], 1)));
        assert_eq!((a.at, a.walking()), (t(11, 10), false));
    }

    #[test]
    fn other_pcs_follow_the_host_within_a_step() {
        let mut a = actor();
        a.walk([t(11, 10), t(12, 10), t(13, 10)]);
        // A step behind or ahead of the host on the same route: left alone.
        assert!(!a.follow(&host(t(11, 10), &[t(12, 10), t(13, 10)], 1)));
        assert_eq!(a.steps.len(), 3);
        a.advance(a.gait.step_seconds(t(10, 10), t(11, 10)) * 1.01);
        assert!(!a.follow(&host(t(10, 10), &[t(11, 10), t(12, 10), t(13, 10)], 1)));
        // The host turned: take its route from where the actor is stepping to.
        assert!(!a.follow(&host(t(12, 10), &[t(12, 11)], 2)));
        assert_eq!(a.destination(), t(12, 11));
        assert_eq!(a.at, t(11, 10), "finishes the step");
        // Far off: slides to the host's tile.
        assert!(a.follow(&host(t(20, 20), &[], 3)));
        assert_eq!((a.at, a.walking()), (t(20, 20), false));
        assert_ne!(a.slide, Vec2::ZERO);
    }

    struct Open(u32);

    impl Walkable for Open {
        fn walkable(&self, at: TileLoc) -> bool {
            at.x < self.0 && at.y < self.0
        }
    }

    #[test]
    fn a_door_is_approached_to_within_range() {
        let path = path_near(&Open(30), t(0, 0), t(10, 0)).unwrap();
        assert_eq!(path.last(), Some(&t(8, 0)), "stops two tiles short: {path:?}");
        assert_eq!(path_near(&Open(30), t(9, 1), t(10, 0)).unwrap().len(), 0, "already in range: {:?}", path_near(&Open(30), t(9, 1), t(10, 0)));
        // A door tile that cannot be stood on is still approached.
        struct Blocked;
        impl Walkable for Blocked {
            fn walkable(&self, at: TileLoc) -> bool {
                at.x < 30 && at.y < 30 && at != TileLoc { x: 10, y: 0 }
            }
        }
        let path = path_near(&Blocked, t(0, 0), t(10, 0)).unwrap();
        assert!(within(*path.last().unwrap(), t(10, 0), USE_RANGE), "{path:?}");
    }

    #[test]
    fn the_client_clock_runs_on_at_game_speed_between_syncs() {
        let mut clock = WorldClock::default();
        clock.advance(1.0);
        assert_eq!(clock.now, None, "nothing before the first sync");
        clock.set(GameTime::at_hour(15));
        for _ in 0..60 {
            clock.advance(1.0 / 60.0);
        }
        let ms = clock.now.unwrap().ms - 15 * arcanum_sim::HOUR_MS;
        assert!((7990..=8000).contains(&ms), "{ms}");
    }

    #[test]
    fn pcs_take_the_party_pace_and_npcs_keep_their_own() {
        let leader = Gait::human(25);
        let mut app = App::new();
        app.insert_resource(Party { leader: Some(NetId(0)), pace: Some(leader), known: HashMap::new() });
        let slow = Gait { frame_millis: 58, pixels_per_frame: Gait::HUMAN_PIXELS.map(|p| p * 0.6) };
        let pc = app.world_mut().spawn(Actor { gait: slow, own_gait: slow, ..actor() }).id();
        let npc = app.world_mut().spawn(Actor { gait: slow, own_gait: slow, pc: false, ..actor() }).id();
        app.add_systems(Update, pace_party);
        app.update();
        let step = |e| app.world().get::<Actor>(e).unwrap().gait.step_seconds(t(10, 10), t(9, 10));
        assert!((step(pc) - leader.step_seconds(t(10, 10), t(9, 10))).abs() < 1e-9, "the PC keeps the leader's pace");
        assert_eq!(step(npc), slow.step_seconds(t(10, 10), t(9, 10)), "an NPC is not in the party");
    }
}
