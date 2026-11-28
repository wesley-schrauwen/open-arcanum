//! Draws the world: the ground tiles and map objects of the sectors around the local PC (critters
//! are in [`actors`](crate::actors)).
//!
//! The map's unchanged content comes from the client's own copy of the game data: the host
//! names the map ([`LocalMap`]) and places the PC ([`LocalPc`]), and the client reads the
//! sectors around the PC itself into a [`LoadedMap`] (the same reader the host uses). Each
//! ground tile becomes a sprite centred on its [`isometric`](crate::isometric) position in the
//! original's fixed 78 x 40 rectangle (the original ignores a tile art's hotspot). Walls,
//! portals and scenery are sprites anchored on their frame's hotspot at the tile centre plus
//! their offset fields, in the original's draw order ([`object_depth`]).
//!
//! World units are the original's screen pixels; on entering the world the camera zooms so the
//! window shows as many of them vertically as the original's 600-pixel screen.

use std::collections::{HashMap, HashSet};

use arcanum_formats::art_id::{ArtId, ArtIndex};
use arcanum_formats::map::{SECTOR_TILES as SECTOR_SIZE, SectorId, TileLoc};
use arcanum_formats::object::{TYPE_CONTAINER, TYPE_GENERIC, TYPE_PORTAL, TYPE_SCENERY, TYPE_WALL, TYPE_WEAPON};
use arcanum_sim::{GroundItemId, KEEP_RADIUS, LedgerDelta, LoadedMap, MapObjectId, object_location, open_frames, portal_frame, sector_within, with_frame};
use bevy::image::{ImageFilterMode, ImageSampler};
use bevy::prelude::*;

use crate::art_loader::{ArtFrames, hotspot_anchor};
use crate::isometric::{ORIGINAL_SCREEN_HEIGHT, TILE_SIZE, tile_to_world};
use crate::main_menu::{AppState, MenuBackground};
use crate::settings::BASELINE_HEIGHT;
use crate::sim_bridge::{LocalMap, LocalPc};
use crate::vfs_asset::VfsResource;

pub struct WorldRenderPlugin;

impl Plugin for WorldRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InGame), enter_world)
            .add_systems(OnExit(AppState::InGame), leave_world)
            .init_resource::<MapDeltas>()
            .add_systems(
                Update,
                (load_sectors, rebase_depth, apply_map_deltas, drop_far_sectors, draw_sectors, animate_doors, show_tiles, follow_pc).chain().run_if(in_state(AppState::InGame)),
            );
    }
}

/// The client's copy of the map around the local PC, and what of it is drawn.
#[derive(Resource, Default)]
pub struct ClientMap {
    pub map: LoadedMap,
    /// The tile at world position (0, 0): the corner of the PC's first sector. Map positions
    /// run to millions of pixels, too large for `f32` world coordinates to stay exact.
    pub origin: Option<TileLoc>,
    /// The tile [`object_depth`] counts from. It starts at `origin` and moves to the PC when the
    /// PC gets halfway to the edge of [`DEPTH_REACH`] ([`rebase_depth`]).
    pub depth_origin: Option<TileLoc>,
    /// `None` until loaded; `Err` once it failed (reported once).
    art: Option<Result<ArtIndex, String>>,
    /// Sectors whose tiles are spawned.
    pub drawn: HashSet<SectorId>,
    paths: HashMap<u32, Option<String>>,
    frames: HashMap<String, Handle<ArtFrames>>,
    /// Tiles and objects whose art id has no path (they are left out).
    pub unresolved: usize,
    /// Objects with no `current_aid`, their own or their prototype's: left out.
    pub no_art: usize,
}

impl ClientMap {
    /// The art index, once read.
    pub fn art_index(&self) -> Option<&ArtIndex> {
        self.art.as_ref()?.as_ref().ok()
    }
}

/// A ground tile.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroundTile(pub TileLoc);

/// A wall, portal, scenery, container or item object from the map.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct MapObject {
    pub location: TileLoc,
    pub object_type: u32,
    /// Its place in its sector's list; `None` for the `.mob` objects.
    pub id: Option<MapObjectId>,
    /// For an item on the ground, which one (to pick it up).
    pub ground: Option<GroundItemId>,
    /// The art id shown, its `.art`, and how its hotspot is placed.
    pub art: ArtId,
    pub frames: Handle<ArtFrames>,
    /// Drawn in the flat class, and its place among its tile's objects: its depth's parts.
    pub flat: bool,
    pub tile_order: usize,
}

/// Changes to the map's objects from the host, waiting for the world to apply them.
#[derive(Resource, Debug, Default)]
pub struct MapDeltas(pub Vec<LedgerDelta>);

impl MapDeltas {
    /// Whether this module handles `delta`.
    pub fn wants(delta: &LedgerDelta) -> bool {
        matches!(delta, LedgerDelta::MapChanges { .. } | LedgerDelta::ObjectFlags { .. } | LedgerDelta::ObjectArt { .. } | LedgerDelta::ObjectTransformed { .. } | LedgerDelta::PortalChanged { .. } | LedgerDelta::GroundItemDropped { .. } | LedgerDelta::GroundItemTaken { .. } | LedgerDelta::PileChanged { .. } | LedgerDelta::ContainerShown { .. } | LedgerDelta::LockChanged { .. })
    }
}

