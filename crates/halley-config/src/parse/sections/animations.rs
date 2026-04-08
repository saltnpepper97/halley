use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_u64, pick_window_close_animation_style};

pub(crate) fn load_animations_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.animations.enabled = pick_bool(
        cfg,
        &["animation.enabled", "animations.enabled"],
        out.animations.enabled,
    );

    out.animations.smooth_resize.enabled = pick_bool(
        cfg,
        &[
            "animation.smooth-resize.enabled",
            "animation.smooth_resize.enabled",
            "animations.smooth-resize.enabled",
            "animations.smooth_resize.enabled",
        ],
        out.animations.smooth_resize.enabled,
    );
    out.animations.smooth_resize.duration_ms = pick_u64(
        cfg,
        &[
            "animation.smooth-resize.duration-ms",
            "animation.smooth-resize.duration_ms",
            "animation.smooth_resize.duration-ms",
            "animation.smooth_resize.duration_ms",
            "animations.smooth-resize.duration-ms",
            "animations.smooth-resize.duration_ms",
            "animations.smooth_resize.duration-ms",
            "animations.smooth_resize.duration_ms",
        ],
        out.animations.smooth_resize.duration_ms,
    );

    out.animations.window_close.enabled = pick_bool(
        cfg,
        &[
            "animation.window-close.enabled",
            "animation.window_close.enabled",
            "animations.window-close.enabled",
            "animations.window_close.enabled",
        ],
        out.animations.window_close.enabled,
    );
    out.animations.window_close.duration_ms = pick_u64(
        cfg,
        &[
            "animation.window-close.duration-ms",
            "animation.window-close.duration_ms",
            "animation.window_close.duration-ms",
            "animation.window_close.duration_ms",
            "animations.window-close.duration-ms",
            "animations.window-close.duration_ms",
            "animations.window_close.duration-ms",
            "animations.window_close.duration_ms",
        ],
        out.animations.window_close.duration_ms,
    );
    out.animations.window_close.style = pick_window_close_animation_style(
        cfg,
        &[
            "animation.window-close.style",
            "animation.window_close.style",
            "animations.window-close.style",
            "animations.window_close.style",
        ],
        out.animations.window_close.style,
    );

    out.animations.window_open.enabled = pick_bool(
        cfg,
        &[
            "animation.window-open.enabled",
            "animation.window_open.enabled",
            "animations.window-open.enabled",
            "animations.window_open.enabled",
        ],
        out.animations.window_open.enabled,
    );
    out.animations.window_open.duration_ms = pick_u64(
        cfg,
        &[
            "animation.window-open.duration-ms",
            "animation.window-open.duration_ms",
            "animation.window_open.duration-ms",
            "animation.window_open.duration_ms",
            "animations.window-open.duration-ms",
            "animations.window-open.duration_ms",
            "animations.window_open.duration-ms",
            "animations.window_open.duration_ms",
        ],
        out.animations.window_open.duration_ms,
    );

    out.animations.tile.enabled = pick_bool(
        cfg,
        &["animation.tile.enabled", "animations.tile.enabled"],
        out.animations.tile.enabled,
    );
    out.animations.tile.duration_ms = pick_u64(
        cfg,
        &[
            "animation.tile.duration-ms",
            "animation.tile.duration_ms",
            "animations.tile.duration-ms",
            "animations.tile.duration_ms",
        ],
        out.animations.tile.duration_ms,
    );

    out.animations.stack.enabled = pick_bool(
        cfg,
        &["animation.stack.enabled", "animations.stack.enabled"],
        out.animations.stack.enabled,
    );
    out.animations.stack.duration_ms = pick_u64(
        cfg,
        &[
            "animation.stack.duration-ms",
            "animation.stack.duration_ms",
            "animations.stack.duration-ms",
            "animations.stack.duration_ms",
        ],
        out.animations.stack.duration_ms,
    );
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::RuntimeTuning;

    use super::load_animations_section;

    #[test]
    fn animations_section_parses_toggles_and_durations() {
        let cfg = RuneConfig::from_str(
            r#"
animations:
  enabled true
  smooth-resize:
    enabled false
    duration-ms 120
  end
  window-close:
    enabled true
    duration-ms 250
    style "shrink"
  end
  window-open:
    enabled true
    duration-ms 900
  end
  tile:
    enabled false
    duration-ms 333
  end
  stack:
    enabled true
    duration-ms 444
  end
end
"#,
        )
        .expect("animations config should parse");

        let mut out = RuntimeTuning::default();
        load_animations_section(&cfg, &mut out);

        assert!(out.animations.enabled);
        assert!(!out.animations.smooth_resize.enabled);
        assert_eq!(out.animations.smooth_resize.duration_ms, 120);
        assert!(out.animations.window_close.enabled);
        assert_eq!(out.animations.window_close.duration_ms, 250);
        assert_eq!(
            out.animations.window_close.style,
            crate::layout::WindowCloseAnimationStyle::Shrink
        );
        assert!(out.animations.window_open.enabled);
        assert_eq!(out.animations.window_open.duration_ms, 900);
        assert!(!out.animations.tile.enabled);
        assert_eq!(out.animations.tile.duration_ms, 333);
        assert!(out.animations.stack.enabled);
        assert_eq!(out.animations.stack.duration_ms, 444);
    }

    #[test]
    fn animation_defaults_match_runtime_defaults() {
        let out = RuntimeTuning::default();
        assert!(out.animations.enabled);
        assert!(out.animations.smooth_resize.enabled);
        assert_eq!(out.animations.smooth_resize.duration_ms, 90);
        assert!(out.animations.window_close.enabled);
        assert_eq!(out.animations.window_close.duration_ms, 250);
        assert_eq!(
            out.animations.window_close.style,
            crate::layout::WindowCloseAnimationStyle::Shrink
        );
        assert!(out.animations.window_open.enabled);
        assert_eq!(out.animations.window_open.duration_ms, 620);
        assert!(out.animations.tile.enabled);
        assert_eq!(out.animations.tile.duration_ms, 240);
        assert!(out.animations.stack.enabled);
        assert_eq!(out.animations.stack.duration_ms, 220);
    }
}
