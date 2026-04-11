use std::time::Instant;

use std::time::Duration;

use eventline::debug;
use halley_config::{KeyModifiers, PointerBindingAction};

use crate::backend::interface::BackendView;
use crate::compositor::actions::window::{
    activate_collapsed_node_from_click, focus_or_reveal_surface_node,
};
use crate::compositor::exit_confirm::exit_confirm_controller;
use crate::compositor::interaction::state::{
    PendingCoreClick, PendingCorePress, PendingTitlebarPress,
};
use crate::compositor::interaction::{
    BloomDragCtx, HitNode, ModState, OverflowDragCtx, PointerState,
};
use crate::compositor::root::Halley;
use crate::compositor::surface_ops::{node_allows_interactive_resize, stack_focus_target_for_node};
use crate::input::ctx::InputCtx;
use crate::input::keyboard::modkeys::modifier_active;
use crate::overlay::{
    bloom_token_hit_test, cluster_overflow_icon_hit_test, cluster_overflow_strip_slot_at,
};
use crate::render::bearing_hit_test;
use crate::spatial::pick_hit_node_at;
use crate::spatial::screen_to_world;
use smithay::backend::input::ButtonState;
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::SERIAL_COUNTER;

use super::focus::{
    grabbed_layer_surface_focus, layer_surface_focus_for_screen, pointer_focus_for_screen,
};
use super::motion::{begin_drag, finish_pointer_drag, node_is_pointer_draggable};
use super::resize::{begin_resize, finalize_resize};
use super::screenshot::handle_screenshot_pointer_button;
use crate::input::keyboard::bindings::{
    apply_bound_pointer_input, apply_compositor_action_press, compositor_binding_action_active,
};

fn begin_bloom_pull_preview(
    st: &mut Halley,
    cluster_id: halley_core::cluster::ClusterId,
    member_id: halley_core::field::NodeId,
    core_sx: i32,
    core_sy: i32,
    slot_sx: i32,
    slot_sy: i32,
    monitor: &str,
) {
    st.input.interaction_state.bloom_pull_preview =
        Some(crate::compositor::interaction::state::BloomPullPreview {
            cluster_id,
            member_id,
            monitor: monitor.to_string(),
            core_screen: halley_core::field::Vec2 {
                x: core_sx as f32,
                y: core_sy as f32,
            },
            slot_screen: halley_core::field::Vec2 {
                x: slot_sx as f32,
                y: slot_sy as f32,
            },
            pointer_screen: halley_core::field::Vec2 {
                x: slot_sx as f32,
                y: slot_sy as f32,
            },
            display_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            hold_progress: 0.0,
            phase: crate::compositor::interaction::state::BloomPullPhase::Pressed,
        });
}

