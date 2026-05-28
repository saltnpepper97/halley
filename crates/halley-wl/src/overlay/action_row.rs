use std::error::Error;

use smithay::{
    backend::renderer::gles::GlesFrame,
    utils::{Physical, Rectangle},
};

use crate::render::state::RenderState;
use crate::text::{draw_ui_text_in, ui_text_size_in};

use super::{
    ACTION_ITEM_GAP, ACTION_KEY_MIN_W, ACTION_KEY_PAD_X, ACTION_KEY_PAD_Y, ACTION_KEY_SCALE,
    ACTION_LABEL_GAP, ACTION_LABEL_SCALE, OverlayVisuals, draw_overlay_chip_without_shadow,
};

pub(super) fn overlay_action_item_metrics(
    render_state: &RenderState,
    font: &halley_config::FontConfig,
    key: &str,
    label: &str,
) -> (i32, i32, i32, i32, i32) {
    let (key_w, key_h) = ui_text_size_in(render_state, font, key, ACTION_KEY_SCALE);
    let (label_w, label_h) = ui_text_size_in(render_state, font, label, ACTION_LABEL_SCALE);
    let keycap_w = (key_w + ACTION_KEY_PAD_X * 2).max(ACTION_KEY_MIN_W);
    let keycap_h = (key_h + ACTION_KEY_PAD_Y * 2).max(20);
    let item_w = keycap_w + ACTION_LABEL_GAP + label_w;
    let item_h = keycap_h.max(label_h);
    (item_w, item_h, keycap_w, keycap_h, label_h)
}

pub(super) fn overlay_action_row_size(
    render_state: &RenderState,
    font: &halley_config::FontConfig,
    actions: &[(&str, &str)],
) -> (i32, i32) {
    let mut total_w = 0;
    let mut total_h = 0;
    for (index, (key, label)) in actions.iter().enumerate() {
        let (item_w, item_h, _, _, _) = overlay_action_item_metrics(render_state, font, key, label);
        if index > 0 {
            total_w += ACTION_ITEM_GAP;
        }
        total_w += item_w;
        total_h = total_h.max(item_h);
    }
    (total_w, total_h)
}

pub(super) fn draw_overlay_action_row(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    font: &halley_config::FontConfig,
    x: i32,
    y: i32,
    actions: &[(&str, &str)],
    damage: Rectangle<i32, Physical>,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    let (_, row_h) = overlay_action_row_size(render_state, font, actions);
    let mut cursor_x = x;
    for (index, (key, label)) in actions.iter().enumerate() {
        if index > 0 {
            cursor_x += ACTION_ITEM_GAP;
        }
        let (item_w, _item_h, keycap_w, keycap_h, label_h) =
            overlay_action_item_metrics(render_state, font, key, label);
        let key_rect = Rectangle::<i32, Physical>::new(
            (cursor_x, y + (row_h - keycap_h) / 2).into(),
            (keycap_w, keycap_h).into(),
        );
        draw_overlay_chip_without_shadow(
            frame,
            render_state,
            visuals,
            key_rect,
            10.0,
            visuals.palette.key_fill.alpha(0.98 * alpha),
            false,
            damage,
            alpha,
        )?;
        let (key_w, key_h) = ui_text_size_in(render_state, font, key, ACTION_KEY_SCALE);
        draw_ui_text_in(
            frame,
            render_state,
            font,
            key_rect.loc.x + (key_rect.size.w - key_w) / 2,
            key_rect.loc.y + (key_rect.size.h - key_h) / 2,
            key,
            ACTION_KEY_SCALE,
            visuals.palette.key_text.alpha(alpha),
            damage,
        )?;
        draw_ui_text_in(
            frame,
            render_state,
            font,
            cursor_x + keycap_w + ACTION_LABEL_GAP,
            y + (row_h - label_h) / 2,
            label,
            ACTION_LABEL_SCALE,
            visuals.palette.subtext.alpha(alpha * 0.96),
            damage,
        )?;
        cursor_x += item_w;
    }
    Ok(())
}
