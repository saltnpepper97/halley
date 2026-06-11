use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Local, Timelike};
use rune_cfg::RuneConfig;

const MIN_COLLAPSED_FONT_PX: u32 = 12;
const COLLAPSED_FONT_SCALE: f32 = 0.56;
const MIN_MINIMAL_FONT_PX: u32 = 12;
const MINIMAL_FONT_SCALE: f32 = 0.40;

// Per-state clock size limits (mirror of `halley-aperture` config.rs). The
// smallest state's height is what the compositor reserves, so it is clamped to
// bar-like sizes.
const CLOCK_LARGE_MIN_PX: u32 = 12;
const CLOCK_LARGE_MAX_PX: u32 = 240;
const CLOCK_MEDIUM_MIN_PX: u32 = 10;
const CLOCK_MEDIUM_MAX_PX: u32 = 200;
const CLOCK_SMALL_FONT_MIN_PX: u32 = 8;
const CLOCK_SMALL_FONT_MAX_PX: u32 = 64;
const CLOCK_SMALL_HEIGHT_MIN_PX: u32 = 14;
const CLOCK_SMALL_HEIGHT_MAX_PX: u32 = 72;
const CLOCK_SMALL_HEIGHT_PAD_PX: u32 = 8;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ApertureMode {
    #[default]
    Normal,
    Minimal,
    Hidden,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum AperturePlacement {
    #[default]
    Cursor,
    All,
    Monitor,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PeekCorner {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClockColor {
    pub(crate) r: f32,
    pub(crate) g: f32,
    pub(crate) b: f32,
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
pub(crate) struct PeekBackgroundColor {
    pub(crate) r: f32,
    pub(crate) g: f32,
    pub(crate) b: f32,
    pub(crate) a: f32,
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
pub(crate) struct ClockConfig {
    pub(crate) font_family: String,
    /// Normal ("large") state font size.
    pub(crate) font_px: u32,
    /// Collapsed ("medium") state font size.
    pub(crate) medium_px: u32,
    /// Minimal ("small") state font size.
    pub(crate) small_px: u32,
    /// Minimal ("small") state reserved bar height — the top clearance the
    /// compositor reserves during clusters/maximize. Clamped to bar-like sizes;
    /// the clock text is centered within it.
    pub(crate) small_height_px: u32,
    pub(crate) color: ClockColor,
}

impl Default for ClockConfig {
    fn default() -> Self {
        let font_px = 30;
        let small_px = minimal_font_px(font_px);
        Self {
            font_family: "monospace".to_string(),
            font_px,
            medium_px: collapsed_font_px(font_px),
            small_px,
            small_height_px: small_px + CLOCK_SMALL_HEIGHT_PAD_PX,
            color: ClockColor::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AperturePeekConfig {
    pub(crate) corner: PeekCorner,
    pub(crate) background: PeekBackgroundColor,
    pub(crate) radius_px: u32,
    pub(crate) clock: ClockConfig,
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
pub(crate) struct ApertureConfig {
    pub(crate) placement: AperturePlacement,
    pub(crate) monitor: Option<String>,
    pub(crate) peek: AperturePeekConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ApertureConfigError {
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
    pub(crate) fn parse_str(raw: &str) -> Result<Self, ApertureConfigError> {
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
            collapsed_font_px(large_px),
        )
        .clamp(CLOCK_MEDIUM_MIN_PX, CLOCK_MEDIUM_MAX_PX);
        out.peek.clock.small_px = pick_u32(
            &cfg,
            &[
                "aperture-peek.clock-small.size-px",
                "aperture-peek.clock-small.size_px",
            ],
            minimal_font_px(large_px),
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
            out.peek.clock.small_px + CLOCK_SMALL_HEIGHT_PAD_PX,
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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
}

impl Rect {
    pub(crate) const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    #[cfg(test)]
    pub(crate) fn right(self) -> f32 {
        self.x + self.w
    }

    #[cfg(test)]
    pub(crate) fn bottom(self) -> f32 {
        self.y + self.h
    }
}

/// Short TTL for the per-monitor derived-mode cache. `ApertureStatus` is a
/// pull-only IPC request handled synchronously on the render loop, so an external
/// client polling it (hardest while the mode is changing, e.g. a cluster opening)
/// would otherwise re-run the full uncached derivation every poll and stall
/// frames. Serving cached modes between recomputes caps that at ~10 Hz/monitor; a
/// ≤100 ms mode-update latency is imperceptible.
const MODE_CACHE_TTL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
pub(crate) struct ApertureRuntime {
    config: ApertureConfig,
    clock_text: String,
    mode_cache: RefCell<HashMap<String, (ApertureMode, Instant)>>,
}

impl ApertureRuntime {
    pub(crate) fn new(config: ApertureConfig) -> Self {
        let mut out = Self {
            config,
            clock_text: String::new(),
            mode_cache: RefCell::new(HashMap::new()),
        };
        out.refresh_clock_text(SystemTime::now());
        out
    }

    pub(crate) fn config(&self) -> &ApertureConfig {
        &self.config
    }

    pub(crate) fn apply_config(&mut self, config: ApertureConfig) {
        self.config = config;
        self.invalidate_mode_cache();
    }

    /// Returns the cached derived mode for `monitor` if it is younger than the TTL.
    pub(crate) fn cached_mode(&self, monitor: &str, now: Instant) -> Option<ApertureMode> {
        self.mode_cache
            .borrow()
            .get(monitor)
            .filter(|(_, at)| now.saturating_duration_since(*at) < MODE_CACHE_TTL)
            .map(|(mode, _)| *mode)
    }

    pub(crate) fn store_mode(&self, monitor: &str, mode: ApertureMode, now: Instant) {
        self.mode_cache
            .borrow_mut()
            .insert(monitor.to_string(), (mode, now));
    }

    /// Drop cached modes so the next poll re-derives immediately. Called on the
    /// discrete transitions that flip the mode (config reload, cluster
    /// enter/exit) so those are reflected without waiting out the TTL.
    pub(crate) fn invalidate_mode_cache(&self) {
        self.mode_cache.borrow_mut().clear();
    }

    fn refresh_clock_text(&mut self, now: SystemTime) {
        let local: DateTime<Local> = now.into();
        self.clock_text = format!("{:02}:{:02}", local.hour(), local.minute());
    }
}

fn collapsed_font_px(normal_font_px: u32) -> u32 {
    ((normal_font_px.max(1) as f32) * COLLAPSED_FONT_SCALE)
        .round()
        .max(MIN_COLLAPSED_FONT_PX as f32) as u32
}

pub(crate) fn minimal_font_px(normal_font_px: u32) -> u32 {
    ((normal_font_px.max(1) as f32) * MINIMAL_FONT_SCALE)
        .round()
        .max(MIN_MINIMAL_FONT_PX as f32) as u32
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
