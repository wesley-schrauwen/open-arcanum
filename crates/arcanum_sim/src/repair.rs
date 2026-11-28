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
//! - **Repair** ([`repair`]): on a carried item that lost hit points (else `skill_ui.mes` 505
//!   "not in need of repair"). No roll: with a chance ([`crate::tech_chance`]) of 1 or more it
//!   restores `max × chance / 100`, at least 1, and the most falls by 10 / 5 / 1 / 0 % by training
//!   (none, Apprentice, Expert, Master); 500 then; with no chance it fails, 501.
//!
//! Not built: a Master mending broken items ([`ItemWear::broken`], 5 %; 0x4c7d6f clears the bit), the critical failure at Master
//! (1 %), effect 9 (everything restored), repairing items on the ground or other critters',
//! critical misses wearing the attacker's weapon (0x4c82e0), items immune to wear (flag
//! 0x400000).

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

/// Repairs `wear` of an item of `max` hit points with `chance` at `training`: the new wear and
/// the `skill_ui.mes` line, or the line why not.
pub fn repair(wear: ItemWear, max: i32, chance: i32, training: u8) -> Result<(ItemWear, u32), u32> {
    if wear.damage < 1 {
        return Err(NOT_DAMAGED);
    }
    if chance < 1 {
        return Err(REPAIR_FAILED);
    }
    let amount = (max * chance / 100).max(1);
    let lost = max * REPAIR_LOSS[usize::from(training.min(3))] / 100;
    Ok((ItemWear { damage: (wear.damage - amount).max(0), lost: wear.lost + lost, ..wear }, REPAIRED))
}

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
        assert_eq!(repair(worn, 100, 0, 3), Err(REPAIR_FAILED));
        assert_eq!(repair(worn, 100, 20, 0), Ok((ItemWear { damage: 10, lost: 10, broken: false }, REPAIRED)));
        assert_eq!(repair(worn, 100, 50, 1), Ok((ItemWear { damage: 0, lost: 5, broken: false }, REPAIRED)));
        assert_eq!(repair(worn, 100, 50, 3), Ok((ItemWear { damage: 0, lost: 0, broken: false }, REPAIRED)));
        assert_eq!(repair(ItemWear { damage: 5, lost: 0, broken: false }, 10, 4, 2), Ok((ItemWear { damage: 4, lost: 0, broken: false }, REPAIRED)), "at least 1");
    }
}
