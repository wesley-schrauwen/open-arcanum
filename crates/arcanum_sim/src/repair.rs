//! Items' hit points and the Repair skill (Arcanum.exe: item hit points 0x43d5a0 / 0x43d600,
//! combat wear 0x4b4390, Repair 0x4c7160 case 0xc at 0x4c7bce, tech chance 0x4c69f0).
//!
//! - **Hit points:** an item's most is `hp_pts + hp_adj` (fields 0x1b, 0x1c), less what repairs
//!   took ([`ItemWear::lost`]); what it lost is `hp_damage` (0x1d, [`ItemWear::damage`]).
//!   Carried and dropped items keep their wear.
//! - **Wear in a fight** ([`wear_item`], 0x4b4390): a critical hit's armour or weapon damage takes
//!   [`COMBAT_WEAR`] hit points; at 0 left it breaks (0x43f1c0 → 0x43f4cd: art bit 0x400,
//!   [`ItemWear::broken`]): its damage is reset to 0, it comes off into the pack (0x464c80) or,
//!   with no room, falls to the ground (0x461a70), and its bust script (event 30) runs. A broken
//!   item worn to nothing again is destroyed (0x43f44a → 0x43cca0, its destroy script first).
//!   Items with no hit points are left alone (builder's call). Nothing else is known to weaken
//!   a worn item.
//! - **Repair** ([`repair`]): on a carried item that lost hit points, or a broken one in a
//!   Master's hands (else `skill_ui.mes` 505 "not in need of repair"). No roll: with a chance
//!   ([`crate::tech_chance`]) of 1 or more it restores `max × chance / 100`, at least 1 (500);
//!   with no chance nothing (501). Below Master the most falls by 10 / 5 / 1 % by training
//!   (none, Apprentice, Expert) either way; a Master loses nothing, and mends a broken item for 5 %
//!   of its most (500). Below Master a broken item stays broken.
//!
//! Not built: the critical failure at Master (1 % of the most when no chance, 0x4c7d9f; 0x4c82e0
//! not decoded), effect 9 (everything restored), repairing items on the ground or other critters',
//! critical misses wearing the attacker's weapon (0x4c82e0).

use arcanum_formats::object::Prototypes;

use crate::{CarriedItem, Inventory, ItemWear};

/// The Repair skill.
pub const SKILL_REPAIR: usize = 12;
/// Hit points a critical hit's wear takes from an item (0x4b4390).
pub const COMBAT_WEAR: i32 = 10;
/// `mes/skill_ui.mes`: repaired, failed, not in need of repair.
pub const REPAIRED: u32 = 500;
pub const REPAIR_FAILED: u32 = 501;
pub const NOT_DAMAGED: u32 = 505;
/// The most hit points a repair takes, in percent, by training (0x4c7bce's table).
pub const REPAIR_LOSS: [i32; 4] = [10, 5, 1, 0];

/// An item's most hit points now: `hp_pts + hp_adj`, less what repairs took.
pub fn item_max_hp(item: &CarriedItem, prototypes: &Prototypes) -> i32 {
    let Some(o) = prototypes.get(item.prototype) else { return 0 };
    prototypes.int(o, "hp_pts").unwrap_or(0) + prototypes.int(o, "hp_adj").unwrap_or(0) - item.wear.lost
}

/// What combat wear did to an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wear {
    /// It lost hit points.
    Worn,
    /// Worn to nothing, it broke (still where it was; the caller takes it off).
    Broke,
    /// A broken item worn to nothing: to be destroyed (left where it is for its script).
    Gone,
}

/// Takes [`COMBAT_WEAR`] hit points from the item at `location` (0x4b4390): `None` without one
/// there or with no hit points (builder's call). At 0 left an item breaks, its damage reset
/// (0x43f522); a broken one is to be destroyed.
pub fn wear_item(inventory: &mut Inventory, location: i32, prototypes: &Prototypes) -> Option<Wear> {
    let i = inventory.items.iter().position(|i| i.location == location)?;
    let max = item_max_hp(&inventory.items[i], prototypes);
    if max <= 0 {
        return None;
    }
    let wear = &mut inventory.items[i].wear;
    wear.damage += COMBAT_WEAR;
    if wear.damage < max {
        return Some(Wear::Worn);
    }
    if wear.broken {
        return Some(Wear::Gone);
    }
    wear.broken = true;
    wear.damage = 0;
    Some(Wear::Broke)
}

