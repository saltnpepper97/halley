use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_overlay_color_mode, pick_overlay_shape};

pub(crate) fn load_overlays_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.overlay_style.background_color = pick_overlay_color_mode(
        cfg,
        &[
            "overlay.background-colour",
            "overlay.background_colour",
            "overlay.background-color",
            "overlay.background_color",
            "overlays.background-colour",
            "overlays.background_colour",
            "overlays.background-color",
            "overlays.background_color",
        ],
        out.overlay_style.background_color,
    );
    out.overlay_style.text_color = pick_overlay_color_mode(
        cfg,
        &[
            "overlay.text-colour",
            "overlay.text_colour",
            "overlay.text-color",
            "overlay.text_color",
            "overlays.text-colour",
            "overlays.text_colour",
            "overlays.text-color",
            "overlays.text_color",
        ],
        out.overlay_style.text_color,
    );
    out.overlay_style.shape = pick_overlay_shape(
        cfg,
        &["overlay.shape", "overlays.shape"],
        out.overlay_style.shape,
    );
    out.overlay_style.borders = pick_bool(
        cfg,
        &["overlay.borders", "overlays.borders"],
        out.overlay_style.borders,
    );
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::{OverlayColorMode, OverlayShape, RuntimeTuning};

    use super::load_overlays_section;

    #[test]
    fn overlays_section_parses_palette_shape_and_borders() {
        let cfg = RuneConfig::from_str(
            r##"
overlays:
  background-colour "#223344"
  text-colour "dark"
  shape "rounded"
  borders false
end
"##,
        )
        .expect("overlay config should parse");

        let mut out = RuntimeTuning::default();
        load_overlays_section(&cfg, &mut out);

        assert_eq!(
            out.overlay_style.background_color,
            OverlayColorMode::Fixed {
                r: 0x22 as f32 / 255.0,
                g: 0x33 as f32 / 255.0,
                b: 0x44 as f32 / 255.0,
            }
        );
        assert_eq!(out.overlay_style.text_color, OverlayColorMode::Dark);
        assert_eq!(out.overlay_style.shape, OverlayShape::Rounded);
        assert!(!out.overlay_style.borders);
    }

    #[test]
    fn overlay_defaults_enable_square_bordered_auto_palette() {
        let defaults = RuntimeTuning::default();
        assert_eq!(
            defaults.overlay_style.background_color,
            OverlayColorMode::Auto
        );
        assert_eq!(defaults.overlay_style.text_color, OverlayColorMode::Auto);
        assert_eq!(defaults.overlay_style.shape, OverlayShape::Square);
        assert!(defaults.overlay_style.borders);
    }
}
