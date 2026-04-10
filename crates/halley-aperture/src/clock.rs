use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local, Timelike};

use crate::config::{ApertureConfig, ApertureMode};
use crate::geometry::{Point, Rect, Size};

const HIDDEN_SLIDE_PX: f32 = 12.0;
const NORMAL_MARGIN_PX: f32 = 18.0;
const COLLAPSED_EDGE_PADDING_PX: f32 = 2.0;
const MIN_COLLAPSED_FONT_PX: u32 = 12;
const COLLAPSED_FONT_SCALE: f32 = 0.56;
const SNAP_EPSILON: f32 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PresentationState {
    pub alpha: f32,
    pub font_px: f32,
    pub edge_padding_px: f32,
    pub hidden_mix: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClockSnapshot {
    pub text: String,
    pub font_family: String,
    pub font_px: u32,
    pub alpha: f32,
    pub bounds: Rect,
    pub text_origin: Point,
}

#[derive(Clone, Debug)]
pub struct ApertureRuntime {
    config: ApertureConfig,
    target_mode: ApertureMode,
    presentation: PresentationState,
    clock_text: String,
}

#[derive(Clone, Copy)]
struct ModeTargets {
    alpha: f32,
    font_px: f32,
    edge_padding_px: f32,
    hidden_mix: f32,
}

impl ApertureRuntime {
    pub fn new(config: ApertureConfig) -> Self {
        let targets = mode_targets(&config, ApertureMode::Normal);
        let mut out = Self {
            config,
            target_mode: ApertureMode::Normal,
            presentation: PresentationState {
                alpha: targets.alpha,
                font_px: targets.font_px,
                edge_padding_px: targets.edge_padding_px,
                hidden_mix: targets.hidden_mix,
            },
            clock_text: String::new(),
        };
        out.refresh_clock_text(SystemTime::now());
        out
    }

    pub fn config(&self) -> &ApertureConfig {
        &self.config
    }

    pub fn target_mode(&self) -> ApertureMode {
        self.target_mode
    }

    pub fn presentation(&self) -> PresentationState {
        self.presentation
    }

    pub fn apply_config(&mut self, config: ApertureConfig) {
        self.config = config;
    }

    pub fn set_mode(&mut self, mode: ApertureMode) {
        self.target_mode = mode;
    }

    pub fn jump_to_mode(&mut self, mode: ApertureMode) {
        self.target_mode = mode;
        let targets = mode_targets(&self.config, mode);
        self.presentation = PresentationState {
            alpha: targets.alpha,
            font_px: targets.font_px,
            edge_padding_px: targets.edge_padding_px,
            hidden_mix: targets.hidden_mix,
        };
    }

    pub fn update(&mut self, dt: Duration, now: SystemTime) {
        self.refresh_clock_text(now);

        let targets = mode_targets(&self.config, self.target_mode);
        let duration_s = 0.18;
        self.presentation.alpha =
            advance_toward(self.presentation.alpha, targets.alpha, dt, duration_s);
        self.presentation.font_px =
            advance_toward(self.presentation.font_px, targets.font_px, dt, duration_s);
        self.presentation.edge_padding_px = advance_toward(
            self.presentation.edge_padding_px,
            targets.edge_padding_px,
            dt,
            duration_s,
        );
        self.presentation.hidden_mix = advance_toward(
            self.presentation.hidden_mix,
            targets.hidden_mix,
            dt,
            duration_s,
        );
    }

    pub fn overlay_active(&self) -> bool {
        if self.presentation.alpha > 0.01 {
            return true;
        }

        let targets = mode_targets(&self.config, self.target_mode);
        self.target_mode != ApertureMode::Hidden || !presentation_near(self.presentation, targets)
    }

    pub fn snapshot<F>(
        &self,
        output_rect: Rect,
        work_area_rect: Rect,
        scale: f64,
        measure_text: F,
    ) -> Option<ClockSnapshot>
    where
        F: FnMut(u32, &str) -> Size,
    {
        self.snapshot_with_presentation(
            output_rect,
            work_area_rect,
            scale,
            self.presentation,
            measure_text,
        )
    }

    pub fn snapshot_for_mode<F>(
        &self,
        mode: ApertureMode,
        output_rect: Rect,
        work_area_rect: Rect,
        scale: f64,
        measure_text: F,
    ) -> Option<ClockSnapshot>
    where
        F: FnMut(u32, &str) -> Size,
    {
        let targets = mode_targets(&self.config, mode);
        self.snapshot_with_presentation(
            output_rect,
            work_area_rect,
            scale,
            PresentationState {
                alpha: targets.alpha,
                font_px: targets.font_px,
                edge_padding_px: targets.edge_padding_px,
                hidden_mix: targets.hidden_mix,
            },
            measure_text,
        )
    }

    fn snapshot_with_presentation<F>(
        &self,
        output_rect: Rect,
        work_area_rect: Rect,
        scale: f64,
        presentation: PresentationState,
        mut measure_text: F,
    ) -> Option<ClockSnapshot>
    where
        F: FnMut(u32, &str) -> Size,
    {
        let targets = mode_targets(&self.config, self.target_mode);
        if self.target_mode == ApertureMode::Hidden
            && presentation.alpha <= 0.005
            && presentation_near(presentation, targets)
        {
            return None;
        }

        let text = self.clock_text.clone();
        if text.is_empty() {
            return None;
        }

        let effective_scale = scale.max(0.25) as f32;
        let render_font_px = (presentation.font_px * effective_scale).round().max(1.0) as u32;
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
        let anchored_padding = presentation.edge_padding_px * effective_scale;
        let slide_y = hidden_slide(presentation.hidden_mix) * effective_scale;
        let x = work_rect.right() - side_margin - text_size.w;
        let y = work_rect.y + anchored_padding + slide_y;
        let bounds = Rect::new(x, y, text_size.w, text_size.h);

        Some(ClockSnapshot {
            text,
            font_family: self.config.clock.font_family.clone(),
            font_px: render_font_px,
            alpha: presentation.alpha.clamp(0.0, 1.0),
            bounds,
            text_origin: Point { x, y },
        })
    }

    fn refresh_clock_text(&mut self, now: SystemTime) {
        self.clock_text = format_clock_text(now);
    }
}

fn mode_targets(config: &ApertureConfig, mode: ApertureMode) -> ModeTargets {
    match mode {
        ApertureMode::Normal => ModeTargets {
            alpha: 1.0,
            font_px: config.clock.font_px.max(1) as f32,
            edge_padding_px: NORMAL_MARGIN_PX,
            hidden_mix: 0.0,
        },
        ApertureMode::Collapsed => ModeTargets {
            alpha: 1.0,
            font_px: collapsed_font_px(config.clock.font_px) as f32,
            edge_padding_px: COLLAPSED_EDGE_PADDING_PX,
            hidden_mix: 0.0,
        },
        ApertureMode::Hidden => ModeTargets {
            alpha: 0.0,
            font_px: collapsed_font_px(config.clock.font_px) as f32,
            edge_padding_px: COLLAPSED_EDGE_PADDING_PX,
            hidden_mix: 1.0,
        },
    }
}

fn collapsed_font_px(normal_font_px: u32) -> u32 {
    ((normal_font_px.max(1) as f32) * COLLAPSED_FONT_SCALE)
        .round()
        .max(MIN_COLLAPSED_FONT_PX as f32) as u32
}

fn advance_toward(current: f32, target: f32, dt: Duration, duration_s: f32) -> f32 {
    if (current - target).abs() <= SNAP_EPSILON {
        return target;
    }

    let t = (dt.as_secs_f32() / duration_s.max(0.001)).clamp(0.0, 1.0);
    let step = if t >= 1.0 {
        1.0
    } else {
        1.0 - (1.0 - t).powf(3.0)
    };
    let next = current + (target - current) * step;
    if (next - target).abs() <= SNAP_EPSILON {
        target
    } else {
        next
    }
}

fn presentation_near(presentation: PresentationState, targets: ModeTargets) -> bool {
    (presentation.alpha - targets.alpha).abs() <= SNAP_EPSILON
        && (presentation.font_px - targets.font_px).abs() <= SNAP_EPSILON
        && (presentation.edge_padding_px - targets.edge_padding_px).abs() <= SNAP_EPSILON
        && (presentation.hidden_mix - targets.hidden_mix).abs() <= SNAP_EPSILON
}

fn hidden_slide(hidden_mix: f32) -> f32 {
    -HIDDEN_SLIDE_PX * hidden_mix.clamp(0.0, 1.0)
}

fn format_clock_text(now: SystemTime) -> String {
    let local: DateTime<Local> = now.into();
    format!("{:02}:{:02}", local.hour(), local.minute())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApertureConfig, ApertureMode, ClockColor};

    #[test]
    fn collapsed_mode_uses_derived_smaller_font() {
        let runtime = ApertureRuntime::new(ApertureConfig::default());
        let normal = runtime
            .snapshot_for_mode(
                ApertureMode::Normal,
                Rect::new(0.0, 0.0, 1920.0, 1080.0),
                Rect::new(0.0, 0.0, 1920.0, 1080.0),
                1.0,
                |font_px, _text| Size {
                    w: font_px as f32 * 3.0,
                    h: font_px as f32,
                },
            )
            .expect("normal");
        let collapsed = runtime
            .snapshot_for_mode(
                ApertureMode::Collapsed,
                Rect::new(0.0, 0.0, 1920.0, 1080.0),
                Rect::new(0.0, 0.0, 1920.0, 1080.0),
                1.0,
                |font_px, _text| Size {
                    w: font_px as f32 * 3.0,
                    h: font_px as f32,
                },
            )
            .expect("collapsed");

        assert!(collapsed.font_px < normal.font_px);
        assert!(collapsed.bounds.y < normal.bounds.y);
    }

    #[test]
    fn hidden_mode_stops_rendering_once_settled() {
        let mut runtime = ApertureRuntime::new(ApertureConfig::default());
        runtime.set_mode(ApertureMode::Hidden);
        for _ in 0..32 {
            runtime.update(Duration::from_millis(16), SystemTime::UNIX_EPOCH);
        }

        assert!(!runtime.overlay_active());
        assert!(
            runtime
                .snapshot(
                    Rect::new(0.0, 0.0, 1280.0, 720.0),
                    Rect::new(0.0, 0.0, 1280.0, 720.0),
                    1.0,
                    |_font_px, _text| Size { w: 90.0, h: 20.0 },
                )
                .is_none()
        );
    }

    #[test]
    fn apply_config_updates_style_without_touching_mode() {
        let mut runtime = ApertureRuntime::new(ApertureConfig::default());
        runtime.set_mode(ApertureMode::Hidden);
        let mut next = ApertureConfig::default();
        next.clock.font_family = "Iosevka".to_string();
        next.clock.color = ClockColor {
            r: 1.0,
            g: 0.0,
            b: 0.0,
        };
        runtime.apply_config(next);

        assert_eq!(runtime.target_mode(), ApertureMode::Hidden);
        assert_eq!(runtime.config().clock.font_family, "Iosevka");
    }
}
