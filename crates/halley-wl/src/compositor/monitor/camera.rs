use super::*;
use std::ops::{Deref, DerefMut};

pub(crate) struct CameraController<T> {
    st: T,
}

pub(crate) fn camera_controller<T>(st: T) -> CameraController<T> {
    CameraController { st }
}

impl<T: Deref<Target = Halley>> Deref for CameraController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for CameraController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

#[inline]
fn zoom_step(st: &Halley) -> f32 {
    st.runtime.tuning.zoom_step.max(1.001)
}

#[inline]
fn zoom_scale_bounds(st: &Halley) -> (f32, f32) {
    let min = st.runtime.tuning.zoom_min.clamp(0.05, 1.0);
    let max = st.runtime.tuning.zoom_max.max(min).clamp(1.0, 16.0);
    (min, max)
}

#[inline]
fn zoom_smooth_rate(st: &Halley) -> f32 {
    st.runtime.tuning.zoom_smooth_rate.clamp(0.1, 120.0)
}

#[inline]
pub(crate) fn camera_view_size(st: &Halley) -> Vec2 {
    st.model.zoom_ref_size
}

impl<T: Deref<Target = Halley>> CameraController<T> {
    #[inline]
    pub(crate) fn view_size(&self) -> Vec2 {
        camera_view_size(self)
    }

    #[inline]
    pub(crate) fn zoom_blocked_by_interaction(&self) -> bool {
        zoom_blocked_by_interaction(self)
    }
}

impl<T: DerefMut<Target = Halley>> CameraController<T> {
    #[inline]
    pub(crate) fn pan_target(&mut self, delta: Vec2) {
        pan_camera_target(self, delta)
    }

    #[inline]
    pub(crate) fn set_target_view_size(&mut self, size: Vec2) {
        set_camera_target_view_size(self, size)
    }

    #[inline]
    pub(crate) fn snap_targets_to_live(&mut self) {
        snap_camera_targets_to_live(self)
    }

    #[inline]
    pub(crate) fn update_zoom_live_surface_sizes(&mut self) {
        update_zoom_live_surface_sizes(self)
    }

    #[inline]
    pub(crate) fn zoom_by_steps(&mut self, steps: f32) {
        zoom_by_steps(self, steps)
    }

    #[inline]
    pub(crate) fn reset_zoom(&mut self) {
        reset_zoom(self)
    }

    #[inline]
    pub(crate) fn tick_smoothing(&mut self, now: Instant) {
        tick_camera_smoothing(self, now)
    }
}

#[inline]
pub(crate) fn pan_camera_target(st: &mut Halley, delta: Vec2) {
    st.model.camera_target_center = Vec2 {
        x: st.model.camera_target_center.x + delta.x,
        y: st.model.camera_target_center.y + delta.y,
    };
    st.request_maintenance();
}

#[inline]
pub(crate) fn set_camera_target_view_size(st: &mut Halley, size: Vec2) {
    st.model.camera_target_view_size = clamp_camera_view_size(st, size);
    st.request_maintenance();
}

#[inline]
pub(crate) fn snap_camera_targets_to_live(st: &mut Halley) {
    st.model.camera_target_center = st.model.viewport.center;
    st.model.camera_target_view_size = st.model.zoom_ref_size;
}

#[inline]
pub(crate) fn clamp_camera_view_size(st: &Halley, size: Vec2) -> Vec2 {
    let base = st.model.viewport.size;
    let (min_zoom, max_zoom) = zoom_scale_bounds(st);
    Vec2 {
        x: size.x.clamp(base.x / max_zoom, base.x / min_zoom),
        y: size.y.clamp(base.y / max_zoom, base.y / min_zoom),
    }
}

#[inline]
pub(crate) fn zoom_blocked_by_interaction(st: &Halley) -> bool {
    st.has_active_cluster_workspace()
        || st
            .model
            .fullscreen_state
            .fullscreen_active_node
            .contains_key(st.model.monitor_state.current_monitor.as_str())
        || st.cluster_mode_active()
        || st.input.interaction_state.grabbed_edge_pan_active
        || st
            .input
            .interaction_state
            .grabbed_edge_pan_monitor
            .is_some()
        || st.input.interaction_state.grabbed_edge_pan_pressure.x > 0.01
        || st.input.interaction_state.grabbed_edge_pan_pressure.y > 0.01
}

