use super::*;
use crate::state::{ClusterState, FocusState, FullscreenState, MonitorState};
use crate::state::ViewportPanAnim;
use eventline::info;
use halley_config::CloseRestorePanMode;
use halley_core::viewport::FocusZone;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::set_primary_focus;

struct FocusReadContext<'a> {
    field: &'a Field,
    cluster_state: &'a ClusterState,
    focus_state: &'a FocusState,
    fullscreen_state: &'a FullscreenState,
    monitor_state: &'a MonitorState,
    tuning: &'a RuntimeTuning,
    viewport: halley_core::viewport::Viewport,
    focused_monitor: &'a str,
}

enum CloseRestorePanPlan {
    None,
    PanTo(Vec2),
}

impl<'a> FocusReadContext<'a> {
    fn surface_node_matches(
        &self,
        id: NodeId,
        allow_active: bool,
        allow_node: bool,
        monitor: Option<&str>,
    ) -> bool {
        self.field.node(id).is_some_and(|n| {
            self.field.is_visible(id)
                && n.kind == halley_core::field::NodeKind::Surface
                && match n.state {
                    halley_core::field::NodeState::Active => allow_active,
                    halley_core::field::NodeState::Node => allow_node,
                    _ => false,
                }
                && monitor.is_none_or(|monitor| {
                    self.monitor_state
                        .node_monitor
                        .get(&id)
                        .is_some_and(|m| m == monitor)
                })
        })
    }

    fn fullscreen_focus_override(&self, requested: Option<NodeId>) -> Option<NodeId> {
        match requested {
            None => self.fullscreen_state
                .fullscreen_active_node
                .get(self.focused_monitor)
                .copied(),
            Some(requested_id) => {
                let requested_monitor = self
                    .fullscreen_monitor_for_node(requested_id)
                    .map(str::to_string)
                    .or_else(|| self.monitor_state.node_monitor.get(&requested_id).cloned());
                let Some(requested_monitor) = requested_monitor else {
                    return requested;
                };
                let fullscreen_id = self.fullscreen_state
                    .fullscreen_active_node
                    .get(requested_monitor.as_str())
                    .copied();
                let fullscreen_monitor = fullscreen_id.and_then(|fullscreen_id| {
                    self.fullscreen_monitor_for_node(fullscreen_id).or_else(|| {
                        self.monitor_state
                            .node_monitor
                            .get(&fullscreen_id)
                            .map(String::as_str)
                    })
                });
                if fullscreen_id == Some(requested_id) {
                    return requested;
                }
                let requested_monitor = self.monitor_state
                    .node_monitor
                    .get(&requested_id)
                    .map(String::as_str)
                    .or(fullscreen_monitor);
                if requested_monitor == fullscreen_monitor {
                    fullscreen_id
                } else {
                    requested
                }
            }
        }
    }

    fn fullscreen_monitor_for_node(&self, id: NodeId) -> Option<&str> {
        self.fullscreen_state
            .fullscreen_active_node
            .iter()
            .find_map(|(monitor, nid)| (*nid == id).then_some(monitor.as_str()))
    }

    fn viewport_for_monitor(&self, monitor: &str) -> halley_core::viewport::Viewport {
        if self.monitor_state.current_monitor == monitor {
            self.viewport
        } else {
            self.monitor_state
                .monitors
                .get(monitor)
                .map(|space| space.viewport)
                .unwrap_or(self.viewport)
        }
    }

