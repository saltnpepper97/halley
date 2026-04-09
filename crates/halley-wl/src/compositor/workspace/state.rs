use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_core::field::{NodeId, NodeKind, NodeState, Vec2};

use crate::compositor::root::Halley;

pub(crate) struct WorkspaceState {
    pub(crate) last_active_size: HashMap<NodeId, Vec2>,
    pub(crate) active_transition_until_ms: HashMap<NodeId, u64>,
    pub(crate) primary_promote_cooldown_until_ms: HashMap<NodeId, u64>,
    pub(crate) manual_collapsed_nodes: HashSet<NodeId>,
}

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

pub(crate) fn start_active_to_node_close_animation(st: &mut Halley, id: NodeId, now: Instant) {
    if !st.runtime.tuning.window_close_animation_enabled() {
        return;
    }
    let Some(node) = st.model.field.node(id) else {
        return;
    };
    if node.kind != NodeKind::Surface || node.state != NodeState::Active {
        return;
    }
    let Some(monitor) = st.model.monitor_state.node_monitor.get(&id).cloned() else {
        return;
    };
    let duration_ms = st.runtime.tuning.window_close_duration_ms();
    let style = st.runtime.tuning.window_close_style();
    let Some((border_rect, offscreen_textures)) =
        crate::render::capture_closing_window_animation(st, monitor.as_str(), id)
    else {
        return;
    };

    st.ui.render_state.start_closing_window_animation(
        id,
        monitor.as_str(),
        now,
        duration_ms,
        style,
        border_rect,
        offscreen_textures,
    );
    st.ui
        .render_state
        .animator
        .snap_to_state(id, NodeState::Node, now);
    st.request_maintenance();
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
