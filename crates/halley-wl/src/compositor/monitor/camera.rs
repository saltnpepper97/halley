use super::*;

impl Halley {
    const ZOOM_PER_STEP: f32 = 1.10;
    const CAMERA_SMOOTH_CENTER_RATE: f32 = 11.0;
    const CAMERA_SMOOTH_ZOOM_RATE: f32 = 12.5;

    #[inline]
    pub(crate) fn camera_view_size(&self) -> Vec2 {
        self.model.zoom_ref_size
    }

    #[inline]
    pub(crate) fn pan_camera_target(&mut self, delta: Vec2) {
        self.model.camera_target_center = Vec2 {
            x: self.model.camera_target_center.x + delta.x,
            y: self.model.camera_target_center.y + delta.y,
        };
        self.request_maintenance();
    }

    #[inline]
    pub(crate) fn set_camera_target_view_size(&mut self, size: Vec2) {
        self.model.camera_target_view_size = self.clamp_camera_view_size(size);
        self.request_maintenance();
    }

    #[inline]
    pub(crate) fn snap_camera_targets_to_live(&mut self) {
        self.model.camera_target_center = self.model.viewport.center;
        self.model.camera_target_view_size = self.model.zoom_ref_size;
    }

    #[inline]
    pub(crate) fn clamp_camera_view_size(&self, size: Vec2) -> Vec2 {
        let base = self.model.viewport.size;
        Vec2 {
            x: size.x.clamp(base.x * 0.82, base.x * 4.0),
            y: size.y.clamp(base.y * 0.82, base.y * 4.0),
        }
    }

    #[inline]
    pub(crate) fn zoom_blocked_by_interaction(&self) -> bool {
        self.has_active_cluster_workspace()
            || self
                .model
                .fullscreen_state
                .fullscreen_active_node
                .contains_key(self.model.monitor_state.current_monitor.as_str())
            || self.cluster_mode_active()
            || self.input.interaction_state.grabbed_edge_pan_active
            || self
                .input
                .interaction_state
                .grabbed_edge_pan_monitor
                .is_some()
            || self.input.interaction_state.grabbed_edge_pan_pressure.x > 0.01
            || self.input.interaction_state.grabbed_edge_pan_pressure.y > 0.01
    }

    pub(crate) fn update_zoom_live_surface_sizes(&mut self) {
        self.ui.render_state.zoom_resize_fallback.clear();
        self.ui.render_state.zoom_resize_reject_streak.clear();
        self.ui.render_state.zoom_resize_static_streak.clear();
        self.ui.render_state.zoom_last_observed_size.clear();
    }

    pub(crate) fn zoom_by_steps(&mut self, steps: f32) {
        if self.zoom_blocked_by_interaction() {
            return;
        }
        let steps = steps.clamp(-4.0, 4.0);
        if steps.abs() < f32::EPSILON {
            return;
        }

        let factor = Self::ZOOM_PER_STEP.powf(steps);
        self.set_camera_target_view_size(Vec2 {
            x: self.model.camera_target_view_size.x / factor,
            y: self.model.camera_target_view_size.y / factor,
        });
    }

    pub(crate) fn reset_zoom(&mut self) {
        if self.zoom_blocked_by_interaction() {
            return;
        }
        self.set_camera_target_view_size(self.model.viewport.size);
    }