fn handle_left_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    drag_binding_active: bool,
    allow_monitor_transfer: bool,
    hit: Option<HitNode>,
    frame: ButtonFrame,
) {
    let Some(hit) = hit else {
        let now = Instant::now();
        let monitor = st
            .monitor_for_screen(frame.global_sx, frame.global_sy)
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
        let _ = st.close_cluster_bloom_for_monitor(monitor.as_str());
        st.focus_monitor_view(monitor.as_str(), now);
        ps.panning = true;
        ps.pan_monitor = Some(monitor);
        ps.pan_last_screen = (frame.global_sx, frame.global_sy);
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(smithay::input::pointer::CursorIcon::Grabbing),
        );
        backend.request_redraw();
        return;
    };
    if frame.workspace_active {
        let drag_target_ok = node_is_pointer_draggable(st, hit.node_id);
        if drag_binding_active && drag_target_ok && !hit.is_core {
            begin_drag(
                st,
                ps,
                backend,
                hit,
                frame,
                frame.world_now,
                allow_monitor_transfer,
                drag_binding_active,
            );
            return;
        }
        if hit.on_titlebar && drag_target_ok && !hit.is_core {
            let now = Instant::now();
            let focus_target = stack_focus_target_for_node(st, hit.node_id).unwrap_or(hit.node_id);
            st.set_recent_top_node(focus_target, now + std::time::Duration::from_millis(1200));
            st.set_interaction_focus(Some(focus_target), 700, now);
            st.input.interaction_state.pending_titlebar_press = Some(PendingTitlebarPress {
                node_id: hit.node_id,
                press_global_sx: frame.global_sx,
                press_global_sy: frame.global_sy,
                workspace_active: true,
            });
            backend.request_redraw();
            return;
        }
        handle_workspace_left_press(st, ps, backend, hit);
        return;
    }

    if !drag_binding_active && hit.is_core {
        handle_core_left_press(st, ps, backend, hit, frame);
        return;
    }

    if !drag_binding_active && restore_fullscreen_click_focus(st, hit.node_id, Instant::now()) {
        backend.request_redraw();
    }

    let drag_target_ok = node_is_pointer_draggable(st, hit.node_id);
    if hit.on_titlebar && drag_target_ok && !hit.is_core {
        let now = Instant::now();
        let focus_target = stack_focus_target_for_node(st, hit.node_id).unwrap_or(hit.node_id);
        st.set_recent_top_node(focus_target, now + std::time::Duration::from_millis(1200));
        st.set_interaction_focus(Some(focus_target), 700, now);
        st.input.interaction_state.pending_titlebar_press = Some(PendingTitlebarPress {
            node_id: hit.node_id,
            press_global_sx: frame.global_sx,
            press_global_sy: frame.global_sy,
            workspace_active: false,
        });
        backend.request_redraw();
        return;
    }

    if !drag_binding_active
        && !hit.on_titlebar
        && st.model.field.node(hit.node_id).is_some_and(|n| {
            n.kind == halley_core::field::NodeKind::Surface
                && st.model.field.is_visible(hit.node_id)
        })
    {
        let focus_target = stack_focus_target_for_node(st, hit.node_id).unwrap_or(hit.node_id);
        st.set_recent_top_node(
            focus_target,
            Instant::now() + std::time::Duration::from_millis(1200),
        );
        st.set_interaction_focus(Some(focus_target), 30_000, Instant::now());
    }

    let mut handled_node_click = false;
    if !drag_binding_active && !hit.on_titlebar && !hit.is_core {
        let is_node = st
            .model
            .field
            .node(hit.node_id)
            .is_some_and(|n| n.state == halley_core::field::NodeState::Node);
        if is_node {
            let now = Instant::now();
            if activate_collapsed_node_from_click(st, hit.node_id, now) {
                backend.request_redraw();
            }
            handled_node_click = true;
        }
    }

    if drag_binding_active && drag_target_ok && !handled_node_click {
        if hit.is_core {
            let _ = collapse_bloom_for_core_if_open(st, hit.node_id);
        }
        begin_drag(
            st,
            ps,
            backend,
            hit,
            frame,
            frame.world_now,
            allow_monitor_transfer,
            drag_binding_active,
        );
        return;
    }

    if hit.is_core {
        let now = Instant::now();
        let focus_target = stack_focus_target_for_node(st, hit.node_id).unwrap_or(hit.node_id);
        st.set_recent_top_node(focus_target, now + std::time::Duration::from_millis(1200));
        st.set_interaction_focus(Some(focus_target), 700, now);
        backend.request_redraw();
    }
}

fn handle_right_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    resize_binding_active: bool,
    hit: Option<HitNode>,
    frame: ButtonFrame,
) {
    let Some(hit) = hit else {
        if frame.workspace_active {
            clear_pointer_activity(st, ps);
            return;
        }
        let now = Instant::now();
        let monitor = st
            .monitor_for_screen(frame.global_sx, frame.global_sy)
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
        st.focus_monitor_view(monitor.as_str(), now);
        ps.panning = true;
        ps.pan_monitor = Some(monitor);
        ps.pan_last_screen = (frame.global_sx, frame.global_sy);
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(smithay::input::pointer::CursorIcon::Grabbing),
        );
        backend.request_redraw();
        return;
    };
    let can_resize = node_allows_interactive_resize(st, hit.node_id);
    if resize_binding_active && can_resize {
        begin_resize(st, ps, backend, hit, frame);
    }
}

