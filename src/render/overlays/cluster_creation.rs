use std::collections::HashMap;
use std::error::Error;

use smithay::backend::renderer::Color32F;
use smithay::output::Output;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Size};

use crate::clusters::CreationState;
use crate::render::draw::DrawContext;
use crate::render::scene::SceneElement;

use super::shell::{OverlayRgb, OverlayVisuals, resolve_visuals};

const BANNER_GAP: i32 = 6;
const ACTION_ROW_GAP_Y: i32 = 10;
const ACTION_ITEM_GAP: i32 = 18;
const ACTION_LABEL_GAP: i32 = 8;
const ACTION_KEY_PAD_X: i32 = 8;
const ACTION_KEY_PAD_Y: i32 = 6;
const ACTION_KEY_MIN_W: i32 = 48;
const CLUSTER_DIALOG_PAD_X: i32 = 18;
const CLUSTER_DIALOG_PAD_Y: i32 = 16;
const CLUSTER_DIALOG_INPUT_PAD_X: i32 = 12;
const CLUSTER_DIALOG_INPUT_PAD_Y: i32 = 10;
const CLUSTER_DIALOG_BUTTON_PAD_X: i32 = 16;
const CLUSTER_DIALOG_BUTTON_PAD_Y: i32 = 10;
const CLUSTER_DIALOG_MIN_WIDTH: i32 = 360;
const CLUSTER_DIALOG_MAX_WIDTH: i32 = 560;
const CLUSTER_DIALOG_INPUT_MIN_H: i32 = 38;
const CLUSTER_DIALOG_BUTTON_MIN_W: i32 = 110;
const CLUSTER_DIALOG_GAP_Y: i32 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CreationOverlayHit {
    ConfirmButton,
    InputCaret(usize),
}

#[derive(Clone, Debug)]
struct NamingHitLayout {
    output_origin: Point<i32, Logical>,
    input_rect: Rectangle<i32, Physical>,
    confirm_rect: Rectangle<i32, Physical>,
    text_x: i32,
    visible_start: usize,
    char_edges: Vec<(usize, i32)>,
}

#[derive(Default)]
pub(crate) struct ClusterCreationOverlay {
    naming_layouts: HashMap<String, NamingHitLayout>,
    confirm_hover: HashMap<String, f32>,
}

