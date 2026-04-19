use std::time::{Duration, Instant};

use eventline::debug;
use halley_config::PointerBindingAction;

use crate::backend::interface::BackendView;
use crate::compositor::interaction::state::PendingCoreClick;
use crate::compositor::interaction::{BloomDragCtx, OverflowDragCtx};
use crate::compositor::root::Halley;
use crate::input::ctx::InputCtx;
use crate::overlay::{
    bloom_token_hit_test, cluster_overflow_icon_hit_test, cluster_overflow_strip_slot_at,
};
use crate::render::bearing_hit_test;
use crate::spatial::pick_hit_node_at;
use smithay::backend::input::ButtonState;
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::utils::SERIAL_COUNTER;

use super::focus::{
    grabbed_layer_surface_focus, layer_surface_focus_for_screen, pointer_focus_for_screen,
};
use super::screenshot::handle_screenshot_pointer_button;
use crate::input::keyboard::bindings::{
    apply_bound_pointer_input, apply_compositor_action_press, compositor_binding_action_active,
};

mod frame;
mod press;
mod release;

#[cfg(test)]
mod tests;

pub(crate) use frame::{
    ButtonFrame, active_pointer_binding, button_frame_for_monitor, now_millis_u32,
};
#[cfg(test)]
pub(crate) use press::{handle_core_left_press, handle_workspace_left_press};
pub(crate) use release::handle_button_release;

use press::{
    begin_bloom_pull_preview, handle_left_press, handle_move_binding_press,
    handle_resize_binding_press, handle_right_press,
};

