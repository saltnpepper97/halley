use std::fmt;

use rune_cfg::RuneConfig;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BlurMethod {
    DualKawase,
    Gaussian,
    Box,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blur {
    pub overlays: bool,
    pub method: BlurMethod,
    pub radius: f32,
    pub passes: u32,
    pub saturation: f32,
    pub noise: f32,
}

/// Resolves whether Halley should add a full-window blur when the client did
/// not supply an explicit region. A false rule is handled by the caller before
/// protocol regions are read, because it suppresses client-requested blur too.
pub fn window_blur_enabled(rule_blur: Option<bool>, excluded: bool) -> bool {
    !excluded && rule_blur == Some(true)
}

impl Default for Blur {
    fn default() -> Self {
        Self {
            overlays: true,
            method: BlurMethod::DualKawase,
            radius: 24.0,
            passes: 3,
            saturation: 1.10,
            noise: 0.012,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowLayer {
    pub enabled: bool,
    pub blur_radius: f32,
    pub spread: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub color: ShadowColor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadows {
    pub window: ShadowLayer,
    pub node: ShadowLayer,
    pub overlay: ShadowLayer,
}

impl Default for Shadows {
    fn default() -> Self {
        Self {
            window: ShadowLayer {
                enabled: true,
                blur_radius: 8.0,
                spread: 0.0,
                offset_x: 0.0,
                offset_y: 5.0,
                color: rgba(0x05, 0x03, 0x05, 0x30),
            },
            node: ShadowLayer {
                enabled: true,
                blur_radius: 14.0,
                spread: 0.0,
                offset_x: 0.0,
                offset_y: 3.0,
                color: rgba(0x05, 0x03, 0x05, 0x24),
            },
            overlay: ShadowLayer {
                enabled: true,
                blur_radius: 24.0,
                spread: 1.0,
                offset_x: 0.0,
                offset_y: 7.0,
                color: rgba(0x05, 0x03, 0x05, 0x38),
            },
        }
    }
}

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> ShadowColor {
    ShadowColor {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Effects {
    pub blur: Blur,
    pub shadows: Shadows,
}

#[derive(Debug)]
pub enum EffectsParseError {
    Rune(rune_cfg::RuneError),
    InvalidValue {
        path: &'static str,
        value: String,
    },
    Deprecated {
        path: &'static str,
        replacement: &'static str,
    },
}

impl fmt::Display for EffectsParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rune(error) => write!(f, "{error}"),
            Self::InvalidValue { path, value } => {
                write!(f, "invalid value {value:?} for {path}")
            }
            Self::Deprecated { path, replacement } => {
                write!(f, "{path} has been removed; {replacement}")
            }
        }
    }
}

impl std::error::Error for EffectsParseError {}

impl From<rune_cfg::RuneError> for EffectsParseError {
    fn from(value: rune_cfg::RuneError) -> Self {
        Self::Rune(value)
    }
}

pub fn parse_effects(config: &RuneConfig) -> Result<Effects, EffectsParseError> {
    let defaults = Blur::default();
    reject_legacy_blur_policy(
        config,
        "effects.blur.windows",
        "use `rules.window-rule.blur` instead",
    )?;
    reject_legacy_blur_policy(
        config,
        "effects.blur.layer-shell",
        "use `rules.layer-rule.blur` instead",
    )?;
    reject_legacy_blur_policy(
        config,
        "effects.blur.enabled",
        "remove it; `rules.window-rule.blur` and `rules.layer-rule.blur` activate blur",
    )?;
    let radius = finite_clamp(
        config.get_or("effects.blur.radius", defaults.radius),
        0.0,
        128.0,
        defaults.radius,
    );
    let saturation = finite_clamp(
        config.get_or("effects.blur.saturation", defaults.saturation),
        0.0,
        4.0,
        defaults.saturation,
    );
    let noise = finite_clamp(
        config.get_or("effects.blur.noise", defaults.noise),
        0.0,
        0.25,
        defaults.noise,
    );
    Ok(Effects {
        blur: Blur {
            overlays: config.get_or("effects.blur.overlays", defaults.overlays),
            method: parse_method(config, defaults.method)?,
            radius,
            passes: config
                .get_or("effects.blur.passes", defaults.passes)
                .clamp(1, 5),
            saturation,
            noise,
        },
        shadows: parse_shadows(config)?,
    })
}

fn reject_legacy_blur_policy(
    config: &RuneConfig,
    path: &'static str,
    replacement: &'static str,
) -> Result<(), EffectsParseError> {
    if config.get_value(path).is_ok() {
        return Err(EffectsParseError::Deprecated { path, replacement });
    }
    Ok(())
}

fn parse_shadows(config: &RuneConfig) -> Result<Shadows, EffectsParseError> {
    let defaults = Shadows::default();
    Ok(Shadows {
        window: parse_shadow_layer(config, "effects.shadows.window", defaults.window)?,
        node: parse_shadow_layer(config, "effects.shadows.node", defaults.node)?,
        overlay: parse_shadow_layer(config, "effects.shadows.overlay", defaults.overlay)?,
    })
}

fn parse_shadow_layer(
    config: &RuneConfig,
    root: &'static str,
    default: ShadowLayer,
) -> Result<ShadowLayer, EffectsParseError> {
    let color_path = format!("{root}.colour");
    let color_alias = format!("{root}.color");
    let raw_color = match config.get_optional::<String>(&color_path)? {
        Some(value) => Some(value),
        None => config.get_optional::<String>(&color_alias)?,
    };
    let color = match raw_color {
        Some(value) => parse_hex_rgba(&value).ok_or(EffectsParseError::InvalidValue {
            path: "effects.shadows.*.colour",
            value,
        })?,
        None => default.color,
    };
    Ok(ShadowLayer {
        enabled: config.get_or(&format!("{root}.enabled"), default.enabled),
        blur_radius: finite_clamp(
            config.get_or(&format!("{root}.blur-radius"), default.blur_radius),
            0.0,
            128.0,
            default.blur_radius,
        ),
        spread: finite_clamp(
            config.get_or(&format!("{root}.spread"), default.spread),
            0.0,
            64.0,
            default.spread,
        ),
        offset_x: finite_clamp(
            config.get_or(&format!("{root}.offset-x"), default.offset_x),
            -256.0,
            256.0,
            default.offset_x,
        ),
        offset_y: finite_clamp(
            config.get_or(&format!("{root}.offset-y"), default.offset_y),
            -256.0,
            256.0,
            default.offset_y,
        ),
        color,
    })
}

fn parse_hex_rgba(value: &str) -> Option<ShadowColor> {
    let value = value.trim();
    let hex = value.strip_prefix('#').unwrap_or(value);
    let expanded = match hex.len() {
        3 => format!(
            "{}{}{}{}{}{}ff",
            &hex[0..1],
            &hex[0..1],
            &hex[1..2],
            &hex[1..2],
            &hex[2..3],
            &hex[2..3]
        ),
        4 => hex.chars().flat_map(|ch| [ch, ch]).collect(),
        6 => format!("{hex}ff"),
        8 => hex.to_string(),
        _ => return None,
    };
    Some(ShadowColor {
        r: u8::from_str_radix(&expanded[0..2], 16).ok()? as f32 / 255.0,
        g: u8::from_str_radix(&expanded[2..4], 16).ok()? as f32 / 255.0,
        b: u8::from_str_radix(&expanded[4..6], 16).ok()? as f32 / 255.0,
        a: u8::from_str_radix(&expanded[6..8], 16).ok()? as f32 / 255.0,
    })
}

fn parse_method(config: &RuneConfig, default: BlurMethod) -> Result<BlurMethod, EffectsParseError> {
    let Some(value) = config.get_optional::<String>("effects.blur.method")? else {
        return Ok(default);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "dual-kawase" | "dual_kawase" | "kawase" => Ok(BlurMethod::DualKawase),
        "gaussian" => Ok(BlurMethod::Gaussian),
        "box" => Ok(BlurMethod::Box),
        _ => Err(EffectsParseError::InvalidValue {
            path: "effects.blur.method",
            value,
        }),
    }
}

fn finite_clamp(value: f32, min: f32, max: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_bounds_blur_knobs() {
        let config = RuneConfig::from_str(
            r#"
effects:
  blur:
    overlays false
    method "dual-kawase"
    radius 30.0
    passes 9
    saturation 1.2
    noise 0.02
  end
end
"#,
        )
        .unwrap();
        let blur = parse_effects(&config).unwrap().blur;
        assert!(!blur.overlays);
        assert_eq!(blur.radius, 30.0);
        assert_eq!(blur.passes, 5);
        assert_eq!(blur.saturation, 1.2);
        assert_eq!(blur.noise, 0.02);
    }

    #[test]
    fn parses_shadow_layers() {
        let config = RuneConfig::from_str(
            r##"
effects:
  shadows:
    window:
      enabled false
      blur-radius 20
      spread 2
      offset-x 3
      offset-y 9
      colour "#10203040"
    end
  end
end
"##,
        )
        .unwrap();
        let shadows = parse_effects(&config).unwrap().shadows;
        assert!(!shadows.window.enabled);
        assert_eq!(shadows.window.blur_radius, 20.0);
        assert_eq!(shadows.window.spread, 2.0);
        assert_eq!(shadows.window.offset_x, 3.0);
        assert_eq!(shadows.window.offset_y, 9.0);
        assert_eq!(shadows.window.color.r, 0x10 as f32 / 255.0);
        assert_eq!(shadows.window.color.a, 0x40 as f32 / 255.0);
        assert_eq!(shadows.node, Shadows::default().node);
    }
}
