//! A host in this process: a [`SimApp`] behind a [`LoopbackHost`]. Single player runs on one.

use arcanum_rules::RulesData;
use arcanum_sim::{PlayerId, SimApp};

use crate::{HostTransport, LoopbackClient, LoopbackHost};

pub struct LocalHost {
    pub sim: SimApp,
    transport: LoopbackHost,
}

impl LocalHost {
    pub fn new(rules: RulesData) -> Self {
        Self { sim: SimApp::new(rules), transport: LoopbackHost::new() }
    }

    /// A client end for `player`.
    pub fn connect(&self, player: PlayerId) -> LoopbackClient {
        self.transport.connect(player)
    }

    /// Hands received intents to the sim, runs one frame, and sends what it wrote. Deltas for
    /// players who are gone are dropped.
    pub fn update(&mut self) {
        self.sim.receive(self.transport.recv_intents());
        self.sim.update();
        for (player, delta) in self.sim.take_deltas() {
            let _ = self.transport.send_delta(player, delta);
        }
    }
}