pub(crate) fn handle_pointer_button_input<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    button_code: u32,
    button_state: ButtonState,
) {
    if exit_confirm_controller(&*st).active() {
        return;
    }
    if crate::compositor::interaction::state::note_cursor_activity(st, st.now_ms(Instant::now())) {
        ctx.backend.request_redraw();
    }

    let left = button_code == 0x110;
    let right = button_code == 0x111;
    let mut ps = ctx.pointer_state.borrow_mut();
    if left {
        ps.left_button_down = matches!(button_state, ButtonState::Pressed);
    }
    let (ws_w, ws_h) = ctx.backend.window_size_i32();
    let (frame, target_monitor, clamped_screen) =
        button_frame_for_monitor(st, ws_w, ws_h, ps.screen);
    let (sx, sy) = clamped_screen;
    let (local_w, local_h, local_sx, local_sy) = (frame.ws_w, frame.ws_h, frame.sx, frame.sy);
    ps.screen = (sx, sy);
    ps.workspace_size = (local_w, local_h);
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        let resize = ps.resize;
        ps.world = frame.world_now;
        drop(ps);
        dispatch_pointer_button(st, frame, resize, button_code, button_state);
        return;
    }
    let layer_focus = layer_surface_focus_for_screen(
        st,
        local_w,
        local_h,
        local_sx,
        local_sy,
        Instant::now(),
        ps.resize,
    );
    if matches!(button_state, ButtonState::Pressed)
        && let Some((surface, _)) = layer_focus.as_ref()
    {
        st.input.interaction_state.grabbed_layer_surface = Some(surface.clone());
        let monitor =
            crate::compositor::monitor::layer_shell::layer_surface_monitor_name(st, surface);
        st.model.spawn_state.pending_spawn_monitor = Some(monitor.clone());
        debug!(
            "pending spawn monitor latched from layer press: {}",
            monitor
        );
    }
    let world_now = frame.world_now;
    let mods = ctx.mod_state.borrow().clone();
    let cluster_pointer_action = match button_state {
        ButtonState::Pressed => active_pointer_binding(st, &mods, button_code),
        ButtonState::Released => ps.intercepted_buttons.get(&button_code).copied(),
    };
    let cluster_pointer_passthrough = matches!(
        cluster_pointer_action,
        Some(
            PointerBindingAction::MoveWindow
                | PointerBindingAction::FieldJump
                | PointerBindingAction::ResizeWindow
        )
    ) || ps.drag.is_some()
        || ps.resize.is_some();
    let prompt_monitor = st.model.monitor_state.current_monitor.clone();
    if handle_screenshot_pointer_button(
        st,
        ctx,
        &mut ps,
        button_code,
        button_state,
        left,
        frame,
        target_monitor.as_str(),
        local_w,
        local_h,
        local_sx,
        local_sy,
        world_now,
    ) {
        return;
    }
    if crate::compositor::clusters::system::cluster_system_controller(&*st)
        .cluster_name_prompt_active_for_monitor(prompt_monitor.as_str())
        && !cluster_pointer_passthrough
    {
        ps.world = world_now;
        match button_state {
            ButtonState::Pressed if left => {
                if target_monitor == prompt_monitor
                    && let Some(hit) = crate::overlay::cluster_naming_dialog_hit_test(
                        st, local_w, local_h, local_sx, local_sy,
                    )
                {
                    match hit {
                        crate::overlay::ClusterNamingDialogHit::ConfirmButton => {
                            let _ =
                                crate::compositor::clusters::system::cluster_system_controller(st)
                                    .confirm_cluster_name_prompt_for_monitor(
                                        prompt_monitor.as_str(),
                                        Instant::now(),
                                    );
                        }
                        crate::overlay::ClusterNamingDialogHit::InputCaret(caret_char) => {
                            let _ =
                                crate::compositor::clusters::system::cluster_system_controller(st)
                                    .begin_cluster_name_prompt_drag_for_monitor(
                                        prompt_monitor.as_str(),
                                        caret_char,
                                    );
                        }
                    }
                    ctx.backend.request_redraw();
                }
                return;
            }
            ButtonState::Released if left || right => {
                let _ = crate::compositor::clusters::system::cluster_system_controller(&mut *st)
                    .end_cluster_name_prompt_drag_for_monitor(prompt_monitor.as_str());
                handle_button_release(st, &mut ps, ctx.backend, button_code, None, world_now);
                return;
            }
            ButtonState::Pressed | ButtonState::Released => {
                return;
            }
        }
    }
    if matches!(button_state, ButtonState::Released) {
        st.input.interaction_state.grabbed_layer_surface = None;
    }
    if st.cluster_mode_active() && !cluster_pointer_passthrough {
        ps.world = world_now;
        match button_state {
            ButtonState::Pressed if left => {
                if let Some((surface, _)) = layer_focus.as_ref() {
                    let _ =
                        crate::compositor::monitor::layer_shell::focus_layer_surface(st, surface);
                    return;
                }
                let hit = pick_hit_node_at(
                    st,
                    local_w,
                    local_h,
                    local_sx,
                    local_sy,
                    Instant::now(),
                    ps.resize,
                );
                if let Some(hit) = hit {
                    let _ = st.toggle_cluster_mode_selection(hit.node_id);
                    ctx.backend.request_redraw();
                    return;
                }

                let now = Instant::now();
                let monitor = st.monitor_for_screen_or_current(frame.global_sx, frame.global_sy);
                st.focus_monitor_view(monitor.as_str(), now);
                if !crate::compositor::monitor::camera::camera_controller(&*st)
                    .pan_blocked_on_monitor(monitor.as_str())
                {
                    ps.panning = true;
                    ps.pan_monitor = Some(monitor);
                    ps.pan_last_screen = (frame.global_sx, frame.global_sy);
                }
                ctx.backend.request_redraw();
                return;
            }
            ButtonState::Released if left || right => {
                handle_button_release(st, &mut ps, ctx.backend, button_code, None, world_now);
                return;
            }
            ButtonState::Pressed | ButtonState::Released => {
                return;
            }
        }
    }
    if matches!(button_state, ButtonState::Pressed)
        && left
        && !frame.workspace_active
        && let Some(layout) = bloom_token_hit_test(
            st,
            local_w,
            local_h,
            target_monitor.as_str(),
            local_sx,
            local_sy,
        )
    {
        ps.bloom_drag = Some(BloomDragCtx {
            cluster_id: layout.cluster_id,
            member_id: layout.member_id,
            monitor: target_monitor.clone(),
            slot_screen: (layout.center_sx as f32, layout.center_sy as f32),
        });
        ps.hover_node = None;
        ps.hover_started_at = None;
        ps.preview_block_until = Some(Instant::now() + Duration::from_millis(500));
        st.input.interaction_state.overlay_hover_target = None;
        begin_bloom_pull_preview(
            st,
            layout.cluster_id,
            layout.member_id,
            layout.core_sx,
            layout.core_sy,
            layout.center_sx,
            layout.center_sy,
            target_monitor.as_str(),
        );
        st.request_maintenance();
        ctx.backend.request_redraw();
        return;
    }
    if matches!(button_state, ButtonState::Pressed)
        && left
        && !frame.workspace_active
        && let Some(node_id) = bearing_hit_test(
            st,
            local_w,
            local_h,
            target_monitor.as_str(),
            local_sx,
            local_sy,
        )
    {
        let now = Instant::now();
        let _ = crate::compositor::actions::window::focus_or_reveal_surface_node(st, node_id, now);
        ps.panning = false;
        ctx.backend.request_redraw();
        return;
    }
    if matches!(button_state, ButtonState::Pressed)
        && left
        && frame.workspace_active
        && let Some(cid) = st.active_cluster_workspace_for_monitor(target_monitor.as_str())
        && let Some(hit) = cluster_overflow_icon_hit_test(
            &crate::overlay::OverlayView::from_halley(st),
            target_monitor.as_str(),
            local_sx,
            local_sy,
            st.now_ms(Instant::now()),
        )
    {
        ps.overflow_drag = Some(OverflowDragCtx {
            cluster_id: cid,
            member_id: hit.member_id,
            monitor: target_monitor.clone(),
        });
        st.input.interaction_state.cluster_overflow_drag_preview = Some(
            crate::compositor::interaction::state::ClusterOverflowDragPreview {
                member_id: hit.member_id,
                monitor: target_monitor.clone(),
                screen_local: (local_sx, local_sy),
            },
        );
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(smithay::input::pointer::CursorIcon::Grabbing),
        );
        ctx.backend.request_redraw();
        return;
    }
    let intercepted_binding = match button_state {
        ButtonState::Pressed => {
            if let Some(action) = compositor_binding_action_active(st, button_code, &mods) {
                ps.intercepted_binding_buttons.insert(button_code);
                ps.panning = false;
                let _ =
                    apply_compositor_action_press(st, action, ctx.config_path, ctx.wayland_display);
                ctx.backend.request_redraw();
                true
            } else if apply_bound_pointer_input(
                st,
                button_code,
                &mods,
                ctx.config_path,
                ctx.wayland_display,
            ) {
                ps.intercepted_binding_buttons.insert(button_code);
                ps.panning = false;
                ctx.backend.request_redraw();
                true
            } else {
                false
            }
        }
        ButtonState::Released => ps.intercepted_binding_buttons.remove(&button_code),
    };
    let matched_action = match button_state {
        ButtonState::Pressed => active_pointer_binding(st, &mods, button_code),
        ButtonState::Released => ps.intercepted_buttons.remove(&button_code),
    };
    let intercepted = intercepted_binding || matched_action.is_some();
    if matches!(button_state, ButtonState::Pressed)
        && let Some(action) = matched_action
    {
        ps.intercepted_buttons.insert(button_code, action);
    }
    if !intercepted {
        dispatch_pointer_button(st, frame, ps.resize, button_code, button_state);
    }
    ps.world = world_now;
    if matches!(button_state, ButtonState::Pressed)
        && !intercepted
        && let Some((surface, _)) = layer_focus
    {
        let _ = crate::compositor::monitor::layer_shell::focus_layer_surface(st, &surface);
        return;
    }
    match button_state {
        ButtonState::Pressed => {
            if intercepted_binding {
                return;
            }
            let hit = pick_hit_node_at(
                st,
                local_w,
                local_h,
                local_sx,
                local_sy,
                Instant::now(),
                ps.resize,
            );
            if left {
                handle_left_press(
                    st,
                    &mut ps,
                    ctx.backend,
                    matches!(
                        matched_action,
                        Some(PointerBindingAction::MoveWindow | PointerBindingAction::FieldJump)
                    ),
                    matches!(matched_action, Some(PointerBindingAction::FieldJump)),
                    hit,
                    frame,
                );
            } else if right {
                handle_right_press(
                    st,
                    &mut ps,
                    ctx.backend,
                    matches!(matched_action, Some(PointerBindingAction::ResizeWindow)),
                    hit,
                    frame,
                );
            } else {
                match matched_action {
                    Some(PointerBindingAction::MoveWindow) => {
                        handle_move_binding_press(st, &mut ps, ctx.backend, hit, frame, false);
                    }
                    Some(PointerBindingAction::FieldJump) => {
                        handle_move_binding_press(st, &mut ps, ctx.backend, hit, frame, true);
                    }
                    Some(PointerBindingAction::ResizeWindow) => {
                        handle_resize_binding_press(st, &mut ps, ctx.backend, hit, frame);
                    }
                    None => {}
                }
            }
        }
        ButtonState::Released => {
            if left {
                st.input.interaction_state.pending_move_press = None;
            }
            if left && ps.drag.is_some() {
                handle_button_release(
                    st,
                    &mut ps,
                    ctx.backend,
                    button_code,
                    matched_action,
                    world_now,
                );
                return;
            }
            if left
                && let Some(pending_press) = st.input.interaction_state.pending_core_press.take()
            {
                let now = Instant::now();
                st.input.interaction_state.pending_core_click = Some(PendingCoreClick {
                    node_id: pending_press.node_id,
                    monitor: pending_press.monitor,
                    deadline_ms: st.now_ms(now).saturating_add(180),
                });
                st.request_maintenance();
                ctx.backend.request_redraw();
                return;
            }
            if left && ps.bloom_drag.take().is_some() {
                let now_ms = st.now_ms(Instant::now());
                let phase = st
                    .input
                    .interaction_state
                    .bloom_pull_preview
                    .as_ref()
                    .map(|preview| preview.phase.clone());
                match phase {
                    Some(crate::compositor::interaction::state::BloomPullPhase::Pressed) => {
                        let should_snap = st
                            .input
                            .interaction_state
                            .bloom_pull_preview
                            .as_ref()
                            .is_some_and(|preview| {
                                preview.display_offset.x.hypot(preview.display_offset.y) > 0.5
                            });
                        if should_snap {
                            if let Some(preview) =
                                st.input.interaction_state.bloom_pull_preview.as_mut()
                            {
                                preview.phase = crate::compositor::interaction::state::BloomPullPhase::Snapback {
                                    started_at_ms: now_ms,
                                    from_offset: preview.display_offset,
                                };
                                preview.hold_progress = 0.0;
                                st.request_maintenance();
                            }
                        } else {
                            st.input.interaction_state.bloom_pull_preview = None;
                        }
                    }
                    Some(crate::compositor::interaction::state::BloomPullPhase::Tethered {
                        ..
                    }) => {
                        if let Some(preview) =
                            st.input.interaction_state.bloom_pull_preview.as_mut()
                        {
                            preview.phase =
                                crate::compositor::interaction::state::BloomPullPhase::Snapback {
                                    started_at_ms: now_ms,
                                    from_offset: preview.display_offset,
                                };
                            preview.hold_progress = 0.0;
                            st.request_maintenance();
                        }
                    }
                    Some(crate::compositor::interaction::state::BloomPullPhase::Snapback {
                        ..
                    })
                    | None => {}
                }
                ps.preview_block_until = Some(Instant::now() + Duration::from_millis(500));
                st.input.interaction_state.overlay_hover_target = None;
                ps.hover_node = None;
                ps.hover_started_at = None;
                ctx.backend.request_redraw();
                return;
            }
            if left && let Some(overflow_drag) = ps.overflow_drag.take() {
                let now = Instant::now();
                st.input.interaction_state.cluster_overflow_drag_preview = None;
                crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
                let now_ms = st.now_ms(now);
                let strip_slot = if overflow_drag.monitor == target_monitor {
                    cluster_overflow_strip_slot_at(
                        &crate::overlay::OverlayView::from_halley(st),
                        target_monitor.as_str(),
                        local_sx,
                        local_sy,
                        now_ms,
                    )
                } else {
                    None
                };
                if let Some(target_slot) = strip_slot {
                    let reordered = st.reorder_cluster_overflow_member(
                        overflow_drag.monitor.as_str(),
                        overflow_drag.cluster_id,
                        overflow_drag.member_id,
                        target_slot,
                        now_ms,
                    );
                    if reordered {
                        ctx.backend.request_redraw();
                    } else {
                        st.reveal_cluster_overflow_for_monitor(
                            overflow_drag.monitor.as_str(),
                            now_ms,
                        );
                        ctx.backend.request_redraw();
                    }
                    return;
                }
                let release_hit =
                    pick_hit_node_at(st, local_w, local_h, local_sx, local_sy, now, ps.resize);
                if overflow_drag.monitor == target_monitor
                    && let Some(hit) = release_hit
                    && let Some(cluster) = st.model.field.cluster(overflow_drag.cluster_id)
                    && cluster
                        .visible_members(st.runtime.tuning.tile_max_stack)
                        .contains(&hit.node_id)
                {
                    let swapped = st.swap_cluster_overflow_member_with_visible(
                        overflow_drag.monitor.as_str(),
                        overflow_drag.cluster_id,
                        overflow_drag.member_id,
                        hit.node_id,
                        st.now_ms(now),
                    );
                    if swapped {
                        ctx.backend.request_redraw();
                    }
                    return;
                }
                st.reveal_cluster_overflow_for_monitor(overflow_drag.monitor.as_str(), now_ms);
                ctx.backend.request_redraw();
                return;
            }
            if intercepted_binding {
                return;
            }
            handle_button_release(
                st,
                &mut ps,
                ctx.backend,
                button_code,
                matched_action,
                world_now,
            );
        }
    }
}

