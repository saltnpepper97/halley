use super::*;
use halley_api::TrailDirection;
use halley_core::decay::DecayLevel;
use halley_core::trail::Trail;

pub(crate) fn trail_for_monitor_mut<'a>(
    st: &'a mut Halley,
    monitor: &str,
) -> &'a mut halley_core::trail::Trail {
    st.model
        .focus_state
        .focus_trail
        .entry(monitor.to_string())
        .or_default()
}

pub(crate) fn record_focus_trail_visit(st: &mut Halley, id: NodeId) {
    let monitor = st
        .model
        .monitor_state
        .node_monitor
        .get(&id)
        .cloned()
        .unwrap_or_else(|| st.focused_monitor().to_string());
    if st
        .active_cluster_workspace_for_monitor(monitor.as_str())
        .is_some()
    {
        return;
    }
    let trail_history_length = st.runtime.tuning.trail_history_length;
    let trail = trail_for_monitor_mut(st, monitor.as_str());
    if trail.cursor() == Some(id) {
        return;
    }
    trail.record(id);
    trail.truncate_to(trail_history_length);
}

fn should_keep_trail_node(st: &Halley, id: NodeId) -> bool {
    st.model.field.node(id).is_some_and(|n| {
        st.model.field.is_visible(id)
            && n.kind == halley_core::field::NodeKind::Surface
            && matches!(
                n.state,
                halley_core::field::NodeState::Active | halley_core::field::NodeState::Node
            )
    })
}

fn select_trail_target(st: &mut Halley, id: NodeId, now: Instant) -> bool {
    let Some(node) = st.model.field.node(id).cloned() else {
        return false;
    };
    if !should_keep_trail_node(st, id) {
        return false;
    }

    st.model.focus_state.suppress_trail_record_once = true;
    let moved = match node.state {
        halley_core::field::NodeState::Active => {
            if crate::compositor::actions::window::focus_from_presentation_navigation(st, id, now) {
                true
            } else {
                let restoring_suspended_fullscreen = st
                    .model
                    .fullscreen_state
                    .fullscreen_suspended_node
                    .values()
                    .any(|&nid| nid == id);
                st.set_interaction_focus(Some(id), 30_000, now);
                let _ = st.raise_overlap_policy_node(id);
                if restoring_suspended_fullscreen {
                    true
                } else {
                    st.animate_viewport_center_to(node.pos, now)
                }
            }
        }
        halley_core::field::NodeState::Node => {
            if crate::compositor::actions::window::focus_from_presentation_navigation(st, id, now) {
                true
            } else {
                crate::compositor::actions::window::promote_node_level(st, id, now)
            }
        }
        _ => false,
    };

    if !moved {
        st.model.focus_state.suppress_trail_record_once = false;
    }

    if !moved && st.model.field.node(id).is_some() {
        st.request_maintenance();
        return true;
    }

    moved
}

pub(crate) fn navigate_window_trail(
    st: &mut Halley,
    direction: TrailDirection,
    now: Instant,
) -> bool {
    let monitor = st.focused_monitor().to_string();
    if st
        .active_cluster_workspace_for_monitor(monitor.as_str())
        .is_some()
    {
        return false;
    }
    let trail_wrap = st.runtime.tuning.trail_wrap;
    let current_focus = st.model.focus_state.primary_interaction_focus;
    let mut remaining = st
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
            let trail = trail_for_monitor_mut(st, monitor.as_str());
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
        if !should_keep_trail_node(st, id) {
            trail_for_monitor_mut(st, monitor.as_str()).forget_node(id);
            continue;
        }
        if st
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .map(|m| m.as_str())
            != Some(monitor.as_str())
        {
            trail_for_monitor_mut(st, monitor.as_str()).forget_node(id);
            continue;
        }
        if Some(id) == current_focus {
            continue;
        }
        return select_trail_target(st, id, now);
    }
}

