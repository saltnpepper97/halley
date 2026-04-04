use super::*;
use crate::compositor::clusters::read::{
    ClusterLayoutPlan, ClusterReadController, ClusterTilePlacement, EnterClusterWorkspacePlan,
    ExitClusterWorkspacePlan,
};
use crate::compositor::clusters::state::ClusterState;
use crate::compositor::interaction::state::InteractionState;
use crate::overlay::OverlayActionHint;
use halley_config::{ClusterDefaultLayout, DirectionalAction};
use halley_core::cluster::{ClusterId, ClusterRemoveMemberOutcome};
use halley_core::cluster_layout::{ClusterCycleDirection, ClusterWorkspaceLayoutKind};
use halley_core::field::RemoveNodeClusterEffect;
use std::ops::{Deref, DerefMut};

struct ClusterMutationController<'a> {
    field: &'a mut Field,
    cluster_state: &'a mut ClusterState,
    interaction_state: &'a mut InteractionState,
    tuning: &'a halley_config::RuntimeTuning,
}

impl<'a> ClusterMutationController<'a> {
    fn cluster_mode_selected_nodes_for_monitor_mut(
        &mut self,
        monitor: &str,
    ) -> &mut std::collections::HashSet<NodeId> {
        self.cluster_state
            .cluster_mode_selected_nodes
            .entry(monitor.to_string())
            .or_default()
    }

    fn open_cluster_bloom_for_monitor(&mut self, monitor: &str, cid: ClusterId) -> bool {
        let Some(cluster) = self.field.cluster(cid) else {
            return false;
        };
        let Some(core_id) = cluster.core else {
            return false;
        };
        self.cluster_state
            .cluster_bloom_open
            .retain(|name, open_cid| *open_cid != cid || name == monitor);
        let _ = self.close_cluster_bloom_for_monitor(monitor);
        let _ = self.field.set_pinned(core_id, true);
        self.interaction_state.physics_velocity.remove(&core_id);
        self.cluster_state
            .cluster_bloom_open
            .insert(monitor.to_string(), cid);
        true
    }

    fn close_cluster_bloom_for_monitor(&mut self, monitor: &str) -> bool {
        let Some(cid) = self.cluster_state.cluster_bloom_open.remove(monitor) else {
            return false;
        };
        if let Some(core_id) = self.field.cluster(cid).and_then(|cluster| cluster.core) {
            let _ = self.field.set_pinned(core_id, false);
        }
        true
    }

    fn enter_cluster_mode(&mut self, monitor: &str) -> bool {
        if self
            .cluster_state
            .cluster_mode_selected_nodes
            .contains_key(monitor)
        {
            return true;
        }
        self.cluster_state
            .cluster_mode_selected_nodes
            .insert(monitor.to_string(), std::collections::HashSet::new());
        true
    }

    fn exit_cluster_mode(&mut self, monitor: &str) -> bool {
        if !self
            .cluster_state
            .cluster_mode_selected_nodes
            .contains_key(monitor)
        {
            return false;
        }
        self.cluster_state
            .cluster_mode_selected_nodes
            .remove(monitor);
        true
    }

    fn toggle_cluster_mode_selection(&mut self, monitor: &str, node_id: NodeId) -> bool {
        if !self
            .cluster_state
            .cluster_mode_selected_nodes
            .contains_key(monitor)
        {
            return false;
        }
        let Some(node) = self.field.node(node_id) else {
            return false;
        };
        if node.kind != halley_core::field::NodeKind::Surface
            || node.state == halley_core::field::NodeState::Core
            || !self.field.is_visible(node_id)
        {
            return false;
        }
        if !self
            .cluster_mode_selected_nodes_for_monitor_mut(monitor)
            .insert(node_id)
        {
            self.cluster_mode_selected_nodes_for_monitor_mut(monitor)
                .remove(&node_id);
        }
        true
    }

    fn detach_member_from_cluster(
        &mut self,
        cid: ClusterId,
        member_id: NodeId,
        world_pos: Vec2,
        now_ms: u64,
    ) -> Option<ClusterRemoveMemberOutcome> {
        let was_active = self
            .field
            .cluster(cid)
            .is_some_and(|cluster| cluster.is_active());
        let outcome = self.field.remove_member_from_cluster(cid, member_id)?;
        if matches!(outcome, ClusterRemoveMemberOutcome::Removed) && was_active {
            let _ = self
                .field
                .move_member_out_of_active_cluster_workspace(cid, member_id);
        }
        let _ = self.field.set_detached(member_id, false);
        let _ = self
            .field
            .set_state(member_id, halley_core::field::NodeState::Active);
        if let Some(node) = self.field.node_mut(member_id) {
            node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
            node.pos = world_pos;
        }
        let _ = self.field.touch(member_id, now_ms);
        Some(outcome)
    }

