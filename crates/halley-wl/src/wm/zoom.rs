use super::*;

impl HalleyWlState {
    const ZOOM_PER_STEP: f32 = 1.10;
    const CAMERA_SMOOTH_CENTER_RATE: f32 = 11.0;
    const CAMERA_SMOOTH_ZOOM_RATE: f32 = 12.5;

    #[inline]
    pub(crate) fn camera_view_size(&self) -> Vec2 {
        self.zoom_ref_size
    }

    #[inline]
    pub(crate) fn pan_camera_target(&mut self, delta: Vec2) {
        self.camera_target_center = Vec2 {
            x: self.camera_target_center.x + delta.x,
            y: self.camera_target_center.y + delta.y,
        };
        self.request_maintenance();
    }

    #[inline]
    pub(crate) fn set_camera_target_view_size(&mut self, size: Vec2) {
        self.camera_target_view_size = self.clamp_camera_view_size(size);
        self.request_maintenance();
    }

    #[inline]
    pub(crate) fn snap_camera_targets_to_live(&mut self) {
        self.camera_target_center = self.viewport.center;
        self.camera_target_view_size = self.zoom_ref_size;
    }

    #[inline]
    pub(crate) fn clamp_camera_view_size(&self, size: Vec2) -> Vec2 {
        let base = self.viewport.size;
        Vec2 {
            x: size.x.clamp(base.x * 0.82, base.x * 4.0),
            y: size.y.clamp(base.y * 0.82, base.y * 4.0),
        }
    }

    pub(crate) fn update_zoom_live_surface_sizes(&mut self) {
        self.render_state.zoom_resize_fallback.clear();
        self.render_state.zoom_resize_reject_streak.clear();
        self.render_state.zoom_resize_static_streak.clear();
        self.render_state.zoom_last_observed_size.clear();
    }

    pub(crate) fn zoom_by_steps(&mut self, steps: f32) {
        let steps = steps.clamp(-4.0, 4.0);
        if steps.abs() < f32::EPSILON {
            return;
        }

        let factor = Self::ZOOM_PER_STEP.powf(steps);
        self.set_camera_target_view_size(Vec2 {
            x: self.camera_target_view_size.x / factor,
            y: self.camera_target_view_size.y / factor,
        });
    }

    pub(crate) fn reset_zoom(&mut self) {
        self.set_camera_target_view_size(self.viewport.size);
    }

    pub(crate) fn tick_camera_smoothing(&mut self, now: Instant) {
        if self.interaction_state.viewport_pan_anim.is_some() {
            self.snap_camera_targets_to_live();
            return;
        }

        if !self.tuning.physics_enabled {
            self.viewport.center = self.camera_target_center;
            self.zoom_ref_size = self.camera_target_view_size;
            self.tuning.viewport_center = self.viewport.center;
            self.tuning.viewport_size = self.zoom_ref_size;
            return;
        }

        let dt = now
            .saturating_duration_since(self.render_state.render_last_tick)
            .as_secs_f32()
            .clamp(1.0 / 240.0, 1.0 / 20.0);
        let center_alpha = (dt * Self::CAMERA_SMOOTH_CENTER_RATE).clamp(0.08, 0.55);
        let zoom_alpha = (dt * Self::CAMERA_SMOOTH_ZOOM_RATE).clamp(0.08, 0.60);

        let mut changed = false;

        let next_center = Vec2 {
            x: self.viewport.center.x
                + (self.camera_target_center.x - self.viewport.center.x) * center_alpha,
            y: self.viewport.center.y
                + (self.camera_target_center.y - self.viewport.center.y) * center_alpha,
        };
        if (self.camera_target_center.x - next_center.x).abs() < 0.15 {
            self.viewport.center.x = self.camera_target_center.x;
        } else {
            self.viewport.center.x = next_center.x;
            changed = true;
        }
        if (self.camera_target_center.y - next_center.y).abs() < 0.15 {
            self.viewport.center.y = self.camera_target_center.y;
        } else {
            self.viewport.center.y = next_center.y;
            changed = true;
        }

        let next_size = Vec2 {
            x: self.zoom_ref_size.x
                + (self.camera_target_view_size.x - self.zoom_ref_size.x) * zoom_alpha,
            y: self.zoom_ref_size.y
                + (self.camera_target_view_size.y - self.zoom_ref_size.y) * zoom_alpha,
        };
        if (self.camera_target_view_size.x - next_size.x).abs() < 0.2 {
            self.zoom_ref_size.x = self.camera_target_view_size.x;
        } else {
            self.zoom_ref_size.x = next_size.x;
            changed = true;
        }
        if (self.camera_target_view_size.y - next_size.y).abs() < 0.2 {
            self.zoom_ref_size.y = self.camera_target_view_size.y;
        } else {
            self.zoom_ref_size.y = next_size.y;
            changed = true;
        }

        self.tuning.viewport_center = self.viewport.center;
        self.tuning.viewport_size = self.zoom_ref_size;
        if changed {
            self.request_maintenance();
        }
    }

    pub fn active_zoom_fallback_scale(&self, id: NodeId) -> Option<f32> {
        let _ = id;
        None
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
        let vp_w = self.viewport.size.x.max(1.0);
        let view_w = self.camera_view_size().x.max(1.0);
        (vp_w / view_w).max(0.01)
    }
}
