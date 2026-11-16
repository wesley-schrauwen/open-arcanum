//! The character editor's stats page, shown after character creation (Arcanum.exe window 14
//! opens the in-game character screen, 0x5597c0, over the menu; see NOTES.md).
//!
//! Background `Char_Maint.art` (800x400 at y = 41) in the shared menu frame. Positions are the
//! original's window coordinates (screen y = window y + 41):
//! - Info panel: name (212,15); Level / Points / XP Next (212, 34/53/72); Race / Gender / Age
//!   (337, 34/53/72). Labels from `mes/charedit.mes` 0-6.
//! - Primary stats: values centred at x 211 (ST CN DX BE) and 350 (IN WP PE CH), rows y 115-235;
//!   minus/plus buttons (`Char_Minus` / `Char_Plus`, 19x19) beside them.
//! - Hit points (heart) and fatigue (cross): values centred at (447,143) and (447,213), with
//!   minus/plus buttons; each point buys 4.
//! - "Derived Stats" (charedit 7) and "Resistance" (8) with their rows (`stat.mes` 8-15,
//!   charedit 12-16), in `Flare12Font`.
//! - Refused clicks show the original's message (charedit 17-20) in the help box.
//!
//! Back returns to character creation with the character kept; Next keeps the finished
//! character and loads the game (the original shows one more screen, window 15, first). The
//! skills, tech and spells tabs on the right are not built yet.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ActivateOnPress, Button};

use crate::art_loader::ArtFrames;
use crate::audio::MenuSounds;
use crate::character_host::CharacterHost;
use crate::create_character::CreationData;
use arcanum_rules::{CharacterAction, Message, PRIMARY_STATS, Pool, RESISTANCES, XP_FOR_LEVEL_2};
use crate::fonts::{ArcanumFonts, UiFont};
use crate::loading::LoadRequest;
use crate::main_menu::AppState;
use crate::menu_window::{HelpMessage, HoverHelp, MenuWindowPlugin, SCREEN_WHITE, TOP_BAR_HEIGHT, WindowArt, at, rect, screen_root, spawn_frame};
use crate::resource_pack::paths::interface;
use crate::widgets::FrameButton;

/// What a character editor button does.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorAction {
    Back,
    Next,
    /// Primary stat (stat.mes order: ST DX CN BE IN PE WP CH) up or down.
    Stat(usize, i32),
    HitPoints(i32),
    Fatigue(i32),
}

/// Root of the editor screen.
#[derive(Component)]
pub struct CharacterEditorScreen;

/// Root of the values (redrawn when the character changes).
#[derive(Component)]
pub struct EditorContent;

/// Marks texts, for tests.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorText {
    Name,
    Level,
    Points,
    XpNext,
    Race,
    Gender,
    Age,
    Stat(usize),
    HitPoints,
    Fatigue,
    Derived(usize),
    Resist(usize),
}

/// A primary stat raised above its starting value (Arcanum.exe font 0x64c7a0).
pub const RAISED: Color = Color::srgb_u8(0, 255, 0);
/// The name in the info panel.
pub const NAME_COLOR: Color = Color::srgb_u8(255, 210, 0);

#[derive(Resource)]
struct EditorArt {
    background: Handle<Image>,
    plus: Handle<ArtFrames>,
    minus: Handle<ArtFrames>,
}

/// Window coordinates (the background starts at screen y = 41).
fn win(x: f32, y: f32, w: f32, h: f32) -> Rect {
    rect(Vec2::new(x, y + TOP_BAR_HEIGHT), Vec2::new(w, h))
}

/// A box of width `w` centred on `x`.
fn centred_on(x: f32, y: f32, w: f32, h: f32) -> Rect {
    win(x - w / 2.0, y, w, h)
}

const BUTTON: f32 = 19.0;
/// Rows of the primary stats, and which stat sits in each (left column ST CN DX BE, right
/// column IN WP PE CH; stat.mes indices).
const STAT_ROWS: [f32; 4] = [115.0, 155.0, 195.0, 235.0];
const LEFT_STATS: [usize; 4] = [0, 2, 1, 3];
const RIGHT_STATS: [usize; 4] = [4, 6, 5, 7];
const DERIVED_ROWS: [f32; 4] = [308.0, 326.0, 344.0, 362.0];
const RESIST_ROWS: [f32; RESISTANCES] = [301.0, 317.0, 333.0, 349.0, 365.0];

pub struct CharacterEditorPlugin;

