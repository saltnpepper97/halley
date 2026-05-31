use std::error::Error;

use smithay::{
    backend::renderer::gles::GlesFrame,
    utils::{Physical, Rectangle},
};

use crate::text::{draw_ui_text_in, ui_text_size_in};

use super::{
    OverlayView, SELECT_MARKER_PAD_X, SELECT_MARKER_PAD_Y, SELECT_MARKER_SCALE, draw_overlay_chip,
    resolve_overlay_visuals,
};

pub(crate) fn draw_cluster_selection_markers(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let visuals = resolve_overlay_visuals(overlay.tuning);
    let selected = overlay
        .cluster_state
        .cluster_mode_selected_nodes
        .get(overlay.monitor_state.current_monitor.as_str())
        .into_iter()
        .flat_map(|nodes| nodes.iter());
    for &node_id in selected {
        let Some(node) = overlay.field.node(node_id) else {
            continue;
        };
        if !overlay.field.is_visible(node_id) || !overlay.node_visible_on_current_monitor(node_id) {
            continue;
        }
        let (sx, sy) = overlay.world_to_screen(screen_w, screen_h, node.pos.x, node.pos.y);
        let (text_w, text_h) = ui_text_size_in(
            overlay.render_state,
            &overlay.tuning.font,
            "SEL",
            SELECT_MARKER_SCALE,
        );
        let rect = Rectangle::<i32, Physical>::new(
            (
                (sx - (text_w + SELECT_MARKER_PAD_X * 2) / 2),
                (sy - (text_h + SELECT_MARKER_PAD_Y * 2) / 2),
            )
                .into(),
            (
                text_w + SELECT_MARKER_PAD_X * 2,
                text_h + SELECT_MARKER_PAD_Y * 2,
            )
                .into(),
        );
        draw_overlay_chip(
            frame,
            overlay.render_state,
            &visuals,
            rect,
            10.0,
            visuals.palette.key_fill.alpha(0.96),
            false,
            damage,
            1.0,
        )?;
        draw_ui_text_in(
            frame,
            overlay.render_state,
            &overlay.tuning.font,
            rect.loc.x + ((rect.size.w - text_w).max(0) / 2),
            rect.loc.y + (rect.size.h - text_h) / 2,
            "SEL",
            SELECT_MARKER_SCALE,
            visuals.palette.text.alpha(1.0),
            damage,
        )?;
    }
    Ok(())
}
