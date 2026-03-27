use std::collections::HashMap;
use std::time::Instant;

use halley_core::cluster::ClusterId;
use halley_core::field::{NodeId, Vec2};

use crate::interaction::types::DragAxisMode;
use crate::state::Halley;
use smithay::input::pointer::CursorIcon;

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

#[derive(Clone)]
pub(crate) struct ClusterJoinCandidate {
    pub(crate) cluster_id: ClusterId,
    pub(crate) node_id: NodeId,
    pub(crate) monitor: String,
    pub(crate) started_at_ms: u64,
    pub(crate) ready: bool,
}

#[derive(Clone)]
pub(crate) struct BloomPullPreview {
    pub(crate) cluster_id: ClusterId,
    pub(crate) member_id: NodeId,
    pub(crate) mix: f32,
}

#[derive(Clone)]
pub(crate) struct ClusterOverflowDragPreview {
    pub(crate) member_id: NodeId,
    pub(crate) monitor: String,
    pub(crate) screen_local: (f32, f32),
}

#[derive(Clone)]
pub(crate) struct OverlayHoverTarget {
    pub(crate) node_id: NodeId,
    pub(crate) monitor: String,
    pub(crate) screen_anchor: (i32, i32),
    pub(crate) prefer_left: bool,
}

#[derive(Clone)]
pub(crate) struct PendingCorePress {
    pub(crate) node_id: NodeId,
    pub(crate) monitor: String,
    pub(crate) press_global_sx: f32,
    pub(crate) press_global_sy: f32,
    pub(crate) reopen_bloom_on_timeout: bool,
}

#[derive(Clone)]
pub(crate) struct PendingCoreClick {
    pub(crate) node_id: NodeId,
    pub(crate) monitor: String,
    pub(crate) deadline_ms: u64,
    pub(crate) reopen_bloom_on_timeout: bool,
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
    pub(crate) cluster_join_candidate: Option<ClusterJoinCandidate>,
    pub(crate) bloom_pull_preview: Option<BloomPullPreview>,
    pub(crate) cluster_overflow_drag_preview: Option<ClusterOverflowDragPreview>,
    pub(crate) overlay_hover_target: Option<OverlayHoverTarget>,
    pub(crate) pending_core_press: Option<PendingCorePress>,
    pub(crate) pending_core_click: Option<PendingCoreClick>,
    pub(crate) grabbed_edge_pan_active: bool,
    pub(crate) grabbed_edge_pan_direction: Vec2,
    pub(crate) grabbed_edge_pan_pressure: Vec2,
    pub(crate) grabbed_edge_pan_monitor: Option<String>,
    pub(crate) cursor_override_icon: Option<CursorIcon>,
}

impl Halley {
    #[inline]
    pub(crate) fn clear_grabbed_edge_pan_state(&mut self) {
        self.input.interaction_state.grabbed_edge_pan_active = false;
        self.input.interaction_state.grabbed_edge_pan_direction = Vec2 { x: 0.0, y: 0.0 };
        self.input.interaction_state.grabbed_edge_pan_pressure = Vec2 { x: 0.0, y: 0.0 };
        self.input.interaction_state.grabbed_edge_pan_monitor = None;
    }

