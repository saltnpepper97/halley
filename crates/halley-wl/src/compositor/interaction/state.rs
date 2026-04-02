use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use halley_config::CompositorBindingAction;
use halley_core::cluster::ClusterId;
use halley_core::field::{NodeId, Vec2};
use halley_core::viewport::Viewport;

use crate::compositor::interaction::drag::DragAxisMode;
use crate::compositor::root::Halley;
use smithay::input::pointer::CursorIcon;

#[derive(Default, Clone)]
pub(crate) struct ModState {
    pub(crate) super_down: bool,
    pub(crate) left_super_down: bool,
    pub(crate) right_super_down: bool,
    pub(crate) alt_down: bool,
    pub(crate) left_alt_down: bool,
    pub(crate) right_alt_down: bool,
    pub(crate) ctrl_down: bool,
    pub(crate) left_ctrl_down: bool,
    pub(crate) right_ctrl_down: bool,
    pub(crate) shift_down: bool,
    pub(crate) left_shift_down: bool,
    pub(crate) right_shift_down: bool,
    pub(crate) intercepted_keys: HashSet<u32>,
    pub(crate) intercepted_compositor_actions: HashMap<u32, CompositorBindingAction>,
}

#[derive(Clone, Copy)]
pub(crate) struct NodeMoveAnim {
    pub(crate) node_id: NodeId,
    pub(crate) from: Vec2,
    pub(crate) to: Vec2,
    pub(crate) started_at: Instant,
    pub(crate) duration: Duration,
}

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
    pub(crate) last_pointer_screen_global: Option<(f32, f32)>,
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

pub(crate) fn take_input_state_reset_request(st: &mut Halley) -> bool {
    std::mem::take(&mut st.input.interaction_state.reset_input_state_requested)
}

pub(crate) fn take_pointer_screen_hint_request(st: &mut Halley) -> Option<(f32, f32)> {
    st.input
        .interaction_state
        .pending_pointer_screen_hint
        .take()
}

pub(crate) fn tick_cluster_join_candidate_ready(st: &mut Halley, now_ms: u64) {
    let dwell_ms = st.runtime.tuning.cluster_dwell_ms;
    let Some(candidate) = st.input.interaction_state.cluster_join_candidate.as_mut() else {
        return;
    };
    if candidate.ready {
        return;
    }
    candidate.ready = now_ms.saturating_sub(candidate.started_at_ms) >= dwell_ms;
}

pub(crate) fn resize_static_active_for(st: &Halley, node_id: NodeId, now_ms: u64) -> bool {
    st.input.interaction_state.resize_static_node == Some(node_id)
        && now_ms < st.input.interaction_state.resize_static_until_ms
}

#[inline]
pub(crate) fn is_recently_resized_node(st: &Halley, id: NodeId, now_ms: u64) -> bool {
    st.input.interaction_state.resize_static_node == Some(id)
        && now_ms < st.input.interaction_state.resize_static_until_ms
}

#[inline]
pub(crate) fn clear_grabbed_edge_pan_state(st: &mut Halley) {
    st.input.interaction_state.grabbed_edge_pan_active = false;
    st.input.interaction_state.grabbed_edge_pan_direction = Vec2 { x: 0.0, y: 0.0 };
    st.input.interaction_state.grabbed_edge_pan_pressure = Vec2 { x: 0.0, y: 0.0 };
    st.input.interaction_state.grabbed_edge_pan_monitor = None;
}

#[inline]
fn field_viewport_for_monitor(st: &Halley, monitor_name: &str) -> Option<Viewport> {
    if st.model.monitor_state.current_monitor == monitor_name {
        return Some(Viewport::new(
            st.model.viewport.center,
            st.model.zoom_ref_size,
        ));
    }

    st.model
        .monitor_state
        .monitors
        .get(monitor_name)
        .map(|space| Viewport::new(space.viewport.center, space.zoom_ref_size))
}

