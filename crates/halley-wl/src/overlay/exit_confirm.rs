use std::error::Error;

use smithay::{
    backend::renderer::{Color32F, gles::GlesFrame},
    utils::{Physical, Rectangle},
};

use crate::render::draw_primitives::draw_rect;
use crate::render::state::RenderState;
use crate::text::{draw_ui_text_in, ui_text_size_in};

use super::{
    ACTION_ROW_GAP_Y, BANNER_EDGE_PAD, EXIT_CONFIRM_MAX_WIDTH_PAD, EXIT_CONFIRM_MIN_WIDTH,
    EXIT_CONFIRM_PAD_X, EXIT_CONFIRM_PAD_Y, EXIT_CONFIRM_TITLE, EXIT_CONFIRM_TITLE_SCALE,
    ExitConfirmOverlaySnapshot, OverlayVisuals, draw_overlay_action_row, draw_overlay_chip,
    overlay_action_row_size, overlay_text_mix,
};

pub(super) fn draw_exit_confirmation(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    font: &halley_config::FontConfig,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
    exit_confirm: &ExitConfirmOverlaySnapshot,
) -> Result<(), Box<dyn Error>> {
    let text_mix = overlay_text_mix(exit_confirm.mix);
    let actions = [("Enter", "leave"), ("Esc", "cancel")];
    draw_rect(
        frame,
        0,
        0,
        screen_w.max(1),
        screen_h.max(1),
        Color32F::new(0.02, 0.03, 0.05, 0.62 * exit_confirm.mix),
        damage,
    )?;

    let (title_w, title_h) = ui_text_size_in(
        render_state,
        font,
        EXIT_CONFIRM_TITLE,
        EXIT_CONFIRM_TITLE_SCALE,
    );
    let (actions_w, actions_h) = overlay_action_row_size(render_state, font, &actions);
    let rect_w = (title_w.max(actions_w) + EXIT_CONFIRM_PAD_X * 2).clamp(
        EXIT_CONFIRM_MIN_WIDTH,
        (screen_w - EXIT_CONFIRM_MAX_WIDTH_PAD).max(EXIT_CONFIRM_MIN_WIDTH),
    );
    let rect_h = (EXIT_CONFIRM_PAD_Y * 2 + title_h + ACTION_ROW_GAP_Y + actions_h).max(72);
    let rect_x = ((screen_w - rect_w) / 2).max(BANNER_EDGE_PAD);
    let rect_y = ((screen_h - rect_h) / 2).max(BANNER_EDGE_PAD);
    let rect = Rectangle::<i32, Physical>::new((rect_x, rect_y).into(), (rect_w, rect_h).into());

    draw_overlay_chip(
        frame,
        render_state,
        visuals,
        rect,
        18.0,
        visuals.palette.fill.alpha(0.97 * exit_confirm.mix),
        true,
        damage,
        exit_confirm.mix,
    )?;
    draw_ui_text_in(
        frame,
        render_state,
        font,
        rect.loc.x + EXIT_CONFIRM_PAD_X,
        rect.loc.y + EXIT_CONFIRM_PAD_Y,
        EXIT_CONFIRM_TITLE,
        EXIT_CONFIRM_TITLE_SCALE,
        visuals.palette.text.alpha(text_mix),
        damage,
    )?;
    draw_overlay_action_row(
        frame,
        render_state,
        visuals,
        font,
        rect.loc.x + EXIT_CONFIRM_PAD_X,
        rect.loc.y + EXIT_CONFIRM_PAD_Y + title_h + ACTION_ROW_GAP_Y,
        &actions,
        damage,
        text_mix,
    )?;
    Ok(())
}
