//! The in-game inventory screen (Arcanum.exe 0x572240, mode 0: the PC's own inventory; render
//! 0x576520). Positions below are in the 800x400 window between the bars (screen y = y + 41).
//!
//! - **Art:** `PDoll` (art 223) at (0, 0), `Inventor` (221) at (358, 0); `TotalAttack` (676) at
//!   (29, 318), `TotalDefens` (677) at (254, 319), the arrange button (`tiny_butt`, 340) at
//!   (763, 197), the use box (`Use_Box_OFF`, 342) at (703, 290) and the drop box (`Drop_OFF`,
//!   224) at (703, 344).
//! - **Grid:** 10 x 12 cells of 32 pixels from (368, 8); an item at location `row * 10 + col`
//!   is drawn at its natural size, centred in its footprint (`(box - art) / 2`, truncated).
//! - **Paperdoll** (table 0x5cac58): helmet (151, 107, 64, 64), rings (247, 107) and (279, 107)
//!   32x32, amulet (247, 139, 64, 32), weapon (23, 170, 96, 128), shield (247, 171, 96, 128),
//!   armour (119, 171, 128, 160), gauntlets (55, 107, 64, 64), boots (150, 331, 64, 64). An
//!   empty slot shows its cover (0x5caf58: `CVR_*`); a worn item its paperdoll art (weapons
//!   `weapon_paper_doll_aid`, armour `armor_paper_doll_aid`, others the inventory art), centred.
//! - **Text:** "Total weight: %d stone", "Encumbrance: %s" (`mes/critter.mes` 18 + level) and
//!   "Speed: %d" at (145, 29 / 44 / 59, 185, 15) (`mes/inven_ui.mes` 9, 6, 10, 11); gold and the
//!   four ammo counts at (729, 70 + 23 n, 58, 20).
//! - **Moving** (callback 0x573850): press picks the item under the cursor up and release drops
//!   it; while held it is not drawn in place, and its inventory art follows the cursor. On the
//!   grid it lands with the grabbed cell under the cursor (cursor cell minus the grab, linear as
//!   the original's); on a slot it is worn if the fit test passes. Off the grid and slots it goes
//!   back. Each move goes to the host ([`PcInventory`]); refusals with a message (`mes/item.mes`)
//!   go to the message box. The original's cursor hotspot for the held art was not decoded: here
//!   the point it was grabbed by stays under the cursor.
//! - Toggled by the top bar's inventory button and `I`; Escape closes it; opening it closes the
//!   character screen and the other way round. Clicks on it do not walk.
//!
//! Not built: using, dropping on the ground or the boxes, splitting stacks, the arrange button, total attack and
//! defence, item hover help, barter and loot modes. The original's fonts for the text and
//! numbers were not identified (Flare12Font stands in). The original's encumbrance line also
//! prints a stray number from its stack (`%s: %s (%d)` given two values); it is left out.

use arcanum_formats::art_id::{ArtId, ArtIndex};
use arcanum_formats::object::{Prototypes, TYPE_ARMOR, TYPE_WEAPON};
use arcanum_sim::{AMMO_TYPES, CarriedItem, GOLD, Inventory, InventoryRequest, ItemInfo, ItemRefusal, PlayerIntent, Slot, Wearer, in_grid};
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use crate::art_loader::ArtFrames;
use crate::create_character::CreationData;
use crate::fonts::{ArcanumFonts, UiFont};
use crate::main_menu::AppState;
use crate::menu_window::{SCREEN_WHITE, TOP_BAR_HEIGHT, at, px, rect};
use crate::resource_pack::paths::interface;

