use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::time::Instant;

use eventline::debug;
use halley_core::decay::DecayLevel;
use halley_core::field::NodeId;
use halley_core::trail::Trail;
use halley_core::viewport::{FocusRing, FocusZone};

use crate::compositor::root::Halley;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::SERIAL_COUNTER;

pub(crate) struct FocusState {
    pub(crate) primary_interaction_focus: Option<NodeId>,
    pub(crate) monitor_focus: HashMap<String, NodeId>,
    pub(crate) blocked_monitor_focus_restore: HashSet<String>,
    pub(crate) interaction_focus_until_ms: u64,
    pub(crate) last_surface_focus_ms: HashMap<NodeId, u64>,
    pub(crate) outside_focus_ring_since_ms: HashMap<NodeId, u64>,
    pub(crate) focus_trail: HashMap<String, Trail>,
    pub(crate) suppress_trail_record_once: bool,
    pub(crate) pan_restore_active_focus: Option<NodeId>,
    pub(crate) app_focused: bool,
    pub(crate) focus_ring_preview_until_ms: HashMap<String, u64>,
    pub(crate) recent_top_node: Option<NodeId>,
    pub(crate) recent_top_until: Option<Instant>,
    pub(crate) overlap_raise_order: HashMap<NodeId, u64>,
    pub(crate) next_overlap_raise_order: u64,
}

pub(crate) const COMPANION_PROTECT_MS: u64 = 12_000;
pub(crate) const FOCUS_RING_PREVIEW_MS: u64 = 1_500;

pub(crate) struct FocusStateController<T> {
    st: T,
}

pub(crate) fn focus_state_controller<T>(st: T) -> FocusStateController<T> {
    FocusStateController { st }
}

impl<T: Deref<Target = Halley>> Deref for FocusStateController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for FocusStateController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

impl<T: Deref<Target = Halley>> FocusStateController<T> {
    pub(crate) fn companion_surface_node(&self, now_ms: u64) -> Option<NodeId> {
        let focused = self.model.focus_state.primary_interaction_focus;
        self.model
            .focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                if Some(id) == focused {
                    return None;
                }
                if now_ms.saturating_sub(at) > COMPANION_PROTECT_MS {
                    return None;
                }
                self.model.field.node(id).and_then(|n| {
                    (self.model.field.is_visible(id)
                        && n.kind == halley_core::field::NodeKind::Surface)
                        .then_some((id, at))
                })
            })
            .max_by_key(|(id, at)| (*at, id.as_u64()))
            .map(|(id, _)| id)
    }

    pub fn active_focus_ring(&self) -> FocusRing {
        self.runtime
            .tuning
            .focus_ring_for_output(self.model.monitor_state.current_monitor.as_str())
    }

    pub fn focus_ring_for_monitor(&self, monitor: &str) -> FocusRing {
        self.runtime.tuning.focus_ring_for_output(monitor)
    }

    pub fn should_draw_focus_ring_preview(&self, now: Instant) -> bool {
        self.model
            .focus_state
            .focus_ring_preview_until_ms
            .get(self.model.monitor_state.current_monitor.as_str())
            .is_some_and(|&until_ms| self.now_ms(now) < until_ms)
    }

    #[allow(dead_code)]
    pub(crate) fn focused_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        self.model.focus_state.monitor_focus.get(monitor).copied()
    }

    #[allow(dead_code)]
    pub(crate) fn focused_monitor_for_node(&self, id: NodeId) -> Option<String> {
        self.model.monitor_state.node_monitor.get(&id).cloned()
    }

    pub fn overlap_policy_stack_rank(&self, node_id: NodeId) -> (u64, u64) {
        (
            self.model
                .focus_state
                .overlap_raise_order
                .get(&node_id)
                .copied()
                .unwrap_or(0),
            node_id.as_u64(),
        )
    }

    fn active_surface_is_frontmost_on_monitor(&self, node_id: NodeId) -> bool {
        let Some(target_monitor) = self.model.monitor_state.node_monitor.get(&node_id) else {
            return false;
        };
        let target_rank = self.overlap_policy_stack_rank(node_id);
        self.model
            .field
            .nodes()
            .iter()
            .filter(|&(id, node)| {
                *id != node_id
                    && node.kind == halley_core::field::NodeKind::Surface
                    && node.state == halley_core::field::NodeState::Active
                    && self.model.field.is_visible(*id)
                    && self
                        .model
                        .monitor_state
                        .node_monitor
                        .get(id)
                        .is_some_and(|monitor| monitor == target_monitor)
            })
            .all(|(&id, _)| self.overlap_policy_stack_rank(id) <= target_rank)
    }
}

