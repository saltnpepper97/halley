//! Window animation timelines and presentation state.

use std::collections::HashMap;
use std::time::Duration;

use halley_config::{AnimationMotion, Animations, WindowOpenAnimationType};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Physical, Point, Rectangle};

const MAX_OVERSHOOT_SCALE: f64 = 1.08;

pub(crate) mod close;
mod launch;
mod motion;

pub(crate) use motion::MotionTimeline;

#[derive(Clone, Copy, Debug)]
struct WindowOpenTimeline {
    motion: MotionTimeline,
    motion_config: AnimationMotion,
    animation_type: WindowOpenAnimationType,
    launch_origin: Option<Point<f64, Physical>>,
    geometry: Option<RectTransition>,
    shader_pixels: bool,
    random_seed: f32,
}

impl WindowOpenTimeline {
    fn visual_at(self, now: Duration, bounds: Rectangle<i32, Physical>) -> WindowOpenVisual {
        let sample = self.motion.sample_at(now);
        let raw_progress = sample.value;
        let progress = raw_progress.clamp(0.0, 1.0);
        let shader_pixels = self.shader_pixels;
        let (scale, alpha) = if shader_pixels {
            (1.0, 1.0)
        } else {
            match self.animation_type {
                WindowOpenAnimationType::CenterOut => {
                    (raw_progress.clamp(0.0, MAX_OVERSHOOT_SCALE), 1.0)
                }
                WindowOpenAnimationType::Fade => (1.0, progress.clamp(0.0, 1.0) as f32),
                WindowOpenAnimationType::Launch => (1.0, launch::alpha(sample.linear_progress)),
            }
        };
        let destination = self
            .geometry
            .map(|geometry| geometry.rect_at(now).round())
            .or_else(|| {
                let traveling = self.animation_type == WindowOpenAnimationType::Launch
                    && !self.motion.is_finished_at(now);
                traveling.then(|| {
                    if shader_pixels {
                        launch::path_rect(bounds, self.launch_origin, sample).round()
                    } else {
                        launch::rect(bounds, self.launch_origin, sample).round()
                    }
                })
            });
        WindowOpenVisual {
            scale,
            alpha,
            destination,
            progress: raw_progress,
            clamped_progress: progress,
            random_seed: self.random_seed,
            shader_pixels,
        }
    }

    fn is_finished_at(self, now: Duration) -> bool {
        self.motion.is_finished_at(now)
            && self
                .geometry
                .is_none_or(|geometry| geometry.is_finished_at(now))
    }

    fn retarget(
        &mut self,
        now: Duration,
        current_bounds: Rectangle<i32, Physical>,
        target_bounds: Rectangle<i32, Physical>,
    ) {
        let (current, velocity) = match self.geometry {
            Some(geometry) => (geometry.rect_at(now), geometry.velocity_at(now)),
            None if self.animation_type == WindowOpenAnimationType::Launch => {
                if self.shader_pixels {
                    launch::path_rect_and_velocity(
                        current_bounds,
                        self.launch_origin,
                        self.motion.sample_at(now),
                    )
                } else {
                    launch::rect_and_velocity(
                        current_bounds,
                        self.launch_origin,
                        self.motion.sample_at(now),
                    )
                }
            }
            None => {
                let scale = self.scale_at(now);
                let scale_velocity = self.scale_velocity_at(now);
                (
                    VisualRect::scaled(current_bounds, scale),
                    VisualRect::scaled_velocity(current_bounds, scale_velocity),
                )
            }
        };
        self.geometry = Some(RectTransition::between(
            self.motion_config,
            now,
            current,
            VisualRect::from(target_bounds),
            velocity,
        ));
    }

    fn scale_at(self, now: Duration) -> f64 {
        if self.shader_pixels {
            return 1.0;
        }
        let motion = self.motion.value_at(now).clamp(0.0, MAX_OVERSHOOT_SCALE);
        match self.animation_type {
            WindowOpenAnimationType::CenterOut => motion,
            WindowOpenAnimationType::Fade => 1.0,
            WindowOpenAnimationType::Launch => 1.0,
        }
    }

