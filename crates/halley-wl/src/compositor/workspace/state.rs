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
