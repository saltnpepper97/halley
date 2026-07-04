use super::*;
use crate::compositor::clusters::read::{
    ClusterLayoutPlan, ClusterTilePlacement, EnterClusterWorkspacePlan, ExitClusterWorkspacePlan,
};
use crate::compositor::clusters::state::ClusterState;
use crate::compositor::interaction::state::InteractionState;
use crate::overlay::OverlayActionHint;
use halley_config::{ClusterDefaultLayout, DirectionalAction};
use halley_core::cluster::{ClusterId, ClusterRemoveMemberOutcome};
use halley_core::cluster_layout::{ClusterCycleDirection, ClusterWorkspaceLayoutKind};
use halley_core::field::RemoveNodeClusterEffect;

fn cluster_mode_selected_nodes_for_monitor_mut<'a>(
    st: &'a mut Halley,
    monitor: &str,
) -> &'a mut std::collections::HashSet<NodeId> {
    st.model
        .cluster_state
        .cluster_mode_selected_nodes
        .entry(monitor.to_string())
        .or_default()
}

fn open_cluster_bloom_for_monitor_inner(st: &mut Halley, monitor: &str, cid: ClusterId) -> bool {
    let Some(cluster) = st.model.field.cluster(cid) else {
        return false;
    };
    let Some(core_id) = cluster.core else {
        return false;
    };
    st.model
        .cluster_state
        .cluster_bloom_open
        .retain(|name, open_cid| *open_cid != cid || name == monitor);
    let _ = close_cluster_bloom_for_monitor_inner(st, monitor);
    let _ = st.model.field.set_pinned(core_id, true);
    st.input.interaction_state.physics_velocity.remove(&core_id);
    st.model
        .cluster_state
        .cluster_bloom_open
        .insert(monitor.to_string(), cid);
    true
}

fn close_cluster_bloom_for_monitor_inner(st: &mut Halley, monitor: &str) -> bool {
    let Some(cid) = st.model.cluster_state.cluster_bloom_open.remove(monitor) else {
        return false;
    };
    if let Some(core_id) = st.model.field.cluster(cid).and_then(|cluster| cluster.core) {
        let _ = st.model.field.set_pinned(core_id, false);
    }
    true
}

fn enter_cluster_mode_inner(st: &mut Halley, monitor: &str) -> bool {
    if st
        .model
        .cluster_state
        .cluster_mode_selected_nodes
        .contains_key(monitor)
    {
        return true;
    }
    st.model
        .cluster_state
        .cluster_mode_selected_nodes
        .insert(monitor.to_string(), std::collections::HashSet::new());
    true
}

fn exit_cluster_mode_inner(st: &mut Halley, monitor: &str) -> bool {
    if !st
        .model
        .cluster_state
        .cluster_mode_selected_nodes
        .contains_key(monitor)
    {
        return false;
    }
    st.model
        .cluster_state
        .cluster_mode_selected_nodes
        .remove(monitor);
    true
}

fn toggle_cluster_mode_selection_inner(st: &mut Halley, monitor: &str, node_id: NodeId) -> bool {
    if !st
        .model
        .cluster_state
        .cluster_mode_selected_nodes
        .contains_key(monitor)
    {
        return false;
    }
    let Some(node) = st.model.field.node(node_id) else {
        return false;
    };
    if node.kind != halley_core::field::NodeKind::Surface
        || node.state == halley_core::field::NodeState::Core
        || !st.model.field.is_visible(node_id)
    {
        return false;
    }
    if !cluster_mode_selected_nodes_for_monitor_mut(st, monitor).insert(node_id) {
        cluster_mode_selected_nodes_for_monitor_mut(st, monitor).remove(&node_id);
    }
    true
}