/// The grid's top-left cell, in the band.
pub const GRID: Vec2 = Vec2::new(368.0, 8.0);
/// The paperdoll slots' rects (in the band), by slot.
pub const SLOTS: [(Slot, [f32; 4], &str); 9] = [
    (Slot::Helmet, [151.0, 107.0, 64.0, 64.0], "CVR_Helmet.ART"),
    (Slot::Ring1, [247.0, 107.0, 32.0, 32.0], "CVR_Ring1.ART"),
    (Slot::Ring2, [279.0, 107.0, 32.0, 32.0], "CVR_Ring2.ART"),
    (Slot::Amulet, [247.0, 139.0, 64.0, 32.0], "CVR_Medalion.ART"),
    (Slot::Weapon, [23.0, 170.0, 96.0, 128.0], "CVR_Weapon.ART"),
    (Slot::Shield, [247.0, 171.0, 96.0, 128.0], "CVR_Shield.ART"),
    (Slot::Armor, [119.0, 171.0, 128.0, 160.0], "CVR_Armor.ART"),
    (Slot::Gauntlets, [55.0, 107.0, 64.0, 64.0], "CVR_Gauntlet.ART"),
    (Slot::Boots, [150.0, 331.0, 64.0, 64.0], "CVR_Boot.ART"),
];
/// The weight, encumbrance and speed lines.
const TEXT_LINES: [f32; 3] = [29.0, 44.0, 59.0];
/// Gold, then the ammo counts.
const COUNTS_X: f32 = 729.0;
const COUNTS_Y: [f32; 5] = [70.0, 93.0, 116.0, 139.0, 162.0];

/// Whether the inventory screen is open.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InventoryPanel {
    pub open: bool,
}

/// The client's copy of its own PC's inventory. As with the sheet ([`crate::pc_sheet`]): a move
/// is applied at once to a preview with the host's rules and sent as a numbered request; the
/// host answers each with its copy, and the preview becomes that copy with the unanswered
/// requests replayed on top.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct PcInventory {
    /// The host's copy, once it has sent one (with the PC's spawn).
    confirmed: Option<Inventory>,
    /// What the screen shows.
    preview: Option<Inventory>,
    pending: Vec<(u32, InventoryRequest)>,
    last_seq: u32,
    outgoing: Vec<PlayerIntent>,
    /// Host answers not yet settled (they need the item facts to replay on).
    incoming: Vec<(u32, Inventory)>,
}

impl PcInventory {
    /// What the screen shows, once the host has sent it.
    pub fn shown(&self) -> Option<&Inventory> {
        self.preview.as_ref()
    }

    /// Applies `request` to the preview and queues it for the host. A refused one changes
    /// nothing and is not sent.
    pub fn request(&mut self, request: InventoryRequest, wearer: Wearer, info: impl Fn(u32) -> Option<ItemInfo>) -> Result<(), ItemRefusal> {
        let Some(preview) = &self.preview else { return Err(ItemRefusal::Silent) };
        self.preview = Some(preview.apply(&request, wearer, info)?);
        self.last_seq += 1;
        self.pending.push((self.last_seq, request.clone()));
        self.outgoing.push(PlayerIntent::Inventory { seq: self.last_seq, request });
        Ok(())
    }

    /// Requests for the host, oldest first.
    pub fn take_outgoing(&mut self) -> Vec<PlayerIntent> {
        std::mem::take(&mut self.outgoing)
    }

    /// Keeps the host's answer for [`Self::settle`].
    pub fn receive(&mut self, ack: u32, inventory: Inventory) {
        self.incoming.push((ack, inventory));
    }

    /// Takes the host's answers: the preview becomes the newest one with the unanswered
    /// requests replayed on top.
    pub fn settle(&mut self, wearer: Wearer, info: impl Fn(u32) -> Option<ItemInfo>) {
        let newest = self.incoming.pop();
        self.incoming.clear();
        let Some((ack, inventory)) = newest else { return };
        self.pending.retain(|(seq, _)| *seq > ack);
        let mut preview = inventory.clone();
        for (_, request) in &self.pending {
            if let Ok(changed) = preview.apply(request, wearer, &info) {
                preview = changed;
            }
        }
        self.confirmed = Some(inventory);
        self.preview = Some(preview);
    }

    /// Whether host answers wait to be settled.
    pub fn unsettled(&self) -> bool {
        !self.incoming.is_empty()
    }
}

/// The item being dragged: where it was, and the cursor's place on it.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
pub struct HeldItem(pub Option<Held>);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Held {
    /// Its location when picked up.
    pub from: i32,
    /// Cursor cell minus the item's location, as the original keeps it (0x683470); 0 off a slot.
    pub grab: i32,
    /// Where the cursor sat on the item's art, in pixels from its top-left.
    pub offset: Vec2,
}

/// The held item's art, following the cursor.
#[derive(Component)]
pub struct HeldArt;

/// Item art handles by path, kept so the art is not unloaded before it loads.
#[derive(Resource, Debug, Default)]
pub struct ItemArtCache(std::collections::HashMap<String, Handle<ArtFrames>>);

