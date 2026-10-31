use std::time::Duration;

use halley_config::{WindowCloseAnimation, WindowCloseAnimationType};
use smithay::utils::{Physical, Point, Rectangle};

use super::motion::MotionSample;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CloseVisual {
    pub(crate) scale: f64,
    pub(crate) alpha: f32,
    pub(crate) progress: f64,
    /// Wall-clock 0..1. Close shaders use this; ease-in-out is only for CPU
    /// shrink/fade/retract envelopes.
    pub(crate) linear_progress: f64,
    retract_sample: Option<MotionSample>,
}

impl CloseVisual {
    pub(crate) fn destination(
        self,
        bounds: Rectangle<i32, Physical>,
        origin: Option<Point<f64, Physical>>,
    ) -> Rectangle<i32, Physical> {
        self.retract_sample
            .map(|sample| super::launch::rect(bounds, origin, sample).round())
            .unwrap_or_else(|| super::scale_rect_from_center(bounds, bounds, self.scale))
    }

    pub(crate) fn shader_geo(
        self,
        bounds: Rectangle<i32, Physical>,
        origin: Option<Point<f64, Physical>>,
    ) -> Rectangle<i32, Physical> {
        self.retract_sample
            .map(|sample| super::launch::path_rect(bounds, origin, sample).round())
            .unwrap_or(bounds)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CloseTimeline {
    started_at: Duration,
    duration: Duration,
    animation_type: WindowCloseAnimationType,
    start_alpha: f32,
    shader_pixels: bool,
}

impl CloseTimeline {
    pub(crate) fn new(
        config: WindowCloseAnimation,
        started_at: Duration,
        start_alpha: f32,
    ) -> Self {
        Self {
            started_at,
            duration: Duration::from_millis(u64::from(config.duration_ms)),
            animation_type: config.animation_type,
            start_alpha: start_alpha.clamp(0.0, 1.0),
            shader_pixels: config.custom_shader.is_some(),
        }
    }

    pub(crate) fn shader_pixels(self) -> bool {
        self.shader_pixels
    }

    pub(crate) fn visual_at(self, now: Duration) -> CloseVisual {
        let linear_progress = if self.duration.is_zero() {
            1.0
        } else {
            now.saturating_sub(self.started_at).as_secs_f64() / self.duration.as_secs_f64()
        }
        .clamp(0.0, 1.0);
        let progress = ease_in_out_cubic(linear_progress);
        match self.animation_type {
            WindowCloseAnimationType::Shrink => CloseVisual {
                scale: 1.0 - progress,
                alpha: self.start_alpha,
                progress,
                linear_progress,
                retract_sample: None,
            },
            WindowCloseAnimationType::Fade => CloseVisual {
                scale: 1.0,
                alpha: self.start_alpha * (1.0 - progress) as f32,
                progress,
                linear_progress,
                retract_sample: None,
            },
            WindowCloseAnimationType::Retract => {
                let reverse_linear = 1.0 - linear_progress;
                CloseVisual {
                    scale: 1.0,
                    alpha: self.start_alpha * super::launch::alpha(reverse_linear),
                    progress,
                    linear_progress,
                    retract_sample: Some(MotionSample {
                        linear_progress: reverse_linear,
                        linear_velocity: 0.0,
                        value: 1.0 - progress,
                        velocity: 0.0,
                    }),
                }
            }
        }
    }

    pub(crate) fn is_finished_at(self, now: Duration) -> bool {
        now.saturating_sub(self.started_at) >= self.duration
    }
}

fn ease_in_out_cubic(progress: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    if progress < 0.5 {
        4.0 * progress.powi(3)
    } else {
        1.0 - (-2.0 * progress + 2.0).powi(3) / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(animation_type: WindowCloseAnimationType) -> WindowCloseAnimation {
        WindowCloseAnimation {
            enabled: true,
            animation_type,
            duration_ms: 200,
            custom_shader: None,
        }
    }

    #[test]
    fn retract_reverses_launch_geometry_and_opacity_toward_its_origin() {
        let timeline = CloseTimeline::new(
            config(WindowCloseAnimationType::Retract),
            Duration::from_secs(1),
            1.0,
        );
        let bounds = Rectangle::new((500, 300).into(), (800, 600).into());
        let origin = Point::from((100.0, 100.0));

        let start = timeline.visual_at(Duration::from_secs(1));
        assert_eq!(start.destination(bounds, Some(origin)), bounds);
        assert_eq!(start.alpha, 1.0);

        let end = timeline.visual_at(Duration::from_millis(1200));
        let destination = end.destination(bounds, Some(origin));
        assert_eq!(destination.size, (640, 480).into());
        assert_eq!(
            (
                destination.loc.x + destination.size.w / 2,
                destination.loc.y + destination.size.h / 2,
            ),
            (629, 430)
        );
        assert_eq!(end.alpha, super::super::launch::START_ALPHA);
    }

    #[test]
    fn zero_duration_finishes_immediately() {
        let mut config = config(WindowCloseAnimationType::Shrink);
        config.duration_ms = 0;
        let now = Duration::from_secs(1);
        let timeline = CloseTimeline::new(config, now, 1.0);

        assert!(timeline.is_finished_at(now));
        assert_eq!(timeline.visual_at(now).scale, 0.0);
    }

    #[test]
    fn shader_clock_is_linear_while_cpu_envelopes_stay_eased() {
        let timeline = CloseTimeline::new(
            WindowCloseAnimation {
                enabled: true,
                animation_type: WindowCloseAnimationType::Shrink,
                duration_ms: 200,
                custom_shader: Some("close.frag".into()),
            },
            Duration::from_secs(1),
            1.0,
        );
        let early = timeline.visual_at(Duration::from_millis(1050));
        assert!((early.linear_progress - 0.25).abs() < 1e-9);
        assert!(
            early.progress < 0.1,
            "ease-in-out stays near zero at 25% time"
        );
        assert_eq!(early.scale, 1.0 - early.progress);
    }

    #[test]
    fn shader_geo_keeps_in_place_bounds_and_retract_size() {
        let bounds = Rectangle::new((500, 300).into(), (800, 600).into());
        let origin = Point::from((100.0, 100.0));
        let shrink = CloseTimeline::new(
            WindowCloseAnimation {
                enabled: true,
                animation_type: WindowCloseAnimationType::Shrink,
                duration_ms: 200,
                custom_shader: Some("close.frag".into()),
            },
            Duration::from_secs(1),
            1.0,
        );
        assert!(shrink.shader_pixels());
        let middle = shrink.visual_at(Duration::from_millis(1100));
        assert_eq!(middle.destination(bounds, None).size, (400, 300).into());
        assert_eq!(middle.shader_geo(bounds, None), bounds);

        let retract = CloseTimeline::new(
            WindowCloseAnimation {
                enabled: true,
                animation_type: WindowCloseAnimationType::Retract,
                duration_ms: 200,
                custom_shader: Some("close.frag".into()),
            },
            Duration::from_secs(1),
            1.0,
        );
        let start = retract.visual_at(Duration::from_secs(1));
        assert_eq!(start.shader_geo(bounds, Some(origin)).size, bounds.size);
        let end = retract.visual_at(Duration::from_millis(1200));
        assert_eq!(end.shader_geo(bounds, Some(origin)).size, bounds.size);
        assert_ne!(end.destination(bounds, Some(origin)).size, bounds.size);
    }
}
