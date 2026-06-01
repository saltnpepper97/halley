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
        let _ = st.raise_overlap_policy_node(node_id);
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
        let _ = st.raise_overlap_policy_node(node_id);
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
            let _ = st.raise_overlap_policy_node(node_id);
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

fn maximize_viewport_for_monitor(st: &Halley, monitor: &str) -> halley_core::viewport::Viewport {
    st.model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| space.usable_viewport)
        .unwrap_or(st.model.viewport)
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
    let viewport = maximize_viewport_for_monitor(st, monitor);
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
    crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(st);
    for (node_id, snapshot) in &node_snapshots {
        let from =
            crate::compositor::workspace::state::maximized_visual_for_node_on_current_monitor_at(
                st, *node_id, now,
            )
            .unwrap_or_else(|| maximize_target_for_monitor(st, monitor));
        if let Some(node) = st.model.field.node_mut(*node_id) {
            node.pos = snapshot.pos;
            node.intrinsic_size = snapshot.size;
        }
        let _ = st.model.field.sync_active_footprint_to_intrinsic(*node_id);
        let _ = st.model.field.set_pinned(*node_id, snapshot.pinned);
        st.request_toplevel_resize(
            *node_id,
            snapshot.size.x.round() as i32,
            snapshot.size.y.round() as i32,
        );
        st.set_last_active_size_now(*node_id, snapshot.size);
        if st.runtime.tuning.maximize_animation_enabled() {
            st.model.workspace_state.maximize_animation.insert(
                *node_id,
                crate::compositor::workspace::state::MaximizeAnimation {
                    monitor: monitor.to_string(),
                    from_pos: from.0,
                    to_pos: snapshot.pos,
                    from_size: from.1,
                    to_size: snapshot.size,
                    start_ms: st.now_ms(now),
                    duration_ms: st.runtime.tuning.maximize_animation_duration_ms(),
                },
            );
        }
    }
    if !st.runtime.tuning.maximize_animation_enabled() {
        st.model.workspace_state.maximize_sessions.remove(monitor);
    }
    st.request_maintenance();
    true
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
    crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(st);

    let _ = node_snapshots;
    let (target_pos, target_size) = maximize_target_for_monitor(st, monitor);
    let from =
        crate::compositor::workspace::state::maximize_animation_visual_for_node_on_monitor_at(
            st, target_id, monitor, now,
        )
        .or_else(|| {
            st.model.field.node(target_id).map(|node| {
                (
                    node.pos,
                    current_window_size_for_node(st, target_id).unwrap_or(node.intrinsic_size),
                )
            })
        })
        .unwrap_or((target_pos, target_size));
    st.request_toplevel_resize(
        target_id,
        target_size.x.round() as i32,
        target_size.y.round() as i32,
    );
    if st.runtime.tuning.maximize_animation_enabled() {
        st.model.workspace_state.maximize_animation.insert(
            target_id,
            crate::compositor::workspace::state::MaximizeAnimation {
                monitor: monitor.to_string(),
                from_pos: from.0,
                to_pos: target_pos,
                from_size: from.1,
                to_size: target_size,
                start_ms: st.now_ms(now),
                duration_ms: st.runtime.tuning.maximize_animation_duration_ms(),
            },
        );
    }
    st.set_recent_top_node(target_id, now + std::time::Duration::from_millis(1200));
    st.request_maintenance();
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
                    crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(st);
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

    st.model.workspace_state.maximize_sessions.insert(
        monitor.to_string(),
        crate::compositor::workspace::state::MaximizeSession {
            target_id: id,
            node_snapshots: node_snapshots.clone(),
            camera,
            state: crate::compositor::workspace::state::MaximizeSessionState::Active,
        },
    );
    crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(st);

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
    if st.node_user_pinned(id) {
        return false;
    }
    if crate::compositor::surface::is_active_cluster_workspace_member(st, id)
        || st.is_fullscreen_active(id)
    {
        return false;
    }

    let maximize_resume_monitor =
        crate::compositor::workspace::state::take_maximize_resume_for_node(st, id);
    let monitor = maximize_resume_monitor.unwrap_or_else(|| {
        st.model
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| focused_monitor.to_string())
    });

    if node.state == halley_core::field::NodeState::Node {
        uncollapse_surface_node_for_action(st, id, now);
    }

    st.set_interaction_focus(Some(id), 30_000, now);
    start_maximize_session(st, id, monitor.as_str(), now)
}

