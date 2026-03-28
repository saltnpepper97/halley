use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use eventline::info;
use halley_config::PointerBindingAction;

use crate::backend::interface::BackendView;
use crate::interaction::actions::{
    activate_collapsed_node_from_click, focus_or_reveal_surface_node,
};
use crate::interaction::types::{BloomDragCtx, HitNode, ModState, OverflowDragCtx, PointerState};
use crate::overlay::{
    bloom_token_hit_test, cluster_overflow_icon_hit_test, cluster_overflow_strip_slot_at,
};
use crate::render::bearing_hit_test;
use crate::spatial::pick_hit_node_at;
use crate::state::{Halley, PendingCoreClick};
use smithay::backend::input::ButtonState;

use super::key_actions::{
    apply_bound_pointer_input, apply_compositor_action_press, compositor_binding_action_active,
};
use super::pointer_core::{
    clear_pointer_activity, collapse_bloom_for_core_if_open, handle_core_left_press,
    handle_workspace_left_press, restore_fullscreen_click_focus, set_title_click,
    title_click_is_double,
};
use super::pointer_dispatch::dispatch_pointer_button;
use super::pointer_drag::{begin_drag, finish_pointer_drag, node_is_pointer_draggable};
use super::pointer_focus::layer_surface_focus_for_screen;
use super::pointer_frame::{ButtonFrame, active_pointer_binding, button_frame_for_monitor};
use super::pointer_resize::{begin_resize, finalize_resize};

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
        ps.last_title_click = None;
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
            );
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
        ps.last_title_click = None;
        backend.request_redraw();
    }

    if !drag_binding_active
        && st.model.field.node(hit.node_id).is_some_and(|n| {
            n.kind == halley_core::field::NodeKind::Surface
                && st.model.field.is_visible(hit.node_id)
        })
    {
        st.set_interaction_focus(Some(hit.node_id), 30_000, Instant::now());
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
            ps.last_title_click = None;
            handled_node_click = true;
        }
    }

    let drag_target_ok = node_is_pointer_draggable(st, hit.node_id);
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
        );
        return;
    }

    if hit.on_titlebar || hit.is_core {
        let now = Instant::now();
        st.set_interaction_focus(Some(hit.node_id), 700, now);
        if !hit.is_core && title_click_is_double(ps, hit.node_id, now) {
            ps.last_title_click = None;
            backend.request_redraw();
        } else {
            if !hit.is_core {
                set_title_click(ps, hit.node_id, now);
            }
        }
    } else if !handled_node_click {
        ps.last_title_click = None;
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
        backend.request_redraw();
        return;
    };
    let can_resize = st
        .model
        .field
        .node(hit.node_id)
        .is_some_and(|n| n.state == halley_core::field::NodeState::Active);
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
        ps.last_title_click = None;
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
        backend.request_redraw();
        return;
    };
    let can_resize = st
        .model
        .field
        .node(hit.node_id)
        .is_some_and(|n| n.state == halley_core::field::NodeState::Active);
    if can_resize {
        // Binding-triggered resize always starts Pending regardless of where
        // the cursor is — drag direction picks the handle.
        begin_resize(st, ps, backend, hit, frame);
    }
}

fn handle_button_release(
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
            }
            st.input.interaction_state.active_drag = None;
            ps.panning = false;
            ps.pan_monitor = None;
            if ps.resize.is_some() {
                finalize_resize(st, ps, backend);
            }
        }
        Some(PointerBindingAction::ResizeWindow) => {
            finalize_resize(st, ps, backend);
        }
        None => {
            if button_code == 0x110
                && let Some(d) = ps.drag
            {
                let now = Instant::now();
                finish_pointer_drag(st, ps, d.node_id, d.started_active, world_now, now);
                st.input.interaction_state.active_drag = None;
            }
            if button_code == 0x110 || button_code == 0x111 {
                ps.panning = false;
                ps.pan_monitor = None;
            }
        }
    }
}

