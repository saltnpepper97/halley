use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_decoration_border_color, pick_i32};

pub(crate) fn load_decorations_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.decorations.border.size_px = pick_i32(
        cfg,
        &["decorations.border.size"],
        out.decorations.border.size_px,
    );
    out.decorations.border.radius_px = pick_i32(
        cfg,
        &["decorations.border.radius"],
        out.decorations.border.radius_px,
    );
    out.decorations.border.color_focused = pick_decoration_border_color(
        cfg,
        &[
            "decorations.border.colour-focused",
            "decorations.border.colour_focused",
            "decorations.border.color-focused",
            "decorations.border.color_focused",
        ],
        out.decorations.border.color_focused,
    );
    out.decorations.border.color_unfocused = pick_decoration_border_color(
        cfg,
        &[
            "decorations.border.colour-unfocused",
            "decorations.border.colour_unfocused",
            "decorations.border.color-unfocused",
            "decorations.border.color_unfocused",
        ],
        out.decorations.border.color_unfocused,
    );

    out.decorations.secondary_border.enabled = pick_bool(
        cfg,
        &["decorations.secondary-border.enabled"],
        out.decorations.secondary_border.enabled,
    );
    out.decorations.secondary_border.size_px = pick_i32(
        cfg,
        &["decorations.secondary-border.size"],
        out.decorations.secondary_border.size_px,
    );
    out.decorations.secondary_border.gap_px = pick_i32(
        cfg,
        &["decorations.secondary-border.gap"],
        out.decorations.secondary_border.gap_px,
    );
    out.decorations.secondary_border.color_focused = pick_decoration_border_color(
        cfg,
        &[
            "decorations.secondary-border.colour-focused",
            "decorations.secondary-border.colour_focused",
            "decorations.secondary-border.color-focused",
            "decorations.secondary-border.color_focused",
        ],
        out.decorations.secondary_border.color_focused,
    );
    out.decorations.secondary_border.color_unfocused = pick_decoration_border_color(
        cfg,
        &[
            "decorations.secondary-border.colour-unfocused",
            "decorations.secondary-border.colour_unfocused",
            "decorations.secondary-border.color-unfocused",
            "decorations.secondary-border.color_unfocused",
        ],
        out.decorations.secondary_border.color_unfocused,
    );

    out.decorations.resize_using_border = pick_bool(
        cfg,
        &["decorations.resize-using-border"],
        out.decorations.resize_using_border,
    );
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::RuntimeTuning;

    use super::load_decorations_section;

    #[test]
    fn decorations_section_parses_nested_border_config() {
        let cfg = RuneConfig::from_str(
            r##"
decorations:
  border:
    size 4
    radius 7
    colour-focused "#d65d26"
    color-unfocused "#333333"
  end

  secondary-border:
    enabled true
    size 2
    gap 3
    colour-focused "#fabd2f"
    color-unfocused "#1f1f1f"
  end

  resize-using-border true
end
"##,
        )
        .expect("decorations config should parse");

        let mut out = RuntimeTuning::default();
        load_decorations_section(&cfg, &mut out);

        assert_eq!(out.decorations.border.size_px, 4);
        assert_eq!(out.decorations.border.radius_px, 7);
        assert_eq!(out.decorations.secondary_border.enabled, true);
        assert_eq!(out.decorations.secondary_border.size_px, 2);
        assert_eq!(out.decorations.secondary_border.gap_px, 3);
        assert!(out.decorations.resize_using_border);
    }

    #[test]
    fn decoration_defaults_match_runtime_defaults() {
        let out = RuntimeTuning::default();
        assert_eq!(out.decorations.border.size_px, 3);
        assert_eq!(out.decorations.border.radius_px, 0);
        assert!(!out.decorations.secondary_border.enabled);
        assert_eq!(out.decorations.secondary_border.size_px, 1);
        assert_eq!(out.decorations.secondary_border.gap_px, 2);
        assert!(!out.decorations.resize_using_border);
    }
}