/// A door playing its frames to `frames`' end, one each `period` seconds.
#[derive(Component, Debug)]
pub struct DoorAnimation {
    pub frames: Vec<u32>,
    pub period: f32,
    pub elapsed: f32,
}

/// A sprite waiting for its `.art` to load: frame `frame` of rotation `rotation` of `art`.
/// Ground tiles are centred on their position (`hotspot: None`); objects put their frame's
/// hotspot there, adjusted by [`ObjectArt`].
#[derive(Component)]
struct PendingArt {
    art: Handle<ArtFrames>,
    rotation: usize,
    frame: usize,
    hotspot: Option<ObjectArt>,
}

/// How an object's art id changes where its frame's hotspot is (0x5031c0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectArt {
    /// Walls and portals in rotations 0, 1, 6 and 7: the hotspot moves 40 left and 20 down.
    pub half_tile: bool,
    /// Walls and portals with id bit 0: mirrored left to right, hotspot x = width - x - 2.
    pub mirrored: bool,
}

impl ObjectArt {
    pub fn of(id: ArtId) -> Self {
        let wall_or_portal = matches!(id.art_type(), 1 | 3);
        let rotation = id.rotation();
        Self { half_tile: wall_or_portal && !(2..=5).contains(&rotation), mirrored: wall_or_portal && id.0 & 1 == 1 }
    }

    /// The hotspot to anchor on, from the frame's stored one.
    pub fn hotspot(self, mut center: IVec2, size: UVec2) -> IVec2 {
        if self.half_tile {
            center += IVec2::new(-40, 20);
        }
        if self.mirrored {
            center.x = size.x as i32 - center.x - 2;
        }
        center
    }
}

/// The frame an object's art id shows (0x502b50): walls and items always 0 (an item's bits
/// 14..16 are part of its key), portals, scenery and containers bits 14..18.
pub fn object_frame(id: ArtId) -> usize {
    if matches!(id.art_type(), 1 | 6) { 0 } else { ((id.0 >> 14) & 0x1f) as usize }
}

/// The rotation an object's art id shows: bits 11..13, except items, which have one (0x502880).
pub fn object_rotation(id: ArtId) -> usize {
    if id.art_type() == 6 { 0 } else { id.rotation() as usize }
}

/// Object flags the original's object pass skips (0x43b390): 0x1 and 0x2 (the default mask at
/// 0x43a4f0, likely off and don't-draw) and 0x100 (mask at 0x43a4fa).
pub const HIDDEN_FLAGS: i32 = 0x1 | 0x2 | 0x100;

/// Object flag 0x4: drawn in the earlier ("flat") class, under every other object.
pub const FLAT_FLAG: i32 = 0x4;

/// The object types drawn.
const DRAWN_OBJECTS: [u32; 3] = [TYPE_WALL, TYPE_PORTAL, TYPE_SCENERY];

/// Whether this client draws objects of `object_type` from its own copy of the map: walls,
/// portals and scenery from the sectors, and containers and items lying on the ground from the
/// `.mob` files. NPCs come from the host.
fn drawn_here(object_type: u32) -> bool {
    DRAWN_OBJECTS.contains(&object_type) || object_type == TYPE_CONTAINER || (TYPE_WEAPON..=TYPE_GENERIC).contains(&object_type)
}

fn enter_world(
    mut commands: Commands,
    mut cameras: Query<&mut Projection, With<Camera2d>>,
    mut background: Query<&mut Visibility, With<MenuBackground>>,
) {
    commands.insert_resource(ClientMap::default());
    for mut projection in &mut cameras {
        if let Projection::Orthographic(o) = &mut *projection {
            o.scale = ORIGINAL_SCREEN_HEIGHT / BASELINE_HEIGHT;
            o.near = -DEPTH_RANGE;
            o.far = DEPTH_RANGE;
        }
    }
    for mut visibility in &mut background {
        *visibility = Visibility::Hidden;
    }
}

fn leave_world(
    mut commands: Commands,
    mut cameras: Query<(&mut Projection, &mut Transform), With<Camera2d>>,
    mut background: Query<&mut Visibility, With<MenuBackground>>,
) {
    commands.remove_resource::<ClientMap>();
    for (mut projection, mut transform) in &mut cameras {
        if let Projection::Orthographic(o) = &mut *projection {
            let menu = OrthographicProjection::default_2d();
            (o.scale, o.near, o.far) = (1.0, menu.near, menu.far);
        }
        transform.translation = Vec3::ZERO;
    }
    for mut visibility in &mut background {
        *visibility = Visibility::Inherited;
    }
}

/// A map tile or object sprite.
type MapSprite = Or<(With<GroundTile>, With<MapObject>)>;