pub(crate) fn handle_pointer_button_input(
    st: &mut Halley,
    backend: &impl BackendView,
    mod_state: &Rc<RefCell<ModState>>,
    pointer_state: &Rc<RefCell<PointerState>>,
    config_path: &str,
    wayland_display: &str,
    button_code: u32,
    button_state: ButtonState,
) {
    let left = button_code == 0x110;
    let right = button_code == 0x111;
    let mut ps = pointer_state.borrow_mut();
    let (ws_w, ws_h) = backend.window_size_i32();
    let (frame, target_monitor, clamped_screen) =
        button_frame_for_monitor(st, ws_w, ws_h, ps.screen);
    let (sx, sy) = clamped_screen;
    let (local_w, local_h, local_sx, local_sy) = (frame.ws_w, frame.ws_h, frame.sx, frame.sy);
    ps.screen = (sx, sy);
    ps.workspace_size = (local_w, local_h);
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
        let monitor = st.layer_surface_monitor_name(surface);
        st.model.spawn_state.pending_spawn_monitor = Some(monitor.clone());
        info!(
            "pending spawn monitor latched from layer press: {}",
            monitor
        );
    }
    let world_now = frame.world_now;
    let mods = mod_state.borrow().clone();
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
    if st.cluster_mode_active() && !cluster_pointer_passthrough {
        ps.world = world_now;
        match button_state {
            ButtonState::Pressed if left => {
                if let Some((surface, _)) = layer_focus.as_ref() {
                    let _ = st.focus_layer_surface(surface);
                    ps.last_title_click = None;
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
                    ps.last_title_click = None;
                    backend.request_redraw();
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
                ps.last_title_click = None;
                backend.request_redraw();
                return;
            }
            ButtonState::Released if left || right => {
                handle_button_release(st, &mut ps, backend, button_code, None, world_now);
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
            core_screen: (layout.core_sx as f32, layout.core_sy as f32),
        });
        st.input.interaction_state.bloom_pull_preview = Some(crate::state::BloomPullPreview {
            cluster_id: layout.cluster_id,
            member_id: layout.member_id,
            mix: 0.0,
        });
        ps.last_title_click = None;
        backend.request_redraw();
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
        ps.last_title_click = None;
        ps.panning = false;
        backend.request_redraw();
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
        st.input.interaction_state.cluster_overflow_drag_preview =
            Some(crate::state::ClusterOverflowDragPreview {
                member_id: hit.member_id,
                monitor: target_monitor.clone(),
                screen_local: (local_sx, local_sy),
            });
        st.set_cursor_override_icon(Some(smithay::input::pointer::CursorIcon::Grabbing));
        ps.last_title_click = None;
        backend.request_redraw();
        return;
    }
    let intercepted_binding = match button_state {
        ButtonState::Pressed => {
            if let Some(action) = compositor_binding_action_active(st, button_code, &mods) {
                ps.intercepted_binding_buttons.insert(button_code);
                ps.panning = false;
                let _ = apply_compositor_action_press(st, action, config_path, wayland_display);
                backend.request_redraw();
                true
            } else if apply_bound_pointer_input(
                st,
                button_code,
                &mods,
                config_path,
                wayland_display,
            ) {
                ps.intercepted_binding_buttons.insert(button_code);
                ps.panning = false;
                backend.request_redraw();
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
        let _ = st.focus_layer_surface(&surface);
        ps.last_title_click = None;
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
                    backend,
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
                    backend,
                    matches!(matched_action, Some(PointerBindingAction::ResizeWindow)),
                    hit,
                    frame,
                );
            } else {
                match matched_action {
                    Some(PointerBindingAction::MoveWindow) => {
                        handle_move_binding_press(st, &mut ps, backend, hit, frame, false);
                    }
                    Some(PointerBindingAction::FieldJump) => {
                        handle_move_binding_press(st, &mut ps, backend, hit, frame, true);
                    }
                    Some(PointerBindingAction::ResizeWindow) => {
                        handle_resize_binding_press(st, &mut ps, backend, hit, frame);
                    }
                    None => {}
                }
            }
        }
        ButtonState::Released => {
            if left
                && let Some(pending_press) = st.input.interaction_state.pending_core_press.take()
            {
                let now = Instant::now();
                st.input.interaction_state.pending_core_click = Some(PendingCoreClick {
                    node_id: pending_press.node_id,
                    monitor: pending_press.monitor,
                    deadline_ms: st.now_ms(now).saturating_add(180),
                    reopen_bloom_on_timeout: pending_press.reopen_bloom_on_timeout,
                });
                st.request_maintenance();
                backend.request_redraw();
                return;
            }
            if left && ps.bloom_drag.take().is_some() {
                st.input.interaction_state.bloom_pull_preview = None;
                backend.request_redraw();
                return;
            }
            if left && let Some(overflow_drag) = ps.overflow_drag.take() {
                let now = Instant::now();
                st.input.interaction_state.cluster_overflow_drag_preview = None;
                st.set_cursor_override_icon(None);
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
                        backend.request_redraw();
                    } else {
                        st.reveal_cluster_overflow_for_monitor(
                            overflow_drag.monitor.as_str(),
                            now_ms,
                        );
                        backend.request_redraw();
                    }
                    return;
                }
                let release_hit =
                    pick_hit_node_at(st, local_w, local_h, local_sx, local_sy, now, ps.resize);
                if overflow_drag.monitor == target_monitor
                    && let Some(hit) = release_hit
                    && let Some(cluster) = st.model.field.cluster(overflow_drag.cluster_id)
                    && cluster.visible_members().contains(&hit.node_id)
                {
                    let swapped = st.swap_cluster_overflow_member_with_visible(
                        overflow_drag.monitor.as_str(),
                        overflow_drag.cluster_id,
                        overflow_drag.member_id,
                        hit.node_id,
                        st.now_ms(now),
                    );
                    if swapped {
                        backend.request_redraw();
                    }
                    return;
                }
                st.reveal_cluster_overflow_for_monitor(overflow_drag.monitor.as_str(), now_ms);
                backend.request_redraw();
                return;
            }
            if intercepted_binding {
                return;
            }
            handle_button_release(st, &mut ps, backend, button_code, matched_action, world_now);
        }
    }
}
