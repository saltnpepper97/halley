use std::ops::{Deref, DerefMut};
use std::time::Instant;

use halley_config::FocusCycleBindingAction;
use halley_core::field::NodeId;
use halley_core::field::Vec2;
use smithay::reexports::wayland_server::Resource;

use crate::compositor::interaction::state::{FocusCycleImmersiveOrigin, FocusCycleSession};
use crate::compositor::root::Halley;

pub(crate) struct FocusCycleController<T> {
    st: T,
}

pub(crate) fn focus_cycle_controller<T>(st: T) -> FocusCycleController<T> {
    FocusCycleController { st }
}

impl<T: Deref<Target = Halley>> Deref for FocusCycleController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for FocusCycleController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

fn is_focus_cycle_candidate(st: &Halley, id: NodeId) -> bool {
    st.model.field.node(id).is_some_and(|node| {
        st.model.field.is_visible(id)
            && node.kind == halley_core::field::NodeKind::Surface
            && matches!(
                node.state,
                halley_core::field::NodeState::Active | halley_core::field::NodeState::Node
            )
    })
}

fn fullscreen_origin_is_immersive_target(st: &Halley, node_id: NodeId) -> bool {
    crate::compositor::interaction::pointer::active_constrained_pointer_surface(st)
        .and_then(|(surface, _)| st.model.surface_to_node.get(&surface.id()).copied())
        == Some(node_id)
}

fn restore_camera_snapshot(st: &mut Halley, monitor: &str, center: Vec2, view_size: Vec2) {
    if let Some(space) = st.model.monitor_state.monitors.get_mut(monitor) {
        space.viewport.center = center;
        space.camera_target_center = center;
        space.zoom_ref_size = view_size;
        space.camera_target_view_size = view_size;
    }
    if st.model.monitor_state.current_monitor == monitor {
        st.model.viewport.center = center;
        st.model.camera_target_center = center;
        st.model.zoom_ref_size = view_size;
        st.model.camera_target_view_size = view_size;
        st.runtime.tuning.viewport_center = center;
        st.runtime.tuning.viewport_size = view_size;
        st.input.interaction_state.viewport_pan_anim = None;
    }
    st.request_maintenance();
}

pub(crate) fn focus_cycle_session_active(st: &Halley) -> bool {
    st.input.interaction_state.focus_cycle_session.is_some()
}

#[cfg(test)]
pub(crate) fn focus_cycle_preview_node(st: &Halley) -> Option<NodeId> {
    let session = st.input.interaction_state.focus_cycle_session.as_ref()?;
    session.candidates.get(session.preview_index).copied()
}

pub(crate) fn focus_cycle_releases_fullscreen_lock_for_monitor(st: &Halley, monitor: &str) -> bool {
    st.input
        .interaction_state
        .focus_cycle_session
        .as_ref()
        .and_then(|session| {
            session.immersive_origin.as_ref().and_then(|origin| {
                (origin.monitor == monitor && session.immersive_lock_released).then_some(())
            })
        })
        .is_some()
}

impl<T: Deref<Target = Halley>> FocusCycleController<T> {
    fn build_candidates(&self, origin_focus: Option<NodeId>) -> Vec<NodeId> {
        let mut candidates = self
            .model
            .field
            .node_ids_all()
            .into_iter()
            .filter(|&id| is_focus_cycle_candidate(self, id))
            .collect::<Vec<_>>();

        candidates.sort_by(|a, b| {
            let a_at = self
                .model
                .focus_state
                .last_surface_focus_ms
                .get(a)
                .copied()
                .unwrap_or(0);
            let b_at = self
                .model
                .focus_state
                .last_surface_focus_ms
                .get(b)
                .copied()
                .unwrap_or(0);

            b_at.cmp(&a_at).then_with(|| b.as_u64().cmp(&a.as_u64()))
        });

        if let Some(origin_focus) = origin_focus
            && let Some(index) = candidates.iter().position(|&id| id == origin_focus)
        {
            let origin = candidates.remove(index);
            candidates.insert(0, origin);
        }

        candidates
    }
}

