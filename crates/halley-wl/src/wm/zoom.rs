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

    pub(crate) fn decay_tiny_nodes_on_zoom_out(&mut self) {
        if !self.tuning.dev_zoom_decay_enabled {
            return;
        }
        let camera = self.camera_view_size();
        let zoom_x = camera.x.max(1.0) / self.viewport.size.x.max(1.0);
        let zoom_y = camera.y.max(1.0) / self.viewport.size.y.max(1.0);
        let zoom = zoom_x.max(zoom_y);
        let zoom_decay_gate = 1.03_f32;
        let force_node_zoom = 1.82_f32;
        let t = self.tuning.dev_zoom_decay_min_frac.max(0.005);
        let ids: Vec<NodeId> = self.field.nodes().keys().copied().collect();
        for id in ids {
            if self.interaction_focus == Some(id) {
                continue;
            }
            let Some(n) = self.field.node(id) else {
                continue;
            };
            if n.kind == halley_core::field::NodeKind::Core {
                continue;
            }
            let in_view = (n.pos.x - self.viewport.center.x).abs() <= camera.x * 0.5
                && (n.pos.y - self.viewport.center.y).abs() <= camera.y * 0.5;
            let frac_x = n.intrinsic_size.x / camera.x.max(1.0);
            let frac_y = n.intrinsic_size.y / camera.y.max(1.0);
            let area_frac = frac_x * frac_y;
            let node_zoom = if in_view {
                force_node_zoom + 0.20
            } else {
                force_node_zoom
            };
            if zoom >= node_zoom
                || (zoom >= zoom_decay_gate
                    && (frac_x < t * 0.48 || frac_y < t * 0.48 || area_frac < (t * t * 0.30)))
            {
                let _ = self.field.set_decay_level(id, DecayLevel::Cold);
            }
        }
    }
}