pub(crate) fn update_zoom_live_surface_sizes(st: &mut Halley) {
    st.ui.render_state.zoom_resize_fallback.clear();
    st.ui.render_state.zoom_resize_reject_streak.clear();
    st.ui.render_state.zoom_resize_static_streak.clear();
    st.ui.render_state.zoom_last_observed_size.clear();
}

pub(crate) fn zoom_by_steps(st: &mut Halley, steps: f32) {
    if !st.runtime.tuning.zoom_enabled {
        return;
    }
    if zoom_blocked_by_interaction(st) {
        return;
    }
    let steps = steps.clamp(-4.0, 4.0);
    if steps.abs() < f32::EPSILON {
        return;
    }

    let factor = zoom_step(st).powf(steps);
    set_camera_target_view_size(
        st,
        Vec2 {
            x: st.model.camera_target_view_size.x / factor,
            y: st.model.camera_target_view_size.y / factor,
        },
    );
}

pub(crate) fn reset_zoom(st: &mut Halley) {
    if !st.runtime.tuning.zoom_enabled {
        return;
    }
    if zoom_blocked_by_interaction(st) {
        return;
    }
    set_camera_target_view_size(st, st.model.viewport.size);
}

pub(crate) fn tick_camera_smoothing(st: &mut Halley, now: Instant) {
    if st.input.interaction_state.viewport_pan_anim.is_some() {
        snap_camera_targets_to_live(st);
        return;
    }

    if st.input.interaction_state.grabbed_edge_pan_active {
        st.model.viewport.center = st.model.camera_target_center;
        st.model.zoom_ref_size = st.model.camera_target_view_size;
        st.runtime.tuning.viewport_center = st.model.viewport.center;
        st.runtime.tuning.viewport_size = st.model.zoom_ref_size;
        crate::compositor::monitor::state::sync_current_monitor_state(st);
        return;
    }

    if !st.runtime.tuning.physics_enabled {
        st.model.viewport.center = st.model.camera_target_center;
        st.model.zoom_ref_size = st.model.camera_target_view_size;
        st.runtime.tuning.viewport_center = st.model.viewport.center;
        st.runtime.tuning.viewport_size = st.model.zoom_ref_size;
        return;
    }

    let dt = now
        .saturating_duration_since(st.ui.render_state.render_last_tick)
        .as_secs_f32()
        .clamp(1.0 / 240.0, 1.0 / 20.0);
    if !st.runtime.tuning.zoom_enabled {
        st.model.camera_target_view_size = st.model.viewport.size;
    }

    let smooth_rate = zoom_smooth_rate(st);
    let center_alpha = if st.runtime.tuning.zoom_smooth {
        (dt * smooth_rate).clamp(0.08, 0.60)
    } else {
        1.0
    };
    let zoom_alpha = if st.runtime.tuning.zoom_smooth {
        (dt * smooth_rate).clamp(0.08, 0.60)
    } else {
        1.0
    };

    let mut changed = false;

    let next_center = Vec2 {
        x: st.model.viewport.center.x
            + (st.model.camera_target_center.x - st.model.viewport.center.x) * center_alpha,
        y: st.model.viewport.center.y
            + (st.model.camera_target_center.y - st.model.viewport.center.y) * center_alpha,
    };
    if (st.model.camera_target_center.x - next_center.x).abs() < 0.15 {
        st.model.viewport.center.x = st.model.camera_target_center.x;
    } else {
        st.model.viewport.center.x = next_center.x;
        changed = true;
    }
    if (st.model.camera_target_center.y - next_center.y).abs() < 0.15 {
        st.model.viewport.center.y = st.model.camera_target_center.y;
    } else {
        st.model.viewport.center.y = next_center.y;
        changed = true;
    }

    let next_size = Vec2 {
        x: st.model.zoom_ref_size.x
            + (st.model.camera_target_view_size.x - st.model.zoom_ref_size.x) * zoom_alpha,
        y: st.model.zoom_ref_size.y
            + (st.model.camera_target_view_size.y - st.model.zoom_ref_size.y) * zoom_alpha,
    };
    if (st.model.camera_target_view_size.x - next_size.x).abs() < 0.2 {
        st.model.zoom_ref_size.x = st.model.camera_target_view_size.x;
    } else {
        st.model.zoom_ref_size.x = next_size.x;
        changed = true;
    }
    if (st.model.camera_target_view_size.y - next_size.y).abs() < 0.2 {
        st.model.zoom_ref_size.y = st.model.camera_target_view_size.y;
    } else {
        st.model.zoom_ref_size.y = next_size.y;
        changed = true;
    }

    st.runtime.tuning.viewport_center = st.model.viewport.center;
    st.runtime.tuning.viewport_size = st.model.zoom_ref_size;
    if changed {
        st.request_maintenance();
    }
}