impl<T: DerefMut<Target = Halley>> FocusCycleController<T> {
    fn refresh_session_candidates(&mut self) -> bool {
        let Some(session) = self.input.interaction_state.focus_cycle_session.as_ref() else {
            return false;
        };

        let preview_index = session.preview_index;
        let current_preview = session.candidates.get(preview_index).copied();
        let filtered = session
            .candidates
            .iter()
            .copied()
            .filter(|&id| is_focus_cycle_candidate(self, id))
            .collect::<Vec<_>>();

        let next_index = if filtered.is_empty() {
            0
        } else {
            current_preview
                .and_then(|current| filtered.iter().position(|&id| id == current))
                .unwrap_or_else(|| preview_index.min(filtered.len().saturating_sub(1)))
        };

        let Some(session) = self.input.interaction_state.focus_cycle_session.as_mut() else {
            return false;
        };
        session.candidates = filtered;

        if session.candidates.is_empty() {
            self.input.interaction_state.focus_cycle_session = None;
            return false;
        }

        session.preview_index = next_index;
        true
    }

    fn preview_step(&mut self, direction: FocusCycleBindingAction) -> bool {
        if !self.refresh_session_candidates() {
            return false;
        }

        let Some(session) = self.input.interaction_state.focus_cycle_session.as_mut() else {
            return false;
        };
        if session.candidates.len() < 2 {
            return false;
        }

        let len = session.candidates.len();
        session.preview_index = match direction {
            FocusCycleBindingAction::Forward => (session.preview_index + 1) % len,
            FocusCycleBindingAction::Backward => (session.preview_index + len - 1) % len,
        };

        let preview = session.candidates[session.preview_index];
        if session
            .immersive_origin
            .as_ref()
            .is_some_and(|origin| preview != origin.node_id)
        {
            session.immersive_lock_released = true;
        }
        let _ = session;
        self.request_maintenance();
        true
    }

