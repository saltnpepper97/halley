use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_core::field::{NodeId, NodeKind, NodeState, Vec2};

use crate::compositor::root::Halley;

pub(crate) struct WorkspaceState {
    pub(crate) last_active_size: HashMap<NodeId, Vec2>,
    pub(crate) active_transition_until_ms: HashMap<NodeId, u64>,
    pub(crate) primary_promote_cooldown_until_ms: HashMap<NodeId, u64>,
    pub(crate) manual_collapsed_nodes: HashSet<NodeId>,
    pub(crate) pending_manual_collapses: HashMap<NodeId, u64>,
    pub(crate) user_pinned_nodes: HashSet<NodeId>,
    pub(crate) maximize_sessions: HashMap<String, MaximizeSession>,
    pub(crate) maximize_animation: HashMap<NodeId, MaximizeAnimation>,
    pub(crate) maximize_resume: HashMap<NodeId, String>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MaximizeNodeSnapshot {
    pub(crate) pos: Vec2,
    pub(crate) size: Vec2,
    pub(crate) pinned: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MaximizeCameraSnapshot {
    pub(crate) center: Vec2,
    pub(crate) view_size: Vec2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MaximizeSessionState {
    Active,
    Restoring,
    SpawnRestoring,
}

#[derive(Clone, Debug)]
pub(crate) struct MaximizeSession {
    pub(crate) target_id: NodeId,
    pub(crate) node_snapshots: HashMap<NodeId, MaximizeNodeSnapshot>,
    pub(crate) camera: MaximizeCameraSnapshot,
    pub(crate) state: MaximizeSessionState,
}

#[derive(Clone, Debug)]
pub(crate) struct MaximizeAnimation {
    pub(crate) monitor: String,
    pub(crate) from_pos: Vec2,
    pub(crate) to_pos: Vec2,
    pub(crate) from_size: Vec2,
    pub(crate) to_size: Vec2,
    pub(crate) start_ms: u64,
    pub(crate) duration_ms: u64,
}

const PENDING_MANUAL_COLLAPSE_MAX_WAIT_MS: u64 = 120;

pub fn mark_active_transition(st: &mut Halley, id: NodeId, now: Instant, duration_ms: u64) {
    if !st.runtime.tuning.animations_enabled() {
        return;
    }
    st.model
        .workspace_state
        .active_transition_until_ms
        .insert(id, st.now_ms(now).saturating_add(duration_ms.max(1)));
    st.request_maintenance();
}

pub fn active_transition_alpha(st: &Halley, id: NodeId, now: Instant) -> f32 {
    if !st.runtime.tuning.animations_enabled() {
        return 0.0;
    }
    let now_ms = st.now_ms(now);
    if st.input.interaction_state.resize_active == Some(id)
        || (st.input.interaction_state.resize_static_node == Some(id)
            && now_ms < st.input.interaction_state.resize_static_until_ms)
    {
        return 0.0;
    }
    let Some(&until) = st.model.workspace_state.active_transition_until_ms.get(&id) else {
        return 0.0;
    };
    if now_ms >= until {
        return 0.0;
    }
    let total = 420.0f32;
    let remaining = (until.saturating_sub(now_ms)) as f32;
    (remaining / total).clamp(0.0, 1.0)
}

pub(crate) fn start_active_to_node_close_animation(
    st: &mut Halley,
    id: NodeId,
    now: Instant,
) -> bool {
    if st.is_fullscreen_session_node(id) {
        return false;
    }
    if !st.runtime.tuning.window_close_animation_enabled() {
        return false;
    }
    let Some(node) = st.model.field.node(id) else {
        return false;
    };
    if node.kind != NodeKind::Surface || node.state != NodeState::Active {
        return false;
    }
    let Some(monitor) = st.model.monitor_state.node_monitor.get(&id).cloned() else {
        return false;
    };
    let duration_ms = st.runtime.tuning.window_close_duration_ms();
    let style = st.runtime.tuning.window_close_style();
    let Some((border_rects, offscreen_textures)) =
        crate::window::capture_closing_window_animation(st, monitor.as_str(), id)
    else {
        return false;
    };

    st.ui.render_state.start_closing_window_animation(
        id,
        monitor.as_str(),
        now,
        duration_ms,
        style,
        border_rects,
        offscreen_textures,
    );
    st.ui
        .render_state
        .animator
        .snap_to_state(id, NodeState::Node, now);
    st.request_maintenance();
    true
}

pub(crate) fn queue_pending_manual_collapse(st: &mut Halley, id: NodeId, now: Instant) {
    if st.is_fullscreen_session_node(id) {
        return;
    }
    let now_ms = st.now_ms(now);
    st.model
        .workspace_state
        .pending_manual_collapses
        .entry(id)
        .or_insert(now_ms);
    st.request_maintenance();
}

pub(crate) fn finish_manual_collapse(st: &mut Halley, id: NodeId, now: Instant) -> bool {
    st.model
        .workspace_state
        .pending_manual_collapses
        .remove(&id);
    if st.is_fullscreen_session_node(id) {
        return false;
    }
    let _ = st.model.field.set_state(id, NodeState::Node);
    let _ = st
        .model
        .field
        .set_decay_level(id, halley_core::decay::DecayLevel::Cold);
    st.model
        .spawn_state
        .pending_spawn_activate_at_ms
        .remove(&id);
    st.model.workspace_state.manual_collapsed_nodes.insert(id);

    if st.model.focus_state.primary_interaction_focus == Some(id) {
        st.set_interaction_focus(None, 0, now);
    }
    if st.model.focus_state.pan_restore_active_focus == Some(id) {
        st.model.focus_state.pan_restore_active_focus = None;
    }
    true
}

pub(crate) fn process_pending_manual_collapses_for_monitor(
    st: &mut Halley,
    monitor: &str,
    now: Instant,
) {
    if st.model.workspace_state.pending_manual_collapses.is_empty() {
        return;
    }

    let now_ms = st.now_ms(now);
    let pending = st
        .model
        .workspace_state
        .pending_manual_collapses
        .iter()
        .map(|(&id, &requested_at_ms)| (id, requested_at_ms))
        .collect::<Vec<_>>();

    let mut needs_retry = false;
    for (id, requested_at_ms) in pending {
        let Some(node) = st.model.field.node(id) else {
            st.model
                .workspace_state
                .pending_manual_collapses
                .remove(&id);
            continue;
        };
        if st
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .is_some_and(|node_monitor| node_monitor != monitor)
        {
            continue;
        }
        if node.kind != NodeKind::Surface
            || node.state != NodeState::Active
            || !st.model.field.is_visible(id)
        {
            st.model
                .workspace_state
                .pending_manual_collapses
                .remove(&id);
            continue;
        }

        if start_active_to_node_close_animation(st, id, now)
            || now_ms.saturating_sub(requested_at_ms) >= PENDING_MANUAL_COLLAPSE_MAX_WAIT_MS
        {
            let _ = finish_manual_collapse(st, id, now);
        } else {
            needs_retry = true;
        }
    }

    if needs_retry {
        st.request_maintenance();
    }
}

pub(crate) fn preserve_collapsed_surface(st: &Halley, id: NodeId) -> bool {
    st.model
        .workspace_state
        .manual_collapsed_nodes
        .contains(&id)
        || st.model.field.node(id).is_some_and(|n| {
            n.kind == halley_core::field::NodeKind::Surface
                && n.state == halley_core::field::NodeState::Node
        })
}

pub(crate) fn maximize_animation_active(st: &Halley) -> bool {
    !st.model.workspace_state.maximize_animation.is_empty()
        || st
            .model
            .workspace_state
            .maximize_sessions
            .values()
            .any(|session| {
                matches!(
                    session.state,
                    MaximizeSessionState::Restoring | MaximizeSessionState::SpawnRestoring
                )
            })
}

pub(crate) fn maximize_session_active_on_monitor(st: &Halley, monitor: &str) -> bool {
    st.model
        .workspace_state
        .maximize_sessions
        .get(monitor)
        .is_some_and(|session| session.state == MaximizeSessionState::Active)
}

pub(crate) fn maximize_session_target_for_monitor(st: &Halley, monitor: &str) -> Option<NodeId> {
    st.model
        .workspace_state
        .maximize_sessions
        .get(monitor)
        .map(|session| session.target_id)
}

pub(crate) fn maximize_session_for_monitor<'a>(
    st: &'a Halley,
    monitor: &str,
) -> Option<&'a MaximizeSession> {
    st.model.workspace_state.maximize_sessions.get(monitor)
}

pub(crate) fn node_in_maximize_session(st: &Halley, node_id: NodeId) -> bool {
    st.model
        .workspace_state
        .maximize_sessions
        .values()
        .any(|session| session.node_snapshots.contains_key(&node_id))
}

pub(crate) fn set_maximize_resume_for_node(st: &mut Halley, node_id: NodeId, monitor: &str) {
    st.model
        .workspace_state
        .maximize_resume
        .insert(node_id, monitor.to_string());
}

pub(crate) fn take_maximize_resume_for_node(st: &mut Halley, node_id: NodeId) -> Option<String> {
    st.model.workspace_state.maximize_resume.remove(&node_id)
}

pub(crate) fn clear_maximize_resume_for_node(st: &mut Halley, node_id: NodeId) {
    st.model.workspace_state.maximize_resume.remove(&node_id);
}

pub(crate) fn snapshot_monitor_camera(st: &Halley, monitor: &str) -> MaximizeCameraSnapshot {
    if st.model.monitor_state.current_monitor == monitor {
        MaximizeCameraSnapshot {
            center: st.model.viewport.center,
            view_size: st.model.zoom_ref_size,
        }
    } else {
        st.model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| MaximizeCameraSnapshot {
                center: space.viewport.center,
                view_size: space.zoom_ref_size,
            })
            .unwrap_or(MaximizeCameraSnapshot {
                center: st.model.viewport.center,
                view_size: st.model.zoom_ref_size,
            })
    }
}