/// Opens the host's map and reads the sectors around the PC from the client's game data. A new
/// map (a teleport, [`arcanum_sim::big_move`]) clears the old one's tiles and objects.
fn load_sectors(mut commands: Commands, mut client: ResMut<ClientMap>, map: Res<LocalMap>, pc: Res<LocalPc>, vfs: Option<Res<VfsResource>>, sprites: Query<Entity, MapSprite>) {
    let (Some(name), Some((_, at))) = (&map.0, pc.0) else { return };
    let Some(vfs) = vfs else { return };
    let Ok(vfs) = vfs.0.read() else { return };
    if client.map.name.as_deref() != Some(name.as_str()) {
        if client.map.name.is_some() {
            for sprite in &sprites {
                commands.entity(sprite).despawn();
            }
        }
        client.map.open(Some(&vfs), name);
        client.drawn.clear();
        let corner = at.sector();
        client.origin = Some(TileLoc { x: corner.x() * SECTOR_SIZE, y: corner.y() * SECTOR_SIZE });
        client.depth_origin = client.origin;
    }
    if client.art.is_none() {
        let art = ArtIndex::load(|p| vfs.read(p).ok()).map_err(|e| e.to_string());
        if let Err(e) = &art {
            warn!("no art index, so the world cannot be drawn: {e}");
        }
        client.art = Some(art);
    }
    let before = (client.map.sectors.len(), client.map.errors.len());
    client.map.load_around(Some(&vfs), at);
    for e in &client.map.errors[before.1..] {
        warn!("map: {e}");
    }
}

/// Unloads the sectors the local PC is not within [`KEEP_RADIUS`] of, and despawns their tile
/// and object sprites.
fn drop_far_sectors(
    mut commands: Commands,
    mut client: ResMut<ClientMap>,
    pc: Res<LocalPc>,
    sprites: Query<(Entity, Option<&GroundTile>, Option<&MapObject>)>,
) {
    let Some((_, at)) = pc.0 else { return };
    let centre = at.sector();
    if client.map.sectors.keys().all(|&id| sector_within(id, centre, KEEP_RADIUS)) {
        return;
    }
    let gone: HashSet<SectorId> = client.map.unload_unless(|id| sector_within(id, centre, KEEP_RADIUS)).into_iter().collect();
    for id in &gone {
        client.drawn.remove(id);
    }
    for (entity, tile, object) in &sprites {
        let loc = tile.map(|t| t.0).or(object.map(|o| o.location));
        if loc.is_some_and(|l| gone.contains(&l.sector())) {
            commands.entity(entity).despawn();
        }
    }
}

