use std::collections::HashMap;
use std::time::Instant;

use halley_core::field::{NodeId, Vec2};

use crate::interaction::types::DragAxisMode;
use crate::state::Halley;

pub(crate) struct ViewportPanAnim {
    pub(crate) start_ms: u64,
    pub(crate) delay_ms: u64,
    pub(crate) duration_ms: u64,
    pub(crate) from_center: Vec2,
    pub(crate) to_center: Vec2,
}

#[derive(Clone)]
pub(crate) struct ActiveDragState {
    pub(crate) node_id: NodeId,
    pub(crate) allow_monitor_transfer: bool,
    pub(crate) edge_pan_eligible: bool,
    pub(crate) current_offset: Vec2,
    pub(crate) pointer_monitor: String,
    pub(crate) pointer_workspace_size: (i32, i32),
    pub(crate) pointer_screen_local: (f32, f32),
    pub(crate) edge_pan_x: DragAxisMode,
    pub(crate) edge_pan_y: DragAxisMode,
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
    pub(crate) drag_authority_velocity: Vec2,
    pub(crate) suspend_overlap_resolve: bool,
    pub(crate) suspend_state_checks: bool,
    pub(crate) physics_velocity: HashMap<NodeId, Vec2>,
    pub(crate) physics_last_tick: Instant,
    pub(crate) smoothed_render_pos: HashMap<NodeId, Vec2>,
    pub(crate) viewport_pan_anim: Option<ViewportPanAnim>,
    pub(crate) pan_dominant_until_ms: u64,
    pub(crate) active_drag: Option<ActiveDragState>,
    pub(crate) grabbed_edge_pan_active: bool,
    pub(crate) grabbed_edge_pan_direction: Vec2,
    pub(crate) grabbed_edge_pan_monitor: Option<String>,
}

impl Halley {
    pub(crate) fn node_fully_visible_on_monitor(
        &self,
        monitor_name: &str,
        node_id: NodeId,
    ) -> Option<bool> {
        let node = self.field.node(node_id)?;
        let monitor = self.monitor_state.monitors.get(monitor_name)?;
        let ext = if node.kind == halley_core::field::NodeKind::Surface {
            self.surface_window_collision_extents(node)
        } else {
            self.collision_extents_for_node(node)
        };

        let (view_center, view_size) = if self.monitor_state.current_monitor == monitor_name {
            (self.viewport.center, self.zoom_ref_size)
        } else {
            (monitor.viewport.center, monitor.zoom_ref_size)
        };
        let left = view_center.x - view_size.x * 0.5;
        let right = view_center.x + view_size.x * 0.5;
        let top = view_center.y - view_size.y * 0.5;
        let bottom = view_center.y + view_size.y * 0.5;

        Some(
            node.pos.x - ext.left >= left
                && node.pos.x + ext.right <= right
                && node.pos.y - ext.top >= top
                && node.pos.y + ext.bottom <= bottom,
        )
    }

    pub(crate) fn dragged_node_edge_pan_clamp(
        &self,
        monitor_name: &str,
        node_id: NodeId,
        desired_center: Vec2,
        previous_contact: Vec2,
    ) -> Option<(Vec2, Vec2)> {
        const EDGE_PAN_EXIT_MARGIN: f32 = 64.0;
        const EDGE_CONTACT_INSET: f32 = 0.75;

        let node = self.field.node(node_id)?;
        let monitor = self.monitor_state.monitors.get(monitor_name)?;
        let ext = if node.kind == halley_core::field::NodeKind::Surface {
            self.surface_window_collision_extents(node)
        } else {
            self.collision_extents_for_node(node)
        };

        let (view_center, view_size) = if self.monitor_state.current_monitor == monitor_name {
            (self.viewport.center, self.zoom_ref_size)
        } else {
            (monitor.viewport.center, monitor.zoom_ref_size)
        };
        let min_center_x = view_center.x - view_size.x * 0.5 + ext.left + EDGE_CONTACT_INSET;
        let max_center_x = view_center.x + view_size.x * 0.5 - ext.right - EDGE_CONTACT_INSET;
        let min_center_y = view_center.y - view_size.y * 0.5 + ext.top + EDGE_CONTACT_INSET;
        let max_center_y = view_center.y + view_size.y * 0.5 - ext.bottom - EDGE_CONTACT_INSET;

        let clamped_center = Vec2 {
            x: desired_center.x.clamp(min_center_x, max_center_x),
            y: desired_center.y.clamp(min_center_y, max_center_y),
        };
        let edge_contact = Vec2 {
            x: if previous_contact.x < 0.0 && desired_center.x < min_center_x + EDGE_PAN_EXIT_MARGIN {
                -1.0
            } else if previous_contact.x > 0.0
                && desired_center.x > max_center_x - EDGE_PAN_EXIT_MARGIN
            {
                1.0
            } else if desired_center.x < min_center_x - 0.01 {
                -1.0
            } else if desired_center.x > max_center_x + 0.01 {
                1.0
            } else {
                0.0
            },
            y: if previous_contact.y < 0.0 && desired_center.y < min_center_y + EDGE_PAN_EXIT_MARGIN {
                -1.0
            } else if previous_contact.y > 0.0
                && desired_center.y > max_center_y - EDGE_PAN_EXIT_MARGIN
            {
                1.0
            } else if desired_center.y < min_center_y - 0.01 {
                -1.0
            } else if desired_center.y > max_center_y + 0.01 {
                1.0
            } else {
                0.0
            },
        };

        Some((clamped_center, edge_contact))
    }

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
