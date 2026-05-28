use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_config::PointerBindingAction;
use halley_core::cluster::ClusterId;
use halley_core::field::{NodeId, Vec2};
use smithay::input::pointer::{CursorIcon, MotionEvent, PointerHandle};
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
use smithay::utils::SERIAL_COUNTER;
use smithay::utils::{Logical, Point};
use smithay::wayland::{
    compositor::{RegionAttributes, SubsurfaceCachedState, get_parent, with_states},
    pointer_constraints::{PointerConstraint, with_pointer_constraint},
};

use crate::compositor::ctx::PointerCtx;
use crate::compositor::interaction::drag::DragCtx;
use crate::compositor::interaction::resize::ResizeCtx;
use crate::compositor::interaction::state::{NodeMoveAnim, PointerContents};
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

#[derive(Clone, Debug)]
pub(crate) struct ActivePointerConstraint {
    pub(crate) surface: WlSurface,
    pub(crate) locked: bool,
    pub(crate) region: Option<RegionAttributes>,
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

pub(crate) fn activate_pointer_constraint_for_surface_at(
    st: &mut Halley,
    surface: &WlSurface,
    surface_origin: Option<smithay::utils::Point<f64, smithay::utils::Logical>>,
) {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    let mut current = surface.clone();
    let mut current_origin = surface_origin;
    loop {
        let activated = with_pointer_constraint(&current, &pointer, |constraint| {
            if let Some(constraint) = constraint
                && !constraint.is_active()
            {
                let origin = current_origin;
                if let Some(region) = constraint.region() {
                    let Some(origin) = origin else {
                        return false;
                    };
                    let pos_within_surface = pointer.current_location() - origin;
                    if !region.contains(pos_within_surface.to_i32_round()) {
                        return false;
                    }
                }
                constraint.activate();
                true
            } else {
                false
            }
        });
        if activated {
            break;
        }
        if let Some(parent) = get_parent(&current) {
            if let Some(origin) = current_origin.as_mut() {
                let location = with_states(&current, |states| {
                    states
                        .cached_state
                        .get::<SubsurfaceCachedState>()
                        .current()
                        .location
                });
                origin.x -= location.x as f64;
                origin.y -= location.y as f64;
            }
            current = parent;
        } else {
            break;
        }
    }
}

pub(crate) fn maybe_activate_pointer_constraint(st: &mut Halley, now: Instant) {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    let Some(current_focus) = pointer.current_focus() else {
        return;
    };
    let focus = pointer_focus_at_last_screen(st, None, now).and_then(|(focus, _, _)| focus);
    let Some((surface, surface_origin)) = focus else {
        return;
    };
    if surface != current_focus {
        return;
    }
    activate_pointer_constraint_for_surface_at(st, &surface, Some(surface_origin));
}

fn pointer_focus_at_last_screen(
    st: &mut Halley,
    resize_preview: Option<ResizeCtx>,
    now: Instant,
) -> Option<(
    Option<(WlSurface, Point<f64, Logical>)>,
    Point<f64, Logical>,
    PointerContents,
)> {
    let pointer = st.platform.seat.get_pointer()?;
    let (global_sx, global_sy) = st
        .input
        .interaction_state
        .last_pointer_screen_global
        .unwrap_or_else(|| {
            let location = pointer.current_location();
            let cam_scale = st.camera_render_scale().max(0.001) as f64;
            let monitor = st.model.monitor_state.current_monitor.as_str();
            st.model
                .monitor_state
                .monitors
                .get(monitor)
                .map(|space| {
                    (
                        space.offset_x as f32 + (location.x * cam_scale) as f32,
                        space.offset_y as f32 + (location.y * cam_scale) as f32,
                    )
                })
                .unwrap_or((
                    (location.x * cam_scale) as f32,
                    (location.y * cam_scale) as f32,
                ))
        });
    let monitor = st.monitor_for_screen_or_interaction(global_sx, global_sy);
    let (ws_w, ws_h, local_sx, local_sy) =
        st.local_screen_in_monitor(monitor.as_str(), global_sx, global_sy);
    let focus = crate::input::pointer::focus::pointer_focus_for_screen(
        st,
        ws_w,
        ws_h,
        local_sx,
        local_sy,
        now,
        resize_preview,
    );
    let location = if focus.as_ref().is_some_and(|(surface, _)| {
        crate::compositor::monitor::layer_shell::is_layer_surface(st, surface)
            || crate::protocol::wayland::session_lock::is_session_lock_surface(st, surface)
    }) {
        (local_sx as f64, local_sy as f64).into()
    } else {
        let cam_scale = st.camera_render_scale() as f64;
        (local_sx as f64 / cam_scale, local_sy as f64 / cam_scale).into()
    };
    let contents = pointer_contents_for_focus(st, monitor, focus.as_ref());
    Some((focus, location, contents))
}

fn pointer_contents_for_focus(
    st: &Halley,
    monitor: String,
    focus: Option<&(WlSurface, Point<f64, Logical>)>,
) -> PointerContents {
    let surface = focus.map(|(surface, _)| surface.id());
    let root_surface = focus.map(|(surface, _)| surface_tree_root(surface).id());
    let node_id = root_surface
        .as_ref()
        .and_then(|id| st.model.surface_to_node.get(id).copied());
    let is_layer_surface = focus.is_some_and(|(surface, _)| {
        crate::compositor::monitor::layer_shell::is_layer_surface(st, surface)
    });
    let is_session_lock_surface = focus.is_some_and(|(surface, _)| {
        crate::protocol::wayland::session_lock::is_session_lock_surface(st, surface)
    });

    PointerContents {
        monitor: Some(monitor),
        surface,
        root_surface,
        node_id,
        is_layer_surface,
        is_session_lock_surface,
    }
}

pub(crate) fn update_pointer_contents_from_focus(
    st: &mut Halley,
    monitor: String,
    focus: Option<&(WlSurface, Point<f64, Logical>)>,
) -> bool {
    let contents = pointer_contents_for_focus(st, monitor, focus);
    if st.input.interaction_state.pointer_contents == contents {
        return false;
    }
    st.input.interaction_state.pointer_contents = contents;
    true
}

pub(crate) fn refresh_pointer_focus_at_last_screen(
    st: &mut Halley,
    resize_preview: Option<ResizeCtx>,
    now: Instant,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    let pointer = st.platform.seat.get_pointer()?;
    let (focus, location, contents) = pointer_focus_at_last_screen(st, resize_preview, now)?;

    pointer.motion(
        st,
        focus.clone(),
        &MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: crate::input::pointer::button::now_millis_u32(),
        },
    );
    pointer.frame(st);
    st.input.interaction_state.pointer_contents = contents;
    maybe_activate_pointer_constraint(st, now);
    focus
}

