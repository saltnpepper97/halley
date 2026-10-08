use crate::field::Vec2;
use crate::viewport::Viewport;

/// Consolidated camera dynamics: live position/extent, the target being
/// eased toward, and the two inertia terms (pan velocity, zoom velocity in
/// log space). This didn't exist as a single type in the old design - the
/// equivalent state was scattered across `Halley`/`MonitorSpace`/
/// `RuntimeTuning` in `halley-wl` as five separate fields
/// (`viewport.center`/`viewport.size`/`zoom_ref_size`/`camera_target_center`/
/// `camera_target_view_size`) plus `pan_vel`/`zoom_log_vel`.
///
/// Naming note: `base_size` is the reference size at 1.0x zoom (what zoom
/// bounds are computed against - roughly the monitor's native output size,
/// not itself animated). `view_size` is the live, currently-rendered view
/// extent, which shrinks/grows as the camera zooms. Conflating these two was
/// the actual source of confusion in the old code (it called the base one
/// `viewport.size` and the live one `zoom_ref_size`, with no type tying them
/// together).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Live center, in Field coordinates.
    pub center: Vec2,
    /// Live view extent, in Field coordinates.
    pub view_size: Vec2,
    /// Reference view extent at 1.0x zoom - not animated.
    pub base_size: Vec2,
    /// Center being eased toward.
    pub target_center: Vec2,
    /// View extent being eased toward.
    pub target_view_size: Vec2,
    /// Inertial pan velocity, in world units per second.
    pub pan_vel: Vec2,
    /// Inertial zoom velocity, in log(view-size) units per second.
    pub zoom_log_vel: f32,
}

impl Camera {
    pub fn new(center: Vec2, base_size: Vec2) -> Self {
        Self {
            center,
            view_size: base_size,
            base_size,
            target_center: center,
            target_view_size: base_size,
            pan_vel: Vec2 { x: 0.0, y: 0.0 },
            zoom_log_vel: 0.0,
        }
    }

    /// The live camera as a plain `Viewport`, for consumers that need that
    /// type (e.g. `Field::in_view`/rendering) rather than the full dynamics.
    pub fn viewport(&self) -> Viewport {
        Viewport::new(self.center, self.view_size)
    }

    /// Clamp a candidate view size to the zoom bounds, relative to this
    /// camera's `base_size` (was `clamp_camera_view_size`, which reached
    /// into `st.model.viewport.size` for the same purpose).
    pub fn clamp_view_size(&self, size: Vec2, zoom_min: f32, zoom_max: f32) -> Vec2 {
        let (min_zoom, max_zoom) = zoom_scale_bounds(zoom_min, zoom_max);
        Vec2 {
            x: size
                .x
                .clamp(self.base_size.x / max_zoom, self.base_size.x / min_zoom),
            y: size
                .y
                .clamp(self.base_size.y / max_zoom, self.base_size.y / min_zoom),
        }
    }

    /// Caps how much quick repeated zooming can pile up (the accel ceiling).
    pub const ZOOM_VEL_STACK_MAX: f32 = 6.0;
    /// Zoom velocity (log-space) below which the glide snaps to rest.
    pub const ZOOM_VEL_EPS: f32 = 0.02;
    /// Pan velocity below which a coasting flick snaps to rest.
    pub const PAN_VEL_EPS: f32 = 4.0;
    /// Caps how fast a flick can launch the viewport (world units/sec).
    pub const PAN_VEL_MAX: f32 = 12000.0;

    /// Offset the pan target by `delta` (was `pan_camera_target`, minus the
    /// `request_maintenance()` scheduling call, which is a `wl`-side concern).
    pub fn pan_target(&mut self, delta: Vec2) {
        self.target_center.x += delta.x;
        self.target_center.y += delta.y;
    }

    /// Set the target view size, clamped to the zoom bounds (was
    /// `set_camera_target_view_size`).
    pub fn set_target_view_size(&mut self, size: Vec2, zoom_min: f32, zoom_max: f32) {
        self.target_view_size = self.clamp_view_size(size, zoom_min, zoom_max);
    }

    /// Snap both targets to the current live values - used when an external
    /// animation (e.g. a deliberate pan-to) is about to take over, so the
    /// easing doesn't fight it (was `snap_camera_targets_to_live`).
    pub fn snap_targets_to_live(&mut self) {
        self.target_center = self.center;
        self.target_view_size = self.view_size;
    }

