use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_f32, pick_u64};

pub(crate) fn load_dev_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.debug_tick_dump = pick_bool(cfg, &["dev.debug_tick_dump"], out.debug_tick_dump);
    out.debug_dump_every_ms = pick_u64(cfg, &["dev.debug_dump_every_ms"], out.debug_dump_every_ms);

    out.dev_enabled = pick_bool(cfg, &["dev.enabled"], out.dev_enabled);
    out.dev_show_geometry_overlay = pick_bool(
        cfg,
        &["dev.show_geometry_overlay"],
        out.dev_show_geometry_overlay,
    );
    out.dev_zoom_decay_enabled =
        pick_bool(cfg, &["dev.zoom_decay_enabled"], out.dev_zoom_decay_enabled);
    out.dev_zoom_decay_min_frac = pick_f32(
        cfg,
        &["dev.zoom_decay_min_frac"],
        out.dev_zoom_decay_min_frac,
    );

    out.dev_anim_enabled = pick_bool(cfg, &["dev.anim.enabled"], out.dev_anim_enabled);
    out.dev_anim_state_change_ms = pick_u64(
        cfg,
        &["dev.anim.state_change_ms"],
        out.dev_anim_state_change_ms,
    );
    out.dev_anim_bounce = pick_f32(cfg, &["dev.anim.bounce"], out.dev_anim_bounce);
}