pub(crate) fn update_pointer_contents_at_last_screen(
    st: &mut Halley,
    resize_preview: Option<ResizeCtx>,
    now: Instant,
) -> bool {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return false;
    };
    if pointer.is_grabbed() {
        return false;
    }
    let Some((focus, location, contents)) = pointer_focus_at_last_screen(st, resize_preview, now)
    else {
        return false;
    };
    if st.input.interaction_state.pointer_contents == contents {
        return false;
    }

    pointer.motion(
        st,
        focus,
        &MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: crate::input::pointer::button::now_millis_u32(),
        },
    );
    pointer.frame(st);
    st.input.interaction_state.pointer_contents = contents;
    maybe_activate_pointer_constraint(st, now);
    true
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
    st.input.interaction_state.pointer_contents = PointerContents::default();
}

pub(crate) fn center_pointer_on_node(st: &mut Halley, node_id: NodeId, now: Instant) -> bool {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return false;
    };
    let Some(node_pos) = st.model.field.node(node_id).map(|node| node.pos) else {
        return false;
    };

    let monitor = st
        .model
        .monitor_state
        .node_monitor
        .get(&node_id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    let Some(space) = st.model.monitor_state.monitors.get(monitor.as_str()) else {
        return false;
    };
    let monitor_width = space.width;
    let monitor_height = space.height;
    let monitor_offset_x = space.offset_x;
    let monitor_offset_y = space.offset_y;

    let previous_monitor = st.begin_temporary_render_monitor(monitor.as_str());
    let (local_sx, local_sy) = crate::presentation::world_to_screen(
        st,
        monitor_width,
        monitor_height,
        node_pos.x,
        node_pos.y,
    );
    let local_sx_f = local_sx.clamp(0, monitor_width.saturating_sub(1).max(0)) as f32;
    let local_sy_f = local_sy.clamp(0, monitor_height.saturating_sub(1).max(0)) as f32;
    let global_sx = monitor_offset_x as f32 + local_sx_f;
    let global_sy = monitor_offset_y as f32 + local_sy_f;
    st.input.interaction_state.pending_pointer_screen_hint = Some((global_sx, global_sy));

    let focus = crate::input::pointer::focus::pointer_focus_for_screen(
        st,
        monitor_width,
        monitor_height,
        local_sx_f,
        local_sy_f,
        now,
        None,
    );
    let cam_scale = st.camera_render_scale().max(0.001) as f64;
    pointer.motion(
        st,
        focus,
        &MotionEvent {
            location: (local_sx_f as f64 / cam_scale, local_sy_f as f64 / cam_scale).into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        },
    );
    pointer.frame(st);
    st.end_temporary_render_monitor(previous_monitor);
    st.request_maintenance();
    true
}

