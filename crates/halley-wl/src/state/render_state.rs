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
        self.physics_velocity.retain(|id, _| alive.contains(id));
        self.smoothed_render_pos.retain(|id, _| alive.contains(id));
        self.smoothed_render_vel.retain(|id, _| alive.contains(id));
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
        let drag_active = self.carry_zone_hint.contains_key(&id);
        let immediate_physics = self.immediate_physics_nodes.contains(&id);
        let release_smoothing = self.release_smoothing_active_for(id, now_ms);
        if self.resize_active == Some(id)
            || (self.resize_static_node == Some(id) && now_ms < self.resize_static_until_ms)
        {
            self.smoothed_render_pos.insert(id, logical);
            self.smoothed_render_vel.insert(id, Vec2 { x: 0.0, y: 0.0 });
            return logical;
        }
        if drag_active || immediate_physics {
            self.smoothed_render_pos.insert(id, logical);
            self.smoothed_render_vel.insert(id, Vec2 { x: 0.0, y: 0.0 });
            return logical;
        }
        if !drag_active
            && !release_smoothing
            && (self.interaction_focus == Some(id)
                || self.companion_surface_node(now_ms) == Some(id)
                || self.is_recently_interacted_surface(id, now_ms))
        {
            self.smoothed_render_pos.insert(id, logical);
            self.smoothed_render_vel.insert(id, Vec2 { x: 0.0, y: 0.0 });
            return logical;
        }
        let dt = now
            .saturating_duration_since(self.render_last_tick)
            .as_secs_f32()
            .clamp(1.0 / 240.0, 1.0 / 20.0);
        let cur = self.smoothed_render_pos.entry(id).or_insert(logical);
        let vel = self
            .smoothed_render_vel
            .entry(id)
            .or_insert(Vec2 { x: 0.0, y: 0.0 });
        let boost = if self.carry_zone_hint.contains_key(&id) {
            self.tuning.drag_smoothing_boost.clamp(0.1, 20.0)
        } else {
            1.0
        };
        let omega = (9.0 * boost.sqrt()).clamp(6.0, 22.0);
        let damping = 2.0 * omega * 0.92;
        let max_speed = (2200.0 * boost).clamp(900.0, 9000.0);
        let ax = (logical.x - cur.x) * omega * omega - vel.x * damping;
        let ay = (logical.y - cur.y) * omega * omega - vel.y * damping;
        vel.x = (vel.x + ax * dt).clamp(-max_speed, max_speed);
        vel.y = (vel.y + ay * dt).clamp(-max_speed, max_speed);
        cur.x += vel.x * dt;
        cur.y += vel.y * dt;
        if (logical.x - cur.x).abs() < 0.6 && vel.x.abs() < 12.0 {
            cur.x = logical.x;
            vel.x = 0.0;
        }
        if (logical.y - cur.y).abs() < 0.6 && vel.y.abs() < 12.0 {
            cur.y = logical.y;
            vel.y = 0.0;
        }
        *cur
    }

    pub fn smoothed_render_pos_read(&self, id: NodeId, logical: Vec2, now: Instant) -> Vec2 {
        if !self.tuning.physics_enabled {
            return logical;
        }
        let now_ms = self.now_ms(now);
        let drag_active = self.carry_zone_hint.contains_key(&id);
        let immediate_physics = self.immediate_physics_nodes.contains(&id);
        let release_smoothing = self.release_smoothing_active_for(id, now_ms);
        if self.resize_active == Some(id)
            || (self.resize_static_node == Some(id) && now_ms < self.resize_static_until_ms)
            || drag_active
            || immediate_physics
            || (!drag_active
                && !release_smoothing
                && (self.interaction_focus == Some(id)
                    || self.companion_surface_node(now_ms) == Some(id)
                    || self.is_recently_interacted_surface(id, now_ms)))
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

    pub fn tick_live_overlap(&mut self) {
        self.tick_passive_physics();
        if self.tuning.physics_enabled || self.suspend_state_checks || self.resize_active.is_some()
        {
            return;
        }
        self.resolve_surface_overlap();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_smoothing_keeps_recently_released_focus_on_damped_path() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        let id =
            state
                .field
                .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let now = Instant::now();
        let now_ms = state.now_ms(now);
        state.interaction_focus = Some(id);
        state.interaction_focus_until_ms = now_ms.saturating_add(30_000);
        state
            .smoothed_render_pos
            .insert(id, Vec2 { x: 0.0, y: 0.0 });
        state
            .smoothed_render_vel
            .insert(id, Vec2 { x: 0.0, y: 0.0 });
        state
            .release_smoothing_until_ms
            .insert(id, now_ms.saturating_add(200));

        let read = state.smoothed_render_pos_read(id, Vec2 { x: 120.0, y: 0.0 }, now);
        assert!(
            (read.x - 0.0).abs() < 0.01,
            "expected cached smoothed position during release smoothing, got {:?}",
            read
        );
    }

    #[test]
    fn live_drag_keeps_passive_windows_on_immediate_physics_path() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        let dragged =
            state
                .field
                .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let other =
            state
                .field
                .spawn_surface("b", Vec2 { x: 500.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let now = Instant::now();
        state.begin_carry_state_tracking(dragged);
        state.immediate_physics_nodes.insert(other);
        state
            .smoothed_render_pos
            .insert(other, Vec2 { x: 470.0, y: 0.0 });
        state
            .smoothed_render_vel
            .insert(other, Vec2 { x: 120.0, y: 0.0 });

        let read = state.smoothed_render_pos_read(other, Vec2 { x: 500.0, y: 0.0 }, now);
        let write = state.smoothed_render_pos(other, Vec2 { x: 500.0, y: 0.0 }, now);

        assert_eq!(
            read.x, 500.0,
            "passive window read should stay on logical position"
        );
        assert_eq!(
            write.x, 500.0,
            "passive window write should stay on logical position"
        );
        assert_eq!(
            state.smoothed_render_vel.get(&other).copied(),
            Some(Vec2 { x: 0.0, y: 0.0 }),
            "immediate physics path should zero any stale smoothing velocity"
        );
    }
}