impl ItemArtCache {
    fn get(&mut self, assets: &AssetServer, path: String) -> Handle<ArtFrames> {
        self.0.entry(path.clone()).or_insert_with(|| assets.load(path)).clone()
    }
}

/// The screen's root.
#[derive(Component)]
pub struct InventoryScreen;

/// The items and texts drawn for the inventory (redrawn when it changes).
#[derive(Component)]
pub struct InventoryContent;

/// An item drawn on the screen, for tests.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct DrawnItem(pub CarriedItem);

/// The numbers and lines, for tests.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryText {
    Weight,
    Encumbrance,
    Speed,
    Gold,
    Ammo(usize),
}

pub struct InventoryScreenPlugin;

impl Plugin for InventoryScreenPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InventoryPanel>()
            .init_resource::<PcInventory>()
            .init_resource::<HeldItem>()
            .init_resource::<ItemArtCache>()
            .add_systems(OnExit(AppState::InGame), |mut panel: ResMut<InventoryPanel>, mut inv: ResMut<PcInventory>, mut held: ResMut<HeldItem>| {
                *panel = InventoryPanel::default();
                *inv = PcInventory::default();
                *held = HeldItem::default();
            })
            .add_systems(PreUpdate, toggle_on_keys.run_if(in_state(AppState::InGame)).after(bevy::input::InputSystems))
            .add_systems(Update, (settle_inventory, one_panel_at_a_time, show_panel, drag_items, show_items, follow_cursor).chain().run_if(in_state(AppState::InGame)))
            .add_observer(on_hud_button);
    }
}

fn band(x: f32, y: f32, w: f32, h: f32) -> Rect {
    rect(Vec2::new(x, y + TOP_BAR_HEIGHT), Vec2::new(w, h))
}

fn art(name: &str) -> String {
    format!("{}/{name}", interface::DIR)
}

/// `I` toggles the screen; Escape closes it (before it would leave the game).
fn toggle_on_keys(mut keys: ResMut<ButtonInput<KeyCode>>, mut panel: ResMut<InventoryPanel>) {
    if keys.just_pressed(KeyCode::KeyI) {
        panel.open = !panel.open;
    } else if panel.open && keys.clear_just_pressed(KeyCode::Escape) {
        panel.open = false;
    }
}

fn on_hud_button(activate: On<Activate>, buttons: Query<&crate::hud::HudAction>, mut panel: ResMut<InventoryPanel>) {
    if buttons.get(activate.entity) == Ok(&crate::hud::HudAction::Inventory) {
        panel.open = !panel.open;
    }
}

/// Opening one of the inventory and character screens closes the other.
fn one_panel_at_a_time(mut inventory: ResMut<InventoryPanel>, mut character: ResMut<crate::character_editor::CharacterPanel>) {
    if inventory.is_changed() && inventory.open && character.open {
        character.open = false;
    } else if character.is_changed() && character.open && inventory.open {
        inventory.open = false;
    }
}

fn show_panel(mut commands: Commands, panel: Res<InventoryPanel>, roots: Query<Entity, With<InventoryScreen>>, assets: Res<AssetServer>) {
    if !panel.is_changed() {
        return;
    }
    if !panel.open {
        for root in &roots {
            commands.entity(root).despawn();
        }
        commands.insert_resource(HeldItem::default());
        return;
    }
    if !roots.is_empty() {
        return;
    }
    let image = |name: &str, x: f32, y: f32, w: f32, h: f32| (ImageNode::new(assets.load(art(name))), at(band(x, y, w, h)));
    commands
        .spawn((
            InventoryScreen,
            DespawnOnExit(AppState::InGame),
            bevy::picking::Pickable::IGNORE,
            Node { position_type: PositionType::Absolute, top: Val::Px(0.0), left: Val::Percent(50.0), margin: UiRect::left(px(-400.0)), width: px(800.0), height: px(600.0), ..default() },
        ))
        .with_children(|screen| {
            screen.spawn(image("PDoll.ART", 0.0, 0.0, 358.0, 400.0));
            screen.spawn(image("Inventor.ART", 358.0, 0.0, 442.0, 400.0));
            screen.spawn(image("TotalAttack.ART", 29.0, 318.0, 54.0, 71.0));
            screen.spawn(image("TotalDefens.ART", 254.0, 319.0, 54.0, 71.0));
            screen.spawn(image("tiny_butt.ART", 763.0, 197.0, 28.0, 27.0));
            screen.spawn(image("Use_Box_OFF.ART", 703.0, 290.0, 80.0, 43.0));
            screen.spawn(image("Drop_OFF.ART", 703.0, 344.0, 80.0, 43.0));
        });
}

