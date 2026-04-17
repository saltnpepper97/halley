use std::error::Error;
use std::fmt;
use std::time::SystemTime;

use chrono::{DateTime, Local, Timelike};
use rune_cfg::RuneConfig;

const NORMAL_MARGIN_PX: f32 = 18.0;
const COLLAPSED_EDGE_PADDING_PX: f32 = 2.0;
const MIN_COLLAPSED_FONT_PX: u32 = 12;
const COLLAPSED_FONT_SCALE: f32 = 0.56;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ApertureMode {
    #[default]
    Normal,
    Collapsed,
    Hidden,
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClockConfig {
    pub(crate) font_family: String,
    pub(crate) font_px: u32,
    pub(crate) color: ClockColor,
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
pub(crate) struct ApertureConfig {
    pub(crate) clock: ClockConfig,
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

        let cfg = RuneConfig::from_str(raw)
            .map_err(|err| ApertureConfigError::new(format!("aperture config parse error: {err}")))?;

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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Point {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Size {
    pub(crate) w: f32,
    pub(crate) h: f32,
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

    pub(crate) fn right(self) -> f32 {
        self.x + self.w
    }

    pub(crate) fn bottom(self) -> f32 {
        self.y + self.h
    }

    pub(crate) fn is_empty(self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClockSnapshot {
    pub(crate) text: String,
    pub(crate) font_family: String,
    pub(crate) font_px: u32,
    pub(crate) alpha: f32,
    pub(crate) bounds: Rect,
    pub(crate) text_origin: Point,
}

#[derive(Clone, Debug)]
pub(crate) struct ApertureRuntime {
    config: ApertureConfig,
    clock_text: String,
}

impl ApertureRuntime {
    pub(crate) fn new(config: ApertureConfig) -> Self {
        let mut out = Self {
            config,
            clock_text: String::new(),
        };
        out.refresh_clock_text(SystemTime::now());
        out
    }

    pub(crate) fn config(&self) -> &ApertureConfig {
        &self.config
    }

    pub(crate) fn apply_config(&mut self, config: ApertureConfig) {
        self.config = config;
    }

    pub(crate) fn snapshot_for_mode<F>(
        &self,
        mode: ApertureMode,
        output_rect: Rect,
        work_area_rect: Rect,
        scale: f64,
        mut measure_text: F,
    ) -> Option<ClockSnapshot>
    where
        F: FnMut(u32, &str) -> Size,
    {
        let text = self.clock_text.clone();
        if text.is_empty() {
            return None;
        }

        let effective_scale = scale.max(0.25) as f32;
        let render_font_px = match mode {
            ApertureMode::Normal => self.config.clock.font_px.max(1),
            ApertureMode::Collapsed | ApertureMode::Hidden => {
                collapsed_font_px(self.config.clock.font_px)
            }
        };
        let render_font_px = (render_font_px as f32 * effective_scale).round().max(1.0) as u32;
        let text_size = measure_text(render_font_px, text.as_str());
        if text_size.w <= 0.0 || text_size.h <= 0.0 {
            return None;
        }

        let work_rect = if work_area_rect.is_empty() {
            output_rect
        } else {
            work_area_rect
        };
        let side_margin = NORMAL_MARGIN_PX * effective_scale;
        let edge_padding = match mode {
            ApertureMode::Normal => NORMAL_MARGIN_PX,
            ApertureMode::Collapsed | ApertureMode::Hidden => COLLAPSED_EDGE_PADDING_PX,
        } * effective_scale;
        let x = work_rect.right() - side_margin - text_size.w;
        let y = work_rect.y + edge_padding;

        if mode == ApertureMode::Hidden {
            return None;
        }

        Some(ClockSnapshot {
            text,
            font_family: self.config.clock.font_family.clone(),
            font_px: render_font_px,
            alpha: 1.0,
            bounds: Rect::new(x, y, text_size.w, text_size.h),
            text_origin: Point { x, y },
        })
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
