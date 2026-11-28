//! The examine script (event 0, Arcanum.exe 0x57cd60): as the player's PC's cursor comes to rest
//! on an object (0x553a70 → 0x57ccf0), the object's examine script runs, the PC the triggerer
//! and the object the attachee (0x57cd93), before its name is shown; "skip the default" shows
//! nothing (0x57cd9b). The hover is drawn on the client, so an object with an examine script
//! asks the host first ([`crate::PlayerIntent::Examine`]) and shows its name only on the host's
//! answer ([`crate::LedgerDelta::Examined`]). Not built: the other windows that examine (the
//! inventory's hover, 0x573cea; the key 0x54dc80), dropped items (their prototype's scripts).

use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{LedgerDelta, LoadedMap, NetId, Outbox, PlayerId, ScriptObject, WorldState};

/// Script event 0: examine.
pub const EVENT_EXAMINE: u32 = 0;

/// What a player's PC examines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Examined {
    /// An object of a sector file.
    Object(crate::MapObjectId),
    /// A container or an item on the ground.
    Ground(crate::GroundItemId),
    /// An NPC.
    Critter(NetId),
}

/// Players' examinations waiting for their scripts: the player, the object, the client's number.
#[derive(Resource, Debug, Default)]
pub struct ExamineRequests(pub Vec<(PlayerId, Examined, u32)>);

/// Runs the examine scripts asked for and answers whether the name shows.
pub fn run_examine_scripts(
    mut requests: ResMut<ExamineRequests>,
    mut host: crate::ScriptHost,
    mut world: ResMut<WorldState>,
    mut map: ResMut<LoadedMap>,
    mut outbox: ResMut<Outbox>,
    npcs: Query<(&NetId, &crate::NpcSource)>,
) {
    for (player, target, seq) in std::mem::take(&mut requests.0) {
        let object = match target {
            Examined::Object(id) => Some(ScriptObject::Map(id)),
            Examined::Ground(id) => ScriptObject::ground(id),
            Examined::Critter(net) => npcs.iter().find(|n| *n.0 == net).map(|n| n.1.object()),
        };
        let shown = object.is_none_or(|o| host.hook_by(&mut world, &mut map, &mut outbox, o, EVENT_EXAMINE, player, ScriptObject::Pc(player)) == crate::HookResult::RunDefault);
        outbox.0.push((player, LedgerDelta::Examined { seq, shown }));
    }
}