fn handle_move_binding_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: Option<HitNode>,
    frame: ButtonFrame,
    allow_monitor_transfer: bool,
) {
    let Some(hit) = hit else {
        if frame.workspace_active {
            clear_pointer_activity(st, ps);
            return;
        }
        let now = Instant::now();
        let monitor = st
            .monitor_for_screen(frame.global_sx, frame.global_sy)
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
        st.focus_monitor_view(monitor.as_str(), now);
        ps.panning = true;
        ps.pan_monitor = Some(monitor);
        ps.pan_last_screen = (frame.global_sx, frame.global_sy);
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(smithay::input::pointer::CursorIcon::Grabbing),
        );
        backend.request_redraw();
        return;
    };
    let drag_target_ok = node_is_pointer_draggable(st, hit.node_id);
    if drag_target_ok {
        if hit.is_core {
            let _ = collapse_bloom_for_core_if_open(st, hit.node_id);
        }
        begin_drag(
            st,
            ps,
            backend,
            hit,
            frame,
            frame.world_now,
            allow_monitor_transfer,
            true,
        );
    }
}

fn handle_resize_binding_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: Option<HitNode>,
    frame: ButtonFrame,
) {
    if frame.workspace_active {
        clear_pointer_activity(st, ps);
        return;
    }

    let Some(hit) = hit else {
        let now = Instant::now();
        let monitor = st
            .monitor_for_screen(frame.global_sx, frame.global_sy)
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
        st.focus_monitor_view(monitor.as_str(), now);
        ps.panning = true;
        ps.pan_monitor = Some(monitor);
        ps.pan_last_screen = (frame.global_sx, frame.global_sy);
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(smithay::input::pointer::CursorIcon::Grabbing),
        );
        backend.request_redraw();
        return;
    };
    let can_resize = node_allows_interactive_resize(st, hit.node_id);
    if can_resize {
        // Binding-triggered resize always starts Pending regardless of where
        // the cursor is — drag direction picks the handle.
        begin_resize(st, ps, backend, hit, frame);
    }
}

pub(super) fn handle_button_release(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    button_code: u32,
    action: Option<PointerBindingAction>,
    world_now: halley_core::field::Vec2,
) {
    match action {
        Some(PointerBindingAction::MoveWindow | PointerBindingAction::FieldJump) => {
            if let Some(d) = ps.drag {
                let now = Instant::now();
                finish_pointer_drag(st, ps, d.node_id, d.started_active, world_now, now);
                crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
            }
            st.input.interaction_state.active_drag = None;
            st.input.interaction_state.pending_titlebar_press = None;
            if ps.panning {
                crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
            }
            ps.panning = false;
            ps.pan_monitor = None;
            if ps.resize.is_some() {
                finalize_resize(st, ps, backend);
            }
        }
        Some(PointerBindingAction::ResizeWindow) => {
            st.input.interaction_state.pending_titlebar_press = None;
            finalize_resize(st, ps, backend);
        }
        None => {
            if button_code == 0x110
                && let Some(d) = ps.drag
            {
                let now = Instant::now();
                finish_pointer_drag(st, ps, d.node_id, d.started_active, world_now, now);
                st.input.interaction_state.active_drag = None;
                crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
            }
            if button_code == 0x110 || button_code == 0x111 {
                if button_code == 0x110 {
                    st.input.interaction_state.pending_titlebar_press = None;
                }
                if ps.panning {
                    crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
                }
                ps.panning = false;
                ps.pan_monitor = None;
            }
        }
    }
}

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
                let monitor = st
                    .monitor_for_screen(frame.global_sx, frame.global_sy)
                    .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
                st.focus_monitor_view(monitor.as_str(), now);
                ps.panning = true;
                ps.pan_monitor = Some(monitor);
                ps.pan_last_screen = (frame.global_sx, frame.global_sy);
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
        let _ = focus_or_reveal_surface_node(st, node_id, now);
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
                st.input.interaction_state.pending_titlebar_press = None;
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