    /// Seed inertial pan with a release velocity (world units/sec), clamped
    /// to `PAN_VEL_MAX`. A subsequent `tick` coasts the target and decays
    /// the velocity with friction.
    pub fn fling_pan(&mut self, vel: Vec2) {
        let speed = vel.x.hypot(vel.y);
        self.pan_vel = if speed > Self::PAN_VEL_MAX {
            Vec2 {
                x: vel.x / speed * Self::PAN_VEL_MAX,
                y: vel.y / speed * Self::PAN_VEL_MAX,
            }
        } else {
            vel
        };
    }

    /// Instant (non-inertial) zoom: jump the target size directly by
    /// `steps` multiplicative zoom-steps, no velocity involved (the
    /// non-smooth branch of the old `zoom_by_steps`).
    pub fn zoom_instant_by_steps(
        &mut self,
        steps: f32,
        zoom_step_cfg: f32,
        zoom_min: f32,
        zoom_max: f32,
    ) {
        let steps = steps.clamp(-4.0, 4.0);
        if steps.abs() < f32::EPSILON {
            return;
        }
        let factor = zoom_step(zoom_step_cfg).powf(steps);
        let new_size = Vec2 {
            x: self.target_view_size.x / factor,
            y: self.target_view_size.y / factor,
        };
        self.set_target_view_size(new_size, zoom_min, zoom_max);
    }

    /// Inertial/lens zoom: inject velocity in log space (the smooth branch
    /// of the old `zoom_by_steps`). Repeating in the same direction stacks
    /// velocity (an accelerating ramp); the opposite direction bleeds it off
    /// or reverses.
    pub fn zoom_inject_velocity(&mut self, steps: f32, zoom_step_cfg: f32, smooth_rate_cfg: f32) {
        let steps = steps.clamp(-4.0, 4.0);
        if steps.abs() < f32::EPSILON {
            return;
        }
        let friction = zoom_smooth_rate(smooth_rate_cfg);
        let step_impulse = friction * zoom_step(zoom_step_cfg).ln();
        let cap = step_impulse * Self::ZOOM_VEL_STACK_MAX;
        // zoom-in (+steps) shrinks the view -> negative log velocity.
        self.zoom_log_vel = (self.zoom_log_vel - steps * step_impulse).clamp(-cap, cap);
    }

    /// Reset the zoom target back to `base_size` and clear zoom velocity
    /// (was the guard-free tail of `reset_zoom`). Setting directly to
    /// `base_size` rather than going through `set_target_view_size` is
    /// deliberate: `base_size` is always within the zoom bounds by
    /// construction (`zoom_scale_bounds` guarantees min <= 1.0 <= max), so
    /// the clamp would always be a no-op anyway.
    pub fn reset_zoom_target(&mut self) {
        self.zoom_log_vel = 0.0;
        self.target_view_size = self.base_size;
    }

