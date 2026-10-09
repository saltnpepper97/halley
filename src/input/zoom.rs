use halley_core::camera::{Camera, CameraTickTuning};

/// The zoom ceiling - hardcoded rather than read from config, so nothing can
/// ever magnify past 1.0x regardless of what a user puts in their config.
/// `Camera::clamp_view_size` guarantees `view_size` never shrinks below
/// `base_size` when this is passed as the upper bound, which is what makes
/// the derived scale in `tick` incapable of exceeding 1.0 - true for both
/// `zoom_out` (which grows `view_size` away from `base_size`) and `zoom_in`
/// (which shrinks it back toward `base_size`, but never past it).
const ZOOM_MAX: f32 = 1.0;

/// Advances `camera` by one tick and derives the live render scale from it.
/// Shared by both session drivers so the tuning-struct construction and
/// scale derivation aren't duplicated - pure and backend-independent, same
/// spirit as `match_bind`.
///
/// Returns `(zoom_scale, still_animating)` - the tty backend's
/// damage-driven redraw scheduler needs to know whether to keep requesting
/// redraws until the ease settles; the nested backend uses the same signal
/// to request another host-window frame only while motion remains.
pub fn tick(
    camera: &mut Camera,
    zoom: &halley_config::Zoom,
    pan_decay_rate: f32,
    dt: f32,
) -> (f32, bool) {
    // `dt` is wall-clock time since the last tick, and the tty backend only
    // redraws on demand (damage-driven) - after any idle gap it can be huge.
    // Left unclamped, a large `dt` fed into the inertial branch's
    // `exp(velocity * dt)` integration overshoots the zoom bound in a single
    // step instead of easing into it, which is what an inconsistent
    // "sometimes smooth, sometimes jumps straight there" feel actually was.
    // Honor short/zero presentation intervals too: a 240 Hz floor advances
    // high-refresh cameras faster than real time and moves duplicate samples.
    let dt = dt.clamp(0.0, 1.0 / 20.0);
    let tuning = CameraTickTuning {
        physics_enabled: true,
        zoom_enabled: zoom.enabled,
        zoom_smooth: true,
        smooth_rate: zoom.smooth_rate,
        // Only inertial gesture releases seed `pan_vel`; direct mouse drags
        // and live touchpad deltas keep using `pan_target`.
        pan_decay_rate,
        zoom_min: zoom.min,
        zoom_max: ZOOM_MAX,
    };
    let animating = camera.tick(dt, tuning);
    (scale(camera), animating)
}

/// The live render/hit-test scale derived from the camera's current
/// view-size vs. its base (1.0x) size - shared by rendering (`tick`, above)
/// and by `input::grab`'s screen↔world conversion, so there's exactly one
/// definition of "what does the camera's zoom state mean as a scale factor".
pub fn scale(camera: &Camera) -> f32 {
    crate::presentation::camera::scale(camera)
}

/// Fullscreen owns the zoom track during exit, while user pan keeps its usual
/// easing and momentum. Do not run zoom clamping over that external track.
pub fn tick_pan(
    camera: &mut Camera,
    zoom: &halley_config::Zoom,
    pan_decay_rate: f32,
    dt: f32,
) -> bool {
    camera.tick_pan(
        dt.clamp(0.0, 1.0 / 20.0),
        halley_core::camera::zoom_smooth_rate(zoom.smooth_rate),
        true,
        pan_decay_rate,
    )
}

/// Applies one zoom-out step - a no-op if zoom is disabled in config
/// (mirrors `Camera::tick`'s own `zoom_enabled` handling, so disabling zoom
/// stops both new input and any in-flight animation).
///
/// Injects log-space velocity (`Camera::zoom_inject_velocity`) rather than
/// jumping the target (`zoom_instant_by_steps`) - matches old halley's own
/// default keybind behavior exactly (its `zoom_smooth` config defaults to
/// `true`, which routes keybind-triggered zoom through this same inertial
/// path: repeated presses stack velocity into an accelerating ramp that then
/// coasts to a stop, rather than each press just resetting a fixed ease).
/// `tick`'s own `clamp_view_size` call (via `CameraTickTuning::zoom_max`,
/// always `ZOOM_MAX` here) still caps the result every step regardless of
/// how the velocity got there, so the 1.0x ceiling holds either way.
pub fn zoom_out(camera: &mut Camera, zoom: &halley_config::Zoom) {
    if !zoom.enabled {
        return;
    }
    camera.zoom_inject_velocity(-1.0, zoom.step, zoom.smooth_rate);
}

