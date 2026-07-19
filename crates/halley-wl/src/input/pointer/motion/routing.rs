use std::time::Instant;

use smithay::input::pointer::{MotionEvent, RelativeMotionEvent};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use super::super::button::active_pointer_binding;
use super::super::context::{clamp_screen_to_monitor, pointer_screen_context_for_monitor};
use super::super::focus::{
    grabbed_layer_surface_focus, pointer_focus_for_screen, seat_focus_from_local,
};
use crate::compositor::interaction::{ModState, PointerState};
use crate::compositor::root::Halley;
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

fn pointer_trace_enabled() -> bool {
    std::env::var_os("HALLEY_POINTER_TRACE").is_some_and(|value| value != "0")
}

fn pointer_trace_verbose_enabled() -> bool {
    pointer_trace_enabled()
        && std::env::var_os("HALLEY_POINTER_TRACE_VERBOSE").is_some_and(|value| value != "0")
}

pub(super) enum MotionDispatchResult {
    ConsumedByPointerConstraint,
    Forwarded {
        desktop_hover: bool,
        hover_focus_blocked: bool,
    },
}

pub(super) fn dispatch_locked_pointer_motion(
    st: &mut Halley,
    delta: (f64, f64),
    delta_unaccel: (f64, f64),
    time_usec: u64,
) -> bool {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return false;
    };
    let Some(constraint) = crate::compositor::interaction::pointer::active_pointer_constraint(st)
        .filter(|constraint| constraint.locked)
    else {
        return false;
    };
    let surface = constraint.surface;
    let origin = constraint.origin;

    if pointer_trace_verbose_enabled() {
        // Smithay delivers relative_motion + frame to its current pointer focus,
        // and only to relative-pointer objects of that surface's client. If
        // current_focus is None or a different client than the constraint
        // surface, the game will not see motion.
        let current_focus = pointer.current_focus();
        let same_client = current_focus
            .as_ref()
            .is_some_and(|focus| focus.id().same_client_as(&surface.id()));
        eventline::info!(
            "pointer_constraint locked_relative surface={:?} current_focus={:?} same_client={} delta={:.3},{:.3} unaccel={:.3},{:.3}",
            surface.id(),
            current_focus.as_ref().map(|focus| focus.id()),
            same_client,
            delta.0,
            delta.1,
            delta_unaccel.0,
            delta_unaccel.1,
        );
    }

    if delta.0.abs() > f64::EPSILON || delta.1.abs() > f64::EPSILON {
        pointer.relative_motion(
            st,
            Some((surface, origin)),
            &RelativeMotionEvent {
                delta: delta.into(),
                delta_unaccel: delta_unaccel.into(),
                utime: time_usec,
            },
        );
    }
    pointer.frame(st);
    true
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
        .filter(|surface| {
            crate::compositor::monitor::layer_shell::is_layer_surface_tree(st, surface)
        });

    let drag_state = ps.drag.map(|drag| {
        let owner = st.monitor_for_node_or_current(drag.node_id);
        let allow_monitor_transfer = if drag.requires_drag_modifier {
            match active_pointer_binding(st, mods, 0x110) {
                Some(PointerBindingAction::PanField) => false,
                Some(PointerBindingAction::MoveWindow) => true,
                _ => drag.allow_monitor_transfer,
            }
        } else {
            drag.allow_monitor_transfer
        };
        (drag, owner, allow_monitor_transfer)
    });

    let constrained_surface_monitor = constrained_surface_info
        .as_ref()
        .map(|(surface, _)| st.monitor_for_constrained_surface_or_current(surface));
    let grabbed_layer_surface_monitor = grabbed_layer_surface.as_ref().map(|surface| {
        crate::compositor::monitor::layer_shell::layer_surface_monitor_name(st, surface)
    });
    let locked_drag_monitor = drag_state
        .as_ref()
        .and_then(|(_, owner, allow_monitor_transfer)| {
            (!*allow_monitor_transfer).then(|| owner.clone())
        });
    let locked_resize_monitor = ps
        .resize
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
    } else if allow_unbounded_screen && drag_state.is_none() {
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

    let drag_allows_monitor_transfer = drag_state
        .as_ref()
        .is_some_and(|(_, _, allow_monitor_transfer)| *allow_monitor_transfer);
    let context = pointer_screen_context_for_monitor(
        st,
        target_monitor,
        (effective_sx, effective_sy),
        !grabbed_layer_surface_active,
        !grabbed_layer_surface_active && !drag_allows_monitor_transfer,
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
    time_msec: u32,
    now: Instant,
) -> MotionDispatchResult {
    let mut desktop_hover = false;
    let mut hover_focus_blocked = false;

    if let Some(pointer) = st.platform.seat.get_pointer() {
        if dispatch_locked_pointer_motion(st, delta, delta_unaccel, time_usec) {
            return MotionDispatchResult::ConsumedByPointerConstraint;
        }

        let active_constraint =
            crate::compositor::interaction::pointer::active_pointer_constraint(st);

        let locked_surface = active_constraint
            .as_ref()
            .and_then(|constraint| constraint.locked.then(|| constraint.surface.clone()));
        let grabbed_layer_surface = st
            .platform
            .seat
            .get_pointer()
            .and_then(|pointer| pointer.is_grabbed().then_some(()))
            .and_then(|_| st.input.interaction_state.grabbed_layer_surface.clone())
            .filter(|surface| {
                crate::compositor::monitor::layer_shell::is_layer_surface_tree(st, surface)
            });

        let resize_preview = ps.resize;
        let mut focus = if let Some(surface) = grabbed_layer_surface.clone() {
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

        if locked_surface.is_none()
            && let Some(current_focus) = focus.as_ref().cloned()
            && let Some(constrained_focus) =
                crate::compositor::interaction::pointer::constrained_focus_in_hierarchy(
                    st,
                    &current_focus,
                )
            && constrained_focus.0 != current_focus.0
        {
            focus = Some(constrained_focus);
        }

        // Second-chance lock guard. `active_constraint` above is seeded from the
        // tracked/last pointer focus, which can momentarily go stale and miss an
        // active lock. Re-check against the surface the fresh hit-test landed on:
        // if it holds an active *locked* constraint, send relative-only and bail
        // before any `pointer.motion()`. Emitting absolute motion here would change
        // the focused surface, making Smithay deactivate the lock and killing
        // XWayland-game mouselook (camera stuck, crosshair only wiggles in place).
        if let Some(focus_tuple) = focus.as_ref()
            && let Some(constraint) =
                crate::compositor::interaction::pointer::locked_constraint_for_focus(
                    st,
                    focus_tuple,
                )
        {
            if pointer_trace_verbose_enabled() {
                eventline::info!(
                    "pointer_constraint locked_relative_fallback surface={:?} hit_focus={:?} delta={:.3},{:.3} unaccel={:.3},{:.3}",
                    constraint.surface.id(),
                    focus_tuple.0.id(),
                    delta.0,
                    delta.1,
                    delta_unaccel.0,
                    delta_unaccel.1,
                );
            }
            if delta.0.abs() > f64::EPSILON || delta.1.abs() > f64::EPSILON {
                pointer.relative_motion(
                    st,
                    Some((constraint.surface.clone(), constraint.origin)),
                    &RelativeMotionEvent {
                        delta: delta.into(),
                        delta_unaccel: delta_unaccel.into(),
                        utime: time_usec,
                    },
                );
            }
            pointer.frame(st);
            return MotionDispatchResult::ConsumedByPointerConstraint;
        }

        crate::compositor::interaction::pointer::update_pointer_contents_from_focus(
            st,
            routing.monitor.clone(),
            focus.as_ref(),
        );

        desktop_hover = focus.is_none();
        hover_focus_blocked = focus.as_ref().is_some_and(|(surface, _)| {
            crate::protocol::wayland::session_lock::is_session_lock_surface(st, surface)
                || crate::compositor::monitor::layer_shell::layer_surface_blocks_desktop_hover(
                    st, surface,
                )
        });

        // A gamescope-managed window fills its output and expects a 1:1 input
        // mapping; bypass the spatial camera scale for it (config-gated). This is
        // a no-op when the camera is unscaled, so it cannot regress normal output.
        let bypass_spatial_camera = st.runtime.tuning.gaming.gamescope.bypass_spatial_camera
            && focus
                .as_ref()
                .is_some_and(|(surface, _)| crate::window::surface_is_gamescope(st, surface));

        let local_location = if locked_surface.is_some() {
            pointer.current_location()
        } else if bypass_spatial_camera
            || focus.as_ref().is_some_and(|(surface, _)| {
                crate::compositor::monitor::layer_shell::is_layer_surface_tree(st, surface)
                    || crate::protocol::wayland::session_lock::is_session_lock_surface(st, surface)
            })
        {
            (routing.local_sx as f64, routing.local_sy as f64).into()
        } else {
            let cam_scale = st.camera_render_scale() as f64;
            (
                routing.local_sx as f64 / cam_scale,
                routing.local_sy as f64 / cam_scale,
            )
                .into()
        };

        let seat_location =
            Point::<f64, Logical>::from((routing.global_sx as f64, routing.global_sy as f64));
        let (focus, location) = if locked_surface.is_some() {
            (focus, local_location)
        } else {
            (
                seat_focus_from_local(focus, local_location, seat_location),
                seat_location,
            )
        };

        if let Some(constraint) = active_constraint
            .as_ref()
            .filter(|constraint| !constraint.locked)
        {
            let mut prevent = focus
                .as_ref()
                .and_then(|(surface, _)| {
                    crate::compositor::interaction::pointer::find_constrained_surface_in_hierarchy(
                        st, surface,
                    )
                })
                .as_ref()
                != Some(&constraint.surface);

            if !prevent
                && let Some(region) = constraint.region.as_ref()
                && let Some((surface, origin)) = focus.as_ref()
                && *surface == constraint.surface
            {
                let pos_within_surface = location - *origin;
                prevent = !region.contains(pos_within_surface.to_i32_round());
            }

            if prevent {
                if delta.0.abs() > f64::EPSILON || delta.1.abs() > f64::EPSILON {
                    pointer.relative_motion(
                        st,
                        Some((constraint.surface.clone(), constraint.origin)),
                        &RelativeMotionEvent {
                            delta: delta.into(),
                            delta_unaccel: delta_unaccel.into(),
                            utime: time_usec,
                        },
                    );
                }
                pointer.frame(st);
                return MotionDispatchResult::ConsumedByPointerConstraint;
            }
        }

        let should_send_motion = match (locked_surface.as_ref(), pointer.current_focus()) {
            (Some(locked), Some(current)) => current != *locked,
            (Some(_), None) => true,
            (None, _) => true,
        };

        if should_send_motion {
            // Diagnostic: if we reach an absolute motion send while the focused
            // surface hierarchy still owns *any* pointer-constraint object, this is
            // the moment a lock could be torn down. Both the early-return above and
            // the second-chance guard should have caught a live lock; a line here
            // means detection missed (capture it once and for all).
            if pointer_trace_enabled()
                && let Some((surface, _)) = focus.as_ref()
                && let Some(constrained) =
                    crate::compositor::interaction::pointer::find_constrained_surface_in_hierarchy(
                        st, surface,
                    )
            {
                eventline::info!(
                    "pointer_constraint absolute_motion_over_constrained surface={:?} constrained={:?} loc={:.1},{:.1}",
                    surface.id(),
                    constrained.id(),
                    location.x,
                    location.y,
                );
            }
            pointer.motion(
                st,
                focus.clone(),
                &MotionEvent {
                    location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: time_msec,
                },
            );
        }
        if let Some((surface, surface_origin)) = focus.as_ref() {
            crate::compositor::interaction::pointer::activate_pointer_constraint_for_surface_at(
                st,
                surface,
                Some(*surface_origin),
            );
        }

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
        pointer.frame(st);
    }

    MotionDispatchResult::Forwarded {
        desktop_hover,
        hover_focus_blocked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::interaction::DragCtx;
    use halley_core::field::Vec2;
    use smithay::reexports::wayland_server::Display;
    use std::time::Instant;

    fn single_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        }];
        tuning
    }

    #[test]
    fn move_window_drag_clamps_to_outer_edge_pointer() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let node_id = st.model.field.spawn_surface(
            "dragged",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 200.0, y: 120.0 },
        );
        st.assign_node_to_monitor(node_id, "monitor_a");
        let mut ps = PointerState::default();
        ps.drag = Some(DragCtx {
            node_id,
            allow_monitor_transfer: true,
            requires_drag_modifier: true,
            edge_pan_eligible: false,
            current_offset: Vec2 { x: 0.0, y: 0.0 },
            center_latched: false,
            started_active: true,
            edge_pan_x: crate::compositor::interaction::DragAxisMode::Free,
            edge_pan_y: crate::compositor::interaction::DragAxisMode::Free,
            edge_pan_pressure: Vec2 { x: 0.0, y: 0.0 },
            last_pointer_world: Vec2 { x: 400.0, y: 300.0 },
            last_update_at: Instant::now(),
            release_velocity: Vec2 { x: 0.0, y: 0.0 },
        });

        let routing = compute_motion_routing(
            &mut st,
            &ps,
            &ModState::default(),
            960.0,
            300.0,
            799.0,
            300.0,
            true,
        );

        assert_eq!(routing.monitor, "monitor_a");
        assert_eq!(routing.global_sx, 799.0);
        assert_eq!(routing.local_sx, 799.0);
    }
}