pub(crate) fn apply_monitor_camera_snapshot(
    st: &mut Halley,
    monitor: &str,
    snapshot: MaximizeCameraSnapshot,
) {
    if st.model.monitor_state.current_monitor == monitor {
        st.model.viewport.center = snapshot.center;
        st.model.zoom_ref_size = snapshot.view_size;
        st.model.camera_target_center = snapshot.center;
        st.model.camera_target_view_size = snapshot.view_size;
        st.runtime.tuning.viewport_center = snapshot.center;
        st.runtime.tuning.viewport_size = snapshot.view_size;
    } else if let Some(space) = st.model.monitor_state.monitors.get_mut(monitor) {
        space.viewport.center = snapshot.center;
        space.zoom_ref_size = snapshot.view_size;
        space.camera_target_center = snapshot.center;
        space.camera_target_view_size = snapshot.view_size;
    }
}

pub(crate) fn set_monitor_camera_target_snapshot(
    st: &mut Halley,
    monitor: &str,
    snapshot: MaximizeCameraSnapshot,
) {
    if st.model.monitor_state.current_monitor == monitor {
        st.model.camera_target_center = snapshot.center;
        st.model.camera_target_view_size = snapshot.view_size;
        st.request_maintenance();
    } else if let Some(space) = st.model.monitor_state.monitors.get_mut(monitor) {
        space.camera_target_center = snapshot.center;
        space.camera_target_view_size = snapshot.view_size;
    }
}

