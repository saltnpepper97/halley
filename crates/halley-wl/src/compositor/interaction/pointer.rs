use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_config::PointerBindingAction;
use halley_core::cluster::ClusterId;
use halley_core::field::{NodeId, Vec2};
use smithay::input::pointer::{CursorIcon, MotionEvent, PointerHandle};
use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};
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
    pub(crate) origin: Point<f64, Logical>,
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
    pub(crate) touchpad_scroll_step_accum_y: f32,
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
            touchpad_scroll_step_accum_y: 0.0,
            left_button_down: false,
            hover_started_at: None,
            preview_block_until: None,
            resize_trace_node: None,
            resize_trace_until: None,
            resize_trace_last_at: None,
        }
    }
}

impl PointerCtx<'_> {
    pub(crate) fn apply_cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Halley>,
        location: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) {
        apply_cursor_position_hint(self.st, surface, pointer, location);
    }
}

pub(crate) fn cursor_position_hint(
    ctx: &mut PointerCtx<'_>,
    surface: &WlSurface,
    pointer: &PointerHandle<Halley>,
    location: smithay::utils::Point<f64, smithay::utils::Logical>,
) {
    ctx.apply_cursor_position_hint(surface, pointer, location);
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
        crate::compositor::monitor::layer_shell::is_layer_surface_tree(st, surface)
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
        crate::compositor::monitor::layer_shell::is_layer_surface_tree(st, surface)
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

fn pointer_surface_origin_for_focus(
    focus: Option<&(WlSurface, Point<f64, Logical>)>,
) -> Option<(ObjectId, f64, f64)> {
    focus.map(|(surface, origin)| (surface.id(), origin.x, origin.y))
}

fn surface_offset_to_ancestor(
    surface: &WlSurface,
    ancestor: &WlSurface,
) -> Option<Point<f64, Logical>> {
    let mut current = surface.clone();
    let mut offset = Point::<f64, Logical>::from((0.0, 0.0));
    loop {
        if current == *ancestor {
            return Some(offset);
        }
        let location = with_states(&current, |states| {
            states
                .cached_state
                .get::<SubsurfaceCachedState>()
                .current()
                .location
        });
        offset.x += location.x as f64;
        offset.y += location.y as f64;
        current = get_parent(&current)?;
    }
}

fn related_surface_origin(
    focused: &WlSurface,
    focused_origin: Point<f64, Logical>,
    target: &WlSurface,
) -> Option<Point<f64, Logical>> {
    if focused == target {
        return Some(focused_origin);
    }
    if let Some(offset) = surface_offset_to_ancestor(target, focused) {
        return Some((focused_origin.x + offset.x, focused_origin.y + offset.y).into());
    }
    if let Some(offset) = surface_offset_to_ancestor(focused, target) {
        return Some((focused_origin.x - offset.x, focused_origin.y - offset.y).into());
    }
    None
}

pub(crate) fn constrained_focus_in_hierarchy(
    st: &Halley,
    focus: &(WlSurface, Point<f64, Logical>),
) -> Option<(WlSurface, Point<f64, Logical>)> {
    let constrained = find_constrained_surface_in_hierarchy(st, &focus.0)?;
    let origin = related_surface_origin(&focus.0, focus.1, &constrained)?;
    Some((constrained, origin))
}

pub(crate) fn current_pointer_focus_with_origin(
    st: &mut Halley,
    pointer: &PointerHandle<Halley>,
    now: Instant,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    let focused = pointer.current_focus()?;
    if let Some((surface_id, x, y)) = st.input.interaction_state.pointer_surface_origin.as_ref()
        && *surface_id == focused.id()
    {
        return Some((focused, (*x, *y).into()));
    }

    let (focus, _, _) = pointer_focus_at_last_screen(st, None, now)?;
    let (surface, origin) = focus?;
    (surface == focused).then_some((surface, origin))
}

pub(crate) fn retarget_pointer_focus_for_constraint(
    st: &mut Halley,
    surface: &WlSurface,
    pointer: &PointerHandle<Halley>,
    current_focus: Option<&(WlSurface, Point<f64, Logical>)>,
) -> Option<Point<f64, Logical>> {
    let (focused, focused_origin) = current_focus?;
    let origin = related_surface_origin(focused, *focused_origin, surface)?;
    if pointer.current_focus().as_ref() != Some(surface) {
        pointer.motion(
            st,
            Some((surface.clone(), origin)),
            &MotionEvent {
                location: pointer.current_location(),
                serial: SERIAL_COUNTER.next_serial(),
                time: crate::input::pointer::button::now_millis_u32(),
            },
        );
        pointer.frame(st);
    }
    let focus = (surface.clone(), origin);
    let monitor = st.monitor_for_constrained_surface_or_current(surface);
    update_pointer_contents_from_focus(st, monitor, Some(&focus));
    Some(origin)
}

pub(crate) fn update_pointer_contents_from_focus(
    st: &mut Halley,
    monitor: String,
    focus: Option<&(WlSurface, Point<f64, Logical>)>,
) -> bool {
    let contents = pointer_contents_for_focus(st, monitor, focus);
    st.input.interaction_state.pointer_surface_origin = pointer_surface_origin_for_focus(focus);
    st.input.interaction_state.pointer_focus = focus.cloned();
    if st.input.interaction_state.pointer_contents == contents {
        return false;
    }
    st.input.interaction_state.pointer_contents = contents;
    true
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
        focus.clone(),
        &MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: crate::input::pointer::button::now_millis_u32(),
        },
    );
    pointer.frame(st);
    st.input.interaction_state.pointer_contents = contents;
    st.input.interaction_state.pointer_surface_origin =
        pointer_surface_origin_for_focus(focus.as_ref());
    st.input.interaction_state.pointer_focus = focus;
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
    st.input.interaction_state.pointer_surface_origin = None;
    st.input.interaction_state.pointer_focus = None;
    st.input.interaction_state.recent_locked_pointer_target = None;
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