fn detach_member_from_cluster_inner(
    st: &mut Halley,
    cid: ClusterId,
    member_id: NodeId,
    world_pos: Vec2,
    now_ms: u64,
) -> Option<ClusterRemoveMemberOutcome> {
    let was_active = st
        .model
        .field
        .cluster(cid)
        .is_some_and(|cluster| cluster.is_active());
    let outcome = st.model.field.remove_member_from_cluster(cid, member_id)?;
    if matches!(outcome, ClusterRemoveMemberOutcome::Removed) && was_active {
        let _ = st
            .model
            .field
            .move_member_out_of_active_cluster_workspace(cid, member_id);
    }
    let _ = st.model.field.set_detached(member_id, false);
    let _ = st
        .model
        .field
        .set_state(member_id, halley_core::field::NodeState::Active);
    if let Some(node) = st.model.field.node_mut(member_id) {
        node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
        node.pos = world_pos;
    }
    let _ = st.model.field.touch(member_id, now_ms);
    Some(outcome)
}

fn absorb_node_into_cluster_inner(st: &mut Halley, cid: ClusterId, node_id: NodeId) -> bool {
    let active_workspace = st
        .model
        .field
        .cluster(cid)
        .is_some_and(|cluster| cluster.is_active());
    let add_result = if matches!(
        st.runtime.tuning.cluster_layout_kind(),
        ClusterWorkspaceLayoutKind::Stacking
    ) {
        st.model.field.add_member_to_cluster_front(cid, node_id)
    } else {
        st.model.field.add_member_to_cluster(cid, node_id)
    };
    if add_result.is_err() {
        return false;
    }
    if st.runtime.tuning.tile_new_on_top
        && matches!(
            st.runtime.tuning.cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Tiling
        )
    {
        let _ = st
            .model
            .field
            .promote_cluster_member_to_master(cid, node_id);
    }
    if active_workspace {
        if !st
            .model
            .field
            .move_member_into_active_cluster_workspace(cid, node_id)
        {
            return false;
        }
        if let Some(cluster) = st.model.field.cluster_mut(cid)
            && let Some(node) = cluster.workspace_member_mut(node_id)
        {
            node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
            node.visibility.set(Visibility::DETACHED, false);
            node.state = halley_core::field::NodeState::Active;
        }
    } else {
        let _ = st
            .model
            .field
            .set_state(node_id, halley_core::field::NodeState::Node);
        if let Some(node) = st.model.field.node_mut(node_id) {
            node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, true);
        }
        let _ = st.model.field.set_detached(node_id, false);
    }
    true
}

fn rect_center(rect: halley_core::tiling::Rect) -> Vec2 {
    Vec2 {
        x: rect.x + rect.w * 0.5,
        y: rect.y + rect.h * 0.5,
    }
}

fn range_gap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    if a1 < b0 {
        b0 - a1
    } else if b1 < a0 {
        a0 - b1
    } else {
        0.0
    }
}

pub(crate) fn directional_candidate_score(
    current: halley_core::tiling::Rect,
    candidate: halley_core::tiling::Rect,
    direction: DirectionalAction,
) -> Option<(f32, f32, f32, f32)> {
    let current_center = rect_center(current);
    let candidate_center = rect_center(candidate);
    let (main_delta, orth_gap, tie_axis) = match direction {
        DirectionalAction::Left => (
            current_center.x - candidate_center.x,
            range_gap(
                current.y,
                current.y + current.h,
                candidate.y,
                candidate.y + candidate.h,
            ),
            candidate_center.y,
        ),
        DirectionalAction::Right => (
            candidate_center.x - current_center.x,
            range_gap(
                current.y,
                current.y + current.h,
                candidate.y,
                candidate.y + candidate.h,
            ),
            candidate_center.y,
        ),
        DirectionalAction::Up => (
            current_center.y - candidate_center.y,
            range_gap(
                current.x,
                current.x + current.w,
                candidate.x,
                candidate.x + candidate.w,
            ),
            candidate_center.x,
        ),
        DirectionalAction::Down => (
            candidate_center.y - current_center.y,
            range_gap(
                current.x,
                current.x + current.w,
                candidate.x,
                candidate.x + candidate.w,
            ),
            candidate_center.x,
        ),
    };
    if main_delta <= 0.5 {
        return None;
    }
    let dx = candidate_center.x - current_center.x;
    let dy = candidate_center.y - current_center.y;
    Some((orth_gap, main_delta, dx * dx + dy * dy, tie_axis))
}