/// Applies the host's changes to the client's copy of the map (once it is open), and starts the
/// sprites of doors that changed playing their frames.
fn apply_map_deltas(
    mut commands: Commands,
    mut deltas: ResMut<MapDeltas>,
    mut client: ResMut<ClientMap>,
    frames: Res<Assets<ArtFrames>>,
    mut objects: Query<(Entity, &mut MapObject)>,
    tiles: Query<(Entity, &GroundTile)>,
    assets: Res<AssetServer>,
) {
    if deltas.0.is_empty() || client.map.name.is_none() {
        return;
    }
    for delta in std::mem::take(&mut deltas.0) {
        match delta {
            LedgerDelta::MapChanges { changes } => {
                client.map.changes = changes;
                let loaded: Vec<_> = client.map.sectors.keys().copied().collect();
                for id in loaded {
                    client.map.apply_changes(id);
                }
                // Items taken off drawn sectors go; dropped ones there appear.
                for (entity, object) in &objects {
                    if let Some(item) = object.ground
                        && client.map.ground_item(item).is_none()
                        && client.map.pile(item).is_none()
                    {
                        commands.entity(entity).despawn();
                    }
                }
                let drawn: Vec<(GroundItemId, arcanum_sim::GroundItem)> = client
                    .map
                    .changes
                    .iter()
                    .filter(|(id, _)| client.drawn.contains(id))
                    .flat_map(|(&sector, delta)| {
                        let dropped = delta.dropped.iter().map(move |(&number, item)| (GroundItemId::Dropped { sector, number }, item.clone()));
                        let piles = delta.piles.iter().map(move |(&number, p)| (GroundItemId::Pile { sector, number }, pile_look(p)));
                        dropped.chain(piles)
                    })
                    .filter(|(id, _)| !objects.iter().any(|(_, o)| o.ground == Some(*id)))
                    .collect();
                for (id, item) in drawn {
                    spawn_dropped(&mut commands, &mut client, &assets, id, item, 0);
                }
                // Containers show open or shut.
                for (entity, mut object) in &mut objects {
                    let Some(container @ GroundItemId::Container { .. }) = object.ground else { continue };
                    let Some(base) = client.map.container_object(container).and_then(|o| client.map.prototypes.art_id(o)) else { continue };
                    let aid = container_art(&client.map, Some(container), base);
                    if aid != object.art {
                        object.art = aid;
                        commands.entity(entity).insert(PendingArt { art: object.frames.clone(), rotation: object_rotation(aid), frame: object_frame(aid), hotspot: Some(ObjectArt::of(aid)) });
                    }
                }
                // Loaded door sprites jump to their state.
                for (entity, mut object) in &mut objects {
                    let Some(door) = object.id.filter(|d| client.map.changes.contains_key(&d.sector)) else { continue };
                    let Some(aid) = client.map.map_object(door).and_then(|o| client.map.prototypes.art_id(o)) else { continue };
                    if aid != object.art {
                        object.art = aid;
                        commands.entity(entity).insert(PendingArt { art: object.frames.clone(), rotation: object_rotation(aid), frame: object_frame(aid), hotspot: Some(ObjectArt::of(aid)) });
                    }
                }
            }
            LedgerDelta::ContainerShown { container, open } => {
                client.map.set_container_open(container, open);
                for (entity, mut object) in &mut objects {
                    if object.ground == Some(container) {
                        let aid = arcanum_sim::with_frame(object.art, u32::from(open));
                        object.art = aid;
                        commands.entity(entity).insert(PendingArt { art: object.frames.clone(), rotation: object_rotation(aid), frame: object_frame(aid), hotspot: Some(ObjectArt::of(aid)) });
                    }
                }
            }
            LedgerDelta::GroundItemTaken { item } => {
                client.map.take_ground_item(item);
                for (entity, object) in &objects {
                    if object.ground == Some(item) {
                        commands.entity(entity).despawn();
                    }
                }
            }
            LedgerDelta::PileChanged { pile, contents } => {
                let shown = contents.as_ref().map(pile_look);
                let drawn = objects.iter().any(|(_, o)| o.ground == Some(pile));
                client.map.set_pile(pile, contents);
                match shown {
                    None => {
                        for (entity, object) in &objects {
                            if object.ground == Some(pile) {
                                commands.entity(entity).despawn();
                            }
                        }
                    }
                    Some(look) if !drawn && client.drawn.contains(&pile.sector()) => {
                        let on_tile = objects.iter().filter(|(_, o)| o.location == look.location).count();
                        spawn_dropped(&mut commands, &mut client, &assets, pile, look, on_tile);
                    }
                    Some(_) => {}
                }
            }
            LedgerDelta::GroundItemDropped { item, ground } => {
                client.map.drop_ground_item(item, ground.clone());
                if client.drawn.contains(&item.sector()) {
                    let on_tile = objects.iter().filter(|(_, o)| o.location == ground.location).count();
                    spawn_dropped(&mut commands, &mut client, &assets, item, ground, on_tile);
                }
            }
            LedgerDelta::LockChanged { target, flags } => {
                client.map.set_lock_flags(target, flags);
            }
            // An object switched off or on, given new art or made another object: the sector is
            // drawn again.
            LedgerDelta::ObjectFlags { sector, .. } | LedgerDelta::ObjectArt { sector, .. } | LedgerDelta::ObjectTransformed { sector, .. } => {
                match delta {
                    LedgerDelta::ObjectFlags { index, mobile, flags, .. } => {
                        client.map.set_object_flags(sector, index, mobile, flags);
                    }
                    LedgerDelta::ObjectArt { index, mobile, art, .. } => client.map.set_object_art(sector, index, mobile, art),
                    LedgerDelta::ObjectTransformed { index, mobile, prototype, .. } => {
                        client.map.transform_object(sector, index, mobile, prototype);
                    }
                    _ => {}
                }
                if client.drawn.remove(&sector) {
                    for (entity, object) in &objects {
                        if object.location.sector() == sector {
                            commands.entity(entity).despawn();
                        }
                    }
                    for (entity, tile) in &tiles {
                        if tile.0.sector() == sector {
                            commands.entity(entity).despawn();
                        }
                    }
                }
            }
            LedgerDelta::PortalChanged { door, open } => {
                client.map.set_portal(door, open);
                let Some((entity, _)) = objects.iter().find(|(_, o)| o.id == Some(door)) else { continue };
                let Ok((_, object)) = objects.get(entity) else { continue };
                let mut sequence = open_frames(object.art);
                if !open {
                    // Back through the open frames to 0, from the one before the current.
                    let current = portal_frame(object.art);
                    sequence.retain(|&f| f < current);
                    sequence.reverse();
                    sequence.push(0);
                }
                let rate = frames.get(&object.frames).map_or(0, |f| f.frame_rate);
                let period = if rate > 0 { 1.0 / rate as f32 } else { 0.1 };
                commands.entity(entity).insert(DoorAnimation { frames: sequence, period, elapsed: period });
            }
            _ => {}
        }
    }
}

/// A container's art as shown: frame 1 while open, if it turns ([`arcanum_sim::container`]).
fn container_art(map: &arcanum_sim::LoadedMap, ground: Option<GroundItemId>, aid: ArtId) -> ArtId {
    match ground {
        Some(id @ GroundItemId::Container { .. }) if arcanum_sim::opens_to_frame(aid) => arcanum_sim::with_frame(aid, u32::from(map.container_open(id))),
        _ => aid,
    }
}