    fn absorb_node_into_cluster(&mut self, cid: ClusterId, node_id: NodeId) -> bool {
        let active_workspace = self
            .field
            .cluster(cid)
            .is_some_and(|cluster| cluster.is_active());
        let add_result = if matches!(
            self.tuning.cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Stacking
        ) {
            self.field.add_member_to_cluster_front(cid, node_id)
        } else {
            self.field.add_member_to_cluster(cid, node_id)
        };
        if add_result.is_err() {
            return false;
        }
        if self.tuning.tile_new_on_top
            && matches!(
                self.tuning.cluster_layout_kind(),
                ClusterWorkspaceLayoutKind::Tiling
            )
        {
            let _ = self.field.promote_cluster_member_to_master(cid, node_id);
        }
        if active_workspace {
            if !self
                .field
                .move_member_into_active_cluster_workspace(cid, node_id)
            {
                return false;
            }
            if let Some(cluster) = self.field.cluster_mut(cid)
                && let Some(node) = cluster.workspace_member_mut(node_id)
            {
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
                node.visibility.set(Visibility::DETACHED, false);
                node.state = halley_core::field::NodeState::Active;
            }
        } else {
            let _ = self
                .field
                .set_state(node_id, halley_core::field::NodeState::Node);
            if let Some(node) = self.field.node_mut(node_id) {
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, true);
            }
            let _ = self.field.set_detached(node_id, false);
        }
        true
    }
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

fn directional_candidate_score(
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
mod navigation;
mod overflow;
mod workspace;

#[cfg(test)]
mod tests;

pub(crate) struct ClusterSystemController<T> {
    st: T,
}

pub(crate) fn cluster_system_controller<T>(st: T) -> ClusterSystemController<T> {
    ClusterSystemController { st }
}

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

pub(crate) fn stack_layout_rects_for_members(
    st: &Halley,
    monitor: &str,
    members: &[NodeId],
) -> Option<std::collections::HashMap<NodeId, halley_core::tiling::Rect>> {
    cluster_system_controller(st).stack_layout_rects_for_members(monitor, members)
}

impl<T: Deref<Target = Halley>> Deref for ClusterSystemController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for ClusterSystemController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

impl<T: Deref<Target = Halley>> ClusterSystemController<T> {
    const CLUSTER_OVERFLOW_VISIBLE_SLOTS: usize = 15;

    fn cluster_read_controller(&self) -> ClusterReadController<'_> {
        ClusterReadController {
            field: &self.model.field,
            cluster_state: &self.model.cluster_state,
            monitor_state: &self.model.monitor_state,
            tuning: &self.runtime.tuning,
        }
    }

    fn preferred_monitor_for_cluster(
        &self,
        cid: ClusterId,
        preferred: Option<&str>,
    ) -> Option<String> {
        self.cluster_read_controller()
            .preferred_monitor_for_cluster(cid, preferred)
    }

    pub(crate) fn cluster_overflow_rect_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<halley_core::tiling::Rect> {
        self.model
            .cluster_state
            .cluster_overflow_rects
            .get(monitor)
            .copied()
    }

    pub(crate) fn cluster_overflow_slot_rect_for_monitor(
        &self,
        monitor: &str,
        overflow_len: usize,
        slot_index: usize,
    ) -> Option<halley_core::tiling::Rect> {
        self.cluster_read_controller()
            .overflow_strip_slot_rect_for_monitor(monitor, overflow_len, slot_index)
    }

    pub(crate) fn active_cluster_tile_rect_for_member(
        &self,
        monitor: &str,
        member_id: NodeId,
    ) -> Option<halley_core::tiling::Rect> {
        self.cluster_read_controller()
            .plan_active_cluster_layout(monitor)?
            .tiles
            .into_iter()
            .find(|tile| tile.node_id == member_id)
            .map(|tile| tile.rect)
    }

    pub(crate) fn cluster_spawn_rect_for_new_member(
        &self,
        monitor: &str,
        cid: ClusterId,
    ) -> Option<halley_core::tiling::Rect> {
        self.cluster_read_controller()
            .cluster_spawn_rect_for_new_member(monitor, cid)
    }

    pub(crate) fn stack_layout_rects_for_members(
        &self,
        monitor: &str,
        members: &[NodeId],
    ) -> Option<std::collections::HashMap<NodeId, halley_core::tiling::Rect>> {
        self.cluster_read_controller()
            .stack_layout_rects_for_members(monitor, members)
    }

    pub fn has_any_active_cluster_workspace(&self) -> bool {
        !self
            .model
            .cluster_state
            .active_cluster_workspaces
            .is_empty()
    }

    pub fn cluster_mode_active(&self) -> bool {
        self.cluster_mode_active_for_monitor(self.model.monitor_state.current_monitor.as_str())
    }

    pub fn cluster_mode_active_for_monitor(&self, monitor: &str) -> bool {
        self.model
            .cluster_state
            .cluster_mode_selected_nodes
            .contains_key(monitor)
    }

    pub fn has_active_cluster_workspace(&self) -> bool {
        self.active_cluster_workspace_for_monitor(self.model.monitor_state.current_monitor.as_str())
            .is_some()
    }

    pub fn active_cluster_workspace_for_monitor(&self, monitor: &str) -> Option<ClusterId> {
        self.model
            .cluster_state
            .active_cluster_workspaces
            .get(monitor)
            .copied()
    }

    fn active_cluster_layout_kind(&self) -> ClusterWorkspaceLayoutKind {
        self.runtime.tuning.cluster_layout_kind()
    }

    fn cluster_overflow_len(&self, cid: ClusterId) -> usize {
        if !matches!(
            self.active_cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Tiling
        ) {
            return 0;
        }
        self.model
            .field
            .cluster(cid)
            .map(|cluster| {
                cluster
                    .overflow_members(self.runtime.tuning.tile_max_stack)
                    .len()
            })
            .unwrap_or(0)
    }
}