pub(crate) fn reset_monitor_zoom_for_maximize(st: &mut Halley, monitor: &str) {
    if st.model.monitor_state.current_monitor == monitor {
        let center = st.model.viewport.center;
        let base_view_size = st.model.viewport.size;
        set_monitor_camera_target_snapshot(
            st,
            monitor,
            MaximizeCameraSnapshot {
                center,
                view_size: base_view_size,
            },
        );
    } else if let Some(space) = st.model.monitor_state.monitors.get(monitor) {
        set_monitor_camera_target_snapshot(
            st,
            monitor,
            MaximizeCameraSnapshot {
                center: space.viewport.center,
                view_size: space.viewport.size,
            },
        );
    }
}

pub(crate) fn monitor_camera_matches_snapshot(
    st: &Halley,
    monitor: &str,
    snapshot: MaximizeCameraSnapshot,
) -> bool {
    if st.model.monitor_state.current_monitor == monitor {
        (st.model.viewport.center.x - snapshot.center.x).abs() < 0.15
            && (st.model.viewport.center.y - snapshot.center.y).abs() < 0.15
            && (st.model.zoom_ref_size.x - snapshot.view_size.x).abs() < 0.15
            && (st.model.zoom_ref_size.y - snapshot.view_size.y).abs() < 0.15
    } else {
        st.model
            .monitor_state
            .monitors
            .get(monitor)
            .is_some_and(|space| {
                (space.camera_target_center.x - snapshot.center.x).abs() < 0.15
                    && (space.camera_target_center.y - snapshot.center.y).abs() < 0.15
                    && (space.camera_target_view_size.x - snapshot.view_size.x).abs() < 0.15
                    && (space.camera_target_view_size.y - snapshot.view_size.y).abs() < 0.15
            })
    }
}