/// Steps door sprites through their frames.
fn animate_doors(mut commands: Commands, time: Res<Time>, mut doors: Query<(Entity, &mut DoorAnimation, &mut MapObject)>) {
    for (entity, mut animation, mut object) in &mut doors {
        animation.elapsed += time.delta_secs();
        if animation.elapsed < animation.period {
            continue;
        }
        animation.elapsed = 0.0;
        if animation.frames.is_empty() {
            commands.entity(entity).remove::<DoorAnimation>();
            continue;
        }
        let frame = animation.frames.remove(0);
        object.art = with_frame(object.art, frame);
        let aid = object.art;
        commands.entity(entity).insert(PendingArt { art: object.frames.clone(), rotation: object_rotation(aid), frame: frame as usize, hotspot: Some(ObjectArt::of(aid)) });
    }
}

impl ClientMap {
    /// The `.art` file of art id `id`, once the art index is loaded.
    pub fn art_path(&self, id: ArtId) -> Option<String> {
        match &self.art {
            Some(Ok(index)) => index.path(id),
            _ => None,
        }
    }
}

/// Spawns a sprite for every ground tile of each loaded sector not yet drawn.
fn draw_sectors(mut commands: Commands, mut client: ResMut<ClientMap>, assets: Res<AssetServer>) {
    let client = &mut *client;
    let (Some(Ok(art)), Some(origin)) = (&client.art, client.origin) else { return };
    let depth_origin = client.depth_origin.unwrap_or(origin);
    let new: Vec<SectorId> = client.map.sectors.keys().filter(|id| !client.drawn.contains(id)).copied().collect();
    // Dropped items, spawned once the sectors' own objects are.
    let mut later = Vec::new();
    for id in new {
        let sector = &client.map.sectors[&id];
        let (base_x, base_y) = (id.x() * SECTOR_SIZE, id.y() * SECTOR_SIZE);
        for y in 0..SECTOR_SIZE {
            for x in 0..SECTOR_SIZE {
                let tile = sector.tile(x, y);
                let path = client.paths.entry(tile.0).or_insert_with(|| art.path(tile.art_id())).clone();
                let Some(path) = path else {
                    client.unresolved += 1;
                    continue;
                };
                let handle = client.frames.entry(path.clone()).or_insert_with(|| assets.load(path)).clone();
                let loc = TileLoc { x: base_x + x, y: base_y + y };
                let frame = if tile.is_facade() { facade_frame(tile.art_id()) } else { 0 };
                commands.spawn((
                    GroundTile(loc),
                    Sprite { flip_x: tile.is_mirrored(), custom_size: Some(TILE_SIZE), ..default() },
                    Transform::from_translation(tile_to_world(loc, origin).extend(0.0)),
                    Visibility::Hidden,
                    PendingArt { art: handle, rotation: 0, frame, hotspot: None },
                    DespawnOnExit(AppState::InGame),
                ));
            }
        }
        // How many objects of this sector each tile has had so far (the original draws a tile's
        // objects in list order).
        let mut on_tile: HashMap<TileLoc, usize> = HashMap::new();
        let protos = &client.map.prototypes;
        // The sector's own objects, then the map's mobile objects on it (containers and items;
        // the original's order between the two is not traced).
        let own = sector.objects.iter().enumerate().map(|(i, o)| (Some(MapObjectId { sector: id, index: i as u32 }), None, o));
        let mobiles = client.map.mobiles.get(&id).into_iter().flatten().enumerate().map(|(i, o)| (None, Some(GroundItemId::Mobile { sector: id, index: i as u32 }), o));
        // Items can be picked up and containers opened: they carry their id.
        let mobiles = mobiles.map(|(a, b, o)| {
            let ground = match b {
                Some(GroundItemId::Mobile { sector, index }) if o.object_type == arcanum_formats::object::TYPE_CONTAINER => Some(GroundItemId::Container { sector, index }),
                b => b.filter(|_| arcanum_sim::is_item(o.object_type)),
            };
            (a, ground, o)
        });
        for (object_id, ground, object) in own.chain(mobiles) {
            // Items taken off the ground are gone.
            if let Some(g @ GroundItemId::Mobile { .. }) = ground
                && client.map.ground_item(g).is_none()
            {
                continue;
            }
            // Fields an object does not store are its prototype's.
            let int = |name| protos.int(object, name);
            let flags = int("flags").unwrap_or(0);
            if !drawn_here(object.object_type) || flags & HIDDEN_FLAGS != 0 {
                continue;
            }
            let Some(aid) = protos.art_id(object) else {
                client.no_art += 1;
                continue;
            };
            let aid = container_art(&client.map, ground, aid);
            let Some(location) = object_location(object) else { continue };
            let tile_order = {
                let n = on_tile.entry(location).or_default();
                *n += 1;
                *n - 1
            };
            let path = client.paths.entry(aid.0).or_insert_with(|| art.path(aid)).clone();
            let Some(path) = path else {
                client.unresolved += 1;
                continue;
            };
            let handle = client.frames.entry(path.clone()).or_insert_with(|| assets.load(path)).clone();
            let offset = Vec2::new(int("offset_x").unwrap_or(0) as f32, -(int("offset_y").unwrap_or(0) as f32));
            let at = tile_to_world(location, origin) + offset;
            commands.spawn((
                MapObject { location, object_type: object.object_type, id: object_id, ground, art: aid, frames: handle.clone(), flat: flags & FLAT_FLAG != 0, tile_order },
                Sprite::default(),
                Transform::from_translation(at.extend(object_depth(location, depth_origin, flags & FLAT_FLAG != 0, tile_order))),
                Visibility::Hidden,
                PendingArt { art: handle, rotation: object_rotation(aid), frame: object_frame(aid), hotspot: Some(ObjectArt::of(aid)) },
                DespawnOnExit(AppState::InGame),
            ));
        }
        let dropped: Vec<(GroundItemId, arcanum_sim::GroundItem)> = client
            .map
            .changes
            .get(&id)
            .map(|d| {
                let piles = d.piles.iter().map(|(&number, p)| (GroundItemId::Pile { sector: id, number }, pile_look(p)));
                d.dropped.iter().map(|(&number, i)| (GroundItemId::Dropped { sector: id, number }, i.clone())).chain(piles).collect()
            })
            .unwrap_or_default();
        for (ground_id, item) in dropped {
            let order = on_tile.entry(item.location).or_default();
            *order += 1;
            let order = *order - 1;
            later.push((ground_id, item, order));
        }
        client.drawn.insert(id);
    }
    for (id, item, order) in later {
        spawn_dropped(&mut commands, client, &assets, id, item, order);
    }
}

