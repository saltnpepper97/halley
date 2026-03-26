use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_core::field::{NodeId, Vec2};

use smithay::backend::renderer::gles::{GlesTexProgram, GlesTexture};
use smithay::wayland::compositor::{
    SurfaceAttributes, TraversalAction, with_surface_tree_downward,
};

use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct WindowOffscreenKey {
    pub width: i32,
    pub height: i32,
}

#[derive(Default)]
pub(crate) struct WindowOffscreenCache {
    /// Native 1.0x surface-tree bbox size used to build the offscreen image.
    pub key: WindowOffscreenKey,

    /// Set when the cached offscreen image should be rebuilt before use.
    pub dirty: bool,

    /// Last frame this cache entry was touched.
    pub last_used_at: Option<Instant>,

    /// Cached 1.0x surface-tree render target for zoomed compositing.
    pub texture: Option<GlesTexture>,

    /// Logical bbox paired with the cached texture.
    pub bbox: Option<Rectangle<i32, Logical>>,
}

impl WindowOffscreenCache {
    #[inline]
    pub(crate) fn matches_size(&self, width: i32, height: i32) -> bool {
        self.key.width == width && self.key.height == height
    }

    #[inline]
    pub(crate) fn set_size(&mut self, width: i32, height: i32) {
        self.key = WindowOffscreenKey { width, height };
    }

    #[inline]
    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    #[inline]
    pub(crate) fn mark_clean(&mut self, now: Instant) {
        self.dirty = false;
        self.last_used_at = Some(now);
    }

    #[inline]
    pub(crate) fn touch(&mut self, now: Instant) {
        self.last_used_at = Some(now);
    }
}

#[derive(Clone)]
pub(crate) struct NodeAppIconTexture {
    pub texture: GlesTexture,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone)]
pub(crate) enum NodeAppIconCacheEntry {
    Ready(NodeAppIconTexture),
    Missing,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PreviewHoverState {
    pub(crate) node: Option<NodeId>,
    pub(crate) mix: f32,
}

pub(crate) struct RenderState {
    pub animator: Animator,

    pub(crate) node_app_icon_cache: HashMap<String, NodeAppIconCacheEntry>,
    pub(crate) node_hover_mix: HashMap<NodeId, f32>,
    pub(crate) node_preview_hover: HashMap<String, PreviewHoverState>,
    pub(crate) node_circle_texture: Option<GlesTexture>,
    pub(crate) node_squircle_program: Option<GlesTexProgram>,
    pub(crate) node_label_program: Option<GlesTexProgram>,

    pub(crate) zoom_nominal_size: HashMap<NodeId, Vec2>,
    pub(crate) zoom_resize_fallback: HashSet<NodeId>,
    pub(crate) zoom_resize_reject_streak: HashMap<NodeId, u8>,
    pub(crate) zoom_last_observed_size: HashMap<NodeId, Vec2>,
    pub(crate) zoom_resize_static_streak: HashMap<NodeId, u8>,

    pub(crate) render_last_tick: Instant,

    pub(crate) bbox_loc: HashMap<NodeId, (f32, f32)>,
    pub(crate) window_geometry: HashMap<NodeId, (f32, f32, f32, f32)>,
    pub(crate) window_offscreen_cache: HashMap<NodeId, WindowOffscreenCache>,
}

impl Halley {
    pub(crate) fn take_input_state_reset_request(&mut self) -> bool {
        std::mem::take(&mut self.interaction_state.reset_input_state_requested)
    }

    pub(crate) fn take_pointer_screen_hint_request(&mut self) -> Option<(f32, f32)> {
        self.interaction_state.pending_pointer_screen_hint.take()
    }