pub(super) fn clear_pointer_activity(st: &mut Halley, ps: &mut PointerState) {
    if let Some(drag) = ps.drag {
        crate::compositor::carry::system::set_drag_authority_node(st, None);
        crate::compositor::carry::system::end_carry_state_tracking(st, drag.node_id);
    }
    crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
    st.input.interaction_state.active_drag = None;
    st.input.interaction_state.pending_core_press = None;
    st.input.interaction_state.pending_titlebar_press = None;
    st.input.interaction_state.cluster_overflow_drag_preview = None;
    crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
    ps.drag = None;
    ps.overflow_drag = None;
    ps.resize = None;
    ps.panning = false;
    ps.pan_monitor = None;
}

pub(super) fn collapse_bloom_for_core_if_open(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
) -> bool {
    let Some(cid) = st.model.field.cluster_id_for_core_public(node_id) else {
        return false;
    };
    let monitor = st
        .model
        .monitor_state
        .node_monitor
        .get(&node_id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    if st.cluster_bloom_for_monitor(monitor.as_str()) != Some(cid) {
        return false;
    }
    st.close_cluster_bloom_for_monitor(monitor.as_str())
}

pub(super) fn restore_fullscreen_click_focus(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    now: Instant,
) -> bool {
    if !st.is_fullscreen_active(node_id) {
        return false;
    }

    let monitor_name = st
        .fullscreen_monitor_for_node(node_id)
        .map(str::to_owned)
        .or_else(|| st.model.monitor_state.node_monitor.get(&node_id).cloned())
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());

    let entry = st
        .model
        .fullscreen_state
        .fullscreen_restore
        .get(&node_id)
        .copied();
    let fallback_center = st
        .model
        .monitor_state
        .monitors
        .get(monitor_name.as_str())
        .map(|space| space.viewport.center)
        .unwrap_or(st.model.viewport.center);
    let target_center = st
        .model
        .field
        .node(node_id)
        .map(|node| node.pos)
        .or_else(|| entry.map(|e| e.viewport_center))
        .unwrap_or(fallback_center);

    st.set_interaction_monitor(monitor_name.as_str());
    let _ = st.activate_monitor(monitor_name.as_str());
    if let Some(space) = st
        .model
        .monitor_state
        .monitors
        .get_mut(monitor_name.as_str())
    {
        let one_x_zoom = halley_core::field::Vec2 {
            x: space.width as f32,
            y: space.height as f32,
        };
        space.viewport.center = target_center;
        space.camera_target_center = target_center;
        space.viewport.size = one_x_zoom;
        space.zoom_ref_size = one_x_zoom;
        space.camera_target_view_size = one_x_zoom;
    }
    if st.model.monitor_state.current_monitor == monitor_name {
        let one_x_zoom = st
            .model
            .monitor_state
            .monitors
            .get(monitor_name.as_str())
            .map(|space| halley_core::field::Vec2 {
                x: space.width as f32,
                y: space.height as f32,
            })
            .unwrap_or(st.model.viewport.size);
        st.model.viewport.center = target_center;
        st.model.camera_target_center = target_center;
        st.model.viewport.size = one_x_zoom;
        st.model.zoom_ref_size = one_x_zoom;
        st.model.camera_target_view_size = one_x_zoom;
        st.runtime.tuning.viewport_center = target_center;
        st.runtime.tuning.viewport_size = one_x_zoom;
        st.input.interaction_state.viewport_pan_anim = None;
    }

    st.set_interaction_focus(Some(node_id), 30_000, now);
    true
}

