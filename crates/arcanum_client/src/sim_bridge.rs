//! Runs the single-player host in this process and connects the client to it.
//!
//! The host is a [`LocalHost`] (an `arcanum_sim` app behind a loopback transport), kept as a
//! non-send resource and updated once a frame. Each frame the bridge sends the
//! [`CharacterHost`]'s queued intents, updates the host, and hands the deltas that came back to
//! the [`CharacterHost`]. The host starts with default rules and takes the game's rules once
//! [`CreationData`] has loaded them.

use arcanum_net::{ClientTransport, LocalHost};
use arcanum_rules::RulesData;
use arcanum_sim::{PlayerId, SimRules};
use bevy::prelude::*;

use crate::character_host::CharacterHost;
use crate::create_character::CreationData;

/// The local player's id, until ids come from the player's config.
pub const LOCAL_PLAYER: PlayerId = PlayerId(1);

/// The client's end of the connection to the host.
#[derive(Resource)]
pub struct HostLink(pub Box<dyn ClientTransport>);

/// The in-process host (non-send: it holds a Bevy `App`).
pub struct LocalHostRes(pub LocalHost);

pub struct SimBridgePlugin;

impl Plugin for SimBridgePlugin {
    fn build(&self, app: &mut App) {
        let host = LocalHost::new(RulesData::default());
        let link = host.connect(LOCAL_PLAYER);
        app.insert_non_send(LocalHostRes(host))
            .insert_resource(HostLink(Box::new(link)))
            .init_resource::<CharacterHost>()
            .add_systems(PostUpdate, (share_rules, send_intents, run_host, receive_deltas).chain());
    }
}

fn share_rules(data: Option<Res<CreationData>>, mut host: NonSendMut<LocalHostRes>) {
    if let Some(data) = data.filter(|d| d.is_changed()) {
        host.0.sim.app.insert_resource(SimRules(data.rules.clone()));
    }
}

fn send_intents(mut character: ResMut<CharacterHost>, link: Res<HostLink>) {
    for intent in character.take_outgoing() {
        if let Err(e) = link.0.send_intent(intent) {
            warn!("cannot reach the host: {e}");
        }
    }
}

fn run_host(mut host: NonSendMut<LocalHostRes>) {
    host.0.update();
}

fn receive_deltas(link: Res<HostLink>, mut character: ResMut<CharacterHost>, data: Option<Res<CreationData>>) {
    let deltas = link.0.recv_deltas();
    if deltas.is_empty() {
        return;
    }
    let fallback;
    let rules = match &data {
        Some(d) => &d.rules,
        None => {
            fallback = RulesData::default();
            &fallback
        }
    };
    for delta in &deltas {
        character.receive(rules, delta);
    }
}

#[cfg(test)]
mod tests {
    use arcanum_rules::CharacterAction;
    use arcanum_sim::{Session, SessionPlayer};

    use super::*;

    #[test]
    fn a_character_made_on_the_client_is_accepted_by_the_host() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, SimBridgePlugin));
        app.world_mut().non_send_mut::<LocalHostRes>().0.sim.use_manual_clock();
        let rules = RulesData::default();
        {
            let mut host = app.world_mut().resource_mut::<CharacterHost>();
            host.reset(&rules);
            host.apply(&rules, CharacterAction::SetName("Virgil".into())).unwrap();
            host.finish(&rules).unwrap();
        }
        // The first frame starts the host's clock; the second runs its tick.
        app.update();
        app.update();
        assert!(app.world().resource::<CharacterHost>().accepted);
        let host = app.world().non_send::<LocalHostRes>();
        let session = host.0.sim.app.world().resource::<Session>();
        assert!(matches!(&session.players[&LOCAL_PLAYER], SessionPlayer::Ready(b) if b.sheet.name == "Virgil"));
    }
}
