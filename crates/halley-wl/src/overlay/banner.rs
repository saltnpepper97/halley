use std::error::Error;

use smithay::{
    backend::renderer::gles::GlesFrame,
    utils::{Physical, Rectangle},
};

use crate::render::state::RenderState;
use crate::text::{draw_ui_text_in, ui_text_size_in};

use super::{
    ACTION_ROW_GAP_Y, BANNER_EDGE_PAD, BANNER_GAP, BANNER_META_SCALE, BANNER_PAD_X, BANNER_PAD_Y,
    BANNER_TITLE_SCALE, OverlayBannerSnapshot, OverlayVisuals, draw_overlay_action_row,
    draw_overlay_chip, overlay_action_row_size, overlay_text_mix,
};

pub(super) fn draw_persistent_banner(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    font: &halley_config::FontConfig,
    damage: Rectangle<i32, Physical>,
    banner: &OverlayBannerSnapshot,
) -> Result<(), Box<dyn Error>> {
    let text_mix = overlay_text_mix(banner.mix);
    let actions = banner
        .actions
        .iter()
        .map(|action| (action.key.as_str(), action.label.as_str()))
        .collect::<Vec<_>>();
    let (title_w, title_h) = ui_text_size_in(
        render_state,
        font,
        banner.title.as_str(),
        BANNER_TITLE_SCALE,
    );
    let (meta_w, meta_h) = banner
        .subtitle
        .as_ref()
        .map(|text| ui_text_size_in(render_state, font, text.as_str(), BANNER_META_SCALE))
        .unwrap_or((0, 0));
    let (actions_w, actions_h) = overlay_action_row_size(render_state, font, actions.as_slice());
    let width: i32 = title_w.max(meta_w).max(actions_w) + BANNER_PAD_X * 2;
    let height: i32 = BANNER_PAD_Y * 2
        + title_h
        + if banner.subtitle.is_some() {
            BANNER_GAP + meta_h
        } else {
            0
        }
        + if actions.is_empty() {
            0
        } else {
            ACTION_ROW_GAP_Y + actions_h
        };
    let rect = Rectangle::<i32, Physical>::new(
        (BANNER_EDGE_PAD, BANNER_EDGE_PAD).into(),
        (width.max(80), height.max(30)).into(),
    );

    draw_overlay_chip(
        frame,
        render_state,
        visuals,
        rect,
        18.0,
        visuals.palette.fill.alpha(0.97 * banner.mix),
        true,
        damage,
        banner.mix,
    )?;
    let mut row_y = rect.loc.y + BANNER_PAD_Y;
    draw_ui_text_in(
        frame,
        render_state,
        font,
        rect.loc.x + BANNER_PAD_X,
        row_y,
        banner.title.as_str(),
        BANNER_TITLE_SCALE,
        visuals.palette.text.alpha(text_mix),
        damage,
    )?;
    row_y += title_h;
    if let Some(subtitle) = banner.subtitle.as_ref() {
        row_y += BANNER_GAP;
        draw_ui_text_in(
            frame,
            render_state,
            font,
            rect.loc.x + BANNER_PAD_X,
            row_y,
            subtitle.as_str(),
            BANNER_META_SCALE,
            visuals.palette.subtext.alpha(text_mix * 0.96),
            damage,
        )?;
        row_y += meta_h;
    }
    if !actions.is_empty() {
        row_y += ACTION_ROW_GAP_Y;
        draw_overlay_action_row(
            frame,
            render_state,
            visuals,
            font,
            rect.loc.x + BANNER_PAD_X,
            row_y,
            actions.as_slice(),
            damage,
            text_mix,
        )?;
    }
    Ok(())
}
