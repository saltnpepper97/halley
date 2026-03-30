use std::time::Instant;

use halley_core::decay::DecayLevel;
use halley_core::field::{NodeId, Vec2};
use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use crate::activity::CommitActivity;
use crate::compositor::ctx::SurfaceLifecycleCtx;
use crate::compositor::root::Halley;

pub(crate) fn refresh_surface_identity(
    ctx: &mut SurfaceLifecycleCtx<'_>,
    surface: &WlSurface,
    fallback_label: &str,
) {
    ctx.st.refresh_node_identity_for_surface(surface, fallback_label);
}

pub(crate) fn on_surface_commit(
    ctx: &mut SurfaceLifecycleCtx<'_>,
    surface: &WlSurface,
    now: Instant,
) {
    ctx.st.note_commit(surface, now);
}

pub(crate) fn ensure_node_for_surface(
    ctx: &mut SurfaceLifecycleCtx<'_>,
    surface: &WlSurface,
    label: &str,
    size_px: (i32, i32),
) -> NodeId {
    ctx.st.ensure_node_for_surface(surface, label, size_px)
}

#[allow(dead_code)]
pub(crate) fn drop_surface(ctx: &mut SurfaceLifecycleCtx<'_>, surface: &WlSurface) {
    ctx.st.drop_surface(surface);
}

pub(crate) fn on_toplevel_destroyed(
    ctx: &mut SurfaceLifecycleCtx<'_>,
    surface: ToplevelSurface,
) {
    let st = &mut ctx.st;
    let key = surface.wl_surface().id();
    let closing_id = st.model.surface_to_node.get(&key).copied();
    let had_keyboard_focus = st
        .platform
        .seat
        .get_keyboard()
        .and_then(|kb| kb.current_focus())
        .is_some_and(|focused| focused.id() == key);
    let had_pointer_focus = st
        .platform
        .seat
        .get_pointer()
        .and_then(|ptr| ptr.current_focus())
        .is_some_and(|focused| focused.id() == key);
    let focused_monitor = st
        .model
        .surface_to_node
        .get(&key)
        .and_then(|id| st.model.monitor_state.node_monitor.get(id))
        .cloned();

    if had_keyboard_focus || had_pointer_focus {
        eventline::info!(
            "toplevel_destroyed with active focus (keyboard={} pointer={}); scheduling input state reset",
            had_keyboard_focus,
            had_pointer_focus
        );
        st.input.interaction_state.reset_input_state_requested = true;
        if let Some(ref focused_monitor) = focused_monitor {
            st.model.spawn_state.pending_spawn_monitor = Some(focused_monitor.clone());
            eventline::info!(
                "pending spawn monitor latched from destroyed toplevel: {}",
                focused_monitor
            );
        }
    }

    if had_keyboard_focus {
        st.clear_keyboard_focus();
    }

    if had_keyboard_focus
        && st.runtime.tuning.close_restore_focus
        && let (Some(closing_id), Some(focused_monitor)) = (closing_id, focused_monitor.as_deref())
    {
        let now = Instant::now();
        if st.active_cluster_workspace_for_monitor(focused_monitor).is_some() {
            if let Some(previous) = st.previous_window_from_trail_on_close(focused_monitor, closing_id) {
                st.set_interaction_focus(Some(previous), 30_000, now);
            } else if let Some(fallback) = st
                .last_focused_surface_node_for_monitor(focused_monitor)
                .filter(|&id| id != closing_id)
            {
                st.set_interaction_focus(Some(fallback), 30_000, now);
            }
        } else if let Some(previous) = st.previous_window_from_trail_on_close(focused_monitor, closing_id) {
            let _ = st.restore_focus_to_node_after_close(focused_monitor, previous, now);
        } else if let Some(fallback) = st
            .last_focused_surface_node_for_monitor(focused_monitor)
            .filter(|&id| id != closing_id)
            .or_else(|| st.last_focused_surface_node().filter(|&id| id != closing_id))
        {
            let _ = st.restore_focus_to_node_after_close(focused_monitor, fallback, now);
        }
    } else if had_keyboard_focus
        && !st.runtime.tuning.close_restore_focus
        && let Some(focused_monitor) = focused_monitor.as_deref()
    {
        st.model
            .focus_state
            .blocked_monitor_focus_restore
            .insert(focused_monitor.to_string());
    }
    if had_pointer_focus {
        st.clear_pointer_focus();
    }
}

