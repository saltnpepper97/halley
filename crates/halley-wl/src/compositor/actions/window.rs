use crate::compositor::root::Halley;
use crate::window::active_window_frame_pad_px;
use eventline::debug;
use halley_config::{ClickCollapsedOutsideFocusMode, ClickCollapsedPanMode};
use halley_core::decay::DecayLevel;
use halley_core::field::NodeId;
use halley_core::viewport::FocusZone;
use halley_ipc::{NodeMoveDirection, TrailDirection};
use std::time::Instant;

pub(crate) fn promote_node_level(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    now: Instant,
) -> bool {
    let Some(n) = st.model.field.node(node_id) else {
        return false;
    };
    if n.kind != halley_core::field::NodeKind::Surface {
        return false;
    }
    if n.state != halley_core::field::NodeState::Node {
        return false;
    }
    let target_pos = n.pos;
    let target_monitor = st.monitor_for_node_or_current(node_id);
    let focus_center = st.view_center_for_monitor(target_monitor.as_str());
    let focus_ring = st.focus_ring_for_monitor(target_monitor.as_str());
    let maximize_resume_monitor =
        crate::compositor::workspace::state::take_maximize_resume_for_node(st, node_id);

    let in_focus_ring = focus_ring.zone(focus_center, target_pos) == FocusZone::Inside;

    if let Some(maximize_monitor) = maximize_resume_monitor.as_deref() {
        st.model
            .workspace_state
            .manual_collapsed_nodes
            .remove(&node_id);
        let _ = st.model.field.set_decay_level(node_id, DecayLevel::Hot);
        crate::compositor::workspace::state::mark_active_transition(st, node_id, now, 360);
        st.set_interaction_focus(Some(node_id), 30_000, now);
        return start_maximize_session(st, node_id, maximize_monitor, now);
    }

    if in_focus_ring {
        // This is a deliberate promote, not a stale auto-resurrect.
        st.model
            .workspace_state
            .manual_collapsed_nodes
            .remove(&node_id);

        let _ = st.model.field.set_decay_level(node_id, DecayLevel::Hot);
        crate::compositor::workspace::state::mark_active_transition(st, node_id, now, 360);

        st.set_interaction_focus(Some(node_id), 30_000, now);
        return true;
    }

    st.set_interaction_focus(Some(node_id), 30_000, now);
    st.set_pan_restore_focus_target(node_id);
    st.animate_viewport_center_to(target_pos, now)
}

pub(crate) fn focus_or_reveal_collapsed_node_from_click(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    now: Instant,
) -> bool {
    let Some(n) = st.model.field.node(node_id) else {
        return false;
    };
    if n.kind != halley_core::field::NodeKind::Surface {
        return false;
    }
    if n.state != halley_core::field::NodeState::Node {
        return false;
    }

    let target_monitor = st.monitor_for_node_or_current(node_id);
    let focus_center = st.view_center_for_monitor(target_monitor.as_str());
    let focus_ring = st.focus_ring_for_monitor(target_monitor.as_str());
    let in_focus_ring = focus_ring.zone(focus_center, n.pos) == FocusZone::Inside;

    st.set_interaction_focus(Some(node_id), 30_000, now);

    if in_focus_ring
        || st.runtime.tuning.click_collapsed_outside_focus == ClickCollapsedOutsideFocusMode::Ignore
    {
        return true;
    }

    match st.runtime.tuning.click_collapsed_pan {
        ClickCollapsedPanMode::Never => true,
        ClickCollapsedPanMode::IfOffscreen => {
            if st.surface_is_fully_visible_on_monitor(target_monitor.as_str(), node_id) {
                true
            } else {
                st.set_pan_restore_focus_target(node_id);
                st.minimal_reveal_center_for_surface_on_monitor(target_monitor.as_str(), node_id)
                    .map(|target| st.animate_viewport_center_to(target, now))
                    .unwrap_or(true)
            }
        }
        ClickCollapsedPanMode::Always => {
            st.set_pan_restore_focus_target(node_id);
            st.minimal_reveal_center_for_surface_on_monitor(target_monitor.as_str(), node_id)
                .map(|target| st.animate_viewport_center_to(target, now))
                .unwrap_or(true)
        }
    }
}

pub(crate) fn focus_or_reveal_surface_node(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    now: Instant,
) -> bool {
    let Some(node) = st.model.field.node(node_id).cloned() else {
        return false;
    };
    if node.kind != halley_core::field::NodeKind::Surface {
        return false;
    }

    let target_monitor = st.monitor_for_node_or_current(node_id);
    if st.focused_monitor() != target_monitor {
        st.focus_monitor_view(target_monitor.as_str(), now);
    }

    match node.state {
        halley_core::field::NodeState::Node => promote_node_level(st, node_id, now),
        halley_core::field::NodeState::Active | halley_core::field::NodeState::Drifting => {
            st.set_interaction_focus(Some(node_id), 30_000, now);
            let is_pending_tiled = st
                .model
                .spawn_state
                .pending_tiled_insert_reveal_at_ms
                .contains_key(&node_id);
            let is_pending_reveal = st
                .model
                .spawn_state
                .pending_initial_reveal
                .contains(&node_id);
            if !is_pending_tiled && !is_pending_reveal {
                let _ = st
                    .minimal_reveal_center_for_surface_on_monitor(target_monitor.as_str(), node_id)
                    .map(|target| st.animate_viewport_center_to(target, now));
            }
            true
        }
        halley_core::field::NodeState::Core => false,
    }
}

