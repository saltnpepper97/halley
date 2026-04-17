use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_config::PointerBindingAction;
use halley_core::cluster::ClusterId;
use halley_core::field::{NodeId, Vec2};
use smithay::input::pointer::{CursorIcon, MotionEvent, PointerHandle};
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::pointer_constraints::{PointerConstraint, with_pointer_constraint};

use crate::compositor::ctx::PointerCtx;
use crate::compositor::interaction::drag::DragCtx;
use crate::compositor::interaction::resize::ResizeCtx;
use crate::compositor::interaction::state::NodeMoveAnim;
use crate::compositor::root::Halley;
use crate::input::active_node_surface_transform_screen_details;

#[derive(Clone)]
pub(crate) struct BloomDragCtx {
    pub(crate) cluster_id: ClusterId,
    pub(crate) member_id: NodeId,
    pub(crate) monitor: String,
    pub(crate) slot_screen: (f32, f32),
}

#[derive(Clone)]
pub(crate) struct OverflowDragCtx {
    pub(crate) cluster_id: ClusterId,
    pub(crate) member_id: NodeId,
    pub(crate) monitor: String,
}

#[derive(Clone, Copy)]
pub(crate) struct HitNode {
    pub(crate) node_id: NodeId,
    pub(crate) move_surface: bool,
    pub(crate) is_core: bool,
}

pub(crate) const CORE_BLOOM_HOLD_MS: u64 = 1_700;

#[derive(Clone)]
pub(crate) struct PointerState {
    pub(crate) world: Vec2,
    pub(crate) screen: (f32, f32),
    pub(crate) workspace_size: (i32, i32),
    pub(crate) hover_node: Option<NodeId>,
    pub(crate) intercepted_buttons: HashMap<u32, PointerBindingAction>,
    pub(crate) intercepted_binding_buttons: HashSet<u32>,
    pub(crate) drag: Option<DragCtx>,
    pub(crate) resize: Option<ResizeCtx>,
    pub(crate) move_anim: HashMap<NodeId, NodeMoveAnim>,
    pub(crate) bloom_drag: Option<BloomDragCtx>,
    pub(crate) overflow_drag: Option<OverflowDragCtx>,
    pub(crate) panning: bool,
    pub(crate) pan_monitor: Option<String>,
    pub(crate) pan_last_screen: (f32, f32),
    pub(crate) left_button_down: bool,
    pub(crate) hover_started_at: Option<Instant>,
    pub(crate) preview_block_until: Option<Instant>,
    pub(crate) resize_trace_node: Option<NodeId>,
    pub(crate) resize_trace_until: Option<Instant>,
    pub(crate) resize_trace_last_at: Option<Instant>,
}

impl Default for PointerState {
    fn default() -> Self {
        Self {
            world: Vec2 { x: 0.0, y: 0.0 },
            screen: (0.0, 0.0),
            workspace_size: (1, 1),
            hover_node: None,
            intercepted_buttons: HashMap::new(),
            intercepted_binding_buttons: HashSet::new(),
            drag: None,
            resize: None,
            move_anim: HashMap::new(),
            bloom_drag: None,
            overflow_drag: None,
            panning: false,
            pan_monitor: None,
            pan_last_screen: (0.0, 0.0),
            left_button_down: false,
            hover_started_at: None,
            preview_block_until: None,
            resize_trace_node: None,
            resize_trace_until: None,
            resize_trace_last_at: None,
        }
    }
}

pub(crate) fn cursor_position_hint(
    ctx: &mut PointerCtx<'_>,
    surface: &WlSurface,
    pointer: &PointerHandle<Halley>,
    location: smithay::utils::Point<f64, smithay::utils::Logical>,
) {
    apply_cursor_position_hint(ctx.st, surface, pointer, location);
}

pub(crate) fn set_cursor_override_icon(st: &mut Halley, icon: Option<CursorIcon>) {
    st.input.interaction_state.cursor_override_icon = icon;
    st.input.interaction_state.cursor_override_until_ms = None;
}

