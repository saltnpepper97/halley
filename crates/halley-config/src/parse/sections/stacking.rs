use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::pick_u64;

pub(crate) fn load_stacking_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.stacking_max_visible = pick_u64(
        cfg,
        &[
            "stacking.max-visible",
            "stacking.max_visible",
            "stacking.visible-limit",
            "stacking.visible_limit",
        ],
        out.stacking_max_visible as u64,
    ) as usize;
}
