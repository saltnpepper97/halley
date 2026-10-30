use halley_core::camera::Camera;
use halley_core::field::Vec2;

const PINCH_ZOOM_ACTIVATE_LOG_DELTA: f32 = 0.12;
const PINCH_ZOOM_NOISE_LOG_DELTA: f32 = 0.04;
const PINCH_ZOOM_STRONG_LOG_DELTA: f32 = 0.18;
const PINCH_PAN_LOCK_PX: f32 = 4.0;
const PINCH_PAN_DEFINITE_LOCK_PX: f32 = 16.0;
const VELOCITY_EMA_WEIGHT: f32 = 0.35;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PinchIntent {
    Pan,
    Zoom,
}

#[derive(Clone, Copy, Debug)]
enum PinchMode {
    Pending { delta: Vec2 },
    Pan,
    Zoom,
}

#[derive(Clone, Debug)]
pub(super) struct PanGesture {
    pub output: String,
    velocity: Vec2,
    last_time: Option<u32>,
}

impl PanGesture {
    pub fn new(output: String, camera: &mut Camera) -> Self {
        camera.snap_targets_to_live();
        camera.pan_vel = Vec2 { x: 0.0, y: 0.0 };
        camera.zoom_log_vel = 0.0;
        Self {
            output,
            velocity: Vec2 { x: 0.0, y: 0.0 },
            last_time: None,
        }
    }

    pub fn update(&mut self, camera: &mut Camera, time: u32, dx: f64, dy: f64) {
        self.velocity = sample_velocity(self.velocity, self.last_time, time, dx, dy);
        self.last_time = Some(time);
        apply_pan(camera, dx, dy);
    }

    pub fn finish(self, camera: &mut Camera, cancelled: bool, momentum: bool, minimum_speed: f32) {
        if cancelled || !momentum || length(self.velocity) < minimum_speed {
            return;
        }
        let velocity = screen_to_world(camera, self.velocity.x as f64, self.velocity.y as f64);
        camera.fling_pan(Vec2 {
            x: -velocity.x,
            y: -velocity.y,
        });
    }
}

#[derive(Clone, Debug)]
pub(super) struct PinchGesture {
    pub output: String,
    start_view_size: Vec2,
    mode: PinchMode,
    velocity: Vec2,
    last_time: Option<u32>,
}

impl PinchGesture {
    pub fn new(output: String, camera: &mut Camera) -> Self {
        camera.snap_targets_to_live();
        camera.pan_vel = Vec2 { x: 0.0, y: 0.0 };
        camera.zoom_log_vel = 0.0;
        Self {
            output,
            start_view_size: camera.view_size,
            mode: PinchMode::Pending {
                delta: Vec2 { x: 0.0, y: 0.0 },
            },
            velocity: Vec2 { x: 0.0, y: 0.0 },
            last_time: None,
        }
    }

    pub fn update(
        &mut self,
        camera: &mut Camera,
        zoom: &halley_config::Zoom,
        time: u32,
        dx: f64,
        dy: f64,
        scale: f64,
    ) -> bool {
        self.velocity = sample_velocity(self.velocity, self.last_time, time, dx, dy);
        self.last_time = Some(time);
        match self.mode {
            PinchMode::Pending { mut delta } => {
                delta.x += dx as f32;
                delta.y += dy as f32;
                match classify_pinch(delta, scale as f32) {
                    Some(PinchIntent::Pan) => {
                        self.mode = PinchMode::Pan;
                        apply_pan(camera, dx, dy);
                    }
                    Some(PinchIntent::Zoom) => {
                        self.mode = PinchMode::Zoom;
                        apply_zoom(camera, zoom, self.start_view_size, scale);
                    }
                    None => self.mode = PinchMode::Pending { delta },
                }
            }
            PinchMode::Pan => apply_pan(camera, dx, dy),
            PinchMode::Zoom => apply_zoom(camera, zoom, self.start_view_size, scale),
        }
        matches!(self.mode, PinchMode::Zoom)
    }

    pub fn finish(self, camera: &mut Camera, cancelled: bool, momentum: bool, minimum_speed: f32) {
        if cancelled
            || !matches!(self.mode, PinchMode::Pan)
            || !momentum
            || length(self.velocity) < minimum_speed
        {
            return;
        }
        let velocity = screen_to_world(camera, self.velocity.x as f64, self.velocity.y as f64);
        camera.fling_pan(Vec2 {
            x: -velocity.x,
            y: -velocity.y,
        });
    }
}