/// Where the item art for `item` is and where it goes: its location's box in the band.
fn placement(item: &CarriedItem, cells: (i32, i32)) -> Option<Rect> {
    if in_grid(item.location) {
        let (col, row) = ((item.location % 10) as f32, (item.location / 10) as f32);
        let at = GRID + Vec2::new(col, row) * 32.0;
        return Some(Rect::from_corners(at, at + Vec2::new(cells.0 as f32, cells.1 as f32) * 32.0));
    }
    SLOTS.iter().find(|(s, ..)| *s as i32 == item.location).map(|(_, r, _)| Rect::from_corners(Vec2::new(r[0], r[1]), Vec2::new(r[0] + r[2], r[1] + r[3])))
}

/// The top-left corner of art `size` centred in `inside`, as the original rounds it: grid
/// items `(box - art) / 2` truncated towards zero, slots `(box - art) >> 1`.
pub fn centred(inside: Rect, size: UVec2, grid: bool) -> Vec2 {
    let gap = |b: f32, a: u32| {
        let d = b as i32 - a as i32;
        if grid { d / 2 } else { d >> 1 }
    };
    inside.min + Vec2::new(gap(inside.width(), size.x) as f32, gap(inside.height(), size.y) as f32)
}

/// The art an item shows at its location: worn weapons and armour their paperdoll art.
fn item_art(prototypes: &Prototypes, item: &CarriedItem) -> Option<ArtId> {
    let p = prototypes.get(item.prototype)?;
    let field = match p.object_type {
        TYPE_WEAPON if !in_grid(item.location) => "weapon_paper_doll_aid",
        TYPE_ARMOR if !in_grid(item.location) => "armor_paper_doll_aid",
        _ => "item_inv_aid",
    };
    p.int(field).or_else(|| p.int("item_inv_aid")).map(|v| ArtId(v as u32))
}

/// The first frame of art `id`, if loaded (a handle is kept in `cache`).
fn art_frame(index: Option<&ArtIndex>, cache: &mut ItemArtCache, assets: &AssetServer, frames: &Assets<ArtFrames>, id: ArtId) -> Option<(Handle<Image>, UVec2)> {
    let handle = cache.get(assets, index?.path(id)?);
    frames.get(&handle).and_then(|f| f.rotation(0)?.frame(0).map(|(i, _, s)| (i.clone(), s)))
}

/// Item facts for the client's rules, footprints from the loaded inventory art.
fn client_info<'a>(prototypes: &'a Prototypes, sizes: &'a std::collections::HashMap<u32, (u32, u32)>) -> impl Fn(u32) -> Option<ItemInfo> + 'a {
    move |n| prototypes.get(n).map(|p| ItemInfo::of(p, |id| sizes.get(&id.0).copied()))
}

/// The loaded inventory art sizes of everything in `inv`, by art id.
fn inventory_sizes(inv: &Inventory, prototypes: &Prototypes, index: Option<&ArtIndex>, cache: &mut ItemArtCache, assets: &AssetServer, frames: &Assets<ArtFrames>) -> std::collections::HashMap<u32, (u32, u32)> {
    let mut sizes = std::collections::HashMap::new();
    for item in &inv.items {
        if let Some(id) = prototypes.get(item.prototype).and_then(|p| p.int("item_inv_aid"))
            && let Some((_, s)) = art_frame(index, cache, assets, frames, ArtId(id as u32))
        {
            sizes.insert(id as u32, (s.x, s.y));
        }
    }
    sizes
}

/// Who wears the items: the local PC's sheet.
fn wearer(sheet: &crate::pc_sheet::SheetHost) -> Wearer {
    sheet.sheet().map_or(Wearer { race: 0, gender: 1 }, |s| Wearer { race: s.race, gender: s.gender })
}

