//! The headless host app and its fixed-rate tick.
//!
//! [`SimApp::new`] builds a Bevy [`App`] with only time and the [`SimPlugin`]: no window,
//! renderer or audio. Whoever owns it calls [`SimApp::update`]; in single player that is a
//! client system, in a dedicated host a loop. Game logic runs in `FixedUpdate` at [`TICK_HZ`].

use std::time::Duration;

use bevy_app::{App, FixedUpdate, Plugin};
use bevy_ecs::prelude::*;
use bevy_time::{Fixed, Time, TimePlugin};

use crate::{NetIdAllocator, Tick};

/// Host ticks per second.
pub const TICK_HZ: f64 = 30.0;

/// The number of the last finished tick (0 before the first).
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SimTick(pub Tick);

/// The host's [`NetIdAllocator`].
#[derive(Resource, Debug, Default)]
pub struct NetIds(pub NetIdAllocator);

/// Host resources and the tick counter.
pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_hz(TICK_HZ))
            .init_resource::<SimTick>()
            .init_resource::<NetIds>()
            .add_systems(FixedUpdate, advance_tick);
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
    pub fn new() -> Self {
        let mut app = App::new();
        app.add_plugins((TimePlugin, SimPlugin));
        Self { app }
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

impl Default for SimApp {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use bevy_time::TimeUpdateStrategy;

    use super::*;

    #[test]
    fn ticks_follow_elapsed_time() {
        let mut sim = SimApp::new();
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
        let mut sim = SimApp::new();
        let mut ids = sim.app.world_mut().resource_mut::<NetIds>();
        assert_ne!(ids.0.allocate(), ids.0.allocate());
    }
}