/// What a pile shows on the map: the junk pile's prototype on its tile.
fn pile_look(pile: &arcanum_sim::Pile) -> arcanum_sim::GroundItem {
    arcanum_sim::GroundItem { prototype: arcanum_sim::PILE, quantity: 0, location: pile.location, keys: vec![], written: 0, wear: Default::default() }
}

/// A sprite for a dropped item or a pile: its prototype's ground art on its tile.
fn spawn_dropped(commands: &mut Commands, client: &mut ClientMap, assets: &AssetServer, id: GroundItemId, item: arcanum_sim::GroundItem, tile_order: usize) {
    let (Some(Ok(art)), Some(origin)) = (&client.art, client.origin) else { return };
    let depth_origin = client.depth_origin.unwrap_or(origin);
    let Some(prototype) = client.map.prototypes.get(item.prototype) else { return };
    let object_type = prototype.object_type;
    let flags = prototype.int("flags").unwrap_or(0);
    let Some(aid) = prototype.int("current_aid").map(|v| ArtId(v as u32)) else { return };
    let Some(path) = client.paths.entry(aid.0).or_insert_with(|| art.path(aid)).clone() else { return };
    let handle = client.frames.entry(path.clone()).or_insert_with(|| assets.load(path)).clone();
    let flat = flags & FLAT_FLAG != 0;
    commands.spawn((
        MapObject { location: item.location, object_type, id: None, ground: Some(id), art: aid, frames: handle.clone(), flat, tile_order },
        Sprite { color: crate::inventory_screen::item_tint(&item.wear), ..default() },
        // Dropped items sit on the tile centre (offsets cleared, 0x4415c0).
        Transform::from_translation(tile_to_world(item.location, origin).extend(object_depth(item.location, depth_origin, flat, tile_order))),
        Visibility::Hidden,
        PendingArt { art: handle, rotation: object_rotation(aid), frame: object_frame(aid), hotspot: Some(ObjectArt::of(aid)) },
        DespawnOnExit(AppState::InGame),
    ));
}

/// The frame of a facade's `.art` that a facade tile shows: bits 1..10 (0x504a90).
pub fn facade_frame(id: ArtId) -> usize {
    ((id.0 >> 1) & 0x3ff) as usize
}

/// Screen rows (and places along a row) either side of the origin's that [`object_depth`]
/// keeps in order: 512, about 256 tiles.
pub const DEPTH_REACH: i64 = 512;

/// Objects on one tile that [`object_depth`] keeps in list order; later ones tie with the last.
pub const DEPTH_TILE_ORDER: usize = 8;

/// The camera's depth range in the world: wide enough for every [`object_depth`].
pub const DEPTH_RANGE: f32 = (1 << 24) as f32;

/// Moves [`ClientMap::depth_origin`] to the local PC once the PC is half of [`DEPTH_REACH`]
/// from it (in screen rows or places along a row), and gives every object its depth again, so
/// the order holds however far the PC walks.
fn rebase_depth(mut client: ResMut<ClientMap>, pc: Res<LocalPc>, mut objects: Query<(&MapObject, &mut Transform)>) {
    let (Some((_, at)), Some(depth_origin)) = (pc.0, client.depth_origin) else { return };
    let dx = i64::from(at.x) - i64::from(depth_origin.x);
    let dy = i64::from(at.y) - i64::from(depth_origin.y);
    if (dx + dy).abs() < DEPTH_REACH / 2 && (dy - dx).abs() < DEPTH_REACH / 2 {
        return;
    }
    client.depth_origin = Some(at);
    for (object, mut transform) in &mut objects {
        transform.translation.z = object_depth(object.location, at, object.flat, object.tile_order);
    }
}