pub(crate) fn set_temporary_cursor_override_icon(
    st: &mut Halley,
    icon: CursorIcon,
    now: Instant,
    duration_ms: u64,
) {
    st.input.interaction_state.cursor_override_icon = Some(icon);
    st.input.interaction_state.cursor_override_until_ms =
        Some(st.now_ms(now).saturating_add(duration_ms.max(1)));
    st.request_maintenance();
}

pub(crate) fn activate_pointer_constraint_for_surface(st: &mut Halley, surface: &WlSurface) {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    with_pointer_constraint(surface, &pointer, |constraint| {
        if let Some(constraint) = constraint
            && !constraint.is_active()
        {
            constraint.activate();
        }
    });
}

pub(crate) fn clear_pointer_focus(st: &mut Halley) {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    st.input.interaction_state.grabbed_layer_surface = None;
    if pointer.is_grabbed() {
        pointer.unset_grab(st, SERIAL_COUNTER.next_serial(), 0);
    }
    let location = pointer.current_location();
    pointer.motion(
        st,
        None,
        &MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        },
    );
    pointer.frame(st);
}

pub(crate) fn apply_cursor_position_hint(
    st: &mut Halley,
    surface: &WlSurface,
    pointer: &PointerHandle<Halley>,
    location: smithay::utils::Point<f64, smithay::utils::Logical>,
) {
    let Some(node_id) = st.model.surface_to_node.get(&surface.id()).copied() else {
        return;
    };
    let monitor = st
        .model
        .monitor_state
        .node_monitor
        .get(&node_id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    let (ws_w, ws_h, _, _) = st.local_screen_in_monitor(monitor.as_str(), 0.0, 0.0);
    let previous_monitor = st.begin_temporary_render_monitor(monitor.as_str());
    let Some(xform) =
        active_node_surface_transform_screen_details(st, ws_w, ws_h, node_id, Instant::now(), None)
    else {
        st.end_temporary_render_monitor(previous_monitor);
        return;
    };

    let sx =
        (xform.origin_x + location.x as f32 * xform.scale).clamp(0.0, (ws_w.max(1) - 1) as f32);
    let sy =
        (xform.origin_y + location.y as f32 * xform.scale).clamp(0.0, (ws_h.max(1) - 1) as f32);
    st.input.interaction_state.pending_pointer_screen_hint = Some((sx, sy));

    let cam_scale = st.camera_render_scale().max(0.001) as f64;
    let focus_origin = smithay::utils::Point::<f64, smithay::utils::Logical>::from((
        xform.origin_x as f64 / cam_scale,
        xform.origin_y as f64 / cam_scale,
    ));
    pointer.motion(
        st,
        Some((surface.clone(), focus_origin)),
        &MotionEvent {
            location: (sx as f64 / cam_scale, sy as f64 / cam_scale).into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        },
    );
    pointer.frame(st);
    st.end_temporary_render_monitor(previous_monitor);
}

pub(crate) fn release_active_pointer_constraint(st: &mut Halley) -> bool {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return false;
    };
    let Some(surface) = pointer.current_focus() else {
        return false;
    };
    let mut released = false;
    with_pointer_constraint(&surface, &pointer, |constraint| {
        if let Some(constraint) = constraint
            && constraint.is_active()
        {
            constraint.deactivate();
            released = true;
        }
    });
    if released {
        clear_pointer_focus(st);
        st.input.interaction_state.reset_input_state_requested = true;
    }
    released
}

pub(crate) fn active_constrained_pointer_surface(st: &Halley) -> Option<(WlSurface, bool)> {
    let pointer = st.platform.seat.get_pointer()?;
    let surface = pointer.current_focus()?;
    let is_locked = with_pointer_constraint(&surface, &pointer, |constraint| {
        let active = constraint
            .as_deref()
            .is_some_and(PointerConstraint::is_active);
        let locked = matches!(constraint.as_deref(), Some(PointerConstraint::Locked(_)));
        if active { Some(locked) } else { None }
    })?;
    Some((surface, is_locked))
}
