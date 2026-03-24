use std::collections::HashMap;
use std::time::Instant;

use halley_core::field::{NodeId, Vec2};

use crate::state::HalleyWlState;

pub(crate) struct ViewportPanAnim {
    pub(crate) start_ms: u64,
    pub(crate) delay_ms: u64,
    pub(crate) duration_ms: u64,
    pub(crate) from_center: Vec2,
    pub(crate) to_center: Vec2,
}

pub(crate) struct InteractionState {
    pub(crate) reset_input_state_requested: bool,
    pub(crate) pending_pointer_screen_hint: Option<(f32, f32)>,
    pub(crate) suppress_layer_shell_configure: bool,
    pub(crate) dpms_just_woke: bool,
    pub(crate) resize_active: Option<NodeId>,
    pub(crate) resize_static_node: Option<NodeId>,
    pub(crate) resize_static_lock_pos: Option<Vec2>,
    pub(crate) resize_static_until_ms: u64,
    pub(crate) drag_authority_node: Option<NodeId>,
    pub(crate) suspend_overlap_resolve: bool,
    pub(crate) suspend_state_checks: bool,
    pub(crate) physics_velocity: HashMap<NodeId, Vec2>,
    pub(crate) physics_last_tick: Instant,
    pub(crate) smoothed_render_pos: HashMap<NodeId, Vec2>,
    pub(crate) viewport_pan_anim: Option<ViewportPanAnim>,
    pub(crate) pan_dominant_until_ms: u64,
}

impl HalleyWlState {
    pub(crate) fn enforce_pan_dominant_zone_states(&mut self, now_ms: u64) {
        let active_outside_ring_delay_ms = self.tuning.active_outside_ring_delay_ms;
        let inactive_outside_ring_delay_ms = self.tuning.inactive_outside_ring_delay_ms;

        let ids: Vec<NodeId> = self.field.nodes().keys().copied().collect();

        for id in ids {
            self.apply_single_surface_decay_policy(
                id,
                now_ms,
                active_outside_ring_delay_ms,
                inactive_outside_ring_delay_ms,
            );
        }
    }
}
