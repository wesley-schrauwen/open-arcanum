//! A single-line text field with the usual behaviour:
//! - the text changes colour while the pointer is over the field;
//! - clicking the field focuses it: the placeholder disappears and a blinking caret follows the
//!   text; typing edits it (Backspace deletes);
//! - clicking anywhere else, Enter or Escape unfocuses it; an empty field shows its placeholder
//!   again. Escape that unfocuses a field does not also leave the screen
//!   ([`FocusedTextField::escape_consumed`]).
//!
//! Put [`TextField`] on a node; [`spawn_text_field`] adds the clickable node and its text.
//! Only characters the game fonts can show (Windows-1252, printable) are accepted.

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::picking::events::PointerPress;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ActivateOnPress, Button};

use crate::bitmap_text::BitmapText;
use crate::fonts::{ArcanumFonts, UiFont};

/// Shown after the text while the field is focused.
pub const CARET: char = '|';
/// Seconds the caret stays on, then off.
pub const CARET_BLINK_SECS: f32 = 0.5;

/// Text colours for the field's three states.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextFieldColors {
    pub normal: Color,
    pub hover: Color,
    pub focused: Color,
}

/// An editable line of text.
#[derive(Component, Debug, Clone, PartialEq)]
#[require(Node)]
pub struct TextField {
    pub value: String,
    /// Shown while the field is empty and not focused.
    pub placeholder: String,
    pub max_chars: usize,
    pub colors: TextFieldColors,
}

impl TextField {
    /// Adds `c` if it fits and the game fonts can show it.
    pub fn insert(&mut self, c: char) -> bool {
        if self.value.chars().count() >= self.max_chars || c.is_control() || arcanum_formats::cp1252::encode_char(c).is_none() {
            return false;
        }
        self.value.push(c);
        true
    }

    /// What to draw: the placeholder when unfocused and empty, otherwise the text plus the caret
    /// when focused and the blink is on.
    pub fn display(&self, focused: bool, caret_on: bool) -> String {
        if focused {
            if caret_on { format!("{}{CARET}", self.value) } else { self.value.clone() }
        } else if self.value.is_empty() {
            self.placeholder.clone()
        } else {
            self.value.clone()
        }
    }
}

/// The field's text entity (a child of the [`TextField`]).
#[derive(Component, Debug)]
pub struct TextFieldText;

/// Which field has the keyboard, and when its caret blink started.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq)]
pub struct FocusedTextField {
    pub entity: Option<Entity>,
    /// Wall-clock time the caret was last reset (focus or a keystroke).
    pub blink_start: f32,
    /// Set for the rest of the frame when Escape unfocused a field, so screens do not also treat
    /// it as "go back".
    pub escape_consumed: bool,
}

impl FocusedTextField {
    /// True while a field has focus or Escape was just used to leave one.
    pub fn captures_escape(&self) -> bool {
        self.entity.is_some() || self.escape_consumed
    }
}

pub struct TextFieldPlugin;

impl Plugin for TextFieldPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FocusedTextField>()
            .add_systems(First, |mut focus: ResMut<FocusedTextField>| {
                if focus.escape_consumed {
                    focus.escape_consumed = false;
                }
            })
            .add_systems(Update, (drop_lost_focus, type_into_field, show_fields).chain())
            .add_observer(focus_on_activate)
            .add_observer(blur_on_press_elsewhere);
    }
}

/// Spawns a clickable text field as a child: `node` places it; the text is centred in `font`.
/// `field_extra` goes on the field, `text_extra` on its text.
pub fn spawn_text_field(
    parent: &mut ChildSpawnerCommands,
    field: TextField,
    node: Node,
    font: UiFont,
    fonts: &ArcanumFonts,
    field_extra: impl Bundle,
    text_extra: impl Bundle,
) {
    let display = field.display(false, false);
    let color = field.colors.normal;
    parent
        .spawn((
            field,
            field_extra,
            Button,
            ActivateOnPress,
            Hovered::default(),
            Node { justify_content: JustifyContent::Center, align_items: AlignItems::Center, overflow: Overflow::clip(), ..node },
        ))
        .with_children(|cell| {
            let mut text = cell.spawn((TextFieldText, TextColor(color), bevy::picking::Pickable::IGNORE, text_extra));
            fonts.insert_text(&mut text, &display, font);
        });
}