impl<T: DerefMut<Target = Halley>> FocusStateController<T> {
    pub(crate) fn focus_monitor_view(&mut self, monitor: &str, now: Instant) {
        let open_monitors = self
            .model
            .cluster_state
            .cluster_bloom_open
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for open_monitor in open_monitors {
            if open_monitor != monitor {
                let _ = self.close_cluster_bloom_for_monitor(open_monitor.as_str());
            }
        }
        self.set_interaction_monitor(monitor);
        self.set_focused_monitor(monitor);
        self.model.spawn_state.pending_spawn_monitor = None;
        let _ = self.activate_monitor(monitor);
        if !self
            .model
            .focus_state
            .blocked_monitor_focus_restore
            .contains(monitor)
            && let Some(id) = self.last_focused_surface_node_for_monitor(monitor)
        {
            self.set_interaction_focus(Some(id), 30_000, now);
            debug!(
                "monitor focus restored surface: monitor={} node_id={}",
                monitor,
                id.as_u64()
            );
            return;
        }
        self.set_interaction_focus(None, 0, now);
        let view_center = self.view_center_for_monitor(monitor);
        let spawn = self.spawn_monitor_state_mut(monitor);
        spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::View;
        spawn.spawn_view_anchor = view_center;
        spawn.spawn_patch = None;
        spawn.spawn_pan_start_center = None;
        debug!(
            "monitor view focus set: monitor={} interaction_monitor={} focused_monitor={}",
            monitor,
            self.interaction_monitor(),
            self.focused_monitor()
        );
    }