    /// Advance the camera one tick toward its targets. `dt` is the elapsed
    /// time in seconds (the caller computes this from its own clock - was
    /// `now: Instant` read against `st.ui.render_state.render_last_tick()`,
    /// a `wl`-side concern). Returns whether anything moved, so the caller
    /// can decide whether to keep repainting.
    ///
    /// Ported from `tick_camera_smoothing_inner`. That function took an
    /// extra `passive: bool` distinguishing the actively-interacted-with
    /// monitor from a background one - but every place it changed behavior
    /// (an interaction-state snap check, mirroring into external tuning, a
    /// pan-viewport-change notification) was a `wl`-side side effect around
    /// this call, not part of the numeric integration itself. So there's no
    /// `passive` parameter here; both of the old wl-side wrapper functions
    /// become thin callers of this same method.
    pub fn tick(&mut self, dt: f32, tuning: CameraTickTuning) -> bool {
        if !tuning.physics_enabled {
            let changed =
                self.center != self.target_center || self.view_size != self.target_view_size;
            self.center = self.target_center;
            self.view_size = self.target_view_size;
            self.zoom_log_vel = 0.0;
            self.pan_vel = Vec2 { x: 0.0, y: 0.0 };
            return changed;
        }

        if !tuning.zoom_enabled {
            self.target_view_size = self.base_size;
            self.zoom_log_vel = 0.0;
        }

        let smooth_rate = zoom_smooth_rate(tuning.smooth_rate);
        let alpha = if tuning.zoom_smooth {
            (dt * smooth_rate).clamp(0.08, 0.60)
        } else {
            1.0
        };

        let mut changed = self.tick_pan(dt, smooth_rate, tuning.zoom_smooth, tuning.pan_decay_rate);

        if tuning.zoom_smooth && self.zoom_log_vel.abs() > Self::ZOOM_VEL_EPS {
            // Inertial zoom: integrate log-space velocity with friction so a
            // sweep accelerates as input stacks and then coasts to a smooth
            // stop, like a powered lens. Working in log space keeps the
            // perceptual zoom rate even.
            let friction = smooth_rate;
            let factor = (self.zoom_log_vel * dt).exp();
            let raw = Vec2 {
                x: self.view_size.x * factor,
                y: self.view_size.y * factor,
            };
            let clamped = self.clamp_view_size(raw, tuning.zoom_min, tuning.zoom_max);
            let hit_bound = clamped.x != raw.x || clamped.y != raw.y;
            self.view_size = clamped;
            self.zoom_log_vel *= (-friction * dt).exp();
            if hit_bound || self.zoom_log_vel.abs() <= Self::ZOOM_VEL_EPS {
                self.zoom_log_vel = 0.0;
            }
            // Pin the target to where we coasted so the ease path stays
            // consistent.
            self.target_view_size = clamped;
            changed = true;
        } else {
            self.zoom_log_vel = 0.0;
            let next_size = Vec2 {
                x: self.view_size.x + (self.target_view_size.x - self.view_size.x) * alpha,
                y: self.view_size.y + (self.target_view_size.y - self.view_size.y) * alpha,
            };
            if (self.target_view_size.x - next_size.x).abs() < 0.2 {
                self.view_size.x = self.target_view_size.x;
            } else {
                self.view_size.x = next_size.x;
                changed = true;
            }
            if (self.target_view_size.y - next_size.y).abs() < 0.2 {
                self.view_size.y = self.target_view_size.y;
            } else {
                self.view_size.y = next_size.y;
                changed = true;
            }
        }

        changed
    }

    /// Integrate the moving pan target and its eased follower together.
    /// Exponential gains depend on elapsed time, not the number of frames.
    /// In particular, sampling an Euler-stepped target first makes both fling
    /// distance and the camera's lag depend on output refresh rate.
    fn tick_pan(&mut self, dt: f32, rate: f32, smooth: bool, friction: f32) -> bool {
        let before = self.center;
        let dt = f64::from(dt.max(0.0));
        if dt == 0.0 {
            return self.center != self.target_center
                || self.pan_vel.x != 0.0
                || self.pan_vel.y != 0.0;
        }
        let rate = f64::from(rate);
        let friction = f64::from(pan_decay_rate(friction));
        let speed = f64::from(self.pan_vel.x).hypot(f64::from(self.pan_vel.y));
        // Stop at the same instant at every refresh rate, including a frame
        // spanning the velocity cutoff. Ease toward the stopped target for
        // the remainder of that frame.
        let coast_time = if speed > f64::from(Self::PAN_VEL_EPS) {
            (speed / f64::from(Self::PAN_VEL_EPS)).ln() / friction
        } else {
            0.0
        };
        let coast_dt = dt.min(coast_time);
        let decay = (-friction * coast_dt).exp();
        let target_gain = -(-friction * coast_dt).exp_m1() / friction;
        let ease_decay = (-rate * coast_dt).exp();
        let follower_gain = if rate == friction {
            coast_dt * ease_decay
        } else {
            (decay - ease_decay) / (rate - friction)
        };
        let rest_decay = (-rate * (dt - coast_dt)).exp();
        let stopped = coast_time <= dt;
        let advance = |center: f32, target: f32, velocity: f32| {
            let center = f64::from(center);
            let target = f64::from(target);
            let velocity = f64::from(velocity);
            let next_target = target + velocity * target_gain;
            let next_center = if smooth {
                let coast_center = center
                    + (target - center) * (1.0 - ease_decay)
                    + velocity * (target_gain - follower_gain);
                next_target + (coast_center - next_target) * rest_decay
            } else {
                next_target
            };
            let next_target = next_target as f32;
            let next_center = next_center as f32;
            // The final snap is subpixel. Also terminate an easing tail whose
            // step is smaller than f32 precision far out in the Field.
            let next_center = if stopped
                && ((next_target - next_center).abs() < 0.15 || next_center == center as f32)
            {
                next_target
            } else {
                next_center
            };
            (next_center, next_target)
        };
        (self.center.x, self.target_center.x) =
            advance(self.center.x, self.target_center.x, self.pan_vel.x);
        (self.center.y, self.target_center.y) =
            advance(self.center.y, self.target_center.y, self.pan_vel.y);
        self.pan_vel = if stopped {
            Vec2 { x: 0.0, y: 0.0 }
        } else {
            Vec2 {
                x: (f64::from(self.pan_vel.x) * decay) as f32,
                y: (f64::from(self.pan_vel.y) * decay) as f32,
            }
        };
        // Report the final changed frame as well as unfinished motion, so
        // damage-driven rendering presents the endpoint before returning idle.
        self.center != before || self.center != self.target_center || !stopped
    }
}