/// Draw depth of an object. The original sorts objects into classes (flat ones first, at
/// 200,000,000 + n; the rest at 600,000,000 + n) and, inside a class, keeps the order it scans
/// tiles in, a screen row at a time, and each tile's objects in list order (0x43b390, comparator
/// 0x443600). Walls and the furniture against them often share a tile, so the list order
/// matters. Here the depth is a whole number below 2^24, so `f32` holds it exactly: class
/// (1 bit), the tile's screen row `x + y` (10 bits), its place along the row `y - x` (10 bits),
/// both within [`DEPTH_REACH`] of the origin (further is clamped; the origin follows the PC,
/// [`rebase_depth`]), and the object's place among
/// its tile's objects in the sector's list (3 bits), plus one so objects stay in front of the
/// ground at 0. The row-by-row scan is inferred from the loop, not proven.
pub fn object_depth(at: TileLoc, origin: TileLoc, flat: bool, tile_order: usize) -> f32 {
    let dx = i64::from(at.x) - i64::from(origin.x);
    let dy = i64::from(at.y) - i64::from(origin.y);
    let place = |v: i64| (v.clamp(-DEPTH_REACH, DEPTH_REACH - 1) + DEPTH_REACH) as u32;
    let (row, column) = (place(dx + dy), place(dy - dx));
    let order = tile_order.min(DEPTH_TILE_ORDER - 1) as u32;
    let class = u32::from(!flat);
    (1 + ((class << 23) | (row << 13) | (column << 3) | order)) as f32
}

/// Gives each waiting sprite its frame once the `.art` has loaded; drops the ones whose art
/// failed to load.
fn show_tiles(
    mut commands: Commands,
    frames: Res<Assets<ArtFrames>>,
    mut images: ResMut<Assets<Image>>,
    assets: Res<AssetServer>,
    mut pending: Query<(Entity, &PendingArt, &mut Sprite, &mut Visibility)>,
) {
    for (entity, wait, mut sprite, mut visibility) in &mut pending {
        if let Some(art) = frames.get(&wait.art) {
            if let Some((image, center, size)) = art.rotation(wait.rotation).and_then(|r| r.frame(wait.frame)) {
                if let Some(object) = wait.hotspot {
                    sprite.flip_x = object.mirrored;
                    commands.entity(entity).insert(hotspot_anchor(object.hotspot(center, size), size));
                }
                // Nearest sampling: linear filtering blends the colour under transparent pixels
                // into the tile edges, a visible line round every diamond.
                if images.get(image).is_some_and(|i| !matches!(i.sampler, ImageSampler::Descriptor(ref d) if d.mag_filter == ImageFilterMode::Nearest))
                    && let Some(mut i) = images.get_mut(image)
                {
                    i.sampler = ImageSampler::nearest();
                }
                sprite.image = image.clone();
                *visibility = Visibility::Inherited;
                commands.entity(entity).remove::<PendingArt>();
            }
        } else if matches!(assets.get_load_state(wait.art.id()), Some(bevy::asset::LoadState::Failed(_))) {
            commands.entity(entity).despawn();
        }
    }
}