    fn surface_is_sufficiently_visible_on_monitor(&self, st: &Halley, monitor: &str, id: NodeId) -> bool {
        let Some(node) = self.field.node(id) else {
            return false;
        };
        let ext = st.spawn_obstacle_extents_for_node(node);
        let viewport = self.viewport_for_monitor(monitor);
        let margin_x = (viewport.size.x * 0.08).clamp(32.0, 160.0);
        let margin_y = (viewport.size.y * 0.08).clamp(32.0, 120.0);
        let min_x = viewport.center.x - viewport.size.x * 0.5 + margin_x;
        let max_x = viewport.center.x + viewport.size.x * 0.5 - margin_x;
        let min_y = viewport.center.y - viewport.size.y * 0.5 + margin_y;
        let max_y = viewport.center.y + viewport.size.y * 0.5 - margin_y;

        node.pos.x - ext.left >= min_x
            && node.pos.x + ext.right <= max_x
            && node.pos.y - ext.top >= min_y
            && node.pos.y + ext.bottom <= max_y
    }

    fn minimal_reveal_center_for_surface_on_monitor(
        &self,
        st: &Halley,
        monitor: &str,
        id: NodeId,
    ) -> Option<Vec2> {
        let node = self.field.node(id)?;
        let ext = st.spawn_obstacle_extents_for_node(node);
        let viewport = self.viewport_for_monitor(monitor);
        let margin_x = (viewport.size.x * 0.08).clamp(32.0, 160.0);
        let margin_y = (viewport.size.y * 0.08).clamp(32.0, 120.0);
        let avail_w = (viewport.size.x - margin_x * 2.0).max(1.0);
        let avail_h = (viewport.size.y - margin_y * 2.0).max(1.0);

        let mut target = viewport.center;
        if ext.left + ext.right > avail_w {
            target.x = node.pos.x;
        } else {
            let min_x = viewport.center.x - viewport.size.x * 0.5 + margin_x;
            let max_x = viewport.center.x + viewport.size.x * 0.5 - margin_x;
            let left = node.pos.x - ext.left;
            let right = node.pos.x + ext.right;
            if left < min_x {
                target.x += left - min_x;
            } else if right > max_x {
                target.x += right - max_x;
            }
        }

        if ext.top + ext.bottom > avail_h {
            target.y = node.pos.y;
        } else {
            let min_y = viewport.center.y - viewport.size.y * 0.5 + margin_y;
            let max_y = viewport.center.y + viewport.size.y * 0.5 - margin_y;
            let top = node.pos.y - ext.top;
            let bottom = node.pos.y + ext.bottom;
            if top < min_y {
                target.y += top - min_y;
            } else if bottom > max_y {
                target.y += bottom - max_y;
            }
        }

        Some(target)
    }

    fn close_restore_pan_plan(&self, st: &Halley, monitor: &str, id: NodeId) -> CloseRestorePanPlan {
        if self.cluster_state.active_cluster_workspaces.contains_key(monitor) {
            return CloseRestorePanPlan::None;
        }
        if !self.tuning.close_restore_focus {
            return CloseRestorePanPlan::None;
        }

        match self.tuning.close_restore_pan {
            CloseRestorePanMode::Never => CloseRestorePanPlan::None,
            CloseRestorePanMode::Always => self
                .field
                .node(id)
                .map(|node| CloseRestorePanPlan::PanTo(node.pos))
                .unwrap_or(CloseRestorePanPlan::None),
            CloseRestorePanMode::IfOffscreen => {
                if self.surface_is_sufficiently_visible_on_monitor(st, monitor, id) {
                    CloseRestorePanPlan::None
                } else {
                    self.minimal_reveal_center_for_surface_on_monitor(st, monitor, id)
                        .map(CloseRestorePanPlan::PanTo)
                        .unwrap_or(CloseRestorePanPlan::None)
                }
            }
        }
    }