impl Plugin for CharacterEditorPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<MenuWindowPlugin>() {
            app.add_plugins(MenuWindowPlugin);
        }
        app.add_systems(Startup, |mut commands: Commands, assets: Res<AssetServer>| {
            commands.insert_resource(EditorArt {
                background: assets.load(interface::CHAR_MAINT),
                plus: assets.load(interface::CHAR_PLUS),
                minus: assets.load(interface::CHAR_MINUS),
            });
        })
        .add_systems(OnEnter(AppState::CharacterEditor), (crate::create_character::load_creation_data, build_screen).chain())
        .add_systems(Update, show_values.run_if(in_state(AppState::CharacterEditor)))
        .add_observer(on_activate);
    }
}

fn button(action: EditorAction, frames: &Handle<ArtFrames>, x: f32, y: f32) -> impl Bundle {
    (
        action,
        Button,
        ActivateOnPress,
        Hovered::default(),
        FrameButton::arrow(frames.clone()),
        ImageNode::default(),
        at(win(x, y, BUTTON, BUTTON)),
    )
}

fn build_screen(mut commands: Commands, art: Res<EditorArt>, frame: Res<WindowArt>, fonts: Option<Res<ArcanumFonts>>) {
    let fonts = fonts.map(|f| f.clone()).unwrap_or_default();
    commands
        .spawn((CharacterEditorScreen, DespawnOnExit(AppState::CharacterEditor), screen_root()))
        .with_children(|screen| {
            spawn_frame(
                screen,
                &frame,
                &fonts,
                art.background.clone(),
                EditorAction::Back,
                (EditorAction::Next, HoverHelp(7003, "Next Screen")),
            );
            for (i, &y) in STAT_ROWS.iter().enumerate() {
                let by = y + 2.0;
                screen.spawn(button(EditorAction::Stat(LEFT_STATS[i], -1), &art.minus, 170.0, by));
                screen.spawn(button(EditorAction::Stat(LEFT_STATS[i], 1), &art.plus, 224.0, by));
                screen.spawn(button(EditorAction::Stat(RIGHT_STATS[i], -1), &art.minus, 313.0, by));
                screen.spawn(button(EditorAction::Stat(RIGHT_STATS[i], 1), &art.plus, 367.0, by));
            }
            screen.spawn(button(EditorAction::HitPoints(-1), &art.minus, 409.0, 144.0));
            screen.spawn(button(EditorAction::HitPoints(1), &art.plus, 465.0, 144.0));
            screen.spawn(button(EditorAction::Fatigue(-1), &art.minus, 409.0, 214.0));
            screen.spawn(button(EditorAction::Fatigue(1), &art.plus, 465.0, 214.0));
        });
}

