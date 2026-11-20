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
use arcanum_sim::{LoadedMap, object_location};
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
            .add_systems(Update, (load_sectors, draw_sectors, show_tiles, follow_pc).chain().run_if(in_state(AppState::InGame)));
    }
}

/// The client's copy of the map around the local PC, and what of it is drawn.
#[derive(Resource, Default)]
pub struct ClientMap {
    pub map: LoadedMap,
    /// The tile at world position (0, 0): the corner of the PC's first sector. Map positions
    /// run to millions of pixels, too large for `f32` world coordinates to stay exact.
    pub origin: Option<TileLoc>,
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

/// A ground tile.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroundTile(pub TileLoc);

/// A wall, portal or scenery object from a sector.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapObject {
    pub location: TileLoc,
    pub object_type: u32,
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

/// Opens the host's map and reads the sectors around the PC from the client's game data.
fn load_sectors(mut client: ResMut<ClientMap>, map: Res<LocalMap>, pc: Res<LocalPc>, vfs: Option<Res<VfsResource>>) {
    let (Some(name), Some((_, at))) = (&map.0, pc.0) else { return };
    let Some(vfs) = vfs else { return };
    let Ok(vfs) = vfs.0.read() else { return };
    if client.map.name.as_deref() != Some(name.as_str()) {
        client.map.open(Some(&vfs), name);
        client.drawn.clear();
        let corner = at.sector();
        client.origin = Some(TileLoc { x: corner.x() * SECTOR_SIZE, y: corner.y() * SECTOR_SIZE });
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
    let new: Vec<SectorId> = client.map.sectors.keys().filter(|id| !client.drawn.contains(id)).copied().collect();
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
        for object in sector.objects.iter().chain(client.map.mobiles.get(&id).into_iter().flatten()) {
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
                MapObject { location, object_type: object.object_type },
                Sprite::default(),
                Transform::from_translation(at.extend(object_depth(location, origin, flags & FLAT_FLAG != 0, tile_order))),
                Visibility::Hidden,
                PendingArt { art: handle, rotation: object_rotation(aid), frame: object_frame(aid), hotspot: Some(ObjectArt::of(aid)) },
                DespawnOnExit(AppState::InGame),
            ));
        }
        client.drawn.insert(id);
    }
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

/// Draw depth of an object. The original sorts objects into classes (flat ones first, at
/// 200,000,000 + n; the rest at 600,000,000 + n) and, inside a class, keeps the order it scans
/// tiles in, a screen row at a time, and each tile's objects in list order (0x43b390, comparator
/// 0x443600). Walls and the furniture against them often share a tile, so the list order
/// matters. Here the depth is a whole number below 2^24, so `f32` holds it exactly: class
/// (1 bit), the tile's screen row `x + y` (10 bits), its place along the row `y - x` (10 bits),
/// both within [`DEPTH_REACH`] of the origin (further is clamped), and the object's place among
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
}