pub(crate) fn previous_window_from_trail_on_close(
    st: &mut Halley,
    monitor: &str,
    closing_id: NodeId,
) -> Option<NodeId> {
    if st.active_cluster_workspace_for_monitor(monitor).is_some() {
        return None;
    }
    let mut remaining = st
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
            let trail = trail_for_monitor_mut(st, monitor);
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
        if !should_keep_trail_node(st, id) {
            trail_for_monitor_mut(st, monitor).forget_node(id);
            continue;
        }
        if st
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .map(|m| m.as_str())
            != Some(monitor)
        {
            trail_for_monitor_mut(st, monitor).forget_node(id);
            continue;
        }
        return Some(id);
    }
}

pub(crate) fn restore_focus_to_node_after_close(
    st: &mut Halley,
    monitor: &str,
    id: NodeId,
    now: Instant,
    suppress_pan: bool,
) -> bool {
    if st.active_cluster_workspace_for_monitor(monitor).is_some() {
        return false;
    }
    let Some(node) = st.model.field.node(id).cloned() else {
        return false;
    };
    if !should_keep_trail_node(st, id) {
        return false;
    }

    st.model.focus_state.suppress_trail_record_once = true;
    let cluster_local = st.active_cluster_workspace_for_monitor(monitor).is_some();
    let restored = match node.state {
        halley_core::field::NodeState::Active => {
            st.set_interaction_focus(Some(id), 30_000, now);
            if !cluster_local && !suppress_pan {
                crate::compositor::focus::system::maybe_pan_to_restored_focus_on_close(
                    st, monitor, id, now,
                );
            }
            true
        }
        halley_core::field::NodeState::Node => {
            st.model.workspace_state.manual_collapsed_nodes.remove(&id);
            let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
            st.model
                .spawn_state
                .pending_spawn_activate_at_ms
                .remove(&id);
            crate::compositor::workspace::state::mark_active_transition(st, id, now, 360);
            st.set_interaction_focus(Some(id), 30_000, now);
            if !cluster_local && !suppress_pan {
                crate::compositor::focus::system::maybe_pan_to_restored_focus_on_close(
                    st, monitor, id, now,
                );
            }
            true
        }
        _ => false,
    };

    if !restored {
        st.model.focus_state.suppress_trail_record_once = false;
    }

    restored
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::spawn::state::AppliedInitialWindowRule;
    use halley_config::{
        CloseRestorePanMode, InitialWindowClusterParticipation, InitialWindowOverlapPolicy,
        InitialWindowSpawnPlacement,
    };

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
    fn trail_navigation_raises_selected_active_window() {
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
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(first);
        state.assign_node_to_current_monitor(second);

        state.set_interaction_focus(Some(first), 30_000, now);
        state.set_interaction_focus(Some(second), 30_000, now);
        state
            .model
            .focus_state
            .overlap_raise_order
            .insert(second, 20);
        state.model.focus_state.next_overlap_raise_order = 20;

        assert!(state.navigate_window_trail(TrailDirection::Prev, now));
        assert!(state.overlap_policy_stack_rank(first) > state.overlap_policy_stack_rank(second));
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
        assert!(state.restore_focus_to_node_after_close("default", first, now, false));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(first)
        );
    }

    #[test]
    fn close_focus_overlap_policy_restore_skips_pan() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.close_restore_pan = CloseRestorePanMode::Always;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let first = state.model.field.spawn_surface(
            "first",
            Vec2 { x: 640.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(first);

        state.model.spawn_state.applied_window_rules.insert(
            first,
            AppliedInitialWindowRule {
                overlap_policy: InitialWindowOverlapPolicy::All,
                spawn_placement: InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: InitialWindowClusterParticipation::Layout,
                opacity: 1.0,
                parent_node: None,
                suppress_reveal_pan: false,
                builtin_rule: None,
            },
        );

        assert!(state.restore_focus_to_node_after_close("default", first, now, true));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(first)
        );
        assert!(state.input.interaction_state.viewport_pan_anim.is_none());
    }

    #[test]
    fn close_focus_normal_restore_still_pans() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.close_restore_pan = CloseRestorePanMode::Always;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let first = state.model.field.spawn_surface(
            "first",
            Vec2 { x: 640.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(first);

        assert!(state.restore_focus_to_node_after_close("default", first, now, false));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(first)
        );
        assert!(state.input.interaction_state.viewport_pan_anim.is_some());
    }

    #[test]
    fn close_focus_restore_skips_pan_when_maximized() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.close_restore_pan = CloseRestorePanMode::Always;
        tuning.animations.maximize.enabled = false;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let first = state.model.field.spawn_surface(
            "first",
            Vec2 { x: 640.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(first);

        crate::compositor::actions::window::toggle_node_maximize_state(
            &mut state, first, now, "default",
        );

        assert!(state.restore_focus_to_node_after_close("default", first, now, false));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(first)
        );
        assert!(state.input.interaction_state.viewport_pan_anim.is_none());
    }

    #[test]
    fn trail_navigation_raises_windows_while_maximized() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.animations.maximize.enabled = false;
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
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state, second, now, "default",
            )
        );

        assert!(state.navigate_window_trail(TrailDirection::Prev, now));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(first)
        );
        assert!(state.overlap_policy_stack_rank(first) > state.overlap_policy_stack_rank(second));
    }

    #[test]
    fn trail_navigation_from_maximize_to_collapsed_node_centers_without_uncollapsing() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.animations.maximize.enabled = false;
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let maximized = state.model.field.spawn_surface(
            "maximized",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let collapsed = state.model.field.spawn_surface(
            "collapsed",
            Vec2 { x: 2_400.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(maximized);
        state.assign_node_to_current_monitor(collapsed);
        let _ = state
            .model
            .field
            .set_state(collapsed, halley_core::field::NodeState::Node);
        state
            .model
            .workspace_state
            .manual_collapsed_nodes
            .insert(collapsed);
        state.set_interaction_focus(Some(collapsed), 30_000, now);
        state.set_interaction_focus(Some(maximized), 30_000, now);
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state, maximized, now, "default",
            )
        );

        assert!(state.navigate_window_trail(TrailDirection::Prev, now));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(collapsed)
        );
        assert_eq!(
            state.model.field.node(collapsed).expect("collapsed").state,
            halley_core::field::NodeState::Node
        );
        assert!(
            state
                .model
                .workspace_state
                .manual_collapsed_nodes
                .contains(&collapsed)
        );
        assert!(
            !state
                .model
                .workspace_state
                .maximize_sessions
                .contains_key("default")
        );
        assert!(state.input.interaction_state.viewport_pan_anim.is_some());
    }

    #[test]
    fn maximize_during_trail_pan_defers_until_pan_completes() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let now = Instant::now();

        let near = state.model.field.spawn_surface(
            "near",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let far = state.model.field.spawn_surface(
            "far",
            Vec2 { x: 2_400.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(near);
        state.assign_node_to_current_monitor(far);

        // Trail history: focus far then near, with the camera parked on `near`.
        state.set_interaction_focus(Some(far), 30_000, now);
        state.set_interaction_focus(Some(near), 30_000, now);
        state.model.viewport.center = Vec2 { x: 0.0, y: 0.0 };

        // Trail back to `far` — starts a camera pan toward far.pos.
        assert!(state.navigate_window_trail(TrailDirection::Prev, now));
        assert_eq!(state.model.focus_state.primary_interaction_focus, Some(far));
        assert!(state.input.interaction_state.viewport_pan_anim.is_some());

        // Maximize mid-pan: it must be deferred, not applied, so the pan can play out first.
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state, far, now, "default",
            )
        );
        assert!(state.input.interaction_state.viewport_pan_anim.is_some());
        assert!(state.input.interaction_state.pending_maximize.is_some());
        assert!(
            !state
                .model
                .workspace_state
                .maximize_sessions
                .contains_key("default")
        );

        // Once the pan completes, the deferred maximize runs.
        state.tick_viewport_pan_animation(state.now_ms(now) + 1_000);
        assert!(state.input.interaction_state.viewport_pan_anim.is_none());
        crate::compositor::actions::window::tick_pending_maximize(&mut state, now);
        assert!(state.input.interaction_state.pending_maximize.is_none());
        assert!(
            state
                .model
                .workspace_state
                .maximize_sessions
                .contains_key("default")
        );
    }
}
