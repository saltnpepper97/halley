use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::pick_f32;

pub(crate) fn load_focus_ring_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.focus_ring_rx = pick_f32(
        cfg,
        &[
            "focus-ring.rx",
            "focus-ring.radius-x",
            "focus-ring.radius_x",
        ],
        out.focus_ring_rx,
    );
    out.focus_ring_ry = pick_f32(
        cfg,
        &[
            "focus-ring.ry",
            "focus-ring.radius-y",
            "focus-ring.radius_y",
        ],
        out.focus_ring_ry,
    );

    out.focus_ring_offset_x = pick_f32(
        cfg,
        &["focus-ring.offset-x", "focus-ring.offset_x"],
        out.focus_ring_offset_x,
    );
    out.focus_ring_offset_y = pick_f32(
        cfg,
        &["focus-ring.offset-y", "focus-ring.offset_y"],
        out.focus_ring_offset_y,
    );

    out.focus_ring_rx = pick_f32(
        cfg,
        &["focus-ring.primary-rx", "focus-ring.primary_rx"],
        out.focus_ring_rx,
    );
    out.focus_ring_ry = pick_f32(
        cfg,
        &["focus-ring.primary-ry", "focus-ring.primary_ry"],
        out.focus_ring_ry,
    );
}