fn pointer_trace_enabled() -> bool {
    std::env::var_os("HALLEY_POINTER_TRACE").is_some_and(|value| value != "0")
}

fn trace_cursor_position_hint(
    label: &str,
    surface: &WlSurface,
    root: &WlSurface,
    detail: impl std::fmt::Display,
) {
    if pointer_trace_enabled() {
        eventline::info!(
            "cursor_position_hint {label} surface={:?} root={:?} {detail}",
            surface.id(),
            root.id(),
        );
    }
}

pub(crate) fn apply_cursor_position_hint(
    st: &mut Halley,
    surface: &WlSurface,
    pointer: &PointerHandle<Halley>,
    location: smithay::utils::Point<f64, smithay::utils::Logical>,
) {
    let mut constraint_active = false;
    let mut constraint_locked = false;
    let mut current = surface.clone();
    loop {
        let active = with_pointer_constraint(&current, pointer, |constraint| {
            constraint.and_then(|constraint| {
                constraint
                    .is_active()
                    .then(|| (true, matches!(&*constraint, PointerConstraint::Locked(_))))
            })
        });
        if let Some((active, locked)) = active
            && active
        {
            constraint_active = true;
            constraint_locked = locked;
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
    let root_id = root.id();
    let pointer_focus_matches = pointer
        .current_focus()
        .as_ref()
        .is_some_and(|focus| surface_tree_root(focus).id() == root_id);
    let pointer_contents_matches = st
        .input
        .interaction_state
        .pointer_contents
        .root_surface
        .as_ref()
        .is_some_and(|contents_root| contents_root == &root_id);
    if !pointer_focus_matches && !pointer_contents_matches {
        trace_cursor_position_hint(
            "reject",
            surface,
            &root,
            format_args!(
                "focus_match=false contents_match=false location={:.1},{:.1}",
                location.x, location.y,
            ),
        );
        return;
    }

    if constraint_locked
        && let Some(constraint) =
            active_pointer_constraint(st).filter(|constraint| constraint.locked)
        && let Some(surface_origin) =
            related_surface_origin(&constraint.surface, constraint.origin, surface)
    {
        // Clamp the hinted position to the locked surface's own bounds. A game that
        // warps-to-recenter can hint a location outside its window (or, cross-monitor,
        // one that resolves onto another output); without this the cursor gets flung
        // off the game's monitor. Mirrors Niri's cursor_position_hint output-constrain,
        // but in Halley's surface-local space so it's independent of camera/monitor
        // coords. A no-op for well-behaved in-bounds hints.
        let (hint_x, hint_y) = st
            .model
            .surface_to_node
            .get(&root.id())
            .copied()
            .and_then(|node_id| {
                crate::compositor::surface::current_surface_size_for_node(st, node_id)
            })
            .map(|size| {
                (
                    location.x.clamp(0.0, (size.x as f64 - 1.0).max(0.0)),
                    location.y.clamp(0.0, (size.y as f64 - 1.0).max(0.0)),
                )
            })
            .unwrap_or((location.x, location.y));
        let pointer_location =
            Point::<f64, Logical>::from((surface_origin.x + hint_x, surface_origin.y + hint_y));
        pointer.set_location(pointer_location);
        trace_cursor_position_hint(
            "sync-locked",
            surface,
            &root,
            format_args!(
                "location={:.1},{:.1} clamped={:.1},{:.1} origin={:.1},{:.1} pointer={:.1},{:.1}",
                location.x,
                location.y,
                hint_x,
                hint_y,
                surface_origin.x,
                surface_origin.y,
                pointer_location.x,
                pointer_location.y,
            ),
        );
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
    let Some(surface_offset) = surface_offset_to_ancestor(surface, &root) else {
        st.end_temporary_render_monitor(previous_monitor);
        return;
    };

    let sx = (xform.origin_x + (surface_offset.x + location.x) as f32 * xform.scale)
        .clamp(0.0, (ws_w.max(1) - 1) as f32);
    let sy = (xform.origin_y + (surface_offset.y + location.y) as f32 * xform.scale)
        .clamp(0.0, (ws_h.max(1) - 1) as f32);
    trace_cursor_position_hint(
        "accept",
        surface,
        &root,
        format_args!(
            "focus_match={} contents_match={} location={:.1},{:.1} screen={:.1},{:.1}",
            pointer_focus_matches, pointer_contents_matches, location.x, location.y, sx, sy,
        ),
    );
    let (global_sx, global_sy) = st
        .model
        .monitor_state
        .monitors
        .get(monitor.as_str())
        .map(|space| (space.offset_x as f32 + sx, space.offset_y as f32 + sy))
        .unwrap_or((sx, sy));
    let cam_scale = st.camera_render_scale().max(0.001) as f64;
    st.input.interaction_state.pending_pointer_screen_hint = Some((global_sx, global_sy));

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

fn trace_pointer_constraint_active(source: &str, constraint: &ActivePointerConstraint) {
    if pointer_trace_enabled() {
        eventline::info!(
            "pointer_constraint active source={} surface={:?} locked={} origin={:.1},{:.1}",
            source,
            constraint.surface.id(),
            constraint.locked,
            constraint.origin.x,
            constraint.origin.y,
        );
    }
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

/// True when the pointer currently holds an active lock/confine on a game-like
/// surface (a `steam_app_*` toplevel, or any surface that is fullscreen on an
/// output). Used to suppress Halley's own overlay reveals (cluster overflow,
/// etc.) while a game owns the pointer, so they cannot pop over the game —
/// windowed or fullscreen.
pub(crate) fn pointer_holds_game_constraint(st: &Halley) -> bool {
    let Some(constraint) = active_pointer_constraint(st) else {
        return false;
    };
    let mut root = constraint.surface;
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    st.model
        .surface_to_node
        .get(&root.id())
        .copied()
        .is_some_and(|node| {
            crate::window::node_is_game_like(st, node)
                || st.fullscreen_monitor_for_node(node).is_some()
        })
}

fn active_pointer_constraint_for_focus(
    pointer: &PointerHandle<Halley>,
    focus: &(WlSurface, Point<f64, Logical>),
) -> Option<ActivePointerConstraint> {
    let mut current = focus.0.clone();
    let mut current_origin = Some(focus.1);
    loop {
        let res = with_pointer_constraint(&current, pointer, |constraint| {
            let constraint = constraint.as_deref()?;
            if !constraint.is_active() {
                return None;
            }
            let origin = current_origin.unwrap_or_else(|| pointer.current_location());
            if let Some(region) = constraint.region() {
                let pos_within_surface = pointer.current_location() - origin;
                if !region.contains(pos_within_surface.to_i32_round()) {
                    return None;
                }
            }
            Some(ActivePointerConstraint {
                surface: current.clone(),
                origin,
                locked: matches!(constraint, PointerConstraint::Locked(_)),
                region: constraint.region().cloned(),
            })
        });
        if res.is_some() {
            return res;
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
    None
}

pub(crate) fn active_pointer_constraint(st: &Halley) -> Option<ActivePointerConstraint> {
    let pointer = st.platform.seat.get_pointer()?;
    if let Some(focus) = st.input.interaction_state.pointer_focus.as_ref()
        && let Some(constraint) = active_pointer_constraint_for_focus(&pointer, focus)
    {
        trace_pointer_constraint_active("tracked", &constraint);
        return Some(constraint);
    }

    let focus = pointer.current_focus()?;
    let mut current = focus;
    let mut current_origin = st
        .input
        .interaction_state
        .pointer_surface_origin
        .as_ref()
        .and_then(|(surface_id, x, y)| {
            (surface_id == &current.id()).then(|| Point::<f64, Logical>::from((*x, *y)))
        });
    loop {
        let res = with_pointer_constraint(&current, &pointer, |constraint| {
            let constraint = constraint.as_deref()?;
            if !constraint.is_active() {
                return None;
            }
            Some(ActivePointerConstraint {
                surface: current.clone(),
                origin: current_origin.unwrap_or_else(|| pointer.current_location()),
                locked: matches!(constraint, PointerConstraint::Locked(_)),
                region: constraint.region().cloned(),
            })
        });
        if res.is_some() {
            return res;
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
    None
}

/// Second-chance lock detection seeded from a *fresh* pointer focus (e.g. the
/// surface a hit-test just landed on) rather than the tracked/last focus that
/// [`active_pointer_constraint`] uses. The tracked focus can momentarily go stale
/// and miss an active lock; if that miss reaches the absolute-motion path it sends
/// `pointer.motion()` with a changed focus, which makes Smithay deactivate the
/// constraint and breaks XWayland-game mouselook (the "wiggle in place" bug).
/// Returns the active *locked* constraint covering `focus`, if any.
pub(crate) fn locked_constraint_for_focus(
    st: &Halley,
    focus: &(WlSurface, Point<f64, Logical>),
) -> Option<ActivePointerConstraint> {
    let pointer = st.platform.seat.get_pointer()?;
    active_pointer_constraint_for_focus(&pointer, focus).filter(|constraint| constraint.locked)
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