/// (Re)draws every value when the character changes or the screen opens.
#[allow(clippy::too_many_arguments)]
fn show_values(
    mut commands: Commands,
    host: Res<CharacterHost>,
    data: Option<Res<CreationData>>,
    added: Query<(), Added<CharacterEditorScreen>>,
    contents: Query<Entity, With<EditorContent>>,
    roots: Query<Entity, With<CharacterEditorScreen>>,
    fonts: Option<Res<ArcanumFonts>>,
) {
    let Some(data) = data else { return };
    if !host.is_changed() && added.is_empty() {
        return;
    }
    let Ok(root) = roots.single() else { return };
    for old in &contents {
        commands.entity(old).despawn();
    }
    let fonts = fonts.map(|f| f.clone()).unwrap_or_default();
    let c = host.sheet();
    let rules = &data.rules;
    let label = |id: u32, fallback: &str| data.charedit.get(id).unwrap_or(fallback).to_string();
    let stat_name = |id: u32| data.stats.get(id).unwrap_or("?").to_string();
    let stats = rules.stats(c);
    let derived = rules.derived(c);
    let resist = rules.resistances(c);
    let race = data.menu.get(720 + u32::from(c.race)).unwrap_or("").to_string();
    let gender = data.menu.get(740 + u32::from(c.gender)).unwrap_or("").to_string();

    let content = commands
        .spawn((
            EditorContent,
            bevy::picking::Pickable::IGNORE,
            Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        ))
        .with_children(|p| {
            let mut put = |r: Rect, text: &str, which: EditorText, font: UiFont, colour: Color, centre: bool| {
                let justify = if centre { JustifyContent::Center } else { JustifyContent::FlexStart };
                p.spawn(Node { justify_content: justify, align_items: AlignItems::Center, ..at(r) }).with_children(|cell| {
                    let mut t = cell.spawn((which, TextColor(colour)));
                    fonts.insert_text(&mut t, text, font);
                });
            };
            // Info panel.
            put(win(212.0, 15.0, 240.0, 19.0), &c.name, EditorText::Name, UiFont::Small, NAME_COLOR, false);
            put(win(212.0, 34.0, 120.0, 19.0), &format!("{}: 1", label(1, "Level")), EditorText::Level, UiFont::Small, SCREEN_WHITE, false);
            put(win(212.0, 53.0, 120.0, 19.0), &format!("{}: {}", label(0, "Points"), rules.unspent(c)), EditorText::Points, UiFont::Small, SCREEN_WHITE, false);
            put(win(212.0, 72.0, 120.0, 19.0), &format!("{}: {XP_FOR_LEVEL_2}", label(2, "XP Next")), EditorText::XpNext, UiFont::Small, SCREEN_WHITE, false);
            put(win(337.0, 34.0, 130.0, 19.0), &format!("{}: {race}", label(4, "Race")), EditorText::Race, UiFont::Small, SCREEN_WHITE, false);
            put(win(337.0, 53.0, 130.0, 19.0), &format!("{}: {gender}", label(5, "Gender")), EditorText::Gender, UiFont::Small, SCREEN_WHITE, false);
            put(win(337.0, 72.0, 130.0, 19.0), &format!("{}: 20", label(6, "Age")), EditorText::Age, UiFont::Small, SCREEN_WHITE, false);
            // Primary stats.
            for (i, &y) in STAT_ROWS.iter().enumerate() {
                for (x, stat) in [(211.0, LEFT_STATS[i]), (350.0, RIGHT_STATS[i])] {
                    let colour = if c.draft.stats[stat] > 0 { RAISED } else { SCREEN_WHITE };
                    put(centred_on(x, y, 40.0, 19.0), &stats[stat].to_string(), EditorText::Stat(stat), UiFont::Small, colour, true);
                }
            }
            let pool_colour = |bought: i32| if bought > 0 { RAISED } else { SCREEN_WHITE };
            put(centred_on(447.0, 143.0, 40.0, 19.0), &derived.hit_points.to_string(), EditorText::HitPoints, UiFont::Small, pool_colour(c.draft.hit_points), true);
            put(centred_on(447.0, 213.0, 40.0, 19.0), &derived.fatigue.to_string(), EditorText::Fatigue, UiFont::Small, pool_colour(c.draft.fatigue), true);
            // Derived stats and resistances.
            put(win(51.0, 282.0, 253.0, 18.0), &label(7, "Derived Stats"), EditorText::Derived(99), UiFont::Small, SCREEN_WHITE, true);
            let values = derived.by_stat_id();
            for (i, &y) in DERIVED_ROWS.iter().enumerate() {
                for (x, slot) in [(49.0, i), (178.0, i + 4)] {
                    let text = format!("{}: {}", stat_name(8 + slot as u32), values[slot]);
                    put(win(x, y, 125.0, 14.0), &text, EditorText::Derived(slot), UiFont::Body, SCREEN_WHITE, false);
                }
            }
            put(win(315.0, 282.0, 136.0, 18.0), &label(8, "Resistance"), EditorText::Resist(99), UiFont::Small, SCREEN_WHITE, true);
            for (i, &y) in RESIST_ROWS.iter().enumerate() {
                let text = format!("{}: {}", label(12 + i as u32, "?"), resist[i]);
                put(win(322.0, y, 105.0, 14.0), &text, EditorText::Resist(i), UiFont::Body, SCREEN_WHITE, false);
            }
        })
        .id();
    commands.entity(root).add_child(content);
}

#[allow(clippy::too_many_arguments)]
fn on_activate(
    activate: On<Activate>,
    actions: Query<&EditorAction>,
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
    let result = match action {
        EditorAction::Back => {
            next_state.set(AppState::CreateCharacter);
            Ok(())
        }
        EditorAction::Next => host.finish(&rules).map(|_| {
            commands.insert_resource(LoadRequest::new(AppState::InGame));
            next_state.set(AppState::Loading);
        }),
        EditorAction::Stat(i, d) if i < PRIMARY_STATS => {
            host.apply(&rules, if d > 0 { CharacterAction::RaiseStat(i) } else { CharacterAction::LowerStat(i) })
        }
        EditorAction::Stat(..) => Ok(()),
        EditorAction::HitPoints(d) => host.apply(&rules, CharacterAction::AdjustPool(Pool::HitPoints, d)),
        EditorAction::Fatigue(d) => host.apply(&rules, CharacterAction::AdjustPool(Pool::Fatigue, d)),
    };
    message.0 = result.err().map(|e| {
        let text = match (e.message(), data.as_ref()) {
            (Some(Message::Charedit(id)), Some(d)) => d.charedit.get(id),
            (Some(Message::MainMenu(id)), Some(d)) => d.menu.get(id),
            _ => None,
        };
        text.unwrap_or("That cannot be changed.").to_string()
    });
}