    pub fn begin_render_frame(&mut self, now: Instant) {
        self.render_state.render_last_tick = now;
        self.popup_manager.cleanup();
        let alive: HashSet<NodeId> = self.field.nodes().keys().copied().collect();
        self.interaction_state
            .physics_velocity
            .retain(|id, _| alive.contains(id));
        self.interaction_state
            .smoothed_render_pos
            .retain(|id, _| alive.contains(id));
        self.render_state
            .node_hover_mix
            .retain(|id, _| alive.contains(id));
        self.render_state.node_preview_hover.retain(|_, state| {
            state.node = state.node.filter(|id| alive.contains(id));
            state.node.is_some() || state.mix > 0.002
        });
        self.prune_window_offscreen_cache(now);
    }

    pub(crate) fn resize_static_active_for(
        &self,
        node_id: halley_core::field::NodeId,
        now_ms: u64,
    ) -> bool {
        self.interaction_state.resize_static_node == Some(node_id)
            && now_ms < self.interaction_state.resize_static_until_ms
    }

    pub fn smoothed_render_pos(&mut self, id: NodeId, logical: Vec2, now: Instant) -> Vec2 {
        if !self.tuning.physics_enabled {
            return logical;
        }
        let now_ms = self.now_ms(now);
        if self.interaction_state.resize_active == Some(id)
            || (self.interaction_state.resize_static_node == Some(id)
                && now_ms < self.interaction_state.resize_static_until_ms)
        {
            self.interaction_state
                .smoothed_render_pos
                .insert(id, logical);
            return logical;
        }
        if self.focus_state.primary_interaction_focus == Some(id)
            || self.companion_surface_node(now_ms) == Some(id)
            || self.is_recently_interacted_surface(id, now_ms)
        {
            self.interaction_state
                .smoothed_render_pos
                .insert(id, logical);
            return logical;
        }
        let dt = now
            .saturating_duration_since(self.render_state.render_last_tick)
            .as_secs_f32()
            .clamp(1.0 / 240.0, 1.0 / 20.0);
        let mut alpha = (dt * 18.0).clamp(0.10, 0.42);
        let mut max_step = (dt * 1800.0).clamp(6.0, 70.0);
        if self.carry_state.carry_zone_hint.contains_key(&id) {
            let boost = self.tuning.drag_smoothing_boost.clamp(0.1, 20.0);
            alpha = (alpha * boost).clamp(0.10, 1.0);
            max_step = (max_step * boost).clamp(6.0, 420.0);
        }

        let cur = self
            .interaction_state
            .smoothed_render_pos
            .entry(id)
            .or_insert(logical);
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
        if self.interaction_state.resize_active == Some(id)
            || (self.interaction_state.resize_static_node == Some(id)
                && now_ms < self.interaction_state.resize_static_until_ms)
            || self.focus_state.primary_interaction_focus == Some(id)
            || self.companion_surface_node(now_ms) == Some(id)
            || self.is_recently_interacted_surface(id, now_ms)
        {
            return logical;
        }
        self.interaction_state
            .smoothed_render_pos
            .get(&id)
            .copied()
            .unwrap_or(logical)
    }

    pub fn node_label_hover_mix(&mut self, id: NodeId, hovered: bool) -> f32 {
        let target = if hovered { 1.0 } else { 0.0 };
        let mix = self.render_state.node_hover_mix.entry(id).or_insert(target);
        let k = if hovered { 0.06 } else { 0.10 };
        *mix += (target - *mix) * k;
        if (*mix - target).abs() < 0.01 {
            *mix = target;
        }
        *mix
    }

    pub fn node_preview_hover_anim_for_monitor(
        &mut self,
        monitor: &str,
        hovered: Option<NodeId>,
    ) -> Option<(NodeId, f32)> {
        let state = self
            .render_state
            .node_preview_hover
            .entry(monitor.to_string())
            .or_default();
        if hovered.is_some() && hovered != state.node {
            state.node = hovered;
            state.mix = 0.0;
        }
        let target = if hovered.is_some() { 1.0 } else { 0.0 };
        let k = if target > 0.5 { 0.30 } else { 0.14 };
        state.mix += (target - state.mix) * k;
        if (state.mix - target).abs() < 0.002 {
            state.mix = target;
        }
        if target <= 0.0 && state.mix <= 0.002 {
            state.mix = 0.0;
            state.node = None;
        }
        state.node.map(|id| (id, state.mix))
    }

