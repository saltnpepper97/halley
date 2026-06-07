use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{
    pick_bool, pick_i32, pick_overlay_color_mode, pick_rail_obstruction_behavior,
    pick_rail_placement, pick_rail_sizing_mode,
};

pub(crate) fn load_rail_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.rail.enabled = pick_bool(cfg, &["rail.enabled"], out.rail.enabled);
    out.rail.placement = pick_rail_placement(cfg, &["rail.placement"], out.rail.placement);
    out.rail.background_color = pick_overlay_color_mode(
        cfg,
        &[
            "rail.background-colour",
            "rail.background_colour",
            "rail.background-color",
            "rail.background_color",
            "rail.bg-colour",
            "rail.bg-color",
        ],
        out.rail.background_color,
    );
    out.rail.foreground_color = pick_overlay_color_mode(
        cfg,
        &[
            "rail.foreground-colour",
            "rail.foreground_colour",
            "rail.foreground-color",
            "rail.foreground_color",
            "rail.fg-colour",
            "rail.fg-color",
            "rail.text-colour",
            "rail.text-color",
        ],
        out.rail.foreground_color,
    );
    out.rail.divider_color = pick_overlay_color_mode(
        cfg,
        &[
            "rail.divider-colour",
            "rail.divider_colour",
            "rail.divider-color",
            "rail.divider_color",
            "rail.separator-colour",
            "rail.separator-color",
        ],
        out.rail.divider_color,
    );
    out.rail.offset_x = pick_i32(cfg, &["rail.offset-x", "rail.offset_x"], out.rail.offset_x);
    out.rail.offset_y = pick_i32(cfg, &["rail.offset-y", "rail.offset_y"], out.rail.offset_y);
    out.rail.width = pick_i32(cfg, &["rail.width"], out.rail.width);
    out.rail.height = pick_i32(cfg, &["rail.height"], out.rail.height);
    out.rail.sizing = pick_rail_sizing_mode(
        cfg,
        &["rail.sizing", "rail.sizing-mode", "rail.sizing_mode"],
        out.rail.sizing,
    );
    out.rail.icon_size = pick_i32(
        cfg,
        &["rail.icon-size", "rail.icon_size"],
        out.rail.icon_size,
    );
    out.rail.gap = pick_i32(cfg, &["rail.gap"], out.rail.gap);
    out.rail.padding = pick_i32(cfg, &["rail.padding"], out.rail.padding);
    out.rail.radius = pick_i32(cfg, &["rail.radius", "rail.radius-px"], out.rail.radius);
    out.rail.pinned_separator = pick_bool(
        cfg,
        &["rail.pinned-separator", "rail.pinned_separator"],
        out.rail.pinned_separator,
    );
    out.rail.obstruction = pick_rail_obstruction_behavior(
        cfg,
        &[
            "rail.obstruction",
            "rail.obstruction-behavior",
            "rail.obstruction_behavior",
        ],
        out.rail.obstruction,
    );
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::{
        OverlayColorMode, RailObstructionBehavior, RailPlacement, RailSizingMode, RuntimeTuning,
    };

    use super::load_rail_section;

    #[test]
    fn rail_section_parses_vertical_left_config() {
        let cfg = RuneConfig::from_str(
            r##"
rail:
  enabled true
  placement "left"
  background-colour "#101014"
  foreground-colour "light"
  divider-colour "#d65d26"
  offset-x 12
  offset-y 4
  width 56
  height 0
  sizing "grow-to-content"
  icon-size 34
  gap 7
  padding 9
  radius 18
  pinned-separator false
  obstruction "auto-hide"
end
"##,
        )
        .expect("rail config should parse");

        let mut out = RuntimeTuning::default();
        load_rail_section(&cfg, &mut out);

        assert!(out.rail.enabled);
        assert_eq!(out.rail.placement, RailPlacement::Left);
        assert_eq!(
            out.rail.background_color,
            OverlayColorMode::Fixed {
                r: 0x10 as f32 / 255.0,
                g: 0x10 as f32 / 255.0,
                b: 0x14 as f32 / 255.0,
            }
        );
        assert_eq!(out.rail.foreground_color, OverlayColorMode::Light);
        assert_eq!(
            out.rail.divider_color,
            OverlayColorMode::Fixed {
                r: 0xd6 as f32 / 255.0,
                g: 0x5d as f32 / 255.0,
                b: 0x26 as f32 / 255.0,
            }
        );
        assert_eq!(out.rail.offset_x, 12);
        assert_eq!(out.rail.offset_y, 4);
        assert_eq!(out.rail.width, 56);
        assert_eq!(out.rail.height, 0);
        assert_eq!(out.rail.sizing, RailSizingMode::GrowToContent);
        assert_eq!(out.rail.icon_size, 34);
        assert_eq!(out.rail.gap, 7);
        assert_eq!(out.rail.padding, 9);
        assert_eq!(out.rail.radius, 18);
        assert!(!out.rail.pinned_separator);
        assert_eq!(out.rail.obstruction, RailObstructionBehavior::AutoHide);
    }
}