/// Tuning knobs `Camera::tick` reads. Mirrors the `DecayPolicy`/
/// `FocusRingDecayPolicy` pattern already used in this crate: a small,
/// explicit params struct instead of the method reaching into external
/// config directly.
#[derive(Clone, Copy, Debug)]
pub struct CameraTickTuning {
    pub physics_enabled: bool,
    pub zoom_enabled: bool,
    pub zoom_smooth: bool,
    /// Raw configured zoom smoothing rate - `tick` clamps it internally via
    /// `zoom_smooth_rate`.
    pub smooth_rate: f32,
    /// Raw configured pan friction - `tick` clamps it internally via
    /// `pan_decay_rate`.
    pub pan_decay_rate: f32,
    pub zoom_min: f32,
    pub zoom_max: f32,
}

/// Clamp a configured pan-friction rate to a sane range.
pub fn pan_decay_rate(rate: f32) -> f32 {
    rate.clamp(0.5, 30.0)
}

/// Clamp a configured zoom-per-step factor to a sane minimum (must be > 1.0
/// to mean anything as a multiplicative step).
pub fn zoom_step(step: f32) -> f32 {
    step.max(1.001)
}

/// Clamp configured zoom min/max into a sane, ordered range.
pub fn zoom_scale_bounds(zoom_min: f32, zoom_max: f32) -> (f32, f32) {
    let min = zoom_min.clamp(0.05, 1.0);
    let max = zoom_max.max(min).clamp(1.0, 16.0);
    (min, max)
}

