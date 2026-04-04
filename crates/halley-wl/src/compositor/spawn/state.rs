use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use halley_config::{
    InitialWindowClusterParticipation, InitialWindowOverlapPolicy, InitialWindowSpawnPlacement,
};
use halley_core::decay::DecayLevel;
use halley_core::field::{NodeId, Vec2};

use super::Halley;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpawnFrontierPoint {
    pub pos: Vec2,
    pub score: f32,
    pub dir: Vec2,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct SpawnPatch {
    pub anchor: Vec2,
    pub focus_node: Option<NodeId>,
    pub focus_pos: Vec2,
    pub growth_dir: Vec2,
    pub placements_in_patch: u32,
    pub frontier: Vec<SpawnFrontierPoint>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingSpawnPan {
    pub node_id: NodeId,
    pub target_center: Vec2,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ActiveSpawnPan {
    pub node_id: NodeId,
    pub pan_start_at_ms: u64,
    pub reveal_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AppliedInitialWindowRule {
    pub(crate) overlap_policy: InitialWindowOverlapPolicy,
    pub(crate) spawn_placement: InitialWindowSpawnPlacement,
    pub(crate) cluster_participation: InitialWindowClusterParticipation,
    pub(crate) parent_node: Option<NodeId>,
    pub(crate) suppress_reveal_pan: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SpawnAnchorMode {
    Focus,
    View,
}

#[derive(Clone, Debug)]
pub(crate) struct MonitorSpawnState {
    pub(crate) spawn_cursor: u32,
    pub(crate) spawn_patch: Option<SpawnPatch>,
    pub(crate) spawn_anchor_mode: SpawnAnchorMode,
    pub(crate) spawn_view_anchor: Vec2,
    pub(crate) spawn_pan_start_center: Option<Vec2>,
    pub(crate) spawn_last_pan_ms: u64,
}

impl MonitorSpawnState {
    pub(crate) fn new(view_anchor: Vec2) -> Self {
        Self {
            spawn_cursor: 0,
            spawn_patch: None,
            spawn_anchor_mode: SpawnAnchorMode::Focus,
            spawn_view_anchor: view_anchor,
            spawn_pan_start_center: None,
            spawn_last_pan_ms: 0,
        }
    }
}

pub(crate) struct SpawnState {
    pub pending_spawn_activate_at_ms: HashMap<NodeId, u64>,
    pub(crate) pending_tiled_insert_reveal_at_ms: HashMap<NodeId, u64>,
    pub(crate) pending_tiled_insert_preserve_focus: HashSet<NodeId>,
    pub(crate) pending_spawn_monitor: Option<String>,
    pub(crate) per_monitor: HashMap<String, MonitorSpawnState>,
    pub(crate) pending_spawn_pan_queue: VecDeque<PendingSpawnPan>,
    pub(crate) active_spawn_pan: Option<ActiveSpawnPan>,
    pub(crate) applied_window_rules: HashMap<NodeId, AppliedInitialWindowRule>,
    pub(crate) pending_rule_rechecks: HashSet<NodeId>,
    pub(crate) pending_initial_reveal: HashSet<NodeId>,
}

pub(crate) fn is_persistent_rule_top(st: &Halley, node_id: NodeId) -> bool {
    st.model
        .spawn_state
        .applied_window_rules
        .contains_key(&node_id)
}

pub(crate) fn default_spawn_view_anchor_for_monitor(st: &Halley, monitor: &str) -> Vec2 {
    st.model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| space.viewport.center)
        .unwrap_or(st.model.viewport.center)
}

pub(crate) fn spawn_monitor_state(st: &Halley, monitor: &str) -> MonitorSpawnState {
    st.model
        .spawn_state
        .per_monitor
        .get(monitor)
        .cloned()
        .unwrap_or_else(|| {
            MonitorSpawnState::new(default_spawn_view_anchor_for_monitor(st, monitor))
        })
}

pub(crate) fn spawn_monitor_state_mut<'a>(
    st: &'a mut Halley,
    monitor: &str,
) -> &'a mut MonitorSpawnState {
    let view_anchor = default_spawn_view_anchor_for_monitor(st, monitor);
    st.model
        .spawn_state
        .per_monitor
        .entry(monitor.to_string())
        .or_insert_with(|| MonitorSpawnState::new(view_anchor))
}

pub(crate) fn process_pending_spawn_activations(st: &mut Halley, now: Instant, now_ms: u64) {
    let due_tiled_reveals: Vec<NodeId> = st
        .model
        .spawn_state
        .pending_tiled_insert_reveal_at_ms
        .iter()
        .filter_map(|(&id, &at)| (now_ms >= at).then_some(id))
        .collect();

    for id in due_tiled_reveals {
        if !st.ui.render_state.window_geometry.contains_key(&id) {
            continue;
        }
        let preserve_focus = st
            .model
            .spawn_state
            .pending_tiled_insert_preserve_focus
            .remove(&id);
        st.model
            .spawn_state
            .pending_tiled_insert_reveal_at_ms
            .remove(&id);
        st.ui.render_state.cluster_tile_entry_pending.insert(id);
        let monitor = st
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
        if st
            .active_cluster_workspace_for_monitor(monitor.as_str())
            .is_some()
        {
            st.layout_active_cluster_workspace_for_monitor(monitor.as_str(), now_ms);
            let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
            st.set_recent_top_node(id, now + std::time::Duration::from_millis(1200));
            crate::compositor::workspace::state::mark_active_transition(st, id, now, 620);
            if !preserve_focus {
                st.set_interaction_focus(Some(id), 30_000, now);
            }
            st.request_maintenance();
        }
    }

    let due: Vec<NodeId> = st
        .model
        .spawn_state
        .pending_spawn_activate_at_ms
        .iter()
        .filter_map(|(&id, &at)| (now_ms >= at).then_some(id))
        .collect();

    for id in due {
        st.model
            .spawn_state
            .pending_spawn_activate_at_ms
            .remove(&id);
        if !st.model.field.is_visible(id) {
            continue;
        }
        let Some(n) = st.model.field.node(id) else {
            continue;
        };
        if n.kind != halley_core::field::NodeKind::Surface {
            continue;
        }
        if crate::compositor::workspace::state::preserve_collapsed_surface(st, id) {
            continue;
        }
        let node_monitor = st
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
        let cluster_local = st
            .active_cluster_workspace_for_monitor(node_monitor.as_str())
            .is_some();
        let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
        if let Some((_, _, w, h)) = st.ui.render_state.window_geometry.get(&id) {
            st.model
                .workspace_state
                .last_active_size
                .insert(id, Vec2 { x: *w, y: *h });
        }
        crate::compositor::workspace::state::mark_active_transition(st, id, now, 620);
        if !cluster_local {
            st.record_focus_trail_visit(id);
            st.model.focus_state.suppress_trail_record_once = true;
        }
        st.set_interaction_focus(Some(id), 30_000, now);
    }
}
