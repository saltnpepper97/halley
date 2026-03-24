use super::*;
use halley_ipc::TrailDirection;

impl Halley {
    pub(crate) fn record_focus_trail_visit(&mut self, id: NodeId) {
        if self.focus_state.focus_trail.cursor() == Some(id) {
            return;
        }
        self.focus_state.focus_trail.record(id);
        self.focus_state
            .focus_trail
            .truncate_to(self.tuning.trail_history_length);
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
        let current_focus = self.focus_state.primary_interaction_focus;
        let mut remaining = self.focus_state.focus_trail.len().max(1);
        loop {
            if remaining == 0 {
                return false;
            }
            remaining -= 1;
            let next = match direction {
                TrailDirection::Prev if self.tuning.trail_wrap => {
                    self.focus_state.focus_trail.back_wrapping()
                }
                TrailDirection::Prev => self.focus_state.focus_trail.back(),
                TrailDirection::Next if self.tuning.trail_wrap => {
                    self.focus_state.focus_trail.forward_wrapping()
                }
                TrailDirection::Next => self.focus_state.focus_trail.forward(),
            };
            let Some(id) = next else {
                return false;
            };
            if !self.should_keep_trail_node(id) {
                self.focus_state.focus_trail.forget_node(id);
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

        state.focus_state.focus_trail.record(first);
        state.focus_state.focus_trail.record(second);
        state.focus_state.focus_trail.record(first);
        state.focus_state.primary_interaction_focus = Some(first);

        assert!(state.navigate_window_trail(TrailDirection::Prev, now));
        assert_eq!(state.focus_state.primary_interaction_focus, Some(second));
    }
}
