//! Character creation: the original's `CreateCharacterBase` screen (window index 12 in
//! Arcanum.exe's screen table, record 0x5c4e28; buttons at 0x5c4c48, click handler 0x544ff0) in
//! the shared menu frame ([`crate::menu_window`]). Reached from the New Game choice menu
//! (New Character). Rules live in [`arcanum_rules::creation`].
//!
//! - Bar 1: the name, a [`crate::text_field`]: hover lightens it, a click focuses it (the
//!   "Choose Name" placeholder, `MainMenu.mes` 500, clears and a caret blinks), clicking elsewhere,
//!   Enter or Escape unfocuses it, and an empty field shows the placeholder again.
//! - Portrait arrows (`Big_Grn_L/R`, 43,118 / 244,118) step through the race-gender's portraits.
//! - Bars 2-4 with `MPCycle` arrows: gender (both arrows toggle; only where the race allows),
//!   race (skips male-only races for women; resets portrait and background), background (the
//!   ones legal for the race and gender).
//! - Upper-right panel: "Initial Stats" (739) and the eight primary stats as "Name: value"
//!   (`mes/stat.mes`), base 8 plus race and background effects. The original also lists seven
//!   derived stats (carry weight ... reaction modifier); their formulas are not known yet.
//! - Large panel: the background's story when one is chosen; otherwise "Race" (745) and the race
//!   description (770 + gender + 2 × race).
//! - Next needs a name ("You must choose a name.", 506, otherwise); then the character is kept as
//!   the [`CharacterHost`](crate::character_host::CharacterHost) and the game loads (the original continues to another screen first).

use arcanum_formats::mes::MesFile;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ActivateOnPress, Button};

use crate::art_loader::ArtFrames;
use crate::audio::MenuSounds;
use crate::bitmap_text::Align;
use arcanum_rules::{CharacterAction, CharacterSheet, MAX_NAME, PRIMARY_STATS, RulesData};

use crate::character_host::CharacterHost;
use crate::fonts::{ArcanumFonts, ORIGINAL_SCALE, UiFont};
use crate::main_menu::AppState;
use crate::menu_window::{HelpMessage, HoverHelp, MenuWindowPlugin, SCREEN_RED, SCREEN_WHITE, TOP_BAR_HEIGHT, WindowArt, at, rect, screen_root, spawn_frame};
use crate::premade::split_story;
use crate::resource_pack::paths::interface;
use crate::text_field::{TextField, TextFieldColors, TextFieldPlugin, spawn_text_field};
use crate::vfs_asset::VfsResource;
use crate::widgets::FrameButton;

/// What a character creation button does.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateAction {
    Back,
    Next,
    Portrait(i32),
    Gender,
    Race(i32),
    Background(i32),
}

/// Root of the character creation screen.
#[derive(Component)]
pub struct CreateCharacterScreen;

/// The name bar's text field.
#[derive(Component)]
pub struct NameField;

/// The name text while the pointer is over it or while typing.
pub const NAME_HOVER: Color = Color::srgb_u8(255, 140, 140);

/// Root of the parts that change with the character (redrawn on change).
#[derive(Component)]
pub struct CreateContent;


/// Marks the text pieces, for tests.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateText {
    Name,
    Gender,
    Race,
    Background,
    StatsHeading,
    Stat(usize),
    PanelHeading,
    PanelBody,
}

/// Rules and text tables, read when the screen first opens.
#[derive(Resource, Debug, Clone, Default)]
pub struct CreationData {
    pub rules: RulesData,
    pub menu: MesFile,
    pub stats: MesFile,
    pub stories: MesFile,
    /// The character editor's labels and messages.
    pub charedit: MesFile,
}

impl CreationData {
    pub fn load(vfs: &arcanum_vfs::Vfs) -> Result<Self, String> {
        let mes = |path: &str| -> Result<MesFile, String> {
            let data = vfs.read(path).map_err(|e| format!("{path}: {e}"))?;
            MesFile::parse(&data).map_err(|e| format!("{path}: {e}"))
        };
        Ok(Self {
            rules: RulesData::load(vfs)?,
            menu: mes("mes/MainMenu.mes")?,
            stats: mes("mes/stat.mes")?,
            stories: mes("mes/gameback.mes")?,
            charedit: mes("mes/charedit.mes")?,
        })
    }

    fn menu_text(&self, id: i64, fallback: &str) -> String {
        u32::try_from(id).ok().and_then(|id| self.menu.get(id)).unwrap_or(fallback).to_string()
    }