/// Centres the camera on the local PC: its sprite once it has one, else its tile.
fn follow_pc(
    pc: Res<LocalPc>,
    client: Res<ClientMap>,
    actors: Query<(&crate::actors::Actor, &Transform), Without<Camera2d>>,
    mut cameras: Query<&mut Transform, With<Camera2d>>,
) {
    let (Some((id, at)), Some(origin)) = (pc.0, client.origin) else { return };
    let target = actors.iter().find(|(a, _)| a.net_id == id).map_or_else(|| tile_to_world(at, origin), |(_, t)| t.translation.truncate());
    for mut transform in &mut cameras {
        transform.translation.x = target.x;
        transform.translation.y = target.y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(x: u32, y: u32) -> TileLoc {
        TileLoc { x, y }
    }

    #[test]
    fn objects_lower_on_screen_draw_later_and_flat_ones_first() {
        let o = t(1000, 1000);
        let d = |x, y, flat| object_depth(t(x, y), o, flat, 0);
        assert!(d(1000, 1000, false) < d(1001, 1000, false), "next row down");
        assert!(d(1000, 1000, false) < d(1000, 1001, false), "next row down");
        assert!(d(1001, 1000, false) < d(1000, 1001, false), "same row, further right");
        assert!(d(1300, 1300, true) < d(700, 700, false), "flat under everything");
        assert!(d(700, 700, true) > 0.0, "every object over the ground");
        assert!(d(1000, 1000, false) < DEPTH_RANGE);
        // Neighbouring rows stay apart in f32.
        assert_ne!(d(1250, 1250, false), d(1250, 1251, false));
        // On one tile, list order; the next tile along the row still comes after.
        let on = |order| object_depth(t(1000, 1000), o, false, order);
        assert!(on(0) < on(1) && on(1) < on(7));
        assert_eq!(on(7), on(20), "clamped");
        assert!(on(20) < d(999, 1001, false), "the next place along the row");
    }

    #[test]
    fn wall_and_portal_hotspots_follow_rotation_and_mirroring() {
        let wall = |rot: u32, flip: u32| ArtId((1 << 28) | (rot << 11) | flip);
        let (c, size) = (IVec2::new(10, 30), UVec2::new(80, 50));
        assert_eq!(ObjectArt::of(wall(3, 0)).hotspot(c, size), c);
        assert_eq!(ObjectArt::of(wall(0, 0)).hotspot(c, size), IVec2::new(-30, 50), "half a tile");
        assert_eq!(ObjectArt::of(wall(7, 0)).hotspot(c, size), IVec2::new(-30, 50));
        assert_eq!(ObjectArt::of(wall(3, 1)).hotspot(c, size), IVec2::new(68, 30), "mirrored: 80 - 10 - 2");
        let scenery = ArtId((4 << 28) | (7 << 11) | 1);
        assert_eq!(ObjectArt::of(scenery), ObjectArt { half_tile: false, mirrored: false });
    }

    #[test]
    fn items_show_their_one_frame_and_rotation() {
        let staff = ArtId(0x6004_0340 | 5 << 14 | 1 << 11);
        assert_eq!((object_frame(staff), object_rotation(staff)), (0, 0));
        let crate_ = ArtId(0x7180_0040 | 1 << 14);
        assert_eq!(object_frame(crate_), 1);
    }

    #[test]
    fn frames_come_from_the_art_id() {
        assert_eq!(object_frame(ArtId((1 << 28) | (5 << 14))), 0, "walls always 0");
        assert_eq!(object_frame(ArtId((4 << 28) | (5 << 14))), 5);
        assert_eq!(object_frame(ArtId((3 << 28) | (31 << 14) | (7 << 11))), 31);
        assert_eq!(facade_frame(ArtId((0xb << 28) | (2 << 1))), 2);
    }

    #[test]
    fn sectors_the_pc_left_are_dropped_with_their_sprites() {
        use arcanum_formats::sector::Sector;
        use arcanum_formats::test_util::SectorBuilder;
        let sector = Sector::parse(&SectorBuilder::new().build()).unwrap();
        let mut client = ClientMap::default();
        for x in 0..6 {
            client.map.sectors.insert(SectorId::new(x, 1), sector.clone());
            client.drawn.insert(SectorId::new(x, 1));
        }
        let mut app = App::new();
        app.insert_resource(client).insert_resource(LocalPc(Some((arcanum_sim::NetId(0), t(5 * 64 + 1, 70)))));
        let near = app.world_mut().spawn(GroundTile(t(4 * 64, 70))).id();
        let far = app.world_mut().spawn(GroundTile(t(64, 70))).id();
        let far_object = app.world_mut().spawn(MapObject { location: t(2 * 64 + 5, 70), object_type: TYPE_SCENERY, id: None, ground: None, art: ArtId(0), frames: Handle::default(), flat: false, tile_order: 0 }).id();
        app.add_systems(Update, drop_far_sectors);
        app.update();
        let client = app.world().resource::<ClientMap>();
        let mut kept: Vec<u32> = client.map.sectors.keys().map(|id| id.x()).collect();
        kept.sort();
        assert_eq!(kept, vec![3, 4, 5]);
        assert_eq!(client.drawn.len(), 3);
        assert!(app.world().get_entity(near).is_ok());
        assert!(app.world().get_entity(far).is_err());
        assert!(app.world().get_entity(far_object).is_err());
    }

    #[test]
    fn the_depth_origin_follows_a_pc_that_walks_far() {
        let mut client = ClientMap::default();
        client.origin = Some(t(1000, 1000));
        client.depth_origin = client.origin;
        let mut app = App::new();
        app.insert_resource(client).insert_resource(LocalPc(Some((arcanum_sim::NetId(0), t(1100, 1100)))));
        let object = |at: TileLoc| MapObject { location: at, object_type: TYPE_SCENERY, id: None, ground: None, art: ArtId(0), frames: Handle::default(), flat: false, tile_order: 0 };
        // Two objects near where the PC will be, in one column, one in front of the other.
        let behind = app.world_mut().spawn((object(t(1300, 1300)), Transform::default())).id();
        let front = app.world_mut().spawn((object(t(1301, 1301)), Transform::default())).id();
        app.add_systems(Update, rebase_depth);
        app.update();
        assert_eq!(app.world().resource::<ClientMap>().depth_origin, Some(t(1000, 1000)), "200 rows away: not yet");
        app.world_mut().resource_mut::<LocalPc>().0 = Some((arcanum_sim::NetId(0), t(1300, 1300)));
        app.update();
        assert_eq!(app.world().resource::<ClientMap>().depth_origin, Some(t(1300, 1300)));
        let z = |e| app.world().get::<Transform>(e).unwrap().translation.z;
        assert!(z(behind) < z(front), "in order again: {} {}", z(behind), z(front));
        // From the old origin both would have been clamped to the same row.
        let o = t(1000, 1000);
        assert_eq!(object_depth(t(1300, 1300), o, false, 0), object_depth(t(1301, 1301), o, false, 0));
    }
}
