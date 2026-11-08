//! The one-time Halley basics card.
//!
//! A single compositor-owned overlay card: the Field/cluster mental model plus
//! the five essential chords. It deliberately keeps the same card styling,
//! typography, and input conventions as the rest of `shell::overlay` instead of
//! introducing a separate UI system, and it dims nothing - it is meant to be
//! read once and dismissed, not to take the desktop away.

use std::error::Error;

use smithay::utils::{Buffer, Physical, Rectangle, Size};

use super::shell::OverlayVisuals;
use crate::input::keybinds::{ResolvedBind, drag_shortcut_label, shortcut_label};
use crate::render::draw::DrawContext;
use crate::render::scene::SceneElement;

const PADDING: i32 = 24;
const TITLE_BOTTOM: i32 = 8;
const MODEL_BOTTOM: i32 = 4;
const MODEL_TO_ROWS: i32 = 20;
const ROW_GAP: i32 = 10;
const ROWS_TO_FOOTER: i32 = 20;
const CHIP_PADDING_X: i32 = 8;
const CHIP_TEXT_GAP: i32 = 12;

pub(crate) const TITLE: &str = "Halley basics";
pub(crate) const MODEL: &str = "Windows live on the Field.";
pub(crate) const MODEL_SUBTEXT: &str = "Clusters are optional named contexts you can create later.";
pub(crate) const FOOTER: &str =
    "Enter, Esc, or click to dismiss. Reopen it anytime from Lift: Show Halley basics.";

/// Shown in place of a chord when the user has no binding for that row.
pub(crate) const NO_SHORTCUT: &str = "No shortcut";
const LIFT: &str = "Launch or search with Lift";
const TERMINAL: &str = "Open a terminal";

