use super::*;

impl HalleyWlState {
    pub(super) fn update_zoom_live_surface_sizes(&mut self) {
        let view_scale_x = self.zoom_ref_size.x.max(1.0) / self.viewport.size.x.max(1.0);
        let view_scale_y = self.zoom_ref_size.y.max(1.0) / self.viewport.size.y.max(1.0);
        let view_scale = ((view_scale_x + view_scale_y) * 0.5).clamp(0.55, 1.0);
        // Keep zoom behavior camera-centric: don't chase live toplevel resizing while zoomed out.
        if view_scale < 0.995 {
            self.zoom_resize_fallback.clear();
            self.zoom_resize_reject_streak.clear();
            self.zoom_resize_static_streak.clear();
            self.zoom_last_observed_size.clear();
            return;
        }
        let near_ref = (view_scale - 1.0).abs() <= 0.03;
        if near_ref {
            self.zoom_resize_fallback.clear();
            self.zoom_resize_reject_streak.clear();
            self.zoom_resize_static_streak.clear();
        }
        let ids: Vec<NodeId> = self.field.nodes().keys().copied().collect();
        for id in ids {
            let Some(n) = self.field.node(id) else {
                continue;
            };
            if !self.field.is_visible(id)
                || n.kind != halley_core::field::NodeKind::Surface
                || n.state != halley_core::field::NodeState::Active
            {
                continue;
            }
            if near_ref {
                self.zoom_nominal_size.insert(id, n.intrinsic_size);
            }
            let base = self
                .zoom_nominal_size
                .get(&id)
                .copied()
                .unwrap_or(n.intrinsic_size);
            let prev_observed = self.zoom_last_observed_size.insert(id, n.intrinsic_size);
            let target_w = (base.x * view_scale)
                .round()
                .clamp(180.0, base.x.max(180.0));
            let target_h = (base.y * view_scale)
                .round()
                .clamp(120.0, base.y.max(120.0));
            let err_w = (n.intrinsic_size.x - target_w).abs();
            let err_h = (n.intrinsic_size.y - target_h).abs();
            if err_w < 6.0 && err_h < 6.0 {
                self.zoom_resize_reject_streak.remove(&id);
                self.zoom_resize_fallback.remove(&id);
                self.zoom_resize_static_streak.remove(&id);
                continue;
            }
            let moved_since_last = prev_observed.is_some_and(|prev| {
                (prev.x - n.intrinsic_size.x).abs() > 1.0
                    || (prev.y - n.intrinsic_size.y).abs() > 1.0
            });
            if moved_since_last {
                self.zoom_resize_reject_streak.remove(&id);
                self.zoom_resize_static_streak.remove(&id);
                self.zoom_resize_fallback.remove(&id);
            }
            let reject_threshold_w = (base.x * 0.08).max(10.0);
            let reject_threshold_h = (base.y * 0.08).max(10.0);
            if err_w > reject_threshold_w || err_h > reject_threshold_h {
                if !moved_since_last {
                    let static_streak = self.zoom_resize_static_streak.entry(id).or_insert(0);
                    *static_streak = static_streak.saturating_add(1);
                    let streak = self.zoom_resize_reject_streak.entry(id).or_insert(0);
                    *streak = streak.saturating_add(1);
                    if *static_streak >= 6 && *streak >= 3 {
                        self.zoom_resize_fallback.insert(id);
                    }
                }
            } else {
                self.zoom_resize_reject_streak.remove(&id);
                self.zoom_resize_static_streak.remove(&id);
            }
            self.request_toplevel_resize(id, target_w as i32, target_h as i32);
        }
    }

    pub fn active_zoom_fallback_scale(&self, id: NodeId) -> Option<f32> {
        let _ = id;
        None
    }

    pub fn active_zoom_lock_scale(&self) -> f32 {
        // Zoom was removed from normal mode. Keep active scale stable.
        1.0
    }

    pub(super) fn decay_tiny_nodes_on_zoom_out(&mut self) {
        if !self.tuning.dev_zoom_decay_enabled {
            return;
        }
        let zoom_x = self.viewport.size.x / self.zoom_ref_size.x.max(1.0);
        let zoom_y = self.viewport.size.y / self.zoom_ref_size.y.max(1.0);
        let zoom = zoom_x.max(zoom_y);
        // Keep live surfaces stable; convert directly to Node on zoom-out.
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
            let in_view = (n.pos.x - self.viewport.center.x).abs() <= self.viewport.size.x * 0.5
                && (n.pos.y - self.viewport.center.y).abs() <= self.viewport.size.y * 0.5;
            let frac_x = n.intrinsic_size.x / self.viewport.size.x.max(1.0);
            let frac_y = n.intrinsic_size.y / self.viewport.size.y.max(1.0);
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
