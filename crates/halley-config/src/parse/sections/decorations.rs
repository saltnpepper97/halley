use rune_cfg::RuneConfig;

use crate::layout::{RuntimeTuning, ShadowLayerConfig};

use super::super::primitives::{
    pick_bool, pick_decoration_border_color, pick_f32, pick_i32, pick_shadow_color,
};

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

    load_shadow_layer(
        cfg,
        "decorations.shadows.window",
        &mut out.decorations.shadows.window,
    );
    load_shadow_layer(
        cfg,
        "decorations.shadows.node",
        &mut out.decorations.shadows.node,
    );
    load_shadow_layer(
        cfg,
        "decorations.shadows.overlay",
        &mut out.decorations.shadows.overlay,
    );

    out.decorations.resize_using_border = pick_bool(
        cfg,
        &["decorations.resize-using-border"],
        out.decorations.resize_using_border,
    );
}

fn load_shadow_layer(cfg: &RuneConfig, root: &str, out: &mut ShadowLayerConfig) {
    out.enabled = pick_bool(cfg, &[format!("{root}.enabled").as_str()], out.enabled);
    out.blur_radius = pick_f32(
        cfg,
        &[
            format!("{root}.blur-radius").as_str(),
            format!("{root}.blur_radius").as_str(),
        ],
        out.blur_radius,
    );
    out.spread = pick_f32(cfg, &[format!("{root}.spread").as_str()], out.spread);
    out.offset_x = pick_f32(
        cfg,
        &[
            format!("{root}.offset-x").as_str(),
            format!("{root}.offset_x").as_str(),
        ],
        out.offset_x,
    );
    out.offset_y = pick_f32(
        cfg,
        &[
            format!("{root}.offset-y").as_str(),
            format!("{root}.offset_y").as_str(),
        ],
        out.offset_y,
    );
    out.color = pick_shadow_color(
        cfg,
        &[
            format!("{root}.colour").as_str(),
            format!("{root}.color").as_str(),
        ],
        out.color,
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

  shadows:
    window:
      enabled true
      blur-radius 40.0
      spread 1.0
      offset-x 2.0
      offset-y 10.0
      colour "#22446688"
    end

    node:
      enabled false
      blur-radius 11.0
      spread 5.0
      offset-x 0.0
      offset-y 6.0
      colour "#0000002e"
    end

    overlay:
      enabled true
      blur-radius 16.0
      spread 4.0
      offset-x 0.0
      offset-y 8.0
      color "#00000038"
    end
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
        assert!(out.decorations.shadows.window.enabled);
        assert_eq!(out.decorations.shadows.window.blur_radius, 40.0);
        assert_eq!(out.decorations.shadows.window.spread, 1.0);
        assert_eq!(out.decorations.shadows.window.offset_x, 2.0);
        assert_eq!(out.decorations.shadows.window.offset_y, 10.0);
        assert_eq!(out.decorations.shadows.window.color.r, 0x22 as f32 / 255.0);
        assert_eq!(out.decorations.shadows.window.color.g, 0x44 as f32 / 255.0);
        assert_eq!(out.decorations.shadows.window.color.b, 0x66 as f32 / 255.0);
        assert_eq!(out.decorations.shadows.window.color.a, 0x88 as f32 / 255.0);
        assert!(!out.decorations.shadows.node.enabled);
        assert_eq!(out.decorations.shadows.node.blur_radius, 11.0);
        assert_eq!(out.decorations.shadows.node.color.a, 0x2e as f32 / 255.0);
        assert!(out.decorations.shadows.overlay.enabled);
        assert_eq!(out.decorations.shadows.overlay.color.a, 0x38 as f32 / 255.0);
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
