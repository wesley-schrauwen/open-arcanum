//! Runs the single-player host in this process and connects the client to it.
//!
//! The host is a [`LocalHost`] (an `arcanum_sim` app behind a loopback transport), kept as a
//! non-send resource and updated once a frame. It is created on entering New Game (the choice
//! menu, the pre-made picker or character creation), with the game's rules and start map, and
//! dropped on going back to the main or Single Player menu. Each frame the bridge sends the
//! [`CharacterHost`]'s queued intents, updates the host, and hands the deltas that came back to
//! the [`CharacterHost`]; the local PC's `Spawn` is kept in [`LocalPc`] and its map in [`LocalMap`],
//! and the deltas about actors wait in [`ActorDeltas`] for the world.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use arcanum_net::{ClientTransport, LocalHost};
use arcanum_rules::RulesData;
use arcanum_formats::map::TileLoc;
use arcanum_sim::{EntityKind, LedgerDelta, NetId, PlayerId, SaveFile, SaveStatus, SimRules, SimVfs, StartMap};
use bevy::prelude::*;

use crate::actors::ActorDeltas;
use crate::character_host::CharacterHost;
use crate::create_character::{CreationData, load_creation_data};
use crate::main_menu::AppState;
use crate::vfs_asset::VfsResource;

/// The local player's id, until ids come from the player's config.
pub const LOCAL_PLAYER: PlayerId = PlayerId(1);

/// The client's end of the connection to the host.
#[derive(Resource)]
pub struct HostLink(pub Box<dyn ClientTransport>);

/// The in-process host (non-send: it holds a Bevy `App`).
pub struct LocalHostRes(pub LocalHost);

/// The local player's PC, once the host has spawned it.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalPc(pub Option<(NetId, TileLoc)>);

/// The map the host put the local player on (a folder under `maps/`).
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalMap(pub Option<String>);

/// How to make the local host.
#[derive(Resource, Debug, Clone, Default)]
pub struct HostSettings {
    /// Folder for save files; `None` writes nothing.
    pub save_dir: Option<PathBuf>,
    /// One host tick per frame, whatever the wall clock says (tests).
    pub manual_clock: bool,
    /// Start new games here instead of the start map's position (debugging).
    pub start_at: Option<TileLoc>,
}

pub struct SimBridgePlugin;

/// Screens that belong to a new game: entering any of them starts the host if none is running.
const NEW_GAME_STATES: [AppState; 3] = [AppState::NewGameChoice, AppState::NewGame, AppState::CreateCharacter];

impl Plugin for SimBridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HostSettings>()
            .init_resource::<CharacterHost>()
            .init_resource::<LocalPc>()
            .init_resource::<LocalMap>()
            .add_systems(OnEnter(AppState::MainMenu), stop_host)
            .add_systems(OnEnter(AppState::SinglePlayer), stop_host)
            .add_systems(PostUpdate, (share_rules, send_intents, run_host, receive_deltas).chain());
        for state in NEW_GAME_STATES {
            app.add_systems(OnEnter(state), (load_creation_data, start_host).chain());
        }
    }
}

/// Starts the host if none is running, with a fresh [`CharacterHost`] to talk to it.
pub fn start_host(world: &mut World) {
    if world.contains_non_send::<LocalHostRes>() {
        return;
    }
    let rules = world.get_resource::<CreationData>().map(|d| d.rules.clone()).unwrap_or_default();
    let vfs = world.get_resource::<VfsResource>().map(|v| v.0.clone());
    let start = vfs.as_ref().map(|v| v.read().map_err(|e| e.to_string()).and_then(|vfs| StartMap::load(&vfs)));
    let settings = world.get_resource::<HostSettings>().cloned().unwrap_or_default();

    let mut host = LocalHost::new(rules);
    match start {
        Some(Ok(mut start)) => {
            if let Some(at) = settings.start_at {
                start.position = at;
            }
            host.sim.app.insert_resource(start);
        }
        Some(Err(e)) => warn!("no start map, so no world to start: {e}"),
        None => {}
    }
    if let Some(vfs) = vfs {
        host.sim.app.insert_resource(SimVfs(vfs));
    }
    if let Some(dir) = &settings.save_dir {
        let session = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        host.sim.app.insert_resource(SaveFile::in_dir(dir, session));
    }
    if settings.manual_clock {
        host.sim.use_manual_clock();
    }
    let link = host.connect(LOCAL_PLAYER);
    world.insert_non_send(LocalHostRes(host));
    world.insert_resource(HostLink(Box::new(link)));
    // Anything the old CharacterHost had in flight was for a host that is gone.
    world.insert_resource(CharacterHost::default());
    world.insert_resource(LocalPc::default());
    world.insert_resource(LocalMap::default());
    world.insert_resource(ActorDeltas::default());
}

fn stop_host(world: &mut World) {
    world.remove_non_send::<LocalHostRes>();
    world.remove_resource::<HostLink>();
}

