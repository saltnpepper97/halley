use std::time::Instant;

use smithay::input::pointer::{MotionEvent, RelativeMotionEvent};
use smithay::utils::SERIAL_COUNTER;

use crate::compositor::interaction::{ModState, PointerState};
use crate::compositor::root::Halley;
use super::super::button::{active_pointer_binding, now_millis_u32};
use super::super::context::{
    clamp_screen_to_monitor, pointer_screen_context_for_monitor,
};
use super::super::focus::{grabbed_layer_surface_focus, pointer_focus_for_screen};
use halley_config::PointerBindingAction;

pub(crate) struct MotionRoutingContext {
    pub monitor: String,
    pub global_sx: f32,
    pub global_sy: f32,
    pub world: halley_core::field::Vec2,
    pub ws_w: i32,
    pub ws_h: i32,
    pub local_sx: f32,
    pub local_sy: f32,
}

pub(super) fn compute_motion_routing(
    st: &mut Halley,
    ps: &PointerState,
    mods: &ModState,
    raw_sx: f32,
    raw_sy: f32,
    sx: f32,
    sy: f32,
    allow_unbounded_screen: bool,
) -> MotionRoutingContext {
    let constrained_surface_info =
        crate::compositor::interaction::pointer::active_constrained_pointer_surface(st);
    let grabbed_layer_surface = st
        .platform
        .seat
        .get_pointer()
        .and_then(|pointer| pointer.is_grabbed().then_some(()))
        .and_then(|_| st.input.interaction_state.grabbed_layer_surface.clone())
        .filter(|surface| crate::compositor::monitor::layer_shell::is_layer_surface(st, surface));
    
    let drag_state = ps.drag.map(|drag| {
        let owner = st.monitor_for_node_or_current(drag.node_id);
        let allow_monitor_transfer =
            active_pointer_binding(st, mods, 0x110) == Some(PointerBindingAction::FieldJump);
        (drag, owner, allow_monitor_transfer)
    });

    let constrained_surface_monitor = constrained_surface_info
        .as_ref()
        .and_then(|(surface, _)| Some(st.monitor_for_surface_or_current(surface)));
    let grabbed_layer_surface_monitor = grabbed_layer_surface.as_ref().map(|surface| {
        crate::compositor::monitor::layer_shell::layer_surface_monitor_name(st, surface)
    });
    let locked_drag_monitor = drag_state
        .as_ref()
        .and_then(|(_, owner, allow_monitor_transfer)| {
            (!*allow_monitor_transfer).then(|| owner.clone())
        });
    let locked_resize_monitor = ps.resize
            .map(|resize| st.monitor_for_node_or_current(resize.node_id));
    let locked_pan_monitor = ps.panning.then(|| ps.pan_monitor.clone()).flatten();
    let locked_bloom_monitor = ps.bloom_drag.as_ref().map(|drag| drag.monitor.clone());
    let locked_overflow_monitor = ps.overflow_drag.as_ref().map(|drag| drag.monitor.clone());

    let (effective_sx, effective_sy) = if grabbed_layer_surface_monitor.is_some() {
        (raw_sx, raw_sy)
    } else if let Some(owner) = constrained_surface_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if let Some(owner) = locked_resize_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if let Some(owner) = locked_bloom_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if let Some(owner) = locked_overflow_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if let Some(owner) = locked_drag_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if let Some(owner) = locked_pan_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if allow_unbounded_screen {
        (raw_sx, raw_sy)
    } else {
        (sx, sy)
    };

    let grabbed_layer_surface_active = grabbed_layer_surface_monitor.is_some();
    let target_monitor = {
        if let Some(owner) = grabbed_layer_surface_monitor {
            owner
        } else if let Some(owner) = locked_overflow_monitor {
            owner
        } else if let Some(owner) = locked_bloom_monitor {
            owner
        } else if let Some(owner) = constrained_surface_monitor {
            owner
        } else if let Some(owner) = locked_resize_monitor {
            owner
        } else if let Some((_, owner, allow_monitor_transfer)) = drag_state.as_ref() {
            if *allow_monitor_transfer {
                st.monitor_for_screen(effective_sx, effective_sy)
                    .unwrap_or_else(|| owner.clone())
            } else {
                owner.clone()
            }
        } else if let Some(owner) = locked_pan_monitor {
            owner
        } else {
            st.monitor_for_screen_or_interaction(effective_sx, effective_sy)
        }
    };

    let context = pointer_screen_context_for_monitor(
        st,
        target_monitor,
        (effective_sx, effective_sy),
        !grabbed_layer_surface_active,
        !grabbed_layer_surface_active,
    );

    MotionRoutingContext {
        monitor: context.monitor,
        global_sx: context.global_sx,
        global_sy: context.global_sy,
        world: context.world,
        ws_w: context.ws_w,
        ws_h: context.ws_h,
        local_sx: context.local_sx,
        local_sy: context.local_sy,
    }
}