    pub(crate) fn tick_camera_smoothing(&mut self, now: Instant) {
        if self.input.interaction_state.viewport_pan_anim.is_some() {
            self.snap_camera_targets_to_live();
            return;
        }

        if self.input.interaction_state.grabbed_edge_pan_active {
            self.model.viewport.center = self.model.camera_target_center;
            self.model.zoom_ref_size = self.model.camera_target_view_size;
            self.runtime.tuning.viewport_center = self.model.viewport.center;
            self.runtime.tuning.viewport_size = self.model.zoom_ref_size;
            self.sync_current_monitor_state();
            return;
        }

        if !self.runtime.tuning.physics_enabled {
            self.model.viewport.center = self.model.camera_target_center;
            self.model.zoom_ref_size = self.model.camera_target_view_size;
            self.runtime.tuning.viewport_center = self.model.viewport.center;
            self.runtime.tuning.viewport_size = self.model.zoom_ref_size;
            return;
        }

        let dt = now
            .saturating_duration_since(self.ui.render_state.render_last_tick)
            .as_secs_f32()
            .clamp(1.0 / 240.0, 1.0 / 20.0);
        let center_alpha = (dt * Self::CAMERA_SMOOTH_CENTER_RATE).clamp(0.08, 0.55);
        let zoom_alpha = (dt * Self::CAMERA_SMOOTH_ZOOM_RATE).clamp(0.08, 0.60);

        let mut changed = false;

        let next_center = Vec2 {
            x: self.model.viewport.center.x
                + (self.model.camera_target_center.x - self.model.viewport.center.x) * center_alpha,
            y: self.model.viewport.center.y
                + (self.model.camera_target_center.y - self.model.viewport.center.y) * center_alpha,
        };
        if (self.model.camera_target_center.x - next_center.x).abs() < 0.15 {
            self.model.viewport.center.x = self.model.camera_target_center.x;
        } else {
            self.model.viewport.center.x = next_center.x;
            changed = true;
        }
        if (self.model.camera_target_center.y - next_center.y).abs() < 0.15 {
            self.model.viewport.center.y = self.model.camera_target_center.y;
        } else {
            self.model.viewport.center.y = next_center.y;
            changed = true;
        }

        let next_size = Vec2 {
            x: self.model.zoom_ref_size.x
                + (self.model.camera_target_view_size.x - self.model.zoom_ref_size.x) * zoom_alpha,
            y: self.model.zoom_ref_size.y
                + (self.model.camera_target_view_size.y - self.model.zoom_ref_size.y) * zoom_alpha,
        };
        if (self.model.camera_target_view_size.x - next_size.x).abs() < 0.2 {
            self.model.zoom_ref_size.x = self.model.camera_target_view_size.x;
        } else {
            self.model.zoom_ref_size.x = next_size.x;
            changed = true;
        }
        if (self.model.camera_target_view_size.y - next_size.y).abs() < 0.2 {
            self.model.zoom_ref_size.y = self.model.camera_target_view_size.y;
        } else {
            self.model.zoom_ref_size.y = next_size.y;
            changed = true;
        }

        self.runtime.tuning.viewport_center = self.model.viewport.center;
        self.runtime.tuning.viewport_size = self.model.zoom_ref_size;
        if changed {
            self.request_maintenance();
        }
    }

    pub fn active_zoom_lock_scale(&self) -> f32 {
        1.0
    }

    /// Ratio of screen pixels to world-view units for the current zoom level.
    ///
    /// - At 1× zoom (zoom_ref_size == viewport.size) → returns 1.0.
    /// - Zoomed in (zoom_ref_size shrunk) → returns > 1.0; windows appear larger.
    /// - Zoomed out (zoom_ref_size grown)  → returns < 1.0; windows appear smaller.
    ///
    /// Multiplying all per-window screen-pixel dimensions by this value produces
    /// optical (lens) zoom: positions, sizes, and gaps all scale by the same factor.
    pub fn camera_render_scale(&self) -> f32 {
        let vp_w = self.model.viewport.size.x.max(1.0);
        let view_w = self.camera_view_size().x.max(1.0);
        (vp_w / view_w).max(0.01)
    }
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
        state.reset_zoom();
        assert_eq!(state.model.camera_target_view_size, zoomed_out);

        state.model.camera_target_view_size = base;
        state.zoom_by_steps(-1.0);
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
        state.zoom_by_steps(-1.0);

        assert!(state.model.camera_target_view_size.x > before.x);
        assert!(state.model.camera_target_view_size.y > before.y);
    }
}
