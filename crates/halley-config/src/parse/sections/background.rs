use rune_cfg::RuneConfig;

use crate::layout::{BackgroundColor, BackgroundFit, BackgroundMode, RuntimeTuning};

use super::super::primitives::{parse_hex_rgb, pick_bool, pick_f32, pick_string};

pub(crate) fn load_background_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.background.mode = pick_background_mode(
        cfg,
        &["background.mode", "gesso.mode"],
        out.background.mode.clone(),
    );
    out.background.fit = pick_background_fit(
        cfg,
        &["background.fit", "gesso.fit"],
        out.background.fit.clone(),
    );
    out.background.intensity = pick_f32(
        cfg,
        &["background.intensity", "gesso.intensity"],
        out.background.intensity,
    );
    out.background.animated = pick_bool(
        cfg,
        &["background.animated", "gesso.animated"],
        out.background.animated,
    );
    out.background.color = pick_background_color(
        cfg,
        &[
            "background.colour",
            "background.color",
            "gesso.colour",
            "gesso.color",
        ],
        out.background.color,
    );
    out.background.accent_color = pick_background_color(
        cfg,
        &[
            "background.accent-colour",
            "background.accent_colour",
            "background.accent-color",
            "background.accent_color",
            "gesso.accent-colour",
            "gesso.accent_colour",
            "gesso.accent-color",
            "gesso.accent_color",
        ],
        out.background.accent_color,
    );

    if let Some(path) = pick_string(cfg, &["background.path", "gesso.path"]) {
        out.background.path = path.trim().trim_matches('"').to_string();
    }
    if let Some(shader) = pick_string(cfg, &["background.shader", "gesso.shader"]) {
        out.background.shader = shader.trim().trim_matches('"').to_string();
    }
}

fn pick_background_color(
    cfg: &RuneConfig,
    paths: &[&str],
    default: BackgroundColor,
) -> BackgroundColor {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    parse_hex_rgb(raw.trim().trim_matches('"'))
        .map(|(r, g, b)| BackgroundColor { r, g, b })
        .unwrap_or(default)
}

fn pick_background_mode(
    cfg: &RuneConfig,
    paths: &[&str],
    default: BackgroundMode,
) -> BackgroundMode {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "none" => BackgroundMode::None,
        "classic" => BackgroundMode::Classic,
        "field-shader" | "field_shader" => BackgroundMode::FieldShader,
        _ => default,
    }
}

fn pick_background_fit(cfg: &RuneConfig, paths: &[&str], default: BackgroundFit) -> BackgroundFit {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "cover" => BackgroundFit::Cover,
        "contain" => BackgroundFit::Contain,
        "stretch" => BackgroundFit::Stretch,
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::{BackgroundFit, BackgroundMode, RuntimeTuning};

    use super::load_background_section;

    #[test]
    fn background_section_parses_field_shader() {
        let cfg = RuneConfig::from_str(
            r##"
background:
  mode "field-shader"
  shader "space"
  colour "#202233"
  accent-colour "#9db7ee"
  intensity 1.35
  animated false
end
"##,
        )
        .expect("background config should parse");

        let mut out = RuntimeTuning::default();
        load_background_section(&cfg, &mut out);

        assert_eq!(out.background.mode, BackgroundMode::FieldShader);
        assert_eq!(out.background.shader, "space");
        assert_eq!(out.background.color.r, 0x20 as f32 / 255.0);
        assert_eq!(out.background.accent_color.b, 0xee as f32 / 255.0);
        assert_eq!(out.background.intensity, 1.35);
        assert!(!out.background.animated);
    }

    #[test]
    fn gesso_alias_parses_classic_background() {
        let cfg = RuneConfig::from_str(
            r##"
gesso:
  mode "classic"
  path "$env.HOME/Pictures/wallpaper.jpg"
  fit "contain"
end
"##,
        )
        .expect("gesso config should parse");

        let mut out = RuntimeTuning::default();
        load_background_section(&cfg, &mut out);

        assert_eq!(out.background.mode, BackgroundMode::Classic);
        assert!(out.background.path.ends_with("/Pictures/wallpaper.jpg"));
        assert_eq!(out.background.fit, BackgroundFit::Contain);
    }
}
