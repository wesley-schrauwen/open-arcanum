//! The bottom bar's ten hotkey slots (Arcanum.exe 0x57e140 / 0x57e8d0 / 0x54fcf0 / 0x57f1d0).
//!
//! - **Filling:** an item dragged from the inventory screen onto a slot moves to location
//!   2000 + slot ([`arcanum_sim::HOTKEYS`], through the host): it stays on the PC and keeps its
//!   weight, but leaves the grid. A slot already holding an item swaps with it: slot to slot,
//!   or the old one goes back where the dragged item was. With the inventory open, a slot's item
//!   can be dragged back into the grid, onto a paperdoll slot or onto another hotkey slot
//!   ([`crate::inventory_screen`]).
//! - **Using:** a left click on a slot with the inventory closed, or keys 1-9 and 0 (0x54de50,
//!   DIK 2-11): a lockpick arms Pick Locks with it ([`crate::skill_use`]); a weapon or armour is
//!   put on (0x550000). Other items do nothing yet: the original holds them on the cursor to use
//!   on what is clicked next (0x573740, 0x434f80).
//! - **Drawing:** the item's inventory art, centred, shrunk to the 32x32 slot when larger.
//! - **Spells** (type 3): dragged from the spells view ([`crate::spell_bar`]) onto a slot
//!   (`InventoryRequest::HotkeySpell`; an item there goes back into the grid); kept in
//!   [`arcanum_sim::Inventory::spells`]. Using the slot arms the spell. With the inventory open
//!   it drags to another slot, swapping; off the bar it stays (0x57e8d0).
//!
//! Not built: skills on the bar (types 2 and 4), the dragged spell's icon at the cursor, the two active slots and the A key, the
//! right-click action (0x550150), the stack count, the hover help, using other items.

use bevy::prelude::*;

use arcanum_formats::object::Prototypes;
use arcanum_sim::{HOTKEYS, InventoryRequest};

use crate::art_loader::ArtFrames;
use crate::hud::{HOTKEY_X, HOTKEY_Y};
use crate::inventory_screen::{HeldItem, InventoryPanel, ItemArtCache, PcInventory, art_frame, client_info, inventory_sizes, item_art, screen_point, wearer};
use crate::main_menu::AppState;
use crate::menu_window::{at, rect};

/// A hotkey slot's art on the bottom bar, by slot (0 is key 1, 9 is key 0).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeySlot(pub usize);

/// An item drawn in a hotkey slot, for tests.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyItem(pub usize);

pub struct HotkeyBarPlugin;

impl Plugin for HotkeyBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (use_hotkeys, show_hotkeys).chain().run_if(in_state(AppState::InGame)));
    }
}

/// The inventory location of hotkey slot `slot`.
pub fn location(slot: usize) -> i32 {
    HOTKEYS.start() + slot as i32
}

/// The hotkey slot at `point` (800x600 screen pixels), if any.
pub fn slot_at(point: Vec2) -> Option<usize> {
    if !(HOTKEY_Y..HOTKEY_Y + 32.0).contains(&point.y) {
        return None;
    }
    HOTKEY_X.iter().position(|&x| (x..x + 32.0).contains(&point.x))
}

/// The slot keys, 1-9 then 0.
const KEYS: [KeyCode; 10] = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9, KeyCode::Digit0];

/// What using a slot's item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotUse {
    /// Arms Pick Locks with the lockpick there.
    PickLocks,
    /// Puts the item on in this paperdoll slot.
    Wear(i32),
}

/// What using `prototype` from the bar does (0x54fcf0), if anything is built for it.
pub fn slot_use(prototype: u32, prototypes: &Prototypes, info: impl Fn(u32) -> Option<arcanum_sim::ItemInfo>) -> Option<SlotUse> {
    if arcanum_sim::is_lockpick(prototype, prototypes) {
        return Some(SlotUse::PickLocks);
    }
    let item = info(prototype)?;
    matches!(item.kind, arcanum_formats::object::TYPE_WEAPON | arcanum_formats::object::TYPE_ARMOR).then(|| item.slot().map(|s| SlotUse::Wear(s as i32))).flatten()
}

