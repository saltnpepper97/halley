use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::pick_bool;

pub(crate) fn load_debug_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.debug.overlay_fps = pick_bool(cfg, &["debug.overlay-fps"], out.debug.overlay_fps);
    out.debug.show_ring_when_resizing = pick_bool(
        cfg,
        &["debug.show-ring-when-resizing"],
        out.debug.show_ring_when_resizing,
    );
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::RuntimeTuning;

    use super::load_debug_section;

    #[test]
    fn debug_section_parses_only_debug_toggles() {
        let cfg = RuneConfig::from_str(
            r#"
debug:
  overlay-fps true
  show-ring-when-resizing false
end
"#,
        )
        .expect("debug config should parse");

        let mut out = RuntimeTuning::default();
        load_debug_section(&cfg, &mut out);

        assert!(out.debug.overlay_fps);
        assert!(!out.debug.show_ring_when_resizing);
    }
}