impl Halley {
    fn predicted_spawn_target_monitor(&self) -> String {
        self.model
            .spawn_state
            .pending_spawn_monitor
            .as_ref()
            .filter(|monitor| {
                self.model
                    .monitor_state
                    .monitors
                    .contains_key(monitor.as_str())
            })
            .cloned()
            .unwrap_or_else(|| {
                let focused = self.focused_monitor().to_string();
                if self
                    .model
                    .monitor_state
                    .monitors
                    .contains_key(focused.as_str())
                {
                    focused
                } else {
                    self.interaction_monitor().to_string()
                }
            })
    }

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
        let Some(node_id) = self.model.surface_to_node.get(&root_key).copied() else {
            return;
        };

        let (title, app_id) = Self::surface_identity(&root_surface);
        let label = title
            .or_else(|| app_id.as_deref().and_then(Self::compact_app_id_label))
            .unwrap_or_else(|| fallback_label.to_string());

        if let Some(node) = self.model.field.node_mut(node_id) {
            node.label = label;
        }

        match app_id {
            Some(app_id) => {
                self.model.node_app_ids.insert(node_id, app_id);
            }
            None => {
                self.model.node_app_ids.remove(&node_id);
            }
        }
    }

    pub fn note_commit(&mut self, surface: &WlSurface, now: Instant) {
        let key = Self::surface_key(surface);
        let root_surface = Self::surface_tree_root(surface);
        let root_key = Self::surface_key(&root_surface);
        self.runtime
            .surface_activity
            .entry(key.clone())
            .or_insert_with(|| CommitActivity::new(now))
            .on_commit(now);
        for output in self.model.monitor_state.outputs.values() {
            output.enter(surface);
        }

        // Grant keyboard focus to layer surfaces (e.g. fuzzel) on their first
        // real commit, when keyboard_interactivity is now populated.
        self.maybe_grant_layer_surface_focus_on_commit(surface);

        // Keep window_geometry and bbox_loc current during resize so the render
        // path has a live source of truth. Outside resize this is handled by
        // sync_node_size_from_surface on every render frame, but that path is
        // bypassed for the resizing node.
        if let Some(node_id) = self.model.surface_to_node.get(&root_key).copied() {
            self.ui.render_state.mark_window_offscreen_dirty(node_id);
            self.refresh_node_identity_for_surface(&root_surface, "Window");
            use smithay::desktop::utils::bbox_from_surface_tree;
            use smithay::wayland::shell::xdg::SurfaceCachedState;

            let bbox = bbox_from_surface_tree(&root_surface, (0, 0));
            self.ui
                .render_state
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
                self.ui.render_state.window_geometry.insert(
                    node_id,
                    (
                        g.loc.x as f32,
                        g.loc.y as f32,
                        g.size.w.max(1) as f32,
                        g.size.h.max(1) as f32,
                    ),
                );
            } else {
                self.ui.render_state.window_geometry.insert(
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
            let size_changed = self.model.field.node(node_id).is_some_and(|node| {
                (node.intrinsic_size.x - new_size.x).abs() > 0.5
                    || (node.intrinsic_size.y - new_size.y).abs() > 0.5
            });

            if size_changed && self.input.interaction_state.resize_active != Some(node_id) {
                if let Some(node) = self.model.field.node_mut(node_id) {
                    node.intrinsic_size = new_size;
                    if node.state == halley_core::field::NodeState::Active {
                        node.footprint = new_size;
                    }
                }
                self.model
                    .workspace_state
                    .last_active_size
                    .insert(node_id, new_size);
                self.request_maintenance();
                if self.input.interaction_state.resize_static_node != Some(node_id) {
                    let node_monitor = self.model.monitor_state.node_monitor.get(&node_id).cloned();
                    let active_cluster = self
                        .model
                        .field
                        .cluster_id_for_member_public(node_id)
                        .zip(node_monitor.as_deref())
                        .is_some_and(|(cid, monitor)| {
                            self.active_cluster_workspace_for_monitor(monitor) == Some(cid)
                        });
                    if active_cluster {
                        if let Some(monitor) = node_monitor {
                            self.layout_active_cluster_workspace_for_monitor(
                                monitor.as_str(),
                                self.now_ms(now),
                            );
                        }
                    } else {
                        self.resolve_overlap_now();
                    }
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
        if let Some(id) = self.model.surface_to_node.get(&key).copied() {
            return id;
        }

        let size = Vec2 {
            x: size_px.0.max(64) as f32,
            y: size_px.1.max(64) as f32,
        };
        let predicted_monitor = self.predicted_spawn_target_monitor();
        let active_cluster = self.active_cluster_workspace_for_monitor(predicted_monitor.as_str());
        let previous_overflow_len = active_cluster
            .and_then(|cid| {
                self.model
                    .field
                    .cluster(cid)
                    .map(|cluster| cluster.overflow_members().len())
            })
            .unwrap_or(0);
        let (monitor, id, needs_pan, spawned_in_active_cluster) = if let Some(cid) = active_cluster
        {
            match self
                .model
                .field
                .spawn_surface_in_active_cluster(cid, label.to_string(), size)
            {
                Ok(id) => (predicted_monitor, id, false, true),
                Err(_) => {
                    let (monitor, pos, needs_pan) = self.pick_spawn_position(size);
                    let id = self.model.field.spawn_surface(label.to_string(), pos, size);
                    (monitor, id, needs_pan, false)
                }
            }
        } else {
            let (monitor, pos, needs_pan) = self.pick_spawn_position(size);
            let id = self.model.field.spawn_surface(label.to_string(), pos, size);
            (monitor, id, needs_pan, false)
        };
        self.assign_node_to_monitor(id, monitor.as_str());
        let _ = self
            .model
            .field
            .set_state(id, halley_core::field::NodeState::Active);
        if !spawned_in_active_cluster {
            let _ = self.model.field.set_decay_level(id, DecayLevel::Hot);
        }

        self.model.surface_to_node.insert(key, id);
        self.ui.render_state.zoom_nominal_size.insert(id, size);
        self.model.workspace_state.last_active_size.insert(id, size);
        let now = Instant::now();
        let joined_active_cluster = spawned_in_active_cluster;
        if self.runtime.tuning.dev_anim_enabled {
            self.ui
                .render_state
                .animator
                .observe_field(&self.model.field, now);
        }
        if needs_pan && !joined_active_cluster {
            self.queue_spawn_pan_to_node(id, now);
        }
        if let Some(cid) = active_cluster.filter(|_| joined_active_cluster) {
            let overflow_len = self
                .model
                .field
                .cluster(cid)
                .map(|cluster| cluster.overflow_members().len())
                .unwrap_or(0);
            if overflow_len > previous_overflow_len {
                self.reveal_cluster_overflow_for_monitor(monitor.as_str(), self.now_ms(now));
            }
        }
        self.refresh_node_identity_for_surface(surface, label);
        id
    }

    pub fn drop_surface(&mut self, surface: &WlSurface) {
        for output in self.model.monitor_state.outputs.values() {
            output.leave(surface);
        }
        let pointer_focused_surface = self
            .platform
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
        self.runtime.surface_activity.remove(&key);
        if let Some(id) = self.model.surface_to_node.remove(&key) {
            self.drop_fullscreen_surface(id, Instant::now());
            if self.model.focus_state.pan_restore_active_focus == Some(id) {
                self.model.focus_state.pan_restore_active_focus = None;
            }
            self.ui.render_state.zoom_nominal_size.remove(&id);
            self.ui.render_state.zoom_resize_fallback.remove(&id);
            self.ui.render_state.zoom_resize_reject_streak.remove(&id);
            self.ui.render_state.zoom_last_observed_size.remove(&id);
            self.ui.render_state.zoom_resize_static_streak.remove(&id);
            self.model.node_app_ids.remove(&id);
            for trail in self.model.focus_state.focus_trail.values_mut() {
                trail.forget_node(id);
            }
            self.model.workspace_state.last_active_size.remove(&id);
            self.ui.render_state.bbox_loc.remove(&id);
            self.ui.render_state.window_geometry.remove(&id);
            self.model
                .spawn_state
                .pending_spawn_activate_at_ms
                .remove(&id);
            self.model
                .workspace_state
                .active_transition_until_ms
                .remove(&id);
            self.model
                .workspace_state
                .primary_promote_cooldown_until_ms
                .remove(&id);
            self.model.focus_state.last_surface_focus_ms.remove(&id);
            self.model.monitor_state.node_monitor.remove(&id);
            self.model.carry_state.carry_zone_hint.remove(&id);
            self.model.carry_state.carry_zone_last_change_ms.remove(&id);
            self.model.carry_state.carry_zone_pending.remove(&id);
            self.model
                .carry_state
                .carry_zone_pending_since_ms
                .remove(&id);
            self.model
                .carry_state
                .carry_activation_anim_armed
                .remove(&id);
            if self.input.interaction_state.resize_active == Some(id) {
                self.input.interaction_state.resize_active = None;
            }
            if self.input.interaction_state.resize_static_node == Some(id) {
                self.input.interaction_state.resize_static_node = None;
                self.input.interaction_state.resize_static_lock_pos = None;
                self.input.interaction_state.resize_static_until_ms = 0;
            }
            if self.model.focus_state.primary_interaction_focus == Some(id) {
                self.model.focus_state.primary_interaction_focus = None;
                self.model.focus_state.interaction_focus_until_ms = 0;
            }
            self.model.focus_state.suppress_trail_record_once = false;
            self.input.interaction_state.smoothed_render_pos.remove(&id);
            self.ui.render_state.clear_window_offscreen_cache_for(id);
            let _ = self.remove_node_from_field(id, self.now_ms(Instant::now()));
        }
        self.request_maintenance();
    }
}
