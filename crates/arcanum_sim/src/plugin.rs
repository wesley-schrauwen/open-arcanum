//! The headless host app and its fixed-rate tick.
//!
//! [`SimApp::new`] builds a Bevy [`App`] with only time and the [`SimPlugin`]: no window,
//! renderer or audio. Whoever owns it calls [`SimApp::update`]; in single player that is a
//! client system, in a dedicated host a loop. Game logic runs in `FixedUpdate` at [`TICK_HZ`].

use std::time::Duration;

use bevy_app::{App, FixedUpdate, Plugin};
use bevy_ecs::prelude::*;
use arcanum_rules::RulesData;
use bevy_time::{Fixed, Time, TimePlugin, TimeUpdateStrategy};

use crate::{Inbox, LoadedMap, MoveRequests, NetIdAllocator, Outbox, SaveStatus, Session, SimRules, SyncClock, Tick, GameClock, advance_clock, WorldSave, WorldState, apply_moves, boot_world, handle_intents, spawn_npcs, stream_sectors, update_activity, PartyPace, update_party_pace, HostRng, npc_idle, sync_state, walk, write_save, PopulatedSectors, UseRequests, ClosingDoors, use_portals, close_doors, SheetRequests, apply_sheet_requests, InventoryRequests, apply_inventory_requests, PickUpRequests, InventoryAcks, pick_up_items, PickLockRequests, pick_locks, CastRequests, cast_spells, recover_fatigue, gate_unconscious, update_consciousness, RestRequests, StopRestRequests, WaitingTowns, start_resting, rest_steps};

/// Host ticks per second.
pub const TICK_HZ: f64 = 30.0;

/// The number of the last finished tick (0 before the first).
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SimTick(pub Tick);

/// The host's [`NetIdAllocator`].
#[derive(Resource, Debug, Default)]
pub struct NetIds(pub NetIdAllocator);

/// Host resources, the session, saving and the tick counter. Optional: [`StartMap`](crate::StartMap)
/// (without it accepted characters have no world to join), [`SaveFile`](crate::SaveFile) and
/// [`SimVfs`](crate::SimVfs) (without it no map is read).
pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_hz(TICK_HZ))
            .init_resource::<SimTick>()
            .init_resource::<NetIds>()
            .init_resource::<SimRules>()
            .init_resource::<Inbox>()
            .init_resource::<Outbox>()
            .init_resource::<Session>()
            .init_resource::<WorldState>()
            .init_resource::<SaveStatus>()
            .init_resource::<LoadedMap>()
            .init_resource::<MoveRequests>()
            .init_resource::<SyncClock>()
            .init_resource::<PopulatedSectors>()
            .init_resource::<crate::world::ToggledNpcs>()
            .init_resource::<GameClock>()
            .init_resource::<PartyPace>()
            .init_resource::<HostRng>()
            .init_resource::<UseRequests>()
            .init_resource::<SheetRequests>()
            .init_resource::<InventoryRequests>()
            .init_resource::<PickUpRequests>()
            .init_resource::<PickLockRequests>()
            .init_resource::<crate::lockpick::LockWitnessing>()
            .init_resource::<crate::dialogue::KnownSent>()
            .init_resource::<crate::script_host::CarriedSeen>()
            .init_resource::<crate::DisarmRequests>()
            .init_resource::<CastRequests>()
            .init_resource::<RestRequests>()
            .init_resource::<StopRestRequests>()
            .init_resource::<WaitingTowns>()
            .init_resource::<crate::BedRequests>()
            .init_resource::<crate::StartedDialogues>()
            .init_resource::<crate::TalkRequests>()
            .init_resource::<crate::barter::BarterStarts>()
            .init_resource::<crate::big_move::ScriptTeleports>()
            .init_resource::<crate::script_host::ScriptOrders>()
            .init_resource::<crate::pickpocket::PickPocketStarts>()
            .init_resource::<crate::pickpocket::CaughtThieves>()
            .init_resource::<crate::prowl::ProwlRequests>()
            .init_resource::<crate::combat::Combat>()
            .init_resource::<crate::combat::CombatRequests>()
            .init_resource::<crate::combat::AiParams>()
            .init_resource::<crate::combat::CombatStarts>()
            .init_resource::<crate::unconscious::Resurrections>()
            .init_resource::<crate::combat::CombatEvents>()
            .init_resource::<crate::magick::Resurrecting>()
            .init_resource::<crate::combat::ScriptChecks>()
            .init_resource::<crate::big_move::TeleportReports>()
            .init_resource::<crate::big_move::BigMoves>()
            .init_resource::<crate::big_move::VoteRequests>()
            .init_resource::<crate::big_move::MapIds>()
            .init_resource::<crate::big_move::LastTiles>()
            .init_resource::<crate::trap::SteppedOnTraps>()
            .init_resource::<crate::script_host::ScriptInventories>()
            .init_resource::<crate::script_host::ScriptSheets>()
            .init_resource::<crate::ExperienceAwards>()
            .init_resource::<crate::FateRequests>()
            .init_resource::<crate::heartbeat::Heartbeats>()
            .init_resource::<crate::script_host::TimedScripts>()
            .init_resource::<crate::areas::AreaTable>()
            .init_resource::<crate::barter::BarterRequests>()
            .init_resource::<crate::DialogueReports>()
            .init_resource::<crate::GenericLines>()
            .init_resource::<crate::ReputationTable>()
            .init_resource::<crate::Scripts>()
            .init_resource::<crate::ScriptReports>()
            .init_resource::<InventoryAcks>()
            .init_resource::<ClosingDoors>()
            .add_systems(FixedUpdate, ((handle_intents, gate_unconscious).chain(), boot_world, (apply_sheet_requests, crate::apply_fate).chain(), apply_inventory_requests, crate::barter::apply_barter, update_party_pace, advance_clock, apply_moves, (crate::trap::spot_traps_walking, walk, crate::trap::spring_stepped_traps, crate::big_move::step_on_jump_points).chain(), use_portals, pick_up_items, (pick_locks, crate::trap::disarm_traps, cast_spells, (crate::magick::cast_on_critters, crate::magick::resurrect_npcs).chain(), start_resting, rest_steps, recover_fatigue, update_consciousness, crate::talk_to_npcs, crate::run_conversations, crate::unconscious::resurrect_pcs, crate::share_experience, crate::barter::open_barters, crate::script_host::run_timed_scripts, crate::big_move::run_big_moves, crate::script_host::run_wield_scripts, crate::script_host::run_carry_scripts, crate::script_host::sync_script_inventories, crate::script_host::sync_script_sheets).chain(), close_doors, stream_sectors, spawn_npcs, update_activity, (crate::heartbeat::run_heartbeats, npc_idle, crate::follow::expire_waits, crate::follow::recheck_followers, crate::follow::sync_followers, crate::follow::follow_leaders, crate::dialogue::send_known_npcs, crate::newspaper::advance_newspapers, crate::script_host::apply_script_orders, crate::pickpocket::catch_thieves, crate::prowl::apply_prowling, crate::combat::notice_foes, crate::combat::catch_lock_pickers, crate::combat::rearm_npcs, crate::combat::run_combat, crate::combat::run_combat_events, crate::combat::recover_surrendered, crate::combat::recover_npc_fatigue, crate::poison::run_poison, (crate::combat::send_npc_health, crate::combat::update_npc_paces, crate::combat::get_up_knocked_down, crate::light::update_borne_lights, crate::combat::update_weapon_art)).chain(), sync_state, advance_tick, write_save).chain());
    }
}

