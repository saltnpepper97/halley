use std::fmt;

use rune_cfg::RuneConfig;

use crate::{OverlayColorMode, Zoom};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CloseRestorePan {
    Never,
    #[default]
    IfOffscreen,
    Always,
}

impl CloseRestorePan {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "never" => Some(Self::Never),
            "if-offscreen" => Some(Self::IfOffscreen),
            "always" => Some(Self::Always),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PinBadgeCorner {
    TopLeft,
    #[default]
    TopRight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pins {
    pub corner: PinBadgeCorner,
    pub color: OverlayColorMode,
    pub background_color: OverlayColorMode,
    pub size: f32,
}

impl Default for Pins {
    fn default() -> Self {
        Self {
            corner: PinBadgeCorner::TopRight,
            color: OverlayColorMode::Auto,
            background_color: OverlayColorMode::Auto,
            size: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field {
    pub gap: f32,
    pub pins: Pins,
    pub close_restore_focus: bool,
    pub close_restore_nodes: bool,
    pub close_restore_pan: CloseRestorePan,
    pub zoom: Zoom,
}

impl Default for Field {
    fn default() -> Self {
        Self {
            gap: 20.0,
            pins: Pins::default(),
            close_restore_focus: true,
            close_restore_nodes: false,
            close_restore_pan: CloseRestorePan::IfOffscreen,
            zoom: Zoom::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldParseError(String);

impl fmt::Display for FieldParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for FieldParseError {}

fn finite_clamp(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

fn canonical_or_legacy_f32(
    config: &RuneConfig,
    canonical: &str,
    legacy: &str,
    fallback: f32,
) -> f32 {
    config
        .get_optional(canonical)
        .ok()
        .flatten()
        .or_else(|| config.get_optional(legacy).ok().flatten())
        .unwrap_or(fallback)
}

fn canonical_or_legacy_bool(
    config: &RuneConfig,
    canonical: &str,
    legacy: &str,
    fallback: bool,
) -> bool {
    config
        .get_optional(canonical)
        .ok()
        .flatten()
        .or_else(|| config.get_optional(legacy).ok().flatten())
        .unwrap_or(fallback)
}

fn optional_string(config: &RuneConfig, paths: &[&str]) -> Result<Option<String>, FieldParseError> {
    for path in paths {
        if let Some(value) = config
            .get_optional::<String>(path)
            .map_err(|error| FieldParseError(format!("{path}: {error}")))?
        {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn parse_pin_corner(
    config: &RuneConfig,
    default: PinBadgeCorner,
) -> Result<PinBadgeCorner, FieldParseError> {
    let Some(raw) = optional_string(
        config,
        &[
            "field.pins.corner",
            "field.pins.badge-corner",
            "field.pins.badge_corner",
        ],
    )?
    else {
        return Ok(default);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "top-left" | "top_left" | "left" => Ok(PinBadgeCorner::TopLeft),
        "top-right" | "top_right" | "right" => Ok(PinBadgeCorner::TopRight),
        _ => Err(FieldParseError(format!(
            "field.pins.corner must be \"top-left\" or \"top-right\", got {raw:?}"
        ))),
    }
}

fn parse_pin_color(
    config: &RuneConfig,
    paths: &[&str],
    canonical: &str,
    default: OverlayColorMode,
) -> Result<OverlayColorMode, FieldParseError> {
    let Some(raw) = optional_string(config, paths)? else {
        return Ok(default);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(OverlayColorMode::Auto),
        "light" => Ok(OverlayColorMode::Light),
        "dark" => Ok(OverlayColorMode::Dark),
        value => parse_hex_color(value).ok_or_else(|| {
            FieldParseError(format!(
                "{canonical} must be \"auto\", \"light\", \"dark\", or a hex color, got {raw:?}"
            ))
        }),
    }
}

fn parse_hex_color(value: &str) -> Option<OverlayColorMode> {
    let hex = value.strip_prefix('#')?;
    let expand = |value: &str| u8::from_str_radix(value, 16).ok().map(|value| value * 17);
    let (r, g, b, a) = match hex.len() {
        3 => (
            expand(&hex[0..1])?,
            expand(&hex[1..2])?,
            expand(&hex[2..3])?,
            255,
        ),
        4 => (
            expand(&hex[0..1])?,
            expand(&hex[1..2])?,
            expand(&hex[2..3])?,
            expand(&hex[3..4])?,
        ),
        6 => (
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
            255,
        ),
        8 => (
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
            u8::from_str_radix(&hex[6..8], 16).ok()?,
        ),
        _ => return None,
    };
    Some(OverlayColorMode::Fixed {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    })
}

pub fn parse_field_checked(config: &RuneConfig) -> Result<Field, FieldParseError> {
    for unsupported in ["max", "smooth", "filter", "sharpen"] {
        let path = format!("field.zoom.{unsupported}");
        if config.get_value(&path).is_ok() {
            return Err(FieldParseError(format!(
                "{path} is not supported; Halley field zoom is always smooth and capped at native scale 1.0"
            )));
        }
    }

    let defaults = Field::default();
    let pan = config
        .get_optional::<String>("field.close-restore-pan")
        .map_err(|error| FieldParseError(format!("field.close-restore-pan: {error}")))?
        .map(|value| {
            CloseRestorePan::parse(&value).ok_or_else(|| {
                FieldParseError(format!(
                    "field.close-restore-pan must be \"never\", \"if-offscreen\", or \"always\", got {value:?}"
                ))
            })
        })
        .transpose()?
        .unwrap_or(defaults.close_restore_pan);

    Ok(Field {
        gap: finite_clamp(
            canonical_or_legacy_f32(config, "field.gap", "field.gap-px", defaults.gap),
            0.0,
            256.0,
            defaults.gap,
        ),
        pins: Pins {
            corner: parse_pin_corner(config, defaults.pins.corner)?,
            color: parse_pin_color(
                config,
                &[
                    "field.pins.colour",
                    "field.pins.color",
                    "field.pins.pin-colour",
                    "field.pins.pin_color",
                    "field.pins.pin-color",
                ],
                "field.pins.colour",
                defaults.pins.color,
            )?,
            background_color: parse_pin_color(
                config,
                &[
                    "field.pins.background-colour",
                    "field.pins.background_colour",
                    "field.pins.background-color",
                    "field.pins.background_color",
                    "field.pins.bg-colour",
                    "field.pins.bg_colour",
                    "field.pins.bg-color",
                    "field.pins.bg_color",
                ],
                "field.pins.background-colour",
                defaults.pins.background_color,
            )?,
            size: finite_clamp(
                config.get_or("field.pins.size", defaults.pins.size),
                0.5,
                3.0,
                defaults.pins.size,
            ),
        },
        close_restore_focus: config
            .get_or("field.close-restore-focus", defaults.close_restore_focus),
        close_restore_nodes: config
            .get_or("field.close-restore-nodes", defaults.close_restore_nodes),
        close_restore_pan: pan,
        zoom: Zoom {
            enabled: canonical_or_legacy_bool(
                config,
                "field.zoom.enabled",
                "zoom.enabled",
                defaults.zoom.enabled,
            ),
            min: finite_clamp(
                canonical_or_legacy_f32(config, "field.zoom.min", "zoom.min", defaults.zoom.min),
                0.05,
                1.0,
                defaults.zoom.min,
            ),
            step: finite_clamp(
                canonical_or_legacy_f32(config, "field.zoom.step", "zoom.step", defaults.zoom.step),
                1.001,
                8.0,
                defaults.zoom.step,
            ),
            smooth_rate: finite_clamp(
                canonical_or_legacy_f32(
                    config,
                    "field.zoom.smooth-rate",
                    "zoom.smooth-rate",
                    defaults.zoom.smooth_rate,
                ),
                0.1,
                120.0,
                defaults.zoom.smooth_rate,
            ),
        },
    })
}

pub fn parse_field(config: &RuneConfig) -> Field {
    parse_field_checked(config).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_field_values_win_per_setting() {
        let config = RuneConfig::from_str(
            r#"
zoom:
  enabled false
  min 0.2
  step 1.5
end
field:
  gap-px 80.0
  gap 24.0
  close-restore-focus false
  close-restore-nodes true
  close-restore-pan "always"
  zoom:
    enabled true
    step 1.2
    smooth-rate 7.0
  end
end
"#,
        )
        .unwrap();

        assert_eq!(
            parse_field_checked(&config).unwrap(),
            Field {
                gap: 24.0,
                pins: Pins::default(),
                close_restore_focus: false,
                close_restore_nodes: true,
                close_restore_pan: CloseRestorePan::Always,
                zoom: Zoom {
                    enabled: true,
                    min: 0.2,
                    step: 1.2,
                    smooth_rate: 7.0,
                },
            }
        );
    }

    #[test]
    fn legacy_zoom_and_gap_remain_compatible() {
        let config = RuneConfig::from_str(
            r#"
zoom:
  min 0.2
end
field:
  gap-px 18.0
end
"#,
        )
        .unwrap();
        let field = parse_field_checked(&config).unwrap();
        assert_eq!(field.gap, 18.0);
        assert_eq!(field.zoom.min, 0.2);
    }

    #[test]
    fn close_restore_nodes_defaults_to_false() {
        let config = RuneConfig::from_str("").unwrap();
        assert!(!parse_field_checked(&config).unwrap().close_restore_nodes);
    }

    #[test]
    fn pin_style_matches_old_halley_names_and_bounds() {
        let config = RuneConfig::from_str(
            r##"
field:
  pins:
    badge_corner "left"
    pin-color "#d65d26"
    bg_colour "dark"
    size 9.0
  end
end
"##,
        )
        .unwrap();
        let pins = parse_field_checked(&config).unwrap().pins;
        assert_eq!(pins.corner, PinBadgeCorner::TopLeft);
        assert_eq!(pins.background_color, OverlayColorMode::Dark);
        assert_eq!(pins.size, 3.0);
        assert_eq!(
            pins.color,
            OverlayColorMode::Fixed {
                r: 0xd6 as f32 / 255.0,
                g: 0x5d as f32 / 255.0,
                b: 0x26 as f32 / 255.0,
                a: 1.0,
            }
        );
    }
}