    pub(crate) fn start_or_step_focus_cycle(
        &mut self,
        direction: FocusCycleBindingAction,
        _now: Instant,
    ) -> bool {
        if self.input.interaction_state.focus_cycle_session.is_none() {
            let origin_focus = self.last_input_surface_node_for_monitor(self.focused_monitor());
            let candidates = self.build_candidates(origin_focus);
            if candidates.len() < 2 {
                return false;
            }

            let immersive_origin = origin_focus.and_then(|node_id| {
                if !self.is_fullscreen_active(node_id)
                    || !fullscreen_origin_is_immersive_target(self, node_id)
                {
                    return None;
                }
                let immersive_monitor = self.fullscreen_monitor_for_node(node_id)?;
                let space = self.model.monitor_state.monitors.get(immersive_monitor)?;
                Some(FocusCycleImmersiveOrigin {
                    node_id,
                    monitor: immersive_monitor.to_string(),
                    saved_camera_center: space.camera_target_center,
                    saved_zoom_view_size: space.camera_target_view_size,
                })
            });

            self.begin_modal_keyboard_capture();
            self.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
                candidates,
                preview_index: 0,
                origin_focus,
                immersive_origin,
                immersive_lock_released: false,
            });
        }

        self.preview_step(direction)
    }

    fn restore_origin_without_tracking(&mut self, session: &FocusCycleSession) {
        if let Some(origin) = session.origin_focus
            && session
                .immersive_origin
                .as_ref()
                .is_some_and(|immersive| immersive.node_id == origin)
            && let Some(immersive) = session.immersive_origin.as_ref()
        {
            restore_camera_snapshot(
                self,
                immersive.monitor.as_str(),
                immersive.saved_camera_center,
                immersive.saved_zoom_view_size,
            );
        }
        self.apply_wayland_focus_state(session.origin_focus);
    }

    pub(crate) fn cancel_focus_cycle(&mut self) -> bool {
        let Some(session) = self.input.interaction_state.focus_cycle_session.take() else {
            return false;
        };
        self.restore_origin_without_tracking(&session);
        true
    }

    pub(crate) fn commit_focus_cycle(&mut self, now: Instant) -> bool {
        let Some(session) = self.input.interaction_state.focus_cycle_session.take() else {
            return false;
        };

        let target = session
            .candidates
            .get(session.preview_index)
            .copied()
            .filter(|&id| is_focus_cycle_candidate(self, id))
            .or(session
                .origin_focus
                .filter(|&id| is_focus_cycle_candidate(self, id)));

        let Some(target) = target else {
            self.apply_wayland_focus_state(None);
            return true;
        };
        let target_monitor = self.monitor_for_node_or_current(target);
        let origin_fullscreen = session.origin_focus.and_then(|node_id| {
            self.is_fullscreen_active(node_id)
                .then_some((node_id, self.monitor_for_node_or_current(node_id)))
        });

        if Some(target) == session.origin_focus {
            if let Some(immersive) = session.immersive_origin.as_ref()
                && immersive.node_id == target
            {
                restore_camera_snapshot(
                    self,
                    immersive.monitor.as_str(),
                    immersive.saved_camera_center,
                    immersive.saved_zoom_view_size,
                );
            }
            self.apply_wayland_focus_state(Some(target));
            crate::compositor::interaction::pointer::center_pointer_on_node(self, target, now);
            return true;
        }

        if let Some(immersive) = session.immersive_origin.as_ref()
            && self.is_fullscreen_active(immersive.node_id)
            && immersive.monitor == target_monitor
        {
            self.suspend_xdg_fullscreen(immersive.node_id, now);
        } else if let Some((origin_id, origin_monitor)) = origin_fullscreen
            && origin_monitor == target_monitor
        {
            self.exit_xdg_fullscreen(origin_id, now);
        }

        let changed =
            crate::compositor::actions::window::focus_or_reveal_surface_node(self, target, now);
        if changed {
            crate::compositor::interaction::pointer::center_pointer_on_node(self, target, now);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_does_not_mutate_focus_or_focus_timestamps() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());

        let a = state.model.field.spawn_surface(
            "a",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        let b = state.model.field.spawn_surface(
            "b",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_current_monitor(a);
        state.assign_node_to_current_monitor(b);

        let now = Instant::now();
        state.set_interaction_focus(Some(a), 30_000, now);
        let a_before = state
            .model
            .focus_state
            .last_surface_focus_ms
            .get(&a)
            .copied()
            .unwrap_or(0);
        let b_before = state
            .model
            .focus_state
            .last_surface_focus_ms
            .get(&b)
            .copied()
            .unwrap_or(0);
        let trail_before_cursor = state
            .model
            .focus_state
            .focus_trail
            .get(state.focused_monitor())
            .and_then(|trail| trail.cursor());
        let trail_before_len = state
            .model
            .focus_state
            .focus_trail
            .get(state.focused_monitor())
            .map(|trail| trail.len())
            .unwrap_or(0);

        assert!(state.start_or_step_focus_cycle(FocusCycleBindingAction::Forward, now));
        assert_eq!(state.model.focus_state.primary_interaction_focus, Some(a));
        assert_eq!(
            state
                .model
                .focus_state
                .last_surface_focus_ms
                .get(&a)
                .copied()
                .unwrap_or(0),
            a_before
        );
        assert_eq!(
            state
                .model
                .focus_state
                .last_surface_focus_ms
                .get(&b)
                .copied()
                .unwrap_or(0),
            b_before
        );
        assert_eq!(
            state
                .model
                .focus_state
                .focus_trail
                .get(state.focused_monitor())
                .and_then(|trail| trail.cursor()),
            trail_before_cursor
        );
        assert_eq!(
            state
                .model
                .focus_state
                .focus_trail
                .get(state.focused_monitor())
                .map(|trail| trail.len())
                .unwrap_or(0),
            trail_before_len
        );
        assert_eq!(state.focus_cycle_preview_node(), Some(b));
    }

    #[test]
    fn cancel_restores_wayland_focus_without_changing_interaction_focus() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());

        let a = state.model.field.spawn_surface(
            "a",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        let b = state.model.field.spawn_surface(
            "b",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_current_monitor(a);
        state.assign_node_to_current_monitor(b);

        let now = Instant::now();
        state.set_interaction_focus(Some(a), 30_000, now);
        assert!(state.start_or_step_focus_cycle(FocusCycleBindingAction::Forward, now));
        assert!(state.cancel_focus_cycle());
        assert_eq!(state.model.focus_state.primary_interaction_focus, Some(a));
        assert!(!state.focus_cycle_session_active());
    }

    #[test]
    fn cycle_candidates_include_visible_windows_on_other_monitors() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());

        let left = state.model.field.spawn_surface(
            "left",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        let right = state.model.field.spawn_surface(
            "right",
            Vec2 { x: 180.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(left, "left");
        state.assign_node_to_monitor(right, "right");

        let now = Instant::now();
        state.set_interaction_focus(Some(left), 30_000, now);

        assert!(state.start_or_step_focus_cycle(FocusCycleBindingAction::Forward, now));
        let session = state
            .input
            .interaction_state
            .focus_cycle_session
            .as_ref()
            .expect("focus cycle session");
        assert!(session.candidates.contains(&left));
        assert!(session.candidates.contains(&right));
    }

    #[test]
    fn cross_monitor_commit_keeps_origin_fullscreen_active() {
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
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert("left".to_string(), fullscreen_left);

        let now = Instant::now();
        state.set_interaction_focus(Some(fullscreen_left), 30_000, now);
        assert!(state.start_or_step_focus_cycle(FocusCycleBindingAction::Forward, now));
        assert!(state.commit_focus_cycle(now));

        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .get("left"),
            Some(&fullscreen_left)
        );
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(right)
        );
    }

    #[test]
    fn same_monitor_commit_exits_origin_fullscreen_for_normal_fullscreen() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        let other = state.model.field.spawn_surface(
            "other",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_current_monitor(fullscreen);
        state.assign_node_to_current_monitor(other);
        let current_monitor = state.focused_monitor().to_string();
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert(current_monitor.clone(), fullscreen);

        let now = Instant::now();
        state.set_interaction_focus(Some(fullscreen), 30_000, now);
        assert!(state.start_or_step_focus_cycle(FocusCycleBindingAction::Forward, now));
        assert!(state.commit_focus_cycle(now));

        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .contains_key(current_monitor.as_str())
        );
        assert!(
            state
                .model
                .fullscreen_state
                .fullscreen_suspended_node
                .is_empty()
        );
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(other)
        );
    }

    #[test]
    fn same_monitor_commit_suspends_origin_fullscreen_for_immersive_session() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        let other = state.model.field.spawn_surface(
            "other",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_current_monitor(fullscreen);
        state.assign_node_to_current_monitor(other);
        let current_monitor = state.focused_monitor().to_string();
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert(current_monitor.clone(), fullscreen);

        let space = state
            .model
            .monitor_state
            .monitors
            .get(current_monitor.as_str())
            .expect("monitor")
            .clone();
        state.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
            candidates: vec![fullscreen, other],
            preview_index: 1,
            origin_focus: Some(fullscreen),
            immersive_origin: Some(FocusCycleImmersiveOrigin {
                node_id: fullscreen,
                monitor: current_monitor.clone(),
                saved_camera_center: space.camera_target_center,
                saved_zoom_view_size: space.camera_target_view_size,
            }),
            immersive_lock_released: true,
        });

        let now = Instant::now();
        assert!(state.commit_focus_cycle(now));

        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .contains_key(current_monitor.as_str())
        );
        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_suspended_node
                .get(current_monitor.as_str()),
            Some(&fullscreen)
        );
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(other)
        );
    }
}
