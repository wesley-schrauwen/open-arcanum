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
//!   go to the message box. The held art's top-left sits at the cursor and the item lands with
//!   its top-left cell there, so the picture is where it goes (the owner's call: the original
//!   keeps the grabbed cell under the cursor, and its cursor hotspot was not decoded). The drop
//!   box lights (`Drop_ON`) while a held item is over it.
//! - Toggled by the top bar's inventory button and `I`; Escape closes it; opening it closes the
//!   character screen and the other way round. Clicks on it do not walk.
//!
//! - Releasing a held item on the drop box drops it on the ground at the PC's feet
//!   ([`arcanum_sim::ground`]).
//!
//! - **Loot window** (mode 2, for junk piles: 0x57cb10): `Loot` (art 234) in place of `PDoll`,
//!   the pile's portrait (`Cont_Junk`, 832) at (183, 33), Take All (`TakeAllButt`, 808) at
//!   (301, 47), and the pile's grid from (8, 136): 10 columns, 8 rows shown of its 96, scrolled a
//!   row a wheel step (0-88). Items drag between the two grids (the grabbed cell under the cursor,
//!   as in the PC's own); a right click sends an item to the other side where the insert puts it;
//!   Take All takes what fits, in order. The PC's paperdoll and its lines are hidden, as the
//!   original's default view (toggle 0x68150c off). Closing tells the host, which collapses a pile
//!   left empty or with one item.
//!
//! - **Barter window** (mode 1, `plans/barter.md`): `Barter` (art 235) in place of `PDoll`, the
//!   NPC's goods from (8, 168), 7 rows shown (its gold hidden); moves across are buys and sells
//!   through the host; the text line (89, 128, 253, 30) shows the hovered or held item's line
//!   with its price, else the NPC's last refusal.
//!
//! Not built: using, the use box, dropping by releasing over the world, splitting stacks, the arrange button, total attack and
//! defence, item hover help, barter's portrait, name, paperdolls and gamble box, the loot window's paperdoll toggle (art 338/339) and its
//! scrollbar art (not found), moving items within a pile. The original's fonts for the text and
//! numbers were not identified (Flare12Font stands in). The original's encumbrance line also
//! prints a stray number from its stack (`%s: %s (%d)` given two values); it is left out.

use arcanum_formats::art_id::{ArtId, ArtIndex};
use arcanum_formats::object::{Prototypes, TYPE_ARMOR, TYPE_WEAPON};
use arcanum_sim::{AMMO_TYPES, CarriedItem, GOLD, GroundItemId, Inventory, InventoryRequest, ItemInfo, ItemRefusal, PlayerIntent, Slot, Wearer, in_grid};
use bevy::prelude::*;
use bevy::picking::hover::Hovered;
use bevy::ui_widgets::{Activate, ActivateOnPress, Button};

use crate::widgets::FrameButton;

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
    /// The pile it shows beside the PC's grid (the loot window), if any.
    pub pile: Option<GroundItemId>,
    /// It shows the barter NPC's goods beside the PC's grid (mode 1).
    pub barter: bool,
}

impl InventoryPanel {
    /// Another inventory is shown beside the PC's grid (loot or barter): the PC's slots and
    /// lines are hidden.
    pub fn two_sided(&self) -> bool {
        self.pile.is_some() || self.barter
    }

    /// The other side's grid.
    pub fn side(&self) -> Option<SideGrid> {
        if self.barter {
            Some(BARTER_SIDE)
        } else {
            self.pile.map(|_| LOOT_SIDE)
        }
    }
}

/// The other side's grid: its top-left cell and the rows shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideGrid {
    pub origin: Vec2,
    pub rows: i32,
}

/// The loot window's pile grid.
pub const LOOT_SIDE: SideGrid = SideGrid { origin: PILE_GRID, rows: PILE_ROWS_SHOWN };
/// The barter NPC's grid (0x575cb0: from (8, 168), 7 rows, clipped to 224).
pub const BARTER_SIDE: SideGrid = SideGrid { origin: Vec2::new(8.0, 168.0), rows: 7 };
/// The barter window's text line: the item under the cursor, or the NPC's refusal (0x576520).
pub const BARTER_LINE: Rect = Rect { min: Vec2::new(89.0, 128.0), max: Vec2::new(342.0, 158.0) };

/// The loot window's first pile row shown.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LootScroll(pub i32);

/// The pile grid's top-left cell, in the band.
pub const PILE_GRID: Vec2 = Vec2::new(8.0, 136.0);
/// Pile rows shown at once.
pub const PILE_ROWS_SHOWN: i32 = 8;
/// The furthest the pile grid scrolls (0x681510 for a container).
pub const PILE_SCROLL_MAX: i32 = 88;

/// The drop box's art: lit while a held item is over it.
#[derive(Component)]
pub struct DropBoxArt;

/// The barter window's text line.
#[derive(Component)]
pub struct BarterText;