/// Takes the host's answers once the item facts are known.
fn settle_inventory(mut inventory: ResMut<PcInventory>, sheet: Res<crate::pc_sheet::SheetHost>, client: Option<Res<crate::world_render::ClientMap>>, mut cache: ResMut<ItemArtCache>, assets: Res<AssetServer>, frames: Res<Assets<ArtFrames>>) {
    if !inventory.unsettled() {
        return;
    }
    let empty = Prototypes::default();
    let prototypes = client.as_ref().map_or(&empty, |c| &c.map.prototypes);
    let index = client.as_ref().and_then(|c| c.art_index());
    let newest = inventory.incoming.last().map(|(_, i)| i.clone()).unwrap_or_default();
    let sizes = inventory_sizes(&newest, prototypes, index, &mut cache, &assets, &frames);
    inventory.settle(wearer(&sheet), client_info(prototypes, &sizes));
}

/// The 800x600 point under the window cursor (the screens are centred, scaled to the height).
pub fn screen_point(window: Vec2, cursor: Vec2) -> Vec2 {
    let scale = window.y / 600.0;
    Vec2::new((cursor.x - (window.x / 2.0 - 400.0 * scale)) / scale, cursor.y / scale)
}

/// The grid cell or paperdoll slot at `point` in the band (0x575cb0).
pub fn location_at(point: Vec2) -> Option<i32> {
    let (col, row) = (((point.x - GRID.x) / 32.0).floor(), ((point.y - GRID.y) / 32.0).floor());
    if (0.0..10.0).contains(&col) && (0.0..12.0).contains(&row) {
        return Some(row as i32 * 10 + col as i32);
    }
    SLOTS.iter().find(|(_, r, _)| point.x >= r[0] && point.x < r[0] + r[2] && point.y >= r[1] && point.y < r[1] + r[3]).map(|(s, ..)| *s as i32)
}

/// The item at `point` in the band (0x575fa0): a grid item covering the cell, or the worn one.
pub fn item_at(inv: &Inventory, point: Vec2, info: impl Fn(u32) -> Option<ItemInfo>) -> Option<&CarriedItem> {
    let location = location_at(point)?;
    if !in_grid(location) {
        return inv.items.iter().find(|i| i.location == location);
    }
    let (col, row) = (location % 10, location / 10);
    inv.items.iter().filter(|i| in_grid(i.location)).find(|i| {
        let (w, h) = info(i.prototype).map_or((1, 1), |x| x.cells);
        let (c, r) = (i.location % 10, i.location / 10);
        (c..c + w).contains(&col) && (r..r + h).contains(&row)
    })
}

/// Where a held item lands for a release at `point`: the cursor's cell minus the grab, or the
/// slot (0x573850 release). `None` (off the grid and slots, or before row 0) returns it.
pub fn drop_target(point: Vec2, held: Held) -> Option<i32> {
    let at = location_at(point)?;
    if in_grid(at) {
        let to = at - held.grab;
        (to >= 0).then_some(to)
    } else {
        Some(at)
    }
}

/// Press picks the item under the cursor up; release drops it where the cursor is, through
/// the host. Refusals with a message go to the message box.
#[allow(clippy::too_many_arguments)]
fn drag_items(
    panel: Res<InventoryPanel>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    mut held: ResMut<HeldItem>,
    mut inventory: ResMut<PcInventory>,
    sheet: Res<crate::pc_sheet::SheetHost>,
    client: Option<Res<crate::world_render::ClientMap>>,
    data: Option<Res<CreationData>>,
    (mut cache, assets, frames): (ResMut<ItemArtCache>, Res<AssetServer>, Res<Assets<ArtFrames>>),
    (boxed, time): (Option<ResMut<crate::message_box::MessageBox>>, Option<Res<Time<Real>>>),
) {
    if !panel.open || !(buttons.just_pressed(MouseButton::Left) || buttons.just_released(MouseButton::Left)) {
        return;
    }
    let Some(point) = windows.iter().find_map(|w| w.cursor_position().map(|c| screen_point(w.size(), c))) else { return };
    let point = point - Vec2::new(0.0, TOP_BAR_HEIGHT);
    let Some(inv) = inventory.shown().cloned() else { return };
    let empty = Prototypes::default();
    let prototypes = client.as_ref().map_or(&empty, |c| &c.map.prototypes);
    let index = client.as_ref().and_then(|c| c.art_index());
    let sizes = inventory_sizes(&inv, prototypes, index, &mut cache, &assets, &frames);
    let info = client_info(prototypes, &sizes);
    if buttons.just_pressed(MouseButton::Left) && held.0.is_none() {
        let Some(item) = item_at(&inv, point, &info) else { return };
        let grab = location_at(point).filter(|l| in_grid(*l)).map_or(0, |l| l - item.location);
        let art = item_art(prototypes, item).and_then(|id| art_frame(index, &mut cache, &assets, &frames, id));
        let corner = art.and_then(|(_, s)| Some(centred(placement(item, info(item.prototype).map_or((1, 1), |i| i.cells))?, s, in_grid(item.location)))).unwrap_or(point);
        held.0 = Some(Held { from: item.location, grab, offset: point - corner });
    } else if buttons.just_released(MouseButton::Left)
        && let Some(h) = held.0.take()
        && let Some(to) = drop_target(point, h)
        && to != h.from
        && let Err(refusal) = inventory.request(InventoryRequest::Move { from: h.from, to }, wearer(&sheet), &info)
        && let (Some(id), Some(mut boxed)) = (refusal.message(), boxed)
    {
        let text = data.as_ref().and_then(|d| d.item.get(id)).unwrap_or("").to_string();
        boxed.post(crate::message_box::BoxMessage::plain(text), time.map_or(0.0, |t| t.elapsed_secs_f64()));
    }
}

