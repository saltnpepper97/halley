use std::time::Instant;

use smithay::input::pointer::{MotionEvent, RelativeMotionEvent};
use smithay::utils::SERIAL_COUNTER;

use crate::backend::interface::BackendView;
use crate::compositor::exit_confirm;
use crate::compositor::root::Halley;
use crate::input::ctx::InputCtx;
use crate::input::pointer::focus;
use crate::spatial::pick_hit_node_at;

pub(crate) mod bloom;
pub(crate) mod cluster;
pub(crate) mod drag;
pub(crate) mod pending;
pub(crate) mod routing;

#[cfg(test)]
pub(crate) mod tests;

/// Logical-pixel distance the pointer must travel out of a rest before
/// hover-focus is allowed to move keyboard focus again. Large enough to swallow
/// a desk bump or trackpad jitter, small enough that a genuine reach for the
/// mouse crosses it almost immediately.
const HOVER_FOCUS_REVEAL_GATE_PX: f64 = 64.0;

/// Idle gap after which the hover-focus gate re-arms. A pointer silent for
/// longer than this is treated as "at rest", so its next motion must prove it is
/// deliberate. Short enough to be invisible during continuous use.
const HOVER_FOCUS_IDLE_REARM_MS: u64 = 400;

pub(crate) use drag::{begin_drag, finish_pointer_drag, node_is_pointer_draggable};

use super::context::{clamp_screen_to_workspace, pointer_screen_context_for_monitor};
use super::focus::pointer_focus_for_screen;
use super::portal_chooser::handle_portal_chooser_pointer_motion;
use super::resize::handle_resize_motion;
use super::screenshot::handle_screenshot_pointer_motion;
use crate::input::keyboard::modkeys::modifier_active;

fn request_apogee_cursor_redraw<B: BackendView>(
    ctx: &InputCtx<'_, B>,
    previous_monitor: Option<&str>,
    target_monitor: &str,
) {
    if let Some(previous_monitor) = previous_monitor
        && previous_monitor != target_monitor
    {
        ctx.backend.request_output_redraw(previous_monitor);
    }
    ctx.backend.request_output_redraw(target_monitor);
}