/// Repairs `wear` of an item of `max` hit points with `chance` at `training` (0x4c7160 case
/// 0xc): the new wear and the `skill_ui.mes` line, or [`NOT_DAMAGED`] when there is nothing to
/// do (no damage, and not a broken item in a Master's hands, 0x4c7c55-0x4c7c82). It restores
/// `max × chance / 100`, at least 1, none with no chance (then [`REPAIR_FAILED`],
/// 0x4c7cf8-0x4c7d1e). Below Master the most falls by [`REPAIR_LOSS`] whether it worked or not
/// (0x4c7daa); a Master loses nothing, but mends a broken item for 5 % of its most
/// (0x4c7d52-0x4c7d83), which always counts as repaired.
pub fn repair(wear: ItemWear, max: i32, chance: i32, training: u8) -> Result<(ItemWear, u32), u32> {
    let master = training >= 3;
    if wear.damage < 1 && !(wear.broken && master) {
        return Err(NOT_DAMAGED);
    }
    let mut repaired = true;
    let mut amount = max * chance / 100;
    if amount < 1 {
        if chance < 1 {
            amount = 0;
            repaired = false;
        } else {
            amount = 1;
        }
    }
    let mut out = ItemWear { damage: (wear.damage - amount).max(0), ..wear };
    let loss = if master {
        if wear.broken {
            out.broken = false;
            repaired = true;
            MASTER_MEND_LOSS
        } else {
            0
        }
    } else {
        REPAIR_LOSS[usize::from(training)]
    };
    out.lost += max * loss / 100;
    Ok((out, if repaired { REPAIRED } else { REPAIR_FAILED }))
}

/// The most hit points a Master's mending of a broken item takes, in percent (0x4c7d62).
pub const MASTER_MEND_LOSS: i32 = 5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combat_wear_breaks_an_item_then_destroys_it() {
        use arcanum_formats::object::{Object, TYPE_WEAPON, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let mut sword = Object::parse_instance(&ObjectBuilder::new(TYPE_WEAPON).field("hp_pts", Value::Int32(15)).build()).unwrap();
        sword.id = prototype_id(6049);
        let mut prototypes = Prototypes::default();
        prototypes.insert(sword);
        let mut inv = Inventory { items: vec![CarriedItem { prototype: 6049, location: 1004, quantity: 0, keys: vec![], written: 0, wear: Default::default() }], ..Default::default() };
        let wear = |inv: &Inventory| inv.items[0].wear;
        assert_eq!(wear_item(&mut inv, 1004, &prototypes), Some(Wear::Worn));
        assert_eq!(wear_item(&mut inv, 1004, &prototypes), Some(Wear::Broke), "20 of 15");
        assert_eq!(wear(&inv), ItemWear { damage: 0, lost: 0, broken: true }, "damage reset (0x43f522), art bit 0x400");
        assert_eq!(wear_item(&mut inv, 1004, &prototypes), Some(Wear::Worn));
        assert_eq!(wear_item(&mut inv, 1004, &prototypes), Some(Wear::Gone), "broken, to be destroyed (0x43f4df)");
        assert_eq!(inv.items.len(), 1, "left for its destroy script");
        assert_eq!(wear_item(&mut inv, 1005, &prototypes), None);
    }

    #[test]
    fn repairs_restore_by_the_chance_and_cost_by_training() {
        let worn = ItemWear { damage: 30, lost: 0, broken: false };
        assert_eq!(repair(ItemWear::default(), 100, 50, 0), Err(NOT_DAMAGED));
        assert_eq!(repair(worn, 100, 0, 3), Ok((worn, REPAIR_FAILED)), "no chance: nothing restored");
        assert_eq!(repair(worn, 100, 0, 0), Ok((ItemWear { damage: 30, lost: 10, broken: false }, REPAIR_FAILED)), "the most falls all the same (0x4c7daa)");
        assert_eq!(repair(worn, 100, 20, 0), Ok((ItemWear { damage: 10, lost: 10, broken: false }, REPAIRED)));
        assert_eq!(repair(worn, 100, 50, 1), Ok((ItemWear { damage: 0, lost: 5, broken: false }, REPAIRED)));
        assert_eq!(repair(worn, 100, 50, 3), Ok((ItemWear { damage: 0, lost: 0, broken: false }, REPAIRED)));
        assert_eq!(repair(ItemWear { damage: 5, lost: 0, broken: false }, 10, 4, 2), Ok((ItemWear { damage: 4, lost: 0, broken: false }, REPAIRED)), "at least 1");
        let broken = ItemWear { damage: 0, lost: 0, broken: true };
        assert_eq!(repair(broken, 100, 50, 2), Err(NOT_DAMAGED), "below Master a broken item is not mended (0x4c7c7d)");
        assert_eq!(repair(broken, 100, 50, 3), Ok((ItemWear { damage: 0, lost: 5, broken: false }, REPAIRED)), "a Master mends it for 5 % (0x4c7d62)");
        assert_eq!(repair(broken, 100, 0, 3), Ok((ItemWear { damage: 0, lost: 5, broken: false }, REPAIRED)), "even with no chance");
        assert_eq!(repair(ItemWear { damage: 30, ..broken }, 100, 50, 1), Ok((ItemWear { damage: 0, lost: 5, broken: true }, REPAIRED)), "damage restored, still broken");
    }
}