pub(crate) fn apply_cursor_position_hint(
    st: &mut Halley,
    surface: &WlSurface,
    pointer: &PointerHandle<Halley>,
    location: smithay::utils::Point<f64, smithay::utils::Logical>,
) {
    let mut constraint_active = false;
    let mut current = surface.clone();
    loop {
        let active = with_pointer_constraint(&current, pointer, |constraint| {
            constraint.is_some_and(|constraint| constraint.is_active())
        });
        if active {
            constraint_active = true;
            break;
        }
        if let Some(parent) = get_parent(&current) {
            current = parent;
        } else {
            break;
        }
    }
    if !constraint_active {
        return;
    }

    let root = surface_tree_root(surface);
    let pointer_focus_matches = pointer
        .current_focus()
        .as_ref()
        .is_some_and(|focus| surface_tree_root(focus).id() == root.id());
    if !pointer_focus_matches {
        return;
    }

    let Some(node_id) = st.model.surface_to_node.get(&root.id()).copied() else {
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
    let (global_sx, global_sy) = st
        .model
        .monitor_state
        .monitors
        .get(monitor.as_str())
        .map(|space| (space.offset_x as f32 + sx, space.offset_y as f32 + sy))
        .unwrap_or((sx, sy));
    st.input.interaction_state.pending_pointer_screen_hint = Some((global_sx, global_sy));

    let cam_scale = st.camera_render_scale().max(0.001) as f64;
    pointer.set_location((sx as f64 / cam_scale, sy as f64 / cam_scale).into());
    st.end_temporary_render_monitor(previous_monitor);
    st.request_maintenance();
}

fn surface_tree_root(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}

pub(crate) fn release_active_pointer_constraint(st: &mut Halley) -> bool {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return false;
    };
    let Some(focus) = pointer.current_focus() else {
        return false;
    };
    let mut released = false;
    let mut current = focus;
    loop {
        with_pointer_constraint(&current, &pointer, |constraint| {
            if let Some(constraint) = constraint
                && constraint.is_active()
            {
                constraint.deactivate();
                released = true;
            }
        });
        if released {
            break;
        }
        if let Some(parent) = get_parent(&current) {
            current = parent;
        } else {
            break;
        }
    }
    if released {
        clear_pointer_focus(st);
        st.input.interaction_state.reset_input_state_requested = true;
    }
    released
}

pub(crate) fn active_constrained_pointer_surface(st: &Halley) -> Option<(WlSurface, bool)> {
    active_pointer_constraint(st).map(|constraint| (constraint.surface, constraint.locked))
}

pub(crate) fn active_pointer_constraint(st: &Halley) -> Option<ActivePointerConstraint> {
    let pointer = st.platform.seat.get_pointer()?;
    let focus = pointer.current_focus()?;
    let mut current = focus;
    loop {
        let res = with_pointer_constraint(&current, &pointer, |constraint| {
            let constraint = constraint.as_deref()?;
            if !constraint.is_active() {
                return None;
            }
            Some(ActivePointerConstraint {
                surface: current.clone(),
                locked: matches!(constraint, PointerConstraint::Locked(_)),
                region: constraint.region().cloned(),
            })
        });
        if res.is_some() {
            return res;
        }
        if let Some(parent) = get_parent(&current) {
            current = parent;
        } else {
            break;
        }
    }
    None
}

pub(crate) fn find_constrained_surface_in_hierarchy(
    st: &Halley,
    surface: &WlSurface,
) -> Option<WlSurface> {
    let pointer = st.platform.seat.get_pointer()?;
    let mut current = surface.clone();
    loop {
        let has_constraint = with_pointer_constraint(&current, &pointer, |c| c.is_some());
        if has_constraint {
            return Some(current);
        }
        if let Some(parent) = get_parent(&current) {
            current = parent;
        } else {
            break;
        }
    }
    None
}
