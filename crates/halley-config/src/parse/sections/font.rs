use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_string, pick_u32};

pub(crate) fn load_font_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    if let Some(family) = pick_string(cfg, &["font.family"]) {
        let family = family.trim();
        if !family.is_empty() {
            out.font.family = family.to_string();
        }
    }
    out.font.size = pick_u32(cfg, &["font.size"], out.font.size);
}