pub(crate) fn focus_surface_node_without_reveal(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    now: Instant,
) -> bool {
    let Some(node) = st.model.field.node(node_id) else {
        return false;
    };
    if node.kind != halley_core::field::NodeKind::Surface {
        return false;
    }
    if node.state == halley_core::field::NodeState::Core {
        return false;
    }

    let target_monitor = st.monitor_for_node_or_current(node_id);
    if st.focused_monitor() != target_monitor {
        st.focus_monitor_view(target_monitor.as_str(), now);
    }
    st.set_interaction_focus(Some(node_id), 30_000, now);
    true
}

pub(crate) fn latest_surface_node(st: &Halley) -> Option<halley_core::field::NodeId> {
    st.last_input_surface_node_for_monitor(st.focused_monitor())
        .or_else(|| st.last_input_surface_node())
        .or_else(|| {
            st.model
                .surface_to_node
                .values()
                .copied()
                .max_by_key(|id| id.as_u64())
        })
}

fn focused_surface_node_for_action(st: &Halley, focused_monitor: &str) -> Option<NodeId> {
    st.focused_node_for_monitor(focused_monitor)
        .filter(|&id| st.model.field.node(id).is_some() && st.model.field.is_visible(id))
        .or(st
            .model
            .focus_state
            .primary_interaction_focus
            .filter(|&id| st.model.field.node(id).is_some() && st.model.field.is_visible(id)))
        .or_else(|| st.last_focused_surface_node_for_monitor(focused_monitor))
        .or_else(|| st.last_focused_surface_node())
}

fn focused_node_for_pin_action(st: &Halley, focused_monitor: &str) -> Option<NodeId> {
    st.focused_node_for_monitor(focused_monitor)
        .filter(|&id| st.model.field.node(id).is_some() && st.model.field.is_visible(id))
        .or(st
            .model
            .focus_state
            .primary_interaction_focus
            .filter(|&id| st.model.field.node(id).is_some() && st.model.field.is_visible(id)))
        .or_else(|| latest_surface_node(st))
}

pub(crate) fn toggle_focused_pin_state(st: &mut Halley) -> bool {
    let focused_monitor = st.focused_monitor().to_string();
    let Some(id) = focused_node_for_pin_action(st, focused_monitor.as_str()) else {
        return false;
    };
    let Some(node) = st.model.field.node(id) else {
        return false;
    };
    if !matches!(
        node.kind,
        halley_core::field::NodeKind::Surface | halley_core::field::NodeKind::Core
    ) || !st.model.field.is_visible(id)
    {
        return false;
    }

    let next = !st.node_user_pinned(id);
    st.set_node_user_pinned(id, next)
}

fn field_viewport_for_monitor(st: &Halley, monitor: &str) -> halley_core::viewport::Viewport {
    if st.model.monitor_state.current_monitor == monitor {
        st.model.viewport
    } else {
        st.model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| space.viewport)
            .unwrap_or(st.model.viewport)
    }
}

fn current_window_size_for_node(st: &Halley, id: NodeId) -> Option<halley_core::field::Vec2> {
    st.model
        .field
        .node(id)
        .and_then(|node| node.resize_footprint)
        .or_else(|| crate::compositor::surface::current_surface_size_for_node(st, id))
        .or_else(|| st.model.workspace_state.last_active_size.get(&id).copied())
        .or_else(|| st.model.field.node(id).map(|node| node.intrinsic_size))
}

fn maximize_viewport_rect_for_monitor(st: &Halley, monitor: &str) -> halley_core::field::Rect {
    let viewport = field_viewport_for_monitor(st, monitor);
    let half = halley_core::field::Vec2 {
        x: viewport.size.x * 0.5,
        y: viewport.size.y * 0.5,
    };
    halley_core::field::Rect {
        min: halley_core::field::Vec2 {
            x: viewport.center.x - half.x,
            y: viewport.center.y - half.y,
        },
        max: halley_core::field::Vec2 {
            x: viewport.center.x + half.x,
            y: viewport.center.y + half.y,
        },
    }
}

fn node_intersects_maximize_monitor_viewport(st: &Halley, id: NodeId, monitor: &str) -> bool {
    let Some(node) = st.model.field.node(id) else {
        return false;
    };
    let ext = st.collision_extents_for_node(node);
    halley_core::field::Rect {
        min: halley_core::field::Vec2 {
            x: node.pos.x - ext.left,
            y: node.pos.y - ext.top,
        },
        max: halley_core::field::Vec2 {
            x: node.pos.x + ext.right,
            y: node.pos.y + ext.bottom,
        },
    }
    .intersects(maximize_viewport_rect_for_monitor(st, monitor))
}

