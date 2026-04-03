use super::*;
use halley_core::decay::DecayLevel;
use halley_core::trail::Trail;
use halley_ipc::TrailDirection;
use std::ops::{Deref, DerefMut};

pub(crate) struct FocusTrailController<T> {
    st: T,
}

pub(crate) fn focus_trail_controller<T>(st: T) -> FocusTrailController<T> {
    FocusTrailController { st }
}

#[cfg(test)]
pub(crate) fn trail_for_monitor_mut<'a>(
    st: &'a mut Halley,
    monitor: &str,
) -> &'a mut halley_core::trail::Trail {
    st.model
        .focus_state
        .focus_trail
        .entry(monitor.to_string())
        .or_insert_with(Trail::new)
}

impl<T: Deref<Target = Halley>> Deref for FocusTrailController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for FocusTrailController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

impl<T: DerefMut<Target = Halley>> FocusTrailController<T> {
    pub(crate) fn trail_for_monitor_mut(
        &mut self,
        monitor: &str,
    ) -> &mut halley_core::trail::Trail {
        self.model
            .focus_state
            .focus_trail
            .entry(monitor.to_string())
            .or_insert_with(Trail::new)
    }

    pub(crate) fn record_focus_trail_visit(&mut self, id: NodeId) {
        let monitor = self
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| self.focused_monitor().to_string());
        if self
            .active_cluster_workspace_for_monitor(monitor.as_str())
            .is_some()
        {
            return;
        }
        let trail_history_length = self.runtime.tuning.trail_history_length;
        let trail = self.trail_for_monitor_mut(monitor.as_str());
        if trail.cursor() == Some(id) {
            return;
        }
        trail.record(id);
        trail.truncate_to(trail_history_length);
    }

    fn should_keep_trail_node(&self, id: NodeId) -> bool {
        self.model.field.node(id).is_some_and(|n| {
            self.model.field.is_visible(id)
                && n.kind == halley_core::field::NodeKind::Surface
                && matches!(
                    n.state,
                    halley_core::field::NodeState::Active | halley_core::field::NodeState::Node
                )
        })
    }

    fn select_trail_target(&mut self, id: NodeId, now: Instant) -> bool {
        let Some(node) = self.model.field.node(id).cloned() else {
            return false;
        };
        if !self.should_keep_trail_node(id) {
            return false;
        }

        self.model.focus_state.suppress_trail_record_once = true;
        let moved = match node.state {
            halley_core::field::NodeState::Active => {
                let restoring_suspended_fullscreen = self
                    .model
                    .fullscreen_state
                    .fullscreen_suspended_node
                    .values()
                    .any(|&nid| nid == id);
                self.set_interaction_focus(Some(id), 30_000, now);
                if restoring_suspended_fullscreen {
                    true
                } else {
                    self.animate_viewport_center_to(node.pos, now)
                }
            }
            halley_core::field::NodeState::Node => {
                crate::compositor::actions::window::promote_node_level(self, id, now)
            }
            _ => false,
        };

        if !moved {
            self.model.focus_state.suppress_trail_record_once = false;
        }

        if !moved && self.model.field.node(id).is_some() {
            self.request_maintenance();
            return true;
        }

        moved
    }

    pub(crate) fn navigate_window_trail(
        &mut self,
        direction: TrailDirection,
        now: Instant,
    ) -> bool {
        let monitor = self.focused_monitor().to_string();
        if self
            .active_cluster_workspace_for_monitor(monitor.as_str())
            .is_some()
        {
            return false;
        }
        let trail_wrap = self.runtime.tuning.trail_wrap;
        let current_focus = self.model.focus_state.primary_interaction_focus;
        let mut remaining = self
            .model
            .focus_state
            .focus_trail
            .get(monitor.as_str())
            .map(|trail| trail.len())
            .unwrap_or(0)
            .max(1);
        loop {
            if remaining == 0 {
                return false;
            }
            remaining -= 1;
            let next = {
                let trail = self.trail_for_monitor_mut(monitor.as_str());
                match direction {
                    TrailDirection::Prev if trail_wrap => trail.back_wrapping(),
                    TrailDirection::Prev => trail.back(),
                    TrailDirection::Next if trail_wrap => trail.forward_wrapping(),
                    TrailDirection::Next => trail.forward(),
                }
            };
            let Some(id) = next else {
                return false;
            };
            if !self.should_keep_trail_node(id) {
                self.trail_for_monitor_mut(monitor.as_str()).forget_node(id);
                continue;
            }
            if self
                .model
                .monitor_state
                .node_monitor
                .get(&id)
                .map(|m| m.as_str())
                != Some(monitor.as_str())
            {
                self.trail_for_monitor_mut(monitor.as_str()).forget_node(id);
                continue;
            }
            if Some(id) == current_focus {
                continue;
            }
            return self.select_trail_target(id, now);
        }
    }

    pub(crate) fn previous_window_from_trail_on_close(
        &mut self,
        monitor: &str,
        closing_id: NodeId,
    ) -> Option<NodeId> {
        if self.active_cluster_workspace_for_monitor(monitor).is_some() {
            return None;
        }
        let mut remaining = self
            .model
            .focus_state
            .focus_trail
            .get(monitor)
            .map(|trail| trail.len())
            .unwrap_or(0)
            .max(1);

        loop {
            if remaining == 0 {
                return None;
            }
            remaining -= 1;

            let next = {
                let trail = self.trail_for_monitor_mut(monitor);
                if trail.cursor() != Some(closing_id) {
                    trail.forget_node(closing_id);
                }
                trail.back()
            };

            let Some(id) = next else {
                return None;
            };
            if id == closing_id {
                continue;
            }
            if !self.should_keep_trail_node(id) {
                self.trail_for_monitor_mut(monitor).forget_node(id);
                continue;
            }
            if self
                .model
                .monitor_state
                .node_monitor
                .get(&id)
                .map(|m| m.as_str())
                != Some(monitor)
            {
                self.trail_for_monitor_mut(monitor).forget_node(id);
                continue;
            }
            return Some(id);
        }
    }

    pub(crate) fn restore_focus_to_node_after_close(
        &mut self,
        monitor: &str,
        id: NodeId,
        now: Instant,
    ) -> bool {
        if self.active_cluster_workspace_for_monitor(monitor).is_some() {
            return false;
        }
        let Some(node) = self.model.field.node(id).cloned() else {
            return false;
        };
        if !self.should_keep_trail_node(id) {
            return false;
        }

        self.model.focus_state.suppress_trail_record_once = true;
        let cluster_local = self.active_cluster_workspace_for_monitor(monitor).is_some();
        let restored = match node.state {
            halley_core::field::NodeState::Active => {
                self.set_interaction_focus(Some(id), 30_000, now);
                if !cluster_local {
                    self.maybe_pan_to_restored_focus_on_close(monitor, id, now);
                }
                true
            }
            halley_core::field::NodeState::Node => {
                self.model
                    .workspace_state
                    .manual_collapsed_nodes
                    .remove(&id);
                let _ = self.model.field.set_decay_level(id, DecayLevel::Hot);
                self.model
                    .spawn_state
                    .pending_spawn_activate_at_ms
                    .remove(&id);
                self.mark_active_transition(id, now, 360);
                self.set_interaction_focus(Some(id), 30_000, now);
                if !cluster_local {
                    self.maybe_pan_to_restored_focus_on_close(monitor, id, now);
                }
                true
            }
            _ => false,
        };

        if !restored {
            self.model.focus_state.suppress_trail_record_once = false;
        }

        restored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trail_navigation_moves_back_and_forward_without_re_recording() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let first = state.model.field.spawn_surface(
            "first",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let second = state.model.field.spawn_surface(
            "second",
            Vec2 { x: 640.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(first);
        state.assign_node_to_current_monitor(second);

        state.set_interaction_focus(Some(first), 30_000, now);
        state.set_interaction_focus(Some(second), 30_000, now);

        assert!(state.navigate_window_trail(TrailDirection::Prev, now));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(first)
        );

        assert!(state.navigate_window_trail(TrailDirection::Next, now));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(second)
        );
    }

    #[test]
    fn trail_navigation_skips_duplicate_current_focus_entries() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let first = state.model.field.spawn_surface(
            "first",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let second = state.model.field.spawn_surface(
            "second",
            Vec2 { x: 640.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(first);
        state.assign_node_to_current_monitor(second);

        state.trail_for_monitor_mut("default").record(first);
        state.trail_for_monitor_mut("default").record(second);
        state.trail_for_monitor_mut("default").record(first);
        state.model.focus_state.primary_interaction_focus = Some(first);

        assert!(state.navigate_window_trail(TrailDirection::Prev, now));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(second)
        );
    }

    #[test]
    fn close_focus_uses_previous_trail_entry() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let first = state.model.field.spawn_surface(
            "first",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let second = state.model.field.spawn_surface(
            "second",
            Vec2 { x: 640.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(first);
        state.assign_node_to_current_monitor(second);

        state.set_interaction_focus(Some(first), 30_000, now);
        state.set_interaction_focus(Some(second), 30_000, now);

        let previous = state.previous_window_from_trail_on_close("default", second);
        assert_eq!(previous, Some(first));
        assert!(state.restore_focus_to_node_after_close("default", first, now));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(first)
        );
    }
}
