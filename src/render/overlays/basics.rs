//! The one-time Halley basics card.
//!
//! A single compositor-owned overlay card: the Field/cluster mental model plus
//! the five essential chords. It deliberately keeps the same card styling,
//! typography, and input conventions as the rest of `shell::overlay` instead of
//! introducing a separate UI system, and it dims nothing - it is meant to be
//! read once and dismissed, not to take the desktop away.

use std::error::Error;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Buffer, Physical, Rectangle, Size};

use super::shell::{OverlayVisuals, card_element};
use crate::render::node::NodeRenderer;
use crate::render::scene::SceneElement;
use crate::render::text::UiTextRenderer;

const PADDING: i32 = 24;
const TITLE_BOTTOM: i32 = 8;
const MODEL_BOTTOM: i32 = 4;
const MODEL_TO_ROWS: i32 = 20;
const ROW_GAP: i32 = 10;
const ROWS_TO_FOOTER: i32 = 20;
const CHIP_PADDING_X: i32 = 8;
const CHIP_TEXT_GAP: i32 = 12;

const TITLE: &str = "Halley basics";
const MODEL: &str = "Windows live on the Field.";
const MODEL_SUBTEXT: &str = "Clusters are optional named contexts you can create later.";
const FOOTER: &str =
    "Enter, Esc, or click to dismiss. Reopen it anytime from Lift: Show Halley basics.";

/// The five essentials, in the order the card teaches them. The key is rendered
/// after the configured modifier, so the card names the chords this session
/// actually listens for.
const ESSENTIALS: [(&str, &str); 5] = [
    ("D", "Launch or search with Lift"),
    ("Left-drag", "Move a window"),
    (
        "A",
        "Arrange visible windows, or restore their saved geometry",
    ),
    ("N", "Collapse or restore a window"),
    ("O", "See everything in Apogee"),
];

struct Row {
    chord: String,
    description: &'static str,
    chord_size: Size<i32, Buffer>,
    description_size: Size<i32, Buffer>,
}

