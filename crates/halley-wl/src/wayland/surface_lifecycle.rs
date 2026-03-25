use std::time::Instant;

use halley_core::decay::DecayLevel;
use halley_core::field::{NodeId, Vec2};
use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use crate::activity::CommitActivity;
use crate::state::Halley;

impl Halley {
    #[inline]
    fn surface_key(surface: &WlSurface) -> ObjectId {
        surface.id()
    }

    fn surface_tree_root(surface: &WlSurface) -> WlSurface {
        let mut root = surface.clone();
        while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
            root = parent;
        }
        root
    }

    pub(super) fn viewport_fully_contains_surface(&self, id: NodeId) -> bool {
        let Some(node) = self.field.node(id) else {
            return false;
        };
        let ext = self.spawn_obstacle_extents_for_node(node);
        let min_x = self.viewport.center.x - self.viewport.size.x * 0.5;
        let max_x = self.viewport.center.x + self.viewport.size.x * 0.5;
        let min_y = self.viewport.center.y - self.viewport.size.y * 0.5;
        let max_y = self.viewport.center.y + self.viewport.size.y * 0.5;

        node.pos.x - ext.left >= min_x
            && node.pos.x + ext.right <= max_x
            && node.pos.y - ext.top >= min_y
            && node.pos.y + ext.bottom <= max_y
    }

    fn compact_app_id_label(app_id: &str) -> Option<String> {
        let tail = app_id
            .rsplit(['.', '/'])
            .next()
            .unwrap_or(app_id)
            .trim_matches(|ch: char| matches!(ch, '"' | '\'' | ' '));
        if tail.is_empty() {
            return None;
        }

        let mut out = String::with_capacity(tail.len());
        let mut upper_next = true;
        for ch in tail.chars() {
            if matches!(ch, '-' | '_' | '.') {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
                upper_next = true;
                continue;
            }
            if upper_next {
                out.extend(ch.to_uppercase());
                upper_next = false;
            } else {
                out.push(ch);
            }
        }

        Some(out.trim().to_string()).filter(|value| !value.is_empty())
    }

    fn surface_identity(surface: &WlSurface) -> (Option<String>, Option<String>) {
        with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|data| {
                    let guard = data.lock().expect("xdg toplevel surface data");
                    (
                        guard.title.clone().filter(|value| !value.trim().is_empty()),
                        guard
                            .app_id
                            .clone()
                            .filter(|value| !value.trim().is_empty()),
                    )
                })
                .unwrap_or((None, None))
        })
    }

    pub(crate) fn refresh_node_identity_for_surface(
        &mut self,
        surface: &WlSurface,
        fallback_label: &str,
    ) {
        let root_surface = Self::surface_tree_root(surface);
        let root_key = Self::surface_key(&root_surface);
        let Some(node_id) = self.surface_to_node.get(&root_key).copied() else {
            return;
        };

        let (title, app_id) = Self::surface_identity(&root_surface);
        let label = title
            .or_else(|| app_id.as_deref().and_then(Self::compact_app_id_label))
            .unwrap_or_else(|| fallback_label.to_string());

        if let Some(node) = self.field.node_mut(node_id) {
            node.label = label;
        }

        match app_id {
            Some(app_id) => {
                self.node_app_ids.insert(node_id, app_id);
            }
            None => {
                self.node_app_ids.remove(&node_id);
            }
        }
    }

    pub fn note_commit(&mut self, surface: &WlSurface, now: Instant) {
        let key = Self::surface_key(surface);
        let root_surface = Self::surface_tree_root(surface);
        let root_key = Self::surface_key(&root_surface);
        self.surface_activity
            .entry(key.clone())
            .or_insert_with(|| CommitActivity::new(now))
            .on_commit(now);
        for output in self.monitor_state.outputs.values() {
            output.enter(surface);
        }

        // Grant keyboard focus to layer surfaces (e.g. fuzzel) on their first
        // real commit, when keyboard_interactivity is now populated.
        self.maybe_grant_layer_surface_focus_on_commit(surface);

        // Keep window_geometry and bbox_loc current during resize so the render
        // path has a live source of truth. Outside resize this is handled by
        // sync_node_size_from_surface on every render frame, but that path is
        // bypassed for the resizing node.
        if let Some(node_id) = self.surface_to_node.get(&root_key).copied() {
            self.mark_window_offscreen_dirty(node_id);
            self.refresh_node_identity_for_surface(&root_surface, "Window");
            use smithay::desktop::utils::bbox_from_surface_tree;
            use smithay::wayland::shell::xdg::SurfaceCachedState;

            let bbox = bbox_from_surface_tree(&root_surface, (0, 0));
            self.render_state
                .bbox_loc
                .insert(node_id, (bbox.loc.x as f32, bbox.loc.y as f32));

            let geo = with_states(&root_surface, |states| {
                states
                    .cached_state
                    .get::<SurfaceCachedState>()
                    .current()
                    .geometry
            });
            if let Some(g) = geo {
                self.render_state.window_geometry.insert(
                    node_id,
                    (
                        g.loc.x as f32,
                        g.loc.y as f32,
                        g.size.w.max(1) as f32,
                        g.size.h.max(1) as f32,
                    ),
                );
            } else {
                self.render_state.window_geometry.insert(
                    node_id,
                    (
                        bbox.loc.x as f32,
                        bbox.loc.y as f32,
                        bbox.size.w.max(1) as f32,
                        bbox.size.h.max(1) as f32,
                    ),
                );
            }

            let new_size = Vec2 {
                x: bbox.size.w.max(1) as f32,
                y: bbox.size.h.max(1) as f32,
            };
            let size_changed = self.field.node(node_id).is_some_and(|node| {
                (node.intrinsic_size.x - new_size.x).abs() > 0.5
                    || (node.intrinsic_size.y - new_size.y).abs() > 0.5
            });

            if size_changed && self.interaction_state.resize_active != Some(node_id) {
                if let Some(node) = self.field.node_mut(node_id) {
                    node.intrinsic_size = new_size;
                    if node.state == halley_core::field::NodeState::Active {
                        node.footprint = new_size;
                    }
                }
                self.workspace_state
                    .last_active_size
                    .insert(node_id, new_size);
                self.request_maintenance();
                if self.interaction_state.resize_static_node != Some(node_id) {
                    self.resolve_overlap_now();
                }
            }
        }
    }

    pub fn ensure_node_for_surface(
        &mut self,
        surface: &WlSurface,
        label: &str,
        size_px: (i32, i32),
    ) -> NodeId {
        let key = Self::surface_key(surface);
        if let Some(id) = self.surface_to_node.get(&key).copied() {
            return id;
        }

        self.spawn_state.spawn_cursor += 1;
        let size = Vec2 {
            x: size_px.0.max(64) as f32,
            y: size_px.1.max(64) as f32,
        };
        let (pos, needs_pan) = self.pick_spawn_position(size);

        let id = self.field.spawn_surface(label.to_string(), pos, size);
        self.assign_node_to_current_monitor(id);
        let _ = self
            .field
            .set_state(id, halley_core::field::NodeState::Active);
        let _ = self.field.set_decay_level(id, DecayLevel::Hot);

        self.surface_to_node.insert(key, id);
        self.render_state.zoom_nominal_size.insert(id, size);
        self.workspace_state.last_active_size.insert(id, size);
        if self.tuning.dev_anim_enabled {
            self.render_state
                .animator
                .observe_field(&self.field, Instant::now());
        }
        if needs_pan {
            self.queue_spawn_pan_to_node(id, Instant::now());
        }
        self.refresh_node_identity_for_surface(surface, label);
        id
    }

    pub fn drop_surface(&mut self, surface: &WlSurface) {
        for output in self.monitor_state.outputs.values() {
            output.leave(surface);
        }
        let pointer_focused_surface = self
            .seat
            .get_pointer()
            .and_then(|pointer| pointer.current_focus());
        if pointer_focused_surface
            .as_ref()
            .is_some_and(|focused| focused.id() == surface.id())
        {
            self.clear_pointer_focus();
        }
        let key = Self::surface_key(surface);
        self.surface_activity.remove(&key);
        if let Some(id) = self.surface_to_node.remove(&key) {
            self.drop_fullscreen_surface(id, Instant::now());
            if self.focus_state.pan_restore_active_focus == Some(id) {
                self.focus_state.pan_restore_active_focus = None;
            }
            self.render_state.zoom_nominal_size.remove(&id);
            self.render_state.zoom_resize_fallback.remove(&id);
            self.render_state.zoom_resize_reject_streak.remove(&id);
            self.render_state.zoom_last_observed_size.remove(&id);
            self.render_state.zoom_resize_static_streak.remove(&id);
            self.node_app_ids.remove(&id);
            self.focus_state.focus_trail.forget_node(id);
            self.workspace_state.last_active_size.remove(&id);
            self.render_state.bbox_loc.remove(&id);
            self.render_state.window_geometry.remove(&id);
            self.spawn_state.pending_spawn_activate_at_ms.remove(&id);
            self.workspace_state.active_transition_until_ms.remove(&id);
            self.workspace_state
                .primary_promote_cooldown_until_ms
                .remove(&id);
            self.focus_state.last_surface_focus_ms.remove(&id);
            self.monitor_state.node_monitor.remove(&id);
            self.carry_state.carry_zone_hint.remove(&id);
            self.carry_state.carry_zone_last_change_ms.remove(&id);
            self.carry_state.carry_zone_pending.remove(&id);
            self.carry_state.carry_zone_pending_since_ms.remove(&id);
            self.carry_state.carry_activation_anim_armed.remove(&id);
            if self.interaction_state.resize_active == Some(id) {
                self.interaction_state.resize_active = None;
            }
            if self.interaction_state.resize_static_node == Some(id) {
                self.interaction_state.resize_static_node = None;
                self.interaction_state.resize_static_lock_pos = None;
                self.interaction_state.resize_static_until_ms = 0;
            }
            if self.focus_state.primary_interaction_focus == Some(id) {
                self.focus_state.primary_interaction_focus = None;
                self.focus_state.interaction_focus_until_ms = 0;
            }
            self.focus_state.suppress_trail_record_once = false;
            self.interaction_state.smoothed_render_pos.remove(&id);
            self.clear_window_offscreen_cache_for(id);
            let _ = self.field.remove(id);
        }
        self.request_maintenance();
    }
}
