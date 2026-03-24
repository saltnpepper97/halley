use std::collections::HashMap;
use std::time::Instant;

use halley_core::field::NodeId;
use halley_core::trail::Trail;

use crate::state::HalleyWlState;

pub(crate) struct FocusState { 
    pub(crate) primary_interaction_focus: Option<NodeId>,
    pub(crate) monitor_focus: HashMap<String, NodeId>,
    pub(crate) interaction_focus_until_ms: u64,
    pub(crate) last_surface_focus_ms: HashMap<NodeId, u64>,
    pub(crate) focus_trail: Trail,
    pub(crate) suppress_trail_record_once: bool,
    pub(crate) pan_restore_active_focus: Option<NodeId>,
    pub(crate) app_focused: bool,
    pub(crate) focus_ring_preview_until_ms: HashMap<String, u64>,
    pub(crate) recent_top_node: Option<NodeId>,
    pub(crate) recent_top_until: Option<Instant>,
}

impl HalleyWlState {
    #[allow(dead_code)]
    pub(crate) fn focused_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        self.focus_state.monitor_focus.get(monitor).copied()
    }

    #[allow(dead_code)]
    pub(crate) fn focused_monitor_for_node(&self, id: NodeId) -> Option<String> {
        self.monitor_state.node_monitor.get(&id).cloned()
    }

    #[allow(dead_code)]
    pub(crate) fn set_monitor_focus(&mut self, monitor: &str, id: NodeId) {
        self.focus_state.monitor_focus.insert(monitor.to_string(), id);
    }

    pub fn set_recent_top_node(&mut self, node_id: NodeId, until: Instant) {
        self.focus_state.recent_top_node = Some(node_id);
        self.focus_state.recent_top_until = Some(until);
    }

    pub fn recent_top_node_active(&mut self, now: Instant) -> Option<NodeId> {
        if self.focus_state.recent_top_until.is_some_and(|until| now >= until) {
            self.focus_state.recent_top_node = None;
            self.focus_state.recent_top_until = None;
            return None;
        }
        self.focus_state.recent_top_node
    }
}
