use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_core::field::{NodeId, Vec2};

use crate::compositor::root::Halley;

pub(crate) struct WorkspaceState {
    pub(crate) last_active_size: HashMap<NodeId, Vec2>,
    pub(crate) active_transition_until_ms: HashMap<NodeId, u64>,
    pub(crate) primary_promote_cooldown_until_ms: HashMap<NodeId, u64>,
    pub(crate) manual_collapsed_nodes: HashSet<NodeId>,
}

impl Halley {
    pub fn mark_active_transition(&mut self, id: NodeId, now: Instant, duration_ms: u64) {
        if !self.runtime.tuning.physics_enabled {
            return;
        }
        self.model
            .workspace_state
            .active_transition_until_ms
            .insert(id, self.now_ms(now).saturating_add(duration_ms.max(1)));
        self.request_maintenance();
    }

    pub fn active_transition_alpha(&self, id: NodeId, now: Instant) -> f32 {
        if !self.runtime.tuning.physics_enabled {
            return 0.0;
        }
        let now_ms = self.now_ms(now);
        if self.input.interaction_state.resize_active == Some(id)
            || (self.input.interaction_state.resize_static_node == Some(id)
                && now_ms < self.input.interaction_state.resize_static_until_ms)
        {
            return 0.0;
        }
        let Some(&until) = self
            .model
            .workspace_state
            .active_transition_until_ms
            .get(&id)
        else {
            return 0.0;
        };
        if now_ms >= until {
            return 0.0;
        }
        let total = 420.0f32;
        let remaining = (until.saturating_sub(now_ms)) as f32;
        (remaining / total).clamp(0.0, 1.0)
    }

    pub(crate) fn preserve_collapsed_surface(&self, id: NodeId) -> bool {
        self.model
            .workspace_state
            .manual_collapsed_nodes
            .contains(&id)
            || self.model.field.node(id).is_some_and(|n| {
                n.kind == halley_core::field::NodeKind::Surface
                    && n.state == halley_core::field::NodeState::Node
            })
    }
}
