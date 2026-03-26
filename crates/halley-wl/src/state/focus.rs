use std::collections::HashMap;
use std::time::Instant;

use eventline::info;
use halley_core::decay::DecayLevel;
use halley_core::field::NodeId;
use halley_core::trail::Trail;
use halley_core::viewport::FocusZone;

use crate::state::Halley;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::SERIAL_COUNTER;

pub(crate) struct FocusState {
    pub(crate) primary_interaction_focus: Option<NodeId>,
    pub(crate) monitor_focus: HashMap<String, NodeId>,
    pub(crate) interaction_focus_until_ms: u64,
    pub(crate) last_surface_focus_ms: HashMap<NodeId, u64>,
    pub(crate) focus_trail: Trail,
    pub(crate) suppress_trail_record_once: bool,
    pub(crate) pan_restore_active_focus: Option<NodeId>,
    pub(crate) app_focused: bool,
    pub(crate) focus_ring_preview_until_ms: HashMap<String, u64>,
    pub(crate) recent_top_node: Option<NodeId>,
    pub(crate) recent_top_until: Option<Instant>,
}

impl Halley {
    pub(crate) fn focus_monitor_view(&mut self, monitor: &str, now: Instant) {
        self.set_interaction_monitor(monitor);
        let _ = self.activate_monitor(monitor);
        self.set_interaction_focus(None, 0, now);
        let view_center = self.view_center_for_monitor(monitor);
        let spawn = self.spawn_monitor_state_mut(monitor);
        spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
        spawn.spawn_view_anchor = view_center;
        spawn.spawn_patch = None;
        spawn.spawn_pan_start_center = None;
    }

    pub fn set_interaction_focus(&mut self, id: Option<NodeId>, hold_ms: u64, now: Instant) {
        let prev = self.focus_state.primary_interaction_focus;
        let now_ms = self.now_ms(now);

        if prev == id {
            if let Some(fid) = id {
                let requested_until = now_ms.saturating_add(hold_ms.max(1));
                self.focus_state.interaction_focus_until_ms = self
                    .focus_state
                    .interaction_focus_until_ms
                    .max(requested_until);
                self.update_focus_tracking_for_surface(fid, now_ms);
                if let Some(monitor) = self.monitor_state.node_monitor.get(&fid).cloned() {
                    self.set_interaction_monitor(monitor.as_str());
                    let spawn = self.spawn_monitor_state_mut(monitor.as_str());
                    spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::Focus;
                    spawn.spawn_pan_start_center = None;
                    self.focus_state.monitor_focus.insert(monitor, fid);
                } else {
                    let current_monitor = self.monitor_state.current_monitor.clone();
                    let spawn = self.spawn_monitor_state_mut(current_monitor.as_str());
                    spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::Focus;
                    spawn.spawn_pan_start_center = None;
                }

                self.reassert_wayland_keyboard_focus_if_drifted(id);
            } else {
                self.focus_state.interaction_focus_until_ms = 0;
                self.reassert_wayland_keyboard_focus_if_drifted(None);
            }
            self.request_maintenance();
            return;
        }

        self.focus_state.primary_interaction_focus = id;
        if let Some(fid) = id {
            self.focus_state.interaction_focus_until_ms = now_ms.saturating_add(hold_ms.max(1));
            self.update_focus_tracking_for_surface(fid, now_ms);
            if let Some(monitor) = self.monitor_state.node_monitor.get(&fid).cloned() {
                self.set_interaction_monitor(monitor.as_str());
                let spawn = self.spawn_monitor_state_mut(monitor.as_str());
                spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::Focus;
                spawn.spawn_pan_start_center = None;
                self.focus_state.monitor_focus.insert(monitor, fid);
            } else {
                let current_monitor = self.monitor_state.current_monitor.clone();
                let spawn = self.spawn_monitor_state_mut(current_monitor.as_str());
                spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::Focus;
                spawn.spawn_pan_start_center = None;
            }
        } else {
            self.focus_state.interaction_focus_until_ms = 0;
        }

        if prev != id {
            info!(
                "interaction focus changed: {:?} -> {:?} (hold_ms={})",
                prev.map(|n| n.as_u64()),
                id.map(|n| n.as_u64()),
                hold_ms
            );
        }
        self.apply_wayland_focus_state(id);
        self.request_maintenance();
    }