/// Applies one zoom-in step - walks a `zoom_out` step back, capped at 1.0x
/// (a no-op once already there). Not a general magnify-past-1.0x action;
/// see `ZOOM_MAX`. Same inertial-velocity approach as `zoom_out`.
pub fn zoom_in(camera: &mut Camera, zoom: &halley_config::Zoom) {
    if !zoom.enabled {
        return;
    }
    camera.zoom_inject_velocity(1.0, zoom.step, zoom.smooth_rate);
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_core::field::Vec2;

    fn default_zoom() -> halley_config::Zoom {
        halley_config::Zoom {
            enabled: true,
            min: 0.5,
            step: 1.1,
            smooth_rate: 20.0,
        }
    }

    #[test]
    fn pan_honors_zero_and_high_refresh_presentation_intervals() {
        let zoom = default_zoom();
        let mut camera = Camera::new(Vec2 { x: 400.0, y: 300.0 }, Vec2 { x: 800.0, y: 600.0 });
        camera.pan_target(Vec2 {
            x: 200.0,
            y: -100.0,
        });
        let before = camera;
        tick(&mut camera, &zoom, 6.0, 0.0);
        assert_eq!(camera.center, before.center);
        let mut reference = before;
        for _ in 0..60 {
            tick(&mut reference, &zoom, 6.0, 1.0 / 60.0);
        }
        for _ in 0..360 {
            tick(&mut camera, &zoom, 6.0, 1.0 / 360.0);
        }
        assert_eq!(camera.center, reference.center);
        assert_eq!(camera.center, camera.target_center);
    }

    #[test]
    fn scale_starts_at_one() {
        let camera = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        assert_eq!(camera.base_size.x / camera.view_size.x, 1.0);
    }

    #[test]
    fn zoom_out_then_many_ticks_settles_below_one_and_never_above() {
        let mut camera = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        let zoom = default_zoom();

        zoom_out(&mut camera, &zoom);
        let mut scale = 1.0;
        for _ in 0..1000 {
            let (s, animating) = tick(&mut camera, &zoom, 8.0, 1.0 / 60.0);
            scale = s;
            assert!(s <= 1.0, "scale must never exceed 1.0, got {s}");
            if !animating {
                break;
            }
        }
        assert!(scale < 1.0, "expected to have zoomed out, got {scale}");
        assert!(scale >= zoom.min - 0.01);
    }

    #[test]
    fn disabled_zoom_out_is_a_no_op() {
        let mut camera = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        let mut zoom = default_zoom();
        zoom.enabled = false;

        zoom_out(&mut camera, &zoom);
        assert_eq!(camera.target_view_size, camera.base_size);
    }

    #[test]
    fn reset_target_eases_scale_back_to_one() {
        let mut camera = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        let zoom = default_zoom();

        zoom_out(&mut camera, &zoom);
        for _ in 0..1000 {
            let (_, animating) = tick(&mut camera, &zoom, 8.0, 1.0 / 60.0);
            if !animating {
                break;
            }
        }
        assert!(camera.view_size.x > camera.base_size.x);

        camera.reset_zoom_target();
        let mut scale = 0.0;
        for _ in 0..1000 {
            let (s, animating) = tick(&mut camera, &zoom, 8.0, 1.0 / 60.0);
            scale = s;
            if !animating {
                break;
            }
        }
        assert_eq!(scale, 1.0);
    }

    fn settle(camera: &mut Camera, zoom: &halley_config::Zoom) -> f32 {
        let mut scale = 1.0;
        for _ in 0..1000 {
            let (s, animating) = tick(camera, zoom, 8.0, 1.0 / 60.0);
            scale = s;
            if !animating {
                break;
            }
        }
        scale
    }

    #[test]
    fn zoom_in_walks_a_zoom_out_step_back() {
        let mut camera = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        let zoom = default_zoom();

        zoom_out(&mut camera, &zoom);
        zoom_out(&mut camera, &zoom);
        let two_steps_out = settle(&mut camera, &zoom);

        zoom_in(&mut camera, &zoom);
        let one_step_back = settle(&mut camera, &zoom);

        assert!(
            one_step_back > two_steps_out,
            "expected zoom_in to move scale back toward 1.0: {one_step_back} should exceed {two_steps_out}"
        );
        assert!(one_step_back < 1.0);
    }

    #[test]
    fn zoom_in_at_rest_is_a_no_op_and_never_exceeds_one() {
        let mut camera = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        let zoom = default_zoom();

        zoom_in(&mut camera, &zoom);
        assert_eq!(camera.target_view_size, camera.base_size);
        let scale = settle(&mut camera, &zoom);
        assert_eq!(scale, 1.0);
    }

    #[test]
    fn repeated_zoom_in_never_pushes_scale_above_one() {
        let mut camera = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        let zoom = default_zoom();

        zoom_out(&mut camera, &zoom);
        settle(&mut camera, &zoom);

        for _ in 0..10 {
            zoom_in(&mut camera, &zoom);
            let scale = settle(&mut camera, &zoom);
            assert!(scale <= 1.0, "scale must never exceed 1.0, got {scale}");
        }
    }

    #[test]
    fn disabled_zoom_in_is_a_no_op() {
        let mut camera = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        let mut zoom = default_zoom();
        zoom_out(&mut camera, &zoom);
        settle(&mut camera, &zoom);
        let before = camera.target_view_size;

        zoom.enabled = false;
        zoom_in(&mut camera, &zoom);
        assert_eq!(camera.target_view_size, before);
    }
}
