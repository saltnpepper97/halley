use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use halley_core::field::{NodeId, Vec2};
use halley_core::decay::DecayLevel;

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
    pub(crate) per_monitor: HashMap<String, MonitorSpawnState>,
    pub(crate) pending_spawn_pan_queue: VecDeque<PendingSpawnPan>,
    pub(crate) active_spawn_pan: Option<ActiveSpawnPan>,
}

impl Halley {
    pub(crate) fn default_spawn_view_anchor_for_monitor(&self, monitor: &str) -> Vec2 {
        self.monitor_state
            .monitors
            .get(monitor)
            .map(|space| space.viewport.center)
            .unwrap_or(self.viewport.center)
    }

    pub(crate) fn spawn_monitor_state(&self, monitor: &str) -> MonitorSpawnState {
        self.spawn_state
            .per_monitor
            .get(monitor)
            .cloned()
            .unwrap_or_else(|| {
                MonitorSpawnState::new(self.default_spawn_view_anchor_for_monitor(monitor))
            })
    }

    pub(crate) fn spawn_monitor_state_mut(&mut self, monitor: &str) -> &mut MonitorSpawnState {
        let view_anchor = self.default_spawn_view_anchor_for_monitor(monitor);
        self.spawn_state
            .per_monitor
            .entry(monitor.to_string())
            .or_insert_with(|| MonitorSpawnState::new(view_anchor))
    }

    pub(crate) fn process_pending_spawn_activations(&mut self, now: Instant, now_ms: u64) {
        let due: Vec<NodeId> = self
            .spawn_state
            .pending_spawn_activate_at_ms
            .iter()
            .filter_map(|(&id, &at)| (now_ms >= at).then_some(id))
            .collect();

        for id in due {
            self.spawn_state.pending_spawn_activate_at_ms.remove(&id);
            if !self.field.is_visible(id) {
                continue;
            }
            let Some(n) = self.field.node(id) else {
                continue;
            };
            if n.kind != halley_core::field::NodeKind::Surface {
                continue;
            }
            if self.preserve_collapsed_surface(id) {
                continue;
            }
            let _ = self.field.set_decay_level(id, DecayLevel::Hot);
            if let Some((_, _, w, h)) = self.render_state.window_geometry.get(&id) {
                self.workspace_state
                    .last_active_size
                    .insert(id, Vec2 { x: *w, y: *h });
            }
            self.mark_active_transition(id, now, 620);
            self.record_focus_trail_visit(id);
            self.focus_state.suppress_trail_record_once = true;
            self.set_interaction_focus(Some(id), 30_000, now);
        }
    }
}
