use std::sync::{Arc, RwLock};

use arcanum_client::main_menu::{MenuLabel, FitToBaseline};
use arcanum_client::options::OptionSlot;
use arcanum_client::{
    ArcanumAssetsPlugin, BitmapTextPlugin, BrightnessPlugin, CharacterEditorPlugin, ConfigPlugin, CreateCharacterPlugin, DisplayPlugin, FontsPlugin, LoadGamePlugin, LoadingPlugin, NewGamePlugin, MainMenuPlugin, MenuAudioPlugin, ResourcePack,
    OptionsPlugin, ScreensPlugin, VfsAssetPlugin,
};
use arcanum_client::AppState;
use arcanum_client::config;
use arcanum_client::load_game::{LoadGameScreen, sample_saves};
use arcanum_client::new_game::NewGameScreen;
use arcanum_client::loading::{LoadRequest, LoadingPicture};
use arcanum_client::settings::EnvOverrides;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

/// Debugging aid: when set to a file path, saves the window there once the menu has loaded,
/// then exits.
const SCREENSHOT_VAR: &str = "DEBUG_ARCANUM_SCREENSHOT";
/// Debugging aid: `options`, `singleplayer`, `newgamechoice`, `newgame`, `createcharacter`,
/// `charactereditor`, `loadgame`, `loading` or `ingame` opens that screen instead of the main menu.
const SCREEN_VAR: &str = "DEBUG_ARCANUM_SCREEN";

fn main() {
    // Game data: `ARCANUM_DIR` (default `assets/Arcanum`), optional `ARCANUM_OVERRIDE` and
    // `ARCANUM_MODULE` (default `Arcanum`).
    let pack = ResourcePack::from_env();
    let vfs = pack
        .open_vfs()
        .unwrap_or_else(|e| panic!("cannot open game data in {}: {e}", pack.game_dir.display()));
    let vfs = Arc::new(RwLock::new(vfs));
    // Defaults, then the saved config file, then ARCANUM_RES / ARCANUM_FULLSCREEN. Resolved before
    // the window exists so it opens at the saved size.
    let (settings, config) = config::load(config::path_from_env(), EnvOverrides::from_env());
    let mut app = App::new();
    app.insert_resource(settings.clone())
        .insert_resource(config)
        .insert_resource(pack)
        // Must come before DefaultPlugins so the VFS replaces the default asset source.
        .add_plugins(VfsAssetPlugin::new(vfs))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Open Arcanum (2026-November)".into(),
                resolution: settings.window_resolution(),
                mode: settings.window_mode(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((
            ArcanumAssetsPlugin,
            BitmapTextPlugin,
            BrightnessPlugin,
            ConfigPlugin,
            CreateCharacterPlugin,
            CharacterEditorPlugin,
            DisplayPlugin,
            FontsPlugin,
            LoadGamePlugin,
            LoadingPlugin,
            NewGamePlugin,
            MenuAudioPlugin,
            MainMenuPlugin,
            OptionsPlugin,
            ScreensPlugin,
        ))
        // The single-player host, in this process. Saves go next to the config file.
        .insert_resource(arcanum_client::sim_bridge::HostSettings {
            save_dir: config::path_from_env().and_then(|p| p.parent().map(|d| d.join("saves"))),
            manual_clock: false,
            // Debugging aid: `DEBUG_ARCANUM_START=x,y` starts new games on that tile.
            start_at: std::env::var("DEBUG_ARCANUM_START").ok().and_then(|v| {
                let (x, y) = v.split_once(',')?;
                Some(arcanum_formats::map::TileLoc { x: x.trim().parse().ok()?, y: y.trim().parse().ok()? })
            }),
        })
        .add_plugins((
            arcanum_client::sim_bridge::SimBridgePlugin,
            arcanum_client::WorldRenderPlugin,
            arcanum_client::animation::AnimationPlugin,
            arcanum_client::actors::ActorsPlugin,
        ));

    // Debugging aid until clicks move the PC: `DEBUG_ARCANUM_WALK=x,y` walks the PC to that
    // tile once it is in the world.
    if let Some(goal) = std::env::var("DEBUG_ARCANUM_WALK").ok().and_then(|v| {
        let (x, y) = v.split_once(',')?;
        Some(arcanum_formats::map::TileLoc { x: x.trim().parse().ok()?, y: y.trim().parse().ok()? })
    }) {
        app.add_systems(OnEnter(AppState::InGame), move |mut walk: ResMut<arcanum_client::actors::WalkLocalPc>| walk.goal = Some(goal));
    }

    let start = match std::env::var(SCREEN_VAR).unwrap_or_default().to_ascii_lowercase().as_str() {
        "options" => Some(AppState::Options),
        "singleplayer" => Some(AppState::SinglePlayer),
        "newgamechoice" => Some(AppState::NewGameChoice),
        "newgame" => Some(AppState::NewGame),
        "createcharacter" => Some(AppState::CreateCharacter),
        "charactereditor" => Some(AppState::CharacterEditor),
        // With example saves, so the list and info display have something to show.
        "loadgame" => {
            app.insert_resource(sample_saves());
            Some(AppState::LoadGame)
        }
        // A new game with a default character, straight into the world.
        "ingame" => {
            app.insert_resource(ScreenshotTheWorld).add_systems(Update, start_debug_game.run_if(in_state(AppState::CreateCharacter)));
            Some(AppState::CreateCharacter)
        }
        // Stays up for a minute so it can be captured.
        "loading" => {
            app.insert_resource(LoadRequest::new(AppState::MainMenu).min_secs(60.0));
            Some(AppState::Loading)
        }
        _ => None,
    };
    if let Some(start) = start {
        app.add_systems(Startup, move |mut next: ResMut<NextState<AppState>>| next.set(start));
    }

    if let Ok(path) = std::env::var(SCREENSHOT_VAR) {
        app.insert_resource(ScreenshotPath(path))
            .add_systems(Update, screenshot_and_exit.run_if(screen_ready));
    }
    app.run();
}

/// `DEBUG_ARCANUM_SCREEN=ingame`: once the creation data is in, names a character "Debug",
/// hands it to the host and goes through the loading screen into the world.
fn start_debug_game(
    mut commands: Commands,
    data: Option<Res<arcanum_client::create_character::CreationData>>,
    mut host: ResMut<arcanum_client::character_host::CharacterHost>,
    mut next: ResMut<NextState<AppState>>,
) {
    let Some(data) = data else { return };
    host.reset(&data.rules);
    let made = host
        .apply(&data.rules, arcanum_rules::CharacterAction::SetName("Debug".into()))
        .and_then(|()| host.finish(&data.rules).map(|_| ()));
    if let Err(e) = made {
        warn!("debug character refused: {e:?}");
        return;
    }
    commands.insert_resource(LoadRequest::new(AppState::InGame).after_world_boots());
    next.set(AppState::Loading);
}

#[derive(Resource)]
struct ScreenshotPath(String);

/// Ground tiles and map objects.
type WorldSprite = Or<(With<arcanum_client::world_render::GroundTile>, With<arcanum_client::world_render::MapObject>)>;

/// The screenshot waits for the world, not the first screen on the way.
#[derive(Resource)]
struct ScreenshotTheWorld;

/// Waits for the menu to finish loading, captures the window, then exits once the capture has
/// had time to be written.
fn screenshot_and_exit(
    mut commands: Commands,
    path: Res<ScreenshotPath>,
    mut frames_since_ready: Local<Option<u32>>,
    mut exit: MessageWriter<AppExit>,
) {
    let frames = frames_since_ready.get_or_insert(0);
    *frames += 1;
    // Grace frames so the layout has been applied and the renderer has finished preparing its
    // pipelines (a screen that is ready very early would otherwise be captured black).
    if *frames == 60 {
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path.0.clone()));
    } else if *frames == 150 {
        exit.write(AppExit::Success);
    }
}