    /// Texts for the bars, the stats and the large panel.
    pub fn texts(&self, c: &CharacterSheet) -> CreateTexts {
        let name = if c.has_name() { c.name.clone() } else { self.menu_text(500, "Choose Name") };
        let story = self.rules.background(c).and_then(|b| self.stories.get(b.description)).unwrap_or("");
        let (title, body) = split_story(story);
        let stats = self.rules.stats(c);
        let stat_lines = std::array::from_fn(|i| format!("{}: {}", self.stats.get(i as u32).unwrap_or("?"), stats[i]));
        let has_background = self.rules.background(c).is_some_and(|b| b.description > 1000);
        let (panel_heading, panel_body) = if has_background {
            (String::new(), body.to_string())
        } else {
            (self.menu_text(745, "Race"), self.menu_text(770 + i64::from(c.gender) + 2 * i64::from(c.race), ""))
        };
        CreateTexts {
            name,
            gender: self.menu_text(740 + i64::from(c.gender), ""),
            race: self.menu_text(720 + i64::from(c.race), ""),
            background: title.to_string(),
            stats_heading: self.menu_text(739, "Initial Stats"),
            stats: stat_lines,
            panel_heading,
            panel_body,
        }
    }
}

/// Everything the screen writes for a character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTexts {
    pub name: String,
    pub gender: String,
    pub race: String,
    pub background: String,
    pub stats_heading: String,
    pub stats: [String; PRIMARY_STATS],
    pub panel_heading: String,
    pub panel_body: String,
}

#[derive(Resource)]
struct CreateArt {
    background: Handle<Image>,
    portrait_left: Handle<ArtFrames>,
    portrait_right: Handle<ArtFrames>,
    cycle_left: Handle<ArtFrames>,
    cycle_right: Handle<ArtFrames>,
}

// Layout: positions from the original's button table are screen coordinates; text rects are in
// background pixels (add TOP_BAR_HEIGHT).
const PORTRAIT: Rect = Rect { min: Vec2::new(95.0, 28.0), max: Vec2::new(223.0, 156.0) };
const BIG_ARROW: Vec2 = Vec2::new(34.0, 33.0);
const CYCLE_ARROW: Vec2 = Vec2::new(23.0, 23.0);
const NAME_BAR: Rect = Rect { min: Vec2::new(46.0, 183.0), max: Vec2::new(274.0, 202.0) };
const BARS: [Rect; 3] = [
    Rect { min: Vec2::new(66.0, 243.0), max: Vec2::new(252.0, 262.0) },
    Rect { min: Vec2::new(66.0, 293.0), max: Vec2::new(252.0, 312.0) },
    Rect { min: Vec2::new(66.0, 343.0), max: Vec2::new(252.0, 362.0) },
];
const STATS_HEADING: Rect = Rect { min: Vec2::new(339.0, 23.0), max: Vec2::new(748.0, 42.0) };
/// Stat rows in the original's order table (0x5c5130: ST CN DX BE / IN WP PE CH) and rects
/// (0x5c4f50): two columns of four.
const STAT_ORDER: [usize; PRIMARY_STATS] = [0, 2, 1, 3, 4, 6, 5, 7];
const STAT_ROWS: [Vec2; PRIMARY_STATS] = [
    Vec2::new(346.0, 46.0),
    Vec2::new(346.0, 66.0),
    Vec2::new(346.0, 86.0),
    Vec2::new(346.0, 106.0),
    Vec2::new(406.0 + 60.0, 46.0),
    Vec2::new(406.0 + 60.0, 66.0),
    Vec2::new(406.0 + 60.0, 86.0),
    Vec2::new(406.0 + 60.0, 106.0),
];
const STAT_SIZE: Vec2 = Vec2::new(115.0, 15.0);
const PANEL: Rect = Rect { min: Vec2::new(350.0, 155.0), max: Vec2::new(745.0, 367.0) };
const PANEL_HEADING: Rect = Rect { min: Vec2::new(350.0, 159.0), max: Vec2::new(745.0, 178.0) };
const PANEL_BODY: Rect = Rect { min: Vec2::new(350.0, 185.0), max: Vec2::new(745.0, 367.0) };

fn on_screen(r: Rect) -> Rect {
    Rect::from_corners(r.min + Vec2::Y * TOP_BAR_HEIGHT, r.max + Vec2::Y * TOP_BAR_HEIGHT)
}

pub struct CreateCharacterPlugin;