fn advance_tick(mut tick: ResMut<SimTick>) {
    tick.0 += 1;
}

/// A headless host.
pub struct SimApp {
    pub app: App,
}

impl SimApp {
    pub fn new(rules: RulesData) -> Self {
        let mut app = App::new();
        app.add_plugins((TimePlugin, SimPlugin)).insert_resource(SimRules(rules));
        Self { app }
    }

    /// Makes every [`update`](Self::update) after the first run exactly one tick, whatever the
    /// wall clock says. For tests.
    pub fn use_manual_clock(&mut self) {
        self.app.insert_resource(TimeUpdateStrategy::ManualDuration(Self::tick_duration()));
    }

    /// Continues `save` (a loaded game or a joined session): its PCs spawn on the next tick.
    pub fn boot(&mut self, save: WorldSave) {
        self.app.world_mut().resource_mut::<SimTick>().0 = save.tick;
        self.app.insert_resource(WorldState(Some(save)));
    }

    /// Queues intents for the next tick.
    pub fn receive(&mut self, intents: impl IntoIterator<Item = (crate::PlayerId, crate::PlayerIntent)>) {
        self.app.world_mut().resource_mut::<Inbox>().0.extend(intents);
    }

    /// Takes the deltas written since the last call.
    pub fn take_deltas(&mut self) -> Vec<(crate::PlayerId, crate::LedgerDelta)> {
        std::mem::take(&mut self.app.world_mut().resource_mut::<Outbox>().0)
    }

    /// Runs one frame: as many fixed ticks as the elapsed time allows.
    pub fn update(&mut self) {
        self.app.update();
    }

    pub fn tick(&self) -> Tick {
        self.app.world().resource::<SimTick>().0
    }

    /// The length of one tick.
    pub fn tick_duration() -> Duration {
        Duration::from_secs_f64(1.0 / TICK_HZ)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_follow_elapsed_time() {
        let mut sim = SimApp::new(RulesData::default());
        sim.app.insert_resource(TimeUpdateStrategy::ManualDuration(SimApp::tick_duration() * 3));
        sim.update(); // The first frame only starts the clock.
        assert_eq!(sim.tick(), 0);
        sim.update();
        assert_eq!(sim.tick(), 3);
        sim.update();
        assert_eq!(sim.tick(), 6);
    }

    #[test]
    fn allocator_is_a_resource() {
        let mut sim = SimApp::new(RulesData::default());
        let mut ids = sim.app.world_mut().resource_mut::<NetIds>();
        assert_ne!(ids.0.allocate(), ids.0.allocate());
    }
}
