use std::collections::{HashMap, HashSet};

use halley_core::cluster::ClusterId;
use halley_core::cluster_policy::ClusterFormationState;
use halley_core::field::{NodeId, Vec2};
use halley_core::viewport::Viewport;

use crate::state::HalleyWlState;

pub(crate) struct WorkspaceState {
    pub(crate) cluster_form_state: ClusterFormationState,
    pub(crate) active_cluster_workspace: Option<ClusterId>,
    pub(crate) workspace_hidden_nodes: Vec<NodeId>,
    pub(crate) workspace_prev_viewport: Option<Viewport>,
    pub(crate) last_active_size: HashMap<NodeId, Vec2>,
    pub(crate) active_transition_until_ms: HashMap<NodeId, u64>,
    pub(crate) primary_promote_cooldown_until_ms: HashMap<NodeId, u64>,
    pub(crate) manual_collapsed_nodes: HashSet<NodeId>,
}

impl HalleyWlState {
    pub(crate) fn preserve_collapsed_surface(&self, id: NodeId) -> bool {
        self.workspace_state.manual_collapsed_nodes.contains(&id)
            || self.field.node(id).is_some_and(|n| {
                n.kind == halley_core::field::NodeKind::Surface
                    && n.state == halley_core::field::NodeState::Node
            })
    }
}