fn maximize_displaced_target(
    pos: halley_core::field::Vec2,
    ordinal: usize,
    viewport_center: halley_core::field::Vec2,
    viewport_size: halley_core::field::Vec2,
) -> halley_core::field::Vec2 {
    let mut dir = halley_core::field::Vec2 {
        x: pos.x - viewport_center.x,
        y: pos.y - viewport_center.y,
    };
    let len = dir.x.hypot(dir.y);
    if len < 1.0 {
        let dirs = [
            halley_core::field::Vec2 { x: 1.0, y: 0.0 },
            halley_core::field::Vec2 { x: -1.0, y: 0.0 },
            halley_core::field::Vec2 { x: 0.0, y: -1.0 },
            halley_core::field::Vec2 { x: 0.0, y: 1.0 },
        ];
        dir = dirs[ordinal % dirs.len()];
    } else {
        dir.x /= len;
        dir.y /= len;
    }

    let radius = viewport_size.x.hypot(viewport_size.y) * 0.85 + 320.0;
    halley_core::field::Vec2 {
        x: viewport_center.x + dir.x * radius,
        y: viewport_center.y + dir.y * radius,
    }
}

fn maximize_snapshot_for_node(
    st: &Halley,
    id: NodeId,
) -> Option<crate::compositor::workspace::state::MaximizeNodeSnapshot> {
    let node = st.model.field.node(id)?;
    Some(crate::compositor::workspace::state::MaximizeNodeSnapshot {
        pos: node.pos,
        size: current_window_size_for_node(st, id).unwrap_or(node.intrinsic_size),
        pinned: node.pinned,
    })
}

fn maximize_target_for_monitor(
    st: &Halley,
    monitor: &str,
) -> (halley_core::field::Vec2, halley_core::field::Vec2) {
    let viewport = field_viewport_for_monitor(st, monitor);
    let inset =
        st.non_overlap_gap_world().max(0.0) + active_window_frame_pad_px(&st.runtime.tuning) as f32;
    (
        viewport.center,
        halley_core::field::Vec2 {
            x: (viewport.size.x - inset * 2.0).max(96.0),
            y: (viewport.size.y - inset * 2.0).max(72.0),
        },
    )
}

fn apply_maximize_geometry_now(
    st: &mut Halley,
    id: NodeId,
    target_pos: halley_core::field::Vec2,
    target_size: halley_core::field::Vec2,
) -> bool {
    if let Some(node) = st.model.field.node_mut(id) {
        node.pos = target_pos;
        node.intrinsic_size = target_size;
    } else {
        return false;
    }
    let _ = st.model.field.sync_active_footprint_to_intrinsic(id);
    st.request_toplevel_resize(
        id,
        target_size.x.round() as i32,
        target_size.y.round() as i32,
    );
    st.set_last_active_size_now(id, target_size);
    true
}

fn begin_maximize_animation(
    st: &mut Halley,
    monitor: &str,
    id: NodeId,
    from_pos: halley_core::field::Vec2,
    from_size: halley_core::field::Vec2,
    to_pos: halley_core::field::Vec2,
    to_size: halley_core::field::Vec2,
    now: Instant,
) -> bool {
    let _ = st.model.field.set_pinned(id, false);
    if !st.runtime.tuning.maximize_animation_enabled() {
        return apply_maximize_geometry_now(st, id, to_pos, to_size);
    }

    st.model.workspace_state.maximize_animation.insert(
        id,
        crate::compositor::workspace::state::MaximizeAnimation {
            monitor: monitor.to_string(),
            from_pos,
            to_pos,
            from_size,
            to_size,
            start_ms: st.now_ms(now),
            duration_ms: st.runtime.tuning.maximize_animation_duration_ms(),
        },
    );
    st.request_maintenance();
    true
}

fn set_session_nodes_pinned(
    st: &mut Halley,
    snapshots: &std::collections::HashMap<
        NodeId,
        crate::compositor::workspace::state::MaximizeNodeSnapshot,
    >,
    pinned: bool,
) {
    for &node_id in snapshots.keys() {
        let _ = st.model.field.set_pinned(node_id, pinned);
    }
}

fn start_restore_maximize_session(
    st: &mut Halley,
    monitor: &str,
    now: Instant,
    state: crate::compositor::workspace::state::MaximizeSessionState,
) -> bool {
    let (node_snapshots, camera) = {
        let Some(session) = st.model.workspace_state.maximize_sessions.get_mut(monitor) else {
            return false;
        };
        session.state = state;
        (session.node_snapshots.clone(), session.camera)
    };

    if state == crate::compositor::workspace::state::MaximizeSessionState::Restoring {
        crate::compositor::workspace::state::set_monitor_camera_target_snapshot(
            st, monitor, camera,
        );
    }
    if !st.runtime.tuning.maximize_animation_enabled() {
        for (node_id, snapshot) in &node_snapshots {
            let _ = st.model.field.set_pinned(*node_id, false);
            let _ = apply_maximize_geometry_now(st, *node_id, snapshot.pos, snapshot.size);
            let _ = st.model.field.set_pinned(*node_id, snapshot.pinned);
        }
        st.model.workspace_state.maximize_sessions.remove(monitor);
        st.resolve_overlap_now();
        return true;
    }

    for (node_id, snapshot) in &node_snapshots {
        let Some(node) = st.model.field.node(*node_id) else {
            continue;
        };
        let from_pos = node.pos;
        let from_intrinsic_size = node.intrinsic_size;
        let from_size = current_window_size_for_node(st, *node_id).unwrap_or(from_intrinsic_size);
        let _ = begin_maximize_animation(
            st,
            monitor,
            *node_id,
            from_pos,
            from_size,
            snapshot.pos,
            snapshot.size,
            now,
        );
    }
    true
}

