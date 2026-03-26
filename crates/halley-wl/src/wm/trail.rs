use super::*;
use halley_core::trail::Trail;
use halley_ipc::TrailDirection;

impl Halley {
    fn trail_for_monitor_mut(&mut self, monitor: &str) -> &mut halley_core::trail::Trail {
        self.focus_state
            .focus_trail
            .entry(monitor.to_string())
            .or_insert_with(Trail::new)
    }

    pub(crate) fn record_focus_trail_visit(&mut self, id: NodeId) {
        let monitor = self
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| self.focused_monitor().to_string());
        let trail_history_length = self.tuning.trail_history_length;
        let trail = self.trail_for_monitor_mut(monitor.as_str());
        if trail.cursor() == Some(id) {
            return;
        }
        trail.record(id);
        trail.truncate_to(trail_history_length);
    }

    fn should_keep_trail_node(&self, id: NodeId) -> bool {
        self.field.node(id).is_some_and(|n| {
            self.field.is_visible(id)
                && n.kind == halley_core::field::NodeKind::Surface
                && matches!(
                    n.state,
                    halley_core::field::NodeState::Active | halley_core::field::NodeState::Node
                )
        })
    }

    fn select_trail_target(&mut self, id: NodeId, now: Instant) -> bool {
        let Some(node) = self.field.node(id).cloned() else {
            return false;
        };
        if !self.should_keep_trail_node(id) {
            return false;
        }

        self.focus_state.suppress_trail_record_once = true;
        let moved = match node.state {
            halley_core::field::NodeState::Active => {
                let restoring_suspended_fullscreen = self
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
                crate::interaction::actions::promote_node_level(self, id, now)
            }
            _ => false,
        };

        if !moved {
            self.focus_state.suppress_trail_record_once = false;
        }

        if !moved && self.field.node(id).is_some() {
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
        let trail_wrap = self.tuning.trail_wrap;
        let current_focus = self.focus_state.primary_interaction_focus;
        let mut remaining = self
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
            if self.monitor_state.node_monitor.get(&id).map(|m| m.as_str()) != Some(monitor.as_str()) {
                self.trail_for_monitor_mut(monitor.as_str()).forget_node(id);
                continue;
            }
            if Some(id) == current_focus {
                continue;
            }
            return self.select_trail_target(id, now);
        }
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

        let first = state.field.spawn_surface(
            "first",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let second = state.field.spawn_surface(
            "second",
            Vec2 { x: 640.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(first);
        state.assign_node_to_current_monitor(second);

        state.set_interaction_focus(Some(first), 30_000, now);
        state.set_interaction_focus(Some(second), 30_000, now);

        assert!(state.navigate_window_trail(TrailDirection::Prev, now));
        assert_eq!(state.focus_state.primary_interaction_focus, Some(first));

        assert!(state.navigate_window_trail(TrailDirection::Next, now));
        assert_eq!(state.focus_state.primary_interaction_focus, Some(second));
    }

    #[test]
    fn trail_navigation_skips_duplicate_current_focus_entries() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let first = state.field.spawn_surface(
            "first",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let second = state.field.spawn_surface(
            "second",
            Vec2 { x: 640.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(first);
        state.assign_node_to_current_monitor(second);

        state.trail_for_monitor_mut("default").record(first);
        state.trail_for_monitor_mut("default").record(second);
        state.trail_for_monitor_mut("default").record(first);
        state.focus_state.primary_interaction_focus = Some(first);

        assert!(state.navigate_window_trail(TrailDirection::Prev, now));
        assert_eq!(state.focus_state.primary_interaction_focus, Some(second));
    }
}
