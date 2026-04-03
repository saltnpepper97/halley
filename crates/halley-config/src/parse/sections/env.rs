use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::merge_env_map;

pub(crate) fn load_env_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    merge_env_map(cfg, &mut out.env, "env");
}