pub(crate) fn restore_maximize_session_for_spawn(
    st: &mut Halley,
    monitor: &str,
    now: Instant,
) -> bool {
    let Some(target_id) =
        crate::compositor::workspace::state::maximize_session_target_for_monitor(st, monitor)
    else {
        return false;
    };
    st.set_recent_top_node(target_id, now + std::time::Duration::from_millis(1200));
    st.set_interaction_focus(Some(target_id), 30_000, now);
    start_restore_maximize_session(
        st,
        monitor,
        now,
        crate::compositor::workspace::state::MaximizeSessionState::SpawnRestoring,
    )
}

fn start_active_maximize_session(
    st: &mut Halley,
    target_id: NodeId,
    monitor: &str,
    node_snapshots: &std::collections::HashMap<
        NodeId,
        crate::compositor::workspace::state::MaximizeNodeSnapshot,
    >,
    now: Instant,
) -> bool {
    crate::compositor::workspace::state::reset_monitor_zoom_for_maximize(st, monitor);

    let viewport = field_viewport_for_monitor(st, monitor);
    let (target_pos, target_size) = maximize_target_for_monitor(st, monitor);
    let mut bystanders = node_snapshots
        .keys()
        .copied()
        .filter(|node_id| *node_id != target_id)
        .collect::<Vec<_>>();
    bystanders.sort_by_key(|node_id| node_id.as_u64());

    if !st.runtime.tuning.maximize_animation_enabled() {
        let _ = apply_maximize_geometry_now(st, target_id, target_pos, target_size);
        for (ordinal, other_id) in bystanders.iter().enumerate() {
            if let Some(snapshot) = node_snapshots.get(other_id).copied() {
                let _ = apply_maximize_geometry_now(
                    st,
                    *other_id,
                    maximize_displaced_target(
                        snapshot.pos,
                        ordinal,
                        viewport.center,
                        viewport.size,
                    ),
                    snapshot.size,
                );
            }
        }
        set_session_nodes_pinned(st, node_snapshots, true);
        st.resolve_overlap_now();
        st.request_maintenance();
        return true;
    }

    let target_from = st
        .model
        .field
        .node(target_id)
        .map(|node| {
            (
                node.pos,
                current_window_size_for_node(st, target_id).unwrap_or(node.intrinsic_size),
            )
        })
        .unwrap_or((target_pos, target_size));
    let _ = begin_maximize_animation(
        st,
        monitor,
        target_id,
        target_from.0,
        target_from.1,
        target_pos,
        target_size,
        now,
    );
    for (ordinal, other_id) in bystanders.iter().enumerate() {
        if let Some(snapshot) = node_snapshots.get(other_id).copied() {
            let from = st
                .model
                .field
                .node(*other_id)
                .map(|node| {
                    (
                        node.pos,
                        current_window_size_for_node(st, *other_id).unwrap_or(node.intrinsic_size),
                    )
                })
                .unwrap_or((snapshot.pos, snapshot.size));
            let _ = begin_maximize_animation(
                st,
                monitor,
                *other_id,
                from.0,
                from.1,
                maximize_displaced_target(snapshot.pos, ordinal, viewport.center, viewport.size),
                snapshot.size,
                now,
            );
        }
    }
    true
}

fn start_maximize_session(st: &mut Halley, id: NodeId, monitor: &str, now: Instant) -> bool {
    if let Some(existing) = st
        .model
        .workspace_state
        .maximize_sessions
        .get(monitor)
        .cloned()
    {
        if existing.target_id == id {
            return match existing.state {
                crate::compositor::workspace::state::MaximizeSessionState::Active => {
                    start_restore_maximize_session(
                        st,
                        monitor,
                        now,
                        crate::compositor::workspace::state::MaximizeSessionState::Restoring,
                    )
                }
                crate::compositor::workspace::state::MaximizeSessionState::Restoring => {
                    if let Some(session) =
                        st.model.workspace_state.maximize_sessions.get_mut(monitor)
                    {
                        session.state =
                            crate::compositor::workspace::state::MaximizeSessionState::Active;
                    }
                    start_active_maximize_session(st, id, monitor, &existing.node_snapshots, now)
                }
                crate::compositor::workspace::state::MaximizeSessionState::SpawnRestoring => {
                    if let Some(session) =
                        st.model.workspace_state.maximize_sessions.get_mut(monitor)
                    {
                        session.state =
                            crate::compositor::workspace::state::MaximizeSessionState::Active;
                    }
                    start_active_maximize_session(st, id, monitor, &existing.node_snapshots, now)
                }
            };
        }
        let _ =
            crate::compositor::workspace::state::abort_maximize_session_for_monitor(st, monitor);
    }

    let Some(target_snapshot) = maximize_snapshot_for_node(st, id) else {
        return false;
    };
    let camera = crate::compositor::workspace::state::snapshot_monitor_camera(st, monitor);

    let mut node_snapshots = std::collections::HashMap::new();
    node_snapshots.insert(id, target_snapshot);

    let mut bystanders = st
        .model
        .field
        .node_ids_all()
        .into_iter()
        .filter(|other_id| *other_id != id)
        .filter_map(|other_id| {
            let node = st.model.field.node(other_id)?;
            (node.kind == halley_core::field::NodeKind::Surface
                && st.model.field.is_visible(other_id)
                && !st.node_user_pinned(other_id)
                && st
                    .model
                    .monitor_state
                    .node_monitor
                    .get(&other_id)
                    .is_some_and(|node_monitor| node_monitor == monitor)
                && node_intersects_maximize_monitor_viewport(st, other_id, monitor))
            .then_some(other_id)
        })
        .collect::<Vec<_>>();
    bystanders.sort_by_key(|node_id| node_id.as_u64());
    for other_id in &bystanders {
        if let Some(snapshot) = maximize_snapshot_for_node(st, *other_id) {
            node_snapshots.insert(*other_id, snapshot);
        }
    }

    st.model.workspace_state.maximize_sessions.insert(
        monitor.to_string(),
        crate::compositor::workspace::state::MaximizeSession {
            target_id: id,
            node_snapshots: node_snapshots.clone(),
            camera,
            state: crate::compositor::workspace::state::MaximizeSessionState::Active,
        },
    );

    start_active_maximize_session(st, id, monitor, &node_snapshots, now)
}

