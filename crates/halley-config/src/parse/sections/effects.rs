use rune_cfg::RuneConfig;

use crate::layout::{RuntimeTuning, ShadowLayerConfig};

use super::super::primitives::{
    pick_blur_method, pick_bool, pick_client_blur_mode, pick_f32, pick_shadow_color, pick_u32,
};

pub(crate) fn load_effects_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.effects.blur.enabled = pick_bool(cfg, &["effects.blur.enabled"], out.effects.blur.enabled);
    out.effects.blur.overlays =
        pick_bool(cfg, &["effects.blur.overlays"], out.effects.blur.overlays);
    out.effects.blur.windows =
        pick_client_blur_mode(cfg, &["effects.blur.windows"], out.effects.blur.windows);
    out.effects.blur.layer_shell = pick_client_blur_mode(
        cfg,
        &["effects.blur.layer-shell", "effects.blur.layer_shell"],
        out.effects.blur.layer_shell,
    );
    out.effects.blur.method =
        pick_blur_method(cfg, &["effects.blur.method"], out.effects.blur.method);
    out.effects.blur.radius = pick_f32(cfg, &["effects.blur.radius"], out.effects.blur.radius);
    out.effects.blur.passes = pick_u32(cfg, &["effects.blur.passes"], out.effects.blur.passes);
    out.effects.blur.saturation = pick_f32(
        cfg,
        &["effects.blur.saturation"],
        out.effects.blur.saturation,
    );
    out.effects.blur.noise = pick_f32(cfg, &["effects.blur.noise"], out.effects.blur.noise);

    load_shadow_layer(
        cfg,
        "effects.shadows.window",
        &mut out.effects.shadows.window,
    );
    load_shadow_layer(cfg, "effects.shadows.node", &mut out.effects.shadows.node);
    load_shadow_layer(
        cfg,
        "effects.shadows.overlay",
        &mut out.effects.shadows.overlay,
    );
}

pub(crate) fn load_shadow_layer(cfg: &RuneConfig, root: &str, out: &mut ShadowLayerConfig) {
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

    use crate::layout::{ClientBlurMode, RuntimeTuning};

    use super::load_effects_section;

    #[test]
    fn effects_section_parses_blur_and_shadows() {
        let cfg = RuneConfig::from_str(
            r##"
effects:
  blur:
    enabled true
    overlays true
    windows "always"
    layer-shell "auto"
    method "dual-kawase"
    radius 30
    passes 4
    saturation 1.2
    noise 0.02
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
end
"##,
        )
        .expect("effects config should parse");

        let mut out = RuntimeTuning::default();
        load_effects_section(&cfg, &mut out);

        assert!(out.effects.blur.enabled);
        assert!(out.effects.blur.overlays);
        assert_eq!(out.effects.blur.windows, ClientBlurMode::Always);
        assert_eq!(out.effects.blur.layer_shell, ClientBlurMode::Auto);
        assert_eq!(out.effects.blur.radius, 30.0);
        assert_eq!(out.effects.blur.passes, 4);
        assert_eq!(out.effects.blur.saturation, 1.2);
        assert_eq!(out.effects.blur.noise, 0.02);

        assert!(out.effects.shadows.window.enabled);
        assert_eq!(out.effects.shadows.window.blur_radius, 40.0);
        assert_eq!(out.effects.shadows.window.color.a, 0x88 as f32 / 255.0);
        assert!(!out.effects.shadows.node.enabled);
        assert_eq!(out.effects.shadows.node.color.a, 0x2e as f32 / 255.0);
        assert!(out.effects.shadows.overlay.enabled);
        assert_eq!(out.effects.shadows.overlay.color.a, 0x38 as f32 / 255.0);
    }

    #[test]
    fn effects_blur_defaults_are_conservative() {
        let out = RuntimeTuning::default();
        assert!(!out.effects.blur.enabled);
        assert!(out.effects.blur.overlays);
        assert_eq!(out.effects.blur.windows, ClientBlurMode::Auto);
        assert_eq!(out.effects.blur.layer_shell, ClientBlurMode::Off);
        assert_eq!(out.effects.blur.radius, 24.0);
        assert_eq!(out.effects.blur.passes, 3);
        // Shadow defaults match the historical decorations.shadows defaults.
        assert_eq!(out.effects.shadows.window.blur_radius, 8.0);
        assert_eq!(out.effects.shadows.overlay.color.a, 0x38 as f32 / 255.0);
    }
}