fn focus_on_activate(activate: On<Activate>, fields: Query<(), With<TextField>>, time: Res<Time<Real>>, mut focus: ResMut<FocusedTextField>) {
    if fields.contains(activate.entity) {
        focus.entity = Some(activate.entity);
        focus.blink_start = time.elapsed_secs();
    }
}

/// A press anywhere outside the focused field (or its children) unfocuses it.
fn blur_on_press_elsewhere(press: On<PointerPress>, parents: Query<&ChildOf>, mut focus: ResMut<FocusedTextField>) {
    let Some(field) = focus.entity else { return };
    let target = press.original_event_target();
    // Propagation calls this once per ancestor; only the first (original) call matters.
    if press.entity != target {
        return;
    }
    let inside = std::iter::once(target).chain(parents.iter_ancestors(target)).any(|e| e == field);
    if !inside {
        focus.entity = None;
    }
}

/// Forgets a focused field that was despawned.
fn drop_lost_focus(fields: Query<(), With<TextField>>, mut focus: ResMut<FocusedTextField>) {
    if let Some(e) = focus.entity
        && !fields.contains(e)
    {
        focus.entity = None;
    }
}

fn type_into_field(
    keys: Option<MessageReader<KeyboardInput>>,
    time: Res<Time<Real>>,
    mut focus: ResMut<FocusedTextField>,
    mut fields: Query<&mut TextField>,
) {
    let Some(mut keys) = keys else { return };
    let Some(entity) = focus.entity else {
        keys.clear();
        return;
    };
    let Ok(mut field) = fields.get_mut(entity) else { return };
    for key in keys.read() {
        if key.state != ButtonState::Pressed {
            continue;
        }
        match &key.logical_key {
            Key::Backspace => {
                field.value.pop();
            }
            Key::Enter => {
                focus.entity = None;
                return;
            }
            Key::Escape => {
                focus.entity = None;
                focus.escape_consumed = true;
                return;
            }
            Key::Space => {
                field.insert(' ');
            }
            Key::Character(s) => {
                for c in s.chars() {
                    field.insert(c);
                }
            }
            _ => continue,
        }
        focus.blink_start = time.elapsed_secs();
    }
}

/// Keeps each field's text and colour in step with its value, focus, hover and the caret blink.
fn show_fields(
    time: Res<Time<Real>>,
    focus: Res<FocusedTextField>,
    fields: Query<(Entity, &TextField, &Hovered, &Children)>,
    mut texts: Query<(Option<&mut BitmapText>, Option<&mut Text>, &mut TextColor), With<TextFieldText>>,
) {
    let caret_on = ((time.elapsed_secs() - focus.blink_start) / CARET_BLINK_SECS).floor() as i64 % 2 == 0;
    for (entity, field, hovered, children) in &fields {
        let focused = focus.entity == Some(entity);
        let wanted = field.display(focused, caret_on);
        let color = if focused {
            field.colors.focused
        } else if hovered.get() {
            field.colors.hover
        } else {
            field.colors.normal
        };
        for child in children {
            let Ok((bitmap, text, mut text_color)) = texts.get_mut(*child) else { continue };
            if let Some(mut bitmap) = bitmap {
                if bitmap.text != wanted {
                    bitmap.text = wanted.clone();
                }
            } else if let Some(mut text) = text
                && text.0 != wanted
            {
                text.0 = wanted.clone();
            }
            if text_color.0 != color {
                text_color.0 = color;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field() -> TextField {
        let c = Color::WHITE;
        TextField { value: String::new(), placeholder: "Choose Name".into(), max_chars: 5, colors: TextFieldColors { normal: c, hover: c, focused: c } }
    }

    #[test]
    fn placeholder_only_when_empty_and_unfocused() {
        let mut f = field();
        assert_eq!(f.display(false, true), "Choose Name");
        assert_eq!(f.display(true, true), "|", "focused: the placeholder clears, the caret shows");
        assert_eq!(f.display(true, false), "", "and blinks off");
        f.value = "Vir".into();
        assert_eq!(f.display(false, true), "Vir");
        assert_eq!(f.display(true, true), "Vir|");
    }

    #[test]
    fn input_is_limited_to_printable_game_characters() {
        let mut f = field();
        for c in "Virgil".chars() {
            f.insert(c);
        }
        assert_eq!(f.value, "Virgi", "at most max_chars");
        let mut g = field();
        assert!(!g.insert('\u{8}'));
        assert!(!g.insert('日'));
        assert!(g.insert('é'), "Windows-1252 letters are fine");
    }
}