pub(crate) fn toggle_focused_maximize_node_state(st: &mut Halley) -> bool {
    let now = Instant::now();
    let focused_monitor = st.focused_monitor().to_string();

    let Some(id) = focused_surface_node_for_action(st, focused_monitor.as_str()) else {
        return false;
    };

    toggle_node_maximize_state(st, id, now, focused_monitor.as_str())
}

pub(crate) fn toggle_node_maximize_state(
    st: &mut Halley,
    id: halley_core::field::NodeId,
    now: Instant,
    focused_monitor: &str,
) -> bool {
    let Some(node) = st.model.field.node(id).cloned() else {
        return false;
    };
    if node.kind != halley_core::field::NodeKind::Surface {
        return false;
    }
    if crate::compositor::surface::is_active_cluster_workspace_member(st, id)
        || st.is_fullscreen_active(id)
    {
        return false;
    }

    let monitor = st
        .model
        .monitor_state
        .node_monitor
        .get(&id)
        .cloned()
        .unwrap_or_else(|| focused_monitor.to_string());
    st.set_interaction_focus(Some(id), 30_000, now);
    start_maximize_session(st, id, monitor.as_str(), now)
}

pub(crate) fn move_latest_node(st: &mut Halley, dx: f32, dy: f32) -> bool {
    let Some(id) = latest_surface_node(st) else {
        return false;
    };
    if crate::compositor::workspace::state::node_in_maximize_session(st, id) {
        return false;
    }
    if crate::compositor::surface::is_active_stacking_workspace_member(st, id) {
        return false;
    }
    let Some(n) = st.model.field.node(id) else {
        return false;
    };
    if n.pinned {
        return false;
    }
    let to = halley_core::field::Vec2 {
        x: n.pos.x + dx,
        y: n.pos.y + dy,
    };
    crate::compositor::carry::system::begin_carry_state_tracking(st, id);
    if st.carry_surface_non_overlap(id, to, false) {
        crate::compositor::carry::system::update_carry_state_preview(st, id, Instant::now());
        crate::compositor::carry::system::end_carry_state_tracking(st, id);
        st.set_interaction_focus(Some(id), 30_000, Instant::now());
        if let Some(nn) = st.model.field.node(id) {
            debug!(
                "moved node id={} to ({:.0},{:.0}) state={:?}",
                id.as_u64(),
                nn.pos.x,
                nn.pos.y,
                nn.state
            );
        }
        return true;
    }
    crate::compositor::carry::system::end_carry_state_tracking(st, id);
    false
}

pub(crate) fn move_latest_node_direction(st: &mut Halley, direction: NodeMoveDirection) -> bool {
    const STEP_NODE: f32 = 80.0;

    match direction {
        NodeMoveDirection::Left => move_latest_node(st, -STEP_NODE, 0.0),
        NodeMoveDirection::Right => move_latest_node(st, STEP_NODE, 0.0),
        NodeMoveDirection::Up => move_latest_node(st, 0.0, -STEP_NODE),
        NodeMoveDirection::Down => move_latest_node(st, 0.0, STEP_NODE),
    }
}

pub(crate) fn step_window_trail(st: &mut Halley, direction: TrailDirection) -> bool {
    if crate::compositor::workspace::state::maximize_session_active_on_monitor(
        st,
        st.focused_monitor(),
    ) {
        return false;
    }
    st.navigate_window_trail(direction, Instant::now())
}

pub(crate) fn toggle_focused_active_node_state(st: &mut Halley) -> bool {
    let now = Instant::now();
    let focused_monitor = st.focused_monitor().to_string();

    let Some(id) = focused_surface_node_for_action(st, focused_monitor.as_str()) else {
        return false;
    };

    toggle_node_state(st, id, now, focused_monitor.as_str())
}