    pub fn set_interaction_focus(&mut self, id: Option<NodeId>, hold_ms: u64, now: Instant) {
        let prev = self.model.focus_state.primary_interaction_focus;
        let now_ms = self.now_ms(now);

        if prev == id {
            if let Some(fid) = id {
                let requested_until = now_ms.saturating_add(hold_ms.max(1));
                self.model.focus_state.interaction_focus_until_ms = self
                    .model
                    .focus_state
                    .interaction_focus_until_ms
                    .max(requested_until);
                self.update_focus_tracking_for_surface(fid, now_ms);
                if let Some(monitor) = self.model.monitor_state.node_monitor.get(&fid).cloned() {
                    self.model
                        .focus_state
                        .blocked_monitor_focus_restore
                        .remove(&monitor);
                    self.set_interaction_monitor(monitor.as_str());
                    self.set_focused_monitor(monitor.as_str());
                    self.model.spawn_state.pending_spawn_monitor = None;
                    let _ = self.activate_monitor(monitor.as_str());
                    let spawn = self.spawn_monitor_state_mut(monitor.as_str());
                    spawn.spawn_anchor_mode =
                        crate::compositor::spawn::state::SpawnAnchorMode::Focus;
                    spawn.spawn_pan_start_center = None;
                    self.model.focus_state.monitor_focus.insert(monitor, fid);
                } else {
                    let current_monitor = self.model.monitor_state.current_monitor.clone();
                    let spawn = self.spawn_monitor_state_mut(current_monitor.as_str());
                    spawn.spawn_anchor_mode =
                        crate::compositor::spawn::state::SpawnAnchorMode::Focus;
                    spawn.spawn_pan_start_center = None;
                }

                self.reassert_wayland_keyboard_focus_if_drifted(id);
            } else {
                self.model.focus_state.interaction_focus_until_ms = 0;
                self.reassert_wayland_keyboard_focus_if_drifted(None);
            }
            self.request_maintenance();
            return;
        }

        self.model.focus_state.primary_interaction_focus = id;
        if let Some(fid) = id {
            self.model.focus_state.interaction_focus_until_ms =
                now_ms.saturating_add(hold_ms.max(1));
            self.update_focus_tracking_for_surface(fid, now_ms);
            if let Some(monitor) = self.model.monitor_state.node_monitor.get(&fid).cloned() {
                let open_monitors = self
                    .model
                    .cluster_state
                    .cluster_bloom_open
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>();
                for open_monitor in open_monitors {
                    if open_monitor != monitor {
                        let _ = self.close_cluster_bloom_for_monitor(open_monitor.as_str());
                    }
                }
                self.model
                    .focus_state
                    .blocked_monitor_focus_restore
                    .remove(&monitor);
                self.set_interaction_monitor(monitor.as_str());
                self.set_focused_monitor(monitor.as_str());
                self.model.spawn_state.pending_spawn_monitor = None;
                let _ = self.activate_monitor(monitor.as_str());
                let spawn = self.spawn_monitor_state_mut(monitor.as_str());
                spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::Focus;
                spawn.spawn_pan_start_center = None;
                self.model.focus_state.monitor_focus.insert(monitor, fid);
            } else {
                let current_monitor = self.model.monitor_state.current_monitor.clone();
                let spawn = self.spawn_monitor_state_mut(current_monitor.as_str());
                spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::Focus;
                spawn.spawn_pan_start_center = None;
            }
        } else {
            self.model.focus_state.interaction_focus_until_ms = 0;
        }

        if prev != id {
            debug!(
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
        if !self.runtime.tuning.restore_last_active_on_pan_return {
            self.model.focus_state.pan_restore_active_focus = None;
            return;
        }
        let Some(id) = self.model.focus_state.pan_restore_active_focus else {
            return;
        };
        let Some(n) = self.model.field.node(id) else {
            self.model.focus_state.pan_restore_active_focus = None;
            return;
        };
        if !self.model.field.is_visible(id) || n.kind != halley_core::field::NodeKind::Surface {
            self.model.focus_state.pan_restore_active_focus = None;
            return;
        }
        if n.state == halley_core::field::NodeState::Active {
            self.model.focus_state.pan_restore_active_focus = None;
            return;
        }

        if crate::compositor::workspace::state::preserve_collapsed_surface(&**self, id) {
            self.model.focus_state.pan_restore_active_focus = None;
            return;
        }

        let target_monitor = self
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone());
        let focus_center = self.view_center_for_monitor(target_monitor.as_str());
        let focus_ring = self.focus_ring_for_monitor(target_monitor.as_str());
        if focus_ring.zone(focus_center, n.pos) != FocusZone::Inside {
            return;
        }

        let _ = self.model.field.set_decay_level(id, DecayLevel::Hot);
        crate::compositor::workspace::state::mark_active_transition(&mut **self, id, now, 280);

        self.set_interaction_focus(Some(id), 12_000, now);
        self.model.focus_state.pan_restore_active_focus = None;
    }

    pub fn reassert_wayland_keyboard_focus_if_drifted(&mut self, id: Option<NodeId>) {
        if self.model.monitor_state.layer_keyboard_focus.is_some() {
            crate::compositor::monitor::layer_shell::reassert_layer_surface_keyboard_focus_if_drifted(self);
            return;
        }
        let desired_focus = id.and_then(|fid| self.wl_surface_for_node(fid));
        if let Some(keyboard) = self.platform.seat.get_keyboard() {
            let current_focus = keyboard.current_focus();
            let matches = match (&current_focus, &desired_focus) {
                (Some(current), Some(desired)) => current.id() == desired.id(),
                (None, None) => true,
                _ => false,
            };
            if !matches {
                debug!(
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
    pub(crate) fn set_monitor_focus(&mut self, monitor: &str, id: NodeId) {
        self.model
            .focus_state
            .monitor_focus
            .insert(monitor.to_string(), id);
    }

    pub fn set_recent_top_node(&mut self, node_id: NodeId, until: Instant) {
        self.model.focus_state.recent_top_node = Some(node_id);
        self.model.focus_state.recent_top_until = Some(until);
    }

    pub fn raise_overlap_policy_node(&mut self, node_id: NodeId) -> bool {
        if !self.model.field.node(node_id).is_some_and(|node| {
            node.kind == halley_core::field::NodeKind::Surface
                && node.state == halley_core::field::NodeState::Active
                && self.model.field.is_visible(node_id)
        }) {
            return false;
        }
        if self.active_surface_is_frontmost_on_monitor(node_id) {
            return false;
        }
        self.model.focus_state.next_overlap_raise_order = self
            .model
            .focus_state
            .next_overlap_raise_order
            .saturating_add(1);
        let order = self.model.focus_state.next_overlap_raise_order;
        self.model
            .focus_state
            .overlap_raise_order
            .insert(node_id, order);
        if self.runtime.tuning.raise_animation_enabled() {
            let now = Instant::now();
            self.request_window_animation_prewarm(node_id, now);
            let duration_ms = self.runtime.tuning.raise_animation_duration_ms();
            let scale = self.runtime.tuning.raise_animation_scale();
            let shadow_boost = self.runtime.tuning.raise_animation_shadow_boost();
            self.ui.render_state.start_raise_animation(
                node_id,
                now,
                duration_ms,
                scale,
                shadow_boost,
            );
        }
        self.request_maintenance();
        true
    }

    pub fn recent_top_node_active(&mut self, now: Instant) -> Option<NodeId> {
        if self
            .model
            .focus_state
            .recent_top_until
            .is_some_and(|until| now >= until)
        {
            self.model.focus_state.recent_top_node = None;
            self.model.focus_state.recent_top_until = None;
            return None;
        }
        self.model.focus_state.recent_top_node
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::reexports::wayland_server::Display;

    #[test]
    fn raising_active_window_starts_raise_animation() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let node = state.model.field.spawn_surface(
            "window",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 320.0, y: 200.0 },
        );
        let _ = state
            .model
            .field
            .set_state(node, halley_core::field::NodeState::Active);

        assert!(state.raise_overlap_policy_node(node));

        assert!(state.ui.render_state.raise_animations.contains_key(&node));
    }

    #[test]
    fn raising_frontmost_window_does_not_restart_raise_animation() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let monitor = state.model.monitor_state.current_monitor.clone();
        let back = state.model.field.spawn_surface(
            "back",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 320.0, y: 200.0 },
        );
        let front = state.model.field.spawn_surface(
            "front",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 320.0, y: 200.0 },
        );
        for node in [back, front] {
            state.assign_node_to_monitor(node, monitor.as_str());
            let _ = state
                .model
                .field
                .set_state(node, halley_core::field::NodeState::Active);
        }

        assert!(!state.raise_overlap_policy_node(front));
        assert!(!state.ui.render_state.raise_animations.contains_key(&front));

        assert!(state.raise_overlap_policy_node(back));
        let order_after_raise = state.model.focus_state.next_overlap_raise_order;
        let started_at = state
            .ui
            .render_state
            .raise_animations
            .get(&back)
            .expect("raise animation")
            .started_at;

        assert!(!state.raise_overlap_policy_node(back));
        assert_eq!(
            state.model.focus_state.next_overlap_raise_order,
            order_after_raise
        );
        assert_eq!(
            state
                .ui
                .render_state
                .raise_animations
                .get(&back)
                .expect("raise animation")
                .started_at,
            started_at
        );
    }

    #[test]
    fn window_resize_does_not_show_focus_ring_preview() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let node = state.model.field.spawn_surface(
            "window",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 320.0, y: 200.0 },
        );
        state.input.interaction_state.resize_active = Some(node);

        assert!(!state.should_draw_focus_ring_preview(state.runtime.started_at));
    }

    #[test]
    fn focus_ring_config_resize_preview_follows_debug_config() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let mut next = state.runtime.tuning.clone();
        next.focus_ring_rx += 80.0;
        next.debug.show_ring_when_resizing = true;

        state.apply_tuning(next);

        assert!(state.should_draw_focus_ring_preview(Instant::now()));

        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let mut next = state.runtime.tuning.clone();
        next.focus_ring_rx += 80.0;
        next.debug.show_ring_when_resizing = false;

        state.apply_tuning(next);

        assert!(!state.should_draw_focus_ring_preview(Instant::now()));
    }

    #[test]
    fn disabling_focus_ring_resize_preview_clears_active_preview() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let mut next = state.runtime.tuning.clone();
        next.focus_ring_rx += 80.0;
        next.debug.show_ring_when_resizing = true;
        state.apply_tuning(next);
        assert!(state.should_draw_focus_ring_preview(Instant::now()));

        let mut next = state.runtime.tuning.clone();
        next.debug.show_ring_when_resizing = false;
        state.apply_tuning(next);

        assert!(!state.should_draw_focus_ring_preview(Instant::now()));
    }
}
