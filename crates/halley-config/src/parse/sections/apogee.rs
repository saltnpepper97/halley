use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_f32, pick_u32, pick_u64};

pub(crate) fn load_apogee_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.apogee.enabled = pick_bool(cfg, &["apogee.enabled"], out.apogee.enabled);
    out.apogee.live_previews = pick_bool(
        cfg,
        &["apogee.live-previews", "apogee.live_previews"],
        out.apogee.live_previews,
    );
    out.apogee.transition_ms = pick_u64(
        cfg,
        &["apogee.transition-ms", "apogee.transition_ms"],
        out.apogee.transition_ms,
    );
    out.apogee.gap = pick_f32(cfg, &["apogee.gap", "apogee.gap-px"], out.apogee.gap);
    out.apogee.max_rows = pick_u32(
        cfg,
        &["apogee.max-rows", "apogee.max_rows", "apogee.rows"],
        out.apogee.max_rows,
    )
    .clamp(1, 5);
    let _ = pick_bool(
        cfg,
        &[
            "apogee.show-collapsed-as-nodes",
            "apogee.show_collapsed_as_nodes",
        ],
        true,
    );
    out.apogee.background_dim = pick_f32(
        cfg,
        &["apogee.background-dim", "apogee.background_dim"],
        out.apogee.background_dim,
    )
    .clamp(0.0, 1.0);
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::RuntimeTuning;

    use super::load_apogee_section;

    #[test]
    fn apogee_section_parses_overview_settings() {
        let cfg = RuneConfig::from_str(
            r#"
apogee:
  enabled false
  live-previews false
  transition-ms 450
  gap 32.0
  max-rows 4
        background-dim 0.6
end
"#,
        )
        .expect("apogee config should parse");

        let mut out = RuntimeTuning::default();
        load_apogee_section(&cfg, &mut out);

        assert!(!out.apogee.enabled);
        assert!(!out.apogee.live_previews);
        assert_eq!(out.apogee.transition_ms, 450);
        assert_eq!(out.apogee.gap, 32.0);
        assert_eq!(out.apogee.max_rows, 4);
        assert_eq!(out.apogee.background_dim, 0.6);
    }

    #[test]
    fn apogee_defaults_use_snapshot_overview() {
        let out = RuntimeTuning::default();
        assert!(out.apogee.enabled);
        assert!(!out.apogee.live_previews);
        assert_eq!(out.apogee.transition_ms, 320);
        assert_eq!(out.apogee.gap, 24.0);
        assert_eq!(out.apogee.max_rows, 3);
        assert_eq!(out.apogee.background_dim, 0.85);
    }

    #[test]
    fn apogee_max_rows_is_clamped() {
        let cfg = RuneConfig::from_str("apogee:\n  max-rows 99\nend\n")
            .expect("apogee config should parse");

        let mut out = RuntimeTuning::default();
        load_apogee_section(&cfg, &mut out);

        assert_eq!(out.apogee.max_rows, 5);
    }

    #[test]
    fn apogee_background_dim_is_clamped() {
        let cfg = RuneConfig::from_str("apogee:\n  background-dim 4.0\nend\n")
            .expect("apogee config should parse");
        let mut out = RuntimeTuning::default();
        load_apogee_section(&cfg, &mut out);
        assert_eq!(out.apogee.background_dim, 1.0);
    }
}