/// Gives the host the game's rules if they load after it started.
fn share_rules(data: Option<Res<CreationData>>, host: Option<NonSendMut<LocalHostRes>>) {
    if let (Some(data), Some(mut host)) = (data, host)
        && data.is_changed()
    {
        host.0.sim.app.insert_resource(SimRules(data.rules.clone()));
    }
}

fn send_intents(mut character: ResMut<CharacterHost>, link: Option<Res<HostLink>>) {
    let Some(link) = link else { return };
    for intent in character.take_outgoing() {
        if let Err(e) = link.0.send_intent(intent) {
            warn!("cannot reach the host: {e}");
        }
    }
}

fn run_host(host: Option<NonSendMut<LocalHostRes>>, mut reported: Local<Option<String>>) {
    let Some(mut host) = host else { return };
    host.0.update();
    let status = host.0.sim.app.world().resource::<SaveStatus>();
    if status.last_error != *reported {
        if let Some(e) = &status.last_error {
            warn!("cannot save: {e}");
        }
        *reported = status.last_error.clone();
    }
}

fn receive_deltas(
    link: Option<Res<HostLink>>,
    mut character: ResMut<CharacterHost>,
    mut pc: ResMut<LocalPc>,
    mut map: ResMut<LocalMap>,
    mut actors: Option<ResMut<ActorDeltas>>,
    data: Option<Res<CreationData>>,
) {
    let Some(link) = link else { return };
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
        if let LedgerDelta::EnterMap { map: name } = delta {
            map.0 = Some(name.clone());
        }
        if let LedgerDelta::Spawn { net_id, kind: EntityKind::Pc { player }, position, .. } = delta
            && *player == link.0.player()
        {
            pc.0 = Some((*net_id, *position));
        }
        if let Some(actors) = actors.as_mut()
            && ActorDeltas::wants(delta)
        {
            actors.0.push(delta.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use arcanum_formats::map::TileLoc;
    use arcanum_rules::CharacterAction;
    use arcanum_sim::{Session, SessionPlayer, WorldSave};

    use super::*;

    fn app(settings: HostSettings) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin, SimBridgePlugin)).init_state::<AppState>().insert_resource(settings);
        app.update();
        app
    }

    fn enter(app: &mut App, state: AppState) {
        app.world_mut().resource_mut::<NextState<AppState>>().set(state);
        app.update();
    }

    fn make_virgil(app: &mut App) {
        let rules = RulesData::default();
        let mut host = app.world_mut().resource_mut::<CharacterHost>();
        host.reset(&rules);
        host.apply(&rules, CharacterAction::SetName("Virgil".into())).unwrap();
        host.finish(&rules).unwrap();
    }

    #[test]
    fn the_host_runs_only_during_a_new_game() {
        let mut app = app(HostSettings { manual_clock: true, ..default() });
        assert!(!app.world().contains_non_send::<LocalHostRes>(), "not on the main menu");
        enter(&mut app, AppState::NewGameChoice);
        assert!(app.world().contains_non_send::<LocalHostRes>());
        make_virgil(&mut app);
        app.update(); // The host's clock starts; the next frame ticks.
        app.update();
        assert!(app.world().resource::<CharacterHost>().accepted);
        let host = app.world().non_send::<LocalHostRes>();
        let session = host.0.sim.app.world().resource::<Session>();
        assert!(matches!(&session.players[&LOCAL_PLAYER], SessionPlayer::Ready(b) if b.sheet.name == "Virgil"));

        enter(&mut app, AppState::NewGame);
        assert!(app.world().resource::<CharacterHost>().accepted, "moving between new-game screens keeps the host");
        enter(&mut app, AppState::MainMenu);
        assert!(!app.world().contains_non_send::<LocalHostRes>());
        assert!(!app.world().contains_resource::<HostLink>());
        enter(&mut app, AppState::CreateCharacter);
        assert!(!app.world().resource::<CharacterHost>().accepted, "a new host, a new character");
        assert_eq!(app.world().resource::<LocalPc>().0, None);
    }

    #[test]
    fn the_accepted_character_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(HostSettings { save_dir: Some(dir.path().to_path_buf()), manual_clock: true, start_at: None });
        enter(&mut app, AppState::NewGameChoice);
        let start = StartMap { map: "Arcanum1-024-fixed".into(), position: TileLoc { x: 92958, y: 82592 } };
        app.world_mut().non_send_mut::<LocalHostRes>().0.sim.app.insert_resource(start.clone());
        make_virgil(&mut app);
        app.update();
        app.update();
        assert_eq!(app.world().resource::<LocalPc>().0, Some((NetId(0), start.position)), "the PC is in the world");
        assert_eq!(app.world().resource::<LocalMap>().0.as_deref(), Some(start.map.as_str()));
        let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(files.len(), 1, "{files:?}");
        let save = WorldSave::read(&files[0]).unwrap();
        assert_eq!((save.map.as_str(), save.players[&LOCAL_PLAYER].position), (start.map.as_str(), start.position));
    }
}