/// One card row: the chord the session actually listens for, and what it does.
pub(crate) type BasicsRow = (String, &'static str);

/// The five essentials, in the order the card teaches them, each labelled with
/// the chord the user really has bound in the Field. A rebinding renames the
/// row; a missing binding says so instead of naming a key that does nothing.
pub(crate) fn rows(binds: &[ResolvedBind]) -> Vec<BasicsRow> {
    let context = crate::input::BindingContext::field();
    let key =
        |action: halley_config::Action| shortcut_label(binds, context, |bound| *bound == action);
    let lift = shortcut_label(binds, context, is_lift);
    let launcher = match lift {
        Some(chord) => (chord, LIFT),
        None => match key(halley_config::Action::OpenTerminal) {
            Some(chord) => (chord, TERMINAL),
            None => (NO_SHORTCUT.into(), LIFT),
        },
    };
    let or_none = |chord: Option<String>| chord.unwrap_or_else(|| NO_SHORTCUT.into());
    vec![
        launcher,
        (
            or_none(drag_shortcut_label(binds, context, |action| {
                *action == halley_config::Action::PointerMoveWindow
            })),
            "Move a window",
        ),
        (
            or_none(key(halley_config::Action::Tidy)),
            "Tidy visible windows, or put them back",
        ),
        (
            or_none(key(halley_config::Action::ToggleState)),
            "Collapse or restore a window",
        ),
        (
            or_none(key(halley_config::Action::Apogee)),
            "See everything in Apogee",
        ),
    ]
}

/// A binding that launches Halley Lift: a command whose program is
/// `halley-lift`, with or without arguments.
fn is_lift(action: &halley_config::Action) -> bool {
    matches!(action, halley_config::Action::Spawn(command)
        if command.split_whitespace().next() == Some("halley-lift"))
}

struct Row {
    chord: String,
    description: &'static str,
    chord_size: Size<i32, Buffer>,
    description_size: Size<i32, Buffer>,
}

pub fn elements(
    ctx: &mut DrawContext<'_>,
    screen: Rectangle<i32, Physical>,
    snapshot: crate::shell::overlay::BasicsCardSnapshot,
    visuals: OverlayVisuals,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let mix = snapshot.mix;
    let title_size = ctx.text_size(TITLE, visuals.text.bytes())?;
    let model_size = ctx.text_size(MODEL, visuals.text.bytes())?;
    let subtext_size = ctx.text_size(MODEL_SUBTEXT, visuals.subtext.bytes())?;
    let footer_size = ctx.text_size(FOOTER, visuals.subtext.bytes())?;

    let mut rows = Vec::with_capacity(snapshot.rows.len());
    for (chord, description) in snapshot.rows {
        let chord_size = ctx.text_size(&chord, visuals.text.bytes())?;
        let description_size = ctx.text_size(description, visuals.subtext.bytes())?;
        rows.push(Row {
            chord,
            description,
            chord_size,
            description_size,
        });
    }

    let row_height = rows
        .iter()
        .map(|row| (row.chord_size.h + 8).max(row.description_size.h))
        .max()
        .unwrap_or(0);
    let row_count = rows.len() as i32;
    let rows_height = row_height * row_count + ROW_GAP * (row_count - 1).max(0);

    let header_width = title_size
        .w
        .max(model_size.w)
        .max(subtext_size.w)
        .max(footer_size.w);
    let rows_width = rows.iter().map(row_width).max().unwrap_or(0);
    let card_width = header_width
        .max(rows_width)
        .saturating_add(2 * PADDING)
        .max(320)
        .min((screen.size.w - 36).max(1));
    let card_height = 2 * PADDING
        + title_size.h
        + TITLE_BOTTOM
        + model_size.h
        + MODEL_BOTTOM
        + subtext_size.h
        + MODEL_TO_ROWS
        + rows_height
        + ROWS_TO_FOOTER
        + footer_size.h;

    let card = Rectangle::new(
        (
            screen.loc.x + (screen.size.w - card_width) / 2,
            screen.loc.y + (screen.size.h - card_height) / 2,
        )
            .into(),
        (card_width, card_height).into(),
    );

    let left = card.loc.x + PADDING;
    let mut y = card.loc.y + PADDING;
    ctx.push_text(elements, (left, y).into(), TITLE, visuals.text.bytes(), mix)?;
    y += title_size.h + TITLE_BOTTOM;
    ctx.push_text(elements, (left, y).into(), MODEL, visuals.text.bytes(), mix)?;
    y += model_size.h + MODEL_BOTTOM;
    ctx.push_text(
        elements,
        (left, y).into(),
        MODEL_SUBTEXT,
        visuals.subtext.bytes(),
        mix,
    )?;
    y += subtext_size.h + MODEL_TO_ROWS;

    for row in &rows {
        let chip_height = row.chord_size.h + 8;
        let chip = Rectangle::new(
            (left, y + (row_height - chip_height) / 2).into(),
            (row_width(row), chip_height).into(),
        );
        ctx.push_text(
            elements,
            (chip.loc.x + CHIP_PADDING_X, chip.loc.y + 4).into(),
            &row.chord,
            visuals.text.bytes(),
            mix,
        )?;
        elements.push(SceneElement::NodeLabel(ctx.card(
            chip,
            OverlayVisuals {
                border_px: 0.0,
                ..visuals
            },
            visuals.key_fill,
            0.96 * mix,
        )?));
        ctx.push_text(
            elements,
            (
                chip.loc.x + chip.size.w + CHIP_TEXT_GAP,
                y + (row_height - row.description_size.h) / 2,
            )
                .into(),
            row.description,
            visuals.subtext.bytes(),
            mix,
        )?;
        y += row_height + ROW_GAP;
    }

    y += ROWS_TO_FOOTER - ROW_GAP;
    ctx.push_text(
        elements,
        (left, y).into(),
        FOOTER,
        visuals.subtext.bytes(),
        mix,
    )?;

    elements.push(SceneElement::NodeLabel(ctx.card(
        card,
        visuals,
        visuals.fill,
        0.97 * mix,
    )?));
    Ok(())
}

/// A row's chord chip is wide enough for the chord text plus padding on both
/// sides.
fn row_width(row: &Row) -> i32 {
    row.chord_size.w + 2 * CHIP_PADDING_X
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keybinds::{BackendKind, resolve_binds};
    use halley_config::{Action, Keybinds};

    fn rows_for(keybinds: &Keybinds, backend: BackendKind) -> Vec<BasicsRow> {
        rows(&resolve_binds(keybinds, backend))
    }

    /// The defaults read as before, and a nested `--winit` session shows the
    /// Alt chords it actually listens for.
    #[test]
    fn default_rows_use_the_session_modifier() {
        let keybinds = Keybinds::default();
        let chords = |backend| {
            rows_for(&keybinds, backend)
                .into_iter()
                .map(|(chord, _)| chord)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            chords(BackendKind::Tty),
            [
                "Super+D",
                "Super+Left-drag",
                "Super+A",
                "Super+N",
                "Super+O"
            ]
        );
        assert_eq!(
            chords(BackendKind::Winit),
            ["Alt+D", "Alt+Left-drag", "Alt+A", "Alt+N", "Alt+O"]
        );
    }

    #[test]
    fn a_rebound_action_renames_its_row() {
        let mut keybinds = Keybinds::default();
        for bind in &mut keybinds.binds {
            if bind.action == Action::ToggleState {
                bind.key = "c".into();
            }
        }
        let rows = rows_for(&keybinds, BackendKind::Tty);
        assert_eq!(rows[3], ("Super+C".into(), "Collapse or restore a window"));
    }

    #[test]
    fn unbound_rows_say_so() {
        let mut keybinds = Keybinds::default();
        keybinds.binds.retain(|bind| bind.action != Action::Apogee);
        let rows = rows_for(&keybinds, BackendKind::Tty);
        assert_eq!(rows[4], (NO_SHORTCUT.into(), "See everything in Apogee"));
    }

    #[test]
    fn without_lift_the_launcher_row_names_the_terminal() {
        let mut keybinds = Keybinds::default();
        keybinds.binds.retain(|bind| !is_lift(&bind.action));
        let rows = rows_for(&keybinds, BackendKind::Tty);
        assert_eq!(rows[0], ("Super+T".into(), TERMINAL));
        keybinds
            .binds
            .retain(|bind| bind.action != Action::OpenTerminal);
        let rows = rows_for(&keybinds, BackendKind::Tty);
        assert_eq!(rows[0], (NO_SHORTCUT.into(), LIFT));
    }

    #[test]
    fn lift_with_arguments_still_counts_as_lift() {
        assert!(is_lift(&Action::Spawn("halley-lift --toggle".into())));
        assert!(!is_lift(&Action::Spawn("halley-lifter".into())));
        assert!(!is_lift(&Action::OpenTerminal));
    }
}
