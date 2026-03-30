use std::time::Instant;

use halley_core::field::{NodeId, Vec2};

#[derive(Clone, Copy)]
pub(crate) enum DragAxisMode {
    Free,
    EdgePanNeg,
    EdgePanPos,
}

impl DragAxisMode {
    pub(crate) fn sign(self) -> f32 {
        match self {
            Self::Free => 0.0,
            Self::EdgePanNeg => -1.0,
            Self::EdgePanPos => 1.0,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DragCtx {
    pub(crate) node_id: NodeId,
    pub(crate) allow_monitor_transfer: bool,
    pub(crate) edge_pan_eligible: bool,
    pub(crate) current_offset: Vec2,
    pub(crate) center_latched: bool,
    pub(crate) started_active: bool,
    pub(crate) edge_pan_x: DragAxisMode,
    pub(crate) edge_pan_y: DragAxisMode,
    pub(crate) edge_pan_pressure: Vec2,
    pub(crate) last_pointer_world: Vec2,
    pub(crate) last_update_at: Instant,
    pub(crate) release_velocity: Vec2,
}
