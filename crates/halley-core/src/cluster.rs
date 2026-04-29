use crate::field::{Node, NodeId};
use crate::tiling::{MasterStackLayout, Rect, layout_master_stack};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClusterId(u64);

impl ClusterId {
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClusterMode {
    Expanded,
    Collapsed,
    Active,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClusterRemoveMemberOutcome {
    Removed,
    RequiresDissolve,
}

#[derive(Clone, Debug, Default)]
pub struct ActiveWorkspace {
    pub nodes: HashMap<NodeId, Node>,
}

/// A cluster is a group of window nodes (members).
/// When collapsed, a Core node represents the cluster as the handle.
#[derive(Clone, Debug)]
pub struct Cluster {
    pub id: ClusterId,
    pub(crate) members: Vec<NodeId>,

    /// When collapsed, which Core node represents this cluster.
    pub core: Option<NodeId>,

    pub pinned: bool,

    pub mode: ClusterMode,
    pub active_workspace: Option<ActiveWorkspace>,
}

impl Cluster {
    pub fn new(id: ClusterId, members: Vec<NodeId>) -> Option<Self> {
        if members.len() < 2 {
            return None;
        }
        if has_duplicates(&members) {
            return None;
        }
        Some(Self {
            id,
            members,
            core: None,
            pinned: false,
            mode: ClusterMode::Expanded,
            active_workspace: None,
        })
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.members.contains(&id)
    }

    pub fn members(&self) -> &[NodeId] {
        &self.members
    }

    pub fn master(&self) -> NodeId {
        self.members[0]
    }

    pub fn secondaries(&self) -> &[NodeId] {
        &self.members[1..]
    }

    pub fn visible_members(&self, max_stack: usize) -> &[NodeId] {
        if max_stack == 0 {
            &self.members
        } else {
            let limit = max_stack + 1;
            let end = self.members.len().min(limit);
            &self.members[..end]
        }
    }

    pub fn overflow_members(&self, max_stack: usize) -> &[NodeId] {
        if max_stack == 0 {
            &[]
        } else {
            let limit = max_stack + 1;
            if self.members.len() <= limit {
                &[]
            } else {
                &self.members[limit..]
            }
        }
    }

    pub fn core_node(&self) -> Option<NodeId> {
        self.core
    }

    pub fn is_collapsed(&self) -> bool {
        matches!(self.mode, ClusterMode::Collapsed)
    }

    pub fn is_active(&self) -> bool {
        matches!(self.mode, ClusterMode::Active)
    }

    pub fn set_collapsed(&mut self, collapsed: bool) {
        self.mode = if collapsed {
            ClusterMode::Collapsed
        } else {
            ClusterMode::Expanded
        };
    }

    pub fn enter_active(&mut self) {
        self.mode = ClusterMode::Active;
        self.active_workspace
            .get_or_insert_with(ActiveWorkspace::default);
    }

    pub fn exit_active(&mut self) {
        self.mode = ClusterMode::Expanded;
        self.active_workspace = None;
    }

    pub fn workspace_layout(&self, bounds: Rect, max_stack: usize) -> MasterStackLayout {
        layout_master_stack(bounds, self.visible_members(max_stack))
    }

    pub(crate) fn add_member(&mut self, member: NodeId) -> bool {
        if self.members.contains(&member) {
            return false;
        }
        self.members.push(member);
        true
    }

    pub(crate) fn add_member_front(&mut self, member: NodeId) -> bool {
        if self.members.contains(&member) {
            return false;
        }
        self.members.insert(0, member);
        true
    }

    pub fn workspace_member(&self, id: NodeId) -> Option<&Node> {
        self.active_workspace.as_ref()?.nodes.get(&id)
    }

    pub fn workspace_member_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.active_workspace.as_mut()?.nodes.get_mut(&id)
    }

    pub(crate) fn insert_workspace_member(&mut self, node: Node) -> bool {
        let Some(active_workspace) = self.active_workspace.as_mut() else {
            return false;
        };
        active_workspace.nodes.insert(node.id, node);
        true
    }

    pub(crate) fn remove_workspace_member(&mut self, id: NodeId) -> Option<Node> {
        self.active_workspace.as_mut()?.nodes.remove(&id)
    }

    pub(crate) fn remove_member(&mut self, member: NodeId) -> Option<ClusterRemoveMemberOutcome> {
        if !self.members.contains(&member) {
            return None;
        }
        if self.members.len() <= 2 {
            return Some(ClusterRemoveMemberOutcome::RequiresDissolve);
        }

        self.members.retain(|&id| id != member);
        Some(ClusterRemoveMemberOutcome::Removed)
    }

    pub(crate) fn remove_member_for_node_removal(&mut self, member: NodeId) -> bool {
        let before = self.members.len();
        self.members.retain(|&id| id != member);
        self.members.len() != before
    }

    pub(crate) fn reorder_members(&mut self, ordered_members: Vec<NodeId>) -> bool {
        if ordered_members.len() != self.members.len() || has_duplicates(&ordered_members) {
            return false;
        }

        let mut current = self.members.clone();
        let mut reordered = ordered_members.clone();
        current.sort_by_key(|id| id.as_u64());
        reordered.sort_by_key(|id| id.as_u64());
        if current != reordered {
            return false;
        }

        self.members = ordered_members;
        true
    }

    pub(crate) fn promote_member_to_master(&mut self, member: NodeId) -> bool {
        let Some(index) = self.members.iter().position(|&id| id == member) else {
            return false;
        };
        if index == 0 {
            return true;
        }
        self.members.remove(index);
        self.members.insert(0, member);
        true
    }

    pub(crate) fn swap_overflow_member_with_visible(
        &mut self,
        overflow_member: NodeId,
        visible_member: NodeId,
        max_stack: usize,
    ) -> bool {
        let Some(overflow_index) = self.members.iter().position(|&id| id == overflow_member) else {
            return false;
        };
        let Some(visible_index) = self.members.iter().position(|&id| id == visible_member) else {
            return false;
        };
        if max_stack > 0 {
            let limit = max_stack + 1;
            if overflow_index < limit || visible_index >= limit {
                return false;
            }
        } else {
            // unlimited; no overflow member can exist
            return false;
        }

        self.members[overflow_index] = visible_member;
        self.members[visible_index] = overflow_member;
        true
    }

    pub(crate) fn reorder_overflow_member(
        &mut self,
        member: NodeId,
        target_overflow_index: usize,
        max_stack: usize,
    ) -> bool {
        let Some(member_index) = self.members.iter().position(|&id| id == member) else {
            return false;
        };
        if max_stack == 0 {
            return false;
        }
        let limit = max_stack + 1;
        if member_index < limit {
            return false;
        }

        let overflow_len = self.members.len().saturating_sub(limit);
        if overflow_len <= 1 {
            return true;
        }

        let member = self.members.remove(member_index);
        let clamped_index = target_overflow_index.min(overflow_len - 1);
        let insert_index = (limit + clamped_index).min(self.members.len());
        self.members.insert(insert_index, member);
        true
    }
}

fn has_duplicates(members: &[NodeId]) -> bool {
    let mut seen = std::collections::HashSet::new();
    for member in members {
        if !seen.insert(*member) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<NodeId> {
        (0..n).map(NodeId::new).collect()
    }

    #[test]
    fn visible_members_respects_max_stack() {
        let members = ids(10);
        let cluster = Cluster::new(ClusterId::new(1), members.clone()).unwrap();

        // max_stack 3 means 4 visible (1 master + 3 stack)
        assert_eq!(cluster.visible_members(3).len(), 4);
        assert_eq!(cluster.overflow_members(3).len(), 6);

        // max_stack 5 means 6 visible
        assert_eq!(cluster.visible_members(5).len(), 6);
        assert_eq!(cluster.overflow_members(5).len(), 4);
    }

    #[test]
    fn zero_max_stack_means_unlimited_visible() {
        let members = ids(10);
        let cluster = Cluster::new(ClusterId::new(1), members.clone()).unwrap();

        assert_eq!(cluster.visible_members(0).len(), 10);
        assert_eq!(cluster.overflow_members(0).len(), 0);
    }

    #[test]
    fn visible_members_capped_by_total_members() {
        let members = ids(3);
        let cluster = Cluster::new(ClusterId::new(1), members.clone()).unwrap();

        assert_eq!(cluster.visible_members(5).len(), 3);
        assert_eq!(cluster.overflow_members(5).len(), 0);
    }
}
