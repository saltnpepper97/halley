use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{
    pick_bool, pick_expanded_placement_strategy, pick_f32, pick_find_empty_mode,
    pick_landmark_placement_strategy, pick_normal_blocker_policy, pick_pan_to_new_mode,
    pick_pinned_blocker_policy, pick_u64,
};

pub(crate) fn load_placement_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.placement.expanded.strategy = pick_expanded_placement_strategy(
        cfg,
        &[
            "placement.expanded.strategy",
            "placement.expanded.spawn-placement",
            "placement.expanded.spawn_placement",
        ],
        out.placement.expanded.strategy,
    );
    out.placement.expanded.fallback = pick_expanded_placement_strategy(
        cfg,
        &["placement.expanded.fallback"],
        out.placement.expanded.fallback,
    );
    out.placement.expanded.find_empty_mode = pick_find_empty_mode(
        cfg,
        &[
            "placement.expanded.find-empty-mode",
            "placement.expanded.find_empty_mode",
        ],
        out.placement.expanded.find_empty_mode,
    );

    out.placement.landmarks.strategy = pick_landmark_placement_strategy(
        cfg,
        &["placement.landmarks.strategy"],
        out.placement.landmarks.strategy,
    );
    out.placement.landmarks.normal_blocker = pick_normal_blocker_policy(
        cfg,
        &[
            "placement.landmarks.normal-blocker",
            "placement.landmarks.normal_blocker",
        ],
        out.placement.landmarks.normal_blocker,
    );
    out.placement.landmarks.pinned_blocker = pick_pinned_blocker_policy(
        cfg,
        &[
            "placement.landmarks.pinned-blocker",
            "placement.landmarks.pinned_blocker",
        ],
        out.placement.landmarks.pinned_blocker,
    );

    out.placement.reveal.enabled = pick_bool(
        cfg,
        &["placement.reveal.enabled"],
        out.placement.reveal.enabled,
    );
    out.placement.reveal.max_pan_px = pick_f32(
        cfg,
        &["placement.reveal.max-pan-px", "placement.reveal.max_pan_px"],
        out.placement.reveal.max_pan_px,
    );
    out.placement.reveal.animation_ms = pick_u64(
        cfg,
        &[
            "placement.reveal.animation-ms",
            "placement.reveal.animation_ms",
        ],
        out.placement.reveal.animation_ms,
    );

    out.pan_to_new = pick_pan_to_new_mode(
        cfg,
        &["placement.reveal.pan-to-new", "placement.reveal.pan_to_new"],
        out.pan_to_new,
    );
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::{ExpandedPlacementStrategy, RuntimeTuning};

    use super::load_placement_section;

    #[test]
    fn placement_section_parses_expanded_strategy() {
        let cfg = RuneConfig::from_str(
            r##"
placement:
  expanded:
    strategy "find-empty"
    fallback "center"
    find-empty-mode "best-effort"
  end
end
"##,
        )
        .expect("placement config should parse");

        let mut out = RuntimeTuning::default();
        load_placement_section(&cfg, &mut out);

        assert_eq!(
            out.placement.expanded.strategy,
            ExpandedPlacementStrategy::FindEmpty
        );
        assert_eq!(
            out.placement.expanded.fallback,
            ExpandedPlacementStrategy::Center
        );
    }
}
