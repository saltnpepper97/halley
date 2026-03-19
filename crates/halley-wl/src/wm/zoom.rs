use super::*;

impl HalleyWlState {
    #[inline]
    pub(crate) fn camera_view_size(&self) -> Vec2 {
        self.zoom_ref_size
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
        self.zoom_resize_fallback.clear();
        self.zoom_resize_reject_streak.clear();
        self.zoom_resize_static_streak.clear();
        self.zoom_last_observed_size.clear();
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