/// True once a menu, the Options screen or the loading screen is up and the background has been
/// fitted.
#[allow(clippy::too_many_arguments)]
fn screen_ready(
    labels: Query<(), With<MenuLabel>>,
    slots: Query<(), With<OptionSlot>>,
    pending: Query<(), With<FitToBaseline>>,
    loading: Query<&ImageNode, With<LoadingPicture>>,
    load_game: Query<(), With<LoadGameScreen>>,
    new_game: Query<(), With<NewGameScreen>>,
    create: Query<(), With<arcanum_client::create_character::CreateCharacterScreen>>,
    editor: Query<(), With<arcanum_client::character_editor::CharacterEditorScreen>>,
    world: Option<Res<arcanum_client::world_render::ClientMap>>,
    tiles: Query<(), With<arcanum_client::world_render::GroundTile>>,
    waiting: Query<&Visibility, WorldSprite>,
    state: Res<State<AppState>>,
    for_world: Option<Res<ScreenshotTheWorld>>,
    images: Res<Assets<Image>>,
) -> bool {
    // The world: once its tiles are drawn and every one has its art.
    if for_world.is_some() || *state.get() == AppState::InGame {
        if *state.get() != AppState::InGame {
            return false;
        }
        return world.is_some_and(|w| !w.drawn.is_empty()) && !tiles.is_empty() && waiting.iter().all(|v| *v != Visibility::Hidden);
    }
    let loading_up = loading.iter().any(|p| images.get(&p.image).is_some());
    (!labels.is_empty() || !slots.is_empty() || loading_up || !load_game.is_empty() || !new_game.is_empty() || !create.is_empty() || !editor.is_empty()) && pending.is_empty()
}
