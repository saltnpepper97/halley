use std::time::Instant;

use eventline::info;
use halley_config::{KeyModifiers, PointerBindingAction};

use crate::backend::interface::BackendView;
use crate::compositor::actions::window::{
    activate_collapsed_node_from_click, focus_or_reveal_surface_node,
};
use crate::compositor::interaction::{
    BloomDragCtx, HitNode, ModState, OverflowDragCtx, PointerState, TitleClickCtx,
    NODE_DOUBLE_CLICK_MS,
};
use crate::input::ctx::InputCtx;
use crate::input::keyboard::modkeys::modifier_active;
use crate::overlay::{
    bloom_token_hit_test, cluster_overflow_icon_hit_test, cluster_overflow_strip_slot_at,
};
use crate::render::bearing_hit_test;
use crate::spatial::screen_to_world;
use crate::spatial::pick_hit_node_at;
use crate::compositor::interaction::state::{PendingCoreClick, PendingCorePress};
use crate::compositor::root::Halley;
use smithay::backend::input::ButtonState;
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::SERIAL_COUNTER;

use crate::input::keyboard::bindings::{
    apply_bound_pointer_input, apply_compositor_action_press, compositor_binding_action_active,
};
use super::motion::{begin_drag, finish_pointer_drag, node_is_pointer_draggable};
use super::focus::{layer_surface_focus_for_screen, pointer_focus_for_screen};
use super::resize::{begin_resize, finalize_resize};

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

pub(crate) fn handle_pointer_button_input<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    button_code: u32,
    button_state: ButtonState,
) {
    let left = button_code == 0x110;
    let right = button_code == 0x111;
    let mut ps = ctx.pointer_state.borrow_mut();
    let (ws_w, ws_h) = ctx.backend.window_size_i32();
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
        let monitor = crate::compositor::monitor::layer_shell::layer_surface_monitor_name(st, surface);
        st.model.spawn_state.pending_spawn_monitor = Some(monitor.clone());
        info!(
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
    if st.cluster_mode_active() && !cluster_pointer_passthrough {
        ps.world = world_now;
        match button_state {
            ButtonState::Pressed if left => {
                if let Some((surface, _)) = layer_focus.as_ref() {
                    let _ = crate::compositor::monitor::layer_shell::focus_layer_surface(st, surface);
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
                ps.last_title_click = None;
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
            core_screen: (layout.core_sx as f32, layout.core_sy as f32),
        });
        st.input.interaction_state.bloom_pull_preview = Some(
            crate::compositor::interaction::state::BloomPullPreview {
            cluster_id: layout.cluster_id,
            member_id: layout.member_id,
            mix: 0.0,
        });
        ps.last_title_click = None;
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
        ps.last_title_click = None;
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
        st.input.interaction_state.cluster_overflow_drag_preview =
            Some(crate::compositor::interaction::state::ClusterOverflowDragPreview {
                member_id: hit.member_id,
                monitor: target_monitor.clone(),
                screen_local: (local_sx, local_sy),
            });
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(smithay::input::pointer::CursorIcon::Grabbing),
        );
        ps.last_title_click = None;
        ctx.backend.request_redraw();
        return;
    }
    let intercepted_binding = match button_state {
        ButtonState::Pressed => {
            if let Some(action) = compositor_binding_action_active(st, button_code, &mods) {
                ps.intercepted_binding_buttons.insert(button_code);
                ps.panning = false;
                let _ = apply_compositor_action_press(st, action, ctx.config_path, ctx.wayland_display);
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
                ctx.backend.request_redraw();
                return;
            }
            if left && ps.bloom_drag.take().is_some() {
                st.input.interaction_state.bloom_pull_preview = None;
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
            handle_button_release(st, &mut ps, ctx.backend, button_code, matched_action, world_now);
        }
    }
}



pub(super) fn title_click_is_double(
    ps: &PointerState,
    node_id: halley_core::field::NodeId,
    now: Instant,
) -> bool {
    ps.last_title_click.is_some_and(|last| {
        last.node_id == node_id
            && now.duration_since(last.at).as_millis() as u64 <= NODE_DOUBLE_CLICK_MS
    })
}

pub(super) fn set_title_click(
    ps: &mut PointerState,
    node_id: halley_core::field::NodeId,
    now: Instant,
) {
    ps.last_title_click = Some(TitleClickCtx { node_id, at: now });
}

pub(super) fn clear_pointer_activity(st: &mut Halley, ps: &mut PointerState) {
    if let Some(drag) = ps.drag {
        crate::compositor::carry::system::set_drag_authority_node(st, None);
        crate::compositor::carry::system::end_carry_state_tracking(st, drag.node_id);
    }
    crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
    st.input.interaction_state.active_drag = None;
    st.input.interaction_state.pending_core_press = None;
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
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
    frame: ButtonFrame,
) {
    let now = Instant::now();
    st.set_interaction_focus(Some(hit.node_id), 700, now);
    let was_bloom_open = collapse_bloom_for_core_if_open(st, hit.node_id);
    let now_ms = st.now_ms(now);
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
        ps.last_title_click = None;
    } else {
        st.input.interaction_state.pending_core_press = Some(PendingCorePress {
            node_id: hit.node_id,
            monitor: st.model.monitor_state.current_monitor.clone(),
            press_global_sx: frame.global_sx,
            press_global_sy: frame.global_sy,
            reopen_bloom_on_timeout: !was_bloom_open,
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
    let focus_hold_ms = if hit.on_titlebar || hit.is_core {
        700
    } else {
        30_000
    };
    st.set_interaction_focus(Some(hit.node_id), focus_hold_ms, now);
    if hit.on_titlebar || hit.is_core {
        if title_click_is_double(ps, hit.node_id, now) {
            let _ = st.exit_cluster_workspace_if_member(hit.node_id, now);
            ps.last_title_click = None;
            clear_pointer_activity(st, ps);
            backend.request_redraw();
        } else {
            set_title_click(ps, hit.node_id, now);
            backend.request_redraw();
        }
    } else {
        ps.last_title_click = None;
        backend.request_redraw();
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
    let focus = pointer_focus_for_screen(
        st,
        frame.ws_w,
        frame.ws_h,
        frame.sx,
        frame.sy,
        std::time::Instant::now(),
        resize_preview,
    );
    let motion_serial = SERIAL_COUNTER.next_serial();
    let button_serial = SERIAL_COUNTER.next_serial();
    let location = if focus
        .as_ref()
        .is_some_and(|(surface, _)| crate::compositor::monitor::layer_shell::is_layer_surface(st, surface))
    {
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
    let target_monitor = crate::compositor::interaction::pointer::active_locked_pointer_surface(st)
        .and_then(|surface| {
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
        .unwrap_or_else(|| {
            st.monitor_for_screen(sx, sy)
                .unwrap_or_else(|| st.interaction_monitor().to_string())
        });
    st.set_interaction_monitor(target_monitor.as_str());
    let _ = st.activate_monitor(target_monitor.as_str());
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
