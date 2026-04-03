use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_close_restore_pan_mode, pick_f32, pick_pan_to_new_mode};

pub(crate) fn load_field_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.non_overlap_gap_px = pick_f32(cfg, &["field.gap", "field.gap-px"], out.non_overlap_gap_px);
    out.pan_to_new = pick_pan_to_new_mode(
        cfg,
        &["field.pan-to-new", "field.pan_to_new"],
        out.pan_to_new,
    );
    out.close_restore_focus = pick_bool(
        cfg,
        &["field.close-restore-focus", "field.close_restore_focus"],
        out.close_restore_focus,
    );
    out.close_restore_pan = pick_close_restore_pan_mode(
        cfg,
        &["field.close-restore-pan", "field.close_restore_pan"],
        out.close_restore_pan,
    );
    out.zoom_enabled = pick_bool(
        cfg,
        &["field.zoom.enabled", "field.zoom_enabled"],
        out.zoom_enabled,
    );
    out.zoom_step = pick_f32(cfg, &["field.zoom.step", "field.zoom_step"], out.zoom_step);
    out.zoom_min = pick_f32(cfg, &["field.zoom.min", "field.zoom_min"], out.zoom_min);
    out.zoom_max = pick_f32(cfg, &["field.zoom.max", "field.zoom_max"], out.zoom_max);
    out.zoom_smooth = pick_bool(
        cfg,
        &["field.zoom.smooth", "field.zoom_smooth"],
        out.zoom_smooth,
    );
    out.zoom_smooth_rate = pick_f32(
        cfg,
        &[
            "field.zoom.smooth-rate",
            "field.zoom.smooth_rate",
            "field.zoom_smooth_rate",
        ],
        out.zoom_smooth_rate,
    );
}