mod mode;
mod mutation;
mod naming;
mod navigation;
mod overflow;
mod workspace;

pub(crate) use mode::*;
pub(crate) use mutation::*;
pub(crate) use naming::*;
pub(crate) use navigation::*;
pub(crate) use overflow::*;
pub(crate) use workspace::*;

#[cfg(test)]
mod tests;

pub(crate) fn active_cluster_workspace_for_monitor(
    st: &Halley,
    monitor: &str,
) -> Option<ClusterId> {
    st.model
        .cluster_state
        .active_cluster_workspaces
        .get(monitor)
        .copied()
}

pub(crate) const CLUSTER_OVERFLOW_VISIBLE_SLOTS: usize = 15;

pub(crate) fn preferred_monitor_for_cluster(
    st: &Halley,
    cid: ClusterId,
    preferred: Option<&str>,
) -> Option<String> {
    crate::compositor::clusters::read::preferred_monitor_for_cluster(st, cid, preferred)
}

pub(crate) fn cluster_overflow_rect_for_monitor(
    st: &Halley,
    monitor: &str,
) -> Option<halley_core::tiling::Rect> {
    st.model
        .cluster_state
        .cluster_overflow_rects
        .get(monitor)
        .copied()
}

pub(crate) fn cluster_overflow_slot_rect_for_monitor(
    st: &Halley,
    monitor: &str,
    overflow_len: usize,
    slot_index: usize,
) -> Option<halley_core::tiling::Rect> {
    crate::compositor::clusters::read::overflow_strip_slot_rect_for_monitor(
        st,
        monitor,
        overflow_len,
        slot_index,
    )
}

pub(crate) fn active_cluster_tile_rect_for_member(
    st: &Halley,
    monitor: &str,
    member_id: NodeId,
) -> Option<halley_core::tiling::Rect> {
    crate::compositor::clusters::read::plan_active_cluster_layout(st, monitor)?
        .tiles
        .into_iter()
        .find(|tile| tile.node_id == member_id)
        .map(|tile| tile.rect)
}

pub(crate) fn cluster_spawn_rect_for_new_member(
    st: &Halley,
    monitor: &str,
    cid: ClusterId,
) -> Option<halley_core::tiling::Rect> {
    crate::compositor::clusters::read::cluster_spawn_rect_for_new_member(st, monitor, cid)
}

pub(crate) fn stack_layout_rects_for_members(
    st: &Halley,
    monitor: &str,
    members: &[NodeId],
) -> Option<std::collections::HashMap<NodeId, halley_core::tiling::Rect>> {
    crate::compositor::clusters::read::stack_layout_rects_for_members(st, monitor, members)
}

pub fn has_any_active_cluster_workspace(st: &Halley) -> bool {
    !st.model.cluster_state.active_cluster_workspaces.is_empty()
}

pub fn cluster_mode_active(st: &Halley) -> bool {
    cluster_mode_active_for_monitor(st, st.model.monitor_state.current_monitor.as_str())
}

pub fn cluster_mode_active_for_monitor(st: &Halley, monitor: &str) -> bool {
    st.model
        .cluster_state
        .cluster_mode_selected_nodes
        .contains_key(monitor)
}

pub fn has_active_cluster_workspace(st: &Halley) -> bool {
    active_cluster_workspace_for_monitor(st, st.model.monitor_state.current_monitor.as_str())
        .is_some()
}

pub(crate) fn active_cluster_layout_kind(st: &Halley) -> ClusterWorkspaceLayoutKind {
    st.runtime.tuning.cluster_layout_kind()
}

pub(crate) fn cluster_overflow_len(st: &Halley, cid: ClusterId) -> usize {
    if !matches!(
        active_cluster_layout_kind(st),
        ClusterWorkspaceLayoutKind::Tiling
    ) {
        return 0;
    }
    st.model
        .field
        .cluster(cid)
        .map(|cluster| {
            cluster
                .overflow_members(st.runtime.tuning.tile_max_stack)
                .len()
        })
        .unwrap_or(0)
}