pub(super) fn dispatch_pointer_button(
    st: &mut Halley,
    frame: ButtonFrame,
    resize_preview: Option<crate::compositor::interaction::ResizeCtx>,
    button_code: u32,
    button_state: smithay::backend::input::ButtonState,
) {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    let grabbed_layer_surface = st
        .input
        .interaction_state
        .grabbed_layer_surface
        .clone()
        .filter(|surface| crate::compositor::monitor::layer_shell::is_layer_surface(st, surface));
    let focus = if let Some(surface) = grabbed_layer_surface {
        grabbed_layer_surface_focus(st, &surface)
    } else {
        pointer_focus_for_screen(
            st,
            frame.ws_w,
            frame.ws_h,
            frame.sx,
            frame.sy,
            std::time::Instant::now(),
            resize_preview,
        )
    };
    let motion_serial = SERIAL_COUNTER.next_serial();
    let button_serial = SERIAL_COUNTER.next_serial();
    crate::protocol::wayland::activation::note_input_serial(
        st,
        button_serial,
        st.now_ms(Instant::now()),
    );
    let location = if focus.as_ref().is_some_and(|(surface, _)| {
        crate::compositor::monitor::layer_shell::is_layer_surface(st, surface)
            || crate::protocol::wayland::session_lock::is_session_lock_surface(st, surface)
    }) {
        (frame.sx as f64, frame.sy as f64).into()
    } else {
        let cam_scale = st.camera_render_scale() as f64;
        (frame.sx as f64 / cam_scale, frame.sy as f64 / cam_scale).into()
    };
    pointer.motion(
        st,
        focus,
        &MotionEvent {
            location,
            serial: motion_serial,
            time: now_millis_u32(),
        },
    );
    pointer.button(
        st,
        &ButtonEvent {
            serial: button_serial,
            time: now_millis_u32(),
            button: button_code,
            state: button_state,
        },
    );
    pointer.frame(st);
}

use crate::compositor::exit_confirm::exit_confirm_controller;
