use std::error::Error;
use std::fmt;

use rune_cfg::RuneConfig;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ApertureMode {
    #[default]
    Normal,
    Collapsed,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Default for ClockColor {
    fn default() -> Self {
        Self {
            r: 0.96,
            g: 0.98,
            b: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClockConfig {
    pub font_family: String,
    pub font_px: u32,
    pub color: ClockColor,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            font_family: "monospace".to_string(),
            font_px: 30,
            color: ClockColor::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApertureConfig {
    pub clock: ClockConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApertureConfigError {
    message: String,
}

impl ApertureConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ApertureConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message.as_str())
    }
}

impl Error for ApertureConfigError {}

impl ApertureConfig {
    pub fn parse_str(raw: &str) -> Result<Self, ApertureConfigError> {
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }

        let cfg = RuneConfig::from_str(raw).map_err(|err| {
            ApertureConfigError::new(format!("aperture config parse error: {err}"))
        })?;

        let mut out = Self::default();
        out.clock.font_family = pick_string(
            &cfg,
            &[
                "clock.font",
                "clock.font-family",
                "clock.font_family",
                "clock.family",
            ],
        )
        .unwrap_or(out.clock.font_family)
        .trim()
        .to_string();
        if out.clock.font_family.is_empty() {
            out.clock.font_family = ClockConfig::default().font_family;
        }
        out.clock.font_px = pick_u32(
            &cfg,
            &[
                "clock.size-px",
                "clock.size_px",
                "clock.font-px",
                "clock.font_px",
            ],
            out.clock.font_px,
        )
        .max(1);
        out.clock.color = pick_color(&cfg, &["clock.colour", "clock.color"], out.clock.color);

        Ok(out)
    }
}

fn pick_u32(cfg: &RuneConfig, paths: &[&str], default: u32) -> u32 {
    for path in paths {
        if let Ok(Some(value)) = cfg.get_optional::<u32>(path) {
            return value;
        }
    }
    default
}

fn pick_string(cfg: &RuneConfig, paths: &[&str]) -> Option<String> {
    for path in paths {
        if let Ok(Some(value)) = cfg.get_optional::<String>(path) {
            return Some(value);
        }
    }
    None
}

fn pick_color(cfg: &RuneConfig, paths: &[&str], default: ClockColor) -> ClockColor {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    parse_hex_rgb(raw.trim().trim_matches('"')).unwrap_or(default)
}

fn parse_hex_rgb(value: &str) -> Option<ClockColor> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    let expanded = match hex.len() {
        3 => {
            let mut out = String::with_capacity(6);
            for ch in hex.chars() {
                out.push(ch);
                out.push(ch);
            }
            out
        }
        6 => hex.to_string(),
        _ => return None,
    };

    let r = u8::from_str_radix(&expanded[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&expanded[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&expanded[4..6], 16).ok()? as f32 / 255.0;
    Some(ClockColor { r, g, b })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_uses_defaults() {
        assert_eq!(
            ApertureConfig::parse_str("   ").expect("default"),
            ApertureConfig::default()
        );
    }

    #[test]
    fn parses_clock_style_overrides() {
        let cfg = ApertureConfig::parse_str(
            r##"
clock:
  font "Iosevka"
  size-px 34
  colour "#d65d26"
end
"##,
        )
        .expect("parsed");

        assert_eq!(cfg.clock.font_family, "Iosevka");
        assert_eq!(cfg.clock.font_px, 34);
        assert_eq!(
            cfg.clock.color,
            ClockColor {
                r: 214.0 / 255.0,
                g: 93.0 / 255.0,
                b: 38.0 / 255.0
            }
        );
    }
}
