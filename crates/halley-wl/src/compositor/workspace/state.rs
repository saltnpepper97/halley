use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use halley_core::field::{NodeId, NodeKind, NodeState, Vec2};

use crate::compositor::root::Halley;

pub(crate) struct WorkspaceState {
    pub(crate) last_active_size: HashMap<NodeId, Vec2>,
    pub(crate) active_transitions: HashMap<NodeId, ActiveTransition>,
    pub(crate) primary_promote_cooldown_until_ms: HashMap<NodeId, u64>,
    pub(crate) manual_collapsed_nodes: HashSet<NodeId>,
    pub(crate) pending_collapses: HashMap<NodeId, PendingCollapse>,
    pub(crate) pending_silent_close_until_ms: HashMap<NodeId, u64>,
    pub(crate) user_pinned_nodes: HashSet<NodeId>,
    pub(crate) maximize_sessions: HashMap<String, MaximizeSession>,
    pub(crate) maximize_animation: HashMap<NodeId, MaximizeAnimation>,
    pub(crate) maximize_resume: HashMap<NodeId, String>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ActiveTransition {
    pub(crate) started_at_ms: u64,
    pub(crate) duration_ms: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingCollapse {
    pub(crate) requested_at_ms: u64,
    // Retry capture without losing where the active window originally collapsed from.
    pub(crate) origin_pos: Vec2,
    pub(crate) preserve_manual: bool,
}

impl ActiveTransition {
    pub(crate) fn until_ms(self) -> u64 {
        self.started_at_ms.saturating_add(self.duration_ms.max(1))
    }

    pub(crate) fn is_active(self, now_ms: u64) -> bool {
        now_ms < self.until_ms()
    }
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

const PENDING_COLLAPSE_MAX_WAIT_MS: u64 = 120;

pub fn mark_active_transition(st: &mut Halley, id: NodeId, now: Instant, duration_ms: u64) {
    if !st.runtime.tuning.animations_enabled() {
        return;
    }
    st.model.workspace_state.active_transitions.insert(
        id,
        ActiveTransition {
            started_at_ms: st.now_ms(now),
            duration_ms: duration_ms.max(1),
        },
    );
    st.request_window_animation_prewarm(id, now);
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
    let Some(&transition) = st.model.workspace_state.active_transitions.get(&id) else {
        return 0.0;
    };
    let until = transition.until_ms();
    if now_ms >= until {
        return 0.0;
    }
    if now_ms <= transition.started_at_ms {
        return 1.0;
    }
    let total = transition.duration_ms.max(1) as f32;
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
    st.request_window_animation_prewarm(id, now);
    let duration_ms = st.runtime.tuning.window_close_duration_ms();
    let style = st.runtime.tuning.window_close_style();
    // Close-to-node animation reuses the already-warmed offscreen cache. A first
    // collapse can race that cache; callers should queue a pending collapse then.
    let Some((border_rects, offscreen_textures, start_scale, start_alpha)) =
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
        start_scale,
        start_alpha,
    );
    st.ui.render_state.finish_window_animation_prewarm(id);
    st.ui
        .render_state
        .animator
        .snap_to_state(id, NodeState::Node, now);
    st.request_maintenance();
    true
}

fn queue_pending_collapse(st: &mut Halley, id: NodeId, now: Instant, preserve_manual: bool) {
    if st.is_fullscreen_session_node(id) {
        return;
    }
    let now_ms = st.now_ms(now);
    let origin_pos = st
        .model
        .field
        .node(id)
        .map(|node| node.pos)
        .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
    st.model
        .workspace_state
        .pending_collapses
        .entry(id)
        .or_insert(PendingCollapse {
            requested_at_ms: now_ms,
            origin_pos,
            preserve_manual,
        });
    st.request_window_animation_prewarm(id, now);
}

pub(crate) fn queue_pending_manual_collapse(st: &mut Halley, id: NodeId, now: Instant) {
    queue_pending_collapse(st, id, now, true);
}

pub(crate) fn queue_pending_auto_collapse(st: &mut Halley, id: NodeId, now: Instant) {
    queue_pending_collapse(st, id, now, false);
}

pub(crate) fn collapse_active_to_node_or_queue_auto(st: &mut Halley, id: NodeId, now: Instant) {
    if !st.runtime.tuning.window_close_animation_enabled() {
        let _ = finish_auto_collapse(st, id, now);
        return;
    }

    if start_active_to_node_close_animation(st, id, now) {
        let _ = finish_auto_collapse(st, id, now);
    } else {
        queue_pending_auto_collapse(st, id, now);
    }
}

pub(crate) fn finish_manual_collapse(st: &mut Halley, id: NodeId, now: Instant) -> bool {
    let pending = st.model.workspace_state.pending_collapses.remove(&id);
    if st.is_fullscreen_session_node(id) {
        return false;
    }
    finish_surface_collapse(
        st,
        id,
        now,
        pending.map(|pending| pending.origin_pos),
        true,
        pending.is_some(),
    )
}

pub(crate) fn finish_auto_collapse(st: &mut Halley, id: NodeId, now: Instant) -> bool {
    let pending = st.model.workspace_state.pending_collapses.remove(&id);
    if st.is_fullscreen_session_node(id) {
        return false;
    }
    finish_surface_collapse(
        st,
        id,
        now,
        pending.map(|pending| pending.origin_pos),
        pending.is_some_and(|pending| pending.preserve_manual),
        pending.is_some(),
    )
}

fn finish_surface_collapse(
    st: &mut Halley,
    id: NodeId,
    now: Instant,
    origin_pos: Option<Vec2>,
    preserve_manual: bool,
    was_pending: bool,
) -> bool {
    let Some(current_pos) = st.model.field.node(id).map(|node| node.pos) else {
        return false;
    };

    let _ = st
        .model
        .field
        .set_decay_level(id, halley_core::decay::DecayLevel::Cold);
    st.model
        .spawn_state
        .pending_spawn_activate_at_ms
        .remove(&id);
    if preserve_manual {
        st.model.workspace_state.manual_collapsed_nodes.insert(id);
    } else {
        st.model.workspace_state.manual_collapsed_nodes.remove(&id);
    }

    // Keep the pre-collapse origin so landmark slide starts where the active
    // window was, not from the post-carry non-overlap position.
    let from = origin_pos.unwrap_or(current_pos);
    let _ = st.carry_surface_non_overlap(id, from, false);
    if let Some(to) = st.model.field.node(id).map(|node| node.pos)
        && ((from.x - to.x).abs() > 0.5 || (from.y - to.y).abs() > 0.5)
    {
        let slide_start = if was_pending {
            now
        } else {
            st.ui
                .render_state
                .window_animations
                .closing_window_animations
                .get(&id)
                .map(|anim| anim.started_at + Duration::from_millis(anim.duration_ms))
                .unwrap_or(now)
        };
        st.ui
            .render_state
            .start_landmark_slide_animation_at(id, from, to, slide_start);
    }

    if st.model.focus_state.primary_interaction_focus == Some(id) {
        st.set_interaction_focus(None, 0, now);
    }
    if st.model.focus_state.pan_restore_active_focus == Some(id) {
        st.model.focus_state.pan_restore_active_focus = None;
    }
    st.request_maintenance();
    true
}

pub(crate) fn process_pending_collapses_for_monitor(st: &mut Halley, monitor: &str, now: Instant) {
    if st.model.workspace_state.pending_collapses.is_empty() {
        return;
    }

    let now_ms = st.now_ms(now);
    let pending = st
        .model
        .workspace_state
        .pending_collapses
        .iter()
        .map(|(&id, pending)| (id, *pending))
        .collect::<Vec<_>>();

    let mut needs_retry = false;
    for (id, pending) in pending {
        let Some(node) = st.model.field.node(id) else {
            st.model.workspace_state.pending_collapses.remove(&id);
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
            st.model.workspace_state.pending_collapses.remove(&id);
            continue;
        }

        if start_active_to_node_close_animation(st, id, now)
            // A later frame may have warmed the window texture. Do not wait
            // indefinitely; bad/no-content surfaces still need to collapse.
            || now_ms.saturating_sub(pending.requested_at_ms) >= PENDING_COLLAPSE_MAX_WAIT_MS
        {
            if pending.preserve_manual {
                let _ = finish_manual_collapse(st, id, now);
            } else {
                let _ = finish_auto_collapse(st, id, now);
            }
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
            .any(|session| matches!(session.state, MaximizeSessionState::Restoring))
}

pub(crate) fn maximize_animation_active_for_monitor(st: &Halley, monitor: &str) -> bool {
    st.model
        .workspace_state
        .maximize_animation
        .values()
        .any(|anim| anim.monitor == monitor)
        || st
            .model
            .workspace_state
            .maximize_sessions
            .get(monitor)
            .is_some_and(|session| matches!(session.state, MaximizeSessionState::Restoring))
}

pub(crate) fn maximize_session_active_on_monitor(st: &Halley, monitor: &str) -> bool {
    st.model
        .workspace_state
        .maximize_sessions
        .get(monitor)
        .is_some_and(|session| session.state == MaximizeSessionState::Active)
}

/// True while a maximize session exists on `monitor` in any state (`Active` or
/// `Restoring`). Used to keep the work area frozen across the whole session,
/// including the restore animation, so the aperture reservation settles once at
/// the end instead of popping back as the window slides shut.
pub(crate) fn maximize_session_present_on_monitor(st: &Halley, monitor: &str) -> bool {
    st.model
        .workspace_state
        .maximize_sessions
        .contains_key(monitor)
}

pub(crate) fn maximize_session_target_for_monitor(st: &Halley, monitor: &str) -> Option<NodeId> {
    st.model
        .workspace_state
        .maximize_sessions
        .get(monitor)
        .map(|session| session.target_id)
}

pub(crate) fn maximize_session_monitor_for_node(st: &Halley, node_id: NodeId) -> Option<String> {
    st.model
        .workspace_state
        .maximize_sessions
        .iter()
        .find_map(|(monitor, session)| (session.target_id == node_id).then(|| monitor.clone()))
}

#[cfg(test)]
pub(crate) fn maximized_visual_for_node_on_current_monitor(
    st: &Halley,
    node_id: NodeId,
) -> Option<(Vec2, Vec2)> {
    maximized_visual_for_node_on_current_monitor_at(st, node_id, Instant::now())
}

pub(crate) fn maximized_visual_for_node_on_current_monitor_at(
    st: &Halley,
    node_id: NodeId,
    now: Instant,
) -> Option<(Vec2, Vec2)> {
    let monitor = st.model.monitor_state.current_monitor.as_str();
    maximized_visual_for_node_on_monitor_at(st, node_id, monitor, now)
}

pub(crate) fn maximized_visual_for_node_on_monitor_at(
    st: &Halley,
    node_id: NodeId,
    monitor: &str,
    now: Instant,
) -> Option<(Vec2, Vec2)> {
    if let Some(rect) = maximize_animation_visual_for_node_on_monitor_at(st, node_id, monitor, now)
    {
        return Some(rect);
    }

    let session = st.model.workspace_state.maximize_sessions.get(monitor)?;
    if session.target_id != node_id || session.state != MaximizeSessionState::Active {
        return None;
    }
    let viewport = st
        .model
        .monitor_state
        .monitors
        .get(monitor)?
        .usable_viewport;
    let inset = st.non_overlap_gap_world().max(0.0)
        + crate::window::active_window_frame_pad_px(&st.runtime.tuning) as f32;
    Some((
        viewport.center,
        Vec2 {
            x: (viewport.size.x - inset * 2.0).max(96.0),
            y: (viewport.size.y - inset * 2.0).max(72.0),
        },
    ))
}

pub(crate) fn maximize_animation_visual_for_node_on_monitor_at(
    st: &Halley,
    node_id: NodeId,
    monitor: &str,
    now: Instant,
) -> Option<(Vec2, Vec2)> {
    let anim = st.model.workspace_state.maximize_animation.get(&node_id)?;
    (anim.monitor == monitor).then(|| maximize_animation_rect(st, anim, now))
}

/// Continuous parallax weight for a node mid maximize transition: `0.0` at the
/// maximized extent (no parallax), `1.0` at the windowed extent (full parallax). Mirrors
/// [`crate::compositor::fullscreen::system::fullscreen_parallax_restore_weight_for_node_on_current_monitor_at`]
/// so the window eases back to its resting parallax-offset position without a snap.
pub(crate) fn maximize_parallax_restore_weight_for_node_on_current_monitor_at(
    st: &Halley,
    node_id: NodeId,
    now: Instant,
) -> Option<f32> {
    let monitor = st.model.monitor_state.current_monitor.as_str();
    if let Some(anim) = st
        .model
        .workspace_state
        .maximize_animation
        .get(&node_id)
        .filter(|anim| anim.monitor == monitor)
    {
        let now_ms = st.now_ms(now);
        let t = (now_ms.saturating_sub(anim.start_ms) as f32 / anim.duration_ms.max(1) as f32)
            .clamp(0.0, 1.0);
        let e = if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
        };
        return Some(
            crate::compositor::fullscreen::system::transition_restore_weight(
                anim.from_size.x,
                anim.to_size.x,
                e,
            ),
        );
    }
    st.model
        .workspace_state
        .maximize_sessions
        .get(monitor)
        .filter(|session| {
            session.target_id == node_id && session.state == MaximizeSessionState::Active
        })
        .map(|_| 0.0)
}

fn maximize_animation_rect(st: &Halley, anim: &MaximizeAnimation, now: Instant) -> (Vec2, Vec2) {
    let now_ms = st.now_ms(now);
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
    (pos, size)
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

pub(crate) fn abort_maximize_session_for_monitor(st: &mut Halley, monitor: &str) -> bool {
    let Some(session) = st.model.workspace_state.maximize_sessions.remove(monitor) else {
        return false;
    };
    crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(st);

    apply_monitor_camera_snapshot(st, monitor, session.camera);

    for (id, snapshot) in session.node_snapshots {
        st.model.workspace_state.maximize_animation.remove(&id);

        let Some(node) = st.model.field.node_mut(id) else {
            continue;
        };
        node.pos = snapshot.pos;
        node.intrinsic_size = snapshot.size;
        let _ = st.model.field.sync_active_footprint_to_intrinsic(id);
        let _ = st.model.field.set_pinned(id, snapshot.pinned);
        st.request_toplevel_resize(
            id,
            snapshot.size.x.round() as i32,
            snapshot.size.y.round() as i32,
        );
        st.set_last_active_size_now(id, snapshot.size);
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

pub(crate) fn abort_maximize_session_for_external_active_node_on_monitor(
    st: &mut Halley,
    monitor: &str,
    entering_id: NodeId,
) -> bool {
    let _ = (st, monitor, entering_id);
    false
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

        if t >= 1.0 {
            finished.push((id, anim));
        }
    }

    let had_finished = !finished.is_empty();
    for (id, anim) in finished {
        st.model.workspace_state.maximize_animation.remove(&id);
        if st
            .model
            .workspace_state
            .maximize_sessions
            .get(anim.monitor.as_str())
            .is_some_and(|session| {
                session.target_id == id && matches!(session.state, MaximizeSessionState::Restoring)
            })
        {
            st.model
                .workspace_state
                .maximize_sessions
                .remove(anim.monitor.as_str());
        }
    }

    if had_finished || !st.model.workspace_state.maximize_animation.is_empty() {
        st.request_maintenance();
    }
}