/// Draws the held item at the cursor, keeping the point it was grabbed by under it.
#[allow(clippy::too_many_arguments)]
fn follow_cursor(
    mut commands: Commands,
    held: Res<HeldItem>,
    inventory: Res<PcInventory>,
    windows: Query<&Window>,
    roots: Query<Entity, With<InventoryScreen>>,
    mut arts: Query<(Entity, &mut Node), With<HeldArt>>,
    client: Option<Res<crate::world_render::ClientMap>>,
    (mut cache, assets, frames): (ResMut<ItemArtCache>, Res<AssetServer>, Res<Assets<ArtFrames>>),
) {
    let item = held.0.and_then(|h| Some((h, inventory.shown()?.items.iter().find(|i| i.location == h.from)?.clone())));
    let (Some((h, item)), Ok(root)) = (item, roots.single()) else {
        for (e, _) in &arts {
            commands.entity(e).despawn();
        }
        return;
    };
    let Some(point) = windows.iter().find_map(|w| w.cursor_position().map(|c| screen_point(w.size(), c))) else { return };
    let corner = point - h.offset;
    if let Ok((_, mut node)) = arts.single_mut() {
        node.left = px(corner.x);
        node.top = px(corner.y);
        return;
    }
    let empty = Prototypes::default();
    let prototypes = client.as_ref().map_or(&empty, |c| &c.map.prototypes);
    let index = client.as_ref().and_then(|c| c.art_index());
    // The original's cursor shows the inventory art (field 93) wherever the item was.
    let grid_item = CarriedItem { location: 0, ..item };
    let Some((image, size)) = item_art(prototypes, &grid_item).and_then(|id| art_frame(index, &mut cache, &assets, &frames, id)) else { return };
    let art = commands.spawn((HeldArt, bevy::picking::Pickable::IGNORE, ImageNode::new(image), at(rect(corner, Vec2::new(size.x as f32, size.y as f32))))).id();
    commands.entity(root).add_child(art);
}