pub(super) fn handle_core_left_press(
    st: &mut Halley,
    _ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
    frame: ButtonFrame,
) {
    let now = Instant::now();
    let now_ms = st.now_ms(now);
    st.input.interaction_state.pending_core_hover = None;
    st.set_interaction_focus(Some(hit.node_id), 700, now);
    if st
        .input
        .interaction_state
        .pending_core_click
        .as_ref()
        .is_some_and(|pending| {
            pending.node_id == hit.node_id
                && pending.monitor == st.model.monitor_state.current_monitor
                && pending.deadline_ms > now_ms
        })
    {
        let _ = st.toggle_cluster_workspace_by_core(hit.node_id, now);
        st.input.interaction_state.pending_core_click = None;
    } else {
        st.input.interaction_state.pending_core_press = Some(PendingCorePress {
            node_id: hit.node_id,
            monitor: st.model.monitor_state.current_monitor.clone(),
            press_global_sx: frame.global_sx,
            press_global_sy: frame.global_sy,
        });
    }
    backend.request_redraw();
}

pub(super) fn handle_workspace_left_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
) {
    let now = Instant::now();
    let monitor = st.model.monitor_state.current_monitor.clone();
    if let Some(rect) = st.cluster_overflow_rect_for_monitor(monitor.as_str()) {
        let (.., local_sx, local_sy) =
            st.local_screen_in_monitor(monitor.as_str(), ps.screen.0, ps.screen.1);
        let inside = local_sx >= rect.x
            && local_sx <= rect.x + rect.w
            && local_sy >= rect.y
            && local_sy <= rect.y + rect.h;
        if inside {
            st.reveal_cluster_overflow_for_monitor(monitor.as_str(), st.now_ms(now));
        } else {
            st.hide_cluster_overflow_for_monitor(monitor.as_str());
        }
    }
    if hit.on_titlebar && !hit.is_core {
        backend.request_redraw();
        return;
    }
    let focus_hold_ms = if hit.is_core { 700 } else { 30_000 };
    let focus_target = stack_focus_target_for_node(st, hit.node_id).unwrap_or(hit.node_id);
    st.set_recent_top_node(focus_target, now + std::time::Duration::from_millis(1200));
    st.set_interaction_focus(Some(focus_target), focus_hold_ms, now);
    backend.request_redraw();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::interface::TtyBackendHandle;
    use smithay::reexports::wayland_server::Display;

    fn single_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
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
    fn workspace_titlebar_double_click_does_not_exit_cluster() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let backend = TtyBackendHandle::new(800, 600);

        let master = st.model.field.spawn_surface(
            "master",
            halley_core::field::Vec2 { x: 100.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = st.model.field.spawn_surface(
            "stack",
            halley_core::field::Vec2 { x: 500.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [master, stack] {
            st.assign_node_to_monitor(id, "monitor_a");
        }
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");

        let now = Instant::now();
        assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));

        let mut ps = PointerState::default();

        handle_workspace_left_press(
            &mut st,
            &mut ps,
            &backend,
            HitNode {
                node_id: master,
                on_titlebar: true,
                is_core: false,
            },
        );

        assert_eq!(
            st.active_cluster_workspace_for_monitor("monitor_a"),
            Some(cid)
        );
    }

    #[test]
    fn core_single_click_only_focuses_without_opening_bloom() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let backend = TtyBackendHandle::new(800, 600);

        let master = st.model.field.spawn_surface(
            "master",
            halley_core::field::Vec2 { x: 100.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = st.model.field.spawn_surface(
            "stack",
            halley_core::field::Vec2 { x: 500.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [master, stack] {
            st.assign_node_to_monitor(id, "monitor_a");
        }
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");

        let mut ps = PointerState::default();
        handle_core_left_press(
            &mut st,
            &mut ps,
            &backend,
            HitNode {
                node_id: core,
                on_titlebar: true,
                is_core: true,
            },
            ButtonFrame {
                ws_w: 800,
                ws_h: 600,
                global_sx: 400.0,
                global_sy: 300.0,
                sx: 400.0,
                sy: 300.0,
                world_now: halley_core::field::Vec2 { x: 400.0, y: 300.0 },
                workspace_active: false,
            },
        );

        assert_eq!(st.model.focus_state.primary_interaction_focus, Some(core));
        let pending_press = st
            .input
            .interaction_state
            .pending_core_press
            .take()
            .expect("pending core press");
        st.input.interaction_state.pending_core_click = Some(PendingCoreClick {
            node_id: pending_press.node_id,
            monitor: pending_press.monitor,
            deadline_ms: st.now_ms(Instant::now()),
        });

        st.run_maintenance(Instant::now());

        assert_eq!(st.cluster_bloom_for_monitor("monitor_a"), None);
        assert_eq!(st.active_cluster_workspace_for_monitor("monitor_a"), None);
    }

    #[test]
    fn core_double_click_enters_cluster_workspace() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let backend = TtyBackendHandle::new(800, 600);

        let master = st.model.field.spawn_surface(
            "master",
            halley_core::field::Vec2 { x: 100.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = st.model.field.spawn_surface(
            "stack",
            halley_core::field::Vec2 { x: 500.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [master, stack] {
            st.assign_node_to_monitor(id, "monitor_a");
        }
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");

        let frame = ButtonFrame {
            ws_w: 800,
            ws_h: 600,
            global_sx: 400.0,
            global_sy: 300.0,
            sx: 400.0,
            sy: 300.0,
            world_now: halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            workspace_active: false,
        };
        let mut ps = PointerState::default();
        handle_core_left_press(
            &mut st,
            &mut ps,
            &backend,
            HitNode {
                node_id: core,
                on_titlebar: true,
                is_core: true,
            },
            frame,
        );
        let pending_press = st
            .input
            .interaction_state
            .pending_core_press
            .take()
            .expect("pending core press");
        st.input.interaction_state.pending_core_click = Some(PendingCoreClick {
            node_id: pending_press.node_id,
            monitor: pending_press.monitor,
            deadline_ms: st.now_ms(Instant::now()) + 350,
        });

        handle_core_left_press(
            &mut st,
            &mut ps,
            &backend,
            HitNode {
                node_id: core,
                on_titlebar: true,
                is_core: true,
            },
            frame,
        );

        assert_eq!(
            st.active_cluster_workspace_for_monitor("monitor_a"),
            Some(cid)
        );
    }

    #[test]
    fn hovering_core_long_enough_opens_bloom() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = st.model.field.spawn_surface(
            "master",
            halley_core::field::Vec2 { x: 100.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = st.model.field.spawn_surface(
            "stack",
            halley_core::field::Vec2 { x: 500.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [master, stack] {
            st.assign_node_to_monitor(id, "monitor_a");
        }
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");

        st.input.interaction_state.pending_core_hover =
            Some(crate::compositor::interaction::state::PendingCoreHover {
                node_id: core,
                monitor: "monitor_a".to_string(),
                started_at_ms: st.now_ms(Instant::now()),
            });

        crate::render::tick_frame_effects(
            &mut st,
            Instant::now()
                + Duration::from_millis(crate::compositor::interaction::CORE_BLOOM_HOLD_MS + 1),
        );

        assert_eq!(st.cluster_bloom_for_monitor("monitor_a"), Some(cid));
        assert!(st.input.interaction_state.pending_core_hover.is_none());
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

#[derive(Clone, Copy)]
pub(crate) struct ButtonFrame {
    pub(super) ws_w: i32,
    pub(super) ws_h: i32,
    pub(super) global_sx: f32,
    pub(super) global_sy: f32,
    pub(super) sx: f32,
    pub(super) sy: f32,
    pub(super) world_now: halley_core::field::Vec2,
    pub(super) workspace_active: bool,
}

#[inline]
pub(super) fn now_millis_u32() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_millis() & 0xffff_ffff) as u32)
        .unwrap_or(0)
}

#[inline]
pub(super) fn clamp_screen_to_workspace(ws_w: i32, ws_h: i32, sx: f32, sy: f32) -> (f32, f32) {
    let max_x = (ws_w.max(1) - 1) as f32;
    let max_y = (ws_h.max(1) - 1) as f32;
    (sx.clamp(0.0, max_x), sy.clamp(0.0, max_y))
}

#[inline]
pub(super) fn clamp_screen_to_monitor(st: &Halley, name: &str, sx: f32, sy: f32) -> (f32, f32) {
    if let Some(monitor) = st.model.monitor_state.monitors.get(name) {
        let max_x = (monitor.offset_x + monitor.width - 1) as f32;
        let max_y = (monitor.offset_y + monitor.height - 1) as f32;
        (
            sx.clamp(monitor.offset_x as f32, max_x),
            sy.clamp(monitor.offset_y as f32, max_y),
        )
    } else {
        (sx, sy)
    }
}

#[inline]
fn modifier_specificity(modifiers: KeyModifiers) -> u32 {
    [
        modifiers.super_key,
        modifiers.left_super,
        modifiers.right_super,
        modifiers.alt,
        modifiers.left_alt,
        modifiers.right_alt,
        modifiers.ctrl,
        modifiers.left_ctrl,
        modifiers.right_ctrl,
        modifiers.shift,
        modifiers.left_shift,
        modifiers.right_shift,
    ]
    .into_iter()
    .filter(|enabled| *enabled)
    .count() as u32
}

#[inline]
pub(super) fn active_pointer_binding(
    st: &Halley,
    mods: &ModState,
    button_code: u32,
) -> Option<PointerBindingAction> {
    if crate::compositor::interaction::pointer::active_constrained_pointer_surface(st).is_some() {
        return None;
    }
    st.runtime
        .tuning
        .pointer_bindings
        .iter()
        .filter(|binding| binding.button == button_code && modifier_active(mods, binding.modifiers))
        .max_by_key(|binding| modifier_specificity(binding.modifiers))
        .map(|binding| binding.action)
}

pub(super) fn button_frame_for_monitor(
    st: &mut Halley,
    ws_w: i32,
    ws_h: i32,
    screen: (f32, f32),
) -> (ButtonFrame, String, (f32, f32)) {
    let (sx, sy) = clamp_screen_to_workspace(ws_w, ws_h, screen.0, screen.1);
    let grabbed_layer_surface_monitor = st
        .input
        .interaction_state
        .grabbed_layer_surface
        .as_ref()
        .map(|surface| {
            crate::compositor::monitor::layer_shell::layer_surface_monitor_name(st, surface)
        });
    let target_monitor =
        crate::compositor::interaction::pointer::active_constrained_pointer_surface(st)
            .and_then(|(surface, _)| {
                let node_id = st.model.surface_to_node.get(&surface.id()).copied()?;
                Some(
                    st.model
                        .monitor_state
                        .node_monitor
                        .get(&node_id)
                        .cloned()
                        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone()),
                )
            })
            .or(grabbed_layer_surface_monitor)
            .unwrap_or_else(|| {
                st.monitor_for_screen(sx, sy)
                    .unwrap_or_else(|| st.interaction_monitor().to_string())
            });
    if st.input.interaction_state.grabbed_layer_surface.is_none() {
        st.set_interaction_monitor(target_monitor.as_str());
        let _ = st.activate_monitor(target_monitor.as_str());
    }
    st.input.interaction_state.last_pointer_screen_global = Some((sx, sy));
    let (local_w, local_h, local_sx, local_sy) =
        st.local_screen_in_monitor(target_monitor.as_str(), sx, sy);
    let world_now = screen_to_world(st, local_w, local_h, local_sx, local_sy);
    (
        ButtonFrame {
            ws_w: local_w,
            ws_h: local_h,
            global_sx: sx,
            global_sy: sy,
            sx: local_sx,
            sy: local_sy,
            world_now,
            workspace_active: st.has_active_cluster_workspace(),
        },
        target_monitor,
        (sx, sy),
    )
}
