use std::collections::{HashMap, HashSet};

use halley_core::field::{NodeId, Vec2};

use crate::state::Halley;

pub(crate) struct WorkspaceState {
    pub(crate) last_active_size: HashMap<NodeId, Vec2>,
    pub(crate) active_transition_until_ms: HashMap<NodeId, u64>,
    pub(crate) primary_promote_cooldown_until_ms: HashMap<NodeId, u64>,
    pub(crate) manual_collapsed_nodes: HashSet<NodeId>,
}

impl Halley {
    pub(crate) fn preserve_collapsed_surface(&self, id: NodeId) -> bool {
        self.workspace_state.manual_collapsed_nodes.contains(&id)
            || self.field.node(id).is_some_and(|n| {
                n.kind == halley_core::field::NodeKind::Surface
                    && n.state == halley_core::field::NodeState::Node
            })
    }
}
