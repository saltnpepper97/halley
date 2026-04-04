use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_f32};

pub(crate) fn load_bearings_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.bearings.show_distance = pick_bool(
        cfg,
        &["bearings.show-distance", "bearings.show_distance"],
        out.bearings.show_distance,
    );
    out.bearings.show_icons = pick_bool(
        cfg,
        &["bearings.show-icons", "bearings.show_icons"],
        out.bearings.show_icons,
    );
    out.bearings.fade_distance = pick_f32(
        cfg,
        &["bearings.fade-distance", "bearings.fade_distance"],
        out.bearings.fade_distance,
    );
}