    fn last_focused_active_surface_node(&self) -> Option<NodeId> {
        if let Some(id) = self.focus_state.primary_interaction_focus
            && self.surface_node_matches(id, true, false, None)
        {
            return Some(id);
        }
        self.focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| self.surface_node_matches(id, true, false, None).then_some((id, at)))
            .max_by_key(|entry: &(NodeId, u64)| (entry.1, entry.0.as_u64()))
            .map(|(id, _)| id)
    }

    fn last_focused_surface_node(&self) -> Option<NodeId> {
        if let Some(id) = self.focus_state.primary_interaction_focus
            && self.surface_node_matches(id, true, true, None)
        {
            return Some(id);
        }
        self.focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| self.surface_node_matches(id, true, true, None).then_some((id, at)))
            .max_by_key(|entry: &(NodeId, u64)| (entry.1, entry.0.as_u64()))
            .map(|(id, _)| id)
    }

    fn last_focused_surface_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        if let Some(id) = self.focus_state.monitor_focus.get(monitor).copied()
            && self.surface_node_matches(id, true, true, Some(monitor))
        {
            return Some(id);
        }
        self.focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                self.surface_node_matches(id, true, true, Some(monitor))
                    .then_some((id, at))
            })
            .max_by_key(|entry: &(NodeId, u64)| (entry.1, entry.0.as_u64()))
            .map(|(id, _)| id)
    }

    fn last_input_surface_node(&self) -> Option<NodeId> {
        if let Some(id) = self.focus_state.primary_interaction_focus
            && self.surface_node_matches(id, true, true, None)
        {
            return Some(id);
        }
        self.focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| self.surface_node_matches(id, true, true, None).then_some((id, at)))
            .max_by_key(|entry: &(NodeId, u64)| (entry.1, entry.0.as_u64()))
            .map(|(id, _)| id)
    }

    fn last_input_surface_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        let primary = self.focus_state.primary_interaction_focus.and_then(|id| {
            self.surface_node_matches(id, true, true, Some(monitor))
                .then_some((id, u64::MAX))
        });
        let monitor_focus = self.focus_state
            .monitor_focus
            .get(monitor)
            .copied()
            .and_then(|id| {
                self.surface_node_matches(id, true, true, Some(monitor)).then_some((
                    id,
                    self.focus_state
                        .last_surface_focus_ms
                        .get(&id)
                        .copied()
                        .unwrap_or(0),
                ))
            });
        primary
            .into_iter()
            .chain(monitor_focus)
            .chain(self.focus_state.last_surface_focus_ms.iter().filter_map(|(&id, &at)| {
                self.surface_node_matches(id, true, true, Some(monitor))
                    .then_some((id, at))
            }))
            .max_by_key(|entry: &(NodeId, u64)| (entry.1, entry.0.as_u64()))
            .map(|(id, _)| id)
    }
}

impl Halley {
    pub(crate) const VIEWPORT_PAN_PRELOAD_MS: u64 = 70;
    pub(crate) const VIEWPORT_PAN_DURATION_MS: u64 = 260;
    const SPAWN_VIEW_HANDOFF_PAN_RATIO: f32 = 0.35;
    const SPAWN_VIEW_HANDOFF_FOCUS_RATIO: f32 = 0.25;