pub fn active_zoom_lock_scale(_st: &Halley) -> f32 {
    1.0
}

/// Ratio of screen pixels to world-view units for the current zoom level.
///
/// - At 1× zoom (zoom_ref_size == viewport.size) -> returns 1.0.
/// - Zoomed in (zoom_ref_size shrunk) -> returns > 1.0; windows appear larger.
/// - Zoomed out (zoom_ref_size grown)  -> returns < 1.0; windows appear smaller.
///
/// Multiplying all per-window screen-pixel dimensions by this value produces
/// optical (lens) zoom: positions, sizes, and gaps all scale by the same factor.
pub fn camera_render_scale(st: &Halley) -> f32 {
    let vp_w = st.model.viewport.size.x.max(1.0);
    let view_w = camera_view_size(st).x.max(1.0);
    (vp_w / view_w).max(0.01)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_on_current_monitor_blocks_zoom_changes() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_current_monitor(fullscreen);
        let current_monitor = state.model.monitor_state.current_monitor.clone();
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert(current_monitor, fullscreen);

        let base = state.model.viewport.size;
        let zoomed_out = Vec2 {
            x: base.x * 1.5,
            y: base.y * 1.5,
        };
        state.model.camera_target_view_size = zoomed_out;
        camera_controller(&mut state).reset_zoom();
        assert_eq!(state.model.camera_target_view_size, zoomed_out);

        state.model.camera_target_view_size = base;
        camera_controller(&mut state).zoom_by_steps(-1.0);
        assert_eq!(state.model.camera_target_view_size, base);
    }

    #[test]
    fn fullscreen_on_other_monitor_does_not_block_zoom() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let fullscreen_left = state.model.field.spawn_surface(
            "fullscreen-left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_monitor(fullscreen_left, "left");
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert("left".to_string(), fullscreen_left);

        state.set_interaction_monitor("right");
        state.set_focused_monitor("right");
        let _ = state.activate_monitor("right");

        let before = state.model.camera_target_view_size;
        camera_controller(&mut state).zoom_by_steps(-1.0);

        assert!(state.model.camera_target_view_size.x > before.x);
        assert!(state.model.camera_target_view_size.y > before.y);
    }

    #[test]
    fn zoom_disabled_ignores_zoom_inputs() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.zoom_enabled = false;
        let mut state = Halley::new_for_test(&dh, tuning);

        let before = state.model.camera_target_view_size;
        camera_controller(&mut state).zoom_by_steps(1.0);
        camera_controller(&mut state).reset_zoom();

        assert_eq!(state.model.camera_target_view_size, before);
    }

    #[test]
    fn camera_view_size_clamps_to_configured_zoom_limits() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.zoom_min = 0.5;
        tuning.zoom_max = 1.5;
        let mut state = Halley::new_for_test(&dh, tuning);
        let base = state.model.viewport.size;

        camera_controller(&mut state).set_target_view_size(Vec2 {
            x: base.x * 10.0,
            y: base.y * 10.0,
        });
        assert_eq!(state.model.camera_target_view_size.x, base.x / 0.5);
        assert_eq!(state.model.camera_target_view_size.y, base.y / 0.5);

        camera_controller(&mut state).set_target_view_size(Vec2 {
            x: base.x * 0.1,
            y: base.y * 0.1,
        });
        assert_eq!(state.model.camera_target_view_size.x, base.x / 1.5);
        assert_eq!(state.model.camera_target_view_size.y, base.y / 1.5);
    }
}
