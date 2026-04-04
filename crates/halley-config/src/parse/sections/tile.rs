use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_f32, pick_u64};

pub(crate) fn load_tile_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.tile_gaps_inner_px = pick_f32(
        cfg,
        &[
            "tile.gaps-inner",
            "tile.gaps_inner",
            "tile.gap-inner",
            "tile.gap_inner",
        ],
        out.tile_gaps_inner_px,
    );
    out.tile_gaps_outer_px = pick_f32(
        cfg,
        &[
            "tile.gaps-outer",
            "tile.gaps_outer",
            "tile.gap-outer",
            "tile.gap_outer",
        ],
        out.tile_gaps_outer_px,
    );
    out.tile_new_on_top = pick_bool(
        cfg,
        &["tile.new-on-top", "tile.new_on_top"],
        out.tile_new_on_top,
    );
    out.tile_queue_show_icons = pick_bool(
        cfg,
        &[
            "tile.queue-show-icons",
            "tile.queue_show_icons",
            "tile.show-queue-icons",
            "tile.show_queue_icons",
        ],
        out.tile_queue_show_icons,
    );
    out.tile_max_stack = pick_u64(
        cfg,
        &[
            "tile.max-stack",
            "tile.max_stack",
            "tile.stack-limit",
            "field.active-windows-allowed",
            "field.active_windows_allowed",
        ],
        out.tile_max_stack as u64,
    ) as usize;
}
