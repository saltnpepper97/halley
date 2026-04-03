use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_u64};

pub(crate) fn load_trail_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.trail_history_length = pick_u64(
        cfg,
        &["trail.history-length", "trail.history_length"],
        out.trail_history_length as u64,
    ) as usize;
    out.trail_wrap = pick_bool(cfg, &["trail.wrap", "trail.wrap-history"], out.trail_wrap);
}