pub(crate) fn toggle_node_state(
    st: &mut Halley,
    id: halley_core::field::NodeId,
    now: Instant,
    focused_monitor: &str,
) -> bool {
    let Some(n) = st.model.field.node(id).cloned() else {
        return false;
    };

    if n.kind == halley_core::field::NodeKind::Core
        && n.state == halley_core::field::NodeState::Core
    {
        return st.toggle_cluster_workspace_by_core(id, now);
    }

    if n.kind != halley_core::field::NodeKind::Surface {
        return false;
    }

    let maximize_monitor = st.model.monitor_state.node_monitor.get(&id).cloned();
    let should_resume_maximize = maximize_monitor.as_deref().is_some_and(|monitor| {
        crate::compositor::workspace::state::maximize_session_target_for_monitor(st, monitor)
            == Some(id)
    });
    if crate::compositor::workspace::state::node_in_maximize_session(st, id)
        && let Some(monitor) = maximize_monitor.as_deref()
    {
        let _ =
            crate::compositor::workspace::state::abort_maximize_session_for_monitor(st, monitor);
        if should_resume_maximize {
            crate::compositor::workspace::state::set_maximize_resume_for_node(st, id, monitor);
        }
    }

    if let Some(cid) = st.model.field.cluster_id_for_member_public(id) {
        if st.active_cluster_workspace_for_monitor(focused_monitor) == Some(cid) {
            return st.collapse_active_cluster_workspace(now);
        }
    }

    match n.state {
        halley_core::field::NodeState::Active => {
            if crate::compositor::workspace::state::start_active_to_node_close_animation(
                st, id, now,
            ) {
                let _ = crate::compositor::workspace::state::finish_manual_collapse(st, id, now);
            } else {
                crate::compositor::workspace::state::queue_pending_manual_collapse(st, id, now);
            }
            true
        }

        halley_core::field::NodeState::Node => {
            st.model.workspace_state.manual_collapsed_nodes.remove(&id);
            st.model
                .workspace_state
                .pending_manual_collapses
                .remove(&id);
            let _ = st
                .model
                .field
                .set_decay_level(id, halley_core::decay::DecayLevel::Hot);
            st.model
                .spawn_state
                .pending_spawn_activate_at_ms
                .remove(&id);
            crate::compositor::workspace::state::mark_active_transition(st, id, now, 360);

            st.set_interaction_focus(Some(id), 30_000, now);
            if let Some(maximize_monitor) =
                crate::compositor::workspace::state::take_maximize_resume_for_node(st, id)
            {
                let _ = start_maximize_session(st, id, maximize_monitor.as_str(), now);
            }
            true
        }

        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        focus_surface_node_without_reveal, maximize_target_for_monitor, toggle_focused_pin_state,
        toggle_node_maximize_state, toggle_node_state,
    };
    use crate::compositor::root::Halley;
    use crate::window::active_window_frame_pad_px;
    use smithay::reexports::wayland_server::Display;
    use std::time::Instant;

    fn single_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
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
        tuning
    }

    #[test]
    fn activation_focus_does_not_pan_to_existing_surface() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let id = st.model.field.spawn_surface(
            "steam",
            halley_core::field::Vec2 {
                x: 1600.0,
                y: 300.0,
            },
            halley_core::field::Vec2 {
                x: 1400.0,
                y: 700.0,
            },
        );
        st.assign_node_to_monitor(id, "monitor_a");
        let before = st.model.viewport.center;

        assert!(focus_surface_node_without_reveal(
            &mut st,
            id,
            Instant::now()
        ));

        assert_eq!(st.model.focus_state.primary_interaction_focus, Some(id));
        assert_eq!(st.model.viewport.center, before);
        assert!(st.input.interaction_state.viewport_pan_anim.is_none());
    }

    #[test]
    fn toggle_focused_pin_state_toggles_user_pin_and_movement_lock() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let id = st.model.field.spawn_surface(
            "app",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(id, "monitor_a");
        st.set_interaction_focus(Some(id), 30_000, Instant::now());

        assert!(toggle_focused_pin_state(&mut st));
        assert!(st.node_user_pinned(id));
        assert!(st.model.field.node(id).expect("node").pinned);

        assert!(toggle_focused_pin_state(&mut st));
        assert!(!st.node_user_pinned(id));
        assert!(!st.model.field.node(id).expect("node").pinned);
    }

    #[test]
    fn pinned_surface_can_still_toggle_maximize() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.animations.maximize.enabled = false;
        let mut st = Halley::new_for_test(&dh, tuning);
        let id = st.model.field.spawn_surface(
            "app",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(id, "monitor_a");

        assert!(st.set_node_user_pinned(id, true));
        assert!(toggle_node_maximize_state(
            &mut st,
            id,
            Instant::now(),
            "monitor_a"
        ));

        let (target_pos, target_size) = maximize_target_for_monitor(&st, "monitor_a");
        let node = st.model.field.node(id).expect("node");
        assert_eq!(node.pos, target_pos);
        assert_eq!(node.intrinsic_size, target_size);
        assert!(st.node_user_pinned(id));
        assert!(node.pinned);
    }

    #[test]
    fn maximize_targets_field_center_with_field_gap_inset() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let st = Halley::new_for_test(&dh, single_monitor_tuning());

        let (pos, size) = maximize_target_for_monitor(&st, "monitor_a");
        let inset =
            st.non_overlap_gap_world() + active_window_frame_pad_px(&st.runtime.tuning) as f32;

        assert_eq!(pos, halley_core::field::Vec2 { x: 400.0, y: 300.0 });
        assert_eq!(
            size,
            halley_core::field::Vec2 {
                x: 800.0 - inset * 2.0,
                y: 600.0 - inset * 2.0,
            }
        );
    }

    #[test]
    fn maximize_toggle_saves_restore_geometry_and_centers_target() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let id = st.model.field.spawn_surface(
            "app",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(id, "monitor_a");

        assert!(toggle_node_maximize_state(
            &mut st,
            id,
            Instant::now(),
            "monitor_a"
        ));

        let session = st
            .model
            .workspace_state
            .maximize_sessions
            .get("monitor_a")
            .expect("maximize session");
        let restore = session
            .node_snapshots
            .get(&id)
            .copied()
            .expect("restore snapshot");
        let anim = st
            .model
            .workspace_state
            .maximize_animation
            .get(&id)
            .cloned()
            .expect("maximize animation");
        let (target_pos, target_size) = maximize_target_for_monitor(&st, "monitor_a");

        assert_eq!(session.target_id, id);
        assert_eq!(restore.pos, halley_core::field::Vec2 { x: 120.0, y: 140.0 });
        assert_eq!(
            restore.size,
            halley_core::field::Vec2 { x: 320.0, y: 240.0 }
        );
        assert_eq!(anim.to_pos, target_pos);
        assert_eq!(anim.to_size, target_size);
    }

    #[test]
    fn maximize_session_tracks_bystanders_and_camera_snapshot() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        st.model.zoom_ref_size = halley_core::field::Vec2 { x: 500.0, y: 375.0 };
        st.model.camera_target_view_size = st.model.zoom_ref_size;
        st.model.viewport.center = halley_core::field::Vec2 { x: 430.0, y: 280.0 };
        st.model.camera_target_center = st.model.viewport.center;

        let target = st.model.field.spawn_surface(
            "target",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        let bystander = st.model.field.spawn_surface(
            "bystander",
            halley_core::field::Vec2 { x: 460.0, y: 260.0 },
            halley_core::field::Vec2 { x: 240.0, y: 180.0 },
        );
        st.assign_node_to_monitor(target, "monitor_a");
        st.assign_node_to_monitor(bystander, "monitor_a");

        assert!(toggle_node_maximize_state(
            &mut st,
            target,
            Instant::now(),
            "monitor_a"
        ));

        let session = st
            .model
            .workspace_state
            .maximize_sessions
            .get("monitor_a")
            .expect("maximize session");
        let bystander_snapshot = session
            .node_snapshots
            .get(&bystander)
            .copied()
            .expect("bystander snapshot");

        assert_eq!(
            session.camera.center,
            halley_core::field::Vec2 { x: 430.0, y: 280.0 }
        );
        assert_eq!(
            session.camera.view_size,
            halley_core::field::Vec2 { x: 500.0, y: 375.0 }
        );
        assert_eq!(
            bystander_snapshot.pos,
            halley_core::field::Vec2 { x: 460.0, y: 260.0 }
        );
        assert_eq!(
            bystander_snapshot.size,
            halley_core::field::Vec2 { x: 240.0, y: 180.0 }
        );
        assert_eq!(
            st.model.zoom_ref_size,
            halley_core::field::Vec2 { x: 500.0, y: 375.0 }
        );
        assert_eq!(st.model.camera_target_view_size, st.model.viewport.size);
        assert_eq!(st.model.camera_target_center, st.model.viewport.center);
    }

    #[test]
    fn unmaximize_restores_bystanders_and_camera_snapshot() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.animations.maximize.enabled = false;
        let mut st = Halley::new_for_test(&dh, tuning);
        st.model.zoom_ref_size = halley_core::field::Vec2 { x: 500.0, y: 375.0 };
        st.model.camera_target_view_size = st.model.zoom_ref_size;
        st.model.viewport.center = halley_core::field::Vec2 { x: 430.0, y: 280.0 };
        st.model.camera_target_center = st.model.viewport.center;

        let target = st.model.field.spawn_surface(
            "target",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        let bystander = st.model.field.spawn_surface(
            "bystander",
            halley_core::field::Vec2 { x: 460.0, y: 260.0 },
            halley_core::field::Vec2 { x: 240.0, y: 180.0 },
        );
        st.assign_node_to_monitor(target, "monitor_a");
        st.assign_node_to_monitor(bystander, "monitor_a");

        assert!(toggle_node_maximize_state(
            &mut st,
            target,
            Instant::now(),
            "monitor_a"
        ));
        assert!(toggle_node_maximize_state(
            &mut st,
            target,
            Instant::now(),
            "monitor_a"
        ));

        assert!(
            !st.model
                .workspace_state
                .maximize_sessions
                .contains_key("monitor_a")
        );
        assert_eq!(
            st.model.field.node(target).expect("target").pos,
            halley_core::field::Vec2 { x: 120.0, y: 140.0 }
        );
        assert_eq!(
            st.model.field.node(target).expect("target").intrinsic_size,
            halley_core::field::Vec2 { x: 320.0, y: 240.0 }
        );
        assert_eq!(
            st.model.field.node(bystander).expect("bystander").pos,
            halley_core::field::Vec2 { x: 460.0, y: 260.0 }
        );
        assert_eq!(
            st.model
                .field
                .node(bystander)
                .expect("bystander")
                .intrinsic_size,
            halley_core::field::Vec2 { x: 240.0, y: 180.0 }
        );
        assert_eq!(
            st.model.zoom_ref_size,
            halley_core::field::Vec2 { x: 500.0, y: 375.0 }
        );
        assert_eq!(
            st.model.viewport.center,
            halley_core::field::Vec2 { x: 430.0, y: 280.0 }
        );
    }

    #[test]
    fn unmaximize_restores_camera_via_smooth_target() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        st.model.zoom_ref_size = halley_core::field::Vec2 { x: 500.0, y: 375.0 };
        st.model.camera_target_view_size = st.model.zoom_ref_size;
        st.model.viewport.center = halley_core::field::Vec2 { x: 430.0, y: 280.0 };
        st.model.camera_target_center = st.model.viewport.center;

        let target = st.model.field.spawn_surface(
            "target",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(target, "monitor_a");

        assert!(toggle_node_maximize_state(
            &mut st,
            target,
            Instant::now(),
            "monitor_a"
        ));
        assert!(toggle_node_maximize_state(
            &mut st,
            target,
            Instant::now(),
            "monitor_a"
        ));

        assert_eq!(
            st.model.camera_target_view_size,
            halley_core::field::Vec2 { x: 500.0, y: 375.0 }
        );
        assert_eq!(
            st.model.camera_target_center,
            halley_core::field::Vec2 { x: 430.0, y: 280.0 }
        );
        assert!(
            st.model
                .workspace_state
                .maximize_sessions
                .get("monitor_a")
                .is_some_and(|session| {
                    session.state
                        == crate::compositor::workspace::state::MaximizeSessionState::Restoring
                })
        );
    }

    #[test]
    fn maximize_toggle_during_restore_reactivates_session() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        st.model.zoom_ref_size = halley_core::field::Vec2 { x: 500.0, y: 375.0 };
        st.model.camera_target_view_size = st.model.zoom_ref_size;

        let target = st.model.field.spawn_surface(
            "target",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(target, "monitor_a");

        assert!(toggle_node_maximize_state(
            &mut st,
            target,
            Instant::now(),
            "monitor_a"
        ));
        assert!(toggle_node_maximize_state(
            &mut st,
            target,
            Instant::now(),
            "monitor_a"
        ));
        assert!(toggle_node_maximize_state(
            &mut st,
            target,
            Instant::now(),
            "monitor_a"
        ));

        let session = st
            .model
            .workspace_state
            .maximize_sessions
            .get("monitor_a")
            .expect("maximize session");
        assert_eq!(
            session.state,
            crate::compositor::workspace::state::MaximizeSessionState::Active
        );
        assert_eq!(st.model.camera_target_view_size, st.model.viewport.size);
    }

    #[test]
    fn collapsing_maximized_window_restores_session_immediately() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.animations.maximize.enabled = false;
        let mut st = Halley::new_for_test(&dh, tuning);

        let monitor = st.model.monitor_state.current_monitor.clone();
        let target = st.model.field.spawn_surface(
            "target",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        let bystander = st.model.field.spawn_surface(
            "bystander",
            halley_core::field::Vec2 { x: 460.0, y: 260.0 },
            halley_core::field::Vec2 { x: 240.0, y: 180.0 },
        );
        st.assign_node_to_monitor(target, monitor.as_str());
        st.assign_node_to_monitor(bystander, monitor.as_str());

        assert!(toggle_node_maximize_state(
            &mut st,
            target,
            Instant::now(),
            monitor.as_str(),
        ));
        assert!(toggle_node_state(
            &mut st,
            target,
            Instant::now(),
            monitor.as_str(),
        ));

        assert!(
            !st.model
                .workspace_state
                .maximize_sessions
                .contains_key(monitor.as_str())
        );
        assert_eq!(
            st.model.field.node(bystander).expect("bystander").pos,
            halley_core::field::Vec2 { x: 460.0, y: 260.0 }
        );
    }

    #[test]
    fn reopening_collapsed_maximized_window_reenters_maximize() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.animations.maximize.enabled = false;
        let mut st = Halley::new_for_test(&dh, tuning);

        let monitor = st.model.monitor_state.current_monitor.clone();
        let target = st.model.field.spawn_surface(
            "target",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(target, monitor.as_str());

        assert!(toggle_node_maximize_state(
            &mut st,
            target,
            Instant::now(),
            monitor.as_str(),
        ));
        assert!(toggle_node_state(
            &mut st,
            target,
            Instant::now(),
            monitor.as_str(),
        ));
        if st
            .model
            .field
            .node(target)
            .is_some_and(|node| node.state != halley_core::field::NodeState::Node)
        {
            assert!(crate::compositor::workspace::state::finish_manual_collapse(
                &mut st,
                target,
                Instant::now(),
            ));
        }

        assert!(toggle_node_state(
            &mut st,
            target,
            Instant::now(),
            monitor.as_str(),
        ));

        assert!(
            st.model
                .workspace_state
                .maximize_sessions
                .contains_key(monitor.as_str())
        );
        let (target_pos, target_size) = maximize_target_for_monitor(&st, monitor.as_str());
        assert_eq!(st.model.field.node(target).expect("target").pos, target_pos);
        assert_eq!(
            st.model.field.node(target).expect("target").intrinsic_size,
            target_size
        );
    }

    #[test]
    fn maximize_toggle_is_blocked_for_active_cluster_workspace_members() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let master = st.model.field.spawn_surface(
            "master",
            halley_core::field::Vec2 { x: 100.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = st.model.field.spawn_surface(
            "stack",
            halley_core::field::Vec2 { x: 300.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(master, "monitor_a");
        st.assign_node_to_monitor(stack, "monitor_a");
        let cid = st.create_cluster(vec![master, stack]).expect("cluster");
        let core = st.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");
        assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

        assert!(!toggle_node_maximize_state(
            &mut st,
            master,
            Instant::now(),
            "monitor_a"
        ));
    }
}
