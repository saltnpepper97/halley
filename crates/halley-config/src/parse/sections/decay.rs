use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::pick_u64;

pub(crate) fn load_decay_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    let active_s = pick_u64(
        cfg,
        &["decay.active-delay", "decay.active_delay"],
        out.active_outside_ring_delay_ms / 1000,
    );
    let inactive_s = pick_u64(
        cfg,
        &["decay.inactive-delay", "decay.inactive_delay"],
        out.inactive_outside_ring_delay_ms / 1000,
    );
    let docked_s = pick_u64(
        cfg,
        &[
            "decay.docked-offscreen-delay",
            "decay.docked_offscreen_delay",
        ],
        out.docked_offscreen_delay_ms / 1000,
    );

    out.active_outside_ring_delay_ms = active_s.saturating_mul(1000);
    out.inactive_outside_ring_delay_ms = inactive_s.saturating_mul(1000);
    out.docked_offscreen_delay_ms = docked_s.saturating_mul(1000);
}