    fn scale_velocity_at(self, now: Duration) -> f64 {
        if self.shader_pixels {
            return 0.0;
        }
        let progress = self.motion.value_at(now);
        if !(0.0..MAX_OVERSHOOT_SCALE).contains(&progress) {
            return 0.0;
        }
        let velocity = self.motion.velocity_at(now);
        match self.animation_type {
            WindowOpenAnimationType::CenterOut => velocity,
            WindowOpenAnimationType::Fade | WindowOpenAnimationType::Launch => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct VisualRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl VisualRect {
    fn scaled(bounds: Rectangle<i32, Physical>, scale: f64) -> Self {
        let bounds = Self::from(bounds);
        let center_x = bounds.x + bounds.width / 2.0;
        let center_y = bounds.y + bounds.height / 2.0;
        let width = bounds.width * scale;
        let height = bounds.height * scale;
        Self {
            x: center_x - width / 2.0,
            y: center_y - height / 2.0,
            width,
            height,
        }
    }

    fn scaled_velocity(bounds: Rectangle<i32, Physical>, scale_velocity: f64) -> Self {
        let width = f64::from(bounds.size.w) * scale_velocity;
        let height = f64::from(bounds.size.h) * scale_velocity;
        Self {
            x: -width / 2.0,
            y: -height / 2.0,
            width,
            height,
        }
    }

    fn round(self) -> Rectangle<i32, Physical> {
        let left = self.x.round() as i32;
        let top = self.y.round() as i32;
        let right = (self.x + self.width).round() as i32;
        let bottom = (self.y + self.height).round() as i32;
        Rectangle::new(
            (left, top).into(),
            ((right - left).max(0), (bottom - top).max(0)).into(),
        )
    }
}

impl From<Rectangle<i32, Physical>> for VisualRect {
    fn from(rect: Rectangle<i32, Physical>) -> Self {
        Self {
            x: f64::from(rect.loc.x),
            y: f64::from(rect.loc.y),
            width: f64::from(rect.size.w),
            height: f64::from(rect.size.h),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct RectTransition {
    x: MotionTimeline,
    y: MotionTimeline,
    width: MotionTimeline,
    height: MotionTimeline,
}

impl RectTransition {
    fn between(
        motion: AnimationMotion,
        now: Duration,
        start: VisualRect,
        target: VisualRect,
        velocity: VisualRect,
    ) -> Self {
        Self {
            x: MotionTimeline::between(motion, now, start.x, target.x, velocity.x),
            y: MotionTimeline::between(motion, now, start.y, target.y, velocity.y),
            width: MotionTimeline::between(motion, now, start.width, target.width, velocity.width),
            height: MotionTimeline::between(
                motion,
                now,
                start.height,
                target.height,
                velocity.height,
            ),
        }
    }

    fn rect_at(self, now: Duration) -> VisualRect {
        VisualRect {
            x: self.x.value_at(now),
            y: self.y.value_at(now),
            width: self.width.value_at(now),
            height: self.height.value_at(now),
        }
    }

    fn velocity_at(self, now: Duration) -> VisualRect {
        VisualRect {
            x: self.x.velocity_at(now),
            y: self.y.velocity_at(now),
            width: self.width.velocity_at(now),
            height: self.height.velocity_at(now),
        }
    }

    /// Fraction of the distance every axis has actually travelled, following
    /// the configured curve rather than wall-clock time.
    fn completion_at(self, now: Duration) -> f64 {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .map(|timeline| timeline.completion_at(now))
            .fold(1.0, f64::min)
    }

    fn is_finished_at(self, now: Duration) -> bool {
        self.x.is_finished_at(now)
            && self.y.is_finished_at(now)
            && self.width.is_finished_at(now)
            && self.height.is_finished_at(now)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowOpenVisual {
    scale: f64,
    alpha: f32,
    destination: Option<Rectangle<i32, Physical>>,
    progress: f64,
    clamped_progress: f64,
    random_seed: f32,
    shader_pixels: bool,
}

impl Default for WindowOpenVisual {
    fn default() -> Self {
        Self {
            scale: 1.0,
            alpha: 1.0,
            destination: None,
            progress: 1.0,
            clamped_progress: 1.0,
            random_seed: 0.0,
            shader_pixels: false,
        }
    }
}

impl WindowOpenVisual {
    pub fn transform_rect(
        self,
        rect: Rectangle<i32, Physical>,
        bounds: Rectangle<i32, Physical>,
    ) -> Rectangle<i32, Physical> {
        self.destination
            .map(|destination| map_rect(rect, bounds, destination))
            .unwrap_or_else(|| scale_rect_from_center(rect, bounds, self.scale))
    }

    pub fn alpha(self) -> f32 {
        self.alpha
    }

    pub fn progress(self) -> f64 {
        self.progress
    }

    pub fn clamped_progress(self) -> f64 {
        self.clamped_progress
    }

    pub fn random_seed(self) -> f32 {
        self.random_seed
    }

    pub fn shader_pixels(self) -> bool {
        self.shader_pixels
    }
}

#[derive(Clone, Copy, Debug)]
struct ArrangeTimeline {
    geometry: RectTransition,
}

impl ArrangeTimeline {
    fn between(
        motion: AnimationMotion,
        now: Duration,
        start: Rectangle<i32, Physical>,
        target: Rectangle<i32, Physical>,
        velocity: VisualRect,
    ) -> Self {
        Self {
            geometry: RectTransition::between(
                motion,
                now,
                VisualRect::from(start),
                VisualRect::from(target),
                velocity,
            ),
        }
    }

    fn rect_at(self, now: Duration) -> Rectangle<i32, Physical> {
        self.geometry.rect_at(now).round()
    }

    fn velocity_at(self, now: Duration) -> VisualRect {
        self.geometry.velocity_at(now)
    }

    /// Eased so texture blends advance with the visible motion; a linear
    /// clock changed the content before the frame had visibly moved.
    fn completion_at(self, now: Duration) -> f64 {
        self.geometry.completion_at(now)
    }

    fn is_finished_at(self, now: Duration) -> bool {
        self.geometry.is_finished_at(now)
    }
}

pub struct WindowAnimations {
    config: Animations,
    opening: HashMap<WlSurface, WindowOpenTimeline>,
    arranging: HashMap<WlSurface, ArrangeTimeline>,
}

impl WindowAnimations {
    pub fn new(config: Animations) -> Self {
        Self {
            config,
            opening: HashMap::new(),
            arranging: HashMap::new(),
        }
    }

    pub fn start(&mut self, surface: WlSurface, now: Duration) -> bool {
        self.start_with_origin(surface, now, None)
    }

    pub fn start_with_origin(
        &mut self,
        surface: WlSurface,
        now: Duration,
        launch_origin: Option<Point<f64, Physical>>,
    ) -> bool {
        let config = &self.config.window_open;
        if !self.config.enabled || !config.enabled {
            return false;
        }

        let std::collections::hash_map::Entry::Vacant(entry) = self.opening.entry(surface) else {
            return false;
        };
        entry.insert(WindowOpenTimeline {
            motion: MotionTimeline::new(config.motion, now),
            motion_config: config.motion,
            animation_type: config.animation_type,
            launch_origin,
            geometry: None,
            shader_pixels: config.custom_shader.is_some(),
            random_seed: animation_seed(),
        });
        true
    }

    pub fn retarget(
        &mut self,
        surface: &WlSurface,
        now: Duration,
        current_bounds: Rectangle<i32, Physical>,
        target_bounds: Rectangle<i32, Physical>,
    ) -> bool {
        let Some(timeline) = self.opening.get_mut(surface) else {
            return false;
        };
        timeline.retarget(now, current_bounds, target_bounds);
        true
    }

    /// Starts or reverses one compositor-owned Field arrangement.
    ///
    /// An in-flight transition contributes its current velocity so rapid
    /// toggle reversals remain continuous instead of restarting from rest.
    pub fn arrange(
        &mut self,
        surface: WlSurface,
        now: Duration,
        current_bounds: Rectangle<i32, Physical>,
        target_bounds: Rectangle<i32, Physical>,
    ) -> bool {
        let config = self.config.arrange;
        if !self.config.enabled || !config.enabled || current_bounds == target_bounds {
            self.arranging.remove(&surface);
            return false;
        }
        let velocity = self
            .arranging
            .get(&surface)
            .filter(|timeline| !timeline.is_finished_at(now))
            .map_or(
                VisualRect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                },
                |timeline| timeline.velocity_at(now),
            );
        self.arranging.insert(
            surface,
            ArrangeTimeline::between(config.motion, now, current_bounds, target_bounds, velocity),
        );
        true
    }

    pub fn arrange_visual(
        &self,
        surface: &WlSurface,
        now: Duration,
    ) -> Option<Rectangle<i32, Physical>> {
        self.arranging
            .get(surface)
            .map(|timeline| timeline.rect_at(now))
    }

    pub fn arrange_completion(&self, surface: &WlSurface, now: Duration) -> Option<f64> {
        self.arranging
            .get(surface)
            .map(|timeline| timeline.completion_at(now))
    }

    pub fn is_arranging(&self, surface: &WlSurface, now: Duration) -> bool {
        self.arranging
            .get(surface)
            .is_some_and(|timeline| !timeline.is_finished_at(now))
    }

    pub fn has_arrange_timeline(&self, surface: &WlSurface) -> bool {
        self.arranging.contains_key(surface)
    }

    /// Updates policy for future windows without disturbing animations
    /// already in flight.
    pub fn reload(&mut self, config: Animations) {
        self.config = config;
    }

    pub fn visual(
        &self,
        surface: &WlSurface,
        now: Duration,
        bounds: Rectangle<i32, Physical>,
    ) -> Option<WindowOpenVisual> {
        self.opening
            .get(surface)
            .map(|timeline| timeline.visual_at(now, bounds))
    }

    pub fn is_animating(&self, surface: &WlSurface, now: Duration) -> bool {
        self.opening
            .get(surface)
            .is_some_and(|timeline| !timeline.is_finished_at(now))
            || self
                .arranging
                .get(surface)
                .is_some_and(|timeline| !timeline.is_finished_at(now))
    }

    pub fn remove(&mut self, surface: &WlSurface) {
        self.opening.remove(surface);
        self.arranging.remove(surface);
    }

    /// Retires finished timelines and reports whether an arrangement was
    /// removed. Arrangement snapshots are rendered through the endpoint frame;
    /// callers need this signal to schedule one more frame from the live client
    /// geometry after that presentation state is dropped.
    pub fn cleanup(&mut self, now: Duration) -> bool {
        self.opening
            .retain(|_, timeline| !timeline.is_finished_at(now));
        let arranging_before = self.arranging.len();
        self.arranging
            .retain(|_, timeline| !timeline.is_finished_at(now));
        self.arranging.len() != arranging_before
    }
}

pub(crate) fn animation_seed() -> f32 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    (hasher.finish() >> 11) as f32 / ((1_u64 << 53) as f32)
}

pub(crate) fn scale_rect_from_center(
    rect: Rectangle<i32, Physical>,
    bounds: Rectangle<i32, Physical>,
    scale: f64,
) -> Rectangle<i32, Physical> {
    let scale = scale.clamp(0.0, MAX_OVERSHOOT_SCALE);
    if scale == 1.0 {
        return rect;
    }

    let center_x = bounds.loc.x as f64 + bounds.size.w as f64 / 2.0;
    let center_y = bounds.loc.y as f64 + bounds.size.h as f64 / 2.0;
    let left = center_x + (rect.loc.x as f64 - center_x) * scale;
    let top = center_y + (rect.loc.y as f64 - center_y) * scale;
    let right = center_x + (rect.loc.x as f64 + rect.size.w as f64 - center_x) * scale;
    let bottom = center_y + (rect.loc.y as f64 + rect.size.h as f64 - center_y) * scale;

    let left = left.round() as i32;
    let top = top.round() as i32;
    let right = right.round() as i32;
    let bottom = bottom.round() as i32;
    Rectangle::new(
        (left, top).into(),
        ((right - left).max(0), (bottom - top).max(0)).into(),
    )
}

pub(crate) fn map_rect(
    rect: Rectangle<i32, Physical>,
    source: Rectangle<i32, Physical>,
    destination: Rectangle<i32, Physical>,
) -> Rectangle<i32, Physical> {
    let source_width = f64::from(source.size.w.max(1));
    let source_height = f64::from(source.size.h.max(1));
    let scale_x = f64::from(destination.size.w) / source_width;
    let scale_y = f64::from(destination.size.h) / source_height;
    let left = f64::from(destination.loc.x) + f64::from(rect.loc.x - source.loc.x) * scale_x;
    let top = f64::from(destination.loc.y) + f64::from(rect.loc.y - source.loc.y) * scale_y;
    let right =
        f64::from(destination.loc.x) + f64::from(rect.loc.x + rect.size.w - source.loc.x) * scale_x;
    let bottom =
        f64::from(destination.loc.y) + f64::from(rect.loc.y + rect.size.h - source.loc.y) * scale_y;
    let left = left.round() as i32;
    let top = top.round() as i32;
    let right = right.round() as i32;
    let bottom = bottom.round() as i32;
    Rectangle::new(
        (left, top).into(),
        ((right - left).max(0), (bottom - top).max(0)).into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_config::{AnimationCurve, AnimationMotion, EasingMotion};

    fn timeline(
        animation_type: WindowOpenAnimationType,
        curve: AnimationCurve,
    ) -> WindowOpenTimeline {
        let motion_config = AnimationMotion::Easing(EasingMotion {
            duration_ms: 300,
            curve,
        });
        WindowOpenTimeline {
            motion: MotionTimeline::new(motion_config, Duration::from_secs(1)),
            motion_config,
            animation_type,
            launch_origin: None,
            geometry: None,
            shader_pixels: false,
            random_seed: 0.0,
        }
    }

    fn launch_timeline(origin: Point<f64, Physical>) -> WindowOpenTimeline {
        let mut timeline = timeline(WindowOpenAnimationType::Launch, AnimationCurve::Linear);
        timeline.launch_origin = Some(origin);
        timeline
    }

    fn rect(width: i32, height: i32) -> Rectangle<i32, Physical> {
        Rectangle::new((0, 0).into(), (width, height).into())
    }

    #[test]
    fn timeline_uses_configured_duration() {
        let animation = timeline(WindowOpenAnimationType::CenterOut, AnimationCurve::Linear);

        assert_eq!(animation.motion.value_at(Duration::from_secs(1)), 0.0);
        assert_eq!(animation.motion.value_at(Duration::from_millis(1150)), 0.5);
        assert_eq!(animation.motion.value_at(Duration::from_millis(1300)), 1.0);
    }

    #[test]
    fn easing_curves_preserve_endpoints() {
        for curve in [
            AnimationCurve::Linear,
            AnimationCurve::EaseInOutCubic,
            AnimationCurve::EaseOutQuad,
            AnimationCurve::EaseOutCubic,
            AnimationCurve::EaseOutExpo,
            AnimationCurve::Elastic,
        ] {
            assert_eq!(motion::apply_curve(curve, 0.0), 0.0);
            assert_eq!(motion::apply_curve(curve, 1.0), 1.0);
        }
    }

    #[test]
    fn ease_out_curves_advance_faster_than_linear() {
        let linear = motion::apply_curve(AnimationCurve::Linear, 0.5);
        assert!(motion::apply_curve(AnimationCurve::EaseOutQuad, 0.5) > linear);
        assert!(motion::apply_curve(AnimationCurve::EaseOutCubic, 0.5) > linear);
        assert!(motion::apply_curve(AnimationCurve::EaseOutExpo, 0.5) > linear);
    }

    #[test]
    fn center_out_expands_from_final_center() {
        let bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let timeline = timeline(WindowOpenAnimationType::CenterOut, AnimationCurve::Linear);

        assert_eq!(
            timeline
                .visual_at(Duration::from_secs(1), bounds)
                .transform_rect(bounds, bounds),
            Rectangle::new((500, 350).into(), (0, 0).into())
        );
        assert_eq!(
            timeline
                .visual_at(Duration::from_millis(1150), bounds)
                .transform_rect(bounds, bounds),
            Rectangle::new((300, 200).into(), (400, 300).into())
        );
        assert_eq!(
            timeline
                .visual_at(Duration::from_millis(1300), bounds)
                .transform_rect(bounds, bounds),
            bounds
        );
    }

    #[test]
    fn surface_tree_rects_share_the_window_center() {
        let bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let subsurface = Rectangle::new((110, 60).into(), (100, 50).into());
        let visual = timeline(WindowOpenAnimationType::CenterOut, AnimationCurve::Linear)
            .visual_at(Duration::from_millis(1150), bounds);

        assert_eq!(
            visual.transform_rect(subsurface, bounds),
            Rectangle::new((305, 205).into(), (50, 25).into())
        );
    }

    #[test]
    fn fade_starts_transparent_at_final_geometry() {
        let bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let animation = timeline(WindowOpenAnimationType::Fade, AnimationCurve::Linear);

        let start = animation.visual_at(Duration::from_secs(1), bounds);
        assert_eq!(start.scale, 1.0);
        assert_eq!(start.alpha(), 0.0);
        assert_eq!(start.transform_rect(bounds, bounds), bounds);
    }

    #[test]
    fn fade_advances_alpha_without_transforming_geometry() {
        let bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let animation = timeline(WindowOpenAnimationType::Fade, AnimationCurve::Linear);
        let middle = animation.visual_at(Duration::from_millis(1150), bounds);
        let end = animation.visual_at(Duration::from_millis(1300), bounds);

        assert_eq!(middle.alpha(), 0.5);
        assert_eq!(middle.transform_rect(bounds, bounds), bounds);
        assert_eq!(end, WindowOpenVisual::default());
    }

    #[test]
    fn launch_starts_small_and_translucent_at_the_capped_origin() {
        let bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let animation = launch_timeline(Point::from((-1_000.0, 350.0)));
        let start = animation.visual_at(Duration::from_secs(1), bounds);
        let destination = start.transform_rect(bounds, bounds);

        assert_eq!(start.alpha(), launch::START_ALPHA);
        assert_eq!(destination.size, (640, 480).into());
        assert_eq!(
            launch::rect_center(destination),
            Point::from((180.0, 350.0))
        );
    }

    #[test]
    fn launch_follows_a_subtle_upward_arc() {
        let bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let animation = launch_timeline(Point::from((200.0, 350.0)));
        let middle = animation.visual_at(Duration::from_millis(1150), bounds);
        let center = launch::rect_center(middle.transform_rect(bounds, bounds));

        assert!(center.x > 200.0);
        assert!(center.x < 500.0);
        assert!(center.y < 350.0);
    }

    #[test]
    fn launch_applies_the_configured_travel_curve_once() {
        let bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let origin = Point::from((200.0, 350.0));
        let now = Duration::from_millis(1150);
        let linear = launch_timeline(origin);
        let mut quad = timeline(WindowOpenAnimationType::Launch, AnimationCurve::EaseOutQuad);
        quad.launch_origin = Some(origin);

        let linear_center =
            launch::rect_center(linear.visual_at(now, bounds).transform_rect(bounds, bounds));
        let quad_center =
            launch::rect_center(quad.visual_at(now, bounds).transform_rect(bounds, bounds));

        assert_eq!(linear_center.x, 350.0);
        assert_eq!(quad_center.x, 425.0);
    }

    #[test]
    fn launch_scale_and_alpha_follow_elapsed_duration() {
        let bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let origin = launch::rect_center(bounds);
        let now = Duration::from_millis(1150);
        let mut linear = timeline(WindowOpenAnimationType::Launch, AnimationCurve::Linear);
        let mut expo = timeline(WindowOpenAnimationType::Launch, AnimationCurve::EaseOutExpo);
        linear.launch_origin = Some(origin);
        expo.launch_origin = Some(origin);

        let linear = linear.visual_at(now, bounds);
        let expo = expo.visual_at(now, bounds);

        assert_eq!(
            linear.transform_rect(bounds, bounds).size,
            expo.transform_rect(bounds, bounds).size
        );
        assert_eq!(linear.alpha(), expo.alpha());
    }

    #[test]
    fn elastic_launch_overshoots_then_settles_at_the_deadline() {
        let bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let origin = Point::from((200.0, 350.0));
        let mut animation = timeline(WindowOpenAnimationType::Launch, AnimationCurve::Elastic);
        animation.launch_origin = Some(origin);

        let middle = animation.visual_at(Duration::from_millis(1150), bounds);
        let middle_center = launch::rect_center(middle.transform_rect(bounds, bounds));

        assert!(middle_center.x > launch::rect_center(bounds).x);
        assert!(!animation.is_finished_at(Duration::from_millis(1299)));
        assert_eq!(
            animation.visual_at(Duration::from_millis(1300), bounds),
            WindowOpenVisual::default()
        );
    }

    #[test]
    fn launch_peaks_at_two_percent_overshoot_and_settles_exactly() {
        let bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let animation = launch_timeline(launch::rect_center(bounds));
        let overshoot = animation.visual_at(Duration::from_millis(1234), bounds);
        let destination = overshoot.transform_rect(bounds, bounds);

        assert_eq!(destination.size, (816, 612).into());
        assert_eq!(overshoot.alpha(), 1.0);
        assert_eq!(
            animation.visual_at(Duration::from_millis(1300), bounds),
            WindowOpenVisual::default()
        );
    }

    #[test]
    fn launch_retarget_starts_from_the_current_visual_rect() {
        let windowed = Rectangle::new((100, 50).into(), (800, 600).into());
        let fullscreen = Rectangle::new((0, 0).into(), (1920, 1080).into());
        let now = Duration::from_millis(1120);
        let mut animation = launch_timeline(Point::from((50.0, 900.0)));
        let before = animation
            .visual_at(now, windowed)
            .transform_rect(windowed, windowed);

        animation.retarget(now, windowed, fullscreen);

        assert_eq!(
            animation
                .visual_at(now, fullscreen)
                .transform_rect(fullscreen, fullscreen),
            before
        );
    }

    #[test]
    fn active_opening_uses_updated_final_bounds_without_restarting() {
        let animation = timeline(WindowOpenAnimationType::Fade, AnimationCurve::Linear);
        let now = Duration::from_millis(1150);

        let windowed = animation.visual_at(now, rect(800, 600));
        let fullscreen = animation.visual_at(now, rect(1920, 1080));

        assert_eq!(animation.motion.value_at(now), 0.5);
        assert_eq!(windowed.alpha(), fullscreen.alpha());
        assert_eq!(windowed.scale, 1.0);
        assert_eq!(fullscreen.scale, 1.0);
        assert_eq!(animation.motion.value_at(Duration::from_millis(1300)), 1.0);
    }

    #[test]
    fn center_out_can_use_the_elastic_curve() {
        let bounds = Rectangle::new((0, 0).into(), (800, 600).into());
        let animation = timeline(WindowOpenAnimationType::CenterOut, AnimationCurve::Elastic);

        let middle = animation.visual_at(Duration::from_millis(1150), bounds);
        assert!(middle.scale > 1.0);
        assert_eq!(middle.alpha(), 1.0);

        let end = animation.visual_at(Duration::from_millis(1300), bounds);
        assert_eq!(end, WindowOpenVisual::default());
        assert!(animation.is_finished_at(Duration::from_millis(1300)));
    }

    #[test]
    fn overshoot_does_not_finish_the_timeline_early() {
        let animation = timeline(WindowOpenAnimationType::CenterOut, AnimationCurve::Elastic);
        let middle = Duration::from_millis(1150);

        assert!(animation.visual_at(middle, rect(800, 600)).scale > 1.0);
        assert!(!animation.is_finished_at(middle));
    }

    #[test]
    fn geometry_retarget_preserves_the_current_presentation() {
        let current_bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let target = Rectangle::new((0, 0).into(), (1920, 1080).into());
        let now = Duration::from_millis(1150);
        let mut animation = timeline(WindowOpenAnimationType::CenterOut, AnimationCurve::Linear);
        let before = animation
            .visual_at(now, current_bounds)
            .transform_rect(current_bounds, current_bounds);

        animation.retarget(now, current_bounds, target);
        let after = animation
            .visual_at(now, target)
            .transform_rect(target, target);

        assert_eq!(after, before);
        assert_ne!(
            animation
                .visual_at(Duration::from_millis(1250), target)
                .transform_rect(target, target),
            target
        );
        assert_eq!(
            animation
                .visual_at(Duration::from_millis(1450), target)
                .transform_rect(target, target),
            target
        );
    }

    #[test]
    fn repeated_retarget_starts_from_the_live_intermediate_rect() {
        let windowed = Rectangle::new((100, 50).into(), (800, 600).into());
        let fullscreen = Rectangle::new((0, 0).into(), (1920, 1080).into());
        let restored = Rectangle::new((200, 100).into(), (900, 700).into());
        let mut animation = timeline(WindowOpenAnimationType::CenterOut, AnimationCurve::Linear);

        animation.retarget(Duration::from_millis(1100), windowed, fullscreen);
        let now = Duration::from_millis(1200);
        let before = animation
            .visual_at(now, fullscreen)
            .transform_rect(fullscreen, fullscreen);
        animation.retarget(now, fullscreen, restored);
        let after = animation
            .visual_at(now, restored)
            .transform_rect(restored, restored);

        assert_eq!(after, before);
    }

    #[test]
    fn retarget_keeps_the_original_alpha_timeline() {
        let windowed = rect(800, 600);
        let fullscreen = rect(1920, 1080);
        let now = Duration::from_millis(1150);
        let mut animation = timeline(WindowOpenAnimationType::Fade, AnimationCurve::Linear);
        let alpha = animation.visual_at(now, windowed).alpha();

        animation.retarget(now, windowed, fullscreen);

        assert_eq!(animation.visual_at(now, fullscreen).alpha(), alpha);
    }

    #[test]
    fn arrange_timeline_interpolates_position_and_size() {
        let motion = AnimationMotion::Easing(EasingMotion {
            duration_ms: 300,
            curve: AnimationCurve::Linear,
        });
        let start = Rectangle::new((100, 50).into(), (800, 600).into());
        let target = Rectangle::new((0, 0).into(), (1200, 900).into());
        let zero = VisualRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
        let timeline = ArrangeTimeline::between(motion, Duration::ZERO, start, target, zero);

        assert_eq!(timeline.rect_at(Duration::ZERO), start);
        assert_eq!(
            timeline.rect_at(Duration::from_millis(150)),
            Rectangle::new((50, 25).into(), (1000, 750).into())
        );
        assert_eq!(timeline.rect_at(Duration::from_millis(300)), target);
        assert!(timeline.is_finished_at(Duration::from_millis(300)));
    }

    #[test]
    fn arrange_completion_follows_the_eased_motion() {
        let motion = AnimationMotion::Easing(EasingMotion {
            duration_ms: 400,
            curve: AnimationCurve::EaseInOutCubic,
        });
        let start = Rectangle::new((0, 0).into(), (800, 600).into());
        let target = Rectangle::new((0, 0).into(), (1200, 900).into());
        let zero = VisualRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
        let timeline = ArrangeTimeline::between(motion, Duration::ZERO, start, target, zero);

        // A quarter of the time moves the frame only 1/16 of the way, so the
        // texture blend must not already be a quarter done.
        let quarter = timeline.completion_at(Duration::from_millis(100));
        assert!((quarter - 0.0625).abs() < 1e-9, "{quarter}");
        assert_eq!(timeline.completion_at(Duration::from_millis(200)), 0.5);
        assert_eq!(timeline.completion_at(Duration::from_millis(400)), 1.0);
    }

    #[test]
    fn arrange_reversal_starts_at_the_live_intermediate_rect() {
        let motion = AnimationMotion::Easing(EasingMotion {
            duration_ms: 300,
            curve: AnimationCurve::EaseInOutCubic,
        });
        let start = Rectangle::new((100, 50).into(), (800, 600).into());
        let target = Rectangle::new((0, 0).into(), (1200, 900).into());
        let zero = VisualRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
        let forward = ArrangeTimeline::between(motion, Duration::ZERO, start, target, zero);
        let reversed_at = Duration::from_millis(120);
        let current = forward.rect_at(reversed_at);
        let reversed = ArrangeTimeline::between(
            motion,
            reversed_at,
            current,
            start,
            forward.velocity_at(reversed_at),
        );

        assert_eq!(reversed.rect_at(reversed_at), current);
        assert_eq!(
            reversed.rect_at(reversed_at + Duration::from_millis(300)),
            start
        );
    }

    #[test]
    fn geometry_retarget_keeps_the_opening_motion_and_alpha() {
        let windowed = rect(800, 600);
        let fullscreen = rect(1920, 1080);
        let started = Duration::from_secs(1);
        let mut animation = timeline(WindowOpenAnimationType::Fade, AnimationCurve::Linear);
        let alpha = animation.visual_at(started, windowed).alpha();

        animation.retarget(started, windowed, fullscreen);

        assert_eq!(animation.visual_at(started, fullscreen).alpha(), alpha);
        assert_ne!(
            animation
                .visual_at(started + Duration::from_millis(100), fullscreen)
                .transform_rect(fullscreen, fullscreen),
            fullscreen
        );
        assert_eq!(
            animation
                .visual_at(started + Duration::from_millis(300), fullscreen)
                .transform_rect(fullscreen, fullscreen),
            fullscreen
        );
    }

    #[test]
    fn shader_pixels_keep_in_place_geometry_and_full_opacity() {
        let bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let mut animation = timeline(WindowOpenAnimationType::CenterOut, AnimationCurve::Linear);
        animation.shader_pixels = true;
        let middle = animation.visual_at(Duration::from_millis(1150), bounds);

        assert!(middle.shader_pixels());
        assert_eq!(middle.scale, 1.0);
        assert_eq!(middle.alpha(), 1.0);
        assert_eq!(middle.transform_rect(bounds, bounds), bounds);
        assert_eq!(middle.clamped_progress(), 0.5);
    }

    #[test]
    fn shader_pixels_launch_travels_at_native_size() {
        let bounds = Rectangle::new((100, 50).into(), (800, 600).into());
        let mut animation = launch_timeline(Point::from((200.0, 350.0)));
        animation.shader_pixels = true;
        let start = animation.visual_at(Duration::from_secs(1), bounds);
        let destination = start.transform_rect(bounds, bounds);

        assert_eq!(destination.size, bounds.size);
        assert_eq!(start.alpha(), 1.0);
        assert_eq!(
            launch::rect_center(destination),
            Point::from((200.0, 350.0))
        );
    }
}