pub(crate) fn node_fully_visible_on_monitor(
    st: &Halley,
    monitor_name: &str,
    node_id: NodeId,
) -> Option<bool> {
    let node = st.model.field.node(node_id)?;
    let viewport = field_viewport_for_monitor(st, monitor_name)?;
    let ext = if node.kind == halley_core::field::NodeKind::Surface
        && node.state == halley_core::field::NodeState::Active
    {
        st.surface_window_collision_extents(node)
    } else {
        st.collision_extents_for_node(node)
    };

    let (view_center, view_size) = (viewport.center, viewport.size);
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
    st: &Halley,
    monitor_name: &str,
    node_id: NodeId,
    desired_center: Vec2,
    previous_contact: Vec2,
) -> Option<(Vec2, Vec2)> {
    const EDGE_PAN_EXIT_MARGIN: f32 = 24.0;
    const EDGE_CONTACT_INSET: f32 = 0.75;

    let node = st.model.field.node(node_id)?;
    let viewport = field_viewport_for_monitor(st, monitor_name)?;
    let ext = if node.kind == halley_core::field::NodeKind::Surface
        && node.state == halley_core::field::NodeState::Active
    {
        st.surface_window_collision_extents(node)
    } else {
        st.collision_extents_for_node(node)
    };

    let (view_center, view_size) = (viewport.center, viewport.size);
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
        } else if previous_contact.x > 0.0 && desired_center.x > max_center_x - EDGE_PAN_EXIT_MARGIN
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
        } else if previous_contact.y > 0.0 && desired_center.y > max_center_y - EDGE_PAN_EXIT_MARGIN
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
    st: &Halley,
    monitor_name: &str,
    node_id: NodeId,
    desired_center: Vec2,
) -> Option<(Vec2, ClusterId, f32)> {
    let node = st.model.field.node(node_id)?;
    let mover_ext = if node.kind == halley_core::field::NodeKind::Surface
        && matches!(
            node.state,
            halley_core::field::NodeState::Active | halley_core::field::NodeState::Drifting
        ) {
        st.surface_window_collision_extents(node)
    } else {
        st.collision_extents_for_node(node)
    };
    let gap = st.non_overlap_gap_world();
    let mut mover_pos = desired_center;
    let mut engaged_cluster = None;
    let mut max_push = 0.0f32;

    for _ in 0..12 {
        let cores = st
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
                let core = st.model.field.node(core_id)?;
                let core_monitor = st
                    .model
                    .monitor_state
                    .node_monitor
                    .get(&core_id)
                    .map(String::as_str)
                    .unwrap_or(monitor_name);
                (core_monitor == monitor_name).then_some((
                    cluster.id,
                    core.pos,
                    st.collision_extents_for_node(core),
                ))
            })
            .collect::<Vec<_>>();

        let mut changed = false;
        for (cluster_id, core_pos, core_ext) in cores {
            let dx = mover_pos.x - core_pos.x;
            let dy = mover_pos.y - core_pos.y;
            let req_x = st.required_sep_x(mover_pos.x, mover_ext, core_pos.x, core_ext, gap);
            let req_y = st.required_sep_y(mover_pos.y, mover_ext, core_pos.y, core_ext, gap);
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

pub(crate) fn enforce_pan_dominant_zone_states(st: &mut Halley, now_ms: u64) {
    let active_outside_ring_delay_ms = st.runtime.tuning.active_outside_ring_delay_ms;
    let inactive_outside_ring_delay_ms = st.runtime.tuning.inactive_outside_ring_delay_ms;

    let ids: Vec<NodeId> = st.model.field.nodes().keys().copied().collect();

    for id in ids {
        st.apply_single_surface_decay_policy(
            id,
            now_ms,
            active_outside_ring_delay_ms,
            inactive_outside_ring_delay_ms,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoomed_out_test_state() -> Halley {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.zoom_ref_size = Vec2 {
            x: 3840.0,
            y: 1080.0,
        };
        state.model.camera_target_view_size = state.model.zoom_ref_size;
        state.runtime.tuning.viewport_size = state.model.zoom_ref_size;
        state
    }

    #[test]
    fn node_visibility_uses_zoomed_field_viewport() {
        let mut state = zoomed_out_test_state();
        let monitor = state.model.monitor_state.current_monitor.clone();
        let id = state.model.field.spawn_surface(
            "visible-on-zoomed-field",
            Vec2 { x: 1200.0, y: 0.0 },
            Vec2 { x: 120.0, y: 80.0 },
        );

        assert_eq!(
            node_fully_visible_on_monitor(&state, monitor.as_str(), id),
            Some(true)
        );
    }

    #[test]
    fn edge_pan_clamp_uses_zoomed_field_viewport() {
        let mut state = zoomed_out_test_state();
        let monitor = state.model.monitor_state.current_monitor.clone();
        let id = state.model.field.spawn_surface(
            "dragged-on-zoomed-field",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 120.0, y: 80.0 },
        );
        let desired_center = Vec2 { x: 1200.0, y: 0.0 };

        let (clamped_center, edge_contact) = dragged_node_edge_pan_clamp(
            &state,
            monitor.as_str(),
            id,
            desired_center,
            Vec2 { x: 0.0, y: 0.0 },
        )
        .expect("clamp result");

        assert_eq!(clamped_center, desired_center);
        assert_eq!(edge_contact, Vec2 { x: 0.0, y: 0.0 });
    }
}