impl ClusterCreationOverlay {
    pub(crate) fn hit_test(&self, output: &str, global: (f64, f64)) -> Option<CreationOverlayHit> {
        let layout = self.naming_layouts.get(output)?;
        let local = Point::<i32, Physical>::from((
            (global.0 - f64::from(layout.output_origin.x)).round() as i32,
            (global.1 - f64::from(layout.output_origin.y)).round() as i32,
        ));
        if layout.confirm_rect.contains(local) {
            return Some(CreationOverlayHit::ConfirmButton);
        }
        if !layout.input_rect.contains(local) {
            return None;
        }
        let relative_x = (local.x - layout.text_x).max(0);
        let mut caret = layout.visible_start;
        let mut previous_width = 0;
        for &(index, width) in &layout.char_edges {
            if relative_x < (previous_width + width) / 2 {
                break;
            }
            previous_width = width;
            caret = index;
        }
        Some(CreationOverlayHit::InputCaret(caret))
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn elements(
    ctx: &mut DrawContext<'_>,
    state: &mut ClusterCreationOverlay,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    creation: Option<&CreationState>,
    pointer_position: (f64, f64),
    alpha: f32,
    config: &halley_config::Overlays,
    _decorations: &halley_config::Decorations,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let output_name = output.name();
    if alpha <= 0.0 {
        state.naming_layouts.remove(&output_name);
        state.confirm_hover.remove(&output_name);
        return Ok(Vec::new());
    }
    let Some(creation) =
        creation.filter(|creation| creation.output == output_name && creation.naming)
    else {
        state.naming_layouts.remove(&output_name);
        state.confirm_hover.remove(&output_name);
        return Ok(Vec::new());
    };
    let visuals = resolve_visuals(config);
    let screen = Rectangle::<i32, Physical>::from_size(output_geometry.size.to_physical(1));

    let (mut dialog, layout) = naming_dialog_elements(
        ctx,
        state,
        &output_name,
        output_geometry,
        screen,
        creation,
        pointer_position,
        alpha,
        visuals,
    )?;
    state.naming_layouts.insert(output_name, layout);
    dialog.push(SceneElement::Border(crate::render::solid_color_element(
        ctx.node
            .active_slot_id(crate::render::node::NodeSlot::ClusterCreationBackdrop),
        screen,
        Color32F::new(0.0, 0.0, 0.0, 0.14 * alpha),
    )));
    Ok(dialog)
}

#[allow(clippy::too_many_arguments)]
fn naming_dialog_elements(
    ctx: &mut DrawContext<'_>,
    state: &mut ClusterCreationOverlay,
    output_name: &str,
    output_geometry: Rectangle<i32, Logical>,
    screen: Rectangle<i32, Physical>,
    creation: &CreationState,
    pointer_position: (f64, f64),
    alpha: f32,
    visuals: OverlayVisuals,
) -> Result<(Vec<SceneElement>, NamingHitLayout), Box<dyn Error>> {
    let title = "Create cluster";
    let subtitle = "Choose a name for your new cluster";
    let title_size = ctx.text_size(title, visuals.text.bytes())?;
    let subtitle_size = ctx.text_size(subtitle, visuals.subtext.bytes())?;
    let confirm_size = ctx.text_size("Confirm", visuals.text.bytes())?;
    let button_width =
        (confirm_size.w + CLUSTER_DIALOG_BUTTON_PAD_X * 2).max(CLUSTER_DIALOG_BUTTON_MIN_W);
    let button_height = (confirm_size.h + CLUSTER_DIALOG_BUTTON_PAD_Y * 2).max(34);
    let input_height =
        (confirm_size.h + CLUSTER_DIALOG_INPUT_PAD_Y * 2).max(CLUSTER_DIALOG_INPUT_MIN_H);
    let actions = [("Enter", "confirm"), ("Esc", "cancel")];
    let action_size = action_row_size(ctx, &actions, visuals)?;
    let available_width = (screen.size.w - 36).max(1);
    let maximum = available_width.min(CLUSTER_DIALOG_MAX_WIDTH);
    let minimum = CLUSTER_DIALOG_MIN_WIDTH.min(maximum);
    let width = (title_size.w.max(subtitle_size.w).max(280) + CLUSTER_DIALOG_PAD_X * 2)
        .clamp(minimum, maximum);
    let height = CLUSTER_DIALOG_PAD_Y * 2
        + title_size.h
        + BANNER_GAP
        + subtitle_size.h
        + CLUSTER_DIALOG_GAP_Y
        + input_height
        + CLUSTER_DIALOG_GAP_Y
        + button_height
        + ACTION_ROW_GAP_Y
        + action_size.h;
    let dialog = Rectangle::new(
        (
            ((screen.size.w - width) / 2).max(18),
            ((screen.size.h - height) / 2).max(18),
        )
            .into(),
        (width.max(1), height.max(1)).into(),
    );
    let input = Rectangle::new(
        (
            dialog.loc.x + CLUSTER_DIALOG_PAD_X,
            dialog.loc.y
                + CLUSTER_DIALOG_PAD_Y
                + title_size.h
                + BANNER_GAP
                + subtitle_size.h
                + CLUSTER_DIALOG_GAP_Y,
        )
            .into(),
        (dialog.size.w - CLUSTER_DIALOG_PAD_X * 2, input_height).into(),
    );
    let confirm = Rectangle::new(
        (
            dialog.loc.x + dialog.size.w - CLUSTER_DIALOG_PAD_X - button_width,
            input.loc.y + input.size.h + CLUSTER_DIALOG_GAP_Y,
        )
            .into(),
        (button_width, button_height).into(),
    );
    let text_x = input.loc.x + CLUSTER_DIALOG_INPUT_PAD_X;
    let text_y = input.loc.y + (input.size.h - confirm_size.h) / 2;
    let visible_width = input.size.w - CLUSTER_DIALOG_INPUT_PAD_X * 2;
    let (visible_start, visible_end) =
        visible_text_range(ctx, creation, visible_width, visuals.text.bytes())?;
    let visible_text = char_slice(&creation.name_buffer, visible_start, visible_end);
    let caret_prefix = char_slice(&creation.name_buffer, visible_start, creation.caret_char);
    let caret_width = ctx.text_size(&caret_prefix, visuals.text.bytes())?.w;
    let caret_x = text_x + caret_width;
    let mut char_edges = Vec::new();
    for index in (visible_start + 1)..=visible_end {
        let prefix = char_slice(&creation.name_buffer, visible_start, index);
        char_edges.push((index, ctx.text_size(&prefix, visuals.text.bytes())?.w));
    }
    let selection = selection_range(creation).and_then(|(start, end)| {
        let start = start.clamp(visible_start, visible_end);
        let end = end.clamp(visible_start, visible_end);
        (start < end).then_some((start, end))
    });
    let local_pointer = Point::<i32, Physical>::from((
        (pointer_position.0 - f64::from(output_geometry.loc.x)).round() as i32,
        (pointer_position.1 - f64::from(output_geometry.loc.y)).round() as i32,
    ));
    let hover_target = if confirm.contains(local_pointer) {
        1.0
    } else {
        0.0
    };
    let hover = state
        .confirm_hover
        .entry(output_name.to_string())
        .or_default();
    *hover += (hover_target - *hover) * 0.16;
    if (*hover - hover_target).abs() < 0.015 {
        *hover = hover_target;
    }

    let mut elements = Vec::new();
    ctx.push_text(
        &mut elements,
        (
            dialog.loc.x + CLUSTER_DIALOG_PAD_X,
            dialog.loc.y + CLUSTER_DIALOG_PAD_Y,
        )
            .into(),
        title,
        visuals.text.bytes(),
        alpha,
    )?;
    ctx.push_text(
        &mut elements,
        (
            dialog.loc.x + CLUSTER_DIALOG_PAD_X,
            dialog.loc.y + CLUSTER_DIALOG_PAD_Y + title_size.h + BANNER_GAP,
        )
            .into(),
        subtitle,
        visuals.subtext.bytes(),
        0.98 * alpha,
    )?;
    ctx.push_text(
        &mut elements,
        (text_x, text_y).into(),
        &visible_text,
        visuals.text.bytes(),
        alpha,
    )?;
    if let Some((start, end)) = selection {
        let start_prefix = char_slice(&creation.name_buffer, visible_start, start);
        let selected = char_slice(&creation.name_buffer, start, end);
        let selection_x = text_x + ctx.text_size(&start_prefix, visuals.text.bytes())?.w;
        let selection_width = ctx.text_size(&selected, visuals.text.bytes())?.w.max(1);
        elements.push(SceneElement::Border(crate::render::solid_color_element(
            ctx.node
                .active_slot_id(crate::render::node::NodeSlot::ClusterNameSelection),
            Rectangle::new(
                (selection_x, input.loc.y + 7).into(),
                (selection_width, (input.size.h - 14).max(1)).into(),
            ),
            rgb_color(accent_fill(visuals), alpha),
        )));
    } else {
        elements.push(SceneElement::Border(crate::render::solid_color_element(
            ctx.node
                .active_slot_id(crate::render::node::NodeSlot::ClusterNameCaret),
            Rectangle::new(
                (caret_x, input.loc.y + 7).into(),
                (2, (input.size.h - 14).max(1)).into(),
            ),
            rgb_color(visuals.text, 0.94 * alpha),
        )));
    }
    elements.push(SceneElement::NodeLabel(ctx.card(
        input,
        OverlayVisuals {
            radius: 12.0,
            ..visuals
        },
        visuals.key_fill,
        alpha,
    )?));
    let confirm_fill = visuals.fill.mix(visuals.border, *hover);
    ctx.push_text(
        &mut elements,
        (
            confirm.loc.x + (confirm.size.w - confirm_size.w) / 2,
            confirm.loc.y + (confirm.size.h - confirm_size.h) / 2,
        )
            .into(),
        "Confirm",
        visuals.text.bytes(),
        alpha,
    )?;
    elements.push(SceneElement::NodeLabel(ctx.card(
        confirm,
        OverlayVisuals {
            radius: 12.0,
            ..visuals
        },
        confirm_fill,
        alpha,
    )?));
    push_action_row(
        ctx,
        &mut elements,
        (
            dialog.loc.x + CLUSTER_DIALOG_PAD_X,
            confirm.loc.y + confirm.size.h + ACTION_ROW_GAP_Y,
        )
            .into(),
        &actions,
        visuals,
        alpha,
    )?;
    elements.push(SceneElement::NodeLabel(ctx.card(
        dialog,
        OverlayVisuals {
            radius: 18.0,
            ..visuals
        },
        visuals.fill,
        0.98 * alpha,
    )?));

    Ok((
        elements,
        NamingHitLayout {
            output_origin: output_geometry.loc,
            input_rect: input,
            confirm_rect: confirm,
            text_x,
            visible_start,
            char_edges,
        },
    ))
}

fn char_slice(value: &str, start: usize, end: usize) -> String {
    value
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn selection_range(creation: &CreationState) -> Option<(usize, usize)> {
    (creation.selection_anchor_char != creation.selection_focus_char).then(|| {
        (
            creation
                .selection_anchor_char
                .min(creation.selection_focus_char),
            creation
                .selection_anchor_char
                .max(creation.selection_focus_char),
        )
    })
}

fn visible_text_range(
    ctx: &mut DrawContext<'_>,
    creation: &CreationState,
    visible_width: i32,
    color: [u8; 3],
) -> Result<(usize, usize), Box<dyn Error>> {
    let char_len = creation.name_buffer.chars().count();
    let mut start = creation.scroll_char.min(creation.caret_char).min(char_len);
    while start < creation.caret_char {
        let before = char_slice(&creation.name_buffer, start, creation.caret_char);
        if ctx.text_size(&before, color)?.w <= visible_width.max(1) - 12 {
            break;
        }
        start += 1;
    }
    while start > 0 {
        let candidate = char_slice(&creation.name_buffer, start - 1, creation.caret_char);
        if ctx.text_size(&candidate, color)?.w > visible_width.max(1) - 18 {
            break;
        }
        start -= 1;
    }
    let mut end = start;
    while end < char_len {
        let candidate = char_slice(&creation.name_buffer, start, end + 1);
        if ctx.text_size(&candidate, color)?.w > visible_width.max(1) {
            break;
        }
        end += 1;
    }
    Ok((start, end))
}

fn action_row_size(
    ctx: &mut DrawContext<'_>,
    actions: &[(&str, &str)],
    visuals: OverlayVisuals,
) -> Result<Size<i32, Buffer>, Box<dyn Error>> {
    let mut width = 0;
    let mut height = 0;
    for (index, (key, label)) in actions.iter().enumerate() {
        let key_size = ctx.text_size(key, visuals.text.bytes())?;
        let label_size = ctx.text_size(label, visuals.subtext.bytes())?;
        let key_width = (key_size.w + ACTION_KEY_PAD_X * 2).max(ACTION_KEY_MIN_W);
        width += key_width + ACTION_LABEL_GAP + label_size.w;
        if index + 1 < actions.len() {
            width += ACTION_ITEM_GAP;
        }
        height = height.max(key_size.h.max(label_size.h) + ACTION_KEY_PAD_Y * 2);
    }
    Ok((width, height).into())
}

fn push_action_row(
    ctx: &mut DrawContext<'_>,
    elements: &mut Vec<SceneElement>,
    origin: Point<i32, Physical>,
    actions: &[(&str, &str)],
    visuals: OverlayVisuals,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    let row_size = action_row_size(ctx, actions, visuals)?;
    let mut x = origin.x;
    for (index, (key, label)) in actions.iter().enumerate() {
        let key_size = ctx.text_size(key, visuals.text.bytes())?;
        let label_size = ctx.text_size(label, visuals.subtext.bytes())?;
        let key_width = (key_size.w + ACTION_KEY_PAD_X * 2).max(ACTION_KEY_MIN_W);
        let chip = Rectangle::new((x, origin.y).into(), (key_width, row_size.h).into());
        ctx.push_text(
            elements,
            (
                chip.loc.x + (chip.size.w - key_size.w) / 2,
                chip.loc.y + (chip.size.h - key_size.h) / 2,
            )
                .into(),
            key,
            visuals.text.bytes(),
            alpha,
        )?;
        elements.push(SceneElement::NodeLabel(ctx.label_card(
            chip,
            OverlayVisuals {
                radius: 10.0,
                ..visuals
            },
            visuals.key_fill,
            0.96 * alpha,
        )?));
        x += key_width + ACTION_LABEL_GAP;
        ctx.push_text(
            elements,
            (x, origin.y + (row_size.h - label_size.h) / 2).into(),
            label,
            visuals.subtext.bytes(),
            alpha,
        )?;
        x += label_size.w;
        if index + 1 < actions.len() {
            x += ACTION_ITEM_GAP;
        }
    }
    Ok(())
}

fn accent_fill(visuals: OverlayVisuals) -> OverlayRgb {
    visuals.key_fill.mix(visuals.border, 0.78)
}

fn rgb_color(color: OverlayRgb, alpha: f32) -> Color32F {
    Color32F::new(color.r, color.g, color.b, color.a * alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_range_is_order_independent() {
        let creation = CreationState {
            renaming: None,
            output: "DP-1".into(),
            selected: Default::default(),
            naming: true,
            name_buffer: "halley".into(),
            caret_char: 2,
            selection_anchor_char: 5,
            selection_focus_char: 2,
            scroll_char: 0,
            dragging_selection: true,
            prepared: None,
            name_repeat: None,
            draft: None,
        };
        assert_eq!(selection_range(&creation), Some((2, 5)));
    }

    #[test]
    fn character_slicing_is_unicode_safe() {
        assert_eq!(char_slice("aλ界z", 1, 3), "λ界");
    }
}