#[inline]
fn event_time_msec(time_usec: u64) -> u32 {
    (time_usec / 1_000).min(u32::MAX as u64) as u32
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_pointer_motion_absolute<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    ws_w: i32,
    ws_h: i32,
    sx: f32,
    sy: f32,
    delta: (f64, f64),
    delta_unaccel: (f64, f64),
    time_usec: u64,
) {
    if exit_confirm::active(&*st) {
        return;
    }

    if st.input.interaction_state.apogee_session.is_some()
        && !crate::protocol::wayland::session_lock::session_lock_active(st)
    {
        let (sx, sy) = clamp_screen_to_workspace(ws_w, ws_h, sx, sy);
        // Real pointer motion over the overview reveals a keyboard-hidden cursor.
        // (The programmatic arrow-key warp also funnels through here, but the
        // keyboard path re-arms the hide immediately after the warp.)
        st.input.interaction_state.cursor_hidden_by_keyboard_nav = false;
        let now_ms = st.now_ms(Instant::now());
        st.input.interaction_state.last_cursor_activity_at_ms = now_ms;
        let previous_monitor = st
            .input
            .interaction_state
            .last_pointer_screen_global
            .map(|(last_sx, last_sy)| st.monitor_for_screen_or_interaction(last_sx, last_sy));
        let target_monitor = st.monitor_for_screen_or_interaction(sx, sy);
        let now = Instant::now();
        let (local_w, local_h, local_sx, local_sy) =
            st.local_screen_in_monitor(target_monitor.as_str(), sx, sy);
        {
            let mut ps = ctx.pointer_state.borrow_mut();
            ps.screen = (sx, sy);
            ps.workspace_size = (local_w, local_h);
            ps.hover_node = None;
            ps.hover_started_at = None;
        }
        st.input.interaction_state.last_pointer_screen_global = Some((sx, sy));
        st.input.interaction_state.overlay_hover_target = None;
        st.input.interaction_state.pending_core_hover = None;
        let hit = crate::compositor::overview::apogee_tile_at(
            st,
            target_monitor.as_str(),
            local_sx,
            local_sy,
            now,
        );
        let live_preview = crate::compositor::overview::apogee_window_tile_at(
            st,
            target_monitor.as_str(),
            local_sx,
            local_sy,
            now,
        );
        let live_preview_changed =
            st.input.interaction_state.apogee_live_preview_node != live_preview;
        if live_preview_changed {
            st.input.interaction_state.apogee_live_preview_node = live_preview;
            st.input.interaction_state.apogee_live_preview_last_at = None;
        }
        // Track the hovered tile (window or core) so the overview can ring core
        // tiles too, not just window previews.
        let hover_changed = st.input.interaction_state.apogee_hover_node != hit;
        if hover_changed {
            st.input.interaction_state.apogee_hover_node = hit;
        }
        let icon = if hit.is_some() {
            Some(smithay::input::pointer::CursorIcon::Pointer)
        } else {
            Some(smithay::input::pointer::CursorIcon::Default)
        };
        if st.input.interaction_state.cursor_override_icon != icon {
            crate::compositor::interaction::pointer::set_cursor_override_icon(st, icon);
        }
        request_apogee_cursor_redraw(ctx, previous_monitor.as_deref(), target_monitor.as_str());
        if live_preview_changed || hover_changed {
            ctx.backend.request_output_redraw(target_monitor.as_str());
        }
        return;
    }

    let now = Instant::now();
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        if crate::compositor::interaction::state::note_cursor_activity(
            st,
            st.now_ms(Instant::now()),
        ) {
            ctx.backend.request_redraw();
        }
        let raw_sx = sx;
        let raw_sy = sy;
        let (sx, sy) = clamp_screen_to_workspace(ws_w, ws_h, raw_sx, raw_sy);
        let target_monitor = st.monitor_for_screen_or_interaction(sx, sy);
        st.activate_monitor(target_monitor.as_str());
        let context = pointer_screen_context_for_monitor(st, target_monitor, (sx, sy), true, true);
        {
            let mut ps = ctx.pointer_state.borrow_mut();
            ps.screen = (context.global_sx, context.global_sy);
            ps.workspace_size = (context.ws_w, context.ws_h);
            ps.world = context.world;
        }
        st.input.interaction_state.last_pointer_screen_global =
            Some((context.global_sx, context.global_sy));
        crate::compositor::platform::refresh_cursor_surface_outputs(st);
        if let Some(pointer) = st.platform.seat.get_pointer() {
            let focus = pointer_focus_for_screen(
                st,
                context.ws_w,
                context.ws_h,
                context.local_sx,
                context.local_sy,
                now,
                None,
            );
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
            pointer.motion(
                st,
                focus,
                &MotionEvent {
                    location: (context.local_sx as f64, context.local_sy as f64).into(),
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event_time_msec(time_usec),
                },
            );
        }
        return;
    }

    // Locked pointer motion belongs to the constrained surface only. Do this
    // before Halley's monitor routing so a fullscreen Xwayland game cannot have
    // its active monitor/RandR primary churned while it is consuming relative
    // deltas.
    if routing::dispatch_locked_pointer_motion(st, delta, delta_unaccel, time_usec) {
        return;
    }

    let now_ms = st.now_ms(now);
    if crate::compositor::interaction::state::note_cursor_activity(st, now_ms) {
        ctx.backend.request_redraw();
    }

    // Arm the hover-focus gate after any lull in pointer movement. A pointer at
    // rest that suddenly reports motion is most likely a desk bump or trackpad
    // jitter, not a deliberate reach — so require real travel before it may move
    // keyboard focus off whatever you're typing into (e.g. the Lift launcher).
    // Continuous movement keeps the gate open, so ordinary mousing is untouched.
    {
        let idle = st.input.interaction_state.last_pointer_motion_at_ms;
        if idle == 0 || now_ms.saturating_sub(idle) >= HOVER_FOCUS_IDLE_REARM_MS {
            st.input.interaction_state.hover_focus_reveal_gate_px = HOVER_FOCUS_REVEAL_GATE_PX;
        }
        st.input.interaction_state.last_pointer_motion_at_ms = now_ms;
    }

    // Consume the gate as the pointer actually moves. Deliberate motion burns
    // through it within a frame or two; jitter from a tap never does.
    let deliberate_pointer_motion = {
        let remaining = &mut st.input.interaction_state.hover_focus_reveal_gate_px;
        if *remaining > 0.0 {
            let traveled = (delta.0 * delta.0 + delta.1 * delta.1).sqrt();
            *remaining = (*remaining - traveled).max(0.0);
            *remaining <= 0.0
        } else {
            true
        }
    };

    let allow_unbounded_screen = {
        let ps = ctx.pointer_state.borrow();
        ps.drag.is_some() || ps.resize.is_some() || ps.panning
    };

    let raw_sx = sx;
    let raw_sy = sy;
    let (sx, sy) = clamp_screen_to_workspace(ws_w, ws_h, raw_sx, raw_sy);

    let mods = ctx.mod_state.borrow().clone();
    let routing = {
        let ps = ctx.pointer_state.borrow();
        routing::compute_motion_routing(
            st,
            &ps,
            &mods,
            raw_sx,
            raw_sy,
            sx,
            sy,
            allow_unbounded_screen,
        )
    };

    st.activate_monitor(routing.monitor.as_str());
    // Keep the Xwayland RandR primary on the monitor under the cursor so XWayland
    // games (which read the X primary at startup) get the resolution of the monitor
    // they are launched on. Debounced internally; only acts on monitor changes.
    crate::compositor::monitor::state::sync_xwayland_primary(st, routing.monitor.as_str());

    let (_, hover_focus_blocked) = {
        let ps = ctx.pointer_state.borrow();
        match routing::dispatch_pointer_motion(
            st,
            &ps,
            &routing,
            delta,
            delta_unaccel,
            time_usec,
            event_time_msec(time_usec),
            now,
        ) {
            routing::MotionDispatchResult::ConsumedByPointerConstraint => return,
            routing::MotionDispatchResult::Forwarded {
                desktop_hover,
                hover_focus_blocked,
            } => (desktop_hover, hover_focus_blocked),
        }
    };

    // While a game holds the pointer (lock/confine), never reveal Halley's own
    // overlays over it (config-gated via `gamescope.suppress-overlays`).
    let suppress_game_overlays = st.runtime.tuning.gamescope.suppress_overlays
        && crate::compositor::interaction::pointer::pointer_holds_game_constraint(&*st);

    let p = routing.world;
    let drag_mod_ok = modifier_active(&mods, st.runtime.tuning.keybinds.modifier)
        || matches!(
            super::button::active_pointer_binding(st, &mods, 0x110),
            Some(
                halley_config::PointerBindingAction::MoveWindow
                    | halley_config::PointerBindingAction::PanField
            )
        );

    let error_toast_hovered = crate::overlay::error_toast_hit_test(
        st,
        routing.monitor.as_str(),
        routing.ws_w,
        routing.ws_h,
        routing.local_sx as f64,
        routing.local_sy as f64,
    );
    st.ui.render_state.set_overlay_error_toast_hovered(
        routing.monitor.as_str(),
        error_toast_hovered,
        st.now_ms(now),
    );
    let mut ps = ctx.pointer_state.borrow_mut();
    let previous_cursor_monitor = st.monitor_for_screen(ps.screen.0, ps.screen.1);
    ps.world = p;
    ps.screen = (routing.global_sx, routing.global_sy);
    ps.workspace_size = (routing.ws_w, routing.ws_h);
    st.input.interaction_state.last_pointer_screen_global =
        Some((routing.global_sx, routing.global_sy));
    // Real desktop pointer movement releases an explicit monitor-focus pin so
    // hover focus-mode resumes driving the spawn target.
    st.input.interaction_state.monitor_focus_pinned = false;
    crate::compositor::platform::refresh_cursor_surface_outputs(st);

    if handle_screenshot_pointer_motion(
        st,
        ctx,
        routing.monitor.as_str(),
        routing.ws_w,
        routing.ws_h,
        routing.local_sx,
        routing.local_sy,
        routing.global_sx,
        routing.global_sy,
        now,
    ) {
        return;
    }
    if handle_portal_chooser_pointer_motion(
        st,
        ctx,
        routing.monitor.as_str(),
        routing.ws_w,
        routing.ws_h,
        routing.local_sx,
        routing.local_sy,
    ) {
        return;
    }

    let prompt_monitor = crate::compositor::clusters::system::active_cluster_name_prompt_monitor(
        &*st,
        st.model.monitor_state.current_monitor.as_str(),
    );
    if let Some(prompt_monitor) = prompt_monitor {
        let prompt_hit = if routing.monitor == prompt_monitor {
            crate::overlay::cluster_naming_dialog_hit_test(
                st,
                routing.ws_w,
                routing.ws_h,
                routing.local_sx,
                routing.local_sy,
            )
        } else {
            None
        };
        st.input.interaction_state.overlay_hover_target = None;
        st.input.interaction_state.pending_core_hover = None;
        ps.hover_node = None;
        ps.hover_started_at = None;
        if let Some(crate::overlay::ClusterNamingDialogHit::InputCaret(caret_char)) = prompt_hit {
            let _ =
                crate::compositor::clusters::system::drag_cluster_name_prompt_selection_for_monitor(
                    &mut *st,
                    prompt_monitor.as_str(),
                    caret_char,
                );
        }
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            match prompt_hit {
                Some(crate::overlay::ClusterNamingDialogHit::ConfirmButton) => {
                    Some(smithay::input::pointer::CursorIcon::Pointer)
                }
                Some(crate::overlay::ClusterNamingDialogHit::InputCaret(_)) => {
                    Some(smithay::input::pointer::CursorIcon::Text)
                }
                None => None,
            },
        );
        ctx.backend.request_redraw();
        return;
    }

    pending::maybe_begin_core_drag_from_pending_press(
        st,
        &mut ps,
        ctx.backend,
        routing.ws_w,
        routing.ws_h,
        routing.global_sx,
        routing.global_sy,
        routing.local_sx,
        routing.local_sy,
        routing.world,
    );
    pending::maybe_begin_move_drag_from_pending_press(
        st,
        &mut ps,
        ctx.backend,
        routing.ws_w,
        routing.ws_h,
        routing.global_sx,
        routing.global_sy,
        routing.local_sx,
        routing.local_sy,
        routing.world,
    );

    if bloom::handle_bloom_pull_motion(
        st,
        &mut ps,
        ctx.backend,
        routing.global_sx,
        routing.global_sy,
        now,
    ) {
        return;
    }

    if st.has_active_cluster_workspace() {
        let monitor = st.model.monitor_state.current_monitor.clone();
        let now_ms = st.now_ms(now);
        if let Some(overflow_drag) = ps.overflow_drag.clone() {
            st.input.interaction_state.cluster_overflow_drag_preview = Some(
                crate::compositor::interaction::state::ClusterOverflowDragPreview {
                    member_id: overflow_drag.member_id,
                    monitor: monitor.clone(),
                    screen_local: (routing.local_sx, routing.local_sy),
                },
            );
            crate::compositor::interaction::pointer::set_cursor_override_icon(
                st,
                Some(smithay::input::pointer::CursorIcon::Grabbing),
            );
            st.reveal_cluster_overflow_for_monitor(monitor.as_str(), now_ms);
            st.input.interaction_state.overlay_hover_target = None;
            ps.hover_started_at = None;
            ps.hover_node = None;
            crate::compositor::carry::system::set_drag_authority_node(st, None);
            ps.drag = None;
            ps.resize = None;
            ctx.backend.request_redraw();
            return;
        }
        if ps.drag.is_none() && ps.resize.is_none() && !suppress_game_overlays {
            let queue_hover = crate::overlay::cluster_overflow_icon_hit_test(
                &crate::overlay::OverlayView::from_halley(st),
                monitor.as_str(),
                routing.local_sx,
                routing.local_sy,
                now_ms,
            );
            let mut over_reveal = false;
            if routing.local_sx >= routing.ws_w as f32 - Halley::CLUSTER_OVERFLOW_REVEAL_EDGE_PX {
                st.reveal_cluster_overflow_for_monitor(monitor.as_str(), now_ms);
                over_reveal = true;
            } else if let Some(rect) = st.cluster_overflow_rect_for_monitor(monitor.as_str()) {
                let inside = routing.local_sx >= rect.x
                    && routing.local_sx <= rect.x + rect.w
                    && routing.local_sy >= rect.y
                    && routing.local_sy <= rect.y + rect.h;
                if inside {
                    st.reveal_cluster_overflow_for_monitor(monitor.as_str(), now_ms);
                    over_reveal = true;
                }
            }

            if queue_hover.is_some() || over_reveal {
                crate::compositor::interaction::pointer::set_cursor_override_icon(
                    st,
                    queue_hover
                        .map(|_| smithay::input::pointer::CursorIcon::Pointer)
                        .or(None),
                );
                let next_hover = queue_hover.map(|hit| hit.member_id);
                if next_hover != ps.hover_node {
                    ps.hover_started_at = next_hover.map(|_| now);
                } else if next_hover.is_none() {
                    ps.hover_started_at = None;
                }
                ps.hover_node = next_hover;
                st.input.interaction_state.pending_core_hover = None;
                st.input.interaction_state.overlay_hover_target = next_hover.map(|node_id| {
                    crate::compositor::interaction::state::OverlayHoverTarget {
                        node_id,
                        monitor: monitor.clone(),
                        screen_anchor: (
                            routing.local_sx.round() as i32,
                            routing.local_sy.round() as i32,
                        ),
                        prefer_left: true,
                    }
                });
                crate::compositor::carry::system::set_drag_authority_node(st, None);
                ps.drag = None;
                ps.resize = None;
                ctx.backend.request_redraw();
                return;
            }
        }
    }
    st.input.interaction_state.cluster_overflow_drag_preview = None;
    st.input.interaction_state.overlay_hover_target = None;
    crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);

    let _ = drag::handle_drag_motion(
        st,
        ctx.backend,
        &mods,
        &mut ps,
        drag_mod_ok,
        routing.monitor.as_str(),
        routing.ws_w,
        routing.ws_h,
        routing.local_sx,
        routing.local_sy,
        routing.world,
        now,
    );

    if handle_resize_motion(
        st,
        &mut ps,
        routing.ws_w,
        routing.ws_h,
        routing.local_sx,
        routing.local_sy,
        ctx.backend,
    ) {
        return;
    }

    if ps.panning {
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(smithay::input::pointer::CursorIcon::Grabbing),
        );
        let (lsx, lsy) = ps.pan_last_screen;
        let dx_px = routing.global_sx - lsx;
        let dy_px = routing.global_sy - lsy;
        let camera = crate::compositor::monitor::camera::camera_view_size(&*st);
        let dx_world = dx_px * camera.x.max(1.0) / (routing.ws_w as f32).max(1.0);
        let dy_world = dy_px * camera.y.max(1.0) / (routing.ws_h as f32).max(1.0);
        let now = Instant::now();
        st.note_pan_activity(now);
        crate::compositor::monitor::camera::pan_camera_target(
            &mut *st,
            halley_core::field::Vec2 {
                x: -dx_world,
                y: -dy_world,
            },
        );
        st.note_pan_viewport_change(now);
        ps.pan_last_screen = (routing.global_sx, routing.global_sy);
        ctx.backend.request_output_redraw(routing.monitor.as_str());
    }

    let bloom_hover = if ps.drag.is_none() && ps.resize.is_none() && !ps.panning {
        crate::overlay::bloom_token_hit_test(
            st,
            routing.ws_w,
            routing.ws_h,
            routing.monitor.as_str(),
            routing.local_sx,
            routing.local_sy,
        )
        .map(|layout| {
            (
                layout.member_id,
                crate::compositor::interaction::state::OverlayHoverTarget {
                    node_id: layout.member_id,
                    monitor: routing.monitor.clone(),
                    screen_anchor: (layout.center_sx + layout.token_radius + 4, layout.center_sy),
                    prefer_left: false,
                },
            )
        })
    } else {
        None
    };

    let hover_hit =
        if bloom_hover.is_none() && ps.drag.is_none() && ps.resize.is_none() && !ps.panning {
            pick_hit_node_at(
                st,
                routing.ws_w,
                routing.ws_h,
                routing.local_sx,
                routing.local_sy,
                now,
                ps.resize,
            )
        } else {
            None
        };

    let edge_resize_cursor = hover_hit.and_then(|hit| {
        super::resize::edge_resize_handle_at(
            st,
            routing.ws_w,
            routing.ws_h,
            hit.node_id,
            routing.local_sx,
            routing.local_sy,
        )
    });

    let next_hover = if let Some((node_id, target)) = bloom_hover {
        st.input.interaction_state.overlay_hover_target = Some(target);
        Some(node_id)
    } else {
        hover_hit.and_then(|hit| {
            st.model.field.node(hit.node_id).and_then(|n| {
                matches!(
                    n.state,
                    halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
                )
                .then_some(hit.node_id)
            })
        })
    };

    // Only a deliberate move retargets keyboard focus. A cursor-reveal bump or
    // sub-threshold jitter reveals the pointer but leaves focus where it is
    // (e.g. on the Lift launcher you're typing into).
    if deliberate_pointer_motion {
        focus::apply_hover_focus_mode(st, hover_hit, hover_focus_blocked, now);
    }

    let hover_changed = next_hover != ps.hover_node;
    if hover_changed {
        ps.hover_started_at = next_hover.map(|_| now);
    } else if next_hover.is_none() {
        ps.hover_started_at = None;
    }
    ps.hover_node = next_hover;
    let previous_core_hover = st.input.interaction_state.pending_core_hover.clone();
    st.input.interaction_state.pending_core_hover = next_hover.and_then(|node_id| {
        st.model.field.node(node_id).and_then(|node| {
            (node.kind == halley_core::field::NodeKind::Core
                && node.state == halley_core::field::NodeState::Core)
                .then(|| {
                    let monitor = routing.monitor.clone();
                    let started_at_ms = previous_core_hover
                        .as_ref()
                        .filter(|pending| pending.node_id == node_id && pending.monitor == monitor)
                        .map(|pending| pending.started_at_ms)
                        .unwrap_or(st.now_ms(now));
                    crate::compositor::interaction::state::PendingCoreHover {
                        node_id,
                        monitor,
                        started_at_ms,
                    }
                })
        })
    });
    if st.input.interaction_state.pending_core_hover.is_some() {
        st.request_maintenance();
    }

    if ps.drag.is_none()
        && ps.resize.is_none()
        && !ps.panning
        && st.input.interaction_state.overlay_hover_target.is_none()
        && st.input.interaction_state.pending_core_hover.is_none()
    {
        crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
    }

    if let Some(handle) = edge_resize_cursor {
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(super::resize::cursor_icon_for_resize_handle(handle)),
        );
    }

    if !hover_changed
        && ps.drag.is_none()
        && ps.resize.is_none()
        && !ps.panning
        && st.input.interaction_state.pending_core_hover.is_none()
        && st.input.interaction_state.overlay_hover_target.is_none()
    {
        if let Some(previous_monitor) = previous_cursor_monitor.as_deref()
            && previous_monitor != routing.monitor.as_str()
        {
            ctx.backend.request_output_redraw(previous_monitor);
        }
        ctx.backend.request_output_redraw(routing.monitor.as_str());
    } else {
        ctx.backend.request_redraw();
    }
}