/// Clamp a configured zoom smoothing rate to a sane range.
pub fn zoom_smooth_rate(rate: f32) -> f32 {
    rate.clamp(0.1, 120.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_step_has_a_floor() {
        assert_eq!(zoom_step(0.5), 1.001);
        assert_eq!(zoom_step(2.0), 2.0);
    }

    #[test]
    fn zoom_scale_bounds_orders_and_clamps() {
        // max below min gets pulled up to min.
        assert_eq!(zoom_scale_bounds(0.5, 0.1), (0.5, 1.0));
        // out-of-range values get clamped into the sane window.
        assert_eq!(zoom_scale_bounds(0.0, 100.0), (0.05, 16.0));
        // ordinary values pass through.
        assert_eq!(zoom_scale_bounds(0.5, 2.0), (0.5, 2.0));
    }

    #[test]
    fn zoom_smooth_rate_clamps_to_sane_range() {
        assert_eq!(zoom_smooth_rate(0.0), 0.1);
        assert_eq!(zoom_smooth_rate(1000.0), 120.0);
        assert_eq!(zoom_smooth_rate(10.0), 10.0);
    }

    #[test]
    fn clamp_view_size_respects_base_size_and_bounds() {
        let cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });

        // Zoomed in too far (size too small) clamps to base/max_zoom.
        let too_small = cam.clamp_view_size(Vec2 { x: 1.0, y: 1.0 }, 0.5, 2.0);
        assert_eq!(too_small, Vec2 { x: 400.0, y: 300.0 });

        // Zoomed out too far (size too large) clamps to base/min_zoom.
        let too_large = cam.clamp_view_size(
            Vec2 {
                x: 100_000.0,
                y: 100_000.0,
            },
            0.5,
            2.0,
        );
        assert_eq!(
            too_large,
            Vec2 {
                x: 1600.0,
                y: 1200.0
            }
        );

        // Within bounds passes through unchanged.
        let within = cam.clamp_view_size(Vec2 { x: 900.0, y: 700.0 }, 0.5, 2.0);
        assert_eq!(within, Vec2 { x: 900.0, y: 700.0 });
    }

    #[test]
    fn pan_target_offsets_target_center_only() {
        let mut cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.pan_target(Vec2 { x: 10.0, y: -5.0 });
        assert_eq!(cam.target_center, Vec2 { x: 10.0, y: -5.0 });
        // Live center is untouched - that's tick()'s job.
        assert_eq!(cam.center, Vec2 { x: 0.0, y: 0.0 });
    }

    #[test]
    fn snap_targets_to_live_matches_current_live_values() {
        let mut cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.target_center = Vec2 { x: 999.0, y: 999.0 };
        cam.target_view_size = Vec2 { x: 1.0, y: 1.0 };
        cam.snap_targets_to_live();
        assert_eq!(cam.target_center, cam.center);
        assert_eq!(cam.target_view_size, cam.view_size);
    }

    #[test]
    fn fling_pan_caps_speed_at_max() {
        let mut cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.fling_pan(Vec2 {
            x: Camera::PAN_VEL_MAX * 10.0,
            y: 0.0,
        });
        assert_eq!(cam.pan_vel.x, Camera::PAN_VEL_MAX);

        cam.fling_pan(Vec2 { x: 50.0, y: 0.0 });
        assert_eq!(cam.pan_vel, Vec2 { x: 50.0, y: 0.0 });
    }

    #[test]
    fn zoom_instant_by_steps_shrinks_target_when_zooming_in() {
        let mut cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.zoom_instant_by_steps(1.0, 1.1, 0.1, 4.0);
        assert!(cam.target_view_size.x < 800.0);
        assert!(cam.target_view_size.y < 600.0);
    }

    #[test]
    fn zoom_instant_by_steps_ignores_near_zero_steps() {
        let mut cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.zoom_instant_by_steps(0.0, 1.1, 0.1, 4.0);
        assert_eq!(cam.target_view_size, Vec2 { x: 800.0, y: 600.0 });
    }

    #[test]
    fn zoom_inject_velocity_stacks_in_same_direction() {
        let mut cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.zoom_inject_velocity(1.0, 1.1, 8.0);
        let after_one = cam.zoom_log_vel;
        assert_ne!(after_one, 0.0);

        cam.zoom_inject_velocity(1.0, 1.1, 8.0);
        // Repeating in the same direction stacks (grows) the velocity magnitude.
        assert!(cam.zoom_log_vel.abs() > after_one.abs());
    }

    #[test]
    fn reset_zoom_target_returns_to_base_size_and_clears_velocity() {
        let mut cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.target_view_size = Vec2 {
            x: 1234.0,
            y: 1234.0,
        };
        cam.zoom_log_vel = 42.0;
        cam.reset_zoom_target();
        assert_eq!(cam.target_view_size, cam.base_size);
        assert_eq!(cam.zoom_log_vel, 0.0);
    }

    fn default_tick_tuning() -> CameraTickTuning {
        CameraTickTuning {
            physics_enabled: true,
            zoom_enabled: true,
            zoom_smooth: true,
            smooth_rate: 10.0,
            pan_decay_rate: 8.0,
            zoom_min: 0.1,
            zoom_max: 4.0,
        }
    }

    #[test]
    fn tick_with_physics_disabled_snaps_instantly() {
        let mut cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.target_center = Vec2 { x: 100.0, y: 50.0 };
        cam.target_view_size = Vec2 { x: 400.0, y: 300.0 };
        cam.pan_vel = Vec2 { x: 999.0, y: 999.0 };
        cam.zoom_log_vel = 5.0;

        let mut tuning = default_tick_tuning();
        tuning.physics_enabled = false;

        let changed = cam.tick(1.0 / 60.0, tuning);

        assert!(changed);
        assert_eq!(cam.center, cam.target_center);
        assert_eq!(cam.view_size, cam.target_view_size);
        assert_eq!(cam.pan_vel, Vec2 { x: 0.0, y: 0.0 });
        assert_eq!(cam.zoom_log_vel, 0.0);

        // Already at rest at the target: no further change.
        assert!(!cam.tick(1.0 / 60.0, tuning));
    }

    #[test]
    fn tick_eases_center_toward_target_without_overshoot() {
        let mut cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.target_center = Vec2 { x: 100.0, y: 0.0 };

        let tuning = default_tick_tuning();
        let mut steps = 0;
        while cam.center != cam.target_center && steps < 10_000 {
            cam.tick(1.0 / 60.0, tuning);
            steps += 1;
        }

        assert_eq!(cam.center, cam.target_center);
        assert!(steps > 1, "expected easing to take more than one tick");
    }

    #[test]
    fn tick_decays_pan_velocity_toward_rest() {
        let mut cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.fling_pan(Vec2 { x: 500.0, y: 0.0 });

        let tuning = default_tick_tuning();
        for _ in 0..600 {
            cam.tick(1.0 / 60.0, tuning);
            if cam.pan_vel == (Vec2 { x: 0.0, y: 0.0 }) {
                break;
            }
        }

        assert_eq!(cam.pan_vel, Vec2 { x: 0.0, y: 0.0 });
        // The fling should have actually moved the camera before settling.
        assert!(cam.center.x > 0.0);
    }

    fn pan_after_ticks(hz: u32, seconds: u32, velocity: Vec2, rate: f32) -> Camera {
        let mut cam = Camera::new(Vec2 { x: 400.0, y: 300.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.pan_target(Vec2 {
            x: 300.0,
            y: -120.0,
        });
        cam.fling_pan(velocity);
        let tuning = CameraTickTuning {
            smooth_rate: rate,
            pan_decay_rate: 6.0,
            ..default_tick_tuning()
        };
        for _ in 0..hz * seconds {
            cam.tick(1.0 / hz as f32, tuning);
        }
        cam
    }

    #[test]
    fn pan_trajectory_and_fling_distance_are_refresh_independent() {
        for velocity in [
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 1500.0,
                y: -750.0,
            },
        ] {
            // Include equal easing/friction rates, where the analytic solution
            // needs its limiting form rather than division by their difference.
            for rate in [1.0, 6.0, 12.5, 120.0] {
                let reference = pan_after_ticks(60, 1, velocity, rate);
                for hz in [75, 120, 180, 240, 360] {
                    let actual = pan_after_ticks(hz, 1, velocity, rate);
                    assert!(
                        (actual.center.x - reference.center.x).abs() < 0.03,
                        "hz={hz} rate={rate} center={:?} reference={:?}",
                        actual.center,
                        reference.center
                    );
                    assert!((actual.center.y - reference.center.y).abs() < 0.03);
                    assert!((actual.target_center.x - reference.target_center.x).abs() < 0.03);
                    assert!((actual.target_center.y - reference.target_center.y).abs() < 0.03);
                }
            }
        }
    }

    #[test]
    fn final_pan_snap_requests_a_frame_before_becoming_idle() {
        let mut cam = Camera::new(Vec2 { x: 10.49, y: 20.49 }, Vec2 { x: 800.0, y: 600.0 });
        cam.target_center = Vec2 { x: 10.51, y: 20.51 };
        assert!(cam.tick(1.0 / 180.0, default_tick_tuning()));
        assert_eq!(cam.center, cam.target_center);
        assert!(!cam.tick(1.0 / 180.0, default_tick_tuning()));
    }

    #[test]
    fn pan_easing_survives_large_field_coordinates() {
        let mut cam = Camera::new(
            Vec2 {
                x: 4_000_000.0,
                y: -4_000_000.0,
            },
            Vec2 { x: 800.0, y: 600.0 },
        );
        cam.pan_target(Vec2 { x: 0.25, y: -0.25 });
        for _ in 0..1000 {
            cam.tick(1.0 / 360.0, default_tick_tuning());
        }
        assert_eq!(cam.center, cam.target_center);
        assert!(!cam.tick(1.0 / 360.0, default_tick_tuning()));
    }

    #[test]
    fn irregular_pan_frames_and_cutoff_preserve_fling_endpoint() {
        let velocity = Vec2 {
            x: 1500.0,
            y: -750.0,
        };
        let reference = pan_after_ticks(60, 3, velocity, 12.5);
        let mut cam = Camera::new(Vec2 { x: 400.0, y: 300.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.pan_target(Vec2 {
            x: 300.0,
            y: -120.0,
        });
        cam.fling_pan(velocity);
        let tuning = CameraTickTuning {
            smooth_rate: 12.5,
            pan_decay_rate: 6.0,
            ..default_tick_tuning()
        };
        let mut remaining: f32 = 3.0;
        for dt in [0.004_f32, 0.011, 0.05, 0.006, 0.017].into_iter().cycle() {
            let dt = dt.min(remaining);
            cam.tick(dt, tuning);
            remaining -= dt;
            if remaining <= 0.0 {
                break;
            }
        }
        assert_eq!(cam.pan_vel, Vec2 { x: 0.0, y: 0.0 });
        assert_eq!(cam.center, cam.target_center);
        assert!((cam.center.x - reference.center.x).abs() < 0.03);
        assert!((cam.center.y - reference.center.y).abs() < 0.03);
        for hz in [75, 180, 360] {
            let actual = pan_after_ticks(hz, 3, velocity, 12.5);
            assert!((actual.center.x - reference.center.x).abs() < 0.03);
            assert!((actual.center.y - reference.center.y).abs() < 0.03);
        }
    }

    #[test]
    fn interrupted_pan_discards_previous_fling_and_settles_monotonically() {
        let mut cam = pan_after_ticks(
            180,
            1,
            Vec2 {
                x: 1500.0,
                y: -750.0,
            },
            1.0,
        );
        cam.snap_targets_to_live();
        cam.pan_vel = Vec2 { x: 0.0, y: 0.0 };
        cam.pan_target(Vec2 { x: -60.0, y: 30.0 });
        for _ in 0..2000 {
            let previous = cam.center;
            if !cam.tick(1.0 / 180.0, default_tick_tuning()) {
                break;
            }
            assert!(cam.center.x <= previous.x && cam.center.x >= cam.target_center.x);
            assert!(cam.center.y >= previous.y && cam.center.y <= cam.target_center.y);
        }
        assert_eq!(cam.center, cam.target_center);
    }

    #[test]
    fn tick_zoom_disabled_eases_view_size_back_to_base() {
        let mut cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        cam.view_size = Vec2 { x: 200.0, y: 150.0 };
        cam.target_view_size = Vec2 { x: 200.0, y: 150.0 };

        let mut tuning = default_tick_tuning();
        tuning.zoom_enabled = false;

        for _ in 0..1000 {
            cam.tick(1.0 / 60.0, tuning);
        }

        assert_eq!(cam.view_size, cam.base_size);
    }

    #[test]
    fn tick_zoom_velocity_respects_clamp_and_settles() {
        let mut cam = Camera::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 });
        let tuning = default_tick_tuning();

        cam.zoom_inject_velocity(4.0, 1.1, tuning.smooth_rate);
        assert_ne!(cam.zoom_log_vel, 0.0);

        for _ in 0..2000 {
            cam.tick(1.0 / 60.0, tuning);
        }

        assert_eq!(cam.zoom_log_vel, 0.0);
        let (min_zoom, max_zoom) = zoom_scale_bounds(tuning.zoom_min, tuning.zoom_max);
        assert!(cam.view_size.x >= cam.base_size.x / max_zoom - 0.01);
        assert!(cam.view_size.x <= cam.base_size.x / min_zoom + 0.01);
    }

    #[test]
    fn new_camera_starts_at_rest_with_targets_matching_live() {
        let center = Vec2 { x: 10.0, y: 20.0 };
        let base_size = Vec2 { x: 800.0, y: 600.0 };
        let cam = Camera::new(center, base_size);

        assert_eq!(cam.center, center);
        assert_eq!(cam.view_size, base_size);
        assert_eq!(cam.base_size, base_size);
        assert_eq!(cam.target_center, center);
        assert_eq!(cam.target_view_size, base_size);
        assert_eq!(cam.pan_vel, Vec2 { x: 0.0, y: 0.0 });
        assert_eq!(cam.zoom_log_vel, 0.0);
    }

    #[test]
    fn viewport_reflects_live_center_and_view_size() {
        let cam = Camera::new(Vec2 { x: 1.0, y: 2.0 }, Vec2 { x: 100.0, y: 50.0 });
        let vp = cam.viewport();
        assert_eq!(vp.center, Vec2 { x: 1.0, y: 2.0 });
        assert_eq!(vp.size, Vec2 { x: 100.0, y: 50.0 });
    }
}
