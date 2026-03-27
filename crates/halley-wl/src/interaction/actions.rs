use crate::state::Halley;
use eventline::info;
use halley_config::{ClickCollapsedOutsideFocusMode, ClickCollapsedPanMode};
use halley_core::decay::DecayLevel;
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
    let target_monitor = st.model.monitor_state
        .node_monitor
        .get(&node_id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    let focus_center = st.view_center_for_monitor(target_monitor.as_str());
    let focus_ring = st.focus_ring_for_monitor(target_monitor.as_str());

    let in_focus_ring = focus_ring.zone(focus_center, target_pos) == FocusZone::Inside;

    if in_focus_ring {
        // This is a deliberate promote, not a stale auto-resurrect.
        st.model.workspace_state.manual_collapsed_nodes.remove(&node_id);

        let _ = st.model.field.set_decay_level(node_id, DecayLevel::Hot);
        st.mark_active_transition(node_id, now, 360);

        st.set_interaction_focus(Some(node_id), 30_000, now);
        return true;
    }

    st.set_interaction_focus(Some(node_id), 30_000, now);
    st.set_pan_restore_focus_target(node_id);
    st.animate_viewport_center_to(target_pos, now)
}

pub(crate) fn activate_collapsed_node_from_click(
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

    let target_monitor = st.model.monitor_state
        .node_monitor
        .get(&node_id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    let focus_center = st.view_center_for_monitor(target_monitor.as_str());
    let focus_ring = st.focus_ring_for_monitor(target_monitor.as_str());
    let in_focus_ring = focus_ring.zone(focus_center, n.pos) == FocusZone::Inside;

    if in_focus_ring
        || st.runtime.tuning.click_collapsed_outside_focus == ClickCollapsedOutsideFocusMode::Ignore
    {
        return promote_node_level(st, node_id, now);
    }

    st.model.workspace_state.manual_collapsed_nodes.remove(&node_id);
    let _ = st.model.field.set_decay_level(node_id, DecayLevel::Hot);
    st.mark_active_transition(node_id, now, 360);
    st.set_interaction_focus(Some(node_id), 30_000, now);

    match st.runtime.tuning.click_collapsed_pan {
        ClickCollapsedPanMode::Never => false,
        ClickCollapsedPanMode::IfOffscreen => {
            if st.surface_is_sufficiently_visible_on_monitor(target_monitor.as_str(), node_id) {
                false
            } else {
                st.minimal_reveal_center_for_surface_on_monitor(target_monitor.as_str(), node_id)
                    .map(|target| st.animate_viewport_center_to(target, now))
                    .unwrap_or(false)
            }
        }
        ClickCollapsedPanMode::Always => st
            .minimal_reveal_center_for_surface_on_monitor(target_monitor.as_str(), node_id)
            .map(|target| st.animate_viewport_center_to(target, now))
            .unwrap_or(false),
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

    let target_monitor = st.model.monitor_state
        .node_monitor
        .get(&node_id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    if st.focused_monitor() != target_monitor {
        st.focus_monitor_view(target_monitor.as_str(), now);
    }

    match node.state {
        halley_core::field::NodeState::Node => promote_node_level(st, node_id, now),
        halley_core::field::NodeState::Active | halley_core::field::NodeState::Drifting => {
            st.set_interaction_focus(Some(node_id), 30_000, now);
            let panned = st
                .minimal_reveal_center_for_surface_on_monitor(target_monitor.as_str(), node_id)
                .map(|target| st.animate_viewport_center_to(target, now))
                .unwrap_or(false);
            panned || true
        }
        halley_core::field::NodeState::Core => false,
    }
}

pub(crate) fn latest_surface_node(st: &Halley) -> Option<halley_core::field::NodeId> {
    st.last_input_surface_node_for_monitor(st.focused_monitor())
        .or_else(|| st.last_input_surface_node())
        .or_else(|| {
            st.model.surface_to_node
                .values()
                .copied()
                .max_by_key(|id| id.as_u64())
        })
}

pub(crate) fn move_latest_node(st: &mut Halley, dx: f32, dy: f32) -> bool {
    let Some(id) = latest_surface_node(st) else {
        return false;
    };
    let Some(n) = st.model.field.node(id) else {
        return false;
    };
    let to = halley_core::field::Vec2 {
        x: n.pos.x + dx,
        y: n.pos.y + dy,
    };
    let _ = st.model.field.set_pinned(id, false);
    st.begin_carry_state_tracking(id);
    if st.carry_surface_non_overlap(id, to, false) {
        st.update_carry_state_preview(id, Instant::now());
        st.end_carry_state_tracking(id);
        st.set_interaction_focus(Some(id), 30_000, Instant::now());
        if let Some(nn) = st.model.field.node(id) {
            info!(
                "moved node id={} to ({:.0},{:.0}) state={:?}",
                id.as_u64(),
                nn.pos.x,
                nn.pos.y,
                nn.state
            );
        }
        return true;
    }
    st.end_carry_state_tracking(id);
    false
}

pub(crate) fn move_latest_node_direction(st: &mut Halley, direction: NodeMoveDirection) -> bool {
    const STEP_NODE: f32 = 80.0;

    match direction {
        NodeMoveDirection::Left => move_latest_node(st, -STEP_NODE, 0.0),
        NodeMoveDirection::Right => move_latest_node(st, STEP_NODE, 0.0),
        NodeMoveDirection::Up => move_latest_node(st, 0.0, STEP_NODE),
        NodeMoveDirection::Down => move_latest_node(st, 0.0, -STEP_NODE),
    }
}

pub(crate) fn step_window_trail(st: &mut Halley, direction: TrailDirection) -> bool {
    st.navigate_window_trail(direction, Instant::now())
}

pub(crate) fn toggle_focused_active_node_state(st: &mut Halley) -> bool {
    let now = Instant::now();

    let Some(id) = st
        .last_focused_surface_node_for_monitor(st.focused_monitor())
        .or_else(|| st.last_focused_surface_node())
    else {
        return false;
    };

    let Some(n) = st.model.field.node(id) else {
        return false;
    };

    if n.kind != halley_core::field::NodeKind::Surface {
        return false;
    }

    match n.state {
        halley_core::field::NodeState::Active => {
            let _ = st.model.field.set_state(id, halley_core::field::NodeState::Node);
            let _ = st.model.field
                .set_decay_level(id, halley_core::decay::DecayLevel::Cold);
            st.model.spawn_state.pending_spawn_activate_at_ms.remove(&id);
            st.model.workspace_state.manual_collapsed_nodes.insert(id);

            st.set_interaction_focus(None, 0, now);
            st.model.focus_state.pan_restore_active_focus = None;
            true
        }

        halley_core::field::NodeState::Node => {
            st.model.workspace_state.manual_collapsed_nodes.remove(&id);
            let _ = st.model.field
                .set_decay_level(id, halley_core::decay::DecayLevel::Hot);
            st.model.spawn_state.pending_spawn_activate_at_ms.remove(&id);
            st.mark_active_transition(id, now, 360);

            st.set_interaction_focus(Some(id), 30_000, now);
            true
        }

        _ => false,
    }
}