#[allow(clippy::too_many_arguments)]
fn show_items(
    mut commands: Commands,
    inventory: Res<PcInventory>,
    held: Res<HeldItem>,
    sheet: Res<crate::pc_sheet::SheetHost>,
    client: Option<Res<crate::world_render::ClientMap>>,
    data: Option<Res<CreationData>>,
    fonts: Option<Res<ArcanumFonts>>,
    roots: Query<Entity, With<InventoryScreen>>,
    contents: Query<Entity, With<InventoryContent>>,
    added: Query<(), Added<InventoryScreen>>,
    (assets, frames, mut pending, mut cache): (Res<AssetServer>, Res<Assets<ArtFrames>>, Local<bool>, ResMut<ItemArtCache>),
) {
    let Ok(root) = roots.single() else { return };
    if !inventory.is_changed() && !held.is_changed() && added.is_empty() && !*pending {
        return;
    }
    *pending = false;
    for old in &contents {
        commands.entity(old).despawn();
    }
    let Some(inv) = inventory.shown() else { return };
    let empty = Prototypes::default();
    let prototypes = client.as_ref().map_or(&empty, |c| &c.map.prototypes);
    let index = client.as_ref().and_then(|c| c.art_index());
    let fonts = fonts.map(|f| f.clone()).unwrap_or_default();
    let mut content = Vec::new();
    for (slot, r, cover) in SLOTS {
        if inv.worn(slot).is_none() {
            content.push(commands.spawn((InventoryContent, bevy::picking::Pickable::IGNORE, ImageNode::new(assets.load(art(cover))), Node { width: Val::Auto, height: Val::Auto, ..at(band(r[0], r[1], r[2], r[3])) })).id());
        }
    }
    // The held item is not drawn where it was (item flag 0x10, 0x4a50d0).
    let held_at = held.0.map(|h| h.from);
    for item in inv.items.iter().filter(|i| Some(i.location) != held_at) {
        let Some(id) = item_art(prototypes, item) else { continue };
        let Some((image, size)) = art_frame(index, &mut cache, &assets, &frames, id) else {
            // Not loaded yet: draw again next frame.
            *pending = true;
            continue;
        };
        let cells = arcanum_sim::cells_for(size.x, size.y);
        let Some(inside) = placement(item, cells) else { continue };
        let corner = centred(inside, size, in_grid(item.location));
        content.push(commands.spawn((InventoryContent, DrawnItem(item.clone()), bevy::picking::Pickable::IGNORE, ImageNode::new(image), at(band(corner.x, corner.y, size.x as f32, size.y as f32)))).id());
    }
    // The texts.
    let rules = data.as_ref().map(|d| d.rules.clone()).unwrap_or_default();
    let c = sheet.sheet().cloned().unwrap_or_default();
    // Weight needs no art sizes.
    let info = arcanum_sim::item_info(prototypes, |_| None);
    let aptitude = rules.aptitude(&c);
    let carry = rules.derived(&c).carry_weight;
    let total = inv.total_weight(&info, aptitude);
    let level = arcanum_sim::encumbrance(total, carry);
    let label = |id: u32, fallback: &str| data.as_ref().and_then(|d| d.inven_ui.get(id)).unwrap_or(fallback).to_string();
    let level_name = data.as_ref().and_then(|d| d.critter.get(18 + level as u32)).unwrap_or("").to_string();
    let lines = [
        (InventoryText::Weight, format!("{}: {} {}", label(9, "Total weight"), total, label(6, "stone"))),
        (InventoryText::Encumbrance, format!("{}: {}", label(10, "Encumbrance"), level_name)),
        (InventoryText::Speed, format!("{}: {}", label(11, "Speed"), rules.derived(&c).speed)),
    ];
    for ((which, text), y) in lines.into_iter().zip(TEXT_LINES) {
        content.push(text_cell(&mut commands, &fonts, which, &text, band(145.0, y, 185.0, 15.0)));
    }
    let count = |proto: Option<u32>, kind: Option<i32>| -> i32 {
        inv.items.iter().filter(|i| proto.is_none_or(|p| i.prototype == p) && kind.is_none_or(|k| prototypes.get(i.prototype).and_then(|p| p.int("ammo_type")) == Some(k) && prototypes.get(i.prototype).is_some_and(|p| p.object_type == arcanum_formats::object::TYPE_AMMO))).map(|i| i.quantity).sum()
    };
    content.push(text_cell(&mut commands, &fonts, InventoryText::Gold, &count(Some(GOLD), None).to_string(), band(COUNTS_X, COUNTS_Y[0], 58.0, 20.0)));
    for a in 0..AMMO_TYPES {
        content.push(text_cell(&mut commands, &fonts, InventoryText::Ammo(a), &count(None, Some(a as i32)).to_string(), band(COUNTS_X, COUNTS_Y[a + 1], 58.0, 20.0)));
    }
    commands.entity(root).add_children(&content);
}