impl Plugin for CreateCharacterPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<MenuWindowPlugin>() {
            app.add_plugins(MenuWindowPlugin);
        }
        if !app.is_plugin_added::<TextFieldPlugin>() {
            app.add_plugins(TextFieldPlugin);
        }
        app.init_resource::<CharacterHost>()
            .add_systems(Startup, |mut commands: Commands, assets: Res<AssetServer>| {
                commands.insert_resource(CreateArt {
                    background: assets.load(interface::CREATE_BACK),
                    portrait_left: assets.load(interface::PICKER_LEFT),
                    portrait_right: assets.load(interface::PICKER_RIGHT),
                    cycle_left: assets.load(interface::CYCLE_LEFT),
                    cycle_right: assets.load(interface::CYCLE_RIGHT),
                });
            })
            .add_systems(OnEnter(AppState::CreateCharacter), (load_creation_data, build_screen).chain())
            .add_systems(Update, (sync_name, show_character).chain().run_if(in_state(AppState::CreateCharacter)))
            .add_observer(on_activate);
    }
}

/// Reads the rules and text tables once (character creation and the editor both need them). The
/// draft itself is reset by the New Character menu entry, so coming back from the editor keeps it.
pub fn load_creation_data(mut commands: Commands, data: Option<Res<CreationData>>, vfs: Option<Res<VfsResource>>) {
    if data.is_some() {
        return;
    }
    let loaded = vfs
        .and_then(|v| v.0.read().ok().map(|vfs| CreationData::load(&vfs)))
        .unwrap_or_else(|| Err("no game data".into()));
    match loaded {
        Ok(d) => commands.insert_resource(d),
        Err(e) => {
            warn!("character creation data unavailable: {e}");
            commands.insert_resource(CreationData::default());
        }
    }
}

fn button(action: CreateAction, frames: &Handle<ArtFrames>, r: Rect) -> impl Bundle {
    (action, Button, ActivateOnPress, Hovered::default(), FrameButton::arrow(frames.clone()), ImageNode::default(), at(r))
}

fn build_screen(
    mut commands: Commands,
    art: Res<CreateArt>,
    frame: Res<WindowArt>,
    fonts: Option<Res<ArcanumFonts>>,
    data: Option<Res<CreationData>>,
    host: Res<CharacterHost>,
) {
    let fonts = fonts.map(|f| f.clone()).unwrap_or_default();
    let placeholder = data.as_ref().and_then(|d| d.menu.get(500)).unwrap_or("Choose Name").to_string();
    commands
        .spawn((CreateCharacterScreen, DespawnOnExit(AppState::CreateCharacter), screen_root()))
        .with_children(|screen| {
            spawn_frame(
                screen,
                &frame,
                &fonts,
                art.background.clone(),
                CreateAction::Back,
                (CreateAction::Next, HoverHelp(7003, "Next Screen")),
            );
            let name = TextField {
                value: host.sheet().name.clone(),
                placeholder,
                max_chars: MAX_NAME,
                colors: TextFieldColors { normal: SCREEN_RED, hover: NAME_HOVER, focused: NAME_HOVER },
            };
            spawn_text_field(screen, name, at(on_screen(NAME_BAR)), UiFont::Small, &fonts, NameField, CreateText::Name);
            // Screen coordinates from the original's button table.
            screen.spawn(button(CreateAction::Portrait(-1), &art.portrait_left, rect(Vec2::new(43.0, 118.0), BIG_ARROW)));
            screen.spawn(button(CreateAction::Portrait(1), &art.portrait_right, rect(Vec2::new(244.0, 118.0), BIG_ARROW)));
            for (y, left, right) in [
                (281.0, CreateAction::Gender, CreateAction::Gender),
                (331.0, CreateAction::Race(-1), CreateAction::Race(1)),
                (381.0, CreateAction::Background(-1), CreateAction::Background(1)),
            ] {
                screen.spawn(button(left, &art.cycle_left, rect(Vec2::new(42.0, y), CYCLE_ARROW)));
                screen.spawn(button(right, &art.cycle_right, rect(Vec2::new(255.0, y + 1.0), CYCLE_ARROW)));
            }
        });
}