    fn focus_read_context(&self) -> FocusReadContext<'_> {
        FocusReadContext {
            field: &self.model.field,
            cluster_state: &self.model.cluster_state,
            focus_state: &self.model.focus_state,
            fullscreen_state: &self.model.fullscreen_state,
            monitor_state: &self.model.monitor_state,
            tuning: &self.runtime.tuning,
            viewport: self.model.viewport,
            focused_monitor: self.focused_monitor(),
        }
    }

    pub fn wl_surface_for_node(&self, id: NodeId) -> Option<WlSurface> {
        for top in self.platform.xdg_shell_state.toplevel_surfaces() {
            let wl = top.wl_surface().clone();
            if self.model.surface_to_node.get(&wl.id()).copied() == Some(id) {
                return Some(wl);
            }
        }
        None
    }

    pub(crate) fn update_selection_focus_from_surface(&self, surface: Option<&WlSurface>) {
        let client = surface.and_then(|wl| wl.client());
        set_data_device_focus(&self.platform.display_handle, &self.platform.seat, client.clone());
        set_primary_focus(&self.platform.display_handle, &self.platform.seat, client);
    }

    fn fullscreen_focus_override(&self, requested: Option<NodeId>) -> Option<NodeId> {
        self.focus_read_context().fullscreen_focus_override(requested)
    }

    pub fn apply_wayland_focus_state(&mut self, id: Option<NodeId>) {
        let focus_id = self.fullscreen_focus_override(id).or(id);
        if let Some(fid) = focus_id
            && self.model.fullscreen_state
                .fullscreen_suspended_node
                .values()
                .any(|&nid| nid == fid)
        {
            if let Some(entry) = self.model.fullscreen_state.fullscreen_restore.get(&fid).copied() {
                let target_monitor = self.model.monitor_state
                    .node_monitor
                    .get(&fid)
                    .cloned()
                    .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone());
                if let Some(space) = self.model.monitor_state.monitors.get_mut(&target_monitor) {
                    space.viewport.center = entry.viewport_center;
                    space.camera_target_center = entry.viewport_center;
                }
                if self.model.monitor_state.current_monitor == target_monitor {
                    self.model.viewport.center = entry.viewport_center;
                    self.model.camera_target_center = self.model.viewport.center;
                    self.runtime.tuning.viewport_center = self.model.viewport.center;
                    self.input.interaction_state.viewport_pan_anim = None;
                }
            }
            self.enter_xdg_fullscreen(fid, None, Instant::now());
        }
        self.model.monitor_state.layer_keyboard_focus = None;
        let requested_focus_surface = focus_id.and_then(|fid| self.wl_surface_for_node(fid));
        let active_locked_surface = self.active_locked_pointer_surface();
        let locked_surface_node = active_locked_surface
            .as_ref()
            .and_then(|surface| self.model.surface_to_node.get(&surface.id()).copied());
        let keep_locked_focus =
            locked_surface_node.is_some_and(|nid| self.is_fullscreen_active(nid));
        let focus_surface = if keep_locked_focus {
            active_locked_surface
                .clone()
                .or(requested_focus_surface.clone())
        } else {
            requested_focus_surface.clone()
        };
        if !keep_locked_focus
            && active_locked_surface.as_ref().is_some_and(|surface| {
                Some(surface.id()) != focus_surface.as_ref().map(|wl| wl.id())
            })
        {
            self.release_active_pointer_constraint();
        }
        if let Some(keyboard) = self.platform.seat.get_keyboard() {
            keyboard.set_focus(self, focus_surface.clone(), SERIAL_COUNTER.next_serial());
        }
        self.update_selection_focus_from_surface(focus_surface.as_ref());

        for top in self.platform.xdg_shell_state.toplevel_surfaces() {
            let key = top.wl_surface().id();
            let node_id = self.model.surface_to_node.get(&key).copied();

            let activated = node_id.is_some_and(|nid| {
                self.model.monitor_state
                    .node_monitor
                    .get(&nid)
                    .and_then(|monitor| self.model.focus_state.monitor_focus.get(monitor))
                    .copied()
                    == Some(nid)
                    || Some(nid) == focus_id
            });

            let state_changed = top.with_pending_state(|s| {
                let was_active = s.states.contains(xdg_toplevel::State::Activated);
                if activated {
                    s.states.set(xdg_toplevel::State::Activated);
                } else {
                    s.states.unset(xdg_toplevel::State::Activated);
                }
                self.apply_toplevel_tiled_hint(s);
                was_active != activated
            });

            if state_changed {
                top.send_configure();
            }
        }
    }

    pub fn update_focus_tracking_for_surface(&mut self, fid: NodeId, now_ms: u64) {
        let Some(node_state) = self.model.field
            .node(fid)
            .map(|n| (n.kind.clone(), n.state.clone()))
        else {
            return;
        };
        if node_state.0 != halley_core::field::NodeKind::Surface || !self.model.field.is_visible(fid) {
            return;
        }

        self.model.focus_state.last_surface_focus_ms.insert(fid, now_ms);
        if self.model.focus_state.suppress_trail_record_once {
            self.model.focus_state.suppress_trail_record_once = false;
        } else {
            self.record_focus_trail_visit(fid);
        }

        if node_state.1 == halley_core::field::NodeState::Active {
            let _ = self.model.field.touch(fid, now_ms);
            let _ = self.model.field.set_decay_level(fid, DecayLevel::Hot);
            if self.runtime.tuning.restore_last_active_on_pan_return {
                self.model.focus_state.pan_restore_active_focus = Some(fid);
            }
        }
    }

    pub fn note_pan_activity(&mut self, now: Instant) {
        self.input.interaction_state.viewport_pan_anim = None;
        let now_ms = self.now_ms(now);
        self.input.interaction_state.pan_dominant_until_ms = now_ms.saturating_add(220);
        let current_monitor = self.model.monitor_state.current_monitor.clone();
        let viewport_center = self.model.viewport.center;
        let spawn = self.spawn_monitor_state_mut(current_monitor.as_str());
        spawn.spawn_last_pan_ms = now_ms;
        spawn.spawn_pan_start_center.get_or_insert(viewport_center);
        if self.runtime.tuning.restore_last_active_on_pan_return
            && self.model.focus_state.pan_restore_active_focus.is_none()
        {
            self.model.focus_state.pan_restore_active_focus = self.last_focused_active_surface_node();
        }
        self.input.interaction_state.suspend_overlap_resolve = false;
        self.input.interaction_state.suspend_state_checks = false;
        self.request_maintenance();
    }

    fn spawn_view_handoff_pan_distance(&self) -> f32 {
        self.model.viewport.size.x.min(self.model.viewport.size.y) * Self::SPAWN_VIEW_HANDOFF_PAN_RATIO
    }

    fn spawn_view_handoff_focus_distance(&self) -> f32 {
        self.model.viewport.size.x.hypot(self.model.viewport.size.y) * Self::SPAWN_VIEW_HANDOFF_FOCUS_RATIO
    }

    pub(crate) fn note_pan_viewport_change(&mut self, _now: Instant) {
        let current_monitor = self.model.monitor_state.current_monitor.clone();
        if self
            .spawn_monitor_state(current_monitor.as_str())
            .spawn_anchor_mode
            == crate::state::SpawnAnchorMode::View
        {
            self.spawn_monitor_state_mut(current_monitor.as_str())
                .spawn_view_anchor = self.model.viewport.center;
        }
        self.request_maintenance();
        let Some(start_center) = self
            .spawn_monitor_state(current_monitor.as_str())
            .spawn_pan_start_center
        else {
            return;
        };

        let moved = ((self.model.viewport.center.x - start_center.x).powi(2)
            + (self.model.viewport.center.y - start_center.y).powi(2))
        .sqrt();
        if moved < self.spawn_view_handoff_pan_distance() {
            return;
        }

        let focus_far = self
            .last_input_surface_node()
            .and_then(|id| self.model.field.node(id))
            .map(|node| {
                let dx = self.model.viewport.center.x - node.pos.x;
                let dy = self.model.viewport.center.y - node.pos.y;
                dx.hypot(dy) >= self.spawn_view_handoff_focus_distance()
            })
            .unwrap_or(true);
        if !focus_far {
            return;
        }

        let viewport_center = self.model.viewport.center;
        let spawn = self.spawn_monitor_state_mut(current_monitor.as_str());
        spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
        spawn.spawn_view_anchor = viewport_center;
        spawn.spawn_patch = None;
        spawn.spawn_pan_start_center = Some(viewport_center);
        self.model.focus_state.pan_restore_active_focus = None;
    }

    pub fn set_pan_restore_focus_target(&mut self, id: NodeId) {
        self.model.focus_state.pan_restore_active_focus = Some(id);
    }

    pub fn animate_viewport_center_to(&mut self, target_center: Vec2, now: Instant) -> bool {
        self.animate_viewport_center_to_delayed(target_center, now, 0)
    }

    pub fn animate_viewport_center_to_delayed(
        &mut self,
        target_center: Vec2,
        now: Instant,
        delay_ms: u64,
    ) -> bool {
        let from = self.model.viewport.center;
        let dx = target_center.x - from.x;
        let dy = target_center.y - from.y;
        if dx.abs() < 0.25 && dy.abs() < 0.25 {
            return false;
        }
        self.input.interaction_state.viewport_pan_anim = Some(ViewportPanAnim {
            start_ms: self.now_ms(now),
            delay_ms,
            duration_ms: Self::VIEWPORT_PAN_DURATION_MS,
            from_center: from,
            to_center: target_center,
        });
        true
    }

    pub(crate) fn tick_viewport_pan_animation(&mut self, now_ms: u64) {
        let Some(anim) = &self.input.interaction_state.viewport_pan_anim else {
            return;
        };
        if now_ms <= anim.start_ms.saturating_add(anim.delay_ms) {
            self.model.viewport.center = anim.from_center;
            self.model.camera_target_center = self.model.viewport.center;
            self.runtime.tuning.viewport_center = self.model.viewport.center;
            return;
        }
        let dur = anim.duration_ms.max(1);
        let elapsed_ms = now_ms.saturating_sub(anim.start_ms.saturating_add(anim.delay_ms));
        let t = (elapsed_ms as f32 / dur as f32).clamp(0.0, 1.0);
        let e = if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
        };
        self.model.viewport.center = Vec2 {
            x: anim.from_center.x + (anim.to_center.x - anim.from_center.x) * e,
            y: anim.from_center.y + (anim.to_center.y - anim.from_center.y) * e,
        };
        self.model.camera_target_center = self.model.viewport.center;
        self.runtime.tuning.viewport_center = self.model.viewport.center;
        if t >= 1.0 {
            self.input.interaction_state.viewport_pan_anim = None;
        }
    }

    pub(crate) fn surface_is_sufficiently_visible_on_monitor(
        &self,
        monitor: &str,
        id: NodeId,
    ) -> bool {
        self.focus_read_context()
            .surface_is_sufficiently_visible_on_monitor(self, monitor, id)
    }

    pub(crate) fn minimal_reveal_center_for_surface_on_monitor(
        &self,
        monitor: &str,
        id: NodeId,
    ) -> Option<Vec2> {
        self.focus_read_context()
            .minimal_reveal_center_for_surface_on_monitor(self, monitor, id)
    }

    pub(crate) fn maybe_pan_to_restored_focus_on_close(
        &mut self,
        monitor: &str,
        id: NodeId,
        now: Instant,
    ) -> bool {
        match self.focus_read_context().close_restore_pan_plan(self, monitor, id) {
            CloseRestorePanPlan::None => false,
            CloseRestorePanPlan::PanTo(target) => self.animate_viewport_center_to(target, now),
        }
    }

    fn last_focused_active_surface_node(&self) -> Option<NodeId> {
        self.focus_read_context().last_focused_active_surface_node()
    }

    pub fn begin_resize_interaction(&mut self, id: NodeId, now: Instant) {
        self.input.interaction_state.resize_active = Some(id);
        self.input.interaction_state.resize_static_node = Some(id);
        self.input.interaction_state.resize_static_lock_pos = None;
        self.input.interaction_state.resize_static_until_ms = self.now_ms(now).saturating_add(60_000);
        self.input.interaction_state.suspend_overlap_resolve = true;
        self.input.interaction_state.suspend_state_checks = true;
        self.set_interaction_focus(Some(id), 60_000, now);
        let now_ms = self.now_ms(now);
        let _ = self.model.field.touch(id, now_ms);
        let _ = self.model.field.set_decay_level(id, DecayLevel::Hot);
        self.model.workspace_state.manual_collapsed_nodes.remove(&id);
        self.request_maintenance();
    }

    pub fn end_resize_interaction(&mut self, now: Instant) {
        let ended = self.input.interaction_state.resize_active.take();
        if let Some(id) = ended {
            self.input.interaction_state.resize_static_node = Some(id);
            self.input.interaction_state.resize_static_lock_pos = self.model.field.node(id).map(|n| n.pos);
            self.input.interaction_state.resize_static_until_ms = self.now_ms(now).saturating_add(120);
            self.set_interaction_focus(Some(id), 30_000, now);
        } else {
            self.input.interaction_state.resize_static_lock_pos = None;
            self.set_interaction_focus(None, 0, now);
        }
        self.input.interaction_state.suspend_state_checks = false;
        self.input.interaction_state.suspend_overlap_resolve = false;
        self.resolve_surface_overlap();
        self.request_maintenance();
    }

    pub fn resolve_overlap_now(&mut self) {
        let saved_suspend = self.input.interaction_state.suspend_overlap_resolve;
        self.input.interaction_state.suspend_overlap_resolve = false;
        self.resolve_surface_overlap();
        self.input.interaction_state.suspend_overlap_resolve = saved_suspend;
    }

    pub fn set_last_active_size_now(&mut self, id: NodeId, size: Vec2) {
        self.model.workspace_state.last_active_size.insert(id, size);
    }

    pub fn last_focused_surface_node(&self) -> Option<NodeId> {
        self.focus_read_context().last_focused_surface_node()
    }

    pub fn last_focused_surface_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        self.focus_read_context()
            .last_focused_surface_node_for_monitor(monitor)
    }

    pub fn last_input_surface_node(&self) -> Option<NodeId> {
        self.focus_read_context().last_input_surface_node()
    }

    pub fn last_input_surface_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        self.focus_read_context()
            .last_input_surface_node_for_monitor(monitor)
    }

    pub fn toggle_last_focused_surface_node(&mut self, now: Instant) -> Option<NodeId> {
        let id = self
            .last_focused_surface_node_for_monitor(self.model.monitor_state.current_monitor.as_str())
            .or_else(|| self.last_focused_surface_node())?;

        let state = self.model.field.node(id)?.state.clone();
        match state {
            halley_core::field::NodeState::Active => {
                let _ = self.model.field
                    .set_state(id, halley_core::field::NodeState::Node);
                let _ = self.model.field.set_decay_level(id, DecayLevel::Cold);
                self.model.spawn_state.pending_spawn_activate_at_ms.remove(&id);
                self.model.workspace_state.manual_collapsed_nodes.insert(id);

                self.set_interaction_focus(None, 0, now);
                self.model.focus_state.pan_restore_active_focus = None;
                self.request_maintenance();
                Some(id)
            }
            halley_core::field::NodeState::Node => {
                self.model.workspace_state.manual_collapsed_nodes.remove(&id);
                let _ = self.model.field.set_decay_level(id, DecayLevel::Hot);
                self.model.spawn_state.pending_spawn_activate_at_ms.remove(&id);

                self.set_interaction_focus(Some(id), 30_000, now);
                self.request_maintenance();
                Some(id)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_input_surface_prefers_current_monitor_local_focus() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let left = state.model.field.spawn_surface(
            "left",
            Vec2 { x: -200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let right = state.model.field.spawn_surface(
            "right",
            Vec2 { x: 200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.assign_node_to_monitor(left, "default");
        state.assign_node_to_monitor(right, "other");
        state.model.focus_state.primary_interaction_focus = Some(right);
        state.model.focus_state.last_surface_focus_ms.insert(left, 1);
        state.model.focus_state.last_surface_focus_ms.insert(right, 2);
        state.model.focus_state
            .monitor_focus
            .insert("default".to_string(), left);
        state.model.focus_state
            .monitor_focus
            .insert("other".to_string(), right);

        assert_eq!(
            state.last_input_surface_node_for_monitor("default"),
            Some(left)
        );
        assert_eq!(
            state.last_input_surface_node_for_monitor("other"),
            Some(right)
        );
    }

    #[test]
    fn fullscreen_focus_override_stays_on_requested_monitor() {
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
        let right = state.model.field.spawn_surface(
            "right",
            Vec2 {
                x: 1200.0,
                y: 300.0,
            },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_monitor(fullscreen_left, "left");
        state.assign_node_to_monitor(right, "right");
        state.model.fullscreen_state
            .fullscreen_active_node
            .insert("left".to_string(), fullscreen_left);
        state.set_interaction_monitor("right");
        state.set_focused_monitor("right");

        assert_eq!(state.fullscreen_focus_override(Some(right)), Some(right));
        assert_eq!(state.fullscreen_focus_override(None), None);
    }

    #[test]
    fn fullscreen_focus_override_keeps_same_monitor_fullscreen() {
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
        let other_left = state.model.field.spawn_surface(
            "other-left",
            Vec2 { x: 500.0, y: 300.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_monitor(fullscreen_left, "left");
        state.assign_node_to_monitor(other_left, "left");
        state.model.fullscreen_state
            .fullscreen_active_node
            .insert("left".to_string(), fullscreen_left);
        state.set_interaction_monitor("left");
        state.set_focused_monitor("left");

        assert_eq!(
            state.fullscreen_focus_override(Some(other_left)),
            Some(fullscreen_left)
        );
        assert_eq!(state.fullscreen_focus_override(None), Some(fullscreen_left));
    }

    #[test]
    fn setting_interaction_focus_switches_current_monitor_to_focused_node_monitor() {
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
        let _ = state.activate_monitor("left");

        let right = state.model.field.spawn_surface(
            "right",
            Vec2 {
                x: 1200.0,
                y: 300.0,
            },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_monitor(right, "right");

        state.set_interaction_focus(Some(right), 30_000, Instant::now());

        assert_eq!(state.focused_monitor(), "right");
        assert_eq!(state.interaction_monitor(), "right");
        assert_eq!(state.model.monitor_state.current_monitor, "right");
    }

    #[test]
    fn focus_monitor_view_restores_last_focused_surface_on_monitor() {
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

        let right = state.model.field.spawn_surface(
            "right",
            Vec2 {
                x: 1200.0,
                y: 300.0,
            },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_monitor(right, "right");
        state.set_interaction_focus(Some(right), 30_000, Instant::now());

        state.focus_monitor_view("left", Instant::now());
        assert_eq!(state.model.focus_state.primary_interaction_focus, None);

        state.focus_monitor_view("right", Instant::now());

        assert_eq!(state.focused_monitor(), "right");
        assert_eq!(state.interaction_monitor(), "right");
        assert_eq!(state.model.monitor_state.current_monitor, "right");
        assert_eq!(state.model.focus_state.primary_interaction_focus, Some(right));
    }

    #[test]
    fn focus_monitor_view_uses_bare_monitor_view_when_no_surface_exists() {
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

        state.focus_monitor_view("right", Instant::now());

        assert_eq!(state.focused_monitor(), "right");
        assert_eq!(state.interaction_monitor(), "right");
        assert_eq!(state.model.monitor_state.current_monitor, "right");
        assert_eq!(state.model.focus_state.primary_interaction_focus, None);
    }

    #[test]
    fn focus_monitor_view_does_not_restore_blocked_monitor_focus() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.close_restore_focus = false;
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "right".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        }];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let right = state.model.field.spawn_surface(
            "right",
            Vec2 { x: 200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.assign_node_to_monitor(right, "right");
        state.model.focus_state
            .monitor_focus
            .insert("right".to_string(), right);
        state.model.focus_state
            .blocked_monitor_focus_restore
            .insert("right".to_string());

        state.focus_monitor_view("right", Instant::now());

        assert_eq!(state.model.focus_state.primary_interaction_focus, None);
    }
}
