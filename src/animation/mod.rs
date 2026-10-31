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
mod movement;

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
    waiting_since: Option<Duration>,
}

impl WindowOpenTimeline {
    fn visual_at(self, now: Duration, bounds: Rectangle<i32, Physical>) -> WindowOpenVisual {
        let now = self.waiting_since.unwrap_or(now);
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
        self.waiting_since.is_none()
            && self.motion.is_finished_at(now)
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
    moving: HashMap<WlSurface, movement::MoveTimeline>,
}

impl WindowAnimations {
    pub fn new(config: Animations) -> Self {
        Self {
            config,
            opening: HashMap::new(),
            arranging: HashMap::new(),
            moving: HashMap::new(),
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
            waiting_since: None,
        });
        true
    }

    /// A restored X11 frame may replace its surface and arrive after the
    /// shortcut. Keep the initial visual until that surface has pixels.
    pub(crate) fn defer_opening_until_commit(&mut self, surface: &WlSurface, now: Duration) {
        if let Some(timeline) = self.opening.get_mut(surface) {
            timeline.waiting_since = Some(now);
        }
    }

    pub(crate) fn opening_committed(&mut self, surface: &WlSurface, now: Duration) -> bool {
        let Some(timeline) = self.opening.get_mut(surface) else {
            return false;
        };
        if timeline.waiting_since.take().is_none() {
            return false;
        }
        timeline.motion = MotionTimeline::new(timeline.motion_config, now);
        true
    }

    pub(crate) fn reassociate_opening(&mut self, previous: &WlSurface, surface: &WlSurface) {
        reassociate_entry(&mut self.opening, previous, surface);
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
        if timeline.waiting_since.is_some() {
            return false;
        }
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
        self.moving.remove(&surface);
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

    pub fn is_opening(&self, surface: &WlSurface, now: Duration) -> bool {
        self.opening.get(surface).is_some_and(|timeline| {
            timeline.waiting_since.is_none() && !timeline.is_finished_at(now)
        })
    }

    pub fn is_animating(&self, surface: &WlSurface, now: Duration) -> bool {
        self.is_opening(surface, now)
            || self.is_arranging(surface, now)
            || self.is_moving(surface, now)
    }

    pub fn remove(&mut self, surface: &WlSurface) {
        self.moving.remove(surface);
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
        let moving_before = self.moving.len();
        self.moving.retain(|_, timeline| !timeline.finished(now));
        let arranging_before = self.arranging.len();
        self.arranging
            .retain(|_, timeline| !timeline.is_finished_at(now));
        self.arranging.len() != arranging_before || self.moving.len() != moving_before
    }
}

// Wayland object equality and hashing use stable object identity, not liveness.
#[allow(clippy::mutable_key_type)]
pub(crate) fn reassociate_entry<T>(
    entries: &mut HashMap<WlSurface, T>,
    previous: &WlSurface,
    surface: &WlSurface,
) {
    if previous != surface
        && let Some(entry) = entries.remove(previous)
    {
        entries.insert(surface.clone(), entry);
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
            waiting_since: None,
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
    fn restored_opening_survives_surface_replacement_and_waits_for_pixels() {
        use smithay::reexports::wayland_server::protocol::wl_surface;
        use smithay::reexports::wayland_server::{
            Client, DataInit, Dispatch, Display, DisplayHandle,
        };
        use std::os::unix::net::UnixStream;
        use std::sync::Arc;

        struct State;
        impl Dispatch<WlSurface, ()> for State {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &WlSurface,
                _: wl_surface::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
            }
        }
        let display = Display::<State>::new().unwrap();
        let mut handle = display.handle();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let client = handle.insert_client(socket, Arc::new(())).unwrap();
        let previous = client
            .create_resource::<WlSurface, (), State>(&handle, 1, ())
            .unwrap();
        let restored = client
            .create_resource::<WlSurface, (), State>(&handle, 1, ())
            .unwrap();
        let config = Animations {
            window_open: halley_config::WindowOpenAnimation {
                motion: AnimationMotion::Easing(EasingMotion {
                    duration_ms: 300,
                    curve: AnimationCurve::Linear,
                }),
                animation_type: WindowOpenAnimationType::CenterOut,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut animations = WindowAnimations::new(config);
        let requested = Duration::from_secs(1);
        let ready = Duration::from_secs(10);
        let bounds = rect(800, 600);
        let origin = Some(Point::from((150.0, 200.0)));
        assert!(animations.start_with_origin(previous.clone(), requested, origin));
        animations.defer_opening_until_commit(&previous, requested);
        animations.cleanup(ready);
        assert_eq!(
            animations
                .visual(&previous, ready, bounds)
                .unwrap()
                .progress(),
            0.0
        );
        assert!(
            !animations.is_animating(&previous, ready),
            "waiting for a client must not spin frames"
        );
        assert!(!animations.retarget(&previous, ready, bounds, rect(1000, 800)));

        animations.reassociate_opening(&previous, &restored);
        assert!(animations.visual(&previous, ready, bounds).is_none());
        assert_eq!(animations.opening[&restored].launch_origin, origin);
        assert!(animations.opening_committed(&restored, ready));
        assert!(animations.is_opening(&restored, ready));
        assert_eq!(
            animations
                .visual(&restored, ready, bounds)
                .unwrap()
                .progress(),
            0.0
        );
        let middle = ready + Duration::from_millis(150);
        assert!(
            !animations.opening_committed(&restored, middle),
            "later commits must not restart the animation"
        );
        assert_eq!(
            animations
                .visual(&restored, middle, bounds)
                .unwrap()
                .progress(),
            0.5
        );
        animations.reassociate_opening(&restored, &restored);
        assert!(animations.is_opening(&restored, middle));

        // Collapsing again cancels the restore, including one still waiting
        // for a buffer, so another restore can start a fresh timeline.
        animations.remove(&restored);
        assert!(animations.start(restored.clone(), middle));
        animations.defer_opening_until_commit(&restored, middle);
        animations.remove(&restored);
        assert!(!animations.opening_committed(&restored, ready + Duration::from_secs(1)));
        assert!(animations.start(restored.clone(), middle));
        animations.cleanup(middle + Duration::from_millis(300));
        assert!(
            animations
                .visual(&restored, middle + Duration::from_millis(300), bounds)
                .is_none()
        );
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
