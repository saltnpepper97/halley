use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::pick_f32;
use super::super::viewport::parse_viewport_outputs;

pub(crate) fn load_viewport_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.viewport_center.x = pick_f32(
        cfg,
        &["viewport.center-x", "viewport.center_x"],
        out.viewport_center.x,
    );
    out.viewport_center.y = pick_f32(
        cfg,
        &["viewport.center-y", "viewport.center_y"],
        out.viewport_center.y,
    );

    out.viewport_size.x = pick_f32(
        cfg,
        &["viewport.size-w", "viewport.size_w"],
        out.viewport_size.x,
    );
    out.viewport_size.y = pick_f32(
        cfg,
        &["viewport.size-h", "viewport.size_h"],
        out.viewport_size.y,
    );

    out.tty_viewports = parse_viewport_outputs(cfg, "viewport");

    if let Some(primary) = out.tty_viewports.iter().find(|viewport| viewport.enabled) {
        out.viewport_size.x = primary.width as f32;
        out.viewport_size.y = primary.height as f32;

        out.viewport_center.x = primary.offset_x as f32 + primary.width as f32 / 2.0;
        out.viewport_center.y = primary.offset_y as f32 + primary.height as f32 / 2.0;
    }
}