    pub fn set_app_focused(&mut self, focused: bool) {
        self.focus_state.app_focused = focused;
    }

    pub fn tick_animator_frame(&mut self, now: Instant) {
        if !self.tuning.physics_enabled {
            return;
        }
        self.render_state.animator.observe_field(&self.field, now);
    }

    pub fn tick_frame_effects(&mut self, now: Instant) {
        let now_ms = self.now_ms(now);
        self.tick_viewport_pan_animation(now_ms);
        self.tick_pending_spawn_pan(now, now_ms);
        self.tick_active_drag(now);
        self.tick_camera_smoothing(now);
    }

    pub(crate) fn tick_active_drag(&mut self, now: Instant) {
        let Some(mut active_drag) = self.interaction_state.active_drag.clone() else {
            self.interaction_state.grabbed_edge_pan_active = false;
            self.interaction_state.grabbed_edge_pan_direction = halley_core::field::Vec2 {
                x: 0.0,
                y: 0.0,
            };
            self.interaction_state.grabbed_edge_pan_monitor = None;
            return;
        };

        let Some(node_id) = self.interaction_state.drag_authority_node else {
            self.interaction_state.active_drag = None;
            return;
        };
        if node_id != active_drag.node_id {
            self.interaction_state.active_drag = None;
            self.interaction_state.grabbed_edge_pan_active = false;
            self.interaction_state.grabbed_edge_pan_direction = halley_core::field::Vec2 {
                x: 0.0,
                y: 0.0,
            };
            self.interaction_state.grabbed_edge_pan_monitor = None;
            return;
        }

        let pointer_world = crate::spatial::screen_to_world(
            self,
            active_drag.pointer_workspace_size.0,
            active_drag.pointer_workspace_size.1,
            active_drag.pointer_screen_local.0,
            active_drag.pointer_screen_local.1,
        );
        let desired_to = halley_core::field::Vec2 {
            x: pointer_world.x - active_drag.current_offset.x,
            y: pointer_world.y - active_drag.current_offset.y,
        };

        let moved = if active_drag.allow_monitor_transfer {
            self.interaction_state.grabbed_edge_pan_active = false;
            self.interaction_state.grabbed_edge_pan_direction = halley_core::field::Vec2 {
                x: 0.0,
                y: 0.0,
            };
            self.interaction_state.grabbed_edge_pan_monitor = None;
            self.assign_node_to_monitor(node_id, active_drag.pointer_monitor.as_str());
            self.carry_surface_non_overlap(node_id, desired_to, false)
        } else if !active_drag.edge_pan_eligible {
            self.interaction_state.grabbed_edge_pan_active = false;
            self.interaction_state.grabbed_edge_pan_direction = halley_core::field::Vec2 {
                x: 0.0,
                y: 0.0,
            };
            self.interaction_state.grabbed_edge_pan_monitor = None;
            self.carry_surface_non_overlap(node_id, desired_to, false)
        } else if let Some((clamped_center, edge_contact)) = self.dragged_node_edge_pan_clamp(
            active_drag.pointer_monitor.as_str(),
            node_id,
            desired_to,
            halley_core::field::Vec2 {
                x: active_drag.edge_pan_x.sign(),
                y: active_drag.edge_pan_y.sign(),
            },
        ) {
            if active_drag.edge_pan_x.sign() != 0.0 && edge_contact.x != active_drag.edge_pan_x.sign()
            {
                active_drag.edge_pan_x = crate::interaction::types::DragAxisMode::Free;
            }
            if active_drag.edge_pan_y.sign() != 0.0 && edge_contact.y != active_drag.edge_pan_y.sign()
            {
                active_drag.edge_pan_y = crate::interaction::types::DragAxisMode::Free;
            }

            let direction = halley_core::field::Vec2 {
                x: active_drag.edge_pan_x.sign(),
                y: active_drag.edge_pan_y.sign(),
            };
            let edge_pan_active = direction.x != 0.0 || direction.y != 0.0;
            self.interaction_state.grabbed_edge_pan_active = edge_pan_active;
            self.interaction_state.grabbed_edge_pan_direction = direction;
            self.interaction_state.grabbed_edge_pan_monitor = edge_pan_active
                .then(|| active_drag.pointer_monitor.clone());

            let mut to = clamped_center;
            if edge_pan_active {
                let dt = now
                    .saturating_duration_since(self.render_state.render_last_tick)
                    .as_secs_f32()
                    .clamp(1.0 / 240.0, 1.0 / 30.0);
                const DRAG_EDGE_PAN_SPEED: f32 = 720.0;
                let pan_delta = halley_core::field::Vec2 {
                    x: direction.x * DRAG_EDGE_PAN_SPEED * dt,
                    y: direction.y * DRAG_EDGE_PAN_SPEED * dt,
                };
                self.note_pan_activity(now);
                self.pan_camera_target(pan_delta);
                self.viewport.center = self.camera_target_center;
                self.tuning.viewport_center = self.viewport.center;
                self.sync_current_monitor_state();
                self.note_pan_viewport_change(now);

                if let Some(current_pos) = self.field.node(node_id).map(|n| n.pos) {
                    if direction.x != 0.0 {
                        to.x = current_pos.x + pan_delta.x;
                    }
                    if direction.y != 0.0 {
                        to.y = current_pos.y + pan_delta.y;
                    }
                }
            }

            self.interaction_state.active_drag = Some(active_drag);
            self.carry_surface_non_overlap(node_id, to, false)
        } else {
            self.interaction_state.active_drag = None;
            self.interaction_state.grabbed_edge_pan_active = false;
            self.interaction_state.grabbed_edge_pan_direction = halley_core::field::Vec2 {
                x: 0.0,
                y: 0.0,
            };
            self.interaction_state.grabbed_edge_pan_monitor = None;
            return;
        };
        if moved {
            self.request_maintenance();
        }
    }

