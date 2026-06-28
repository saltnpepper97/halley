use rune_cfg::RuneConfig;

use crate::layout::{ClusterLayoutAnimConfig, RuntimeTuning};

use super::super::primitives::{
    pick_bool, pick_f32, pick_raise_animation_trigger, pick_u64, pick_window_close_animation_style,
};

/// Load `animation(s).cluster.<layout>.{open-duration-ms,stagger-ms,close-duration-ms}`,
/// accepting both `animation`/`animations` roots and dash/underscore key spellings.
fn load_cluster_layout_anim(cfg: &RuneConfig, layout: &str, out: &mut ClusterLayoutAnimConfig) {
    let pick = |dash: &str, underscore: &str, default: u64| {
        let keys = [
            format!("animation.cluster.{layout}.{dash}"),
            format!("animations.cluster.{layout}.{dash}"),
            format!("animation.cluster.{layout}.{underscore}"),
            format!("animations.cluster.{layout}.{underscore}"),
        ];
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        pick_u64(cfg, &refs, default)
    };
    out.open_duration_ms = pick("open-duration-ms", "open_duration_ms", out.open_duration_ms);
    out.stagger_ms = pick("stagger-ms", "stagger_ms", out.stagger_ms);
    out.close_duration_ms = pick("close-duration-ms", "close_duration_ms", out.close_duration_ms);
    out.reflow_duration_ms = pick(
        "reflow-duration-ms",
        "reflow_duration_ms",
        out.reflow_duration_ms,
    );
}

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

    out.animations.maximize.enabled = pick_bool(
        cfg,
        &["animation.maximize.enabled", "animations.maximize.enabled"],
        out.animations.maximize.enabled,
    );
    out.animations.maximize.duration_ms = pick_u64(
        cfg,
        &[
            "animation.maximize.duration-ms",
            "animation.maximize.duration_ms",
            "animations.maximize.duration-ms",
            "animations.maximize.duration_ms",
        ],
        out.animations.maximize.duration_ms,
    );

    out.animations.fullscreen.enabled = pick_bool(
        cfg,
        &[
            "animation.fullscreen.enabled",
            "animations.fullscreen.enabled",
        ],
        out.animations.fullscreen.enabled,
    );
    out.animations.fullscreen.duration_ms = pick_u64(
        cfg,
        &[
            "animation.fullscreen.duration-ms",
            "animation.fullscreen.duration_ms",
            "animations.fullscreen.duration-ms",
            "animations.fullscreen.duration_ms",
        ],
        out.animations.fullscreen.duration_ms,
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

    out.animations.cluster.enabled = pick_bool(
        cfg,
        &["animation.cluster.enabled", "animations.cluster.enabled"],
        out.animations.cluster.enabled,
    );
    load_cluster_layout_anim(cfg, "tiling", &mut out.animations.cluster.tiling);
    load_cluster_layout_anim(cfg, "stacking", &mut out.animations.cluster.stacking);

    out.animations.raise.enabled = pick_bool(
        cfg,
        &["animation.raise.enabled", "animations.raise.enabled"],
        out.animations.raise.enabled,
    );
    out.animations.raise.duration_ms = pick_u64(
        cfg,
        &[
            "animation.raise.duration-ms",
            "animation.raise.duration_ms",
            "animations.raise.duration-ms",
            "animations.raise.duration_ms",
        ],
        out.animations.raise.duration_ms,
    );
    out.animations.raise.scale = pick_f32(
        cfg,
        &["animation.raise.scale", "animations.raise.scale"],
        out.animations.raise.scale,
    );
    out.animations.raise.shadow_boost = pick_f32(
        cfg,
        &[
            "animation.raise.shadow-boost",
            "animation.raise.shadow_boost",
            "animations.raise.shadow-boost",
            "animations.raise.shadow_boost",
        ],
        out.animations.raise.shadow_boost,
    );
    out.animations.raise.trigger = pick_raise_animation_trigger(
        cfg,
        &["animation.raise.trigger", "animations.raise.trigger"],
        out.animations.raise.trigger,
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
  maximize:
    enabled true
    duration-ms 345
  end
  fullscreen:
    enabled true
    duration-ms 456
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
  cluster:
    enabled false
    tiling:
      open-duration-ms 321
      stagger-ms 40
      close-duration-ms 234
      reflow-duration-ms 277
    end
    stacking:
      open-duration-ms 210
      close-duration-ms 222
    end
  end
  raise:
    enabled true
    duration-ms 155
    scale 1.04
    shadow-boost 0.25
    trigger "overlap"
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
        assert!(out.animations.maximize.enabled);
        assert_eq!(out.animations.maximize.duration_ms, 345);
        assert!(out.animations.fullscreen.enabled);
        assert_eq!(out.animations.fullscreen.duration_ms, 456);
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
        assert!(!out.animations.cluster.enabled);
        assert_eq!(out.animations.cluster.tiling.open_duration_ms, 321);
        assert_eq!(out.animations.cluster.tiling.stagger_ms, 40);
        assert_eq!(out.animations.cluster.tiling.close_duration_ms, 234);
        assert_eq!(out.animations.cluster.tiling.reflow_duration_ms, 277);
        assert_eq!(out.animations.cluster.stacking.open_duration_ms, 210);
        assert_eq!(out.animations.cluster.stacking.close_duration_ms, 222);
        assert!(out.animations.raise.enabled);
        assert_eq!(out.animations.raise.duration_ms, 155);
        assert_eq!(out.animations.raise.scale, 1.04);
        assert_eq!(out.animations.raise.shadow_boost, 0.25);
        assert_eq!(
            out.animations.raise.trigger,
            crate::layout::RaiseAnimationTrigger::Overlap
        );
    }

    #[test]
    fn animation_defaults_match_runtime_defaults() {
        let out = RuntimeTuning::default();
        assert!(out.animations.enabled);
        assert!(out.animations.smooth_resize.enabled);
        assert_eq!(out.animations.smooth_resize.duration_ms, 90);
        assert!(out.animations.fullscreen.enabled);
        assert_eq!(out.animations.fullscreen.duration_ms, 240);
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
        assert!(out.animations.cluster.enabled);
        assert_eq!(out.animations.cluster.tiling.open_duration_ms, 300);
        assert_eq!(out.animations.cluster.tiling.stagger_ms, 55);
        assert_eq!(out.animations.cluster.tiling.close_duration_ms, 420);
        assert_eq!(out.animations.cluster.tiling.reflow_duration_ms, 400);
        assert_eq!(out.animations.cluster.stacking.open_duration_ms, 240);
        assert_eq!(out.animations.cluster.stacking.close_duration_ms, 360);
        assert!(out.animations.raise.enabled);
        assert_eq!(out.animations.raise.duration_ms, 140);
        assert_eq!(out.animations.raise.scale, 1.025);
        assert_eq!(out.animations.raise.shadow_boost, 0.18);
        assert_eq!(
            out.animations.raise.trigger,
            crate::layout::RaiseAnimationTrigger::Always
        );
    }

    #[test]
    fn window_close_style_accepts_fade() {
        let cfg = RuneConfig::from_str(
            r#"
animations:
  window-close:
    style "fade"
  end
end
"#,
        )
        .expect("animations config should parse");

        let mut out = RuntimeTuning::default();
        load_animations_section(&cfg, &mut out);

        assert_eq!(
            out.animations.window_close.style,
            crate::layout::WindowCloseAnimationStyle::Fade
        );
    }
}
