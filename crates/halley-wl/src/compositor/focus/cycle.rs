use std::time::Instant;

use halley_config::FocusCycleBindingAction;
use halley_core::field::NodeId;
use halley_core::field::Vec2;
use smithay::reexports::wayland_server::Resource;

use crate::compositor::interaction::state::{FocusCycleImmersiveOrigin, FocusCycleSession};
use crate::compositor::root::Halley;

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
    st.input
        .interaction_state
        .focus_cycle_session
        .as_ref()
        .is_some_and(|session| session.closing_started_at.is_none())
}

pub(crate) fn tick_focus_cycle_session(st: &mut Halley, now: Instant) {
    const FOCUS_CYCLE_CLOSE_MS: u64 = 120;

    let Some(session) = st.input.interaction_state.focus_cycle_session.as_ref() else {
        return;
    };
    let Some(closing_started_at) = session.closing_started_at else {
        return;
    };
    if now
        .saturating_duration_since(closing_started_at)
        .as_millis() as u64
        >= FOCUS_CYCLE_CLOSE_MS
    {
        st.input.interaction_state.focus_cycle_session = None;
        st.ui.render_state.clear_focus_cycle_still();
    } else {
        st.request_maintenance();
    }
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

fn build_candidates(st: &Halley, origin_focus: Option<NodeId>) -> Vec<NodeId> {
    let mut candidates = st
        .model
        .field
        .node_ids_all()
        .into_iter()
        .filter(|&id| is_focus_cycle_candidate(st, id))
        .collect::<Vec<_>>();

    candidates.sort_by(|a, b| {
        let a_at = st
            .model
            .focus_state
            .last_surface_focus_ms
            .get(a)
            .copied()
            .unwrap_or(0);
        let b_at = st
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

fn refresh_session_candidates(st: &mut Halley, now: Instant) -> bool {
    let Some(session) = st.input.interaction_state.focus_cycle_session.as_ref() else {
        return false;
    };

    let preview_index = session.preview_index;
    let current_preview = session.candidates.get(preview_index).copied();
    let filtered = session
        .candidates
        .iter()
        .copied()
        .filter(|&id| is_focus_cycle_candidate(st, id))
        .collect::<Vec<_>>();

    let next_index = if filtered.is_empty() {
        0
    } else {
        current_preview
            .and_then(|current| filtered.iter().position(|&id| id == current))
            .unwrap_or_else(|| preview_index.min(filtered.len().saturating_sub(1)))
    };

    let Some(session) = st.input.interaction_state.focus_cycle_session.as_mut() else {
        return false;
    };
    session.candidates = filtered;

    if session.candidates.is_empty() {
        st.input.interaction_state.focus_cycle_session = None;
        st.ui.render_state.clear_focus_cycle_still();
        return false;
    }

    session.preview_index = next_index;
    session.step_from_visual_index = next_index as f32;
    session.step_to_visual_index = next_index as f32;
    session.step_started_at = now;
    true
}

fn preview_step(st: &mut Halley, direction: FocusCycleBindingAction, now: Instant) -> bool {
    if !refresh_session_candidates(st, now) {
        return false;
    }

    let Some(session) = st.input.interaction_state.focus_cycle_session.as_mut() else {
        return false;
    };
    if session.candidates.len() < 2 {
        return false;
    }
    if session.closing_started_at.is_some() {
        return false;
    }

    let len = session.candidates.len();
    let from_index = session.preview_index;
    let to_index = match direction {
        FocusCycleBindingAction::Forward => (from_index + 1) % len,
        FocusCycleBindingAction::Backward => (from_index + len - 1) % len,
    };
    let to_visual = match direction {
        FocusCycleBindingAction::Forward if from_index + 1 == len && to_index == 0 => len as f32,
        FocusCycleBindingAction::Backward if from_index == 0 && to_index + 1 == len => -1.0,
        _ => to_index as f32,
    };
    session.preview_index = to_index;
    session.step_from_visual_index = from_index as f32;
    session.step_to_visual_index = to_visual;
    session.step_started_at = now;

    let preview = session.candidates[session.preview_index];
    if session
        .immersive_origin
        .as_ref()
        .is_some_and(|origin| preview != origin.node_id)
    {
        session.immersive_lock_released = true;
    }
    let _ = session;
    st.request_maintenance();
    true
}

pub(crate) fn start_or_step_focus_cycle(
    st: &mut Halley,
    direction: FocusCycleBindingAction,
    now: Instant,
) -> bool {
    if st.input.interaction_state.focus_cycle_session.is_none() {
        let origin_focus = st.last_input_surface_node_for_monitor(st.focused_monitor());
        let candidates = build_candidates(st, origin_focus);
        if candidates.len() < 2 {
            return false;
        }

        let immersive_origin = origin_focus.and_then(|node_id| {
            if !st.is_fullscreen_active(node_id)
                || !fullscreen_origin_is_immersive_target(st, node_id)
            {
                return None;
            }
            let immersive_monitor = st.fullscreen_monitor_for_node(node_id)?;
            let space = st.model.monitor_state.monitors.get(immersive_monitor)?;
            Some(FocusCycleImmersiveOrigin {
                node_id,
                monitor: immersive_monitor.to_string(),
                saved_camera_center: space.camera_target_center,
                saved_zoom_view_size: space.camera_target_view_size,
            })
        });

        st.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
            candidates,
            preview_index: 0,
            opened_at: now,
            step_from_visual_index: 0.0,
            step_to_visual_index: 0.0,
            step_started_at: now,
            closing_started_at: None,
            origin_focus,
            immersive_origin,
            immersive_lock_released: false,
        });
    }

    preview_step(st, direction, now)
}

fn restore_origin_without_tracking(st: &mut Halley, session: &FocusCycleSession) {
    if let Some(origin) = session.origin_focus
        && session
            .immersive_origin
            .as_ref()
            .is_some_and(|immersive| immersive.node_id == origin)
        && let Some(immersive) = session.immersive_origin.as_ref()
    {
        restore_camera_snapshot(
            st,
            immersive.monitor.as_str(),
            immersive.saved_camera_center,
            immersive.saved_zoom_view_size,
        );
    }
    st.apply_wayland_focus_state(session.origin_focus);
}

pub(crate) fn cancel_focus_cycle(st: &mut Halley) -> bool {
    let Some(session) = st.input.interaction_state.focus_cycle_session.clone() else {
        return false;
    };
    if session.closing_started_at.is_some() {
        return false;
    }
    restore_origin_without_tracking(st, &session);
    if let Some(session) = st.input.interaction_state.focus_cycle_session.as_mut() {
        session.closing_started_at = Some(Instant::now());
    }
    st.request_maintenance();
    true
}

pub(crate) fn commit_focus_cycle(st: &mut Halley, now: Instant) -> bool {
    let Some(session) = st.input.interaction_state.focus_cycle_session.clone() else {
        return false;
    };
    if session.closing_started_at.is_some() {
        return false;
    }

    let target = session
        .candidates
        .get(session.preview_index)
        .copied()
        .filter(|&id| is_focus_cycle_candidate(st, id))
        .or(session
            .origin_focus
            .filter(|&id| is_focus_cycle_candidate(st, id)));

    let Some(target) = target else {
        st.apply_wayland_focus_state(None);
        if let Some(session) = st.input.interaction_state.focus_cycle_session.as_mut() {
            session.closing_started_at = Some(now);
        }
        st.request_maintenance();
        return true;
    };
    // The alt+tab prewarm captured `target` into the shared offscreen cache, and
    // the live-window prewarm never refreshes a complete cache entry. Drop it so the
    // live path rebuilds the picked window at its real geometry next frame instead of
    // reusing the switcher snapshot.
    st.ui.render_state.clear_window_offscreen_cache_for(target);
    if Some(target) == session.origin_focus {
        if let Some(immersive) = session.immersive_origin.as_ref()
            && immersive.node_id == target
        {
            restore_camera_snapshot(
                st,
                immersive.monitor.as_str(),
                immersive.saved_camera_center,
                immersive.saved_zoom_view_size,
            );
        }
        st.apply_wayland_focus_state(Some(target));
        let _ = st.raise_overlap_policy_node(target);
        crate::compositor::interaction::pointer::center_pointer_on_node(st, target, now);
        if let Some(session) = st.input.interaction_state.focus_cycle_session.as_mut() {
            session.closing_started_at = Some(now);
        }
        st.request_maintenance();
        return true;
    }

    let changed = if st
        .model
        .field
        .cluster_id_for_member_public(target)
        .is_some()
    {
        let changed =
            crate::compositor::actions::window::focus_surface_node_without_reveal(st, target, now);
        let _ = st.raise_overlap_policy_node(target);
        changed
    } else {
        let target_monitor = st.monitor_for_node_or_current(target);
        if crate::compositor::workspace::state::maximize_session_target_for_monitor(
            st,
            target_monitor.as_str(),
        ) == Some(target)
        {
            let changed = crate::compositor::actions::window::focus_surface_node_without_reveal(
                st, target, now,
            );
            let _ = st.raise_overlap_policy_node(target);
            changed
        } else if st
            .model
            .fullscreen_state
            .fullscreen_active_node
            .get(target_monitor.as_str())
            .copied()
            .filter(|&fullscreen_id| fullscreen_id != target)
            .is_some_and(|fullscreen_id| {
                !st.model
                    .fullscreen_state
                    .fullscreen_restore
                    .contains_key(&fullscreen_id)
            })
            && st.model.field.node(target).is_some_and(|node| {
                node.state == halley_core::field::NodeState::Active
                    && st.surface_is_fully_visible_on_monitor(target_monitor.as_str(), target)
            })
        {
            let _ = st.raise_overlap_policy_node(target);

            crate::compositor::actions::window::focus_surface_node_without_reveal(st, target, now)
        } else {
            crate::compositor::actions::window::focus_from_presentation_navigation(st, target, now)
                || crate::compositor::actions::window::focus_or_reveal_surface_node(st, target, now)
        }
    };
    if changed {
        crate::compositor::interaction::pointer::center_pointer_on_node(st, target, now);
    }
    if let Some(session) = st.input.interaction_state.focus_cycle_session.as_mut() {
        session.closing_started_at = Some(now);
    }
    st.request_maintenance();
    changed
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
    fn focus_cycle_steps_record_visual_animation_indices() {
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
        let session = state
            .input
            .interaction_state
            .focus_cycle_session
            .as_ref()
            .expect("session");
        assert_eq!(session.opened_at, now);
        assert_eq!(session.step_started_at, now);
        assert_eq!(session.step_from_visual_index, 0.0);
        assert_eq!(session.step_to_visual_index, 1.0);
        assert_eq!(session.preview_index, 1);

        let later = now + std::time::Duration::from_millis(8);
        assert!(state.start_or_step_focus_cycle(FocusCycleBindingAction::Forward, later));
        let session = state
            .input
            .interaction_state
            .focus_cycle_session
            .as_ref()
            .expect("session");
        assert_eq!(session.step_started_at, later);
        assert_eq!(session.step_from_visual_index, 1.0);
        assert_eq!(session.step_to_visual_index, 2.0);
        assert_eq!(session.preview_index, 0);
    }

    #[test]
    fn commit_keeps_visual_session_until_close_animation_finishes() {
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
        assert!(state.focus_cycle_session_active());
        assert!(state.commit_focus_cycle(now));

        assert!(!state.focus_cycle_session_active());
        assert!(
            state
                .input
                .interaction_state
                .focus_cycle_session
                .as_ref()
                .and_then(|session| session.closing_started_at)
                .is_some()
        );

        tick_focus_cycle_session(&mut state, now + std::time::Duration::from_millis(60));
        assert!(state.input.interaction_state.focus_cycle_session.is_some());
        tick_focus_cycle_session(&mut state, now + std::time::Duration::from_millis(140));
        assert!(state.input.interaction_state.focus_cycle_session.is_none());
    }

    #[test]
    fn focus_cycle_start_does_not_reset_keyboard_focus() {
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
        state.input.interaction_state.reset_input_state_requested = false;

        assert!(state.start_or_step_focus_cycle(FocusCycleBindingAction::Forward, now));

        assert!(state.focus_cycle_session_active());
        assert!(!state.input.interaction_state.reset_input_state_requested);
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
    fn commit_to_cluster_member_does_not_pan_to_reveal() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
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

        let origin = state.model.field.spawn_surface(
            "origin",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 160.0, y: 120.0 },
        );
        let cluster_a = state.model.field.spawn_surface(
            "cluster-a",
            Vec2 { x: 120.0, y: 0.0 },
            Vec2 { x: 160.0, y: 120.0 },
        );
        let target = state.model.field.spawn_surface(
            "target",
            Vec2 { x: 240.0, y: 0.0 },
            Vec2 { x: 160.0, y: 120.0 },
        );
        for id in [origin, cluster_a, target] {
            state.assign_node_to_monitor(id, "monitor_a");
        }
        let cid = state
            .create_cluster(vec![cluster_a, target])
            .expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");

        let now = Instant::now();
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", now));
        if let Some(node) = state.model.field.node_mut(target) {
            node.pos = Vec2 {
                x: 5_000.0,
                y: 5_000.0,
            };
        }
        let reveal_target = state
            .minimal_reveal_center_for_surface_on_monitor("monitor_a", target)
            .expect("reveal target");
        assert_ne!(reveal_target, state.model.viewport.center);

        state.set_interaction_focus(Some(origin), 30_000, now);
        state.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
            candidates: vec![origin, target],
            preview_index: 1,
            opened_at: now,
            step_from_visual_index: 1.0,
            step_to_visual_index: 1.0,
            step_started_at: now,
            closing_started_at: None,
            origin_focus: Some(origin),
            immersive_origin: None,
            immersive_lock_released: false,
        });
        assert!(state.commit_focus_cycle(now));

        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(target)
        );
        assert!(state.input.interaction_state.viewport_pan_anim.is_none());
    }

    #[test]
    fn commit_from_maximize_to_collapsed_node_centers_without_uncollapsing() {
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

        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state, maximized, now, "default",
            )
        );
        state.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
            candidates: vec![maximized, collapsed],
            preview_index: 1,
            opened_at: now,
            step_from_visual_index: 1.0,
            step_to_visual_index: 1.0,
            step_started_at: now,
            closing_started_at: None,
            origin_focus: Some(maximized),
            immersive_origin: None,
            immersive_lock_released: false,
        });

        assert!(state.commit_focus_cycle(now));
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
    fn commit_from_fullscreen_to_collapsed_node_centers_without_uncollapsing() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.animations.fullscreen.enabled = false;
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let collapsed = state.model.field.spawn_surface(
            "collapsed",
            Vec2 { x: 2_400.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(fullscreen);
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
        state.enter_xdg_fullscreen(fullscreen, None, now);
        state.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
            candidates: vec![fullscreen, collapsed],
            preview_index: 1,
            opened_at: now,
            step_from_visual_index: 1.0,
            step_to_visual_index: 1.0,
            step_started_at: now,
            closing_started_at: None,
            origin_focus: Some(fullscreen),
            immersive_origin: None,
            immersive_lock_released: false,
        });

        assert!(state.commit_focus_cycle(now));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(collapsed)
        );
        assert_eq!(
            state.model.field.node(collapsed).expect("collapsed").state,
            halley_core::field::NodeState::Node
        );
        assert!(!state.is_fullscreen_active(fullscreen));
        assert!(state.input.interaction_state.viewport_pan_anim.is_some());
    }

    #[test]
    fn commit_from_maximize_to_visible_active_window_keeps_maximize_and_raises() {
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
        let visible = state.model.field.spawn_surface(
            "visible",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(maximized);
        state.assign_node_to_current_monitor(visible);
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state, maximized, now, "default",
            )
        );
        state.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
            candidates: vec![maximized, visible],
            preview_index: 1,
            opened_at: now,
            step_from_visual_index: 1.0,
            step_to_visual_index: 1.0,
            step_started_at: now,
            closing_started_at: None,
            origin_focus: Some(maximized),
            immersive_origin: None,
            immersive_lock_released: false,
        });

        assert!(state.commit_focus_cycle(now));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(visible)
        );
        assert!(
            state
                .model
                .workspace_state
                .maximize_sessions
                .contains_key("default")
        );
        assert!(
            state.overlap_policy_stack_rank(visible) > state.overlap_policy_stack_rank(maximized)
        );
        assert!(state.input.interaction_state.viewport_pan_anim.is_none());
    }

    #[test]
    fn commit_to_maximized_target_focuses_without_panning() {
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
        let other = state.model.field.spawn_surface(
            "other",
            Vec2 { x: 2_400.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(maximized);
        state.assign_node_to_current_monitor(other);
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state, maximized, now, "default",
            )
        );
        state.set_interaction_focus(Some(other), 30_000, now);
        state.input.interaction_state.viewport_pan_anim = None;
        state.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
            candidates: vec![other, maximized],
            preview_index: 1,
            opened_at: now,
            step_from_visual_index: 1.0,
            step_to_visual_index: 1.0,
            step_started_at: now,
            closing_started_at: None,
            origin_focus: Some(other),
            immersive_origin: None,
            immersive_lock_released: false,
        });

        assert!(state.commit_focus_cycle(now));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(maximized)
        );
        assert!(
            state
                .model
                .workspace_state
                .maximize_sessions
                .contains_key("default")
        );
        assert!(state.input.interaction_state.viewport_pan_anim.is_none());
    }

    #[test]
    fn commit_from_maximize_to_offscreen_active_window_exits_and_centers() {
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
        let offscreen = state.model.field.spawn_surface(
            "offscreen",
            Vec2 { x: 2_400.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(maximized);
        state.assign_node_to_current_monitor(offscreen);
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state, maximized, now, "default",
            )
        );
        state.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
            candidates: vec![maximized, offscreen],
            preview_index: 1,
            opened_at: now,
            step_from_visual_index: 1.0,
            step_to_visual_index: 1.0,
            step_started_at: now,
            closing_started_at: None,
            origin_focus: Some(maximized),
            immersive_origin: None,
            immersive_lock_released: false,
        });

        assert!(state.commit_focus_cycle(now));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(offscreen)
        );
        assert_eq!(
            state.model.field.node(offscreen).expect("offscreen").state,
            halley_core::field::NodeState::Active
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
    fn commit_from_fullscreen_to_offscreen_active_window_soft_suspends_and_centers() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.animations.fullscreen.enabled = false;
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let offscreen = state.model.field.spawn_surface(
            "offscreen",
            Vec2 { x: 2_400.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(fullscreen);
        state.assign_node_to_current_monitor(offscreen);
        state.enter_xdg_fullscreen(fullscreen, None, now);
        state.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
            candidates: vec![fullscreen, offscreen],
            preview_index: 1,
            opened_at: now,
            step_from_visual_index: 1.0,
            step_to_visual_index: 1.0,
            step_started_at: now,
            closing_started_at: None,
            origin_focus: Some(fullscreen),
            immersive_origin: None,
            immersive_lock_released: false,
        });

        assert!(state.commit_focus_cycle(now));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(offscreen)
        );
        assert_eq!(
            state.model.field.node(offscreen).expect("offscreen").state,
            halley_core::field::NodeState::Active
        );
        assert!(!state.is_fullscreen_active(fullscreen));
        assert!(
            state
                .model
                .fullscreen_state
                .fullscreen_suspended_node
                .values()
                .any(|&node_id| node_id == fullscreen)
        );
        assert!(state.input.interaction_state.viewport_pan_anim.is_some());

        state.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
            candidates: vec![offscreen, fullscreen],
            preview_index: 1,
            opened_at: now,
            step_from_visual_index: 1.0,
            step_to_visual_index: 1.0,
            step_started_at: now,
            closing_started_at: None,
            origin_focus: Some(offscreen),
            immersive_origin: None,
            immersive_lock_released: false,
        });

        assert!(state.commit_focus_cycle(now));
        assert!(state.is_fullscreen_active(fullscreen));
        assert!(
            state
                .model
                .fullscreen_state
                .fullscreen_suspended_node
                .is_empty()
        );
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(fullscreen)
        );
    }

    #[test]
    fn commit_from_fullscreen_to_visible_active_window_keeps_fullscreen_and_raises() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.animations.fullscreen.enabled = false;
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let visible = state.model.field.spawn_surface(
            "visible",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(fullscreen);
        state.assign_node_to_current_monitor(visible);
        state.enter_xdg_fullscreen(fullscreen, None, now);
        state.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
            candidates: vec![fullscreen, visible],
            preview_index: 1,
            opened_at: now,
            step_from_visual_index: 1.0,
            step_to_visual_index: 1.0,
            step_started_at: now,
            closing_started_at: None,
            origin_focus: Some(fullscreen),
            immersive_origin: None,
            immersive_lock_released: false,
        });

        assert!(state.commit_focus_cycle(now));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(visible)
        );
        assert!(state.is_fullscreen_active(fullscreen));
        assert!(
            state
                .model
                .fullscreen_state
                .fullscreen_suspended_node
                .is_empty()
        );
        assert!(
            state.overlap_policy_stack_rank(visible) > state.overlap_policy_stack_rank(fullscreen)
        );
        assert!(state.node_draws_above_fullscreen_on_monitor(visible, "default"));
        assert!(state.input.interaction_state.viewport_pan_anim.is_none());
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
    fn same_monitor_commit_keeps_origin_fullscreen_behind_target() {
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

        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .get(current_monitor.as_str()),
            Some(&fullscreen)
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
        assert!(
            state.overlap_policy_stack_rank(other) > state.overlap_policy_stack_rank(fullscreen)
        );
        assert!(state.node_draws_above_fullscreen_on_monitor(other, current_monitor.as_str()));
    }

    #[test]
    fn same_monitor_commit_keeps_immersive_fullscreen_behind_target() {
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
        let now = Instant::now();
        state.input.interaction_state.focus_cycle_session = Some(FocusCycleSession {
            candidates: vec![fullscreen, other],
            preview_index: 1,
            opened_at: now,
            step_from_visual_index: 1.0,
            step_to_visual_index: 1.0,
            step_started_at: now,
            closing_started_at: None,
            origin_focus: Some(fullscreen),
            immersive_origin: Some(FocusCycleImmersiveOrigin {
                node_id: fullscreen,
                monitor: current_monitor.clone(),
                saved_camera_center: space.camera_target_center,
                saved_zoom_view_size: space.camera_target_view_size,
            }),
            immersive_lock_released: true,
        });

        assert!(state.commit_focus_cycle(now));

        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .get(current_monitor.as_str()),
            Some(&fullscreen)
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
        assert!(
            state.overlap_policy_stack_rank(other) > state.overlap_policy_stack_rank(fullscreen)
        );
        assert!(state.node_draws_above_fullscreen_on_monitor(other, current_monitor.as_str()));
    }
}