/// A click on a slot (the inventory closed) or its key uses the item there.
#[allow(clippy::too_many_arguments)]
fn use_hotkeys(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    panel: Res<InventoryPanel>,
    held: Res<HeldItem>,
    mut inventory: ResMut<PcInventory>,
    sheet: Res<crate::pc_sheet::SheetHost>,
    client: Option<Res<crate::world_render::ClientMap>>,
    (mut cache, assets, frames): (ResMut<ItemArtCache>, Res<AssetServer>, Res<Assets<ArtFrames>>),
    armed: Option<ResMut<crate::skill_use::SkillTargeting>>,
) {
    let clicked = (buttons.just_pressed(MouseButton::Left) && !panel.open && held.0.is_none())
        .then(|| windows.iter().find_map(|w| w.cursor_position().map(|c| screen_point(w.size(), c))))
        .flatten()
        .and_then(slot_at);
    let Some(slot) = clicked.or_else(|| KEYS.iter().position(|k| keys.just_pressed(*k))) else { return };
    let Some(inv) = inventory.shown().cloned() else { return };
    if let Some(&spell) = inv.spells.get(&location(slot)) {
        // A spell is cast as from the spells view (0x54fcf0 type 3 → 0x57bc70).
        if let Some(mut armed) = armed {
            armed.0 = Some(crate::skill_use::Armed::Spell(spell));
        }
        return;
    }
    let Some(item) = inv.items.iter().find(|i| i.location == location(slot)) else { return };
    let empty = Prototypes::default();
    let prototypes = client.as_ref().map_or(&empty, |c| &c.map.prototypes);
    let sizes = inventory_sizes(&inv, prototypes, client.as_ref().and_then(|c| c.art_index()), &mut cache, &assets, &frames);
    let info = client_info(prototypes, &sizes);
    match slot_use(item.prototype, prototypes, &info) {
        Some(SlotUse::PickLocks) => {
            if let Some(mut armed) = armed {
                armed.0 = Some(crate::skill_use::Armed::Skill { skill: arcanum_sim::SKILL_PICK_LOCKS, tool: Some(item.location) });
            }
        }
        Some(SlotUse::Wear(to)) => {
            let _ = inventory.request(InventoryRequest::Move { from: item.location, to }, wearer(&sheet), &info);
        }
        None => {}
    }
}

/// Draws each slot's item over the slot art, except one being dragged.
#[allow(clippy::too_many_arguments)]
fn show_hotkeys(
    mut commands: Commands,
    inventory: Res<PcInventory>,
    held: Res<HeldItem>,
    client: Option<Res<crate::world_render::ClientMap>>,
    slots: Query<(Entity, &HotkeySlot)>,
    drawn: Query<Entity, With<HotkeyItem>>,
    added: Query<(), Added<HotkeySlot>>,
    (mut cache, assets, frames, mut pending): (ResMut<ItemArtCache>, Res<AssetServer>, Res<Assets<ArtFrames>>, Local<bool>),
) {
    if !inventory.is_changed() && !held.is_changed() && added.is_empty() && !*pending {
        return;
    }
    *pending = false;
    for e in &drawn {
        commands.entity(e).despawn();
    }
    let Some(inv) = inventory.shown() else { return };
    let empty = Prototypes::default();
    let prototypes = client.as_ref().map_or(&empty, |c| &c.map.prototypes);
    let index = client.as_ref().and_then(|c| c.art_index());
    let lifted = held.0.filter(|h| !h.from_pile).map(|h| h.from);
    for (entity, slot) in &slots {
        if let Some(&spell) = inv.spells.get(&location(slot.0)).filter(|_| Some(location(slot.0)) != lifted) {
            let art = commands.spawn((HotkeyItem(slot.0), bevy::picking::Pickable::IGNORE, ImageNode::new(assets.load(crate::spell_bar::spell_icon(spell))), at(rect(Vec2::ZERO, Vec2::splat(32.0))))).id();
            commands.entity(entity).add_child(art);
            continue;
        }
        let Some(item) = inv.items.iter().find(|i| i.location == location(slot.0) && Some(i.location) != lifted) else { continue };
        let Some(id) = item_art(prototypes, item) else { continue };
        let Some((image, size)) = art_frame(index, &mut cache, &assets, &frames, id) else {
            *pending = true;
            continue;
        };
        let (corner, shown) = fit(size);
        let art = commands.spawn((HotkeyItem(slot.0), bevy::picking::Pickable::IGNORE, crate::inventory_screen::item_image(image, &item.wear), at(rect(corner, shown)))).id();
        commands.entity(entity).add_child(art);
    }
}

/// Where art of `size` goes in a 32x32 slot and how big: centred, shrunk to fit when larger.
pub fn fit(size: UVec2) -> (Vec2, Vec2) {
    let size = size.as_vec2();
    let scale = (32.0 / size.x.max(size.y)).min(1.0);
    let shown = (size * scale).floor();
    (((Vec2::splat(32.0) - shown) / 2.0).floor(), shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_found_under_the_cursor() {
        assert_eq!(slot_at(Vec2::new(198.0, 445.0)), Some(0));
        assert_eq!(slot_at(Vec2::new(604.0, 476.0)), Some(9));
        assert_eq!(slot_at(Vec2::new(232.0, 450.0)), None, "between slots");
        assert_eq!(slot_at(Vec2::new(200.0, 480.0)), None, "below");
        assert_eq!(location(9), 2009);
    }

    #[test]
    fn art_is_centred_and_shrunk_to_the_slot() {
        assert_eq!(fit(UVec2::new(20, 10)), (Vec2::new(6.0, 11.0), Vec2::new(20.0, 10.0)));
        assert_eq!(fit(UVec2::new(64, 96)), (Vec2::new(5.0, 0.0), Vec2::new(21.0, 32.0)));
    }
}
