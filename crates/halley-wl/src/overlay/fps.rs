use std::error::Error;
use std::time::Instant;

use smithay::{
    backend::renderer::gles::GlesFrame,
    utils::{Physical, Rectangle},
};

use crate::compositor::root::Halley;
use crate::text::{draw_ui_text_in, ui_text_size_in};

use super::{OverlayVisuals, draw_overlay_chip, resolve_overlay_visuals};

const FPS_EDGE_PAD: i32 = 20;
const FPS_PAD_X: i32 = 18;
const FPS_PAD_Y: i32 = 10;
const FPS_SCALE: i32 = 3;
const FPS_CORNER_RADIUS: f32 = 14.0;

pub(crate) fn draw_debug_fps_overlay(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    damage: Rectangle<i32, Physical>,
    now: Instant,
) -> Result<(), Box<dyn Error>> {
    if !st.runtime.tuning.debug.overlay_fps {
        return Ok(());
    }

    let monitor = st.model.monitor_state.current_monitor.clone();
    let fps = st
        .ui
        .render_state
        .sample_fps_for_monitor(monitor.as_str(), now);
    let label = format!("{:.0} FPS", fps.clamp(0.0, 999.0));
    let visuals = fps_overlay_visuals(&st.runtime.tuning);
    let (text_w, text_h) = ui_text_size_in(
        &st.ui.render_state,
        &st.runtime.tuning.font,
        label.as_str(),
        FPS_SCALE,
    );
    let rect = Rectangle::<i32, Physical>::new(
        (FPS_EDGE_PAD, FPS_EDGE_PAD).into(),
        (
            text_w.saturating_add(FPS_PAD_X * 2).max(1),
            text_h.saturating_add(FPS_PAD_Y * 2).max(1),
        )
            .into(),
    );
    draw_overlay_chip(
        frame,
        &st.ui.render_state,
        &visuals,
        rect,
        FPS_CORNER_RADIUS,
        visuals.palette.fill.alpha(0.88),
        true,
        damage,
        1.0,
    )?;
    draw_ui_text_in(
        frame,
        &st.ui.render_state,
        &st.runtime.tuning.font,
        rect.loc.x + FPS_PAD_X,
        rect.loc.y + FPS_PAD_Y,
        label.as_str(),
        FPS_SCALE,
        visuals.palette.text.alpha(1.0),
        damage,
    )?;
    Ok(())
}

fn fps_overlay_visuals(tuning: &halley_config::RuntimeTuning) -> OverlayVisuals {
    let mut visuals = resolve_overlay_visuals(tuning);
    visuals.rounded = true;
    visuals
}