fn text_cell(commands: &mut Commands, fonts: &ArcanumFonts, which: InventoryText, text: &str, r: Rect) -> Entity {
    commands
        .spawn((InventoryContent, bevy::picking::Pickable::IGNORE, Node { overflow: Overflow::clip(), ..at(r) }))
        .with_children(|cell| {
            let mut t = cell.spawn((which, TextColor(SCREEN_WHITE)));
            fonts.insert_text(&mut t, text, UiFont::Body);
        })
        .id()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sword(n: u32) -> Option<ItemInfo> {
        (n == 1).then_some(ItemInfo { kind: TYPE_WEAPON, inventory_art: 0, cells: (1, 3), weight: 1, magic_weight_adj: 0, complexity: 0, generic_flags: 0, weapon_flags: 0, armor_flags: 0 })
    }

    #[test]
    fn the_cursor_finds_cells_slots_and_items() {
        assert_eq!(screen_point(Vec2::new(1600.0, 1200.0), Vec2::new(800.0, 600.0)), Vec2::new(400.0, 300.0));
        assert_eq!(screen_point(Vec2::new(2560.0, 1200.0), Vec2::new(480.0, 0.0)), Vec2::new(0.0, 0.0), "a wide window centres the screen");
        assert_eq!(location_at(Vec2::new(368.0, 8.0)), Some(0));
        assert_eq!(location_at(Vec2::new(368.0 + 32.0 * 9.5, 8.0 + 32.0 * 11.5)), Some(119));
        assert_eq!(location_at(Vec2::new(30.0, 200.0)), Some(Slot::Weapon as i32));
        assert_eq!(location_at(Vec2::new(700.0, 200.0)), None);
        let inv = Inventory { items: vec![CarriedItem { prototype: 1, location: 13, quantity: 0 }] };
        let on = |cell: i32| Vec2::new(368.0 + 32.0 * (cell % 10) as f32 + 5.0, 8.0 + 32.0 * (cell / 10) as f32 + 5.0);
        assert!(item_at(&inv, on(33), sword).is_some(), "the footprint's last cell");
        assert!(item_at(&inv, on(43), sword).is_none());
        // Picked up by its middle cell, dropped with that cell under the cursor.
        let held = Held { from: 13, grab: 10, offset: Vec2::ZERO };
        assert_eq!(drop_target(on(55), held), Some(45));
        assert_eq!(drop_target(on(5), held), None, "above row 0 it goes back");
        assert_eq!(drop_target(Vec2::new(30.0, 200.0), held), Some(Slot::Weapon as i32));
    }

    #[test]
    fn moves_show_at_once_and_the_hosts_answers_win() {
        let man = Wearer { race: 0, gender: 1 };
        let mut inv = PcInventory::default();
        assert!(inv.request(InventoryRequest::Move { from: 0, to: 1 }, man, sword).is_err(), "nothing before the host's copy");
        inv.receive(0, Inventory { items: vec![CarriedItem { prototype: 1, location: 0, quantity: 0 }] });
        inv.settle(man, sword);
        inv.request(InventoryRequest::Move { from: 0, to: 5 }, man, sword).unwrap();
        inv.request(InventoryRequest::Move { from: 5, to: 7 }, man, sword).unwrap();
        assert_eq!(inv.shown().unwrap().items[0].location, 7);
        assert_eq!(inv.take_outgoing().len(), 2);
        // The host took the first; the second is replayed on its copy.
        inv.receive(1, Inventory { items: vec![CarriedItem { prototype: 1, location: 5, quantity: 0 }] });
        inv.settle(man, sword);
        assert_eq!(inv.shown().unwrap().items[0].location, 7);
        // The host refused the second: its copy wins.
        inv.receive(2, Inventory { items: vec![CarriedItem { prototype: 1, location: 5, quantity: 0 }] });
        inv.settle(man, sword);
        assert_eq!(inv.shown().unwrap().items[0].location, 5);
        assert_eq!(inv.request(InventoryRequest::Move { from: 5, to: 1001 }, man, sword), Err(ItemRefusal::WrongSlot));
    }

    #[test]
    fn items_sit_centred_in_their_footprint() {
        let item = CarriedItem { prototype: 1, location: 12, quantity: 0 };
        let inside = placement(&item, (2, 3)).unwrap();
        assert_eq!(inside, Rect::from_corners(Vec2::new(432.0, 40.0), Vec2::new(496.0, 136.0)));
        assert_eq!(centred(inside, UVec2::new(41, 90), true), Vec2::new(443.0, 43.0));
        let sword = CarriedItem { prototype: 1, location: Slot::Weapon as i32, quantity: 0 };
        assert_eq!(placement(&sword, (1, 1)).unwrap().min, Vec2::new(23.0, 170.0));
        assert_eq!(centred(placement(&sword, (1, 1)).unwrap(), UVec2::new(101, 128), false), Vec2::new(20.0, 170.0), "an arithmetic shift rounds down");
    }
}