    pub fn tick_live_overlap(&mut self) {
        if self.interaction_state.suspend_state_checks
            || self.interaction_state.resize_active.is_some()
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

    pub(crate) fn ensure_window_offscreen_cache(
        &mut self,
        node_id: NodeId,
        width: i32,
        height: i32,
        now: Instant,
    ) -> &mut WindowOffscreenCache {
        let cache = self
            .render_state
            .window_offscreen_cache
            .entry(node_id)
            .or_default();

        let width = width.max(1);
        let height = height.max(1);

        if !cache.matches_size(width, height) {
            cache.set_size(width, height);
            cache.mark_dirty();
        }

        cache.touch(now);
        cache
    }

    pub(crate) fn mark_window_offscreen_dirty(&mut self, node_id: NodeId) {
        if let Some(cache) = self.render_state.window_offscreen_cache.get_mut(&node_id) {
            cache.mark_dirty();
        }
    }

    pub(crate) fn clear_window_offscreen_cache_for(&mut self, node_id: NodeId) {
        self.render_state.window_offscreen_cache.remove(&node_id);
    }

    pub(crate) fn prune_window_offscreen_cache(&mut self, now: Instant) {
        let alive: HashSet<NodeId> = self.field.nodes().keys().copied().collect();
        self.render_state
            .window_offscreen_cache
            .retain(|id, cache| {
                alive.contains(id)
                    && cache
                        .last_used_at
                        .is_none_or(|t| now.saturating_duration_since(t).as_secs() < 5)
            });
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