pub(crate) fn abort_maximize_session_for_monitor(st: &mut Halley, monitor: &str) -> bool {
    let Some(session) = st.model.workspace_state.maximize_sessions.remove(monitor) else {
        return false;
    };

    apply_monitor_camera_snapshot(st, monitor, session.camera);

    let mut restored_any = false;
    for (id, snapshot) in session.node_snapshots {
        st.model.workspace_state.maximize_animation.remove(&id);

        let Some(node) = st.model.field.node_mut(id) else {
            continue;
        };
        node.pos = snapshot.pos;
        node.intrinsic_size = snapshot.size;
        restored_any = true;
        let _ = st.model.field.sync_active_footprint_to_intrinsic(id);
        let _ = st.model.field.set_pinned(id, snapshot.pinned);
        st.request_toplevel_resize(
            id,
            snapshot.size.x.round() as i32,
            snapshot.size.y.round() as i32,
        );
        st.set_last_active_size_now(id, snapshot.size);
    }

    if restored_any {
        st.resolve_overlap_now();
    }
    true
}

pub(crate) fn abort_maximize_session_for_node(st: &mut Halley, id: NodeId) -> bool {
    let monitor =
        st.model
            .workspace_state
            .maximize_sessions
            .iter()
            .find_map(|(monitor, session)| {
                session
                    .node_snapshots
                    .contains_key(&id)
                    .then(|| monitor.clone())
            });
    monitor
        .as_deref()
        .is_some_and(|monitor| abort_maximize_session_for_monitor(st, monitor))
}

pub(crate) fn tick_maximize_animation(st: &mut Halley, now: Instant) {
    let now_ms = st.now_ms(now);
    let animations = st
        .model
        .workspace_state
        .maximize_animation
        .iter()
        .map(|(&id, anim)| (id, anim.clone()))
        .collect::<Vec<_>>();
    let mut finished = Vec::new();

    for (id, anim) in animations {
        let elapsed = now_ms.saturating_sub(anim.start_ms);
        let t = (elapsed as f32 / anim.duration_ms.max(1) as f32).clamp(0.0, 1.0);
        let e = if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
        };
        let pos = Vec2 {
            x: anim.from_pos.x + (anim.to_pos.x - anim.from_pos.x) * e,
            y: anim.from_pos.y + (anim.to_pos.y - anim.from_pos.y) * e,
        };
        let size = Vec2 {
            x: (anim.from_size.x + (anim.to_size.x - anim.from_size.x) * e).max(96.0),
            y: (anim.from_size.y + (anim.to_size.y - anim.from_size.y) * e).max(72.0),
        };

        if let Some(node) = st.model.field.node_mut(id) {
            node.pos = pos;
        }
        let _ = st.model.field.set_resize_footprint(id, Some(size));
        st.request_toplevel_resize(id, size.x.round() as i32, size.y.round() as i32);
        st.input
            .interaction_state
            .physics_velocity
            .insert(id, Vec2 { x: 0.0, y: 0.0 });

        if t >= 1.0 {
            finished.push((id, anim));
        }
    }

    let had_finished = !finished.is_empty();
    for (id, anim) in finished {
        st.model.workspace_state.maximize_animation.remove(&id);
        if let Some(node) = st.model.field.node_mut(id) {
            node.pos = anim.to_pos;
            node.intrinsic_size = anim.to_size;
        }
        let _ = st.model.field.sync_active_footprint_to_intrinsic(id);
        st.set_last_active_size_now(id, anim.to_size);

        if let Some(session) = st
            .model
            .workspace_state
            .maximize_sessions
            .get(anim.monitor.as_str())
            && let Some(snapshot) = session.node_snapshots.get(&id).copied()
        {
            let pinned = match session.state {
                MaximizeSessionState::Active => true,
                MaximizeSessionState::Restoring | MaximizeSessionState::SpawnRestoring => {
                    snapshot.pinned
                }
            };
            let _ = st.model.field.set_pinned(id, pinned);
        }
    }

    let sessions_to_remove =
        st.model
            .workspace_state
            .maximize_sessions
            .iter()
            .filter_map(|(monitor, session)| {
                ((session.state == MaximizeSessionState::Restoring
                    && session
                        .node_snapshots
                        .keys()
                        .all(|id| !st.model.workspace_state.maximize_animation.contains_key(id))
                    && monitor_camera_matches_snapshot(st, monitor, session.camera))
                    || (session.state == MaximizeSessionState::SpawnRestoring
                        && session.node_snapshots.keys().all(|id| {
                            !st.model.workspace_state.maximize_animation.contains_key(id)
                        })))
                .then(|| monitor.clone())
            })
            .collect::<Vec<_>>();
    for monitor in sessions_to_remove {
        st.model.workspace_state.maximize_sessions.remove(&monitor);
    }

    if had_finished {
        st.resolve_overlap_now();
    }
}
