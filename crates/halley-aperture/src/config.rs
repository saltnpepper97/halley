use std::error::Error;
use std::fmt;

use rune_cfg::RuneConfig;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ApertureMode {
    #[default]
    Normal,
    Collapsed,
    Minimal,
    Hidden,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AperturePlacement {
    #[default]
    Cursor,
    All,
    Monitor,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PeekCorner {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeekBackgroundColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Default for PeekBackgroundColor {
    fn default() -> Self {
        Self {
            r: 16.0 / 255.0,
            g: 16.0 / 255.0,
            b: 20.0 / 255.0,
            a: 204.0 / 255.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClockConfig {
    pub font_family: String,
    /// Normal ("large") state font size.
    pub font_px: u32,
    /// Collapsed ("medium") state font size.
    pub medium_px: u32,
    /// Minimal ("small") state font size.
    pub small_px: u32,
    /// Minimal ("small") state reserved bar height. This is the top clearance the
    /// compositor reserves for the aperture during clusters/maximize, so it is
    /// clamped to bar-like sizes; the clock text is centered within it.
    pub small_height_px: u32,
    pub color: ClockColor,
}

impl Default for ClockConfig {
    fn default() -> Self {
        let font_px = 30;
        let small_px = clock_small_default_px(font_px);
        Self {
            font_family: "monospace".to_string(),
            font_px,
            medium_px: clock_medium_default_px(font_px),
            small_px,
            small_height_px: clock_small_height_default_px(small_px),
            color: ClockColor::default(),
        }
    }
}

// Per-state clock size limits. The smallest state's height is what the compositor
// reserves, so it is clamped to bar-like sizes.
const CLOCK_LARGE_MIN_PX: u32 = 12;
const CLOCK_LARGE_MAX_PX: u32 = 240;
const CLOCK_MEDIUM_MIN_PX: u32 = 10;
const CLOCK_MEDIUM_MAX_PX: u32 = 200;
const CLOCK_SMALL_FONT_MIN_PX: u32 = 8;
const CLOCK_SMALL_FONT_MAX_PX: u32 = 64;
const CLOCK_SMALL_HEIGHT_MIN_PX: u32 = 14;
const CLOCK_SMALL_HEIGHT_MAX_PX: u32 = 72;
const CLOCK_MEDIUM_SCALE: f32 = 0.56;
const CLOCK_SMALL_SCALE: f32 = 0.40;
const CLOCK_SMALL_HEIGHT_PAD_PX: u32 = 8;

fn clock_medium_default_px(large_px: u32) -> u32 {
    ((large_px.max(1) as f32) * CLOCK_MEDIUM_SCALE)
        .round()
        .max(CLOCK_MEDIUM_MIN_PX as f32) as u32
}

fn clock_small_default_px(large_px: u32) -> u32 {
    ((large_px.max(1) as f32) * CLOCK_SMALL_SCALE)
        .round()
        .max(CLOCK_SMALL_FONT_MIN_PX as f32) as u32
}

fn clock_small_height_default_px(small_px: u32) -> u32 {
    small_px + CLOCK_SMALL_HEIGHT_PAD_PX
}

#[derive(Clone, Debug, PartialEq)]
pub struct AperturePeekConfig {
    pub corner: PeekCorner,
    pub background: PeekBackgroundColor,
    pub radius_px: u32,
    pub clock: ClockConfig,
}

impl Default for AperturePeekConfig {
    fn default() -> Self {
        Self {
            corner: PeekCorner::TopRight,
            background: PeekBackgroundColor::default(),
            radius_px: 24,
            clock: ClockConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApertureConfig {
    pub placement: AperturePlacement,
    pub monitor: Option<String>,
    pub peek: AperturePeekConfig,
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
        out.placement = pick_string(&cfg, &["aperture.placement"])
            .as_deref()
            .and_then(parse_placement)
            .unwrap_or(out.placement);
        out.monitor = pick_string(&cfg, &["aperture.monitor"]).and_then(non_empty_trimmed);
        out.peek.corner = pick_string(&cfg, &["aperture-peek.corner"])
            .as_deref()
            .and_then(parse_corner)
            .unwrap_or(out.peek.corner);
        out.peek.background = pick_background_color(
            &cfg,
            &[
                "aperture-peek.background",
                "aperture-peek.background-colour",
                "aperture-peek.background-color",
            ],
            out.peek.background,
        );
        out.peek.radius_px = pick_u32(
            &cfg,
            &[
                "aperture-peek.radius-px",
                "aperture-peek.radius_px",
                "aperture-peek.corner-radius-px",
                "aperture-peek.corner_radius_px",
            ],
            out.peek.radius_px,
        );
        out.peek.clock.font_family = pick_string(
            &cfg,
            &[
                "aperture-peek.clock.font",
                "aperture-peek.clock.font-family",
                "aperture-peek.clock.font_family",
                "aperture-peek.clock.family",
            ],
        )
        .unwrap_or(out.peek.clock.font_family)
        .trim()
        .to_string();
        if out.peek.clock.font_family.is_empty() {
            out.peek.clock.font_family = ClockConfig::default().font_family;
        }
        // Normal / "large" font size. `clock-large.size-px` is preferred; the
        // legacy `clock.size-px` is kept for back-compat.
        let large_px = pick_u32(
            &cfg,
            &[
                "aperture-peek.clock-large.size-px",
                "aperture-peek.clock-large.size_px",
                "aperture-peek.clock.size-px",
                "aperture-peek.clock.size_px",
                "aperture-peek.clock.font-px",
                "aperture-peek.clock.font_px",
            ],
            out.peek.clock.font_px,
        )
        .clamp(CLOCK_LARGE_MIN_PX, CLOCK_LARGE_MAX_PX);
        out.peek.clock.font_px = large_px;
        // Collapsed / "medium" and Minimal / "small" font sizes. When a per-state
        // block is absent they derive from the large size (back-compat).
        out.peek.clock.medium_px = pick_u32(
            &cfg,
            &[
                "aperture-peek.clock-medium.size-px",
                "aperture-peek.clock-medium.size_px",
            ],
            clock_medium_default_px(large_px),
        )
        .clamp(CLOCK_MEDIUM_MIN_PX, CLOCK_MEDIUM_MAX_PX);
        out.peek.clock.small_px = pick_u32(
            &cfg,
            &[
                "aperture-peek.clock-small.size-px",
                "aperture-peek.clock-small.size_px",
            ],
            clock_small_default_px(large_px),
        )
        .clamp(CLOCK_SMALL_FONT_MIN_PX, CLOCK_SMALL_FONT_MAX_PX);
        // Minimal reserved bar height: clamped to bar-like sizes and never smaller
        // than its own font so the clock fits inside it.
        out.peek.clock.small_height_px = pick_u32(
            &cfg,
            &[
                "aperture-peek.clock-small.height-px",
                "aperture-peek.clock-small.height_px",
            ],
            clock_small_height_default_px(out.peek.clock.small_px),
        )
        .clamp(CLOCK_SMALL_HEIGHT_MIN_PX, CLOCK_SMALL_HEIGHT_MAX_PX)
        .max(out.peek.clock.small_px);
        out.peek.clock.color = pick_clock_color(
            &cfg,
            &["aperture-peek.clock.colour", "aperture-peek.clock.color"],
            out.peek.clock.color,
        );

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

fn non_empty_trimmed(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn parse_placement(value: &str) -> Option<AperturePlacement> {
    match value.trim().to_ascii_lowercase().as_str() {
        "cursor" => Some(AperturePlacement::Cursor),
        "all" => Some(AperturePlacement::All),
        "monitor" | "output" => Some(AperturePlacement::Monitor),
        _ => None,
    }
}

fn parse_corner(value: &str) -> Option<PeekCorner> {
    match value.trim().to_ascii_lowercase().as_str() {
        "top-left" | "top_left" => Some(PeekCorner::TopLeft),
        "top-right" | "top_right" => Some(PeekCorner::TopRight),
        "bottom-left" | "bottom_left" => Some(PeekCorner::BottomLeft),
        "bottom-right" | "bottom_right" => Some(PeekCorner::BottomRight),
        _ => None,
    }
}

fn pick_clock_color(cfg: &RuneConfig, paths: &[&str], default: ClockColor) -> ClockColor {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    parse_hex_rgb(raw.trim().trim_matches('"')).unwrap_or(default)
}

fn pick_background_color(
    cfg: &RuneConfig,
    paths: &[&str],
    default: PeekBackgroundColor,
) -> PeekBackgroundColor {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    parse_hex_rgba(raw.trim().trim_matches('"')).unwrap_or(default)
}

fn parse_hex_rgb(value: &str) -> Option<ClockColor> {
    let rgba = parse_hex_rgba(value)?;
    Some(ClockColor {
        r: rgba.r,
        g: rgba.g,
        b: rgba.b,
    })
}

fn parse_hex_rgba(value: &str) -> Option<PeekBackgroundColor> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    let expanded = match hex.len() {
        3 => {
            let mut out = String::with_capacity(8);
            for ch in hex.chars() {
                out.push(ch);
                out.push(ch);
            }
            out.push_str("ff");
            out
        }
        4 => {
            let mut out = String::with_capacity(8);
            for ch in hex.chars() {
                out.push(ch);
                out.push(ch);
            }
            out
        }
        6 => {
            let mut out = hex.to_string();
            out.push_str("ff");
            out
        }
        8 => hex.to_string(),
        _ => return None,
    };

    let r = u8::from_str_radix(&expanded[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&expanded[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&expanded[4..6], 16).ok()? as f32 / 255.0;
    let a = u8::from_str_radix(&expanded[6..8], 16).ok()? as f32 / 255.0;
    Some(PeekBackgroundColor { r, g, b, a })
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
    fn parses_split_aperture_config() {
        let cfg = ApertureConfig::parse_str(
            r##"
aperture:
  placement "monitor"
  monitor "DP-1"
end

aperture-peek:
  corner "bottom-left"
  background "#101014cc"
  radius-px 24

  clock:
    font "CommitMono Nerd Font Bold"
    size-px 80
    colour "#e8a1a7"
  end
end
"##,
        )
        .expect("parsed");

        assert_eq!(cfg.placement, AperturePlacement::Monitor);
        assert_eq!(cfg.monitor.as_deref(), Some("DP-1"));
        assert_eq!(cfg.peek.corner, PeekCorner::BottomLeft);
        assert_eq!(
            cfg.peek.background,
            PeekBackgroundColor {
                r: 16.0 / 255.0,
                g: 16.0 / 255.0,
                b: 20.0 / 255.0,
                a: 204.0 / 255.0
            }
        );
        assert_eq!(cfg.peek.radius_px, 24);
        assert_eq!(cfg.peek.clock.font_family, "CommitMono Nerd Font Bold");
        assert_eq!(cfg.peek.clock.font_px, 80);
        assert_eq!(
            cfg.peek.clock.color,
            ClockColor {
                r: 232.0 / 255.0,
                g: 161.0 / 255.0,
                b: 167.0 / 255.0
            }
        );
    }

    #[test]
    fn defaults_to_cursor_for_unknown_placement() {
        let cfg = ApertureConfig::parse_str(
            r#"
aperture:
  placement "bogus"
end
"#,
        )
        .expect("parsed");

        assert_eq!(cfg.placement, AperturePlacement::Cursor);
    }
}