pub(super) fn dispatch_pointer_motion(
    st: &mut Halley,
    ps: &PointerState,
    routing: &MotionRoutingContext,
    delta: (f64, f64),
    delta_unaccel: (f64, f64),
    time_usec: u64,
    now: Instant,
) -> (bool, bool) {
    let mut desktop_hover = false;
    let mut hover_focus_blocked = false;

    if let Some(pointer) = st.platform.seat.get_pointer() {
        let constrained_surface_info =
            crate::compositor::interaction::pointer::active_constrained_pointer_surface(st);
        let locked_surface = constrained_surface_info
            .as_ref()
            .and_then(|(s, locked)| if *locked { Some(s.clone()) } else { None });
        let grabbed_layer_surface = st
            .platform
            .seat
            .get_pointer()
            .and_then(|pointer| pointer.is_grabbed().then_some(()))
            .and_then(|_| st.input.interaction_state.grabbed_layer_surface.clone())
            .filter(|surface| crate::compositor::monitor::layer_shell::is_layer_surface(st, surface));

        let resize_preview = ps.resize;
        let focus = if let Some(surface) = grabbed_layer_surface.clone() {
            grabbed_layer_surface_focus(st, &surface)
        } else if let Some(surface) = locked_surface.clone() {
            Some((surface, pointer.current_location()))
        } else {
            pointer_focus_for_screen(
                st,
                routing.ws_w,
                routing.ws_h,
                routing.local_sx,
                routing.local_sy,
                now,
                resize_preview,
            )
        };
        desktop_hover = focus.is_none();
        hover_focus_blocked = focus.as_ref().is_some_and(|(surface, _)| {
            crate::compositor::monitor::layer_shell::is_layer_surface(st, surface)
                || crate::protocol::wayland::session_lock::is_session_lock_surface(st, surface)
        });

        if delta.0.abs() > f64::EPSILON || delta.1.abs() > f64::EPSILON {
            pointer.relative_motion(
                st,
                focus.clone(),
                &RelativeMotionEvent {
                    delta: delta.into(),
                    delta_unaccel: delta_unaccel.into(),
                    utime: time_usec,
                },
            );
        }

        if locked_surface.is_none() {
            let location = if focus.as_ref().is_some_and(|(surface, _)| {
                crate::compositor::monitor::layer_shell::is_layer_surface(st, surface)
                    || crate::protocol::wayland::session_lock::is_session_lock_surface(st, surface)
            }) {
                (routing.local_sx as f64, routing.local_sy as f64).into()
            } else {
                let cam_scale = st.camera_render_scale() as f64;
                (routing.local_sx as f64 / cam_scale, routing.local_sy as f64 / cam_scale).into()
            };
            pointer.motion(
                st,
                focus.clone(),
                &MotionEvent {
                    location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: now_millis_u32(),
                },
            );
            if let Some((surface, _)) = focus.as_ref() {
                crate::compositor::interaction::pointer::activate_pointer_constraint_for_surface(
                    st, surface,
                );
            }
        }
        pointer.frame(st);
    }

    (desktop_hover, hover_focus_blocked)
}