pub(super) fn apply_pan(camera: &mut Camera, dx: f64, dy: f64) {
    let delta = screen_to_world(camera, dx, dy);
    camera.pan_target(Vec2 {
        x: -delta.x,
        y: -delta.y,
    });
}

fn screen_to_world(camera: &Camera, dx: f64, dy: f64) -> Vec2 {
    crate::input::grab::screen_delta_to_world(dx, dy, camera)
}

fn apply_zoom(camera: &mut Camera, zoom: &halley_config::Zoom, start_view_size: Vec2, scale: f64) {
    let scale = (scale as f32).clamp(0.05, 20.0);
    camera.set_target_view_size(
        Vec2 {
            x: start_view_size.x / scale,
            y: start_view_size.y / scale,
        },
        zoom.min,
        1.0,
    );
}

fn classify_pinch(delta: Vec2, scale: f32) -> Option<PinchIntent> {
    let pan = length(delta);
    let zoom = scale.max(0.001).ln().abs();
    if pan >= PINCH_PAN_DEFINITE_LOCK_PX && zoom < PINCH_ZOOM_STRONG_LOG_DELTA
        || pan >= PINCH_PAN_LOCK_PX && zoom < PINCH_ZOOM_NOISE_LOG_DELTA
    {
        Some(PinchIntent::Pan)
    } else if zoom >= PINCH_ZOOM_ACTIVATE_LOG_DELTA {
        Some(PinchIntent::Zoom)
    } else {
        None
    }
}

fn sample_velocity(
    previous: Vec2,
    previous_time: Option<u32>,
    time: u32,
    dx: f64,
    dy: f64,
) -> Vec2 {
    let Some(previous_time) = previous_time else {
        return Vec2 { x: 0.0, y: 0.0 };
    };
    let dt = (time.wrapping_sub(previous_time).max(1) as f32 / 1000.0).clamp(0.001, 0.1);
    let instantaneous = Vec2 {
        x: dx as f32 / dt,
        y: dy as f32 / dt,
    };
    Vec2 {
        x: previous.x + (instantaneous.x - previous.x) * VELOCITY_EMA_WEIGHT,
        y: previous.y + (instantaneous.y - previous.y) * VELOCITY_EMA_WEIGHT,
    }
}

fn length(value: Vec2) -> f32 {
    value.x.hypot(value.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinch_intent_ignores_noise_then_latches_pan_or_zoom() {
        assert_eq!(classify_pinch(Vec2 { x: 1.0, y: 1.0 }, 1.01), None);
        assert_eq!(
            classify_pinch(Vec2 { x: 18.0, y: 0.0 }, 1.01),
            Some(PinchIntent::Pan)
        );
        assert_eq!(
            classify_pinch(Vec2 { x: 0.0, y: 0.0 }, 1.2),
            Some(PinchIntent::Zoom)
        );
    }

    #[test]
    fn pinch_update_reports_only_latched_zoom_activity() {
        let mut camera = Camera::new(Vec2 { x: 400.0, y: 300.0 }, Vec2 { x: 800.0, y: 600.0 });
        let mut gesture = PinchGesture::new("DP-1".into(), &mut camera);
        let zoom = halley_config::Zoom::default();

        assert!(!gesture.update(&mut camera, &zoom, 10, 1.0, 1.0, 1.01));
        assert!(gesture.update(&mut camera, &zoom, 20, 0.0, 0.0, 1.20));
    }

    #[test]
    fn pan_uses_the_same_zoom_aware_delta_as_pointer_dragging() {
        let mut camera = Camera::new(Vec2 { x: 400.0, y: 300.0 }, Vec2 { x: 800.0, y: 600.0 });
        camera.view_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        camera.target_view_size = camera.view_size;
        apply_pan(&mut camera, 10.0, -5.0);
        assert_eq!(camera.target_center, Vec2 { x: 380.0, y: 310.0 });
    }

    #[test]
    fn velocity_sampling_handles_wrapping_event_times() {
        let sampled = sample_velocity(Vec2 { x: 0.0, y: 0.0 }, Some(u32::MAX - 5), 4, 10.0, 0.0);
        assert!(sampled.x.is_finite());
        assert!(sampled.x > 0.0);
    }
}