/// Copies the name field into the draft.
fn sync_name(fields: Query<&TextField, (With<NameField>, Changed<TextField>)>, mut host: ResMut<CharacterHost>, data: Option<Res<CreationData>>) {
    let rules = data.as_ref().map(|d| d.rules.clone()).unwrap_or_default();
    for field in &fields {
        if host.sheet().name != field.value {
            let _ = host.apply(&rules, CharacterAction::SetName(field.value.clone()));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn show_character(
    mut commands: Commands,
    host: Res<CharacterHost>,
    data: Option<Res<CreationData>>,
    added: Query<(), Added<CreateCharacterScreen>>,
    contents: Query<Entity, With<CreateContent>>,
    roots: Query<Entity, With<CreateCharacterScreen>>,
    assets: Res<AssetServer>,
    fonts: Option<Res<ArcanumFonts>>,
) {
    let Some(data) = data else { return };
    if !host.is_changed() && added.is_empty() && !data.is_changed() {
        return;
    }
    let Ok(root) = roots.single() else { return };
    for old in &contents {
        commands.entity(old).despawn();
    }
    let fonts = fonts.map(|f| f.clone()).unwrap_or_default();
    let texts = data.texts(host.sheet());
    let portrait = data.rules.portrait(host.sheet()).map(|stem| assets.load::<Image>(format!("portrait/{stem}_b.bmp")));
    let content = commands
        .spawn((
            CreateContent,
            // Full-screen layer above the buttons: let the pointer through.
            bevy::picking::Pickable::IGNORE,
            Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        ))
        .with_children(|c| {
            if let Some(image) = portrait {
                c.spawn((ImageNode::new(image), at(on_screen(PORTRAIT))));
            }
            // Fonts and colours as the original's slots: name morph15 red, bars Flare12 red,
            // headings morph15 white, stats and story Flare12 white.
            let mut text = |r: Rect, value: &str, which: CreateText, centre: bool, font: UiFont, colour: Color| {
                let justify = if centre { JustifyContent::Center } else { JustifyContent::FlexStart };
                // Clipped to the box: long texts (e.g. "No significant background") must not run
                // under the bar's arrows.
                c.spawn(Node { justify_content: justify, align_items: AlignItems::Center, overflow: Overflow::clip(), ..at(on_screen(r)) }).with_children(|cell| {
                    let mut t = cell.spawn((which, TextColor(colour)));
                    fonts.insert_text(&mut t, value, font);
                });
            };
            text(BARS[0], &texts.gender, CreateText::Gender, true, UiFont::Body, SCREEN_RED);
            text(BARS[1], &texts.race, CreateText::Race, true, UiFont::Body, SCREEN_RED);
            text(BARS[2], &texts.background, CreateText::Background, true, UiFont::Body, SCREEN_RED);
            text(STATS_HEADING, &texts.stats_heading, CreateText::StatsHeading, true, UiFont::Small, SCREEN_WHITE);
            for (slot, &stat) in STAT_ORDER.iter().enumerate() {
                text(rect(STAT_ROWS[slot], STAT_SIZE), &texts.stats[stat], CreateText::Stat(stat), false, UiFont::Body, SCREEN_WHITE);
            }
            let body_rect = if texts.panel_heading.is_empty() { PANEL } else { PANEL_BODY };
            if !texts.panel_heading.is_empty() {
                text(PANEL_HEADING, &texts.panel_heading, CreateText::PanelHeading, true, UiFont::Small, SCREEN_WHITE);
            }
            c.spawn(Node { overflow: Overflow::clip(), ..at(on_screen(body_rect)) }).with_children(|panel| {
                let mut t = panel.spawn((CreateText::PanelBody, TextColor(SCREEN_WHITE)));
                let (w, h) = (body_rect.width() * ORIGINAL_SCALE, body_rect.height() * ORIGINAL_SCALE);
                fonts.insert_block(&mut t, &texts.panel_body, UiFont::Body, w, Some(h), Align::Left);
            });
        })
        .id();
    commands.entity(root).add_child(content);
}

#[allow(clippy::too_many_arguments)]
fn on_activate(
    activate: On<Activate>,
    actions: Query<&CreateAction>,
    mut commands: Commands,
    mut next_state: ResMut<NextState<AppState>>,
    mut host: ResMut<CharacterHost>,
    data: Option<Res<CreationData>>,
    mut message: ResMut<HelpMessage>,
    sounds: Option<Res<MenuSounds>>,
) {
    let Ok(&action) = actions.get(activate.entity) else { return };
    if let Some(sounds) = &sounds {
        sounds.play_button_press(&mut commands);
    }
    let rules = data.as_ref().map(|d| d.rules.clone()).unwrap_or_default();
    match action {
        CreateAction::Back => next_state.set(AppState::NewGameChoice),
        // Refusals here have no message in the original (e.g. female for a male-only race).
        CreateAction::Portrait(d) => drop(host.apply(&rules, CharacterAction::StepPortrait(d))),
        CreateAction::Gender => drop(host.apply(&rules, CharacterAction::ToggleGender)),
        CreateAction::Race(d) => drop(host.apply(&rules, CharacterAction::StepRace(d))),
        CreateAction::Background(d) => drop(host.apply(&rules, CharacterAction::StepBackground(d))),
        CreateAction::Next => {
            if !host.sheet().has_name() {
                let text = data.as_ref().and_then(|d| d.menu.get(506)).unwrap_or("You must choose a name.");
                message.0 = Some(text.to_string());
                return;
            }
            // On to the character editor to spend the starting points (the original's window 14).
            next_state.set(AppState::CharacterEditor);
        }
    }
}
