use std::fmt;

use rune_cfg::RuneConfig;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BackgroundMode {
    #[default]
    None,
    Classic,
    FieldShader,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BackgroundFit {
    #[default]
    Cover,
    Contain,
    Stretch,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackgroundColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Background {
    pub mode: BackgroundMode,
    pub path: String,
    pub shader: String,
    pub fit: BackgroundFit,
    pub intensity: f32,
    pub animated: bool,
    pub color: BackgroundColor,
    pub accent_color: BackgroundColor,
}

impl Default for Background {
    fn default() -> Self {
        Self {
            mode: BackgroundMode::None,
            path: String::new(),
            shader: "space".to_string(),
            fit: BackgroundFit::Cover,
            intensity: 1.0,
            animated: false,
            color: BackgroundColor {
                r: 0x18 as f32 / 255.0,
                g: 0x1a as f32 / 255.0,
                b: 0x26 as f32 / 255.0,
            },
            accent_color: BackgroundColor {
                r: 0x8f as f32 / 255.0,
                g: 0xa8 as f32 / 255.0,
                b: 0xd8 as f32 / 255.0,
            },
        }
    }
}

#[derive(Debug)]
pub enum BackgroundParseError {
    Rune(rune_cfg::RuneError),
    InvalidValue { path: &'static str, value: String },
}

impl fmt::Display for BackgroundParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rune(error) => write!(f, "{error}"),
            Self::InvalidValue { path, value } => {
                write!(f, "invalid value {value:?} for {path}")
            }
        }
    }
}

impl std::error::Error for BackgroundParseError {}

impl From<rune_cfg::RuneError> for BackgroundParseError {
    fn from(value: rune_cfg::RuneError) -> Self {
        Self::Rune(value)
    }
}

pub fn parse_background(config: &RuneConfig) -> Result<Background, BackgroundParseError> {
    let defaults = Background::default();
    let mode = match optional_string(config, &["wallpaper.mode", "background.mode", "gesso.mode"])?
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("none") => BackgroundMode::None,
        Some("classic") => BackgroundMode::Classic,
        Some("field-shader" | "field_shader") => BackgroundMode::FieldShader,
        Some(value) => {
            return Err(BackgroundParseError::InvalidValue {
                path: "wallpaper.mode",
                value: value.to_string(),
            });
        }
    };
    let fit = match optional_string(config, &["wallpaper.fit", "background.fit", "gesso.fit"])?
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("cover") => BackgroundFit::Cover,
        Some("contain") => BackgroundFit::Contain,
        Some("stretch") => BackgroundFit::Stretch,
        Some(value) => {
            return Err(BackgroundParseError::InvalidValue {
                path: "wallpaper.fit",
                value: value.to_string(),
            });
        }
    };
    let intensity = optional_f32(
        config,
        &[
            "wallpaper.intensity",
            "background.intensity",
            "gesso.intensity",
        ],
    )?
    .unwrap_or(1.0);
    if !intensity.is_finite() || intensity < 0.0 {
        return Err(BackgroundParseError::InvalidValue {
            path: "wallpaper.intensity",
            value: intensity.to_string(),
        });
    }

    Ok(Background {
        mode,
        path: optional_string(config, &["wallpaper.path", "background.path", "gesso.path"])?
            .unwrap_or_default(),
        shader: optional_string(
            config,
            &["wallpaper.shader", "background.shader", "gesso.shader"],
        )?
        .unwrap_or(defaults.shader),
        fit,
        intensity,
        animated: optional_bool(
            config,
            &[
                "wallpaper.animated",
                "background.animated",
                "gesso.animated",
            ],
        )?
        .unwrap_or(false),
        color: optional_color(
            config,
            &[
                "wallpaper.colour",
                "wallpaper.color",
                "background.colour",
                "background.color",
                "gesso.colour",
                "gesso.color",
            ],
            defaults.color,
            "wallpaper.colour",
        )?,
        accent_color: optional_color(
            config,
            &[
                "wallpaper.accent-colour",
                "wallpaper.accent_colour",
                "wallpaper.accent-color",
                "wallpaper.accent_color",
                "background.accent-colour",
                "background.accent_colour",
                "background.accent-color",
                "background.accent_color",
                "gesso.accent-colour",
                "gesso.accent_colour",
                "gesso.accent-color",
                "gesso.accent_color",
            ],
            defaults.accent_color,
            "wallpaper.accent-colour",
        )?,
    })
}

fn optional_string(
    config: &RuneConfig,
    paths: &[&str],
) -> Result<Option<String>, rune_cfg::RuneError> {
    for path in paths {
        if let Some(value) = config.get_optional::<String>(path)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn optional_f32(config: &RuneConfig, paths: &[&str]) -> Result<Option<f32>, rune_cfg::RuneError> {
    for path in paths {
        if let Some(value) = config.get_optional::<f32>(path)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn optional_bool(config: &RuneConfig, paths: &[&str]) -> Result<Option<bool>, rune_cfg::RuneError> {
    for path in paths {
        if let Some(value) = config.get_optional::<bool>(path)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn optional_color(
    config: &RuneConfig,
    paths: &[&str],
    default: BackgroundColor,
    canonical_path: &'static str,
) -> Result<BackgroundColor, BackgroundParseError> {
    let Some(value) = optional_string(config, paths)? else {
        return Ok(default);
    };
    parse_hex_rgb(&value).ok_or(BackgroundParseError::InvalidValue {
        path: canonical_path,
        value,
    })
}

fn parse_hex_rgb(value: &str) -> Option<BackgroundColor> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(BackgroundColor {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_wallpaper_section() {
        let config = RuneConfig::from_str(
            r##"
wallpaper:
  mode "field_shader"
  shader "space"
  fit "contain"
  colour "#202233"
  accent-color "#9db7ee"
  intensity 1.35
  animated true
end
"##,
        )
        .unwrap();
        let background = parse_background(&config).unwrap();
        assert_eq!(background.mode, BackgroundMode::FieldShader);
        assert_eq!(background.fit, BackgroundFit::Contain);
        assert_eq!(background.color.r, 0x20 as f32 / 255.0);
        assert_eq!(background.accent_color.b, 0xee as f32 / 255.0);
        assert_eq!(background.intensity, 1.35);
        assert!(background.animated);
    }

    #[test]
    fn accepts_legacy_background_and_gesso_aliases() {
        let background = RuneConfig::from_str("background:\n  mode \"classic\"\nend\n").unwrap();
        assert_eq!(
            parse_background(&background).unwrap().mode,
            BackgroundMode::Classic
        );

        let gesso = RuneConfig::from_str("gesso:\n  mode \"field_shader\"\nend\n").unwrap();
        assert_eq!(
            parse_background(&gesso).unwrap().mode,
            BackgroundMode::FieldShader
        );
    }

    #[test]
    fn rejects_invalid_values() {
        let config = RuneConfig::from_str("wallpaper:\n  mode \"clouds\"\nend\n").unwrap();
        assert!(parse_background(&config).is_err());

        let config = RuneConfig::from_str("wallpaper:\n  colour \"white\"\nend\n").unwrap();
        assert!(parse_background(&config).is_err());
    }
}
