use std::collections::HashSet;

use smithay::wayland::compositor::{
    SurfaceAttributes, TraversalAction, with_surface_tree_downward,
};

use super::*;

impl HalleyWlState {
    pub fn begin_render_frame(&mut self, now: Instant) {
        self.render_last_tick = now;
        self.popup_manager.cleanup();
        let alive: HashSet<NodeId> = self.field.nodes().keys().copied().collect();
        self.smoothed_render_pos.retain(|id, _| alive.contains(id));
        self.node_hover_mix.retain(|id, _| alive.contains(id));
    }

    pub(crate) fn resize_static_active_for(
        &self,
        node_id: halley_core::field::NodeId,
        now_ms: u64,
    ) -> bool {
        self.resize_static_node == Some(node_id) && now_ms < self.resize_static_until_ms
    }

    pub fn smoothed_render_pos(&mut self, id: NodeId, logical: Vec2, now: Instant) -> Vec2 {
        if !self.tuning.physics_enabled {
            return logical;
        }
        let now_ms = self.now_ms(now);
        if self.resize_active == Some(id)
            || (self.resize_static_node == Some(id) && now_ms < self.resize_static_until_ms)
        {
            self.smoothed_render_pos.insert(id, logical);
            return logical;
        }
        if self.interaction_focus == Some(id)
            || self.companion_surface_node(now_ms) == Some(id)
            || self.is_recently_interacted_surface(id, now_ms)
        {
            self.smoothed_render_pos.insert(id, logical);
            return logical;
        }
        let dt = now
            .saturating_duration_since(self.render_last_tick)
            .as_secs_f32()
            .clamp(1.0 / 240.0, 1.0 / 20.0);
        let mut alpha = (dt * 18.0).clamp(0.10, 0.42);
        let mut max_step = (dt * 1800.0).clamp(6.0, 70.0);
        if self.carry_zone_hint.contains_key(&id) {
            let boost = self.tuning.drag_smoothing_boost.clamp(0.1, 20.0);
            alpha = (alpha * boost).clamp(0.10, 1.0);
            max_step = (max_step * boost).clamp(6.0, 420.0);
        }

        let cur = self.smoothed_render_pos.entry(id).or_insert(logical);
        let dx = logical.x - cur.x;
        let dy = logical.y - cur.y;
        let mut sx = dx * alpha;
        let mut sy = dy * alpha;
        sx = sx.clamp(-max_step, max_step);
        sy = sy.clamp(-max_step, max_step);
        cur.x += sx;
        cur.y += sy;
        if (logical.x - cur.x).abs() < 0.6 {
            cur.x = logical.x;
        }
        if (logical.y - cur.y).abs() < 0.6 {
            cur.y = logical.y;
        }
        *cur
    }

    pub fn smoothed_render_pos_read(&self, id: NodeId, logical: Vec2, now: Instant) -> Vec2 {
        if !self.tuning.physics_enabled {
            return logical;
        }
        let now_ms = self.now_ms(now);
        if self.resize_active == Some(id)
            || (self.resize_static_node == Some(id) && now_ms < self.resize_static_until_ms)
            || self.interaction_focus == Some(id)
            || self.companion_surface_node(now_ms) == Some(id)
            || self.is_recently_interacted_surface(id, now_ms)
        {
            return logical;
        }
        self.smoothed_render_pos
            .get(&id)
            .copied()
            .unwrap_or(logical)
    }

    pub fn node_label_hover_mix(&mut self, id: NodeId, hovered: bool) -> f32 {
        let target = if hovered { 1.0 } else { 0.0 };
        let mix = self.node_hover_mix.entry(id).or_insert(target);
        let k = if hovered { 0.30 } else { 0.24 };
        *mix += (target - *mix) * k;
        if (*mix - target).abs() < 0.01 {
            *mix = target;
        }
        *mix
    }

    pub fn node_preview_hover_anim(&mut self, hovered: Option<NodeId>) -> Option<(NodeId, f32)> {
        if hovered.is_some() && hovered != self.node_preview_hover_node {
            self.node_preview_hover_node = hovered;
            self.node_preview_hover_mix = 0.0;
        }
        let target = if hovered.is_some() { 1.0 } else { 0.0 };
        let k = if target > 0.5 { 0.30 } else { 0.14 };
        self.node_preview_hover_mix += (target - self.node_preview_hover_mix) * k;
        if (self.node_preview_hover_mix - target).abs() < 0.002 {
            self.node_preview_hover_mix = target;
        }
        if target <= 0.0 && self.node_preview_hover_mix <= 0.002 {
            self.node_preview_hover_mix = 0.0;
            self.node_preview_hover_node = None;
        }
        self.node_preview_hover_node
            .map(|id| (id, self.node_preview_hover_mix))
    }

    pub fn set_app_focused(&mut self, focused: bool) {
        self.app_focused = focused;
    }

    pub fn tick_animator_frame(&mut self, now: Instant) {
        if !self.tuning.physics_enabled {
            return;
        }
        self.animator.observe_field(&self.field, now);
    }

    pub fn tick_frame_effects(&mut self, now: Instant) {
        let now_ms = self.now_ms(now);
        self.tick_viewport_pan_animation(now_ms);
        self.tick_pending_spawn_pan(now, now_ms);
    }

    pub fn send_frame_callbacks(&mut self, now: Instant) {
        let elapsed_ms = now.duration_since(self.started_at).as_millis();
        let time_ms = elapsed_ms.min(u32::MAX as u128) as u32;
        for layer in self.wlr_layer_shell_state.layer_surfaces() {
            send_frames_surface_tree(layer.wl_surface(), time_ms);
        }
        for top in self.xdg_shell_state.toplevel_surfaces() {
            send_frames_surface_tree(top.wl_surface(), time_ms);
        }
        for popup in self.xdg_shell_state.popup_surfaces() {
            send_frames_surface_tree(popup.wl_surface(), time_ms);
        }
    }
}

fn send_frames_surface_tree(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    time_ms: u32,
) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time_ms);
            }
        },
        |_, _, &()| true,
    );
}