    pub(crate) fn restore_pan_return_active_focus(&mut self, now: Instant) {
        if !self.tuning.restore_last_active_on_pan_return {
            self.focus_state.pan_restore_active_focus = None;
            return;
        }
        let Some(id) = self.focus_state.pan_restore_active_focus else {
            return;
        };
        let Some(n) = self.field.node(id) else {
            self.focus_state.pan_restore_active_focus = None;
            return;
        };
        if !self.field.is_visible(id) || n.kind != halley_core::field::NodeKind::Surface {
            self.focus_state.pan_restore_active_focus = None;
            return;
        }
        if n.state == halley_core::field::NodeState::Active {
            self.focus_state.pan_restore_active_focus = None;
            return;
        }

        if self.preserve_collapsed_surface(id) {
            self.focus_state.pan_restore_active_focus = None;
            return;
        }

        let target_monitor = self
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| self.monitor_state.current_monitor.clone());
        let focus_center = self.view_center_for_monitor(target_monitor.as_str());
        let focus_ring = self.focus_ring_for_monitor(target_monitor.as_str());
        if focus_ring.zone(focus_center, n.pos) != FocusZone::Inside {
            return;
        }

        let _ = self.field.set_decay_level(id, DecayLevel::Hot);
        self.mark_active_transition(id, now, 280);

        self.set_interaction_focus(Some(id), 12_000, now);
        self.focus_state.pan_restore_active_focus = None;
    }

    pub fn reassert_wayland_keyboard_focus_if_drifted(&mut self, id: Option<NodeId>) {
        if self.monitor_state.layer_keyboard_focus.is_some() {
            self.reassert_layer_surface_keyboard_focus_if_drifted();
            return;
        }
        let desired_focus = id.and_then(|fid| self.wl_surface_for_node(fid));
        if let Some(keyboard) = self.seat.get_keyboard() {
            let current_focus = keyboard.current_focus();
            let matches = match (&current_focus, &desired_focus) {
                (Some(current), Some(desired)) => current.id() == desired.id(),
                (None, None) => true,
                _ => false,
            };
            if !matches {
                info!(
                    "keyboard focus drift detected; reasserting desired focus={:?} current={:?}",
                    desired_focus.as_ref().map(|wl| format!("{:?}", wl.id())),
                    current_focus.as_ref().map(|wl| format!("{:?}", wl.id()))
                );
                keyboard.set_focus(self, desired_focus.clone(), SERIAL_COUNTER.next_serial());
                self.update_selection_focus_from_surface(desired_focus.as_ref());
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn focused_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        self.focus_state.monitor_focus.get(monitor).copied()
    }

    #[allow(dead_code)]
    pub(crate) fn focused_monitor_for_node(&self, id: NodeId) -> Option<String> {
        self.monitor_state.node_monitor.get(&id).cloned()
    }

    #[allow(dead_code)]
    pub(crate) fn set_monitor_focus(&mut self, monitor: &str, id: NodeId) {
        self.focus_state
            .monitor_focus
            .insert(monitor.to_string(), id);
    }

    pub fn set_recent_top_node(&mut self, node_id: NodeId, until: Instant) {
        self.focus_state.recent_top_node = Some(node_id);
        self.focus_state.recent_top_until = Some(until);
    }

    pub fn recent_top_node_active(&mut self, now: Instant) -> Option<NodeId> {
        if self
            .focus_state
            .recent_top_until
            .is_some_and(|until| now >= until)
        {
            self.focus_state.recent_top_node = None;
            self.focus_state.recent_top_until = None;
            return None;
        }
        self.focus_state.recent_top_node
    }
}