/// The loot window's Take All button.
#[derive(Component)]
pub struct TakeAllButton;

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
    /// The host's refusals to show (`mes/item.mes` ids).
    refused: Vec<u32>,
    /// Containers that would not open: `mes/inven_ui.mes` lines.
    refused_open: Vec<u32>,
    /// A pile the host opened for the loot window.
    opened: Option<GroundItemId>,
    /// The barter window's goods and prices, from the host.
    barter: Option<arcanum_sim::barter::BarterView>,
    /// The host opened a barter window.
    barter_opened: bool,
    /// The barter NPC's last refusal.
    barter_line: Option<String>,
    /// The party left the map: the window closes.
    left_map: bool,
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

    /// Queues a loot request: the pile is the host's, so nothing is shown until it answers.
    pub fn send(&mut self, request: InventoryRequest) {
        self.last_seq += 1;
        self.outgoing.push(PlayerIntent::Inventory { seq: self.last_seq, request });
    }

    /// The host opened `pile` for the loot window.
    pub fn open_pile(&mut self, pile: GroundItemId) {
        self.opened = Some(pile);
    }

    /// The host's barter window: the first view opens it.
    pub fn barter_view(&mut self, view: arcanum_sim::barter::BarterView) {
        if self.barter.is_none() {
            self.barter_opened = true;
            self.barter_line = None;
        }
        self.barter = Some(view);
    }

    /// The party went to another map: the loot and barter windows close (their host side is
    /// gone).
    pub fn left_map(&mut self) {
        self.barter = None;
        self.opened = None;
        self.left_map = true;
    }

    /// The barter NPC refused a trade.
    pub fn barter_refused(&mut self, text: String) {
        self.barter_line = Some(text);
    }

    /// The barter window's view, while open.
    pub fn barter(&self) -> Option<&arcanum_sim::barter::BarterView> {
        self.barter.as_ref()
    }

    /// The NPC's goods as the grid shows them: its gold is not shown.
    pub fn barter_goods(&self) -> Option<Inventory> {
        let view = self.barter.as_ref()?;
        Some(Inventory { items: view.stock.items.iter().filter(|i| i.prototype != GOLD).cloned().collect(), ..view.stock.clone() })
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

    /// Keeps a refusal from the host for the message box.
    pub fn refuse(&mut self, message: u32) {
        self.refused.push(message);
    }

    /// Keeps a container's refusal to open (`mes/inven_ui.mes`) for the message box.
    pub fn refuse_open(&mut self, message: u32) {
        self.refused_open.push(message);
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
    /// Taken from the open pile, not the PC.
    pub from_pile: bool,
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
            .init_resource::<LootScroll>()
            .add_message::<bevy::input::mouse::MouseWheel>()
            .add_systems(OnExit(AppState::InGame), |mut panel: ResMut<InventoryPanel>, mut inv: ResMut<PcInventory>, mut held: ResMut<HeldItem>| {
                *panel = InventoryPanel::default();
                *inv = PcInventory::default();
                *held = HeldItem::default();
            })
            .add_systems(PreUpdate, toggle_on_keys.run_if(in_state(AppState::InGame)).after(bevy::input::InputSystems))
            .add_systems(Update, (settle_inventory, show_refusals, open_loot, one_panel_at_a_time, show_panel, scroll_pile, drag_items, show_items, follow_cursor, light_drop_box, show_barter_line).chain().run_if(in_state(AppState::InGame)))
            .add_observer(on_hud_button)
            .add_observer(on_take_all);
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

/// The host opened a pile: the loot window shows it.
fn open_loot(mut inventory: ResMut<PcInventory>, mut panel: ResMut<InventoryPanel>, mut scroll: ResMut<LootScroll>) {
    if let Some(pile) = inventory.opened.take() {
        *panel = InventoryPanel { open: true, pile: Some(pile), barter: false };
        scroll.0 = 0;
    }
    if std::mem::take(&mut inventory.barter_opened) {
        *panel = InventoryPanel { open: true, pile: None, barter: true };
        scroll.0 = 0;
    }
    if std::mem::take(&mut inventory.left_map) && panel.two_sided() {
        panel.open = false;
    }
}

/// The other side's items: the pile, or the barter NPC's goods.
fn other_side(panel: &InventoryPanel, client: Option<&crate::world_render::ClientMap>, inventory: &PcInventory) -> Option<Inventory> {
    if panel.barter {
        return inventory.barter_goods();
    }
    panel.pile.and_then(|p| client?.map.pile(p)).map(|p| p.inventory)
}

fn on_take_all(activate: On<Activate>, buttons: Query<(), With<TakeAllButton>>, mut inventory: ResMut<PcInventory>) {
    if buttons.contains(activate.entity) {
        inventory.send(InventoryRequest::TakeAll);
    }
}

/// The mouse wheel over the pile's grid scrolls it a row a step.
fn scroll_pile(panel: Res<InventoryPanel>, inventory: Res<PcInventory>, mut wheel: MessageReader<bevy::input::mouse::MouseWheel>, windows: Query<&Window>, mut scroll: ResMut<LootScroll>) {
    let steps: f32 = wheel.read().map(|w| w.y.signum()).sum();
    let Some(side) = panel.side().filter(|_| panel.open && steps != 0.0) else { return };
    let Some(point) = windows.iter().find_map(|w| w.cursor_position().map(|c| screen_point(w.size(), c) - Vec2::new(0.0, TOP_BAR_HEIGHT))) else { return };
    // The wheel region (0x5cafa0): the grid and its track, (8, 136, 337, 256) for a pile.
    // Barter: a critter's 12 rows, or a substitute container's 96 (builder's call).
    let max = if panel.barter { inventory.barter().map_or(0, |v| v.stock.rows - side.rows).max(0) } else { PILE_SCROLL_MAX };
    if Rect::from_corners(side.origin, side.origin + Vec2::new(337.0, side.rows as f32 * 32.0)).contains(point) {
        let to = (scroll.0 - steps as i32).clamp(0, max);
        if to != scroll.0 {
            scroll.0 = to;
        }
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

#[allow(clippy::too_many_arguments)]
fn show_panel(mut commands: Commands, mut panel: ResMut<InventoryPanel>, roots: Query<Entity, With<InventoryScreen>>, assets: Res<AssetServer>, mut inventory: ResMut<PcInventory>, client: Option<Res<crate::world_render::ClientMap>>, mut shown: Local<(Option<GroundItemId>, bool)>, fonts: Option<Res<ArcanumFonts>>) {
    if !panel.is_changed() {
        return;
    }
    let built = !roots.is_empty();
    if built && panel.open && *shown == (panel.pile, panel.barter) {
        return;
    }
    if built {
        for root in &roots {
            commands.entity(root).despawn();
        }
        commands.insert_resource(HeldItem::default());
        // The loot window closed, or shows another pile now: the host may collapse the old one.
        if shown.0.is_some() {
            inventory.send(InventoryRequest::ClosePile);
        }
        if shown.1 {
            inventory.send(InventoryRequest::CloseBarter);
            inventory.barter = None;
        }
    }
    if !panel.open {
        if panel.pile.is_some() || panel.barter {
            panel.pile = None;
            panel.barter = false;
        }
        *shown = (None, false);
        return;
    }
    *shown = (panel.pile, panel.barter);
    let loot = panel.pile.is_some();
    // The other side's portrait by its prototype: a pile's, or the container's.
    let portrait = match panel.pile {
        Some(id @ GroundItemId::Container { .. }) => client.as_ref().and_then(|c| c.map.container_object(id)).and_then(|o| o.prototype.prototype()).map_or("ContainerIcon.ART", arcanum_sim::portrait),
        _ => arcanum_sim::portrait(arcanum_sim::PILE),
    };
    let image = |name: &str, x: f32, y: f32, w: f32, h: f32| (ImageNode::new(assets.load(art(name))), at(band(x, y, w, h)));
    commands
        .spawn((
            InventoryScreen,
            DespawnOnExit(AppState::InGame),
            bevy::picking::Pickable::IGNORE,
            Node { position_type: PositionType::Absolute, top: Val::Px(0.0), left: Val::Percent(50.0), margin: UiRect::left(px(-400.0)), width: px(800.0), height: px(600.0), ..default() },
        ))
        .with_children(|screen| {
            // The window starts black (the loot art's lens is see-through).
            screen.spawn((BackgroundColor(Color::BLACK), at(band(0.0, 0.0, 800.0, 400.0))));
            if panel.barter {
                // Barter (art 235); the NPC's portrait and name are not drawn yet.
                screen.spawn(image("Barter.ART", 0.0, 0.0, 358.0, 400.0));
                // The NPC's name, centred on x 215 at y 102 (0x5727b0).
                let name = inventory.barter().map(|v| v.npc_name.clone()).unwrap_or_default();
                let fonts = fonts.as_deref().cloned().unwrap_or_default();
                screen.spawn((bevy::picking::Pickable::IGNORE, Node { justify_content: JustifyContent::Center, overflow: Overflow::clip(), ..at(band(115.0, 102.0, 200.0, 20.0)) })).with_children(|cell| {
                    let mut t = cell.spawn((TextColor(SCREEN_WHITE), bevy::picking::Pickable::IGNORE));
                    fonts.insert_text(&mut t, &name, UiFont::Body);
                });
                screen.spawn((BarterText, bevy::picking::Pickable::IGNORE, Node { overflow: Overflow::clip(), ..at(band(BARTER_LINE.min.x, BARTER_LINE.min.y, BARTER_LINE.width(), BARTER_LINE.height())) }));
            } else if loot {
                screen.spawn(image("Loot.ART", 0.0, 0.0, 358.0, 400.0));
                screen.spawn((ImageNode::new(assets.load(art(portrait))), Node { width: Val::Auto, height: Val::Auto, ..at(band(183.0, 33.0, 64.0, 64.0)) }));
                screen.spawn((TakeAllButton, Button, ActivateOnPress, Hovered::default(), FrameButton::arrow(assets.load(art("TakeAllButt.ART"))), ImageNode::default(), at(band(301.0, 47.0, 51.0, 50.0))));
            } else {
                screen.spawn(image("PDoll.ART", 0.0, 0.0, 358.0, 400.0));
            }
            screen.spawn(image("Inventor.ART", 358.0, 0.0, 442.0, 400.0));
            if !panel.two_sided() {
                screen.spawn(image("TotalAttack.ART", 29.0, 318.0, 54.0, 71.0));
                screen.spawn(image("TotalDefens.ART", 254.0, 319.0, 54.0, 71.0));
            }
            screen.spawn(image("tiny_butt.ART", 763.0, 197.0, 28.0, 27.0));
            screen.spawn(image("Use_Box_OFF.ART", 703.0, 290.0, 80.0, 43.0));
            screen.spawn((DropBoxArt, image("Drop_OFF.ART", 703.0, 344.0, 80.0, 43.0)));
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

/// Red paint over a broken item's art (owner's call; the original swaps in a broken art,
/// bit 0x400 of the item's art id).
pub(crate) const BROKEN_TINT: Color = Color::srgb(1.0, 0.25, 0.25);

/// The colour an item's art is drawn in: [`BROKEN_TINT`] when broken, else plain.
pub(crate) fn item_tint(wear: &arcanum_sim::ItemWear) -> Color {
    if wear.broken { BROKEN_TINT } else { Color::WHITE }
}

/// An item's art drawn in its [`item_tint`].
pub(crate) fn item_image(image: Handle<Image>, wear: &arcanum_sim::ItemWear) -> ImageNode {
    ImageNode { color: item_tint(wear), ..ImageNode::new(image) }
}

/// The art an item shows at its location: worn weapons and armour their paperdoll art.
pub(crate) fn item_art(prototypes: &Prototypes, item: &CarriedItem) -> Option<ArtId> {
    let p = prototypes.get(item.prototype)?;
    let field = match p.object_type {
        TYPE_WEAPON if !in_grid(item.location) => "weapon_paper_doll_aid",
        TYPE_ARMOR if !in_grid(item.location) => "armor_paper_doll_aid",
        _ => "item_inv_aid",
    };
    p.int(field).or_else(|| p.int("item_inv_aid")).map(|v| ArtId(v as u32))
}

/// The first frame of art `id`, if loaded (a handle is kept in `cache`).
pub(crate) fn art_frame(index: Option<&ArtIndex>, cache: &mut ItemArtCache, assets: &AssetServer, frames: &Assets<ArtFrames>, id: ArtId) -> Option<(Handle<Image>, UVec2)> {
    let handle = cache.get(assets, index?.path(id)?);
    frames.get(&handle).and_then(|f| f.rotation(0)?.frame(0).map(|(i, _, s)| (i.clone(), s)))
}

/// Item facts for the client's rules, footprints from the loaded inventory art.
pub(crate) fn client_info<'a>(prototypes: &'a Prototypes, sizes: &'a std::collections::HashMap<u32, (u32, u32)>) -> impl Fn(u32) -> Option<ItemInfo> + 'a {
    move |n| prototypes.get(n).map(|p| ItemInfo::of(p, |id| sizes.get(&id.0).copied()))
}

/// The loaded inventory art sizes of everything in `inv`, by art id.
pub(crate) fn inventory_sizes(inv: &Inventory, prototypes: &Prototypes, index: Option<&ArtIndex>, cache: &mut ItemArtCache, assets: &AssetServer, frames: &Assets<ArtFrames>) -> std::collections::HashMap<u32, (u32, u32)> {
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
pub(crate) fn wearer(sheet: &crate::pc_sheet::SheetHost) -> Wearer {
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

/// What reading written `item` opens (0x56bb60): the written window's kind (its prototype's
/// `written_subtype`) and first line (the item's own, else its prototype's
/// `written_text_start_line`). Schematics (subtype 5) are not read here.
pub fn written_page(item: &CarriedItem, prototypes: &Prototypes) -> Option<(u8, i32)> {
    let proto = prototypes.get(item.prototype).filter(|p| p.object_type == arcanum_formats::object::TYPE_WRITTEN)?;
    let kind = u8::try_from(prototypes.int(proto, "written_subtype").unwrap_or(0)).ok().filter(|&k| k != 5)?;
    let line = if item.written != 0 { item.written } else { prototypes.int(proto, "written_text_start_line").unwrap_or(0) };
    Some((kind, line))
}

/// The use box (band coordinates).
pub const USE_BOX: Rect = Rect { min: Vec2::new(703.0, 290.0), max: Vec2::new(783.0, 333.0) };

/// The drop box (band coordinates; the original tests from screen (703, 385)).
pub const DROP_BOX: Rect = Rect { min: Vec2::new(703.0, 344.0), max: Vec2::new(783.0, 387.0) };

/// Shows the host's refusals in the message box.
fn show_refusals(mut inventory: ResMut<PcInventory>, data: Option<Res<CreationData>>, boxed: Option<ResMut<crate::message_box::MessageBox>>, time: Option<Res<Time<Real>>>) {
    if inventory.refused.is_empty() && inventory.refused_open.is_empty() {
        return;
    }
    let refused = std::mem::take(&mut inventory.refused);
    let refused_open = std::mem::take(&mut inventory.refused_open);
    let Some(mut boxed) = boxed else { return };
    let texts = refused.into_iter().map(|id| data.as_ref().and_then(|d| d.item.get(id))).chain(refused_open.into_iter().map(|id| data.as_ref().and_then(|d| d.inven_ui.get(id))));
    for text in texts.collect::<Vec<_>>() {
        let text = text.unwrap_or("").to_string();
        boxed.post(crate::message_box::BoxMessage::plain(text), time.as_ref().map_or(0.0, |t| t.elapsed_secs_f64()));
    }
}

/// The 800x600 point under the window cursor (the screens are centred, scaled to the height).
pub fn screen_point(window: Vec2, cursor: Vec2) -> Vec2 {
    let scale = window.y / 600.0;
    Vec2::new((cursor.x - (window.x / 2.0 - 400.0 * scale)) / scale, cursor.y / scale)
}

/// A place on the screen an item can be: the PC's grid or slots, or the open pile's grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    Pc(i32),
    Pile(i32),
}

/// What is at `point` in the band (0x575cb0): with another side open, its grid scrolled by
/// `scroll` rows and the PC's grid (its slots are hidden); otherwise the PC's grid and slots.
pub fn spot_at(point: Vec2, side: Option<SideGrid>, scroll: i32) -> Option<Spot> {
    if let Some(side) = side {
        let (col, row) = (((point.x - side.origin.x) / 32.0).floor(), ((point.y - side.origin.y) / 32.0).floor());
        if (0.0..10.0).contains(&col) && (0.0..side.rows as f32).contains(&row) {
            return Some(Spot::Pile((row as i32 + scroll) * 10 + col as i32));
        }
        return location_at(point).filter(|l| in_grid(*l)).map(Spot::Pc);
    }
    location_at(point).map(Spot::Pc)
}

/// The pile item covering `location` of `pile`.
fn pile_item_at(pile: &Inventory, location: i32, info: impl Fn(u32) -> Option<ItemInfo>) -> Option<&CarriedItem> {
    let (col, row) = (location % 10, location / 10);
    pile.items.iter().find(|i| {
        let (w, h) = info(i.prototype).map_or((1, 1), |x| x.cells);
        let (c, r) = (i.location % 10, i.location / 10);
        (c..c + w).contains(&col) && (r..r + h).contains(&row)
    })
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
    mut panel: ResMut<InventoryPanel>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    mut held: ResMut<HeldItem>,
    mut inventory: ResMut<PcInventory>,
    sheet: Res<crate::pc_sheet::SheetHost>,
    client: Option<Res<crate::world_render::ClientMap>>,
    data: Option<Res<CreationData>>,
    (mut cache, assets, frames): (ResMut<ItemArtCache>, Res<AssetServer>, Res<Assets<ArtFrames>>),
    (boxed, time): (Option<ResMut<crate::message_box::MessageBox>>, Option<Res<Time<Real>>>),
    scroll: Res<LootScroll>,
    (armed, mut written): (Option<ResMut<crate::skill_use::SkillTargeting>>, Option<ResMut<crate::written_window::WrittenPanel>>),
) {
    let right = buttons.just_pressed(MouseButton::Right);
    if !panel.open || !(buttons.just_pressed(MouseButton::Left) || buttons.just_released(MouseButton::Left) || right) {
        return;
    }
    let Some(point) = windows.iter().find_map(|w| w.cursor_position().map(|c| screen_point(w.size(), c))) else { return };
    let point = point - Vec2::new(0.0, TOP_BAR_HEIGHT);
    let Some(inv) = inventory.shown().cloned() else { return };
    let empty = Prototypes::default();
    let prototypes = client.as_ref().map_or(&empty, |c| &c.map.prototypes);
    let index = client.as_ref().and_then(|c| c.art_index());
    let pile = other_side(&panel, client.as_deref(), &inventory);
    let mut sizes = inventory_sizes(&inv, prototypes, index, &mut cache, &assets, &frames);
    if let Some(pile) = &pile {
        sizes.extend(inventory_sizes(pile, prototypes, index, &mut cache, &assets, &frames));
    }
    let info = client_info(prototypes, &sizes);
    let loot = panel.two_sided();
    let barter = panel.barter;
    let spot = spot_at(point, panel.side(), scroll.0);
    // Barter trades as the loot window moves: taking is buying, putting is selling.
    let take = |from: i32, to: Option<i32>| if barter { InventoryRequest::Buy { from, to } } else { InventoryRequest::Take { from, to } };
    let put = |from: i32, to: Option<i32>| if barter { InventoryRequest::Sell { from, to } } else { InventoryRequest::Put { from, to } };
    if (buttons.just_pressed(MouseButton::Left) || right) && held.0.is_none() {
        let hotkey = crate::hotkey_bar::slot_at(point + Vec2::new(0.0, TOP_BAR_HEIGHT)).map(crate::hotkey_bar::location);
        let picked = match spot {
            _ if hotkey.is_some() => inv
                .items
                .iter()
                .find(|i| Some(i.location) == hotkey)
                .cloned()
                .or_else(|| hotkey.filter(|h| inv.spells.contains_key(h)).map(|h| CarriedItem { prototype: 0, location: h, quantity: 0, keys: vec![], written: 0, wear: Default::default() }))
                .map(|i| (i.clone(), i.location, false)),
            Some(Spot::Pile(at)) => pile.as_ref().and_then(|p| pile_item_at(p, at, &info)).map(|i| (i.clone(), at, true)),
            Some(Spot::Pc(_)) => item_at(&inv, point, &info).map(|i| (i.clone(), location_at(point).unwrap_or(i.location), false)),
            None => None,
        };
        let Some((item, at, from_pile)) = picked else { return };
        // Repair armed (from the skills view): a left click on a carried item repairs it and
        // puts the skill away (builder's call: the original's target cursor over the window).
        if !right
            && !from_pile
            && let Some(mut armed) = armed
            && armed.0 == Some(crate::skill_use::Armed::Skill { skill: arcanum_sim::repair::SKILL_REPAIR, tool: None })
        {
            armed.0 = None;
            inventory.send(InventoryRequest::Repair { from: item.location });
            return;
        }
        if right && hotkey.is_some() {
            return;
        }
        if right {
            // A right click sends it to the other side where the insert puts it (0x574248).
            if loot {
                inventory.send(if from_pile { take(item.location, None) } else { put(item.location, None) });
            } else if let (Some(page), Some(written)) = (written_page(&item, prototypes), written.as_mut()) {
                // Not looting: a written item is read (builder's call: the original's use).
                written.show(page.0, page.1);
                panel.open = false;
            }
            return;
        }
        // The held art's top-left sits at the cursor, and the item lands with its top-left cell
        // under it: what is seen is where it goes (the owner's call; the original keeps the
        // grabbed cell, 0x683470, and its cursor hotspot was not decoded).
        let _ = at;
        held.0 = Some(Held { from: item.location, grab: 0, offset: Vec2::ZERO, from_pile });
    } else if buttons.just_released(MouseButton::Left)
        && let Some(h) = held.0.take()
    {
        if USE_BOX.contains(point) && !h.from_pile {
            // A lockpick on the use box arms Pick Locks with it; a written item is read
            // (0x56bb60); the screen closes.
            let used = inv.items.iter().find(|i| i.location == h.from);
            let lockpick = used.is_some_and(|i| arcanum_sim::is_lockpick(i.prototype, prototypes));
            if lockpick && let Some(mut armed) = armed {
                armed.0 = Some(crate::skill_use::Armed::Skill { skill: arcanum_sim::SKILL_PICK_LOCKS, tool: Some(h.from) });
                panel.open = false;
            } else if let (Some(page), Some(written)) = (used.and_then(|i| written_page(i, prototypes)), written.as_mut()) {
                written.show(page.0, page.1);
                panel.open = false;
            }
            return;
        }
        let hotkey = crate::hotkey_bar::slot_at(point + Vec2::new(0.0, TOP_BAR_HEIGHT)).map(crate::hotkey_bar::location);
        let request = if let Some(to) = hotkey {
            // Onto the hotkey bar (0x57e8d0): the PC's items only.
            (!h.from_pile && to != h.from).then_some(InventoryRequest::Move { from: h.from, to })
        } else if DROP_BOX.contains(point) && !h.from_pile {
            // On the drop box: onto the ground at the PC's feet.
            Some(InventoryRequest::Drop { from: h.from })
        } else {
            match (spot, h.from_pile) {
                (Some(Spot::Pile(at)), false) => (at - h.grab >= 0).then(|| put(h.from, Some(at - h.grab))),
                (Some(Spot::Pc(at)), true) => {
                    let to = if in_grid(at) { at - h.grab } else { at };
                    (to >= 0).then(|| take(h.from, Some(to)))
                }
                (Some(Spot::Pc(_)), false) => drop_target(point, h).filter(|to| *to != h.from).map(|to| InventoryRequest::Move { from: h.from, to }),
                _ => None,
            }
        };
        let Some(request) = request else { return };
        if request.is_host_only() {
            inventory.send(request);
            return;
        }
        let Err(refusal) = inventory.request(request, wearer(&sheet), &info) else { return };
        let (Some(id), Some(mut boxed)) = (refusal.message(), boxed) else { return };
        let text = data.as_ref().and_then(|d| d.item.get(id)).unwrap_or("").to_string();
        boxed.post(crate::message_box::BoxMessage::plain(text), time.map_or(0.0, |t| t.elapsed_secs_f64()));
    }
}

/// The barter window's text line (0x578330): the line for the item under the cursor (the held
/// one while dragging), else the NPC's last refusal. The host made each line with its price.
#[allow(clippy::too_many_arguments)]
fn show_barter_line(
    mut commands: Commands,
    inventory: Res<PcInventory>,
    panel: Res<InventoryPanel>,
    held: Res<HeldItem>,
    scroll: Res<LootScroll>,
    windows: Query<&Window>,
    client: Option<Res<crate::world_render::ClientMap>>,
    fonts: Option<Res<ArcanumFonts>>,
    boxes: Query<Entity, With<BarterText>>,
    (mut cache, assets, frames): (ResMut<ItemArtCache>, Res<AssetServer>, Res<Assets<ArtFrames>>),
    mut shown: Local<Option<String>>,
) {
    let (Ok(text_box), Some(view), Some(inv)) = (boxes.single(), inventory.barter(), inventory.shown()) else {
        *shown = None;
        return;
    };
    let point = windows.iter().find_map(|w| w.cursor_position().map(|c| screen_point(w.size(), c) - Vec2::new(0.0, TOP_BAR_HEIGHT)));
    let empty = Prototypes::default();
    let prototypes = client.as_ref().map_or(&empty, |c| &c.map.prototypes);
    let index = client.as_ref().and_then(|c| c.art_index());
    let goods = inventory.barter_goods().unwrap_or_default();
    let mut sizes = inventory_sizes(inv, prototypes, index, &mut cache, &assets, &frames);
    sizes.extend(inventory_sizes(&goods, prototypes, index, &mut cache, &assets, &frames));
    let info = client_info(prototypes, &sizes);
    let line = match (held.0, point.and_then(|p| spot_at(p, panel.side(), scroll.0).map(|s| (s, p)))) {
        (Some(h), _) if !h.from_pile => view.sell.get(&h.from).map(|o| o.line.clone()),
        (Some(h), _) => view.buy.get(&h.from).map(|o| o.line.clone()),
        (None, Some((Spot::Pile(at), _))) => pile_item_at(&goods, at, &info).and_then(|i| view.buy.get(&i.location)).map(|o| o.line.clone()),
        (None, Some((Spot::Pc(_), p))) => item_at(inv, p, &info).and_then(|i| view.sell.get(&i.location)).map(|o| o.line.clone()),
        (None, None) => None,
    };
    let line = line.or_else(|| inventory.barter_line.clone()).unwrap_or_default();
    if shown.as_deref() == Some(line.as_str()) {
        return;
    }
    *shown = Some(line.clone());
    commands.entity(text_box).despawn_children();
    let fonts = fonts.map(|f| f.clone()).unwrap_or_default();
    commands.entity(text_box).with_children(|cell| {
        let mut t = cell.spawn((TextColor(SCREEN_WHITE), bevy::picking::Pickable::IGNORE));
        fonts.insert_text(&mut t, &line, UiFont::Body);
    });
}

/// A pile item's footprint box in the band, with the grid scrolled by `scroll` rows.
fn pile_box(location: i32, cells: (i32, i32), scroll: i32, origin: Vec2) -> Rect {
    let at = origin + Vec2::new((location % 10) as f32, (location / 10 - scroll) as f32) * 32.0;
    Rect::from_corners(at, at + Vec2::new(cells.0 as f32, cells.1 as f32) * 32.0)
}

/// The drop box lights while a held item of the PC's is over it (the original swaps its art:
/// `Drop_ON`, art 225, or `Drop_ILL`, 348, for an item that cannot be dropped).
#[allow(clippy::too_many_arguments)]
fn light_drop_box(
    held: Res<HeldItem>,
    inventory: Res<PcInventory>,
    windows: Query<&Window>,
    client: Option<Res<crate::world_render::ClientMap>>,
    assets: Res<AssetServer>,
    mut boxes: Query<&mut ImageNode, With<DropBoxArt>>,
    mut lit: Local<Option<&'static str>>,
) {
    let point = windows.iter().find_map(|w| w.cursor_position().map(|c| screen_point(w.size(), c) - Vec2::new(0.0, TOP_BAR_HEIGHT)));
    let over = held.0.filter(|h| !h.from_pile && point.is_some_and(|p| DROP_BOX.contains(p)));
    let name = match over {
        None => "Drop_OFF.ART",
        Some(h) => {
            let no_drop = inventory
                .shown()
                .and_then(|inv| inv.items.iter().find(|i| i.location == h.from))
                .and_then(|i| client.as_ref()?.map.prototypes.get(i.prototype)?.int("item_flags"))
                .is_some_and(|f| f & arcanum_sim::ITEM_NO_DROP != 0);
            if no_drop { "Drop_ILL.ART" } else { "Drop_ON.ART" }
        }
    };
    if *lit == Some(name) && !boxes.is_empty() {
        return;
    }
    *lit = Some(name);
    for mut image in &mut boxes {
        image.image = assets.load(art(name));
    }
}

/// Draws the held item with its top-left at the cursor.
#[allow(clippy::too_many_arguments)]
fn follow_cursor(
    mut commands: Commands,
    held: Res<HeldItem>,
    inventory: Res<PcInventory>,
    windows: Query<&Window>,
    roots: Query<Entity, With<InventoryScreen>>,
    mut arts: Query<(Entity, &mut Node), With<HeldArt>>,
    client: Option<Res<crate::world_render::ClientMap>>,
    (mut cache, assets, frames, panel): (ResMut<ItemArtCache>, Res<AssetServer>, Res<Assets<ArtFrames>>, Res<InventoryPanel>),
) {
    let item = held.0.and_then(|h| {
        let source = if h.from_pile { other_side(&panel, client.as_deref(), &inventory)? } else { inventory.shown()?.clone() };
        Some((h, source.items.iter().find(|i| i.location == h.from)?.clone()))
    });
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
    let art = commands.spawn((HeldArt, bevy::picking::Pickable::IGNORE, item_image(image, &grid_item.wear), at(rect(corner, Vec2::new(size.x as f32, size.y as f32))))).id();
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
    (panel, scroll, mut drawn_pile): (Res<InventoryPanel>, Res<LootScroll>, Local<Option<Inventory>>),
) {
    let Ok(root) = roots.single() else { return };
    let pile = other_side(&panel, client.as_deref(), &inventory);
    let pile_changed = pile != *drawn_pile;
    if !inventory.is_changed() && !held.is_changed() && !scroll.is_changed() && !pile_changed && added.is_empty() && !*pending {
        return;
    }
    *pending = false;
    *drawn_pile = pile.clone();
    let loot = panel.two_sided();
    let side = panel.side().unwrap_or(LOOT_SIDE);
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
        if !loot && inv.worn(slot).is_none() {
            content.push(commands.spawn((InventoryContent, bevy::picking::Pickable::IGNORE, ImageNode::new(assets.load(art(cover))), Node { width: Val::Auto, height: Val::Auto, ..at(band(r[0], r[1], r[2], r[3])) })).id());
        }
    }
    // The held item is not drawn where it was (item flag 0x10, 0x4a50d0).
    let held_pc = held.0.filter(|h| !h.from_pile).map(|h| h.from);
    let held_pile = held.0.filter(|h| h.from_pile).map(|h| h.from);
    // The other side's items, clipped to its rows shown (8 for a pile, 7 for barter).
    if let Some(pile) = &pile {
        let clip = commands.spawn((InventoryContent, bevy::picking::Pickable::IGNORE, Node { overflow: Overflow::clip(), ..at(band(side.origin.x, side.origin.y, 320.0, side.rows as f32 * 32.0)) })).id();
        let mut inside = Vec::new();
        for item in pile.items.iter().filter(|i| Some(i.location) != held_pile && pile.in_grid(i.location)) {
            let Some(id) = item_art(prototypes, &CarriedItem { location: 0, ..item.clone() }) else { continue };
            let Some((image, size)) = art_frame(index, &mut cache, &assets, &frames, id) else {
                *pending = true;
                continue;
            };
            let boxed = pile_box(item.location, arcanum_sim::cells_for(size.x, size.y), scroll.0, side.origin);
            let corner = centred(boxed, size, true) - side.origin;
            inside.push(commands.spawn((DrawnItem(item.clone()), bevy::picking::Pickable::IGNORE, item_image(image, &item.wear), at(rect(corner, Vec2::new(size.x as f32, size.y as f32))))).id());
        }
        commands.entity(clip).add_children(&inside);
        content.push(clip);
    }
    for item in inv.items.iter().filter(|i| Some(i.location) != held_pc && !(loot && !in_grid(i.location))) {
        let Some(id) = item_art(prototypes, item) else { continue };
        let Some((image, size)) = art_frame(index, &mut cache, &assets, &frames, id) else {
            // Not loaded yet: draw again next frame.
            *pending = true;
            continue;
        };
        let cells = arcanum_sim::cells_for(size.x, size.y);
        let Some(inside) = placement(item, cells) else { continue };
        let corner = centred(inside, size, in_grid(item.location));
        content.push(commands.spawn((InventoryContent, DrawnItem(item.clone()), bevy::picking::Pickable::IGNORE, item_image(image, &item.wear), at(band(corner.x, corner.y, size.x as f32, size.y as f32)))).id());
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
    // The loot window hides the PC's lines (shown only with its paperdoll toggled on).
    for ((which, text), y) in lines.into_iter().zip(TEXT_LINES).filter(|_| !loot) {
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

    #[test]
    fn broken_items_are_painted_red() {
        let broken = arcanum_sim::ItemWear { broken: true, ..Default::default() };
        assert_eq!(item_tint(&broken), BROKEN_TINT);
        assert_eq!(item_tint(&arcanum_sim::ItemWear { damage: 30, ..Default::default() }), Color::WHITE, "worn, not broken");
        assert_eq!(item_image(Handle::default(), &broken).color, BROKEN_TINT);
    }

    use super::*;

    fn sword(n: u32) -> Option<ItemInfo> {
        (n == 1).then_some(ItemInfo { kind: TYPE_WEAPON, inventory_art: 0, cells: (1, 3), weight: 1, magic_weight_adj: 0, complexity: 0, generic_flags: 0, weapon_flags: 0, armor_flags: 0, item_flags: 0, worth: 0 })
    }

    #[test]
    fn the_cursor_finds_cells_slots_and_items() {
        assert_eq!(screen_point(Vec2::new(1600.0, 1200.0), Vec2::new(800.0, 600.0)), Vec2::new(400.0, 300.0));
        assert_eq!(screen_point(Vec2::new(2560.0, 1200.0), Vec2::new(480.0, 0.0)), Vec2::new(0.0, 0.0), "a wide window centres the screen");
        assert_eq!(location_at(Vec2::new(368.0, 8.0)), Some(0));
        assert_eq!(location_at(Vec2::new(368.0 + 32.0 * 9.5, 8.0 + 32.0 * 11.5)), Some(119));
        assert_eq!(location_at(Vec2::new(30.0, 200.0)), Some(Slot::Weapon as i32));
        assert_eq!(location_at(Vec2::new(700.0, 200.0)), None);
        let inv = Inventory { items: vec![CarriedItem { prototype: 1, location: 13, quantity: 0, keys: vec![], written: 0, wear: Default::default() }], ..Default::default() };
        let on = |cell: i32| Vec2::new(368.0 + 32.0 * (cell % 10) as f32 + 5.0, 8.0 + 32.0 * (cell / 10) as f32 + 5.0);
        assert!(item_at(&inv, on(33), sword).is_some(), "the footprint's last cell");
        assert!(item_at(&inv, on(43), sword).is_none());
        // Picked up by its middle cell, dropped with that cell under the cursor.
        let held = Held { from: 13, grab: 10, offset: Vec2::ZERO, from_pile: false };
        assert_eq!(drop_target(on(55), held), Some(45));
        assert_eq!(drop_target(on(5), held), None, "above row 0 it goes back");
        assert_eq!(drop_target(Vec2::new(30.0, 200.0), held), Some(Slot::Weapon as i32));
    }

    #[test]
    fn the_loot_window_finds_the_piles_cells_scrolled() {
        let cell = |x: i32, y: i32| Vec2::new(8.0 + 32.0 * x as f32 + 3.0, 136.0 + 32.0 * y as f32 + 3.0);
        assert_eq!(spot_at(cell(0, 0), Some(LOOT_SIDE), 0), Some(Spot::Pile(0)));
        assert_eq!(spot_at(cell(9, 7), Some(LOOT_SIDE), 2), Some(Spot::Pile(99)), "row 7 shown is row 9 two rows down");
        assert_eq!(spot_at(cell(0, 8), Some(LOOT_SIDE), 0), None, "eight rows shown");
        assert_eq!(spot_at(Vec2::new(30.0, 200.0), None, 0), Some(Spot::Pc(Slot::Weapon as i32)), "the paperdoll without a pile");
        assert_eq!(spot_at(Vec2::new(370.0, 10.0), Some(LOOT_SIDE), 0), Some(Spot::Pc(0)));
        // Barter: the NPC's grid from (8, 168), seven rows.
        assert_eq!(spot_at(Vec2::new(10.0, 170.0), Some(BARTER_SIDE), 0), Some(Spot::Pile(0)));
        assert_eq!(spot_at(Vec2::new(10.0, 170.0 + 7.0 * 32.0), Some(BARTER_SIDE), 0), None, "seven rows shown");
        assert_eq!(spot_at(Vec2::new(10.0, 140.0), Some(BARTER_SIDE), 0), None, "the text line, not the grid");
        assert_eq!(pile_box(23, (1, 2), 1, PILE_GRID).min, Vec2::new(8.0 + 96.0, 136.0 + 32.0));
        let pile = Inventory { items: vec![CarriedItem { prototype: 1, location: 12, quantity: 0, keys: vec![], written: 0, wear: Default::default() }], ..Inventory::container() };
        assert!(pile_item_at(&pile, 32, sword).is_some(), "a 1x3 sword from row 1 to 3");
    }

    #[test]
    fn moves_show_at_once_and_the_hosts_answers_win() {
        let man = Wearer { race: 0, gender: 1 };
        let mut inv = PcInventory::default();
        assert!(inv.request(InventoryRequest::Move { from: 0, to: 1 }, man, sword).is_err(), "nothing before the host's copy");
        inv.receive(0, Inventory { items: vec![CarriedItem { prototype: 1, location: 0, quantity: 0, keys: vec![], written: 0, wear: Default::default() }], ..Default::default() });
        inv.settle(man, sword);
        inv.request(InventoryRequest::Move { from: 0, to: 5 }, man, sword).unwrap();
        inv.request(InventoryRequest::Move { from: 5, to: 7 }, man, sword).unwrap();
        assert_eq!(inv.shown().unwrap().items[0].location, 7);
        assert_eq!(inv.take_outgoing().len(), 2);
        // The host took the first; the second is replayed on its copy.
        inv.receive(1, Inventory { items: vec![CarriedItem { prototype: 1, location: 5, quantity: 0, keys: vec![], written: 0, wear: Default::default() }], ..Default::default() });
        inv.settle(man, sword);
        assert_eq!(inv.shown().unwrap().items[0].location, 7);
        // The host refused the second: its copy wins.
        inv.receive(2, Inventory { items: vec![CarriedItem { prototype: 1, location: 5, quantity: 0, keys: vec![], written: 0, wear: Default::default() }], ..Default::default() });
        inv.settle(man, sword);
        assert_eq!(inv.shown().unwrap().items[0].location, 5);
        assert_eq!(inv.request(InventoryRequest::Move { from: 5, to: 1001 }, man, sword), Err(ItemRefusal::WrongSlot));
    }

    #[test]
    fn items_sit_centred_in_their_footprint() {
        let item = CarriedItem { prototype: 1, location: 12, quantity: 0, keys: vec![], written: 0, wear: Default::default() };
        let inside = placement(&item, (2, 3)).unwrap();
        assert_eq!(inside, Rect::from_corners(Vec2::new(432.0, 40.0), Vec2::new(496.0, 136.0)));
        assert_eq!(centred(inside, UVec2::new(41, 90), true), Vec2::new(443.0, 43.0));
        let sword = CarriedItem { prototype: 1, location: Slot::Weapon as i32, quantity: 0, keys: vec![], written: 0, wear: Default::default() };
        assert_eq!(placement(&sword, (1, 1)).unwrap().min, Vec2::new(23.0, 170.0));
        assert_eq!(centred(placement(&sword, (1, 1)).unwrap(), UVec2::new(101, 128), false), Vec2::new(20.0, 170.0), "an arithmetic shift rounds down");
    }

    #[test]
    fn the_first_barter_view_opens_the_window_and_gold_is_not_shown() {
        use arcanum_sim::barter::{BarterView, Offer};
        let mut stock = Inventory::default();
        stock.change_gold(40);
        stock.items.push(CarriedItem { prototype: 6049, location: 3, quantity: 0, keys: vec![], written: 0, wear: Default::default() });
        let view = BarterView { npc: arcanum_sim::NetId(4), npc_male: true, npc_name: "Bates".into(), stock, buy: [(3, Offer { price: 12, line: "I will sell that for 12 coins.".into() })].into(), sell: Default::default(), npc_gold: 40, pickpocket: false };
        let mut inventory = PcInventory::default();
        inventory.barter_view(view.clone());
        assert!(inventory.barter_opened);
        assert_eq!(inventory.barter_goods().unwrap().items.iter().map(|i| i.location).collect::<Vec<_>>(), [3], "the NPC's gold is not in its grid");
        inventory.barter_opened = false;
        inventory.barter_refused("No.".into());
        inventory.barter_view(view);
        assert!(!inventory.barter_opened, "a later view only updates it");
        assert_eq!(inventory.barter_line.as_deref(), Some("No."));
        let panel = InventoryPanel { open: true, pile: None, barter: true };
        assert!(panel.two_sided() && panel.side() == Some(BARTER_SIDE));
        inventory.left_map();
        assert!(inventory.barter().is_none() && inventory.left_map, "another map: the window closes");
    }

    #[test]
    fn written_items_open_their_page_and_schematics_do_not() {
        use arcanum_formats::object::{Object, TYPE_WEAPON, TYPE_WRITTEN, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let make = |number: u32, kind: u32, subtype: i32| {
            let b = ObjectBuilder::new(kind);
            let b = if kind == TYPE_WRITTEN { b.field("written_subtype", Value::Int32(subtype)).field("written_text_start_line", Value::Int32(1010)) } else { b };
            let mut o = Object::parse_instance(&b.build()).unwrap();
            o.id = prototype_id(number);
            o
        };
        let mut prototypes = Prototypes::default();
        prototypes.insert(make(9000, TYPE_WRITTEN, 0));
        prototypes.insert(make(9001, TYPE_WRITTEN, 5));
        prototypes.insert(make(9002, TYPE_WEAPON, 0));
        let item = |prototype, written| CarriedItem { prototype, location: 0, quantity: 0, keys: vec![], written, wear: Default::default() };
        assert_eq!(written_page(&item(9000, 0), &prototypes), Some((0, 1010)), "a book from its prototype's line 1010");
        assert_eq!(written_page(&item(9000, 1500), &prototypes), Some((0, 1500)), "its own line first");
        assert_eq!(written_page(&item(9001, 0), &prototypes), None, "a schematic");
        assert_eq!(written_page(&item(9002, 0), &prototypes), None);
    }
}
