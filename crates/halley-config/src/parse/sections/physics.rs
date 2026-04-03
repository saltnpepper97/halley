use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_f32};

pub(crate) fn load_physics_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.physics_enabled = pick_bool(cfg, &["physics.enabled"], out.physics_enabled);

    out.non_overlap_bump_damping =
        pick_f32(cfg, &["physics.damping"], out.non_overlap_bump_damping);
}

