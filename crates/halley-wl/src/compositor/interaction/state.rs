use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use halley_config::CompositorBindingAction;
use halley_core::cluster::ClusterId;
use halley_core::field::{NodeId, Vec2};
use halley_core::viewport::Viewport;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::compositor::interaction::drag::DragAxisMode;
use crate::compositor::root::Halley;
use crate::compositor::screenshot::state::{
    InflightScreenshotCapture, PendingScreenshotCapture, ScreenshotCaptureResult,
    ScreenshotSessionState,
};
use smithay::input::pointer::CursorIcon;

const BLOOM_PULL_SLOP_PX: f32 = 12.0;
const BLOOM_TETHER_MAX_PX: f32 = 60.0;
const BLOOM_TETHER_SOFTNESS_PX: f32 = 30.0;
const BLOOM_DETACH_HOLD_MS: u64 = 1200;
const BLOOM_SNAPBACK_MS: u64 = 170;

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

pub(crate) fn trap_modal_key_release(st: &mut Halley, code: u32) {
    st.input.interaction_state.modal_release_keys.insert(code);
}

impl ModState {
    pub(crate) fn clear_intercepts(&mut self) {
        self.intercepted_keys.clear();
        self.intercepted_compositor_actions.clear();
    }
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
    pub(crate) last_edge_pan_at: Instant,
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
pub(crate) enum BloomPullPhase {
    Pressed,
    Tethered {
        started_at_ms: u64,
    },
    Snapback {
        started_at_ms: u64,
        from_offset: Vec2,
    },
}

#[derive(Clone)]
pub(crate) struct BloomPullPreview {
    pub(crate) cluster_id: ClusterId,
    pub(crate) member_id: NodeId,
    pub(crate) monitor: String,
    pub(crate) core_screen: Vec2,
    pub(crate) slot_screen: Vec2,
    pub(crate) pointer_screen: Vec2,
    pub(crate) display_offset: Vec2,
    pub(crate) hold_progress: f32,
    pub(crate) phase: BloomPullPhase,
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
pub(crate) struct PendingCoreHover {
    pub(crate) node_id: NodeId,
    pub(crate) monitor: String,
    pub(crate) started_at_ms: u64,
}

#[derive(Clone)]
pub(crate) struct PendingCorePress {
    pub(crate) node_id: NodeId,
    pub(crate) monitor: String,
    pub(crate) press_global_sx: f32,
    pub(crate) press_global_sy: f32,
}

#[derive(Clone)]
pub(crate) struct PendingCollapsedNodePress {
    pub(crate) node_id: NodeId,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingMovePress {
    pub(crate) node_id: NodeId,
    pub(crate) press_global_sx: f32,
    pub(crate) press_global_sy: f32,
    pub(crate) workspace_active: bool,
}

#[derive(Clone)]
pub(crate) struct PendingCoreClick {
    pub(crate) node_id: NodeId,
    pub(crate) monitor: String,
    pub(crate) deadline_ms: u64,
}

#[derive(Clone)]
pub(crate) struct PendingCollapsedNodeClick {
    pub(crate) node_id: NodeId,
    pub(crate) deadline_ms: u64,
}

#[derive(Clone, Copy)]
pub(crate) enum ClusterNamePromptRepeatAction {
    Insert(char),
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
}

#[derive(Clone)]
pub(crate) struct ClusterNamePromptRepeatState {
    pub(crate) monitor: String,
    pub(crate) code: u32,
    pub(crate) action: ClusterNamePromptRepeatAction,
    pub(crate) next_repeat_ms: u64,
    pub(crate) interval_ms: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingModalFocusRestore {
    pub(crate) target: Option<NodeId>,
    pub(crate) restore_at_ms: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct FocusCycleImmersiveOrigin {
    pub(crate) node_id: NodeId,
    pub(crate) monitor: String,
    pub(crate) saved_camera_center: Vec2,
    pub(crate) saved_zoom_view_size: Vec2,
}

#[derive(Clone, Debug)]
pub(crate) struct FocusCycleSession {
    pub(crate) candidates: Vec<NodeId>,
    pub(crate) preview_index: usize,
    pub(crate) origin_focus: Option<NodeId>,
    pub(crate) immersive_origin: Option<FocusCycleImmersiveOrigin>,
    pub(crate) immersive_lock_released: bool,
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
    pub(crate) grabbed_layer_surface: Option<WlSurface>,
    pub(crate) cluster_name_prompt_drag_monitor: Option<String>,
    pub(crate) cluster_name_prompt_repeat: Option<ClusterNamePromptRepeatState>,
    pub(crate) screenshot_session: Option<ScreenshotSessionState>,
    pub(crate) pending_screenshot_capture: Option<PendingScreenshotCapture>,
    pub(crate) inflight_screenshot_capture: Option<InflightScreenshotCapture>,
    pub(crate) screenshot_next_serial: u64,
    pub(crate) last_screenshot_result: Option<ScreenshotCaptureResult>,
    pub(crate) modal_release_keys: HashSet<u32>,
    pub(crate) pending_modal_focus_restore: Option<PendingModalFocusRestore>,
    pub(crate) focus_cycle_session: Option<FocusCycleSession>,
    pub(crate) overlay_hover_target: Option<OverlayHoverTarget>,
    pub(crate) cursor_override_until_ms: Option<u64>,
    pub(crate) pending_core_hover: Option<PendingCoreHover>,
    pub(crate) pending_core_press: Option<PendingCorePress>,
    pub(crate) pending_collapsed_node_press: Option<PendingCollapsedNodePress>,
    pub(crate) pending_move_press: Option<PendingMovePress>,
    pub(crate) pending_core_click: Option<PendingCoreClick>,
    pub(crate) pending_collapsed_node_click: Option<PendingCollapsedNodeClick>,
    pub(crate) grabbed_edge_pan_active: bool,
    pub(crate) grabbed_edge_pan_direction: Vec2,
    pub(crate) grabbed_edge_pan_pressure: Vec2,
    pub(crate) grabbed_edge_pan_monitor: Option<String>,
    pub(crate) cursor_override_icon: Option<CursorIcon>,
    pub(crate) cursor_hidden_by_typing: bool,
    pub(crate) last_cursor_activity_at_ms: u64,
}

pub(crate) fn note_cursor_activity(st: &mut Halley, now_ms: u64) -> bool {
    let hide_after_ms = st.runtime.tuning.cursor.hide_after_ms;
    let was_idle_hidden = hide_after_ms > 0
        && now_ms.saturating_sub(st.input.interaction_state.last_cursor_activity_at_ms)
            >= hide_after_ms;

    st.input.interaction_state.last_cursor_activity_at_ms = now_ms;

    let was_typing_hidden = std::mem::take(&mut st.input.interaction_state.cursor_hidden_by_typing);

    was_idle_hidden || was_typing_hidden
}

pub(crate) fn note_typing_activity(st: &mut Halley, now_ms: u64) -> bool {
    if !st.runtime.tuning.cursor.hide_while_typing {
        return false;
    }

    let hide_after_ms = st.runtime.tuning.cursor.hide_after_ms;
    if hide_after_ms > 0
        && now_ms.saturating_sub(st.input.interaction_state.last_cursor_activity_at_ms)
            < hide_after_ms
    {
        return false;
    }

    let changed = !st.input.interaction_state.cursor_hidden_by_typing;
    st.input.interaction_state.cursor_hidden_by_typing = true;
    changed
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
pub(crate) fn bloom_pull_slop_px() -> f32 {
    BLOOM_PULL_SLOP_PX
}

#[inline]
pub(crate) fn bloom_detach_hold_ms() -> u64 {
    BLOOM_DETACH_HOLD_MS
}

#[cfg(test)]
#[inline]
pub(crate) fn bloom_tether_max_px() -> f32 {
    BLOOM_TETHER_MAX_PX
}

pub(crate) fn bloom_pull_constrained_offset(raw_offset: Vec2) -> Vec2 {
    let raw_len = raw_offset.x.hypot(raw_offset.y);
    if raw_len <= f32::EPSILON {
        return Vec2 { x: 0.0, y: 0.0 };
    }
    let display_len = BLOOM_TETHER_MAX_PX * (1.0 - (-raw_len / BLOOM_TETHER_SOFTNESS_PX).exp());
    let scale = display_len / raw_len;
    Vec2 {
        x: raw_offset.x * scale,
        y: raw_offset.y * scale,
    }
}

#[inline]
pub(crate) fn bloom_pull_display_mix(display_offset: Vec2) -> f32 {
    (display_offset.x.hypot(display_offset.y) / BLOOM_TETHER_MAX_PX).clamp(0.0, 1.0)
}

#[inline]
fn bloom_snapback_progress(started_at_ms: u64, now_ms: u64) -> f32 {
    (now_ms.saturating_sub(started_at_ms) as f32 / BLOOM_SNAPBACK_MS.max(1) as f32).clamp(0.0, 1.0)
}

pub(crate) fn bloom_pull_preview_needs_animation(st: &Halley) -> bool {
    st.input
        .interaction_state
        .bloom_pull_preview
        .as_ref()
        .is_some_and(|preview| match preview.phase {
            BloomPullPhase::Pressed => false,
            BloomPullPhase::Tethered { .. } | BloomPullPhase::Snapback { .. } => true,
        })
}

pub(crate) fn bloom_pull_preview_active_for_monitor(st: &Halley, monitor: &str) -> bool {
    st.input
        .interaction_state
        .bloom_pull_preview
        .as_ref()
        .is_some_and(|preview| preview.monitor == monitor)
}

pub(crate) fn tick_bloom_pull_preview(st: &mut Halley, now_ms: u64) {
    let mut clear_preview = false;
    if let Some(preview) = st.input.interaction_state.bloom_pull_preview.as_mut() {
        match preview.phase.clone() {
            BloomPullPhase::Pressed => {
                preview.hold_progress = 0.0;
            }
            BloomPullPhase::Tethered { started_at_ms } => {
                preview.hold_progress = (now_ms.saturating_sub(started_at_ms) as f32
                    / BLOOM_DETACH_HOLD_MS.max(1) as f32)
                    .clamp(0.0, 1.0);
            }
            BloomPullPhase::Snapback {
                started_at_ms,
                from_offset,
            } => {
                let t = bloom_snapback_progress(started_at_ms, now_ms);
                let eased = 1.0 - (1.0 - t).powi(3);
                preview.display_offset = Vec2 {
                    x: from_offset.x * (1.0 - eased),
                    y: from_offset.y * (1.0 - eased),
                };
                preview.pointer_screen = Vec2 {
                    x: preview.slot_screen.x + preview.display_offset.x,
                    y: preview.slot_screen.y + preview.display_offset.y,
                };
                preview.hold_progress = 0.0;
                if t >= 1.0 {
                    clear_preview = true;
                }
            }
        }
    }
    if clear_preview {
        st.input.interaction_state.bloom_pull_preview = None;
    }
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
    fn bloom_pull_constraint_stays_bounded() {
        let offset = bloom_pull_constrained_offset(Vec2 { x: 400.0, y: 0.0 });
        assert!(offset.x <= bloom_tether_max_px() + 0.01);
        assert!(offset.y.abs() <= 0.01);
    }

    #[test]
    fn bloom_pull_display_mix_tracks_constraint_extent() {
        let offset = bloom_pull_constrained_offset(Vec2 { x: 18.0, y: 0.0 });
        let mix = bloom_pull_display_mix(offset);
        assert!(mix > 0.0);
        assert!(mix <= 1.0);
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

    #[test]
    fn hide_cursor_for_typing_waits_for_delay_and_pointer_reveals_it() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cursor.hide_while_typing = true;
        tuning.cursor.hide_after_ms = 2_000;
        let mut state = Halley::new_for_test(&dh, tuning);
        state.input.interaction_state.last_cursor_activity_at_ms = 10_000;

        assert!(!note_typing_activity(&mut state, 11_999));
        assert!(!state.input.interaction_state.cursor_hidden_by_typing);
        assert!(note_typing_activity(&mut state, 12_000));
        assert!(state.input.interaction_state.cursor_hidden_by_typing);
        assert!(!note_typing_activity(&mut state, 12_100));
        assert!(note_cursor_activity(&mut state, 20_001));
        assert!(!state.input.interaction_state.cursor_hidden_by_typing);
        assert!(!note_cursor_activity(&mut state, 20_002));
    }

    #[test]
    fn effective_cursor_image_status_respects_client_hide_when_disabled() {
        use crate::compositor::platform::effective_cursor_image_status;
        use smithay::input::pointer::CursorImageStatus;

        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();

        // Case 1: hide-while-typing is false, client requests hidden
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cursor.hide_while_typing = false;
        let mut state = Halley::new_for_test(&dh, tuning);
        state.platform.cursor_image_status = CursorImageStatus::Hidden;

        // Should return hidden (respecting the client)
        assert!(matches!(
            effective_cursor_image_status(&state),
            CursorImageStatus::Hidden
        ));

        // Case 2: hide-while-typing is true, client requests hidden
        state.runtime.tuning.cursor.hide_while_typing = true;
        assert!(matches!(
            effective_cursor_image_status(&state),
            CursorImageStatus::Hidden
        ));
    }
}