#[allow(clippy::too_many_arguments)]
pub fn elements(
    renderer: &mut GlesRenderer,
    screen: Rectangle<i32, Physical>,
    snapshot: crate::shell::overlay::BasicsCardSnapshot,
    visuals: OverlayVisuals,
    node_renderer: &mut NodeRenderer,
    ui_text: &mut UiTextRenderer,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let mix = snapshot.mix;
    let title_size = text_size(renderer, ui_text, TITLE, visuals.text.bytes())?;
    let model_size = text_size(renderer, ui_text, MODEL, visuals.text.bytes())?;
    let subtext_size = text_size(renderer, ui_text, MODEL_SUBTEXT, visuals.subtext.bytes())?;
    let footer_size = text_size(renderer, ui_text, FOOTER, visuals.subtext.bytes())?;

    let mut rows = Vec::with_capacity(ESSENTIALS.len());
    for (chord, description) in chord_entries(&snapshot.modifier) {
        let chord_size = text_size(renderer, ui_text, &chord, visuals.text.bytes())?;
        let description_size = text_size(renderer, ui_text, description, visuals.subtext.bytes())?;
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
    let rows_height =
        row_height * ESSENTIALS.len() as i32 + ROW_GAP * (ESSENTIALS.len() as i32 - 1);

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
    push_text(
        renderer,
        ui_text,
        elements,
        (left, y),
        TITLE,
        visuals.text.bytes(),
        mix,
    )?;
    y += title_size.h + TITLE_BOTTOM;
    push_text(
        renderer,
        ui_text,
        elements,
        (left, y),
        MODEL,
        visuals.text.bytes(),
        mix,
    )?;
    y += model_size.h + MODEL_BOTTOM;
    push_text(
        renderer,
        ui_text,
        elements,
        (left, y),
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
        push_text(
            renderer,
            ui_text,
            elements,
            (chip.loc.x + CHIP_PADDING_X, chip.loc.y + 4),
            &row.chord,
            visuals.text.bytes(),
            mix,
        )?;
        elements.push(SceneElement::NodeLabel(card_element(
            renderer,
            node_renderer,
            chip,
            OverlayVisuals {
                border_px: 0.0,
                ..visuals
            },
            visuals.key_fill,
            0.96 * mix,
        )?));
        push_text(
            renderer,
            ui_text,
            elements,
            (
                chip.loc.x + chip.size.w + CHIP_TEXT_GAP,
                y + (row_height - row.description_size.h) / 2,
            ),
            row.description,
            visuals.subtext.bytes(),
            mix,
        )?;
        y += row_height + ROW_GAP;
    }

    y += ROWS_TO_FOOTER - ROW_GAP;
    push_text(
        renderer,
        ui_text,
        elements,
        (left, y),
        FOOTER,
        visuals.subtext.bytes(),
        mix,
    )?;

    elements.push(SceneElement::NodeLabel(card_element(
        renderer,
        node_renderer,
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

/// The card's rows, as `(chord, description)`, with the session's own modifier
/// substituted so the card names the chords this session listens for.
fn chord_entries(modifier: &str) -> Vec<(String, &'static str)> {
    ESSENTIALS
        .iter()
        .map(|(key, description)| (format!("{modifier}+{key}"), *description))
        .collect()
}

fn text_size(
    renderer: &mut GlesRenderer,
    ui_text: &mut UiTextRenderer,
    text: &str,
    color: [u8; 3],
) -> Result<Size<i32, Buffer>, Box<dyn Error>> {
    Ok(ui_text
        .measure(renderer, text, color)?
        .unwrap_or((0, 0).into()))
}

#[allow(clippy::too_many_arguments)]
fn push_text(
    renderer: &mut GlesRenderer,
    ui_text: &mut UiTextRenderer,
    elements: &mut Vec<SceneElement>,
    origin: (i32, i32),
    text: &str,
    color: [u8; 3],
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    if let Some(text) = ui_text.element(renderer, origin.into(), text, color, alpha)? {
        elements.push(SceneElement::UiText(text.element));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card_text() -> String {
        let mut text = format!("{TITLE}\n{MODEL}\n{MODEL_SUBTEXT}\n{FOOTER}\n");
        for (chord, description) in chord_entries("Mod") {
            text.push_str(&format!("{chord}: {description}\n"));
        }
        text
    }

    /// The card states the mental model plainly: windows live on the Field, and
    /// clusters are an optional later concept.
    #[test]
    fn card_states_the_field_first_mental_model() {
        assert_eq!(MODEL, "Windows live on the Field.");
        assert!(MODEL_SUBTEXT.contains("Clusters are optional"));
        assert!(MODEL_SUBTEXT.contains("create later"));
    }

    /// Exactly the five essentials, in the order the card teaches them.
    #[test]
    fn card_shows_only_the_five_essentials() {
        let rows = chord_entries("Super");
        assert_eq!(
            rows,
            vec![
                ("Super+D".to_string(), "Launch or search with Lift"),
                ("Super+Left-drag".to_string(), "Move a window"),
                (
                    "Super+A".to_string(),
                    "Arrange visible windows, or restore their saved geometry"
                ),
                ("Super+N".to_string(), "Collapse or restore a window"),
                ("Super+O".to_string(), "See everything in Apogee"),
            ]
        );
    }

    /// The card names the session's own modifier, so a nested `--winit` session
    /// shows the Alt chords it actually listens for.
    #[test]
    fn card_chords_use_the_session_modifier() {
        let rows = chord_entries("Alt");
        assert!(rows.iter().all(|(chord, _)| chord.starts_with("Alt+")));
        assert_eq!(rows[0].0, "Alt+D");
        assert_eq!(rows[1].0, "Alt+Left-drag");
    }

    /// Milestone 3 scope lock: no zoom, Bearings, Trail, pinning, or cluster
    /// layout teaching belongs on this card, and it is not a multi-step
    /// tutorial.
    #[test]
    fn card_teaches_nothing_outside_its_scope() {
        let text = card_text().to_ascii_lowercase();
        for forbidden in [
            "zoom",
            "bearings",
            "trail",
            "pin",
            "cluster layout",
            "cluster composer",
            "workspace",
            "tutorial",
            "step 1",
            "step 2",
        ] {
            assert!(
                !text.contains(forbidden),
                "the basics card must not mention {forbidden:?}"
            );
        }
        assert!(
            FOOTER.contains("Enter") && FOOTER.contains("Esc") && FOOTER.contains("click"),
            "the footer names every dismissal affordance the card implements"
        );
        assert!(
            FOOTER.contains("Show Halley basics"),
            "the footer points at the manual reopening action"
        );
    }
}