fn uncollapse_surface_node_for_action(
    st: &mut Halley,
    id: halley_core::field::NodeId,
    now: Instant,
) {
    st.model.workspace_state.manual_collapsed_nodes.remove(&id);
    st.model.workspace_state.pending_collapses.remove(&id);
    let _ = st
        .model
        .field
        .set_decay_level(id, halley_core::decay::DecayLevel::Hot);
    let _ = st.raise_overlap_policy_node(id);
    st.model
        .spawn_state
        .pending_spawn_activate_at_ms
        .remove(&id);
    crate::compositor::workspace::state::mark_active_transition(st, id, now, 360);
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
            if st.is_fullscreen_session_node(id) {
                return false;
            }
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
            uncollapse_surface_node_for_action(st, id, now);
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

    fn two_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.animations.maximize.enabled = false;
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
    fn toggle_node_state_does_not_collapse_fullscreen_surface() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let id = st.model.field.spawn_surface(
            "game",
            halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            halley_core::field::Vec2 { x: 800.0, y: 600.0 },
        );
        st.assign_node_to_monitor(id, "monitor_a");
        st.model
            .fullscreen_state
            .fullscreen_active_node
            .insert("monitor_a".to_string(), id);

        assert!(!toggle_node_state(&mut st, id, Instant::now(), "monitor_a"));
        assert_eq!(
            st.model.field.node(id).map(|n| n.state.clone()),
            Some(halley_core::field::NodeState::Active)
        );
        assert!(
            !st.model
                .workspace_state
                .manual_collapsed_nodes
                .contains(&id)
        );
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
    fn pinned_surface_cannot_toggle_maximize() {
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
        assert!(!toggle_node_maximize_state(
            &mut st,
            id,
            Instant::now(),
            "monitor_a"
        ));

        let node = st.model.field.node(id).expect("node");
        assert_eq!(node.pos, halley_core::field::Vec2 { x: 120.0, y: 140.0 });
        assert_eq!(
            node.intrinsic_size,
            halley_core::field::Vec2 { x: 320.0, y: 240.0 }
        );
        assert!(st.node_user_pinned(id));
        assert!(node.pinned);
    }

    #[test]
    fn opening_collapsed_surface_raises_it_above_existing_active_window() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let opened = st.model.field.spawn_surface(
            "opened",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 300.0, y: 200.0 },
        );
        let existing = st.model.field.spawn_surface(
            "existing",
            halley_core::field::Vec2 { x: 40.0, y: 40.0 },
            halley_core::field::Vec2 { x: 300.0, y: 200.0 },
        );
        st.assign_node_to_monitor(opened, "monitor_a");
        st.assign_node_to_monitor(existing, "monitor_a");
        let _ = st
            .model
            .field
            .set_state(opened, halley_core::field::NodeState::Node);
        let _ = st.raise_overlap_policy_node(existing);

        assert!(toggle_node_state(
            &mut st,
            opened,
            Instant::now(),
            "monitor_a"
        ));

        assert!(st.overlap_policy_stack_rank(opened) > st.overlap_policy_stack_rank(existing));
    }

    #[test]
    fn maximize_toggle_uncollapses_surface_node_before_maximizing() {
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
        assert!(
            st.model
                .field
                .set_decay_level(id, halley_core::decay::DecayLevel::Cold)
        );
        st.model.workspace_state.manual_collapsed_nodes.insert(id);

        assert!(toggle_node_maximize_state(
            &mut st,
            id,
            Instant::now(),
            "monitor_a"
        ));

        let node = st.model.field.node(id).expect("node");
        assert_eq!(node.state, halley_core::field::NodeState::Active);
        assert_eq!(node.pos, halley_core::field::Vec2 { x: 120.0, y: 140.0 });
        assert_eq!(
            node.intrinsic_size,
            halley_core::field::Vec2 { x: 320.0, y: 240.0 }
        );
        assert!(
            !st.model
                .workspace_state
                .manual_collapsed_nodes
                .contains(&id)
        );
        assert!(
            st.model
                .workspace_state
                .maximize_sessions
                .contains_key("monitor_a")
        );
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
    fn maximize_targets_reserved_usable_viewport() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let usable = halley_core::viewport::Viewport::new(
            halley_core::field::Vec2 { x: 400.0, y: 320.0 },
            halley_core::field::Vec2 { x: 800.0, y: 560.0 },
        );
        st.model
            .monitor_state
            .monitors
            .get_mut("monitor_a")
            .expect("monitor")
            .usable_viewport = usable;

        let (pos, size) = maximize_target_for_monitor(&st, "monitor_a");
        let inset =
            st.non_overlap_gap_world() + active_window_frame_pad_px(&st.runtime.tuning) as f32;

        assert_eq!(pos, usable.center);
        assert_eq!(
            size,
            halley_core::field::Vec2 {
                x: 800.0 - inset * 2.0,
                y: 560.0 - inset * 2.0,
            }
        );
    }

    #[test]
    fn maximize_toggle_saves_restore_geometry_and_centers_target() {
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
        let (target_pos, target_size) = maximize_target_for_monitor(&st, "monitor_a");

        assert_eq!(session.target_id, id);
        assert_eq!(restore.pos, halley_core::field::Vec2 { x: 120.0, y: 140.0 });
        assert_eq!(
            restore.size,
            halley_core::field::Vec2 { x: 320.0, y: 240.0 }
        );
        assert!(st.model.workspace_state.maximize_animation.is_empty());
        assert_eq!(st.model.field.node(id).expect("node").pos, restore.pos);
        assert_eq!(
            st.model.field.node(id).expect("node").intrinsic_size,
            restore.size
        );
        assert_eq!(
            crate::compositor::workspace::state::maximized_visual_for_node_on_current_monitor(
                &st, id
            ),
            Some((target_pos, target_size))
        );
    }

    #[test]
    fn maximize_animation_is_visual_only_and_preserves_field_geometry() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.animations.maximize.enabled = true;
        tuning.animations.maximize.duration_ms = 240;
        let mut st = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();
        let id = st.model.field.spawn_surface(
            "app",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(id, "monitor_a");

        assert!(toggle_node_maximize_state(&mut st, id, now, "monitor_a"));

        let node = st.model.field.node(id).expect("node");
        assert_eq!(node.pos, halley_core::field::Vec2 { x: 120.0, y: 140.0 });
        assert_eq!(
            node.intrinsic_size,
            halley_core::field::Vec2 { x: 320.0, y: 240.0 }
        );
        assert!(
            st.model
                .workspace_state
                .maximize_animation
                .contains_key(&id)
        );
        assert_eq!(
            crate::compositor::workspace::state::maximized_visual_for_node_on_current_monitor_at(
                &st, id, now
            ),
            Some((node.pos, node.intrinsic_size))
        );

        crate::compositor::workspace::state::tick_maximize_animation(
            &mut st,
            now + std::time::Duration::from_millis(260),
        );
        let node = st.model.field.node(id).expect("node");
        assert_eq!(node.pos, halley_core::field::Vec2 { x: 120.0, y: 140.0 });
        assert_eq!(
            node.intrinsic_size,
            halley_core::field::Vec2 { x: 320.0, y: 240.0 }
        );
        assert!(st.model.workspace_state.maximize_animation.is_empty());
    }

    #[test]
    fn maximize_session_tracks_target_only_and_leaves_bystanders() {
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
        assert_eq!(
            session.camera.center,
            halley_core::field::Vec2 { x: 430.0, y: 280.0 }
        );
        assert_eq!(
            session.camera.view_size,
            halley_core::field::Vec2 { x: 500.0, y: 375.0 }
        );
        assert!(session.node_snapshots.contains_key(&target));
        assert!(!session.node_snapshots.contains_key(&bystander));
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
        assert_eq!(st.model.camera_target_view_size, st.model.viewport.size);
        assert_eq!(st.model.camera_target_center, st.model.viewport.center);
    }

    #[test]
    fn unmaximize_restores_target_and_leaves_bystanders() {
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
    fn moving_external_active_window_to_maximized_monitor_keeps_maximize() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, two_monitor_tuning());

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
        let moved = st.model.field.spawn_surface(
            "moved",
            halley_core::field::Vec2 { x: -420.0, y: 0.0 },
            halley_core::field::Vec2 { x: 300.0, y: 180.0 },
        );
        st.assign_node_to_monitor(target, "right");
        st.assign_node_to_monitor(bystander, "right");
        st.assign_node_to_monitor(moved, "left");

        assert!(toggle_node_maximize_state(
            &mut st,
            target,
            Instant::now(),
            "right"
        ));
        assert!(
            st.model
                .workspace_state
                .maximize_sessions
                .contains_key("right")
        );

        st.assign_node_to_monitor(moved, "right");

        assert!(
            st.model
                .workspace_state
                .maximize_sessions
                .contains_key("right")
        );
        assert_eq!(
            st.model.field.node(target).expect("target").pos,
            halley_core::field::Vec2 { x: 120.0, y: 140.0 }
        );
        assert_eq!(
            st.model.field.node(bystander).expect("bystander").pos,
            halley_core::field::Vec2 { x: 460.0, y: 260.0 }
        );
        assert_eq!(
            st.model
                .monitor_state
                .node_monitor
                .get(&moved)
                .map(String::as_str),
            Some("right")
        );
    }

    #[test]
    fn assigning_maximize_session_member_does_not_abort_session() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, two_monitor_tuning());

        let target = st.model.field.spawn_surface(
            "target",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(target, "right");

        assert!(toggle_node_maximize_state(
            &mut st,
            target,
            Instant::now(),
            "right"
        ));
        st.assign_node_to_monitor(target, "right");

        assert!(
            st.model
                .workspace_state
                .maximize_sessions
                .contains_key("right")
        );
    }

    #[test]
    fn unmaximize_restores_camera_via_smooth_target() {
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
            !st.model
                .workspace_state
                .maximize_sessions
                .contains_key("monitor_a")
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
    fn manual_collapse_places_node_out_from_under_active_window() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let monitor = st.model.monitor_state.current_monitor.clone();
        let blocker = st.model.field.spawn_surface(
            "blocker",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 420.0, y: 280.0 },
        );
        let target = st.model.field.spawn_surface(
            "target",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 320.0, y: 220.0 },
        );
        st.assign_node_to_monitor(blocker, monitor.as_str());
        st.assign_node_to_monitor(target, monitor.as_str());
        let blocker_pos = st.model.field.node(blocker).expect("blocker").pos;
        let target_pos = st.model.field.node(target).expect("target").pos;

        assert!(crate::compositor::workspace::state::finish_manual_collapse(
            &mut st,
            target,
            Instant::now(),
        ));

        assert_eq!(
            st.model.field.node(blocker).expect("blocker").pos,
            blocker_pos
        );
        assert_ne!(st.model.field.node(target).expect("target").pos, target_pos);
        assert!(
            st.ui
                .render_state
                .landmark_slide_animations
                .contains_key(&target)
        );
        assert_eq!(
            st.model.field.node(target).expect("target").state,
            halley_core::field::NodeState::Node
        );
    }

    #[test]
    fn pending_manual_collapse_slides_from_original_active_position() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.animations.window_close.enabled = false;
        let mut st = Halley::new_for_test(&dh, tuning);
        let monitor = st.model.monitor_state.current_monitor.clone();
        let blocker = st.model.field.spawn_surface(
            "blocker",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 420.0, y: 280.0 },
        );
        let target = st.model.field.spawn_surface(
            "target",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 320.0, y: 220.0 },
        );
        st.assign_node_to_monitor(blocker, monitor.as_str());
        st.assign_node_to_monitor(target, monitor.as_str());
        let origin = st.model.field.node(target).expect("target").pos;
        let now = Instant::now();

        assert!(toggle_node_state(&mut st, target, now, monitor.as_str()));
        assert!(
            st.model
                .workspace_state
                .pending_collapses
                .contains_key(&target)
        );

        crate::compositor::workspace::state::process_pending_collapses_for_monitor(
            &mut st,
            monitor.as_str(),
            now + std::time::Duration::from_millis(140),
        );

        let resolved = st.model.field.node(target).expect("target").pos;
        assert_ne!(resolved, origin);
        let slide = st
            .ui
            .render_state
            .landmark_slide_animations
            .get(&target)
            .expect("landmark slide animation");
        assert_eq!(slide.from, origin);
        assert_eq!(slide.to, resolved);
        assert_eq!(
            st.model.field.node(target).expect("target").state,
            halley_core::field::NodeState::Node
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
        assert!(
            crate::compositor::workspace::state::maximized_visual_for_node_on_current_monitor(
                &st, target
            )
            .is_some()
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