    pub(crate) fn node_fully_visible_on_monitor(
        &self,
        monitor_name: &str,
        node_id: NodeId,
    ) -> Option<bool> {
        let node = self.model.field.node(node_id)?;
        let monitor = self.model.monitor_state.monitors.get(monitor_name)?;
        let ext = if node.kind == halley_core::field::NodeKind::Surface
            && node.state == halley_core::field::NodeState::Active
        {
            self.surface_window_collision_extents(node)
        } else {
            self.collision_extents_for_node(node)
        };

        let (view_center, view_size) = if self.model.monitor_state.current_monitor == monitor_name {
            (self.model.viewport.center, self.camera_view_size())
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
        const EDGE_PAN_EXIT_MARGIN: f32 = 24.0;
        const EDGE_CONTACT_INSET: f32 = 0.75;

        let node = self.model.field.node(node_id)?;
        let monitor = self.model.monitor_state.monitors.get(monitor_name)?;
        let ext = if node.kind == halley_core::field::NodeKind::Surface
            && node.state == halley_core::field::NodeState::Active
        {
            self.surface_window_collision_extents(node)
        } else {
            self.collision_extents_for_node(node)
        };

        let (view_center, view_size) = if self.model.monitor_state.current_monitor == monitor_name {
            (self.model.viewport.center, self.camera_view_size())
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
            x: if previous_contact.x < 0.0 && desired_center.x < min_center_x + EDGE_PAN_EXIT_MARGIN
            {
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
            y: if previous_contact.y < 0.0 && desired_center.y < min_center_y + EDGE_PAN_EXIT_MARGIN
            {
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

    pub(crate) fn dragged_node_cluster_core_clamp(
        &self,
        monitor_name: &str,
        node_id: NodeId,
        desired_center: Vec2,
    ) -> Option<(Vec2, ClusterId, f32)> {
        let node = self.model.field.node(node_id)?;
        let mover_ext = if node.kind == halley_core::field::NodeKind::Surface
            && matches!(
                node.state,
                halley_core::field::NodeState::Active | halley_core::field::NodeState::Drifting
            )
        {
            self.surface_window_collision_extents(node)
        } else {
            self.collision_extents_for_node(node)
        };
        let gap = self.non_overlap_gap_world();
        let mut mover_pos = desired_center;
        let mut engaged_cluster = None;
        let mut max_push = 0.0f32;

        for _ in 0..12 {
            let cores = self
                .model
                .field
                .clusters_iter()
                .filter(|cluster| {
                    cluster.is_collapsed()
                        && !cluster.contains(node_id)
                        && cluster.core != Some(node_id)
                })
                .filter_map(|cluster| {
                    let core_id = cluster.core?;
                    let core = self.model.field.node(core_id)?;
                    let core_monitor = self
                        .model
                        .monitor_state
                        .node_monitor
                        .get(&core_id)
                        .map(String::as_str)
                        .unwrap_or(monitor_name);
                    (core_monitor == monitor_name).then_some((
                        cluster.id,
                        core.pos,
                        self.collision_extents_for_node(core),
                    ))
                })
                .collect::<Vec<_>>();

            let mut changed = false;
            for (cluster_id, core_pos, core_ext) in cores {
                let dx = mover_pos.x - core_pos.x;
                let dy = mover_pos.y - core_pos.y;
                let req_x = self.required_sep_x(mover_pos.x, mover_ext, core_pos.x, core_ext, gap);
                let req_y = self.required_sep_y(mover_pos.y, mover_ext, core_pos.y, core_ext, gap);
                let ox = req_x - dx.abs();
                let oy = req_y - dy.abs();
                if ox <= 0.0 || oy <= 0.0 {
                    continue;
                }

                engaged_cluster = Some(cluster_id);
                max_push = max_push.max(ox.max(oy));
                if ox < oy {
                    let s = if dx.abs() > f32::EPSILON {
                        dx.signum()
                    } else if core_pos.x <= mover_pos.x {
                        1.0
                    } else {
                        -1.0
                    };
                    mover_pos.x += s * (ox + 0.3);
                } else {
                    let s = if dy.abs() > f32::EPSILON {
                        dy.signum()
                    } else if core_pos.y <= mover_pos.y {
                        1.0
                    } else {
                        -1.0
                    };
                    mover_pos.y += s * (oy + 0.3);
                }
                changed = true;
            }

            if !changed {
                break;
            }
        }

        engaged_cluster.map(|cluster_id| (mover_pos, cluster_id, max_push))
    }

    pub(crate) fn enforce_pan_dominant_zone_states(&mut self, now_ms: u64) {
        let active_outside_ring_delay_ms = self.runtime.tuning.active_outside_ring_delay_ms;
        let inactive_outside_ring_delay_ms = self.runtime.tuning.inactive_outside_ring_delay_ms;

        let ids: Vec<NodeId> = self.model.field.nodes().keys().copied().collect();

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
