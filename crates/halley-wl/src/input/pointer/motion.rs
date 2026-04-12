use std::time::{Duration, Instant};

use smithay::input::pointer::{MotionEvent, RelativeMotionEvent};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::SERIAL_COUNTER;

use crate::backend::interface::BackendView;
use crate::compositor::exit_confirm::exit_confirm_controller;
use crate::compositor::interaction::state::ActiveDragState;
use crate::compositor::interaction::{DragAxisMode, DragCtx, HitNode, ModState, PointerState};
use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::root::Halley;
use crate::compositor::surface_ops::{
    is_active_stacking_workspace_member, stack_focus_target_for_node,
};
use crate::input::ctx::InputCtx;
use crate::spatial::{pick_hit_node_at, screen_to_world};
use halley_config::{InputFocusMode, PointerBindingAction};

use super::button::{
    ButtonFrame, active_pointer_binding, clamp_screen_to_monitor, clamp_screen_to_workspace,
    now_millis_u32,
};
use super::focus::{grabbed_layer_surface_focus, pointer_focus_for_screen};
use super::resize::handle_resize_motion;
use super::screenshot::handle_screenshot_pointer_motion;
use crate::input::keyboard::modkeys::modifier_active;

fn detach_bloom_drag_into_pointer_drag(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &impl BackendView,
    bloom_drag: crate::compositor::interaction::BloomDragCtx,
    effective_sx: f32,
    effective_sy: f32,
) {
    let (bloom_w, bloom_h, bloom_local_sx, bloom_local_sy) =
        st.local_screen_in_monitor(bloom_drag.monitor.as_str(), effective_sx, effective_sy);
    let previous_monitor = st.begin_temporary_render_monitor(bloom_drag.monitor.as_str());
    let pointer_world =
        crate::spatial::screen_to_world(st, bloom_w, bloom_h, bloom_local_sx, bloom_local_sy);
    st.end_temporary_render_monitor(previous_monitor);
    let now = Instant::now();
    if st.detach_member_from_cluster(
        bloom_drag.cluster_id,
        bloom_drag.member_id,
        pointer_world,
        now,
    ) {
        st.assign_node_to_monitor(bloom_drag.member_id, bloom_drag.monitor.as_str());
        st.set_interaction_focus(Some(bloom_drag.member_id), 30_000, now);
        begin_drag(
            st,
            ps,
            backend,
            HitNode {
                node_id: bloom_drag.member_id,
                on_titlebar: false,
                is_core: false,
            },
            ButtonFrame {
                ws_w: bloom_w,
                ws_h: bloom_h,
                global_sx: effective_sx,
                global_sy: effective_sy,
                sx: bloom_local_sx,
                sy: bloom_local_sy,
                world_now: pointer_world,
                workspace_active: false,
            },
            pointer_world,
            false,
            false,
        );
    }
}

pub(crate) fn node_is_pointer_draggable(st: &Halley, node_id: halley_core::field::NodeId) -> bool {
    if st.is_fullscreen_active(node_id) {
        return false;
    }
    if is_active_stacking_workspace_member(st, node_id) {
        return false;
    }
    st.model.field.node(node_id).is_some_and(|n| match n.kind {
        halley_core::field::NodeKind::Surface => st.model.field.is_visible(node_id),
        halley_core::field::NodeKind::Core => n.state == halley_core::field::NodeState::Core,
    })
}

pub(crate) fn begin_drag(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
    frame: ButtonFrame,
    world_now: halley_core::field::Vec2,
    allow_monitor_transfer: bool,
    requires_drag_modifier: bool,
) {
    st.input.interaction_state.pending_core_press = None;
    st.input.interaction_state.pending_core_click = None;
    let drag_monitor = st
        .model
        .monitor_state
        .node_monitor
        .get(&hit.node_id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    let edge_pan_eligible = false;
    let mut drag_ctx = DragCtx {
        node_id: hit.node_id,
        allow_monitor_transfer,
        requires_drag_modifier,
        edge_pan_eligible,
        current_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
        center_latched: false,
        started_active: false,
        edge_pan_x: DragAxisMode::Free,
        edge_pan_y: DragAxisMode::Free,
        edge_pan_pressure: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
        last_pointer_world: world_now,
        last_update_at: Instant::now(),
        release_velocity: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
    };
    if let Some(n) = st.model.field.node(hit.node_id) {
        drag_ctx.started_active = n.state == halley_core::field::NodeState::Active;
        let off = halley_core::field::Vec2 {
            x: world_now.x - n.pos.x,
            y: world_now.y - n.pos.y,
        };
        if st.runtime.tuning.center_window_to_mouse {
            drag_ctx.current_offset = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
            drag_ctx.center_latched = true;
        } else {
            drag_ctx.current_offset = off;
        }
    }
    ps.drag = Some(drag_ctx);
    let _ = st.model.field.set_pinned(hit.node_id, false);
    st.assign_node_to_monitor(hit.node_id, drag_monitor.as_str());
    st.input
        .interaction_state
        .physics_velocity
        .remove(&hit.node_id);
    st.input.interaction_state.drag_authority_velocity =
        halley_core::field::Vec2 { x: 0.0, y: 0.0 };
    crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
    st.input.interaction_state.active_drag = Some(ActiveDragState {
        node_id: hit.node_id,
        allow_monitor_transfer,
        edge_pan_eligible,
        current_offset: drag_ctx.current_offset,
        pointer_monitor: drag_monitor,
        pointer_workspace_size: (frame.ws_w, frame.ws_h),
        pointer_screen_local: (frame.sx, frame.sy),
        edge_pan_x: DragAxisMode::Free,
        edge_pan_y: DragAxisMode::Free,
    });
    crate::compositor::carry::system::set_drag_authority_node(st, Some(hit.node_id));
    crate::compositor::carry::system::begin_carry_state_tracking(st, hit.node_id);
    crate::compositor::interaction::pointer::set_cursor_override_icon(
        st,
        Some(smithay::input::pointer::CursorIcon::Grabbing),
    );
    if !hit.is_core {
        let focus_target = stack_focus_target_for_node(st, hit.node_id).unwrap_or(hit.node_id);
        st.set_recent_top_node(
            focus_target,
            Instant::now() + std::time::Duration::from_millis(1200),
        );
        st.set_interaction_focus(Some(focus_target), 30_000, Instant::now());
    }
    let to = halley_core::field::Vec2 {
        x: world_now.x - drag_ctx.current_offset.x,
        y: world_now.y - drag_ctx.current_offset.y,
    };
    let _ = st.carry_surface_non_overlap(hit.node_id, to, false);
    backend.request_redraw();
}

fn apply_hover_focus_mode(st: &mut Halley, hit: Option<HitNode>, blocked: bool, now: Instant) {
    if !hover_focus_enabled(
        st.runtime.tuning.input.focus_mode,
        blocked,
        crate::compositor::monitor::layer_shell::keyboard_focus_is_layer_surface(st),
    ) {
        return;
    }
    let Some(hit) = hit else {
        return;
    };
    if hit.is_core {
        return;
    }
    let Some(node) = st.model.field.node(hit.node_id) else {
        return;
    };
    if node.kind != halley_core::field::NodeKind::Surface
        || !st.model.field.is_visible(hit.node_id)
        || !matches!(
            node.state,
            halley_core::field::NodeState::Active | halley_core::field::NodeState::Drifting
        )
    {
        return;
    }

    let focus_target = stack_focus_target_for_node(st, hit.node_id).unwrap_or(hit.node_id);
    st.set_recent_top_node(focus_target, now + Duration::from_millis(1200));
    st.set_interaction_focus(Some(focus_target), 30_000, now);
}

fn hover_focus_enabled(
    focus_mode: InputFocusMode,
    blocked: bool,
    layer_shell_keyboard_focus: bool,
) -> bool {
    !blocked && focus_mode == InputFocusMode::Hover && !layer_shell_keyboard_focus
}

pub(crate) fn finish_pointer_drag(
    st: &mut Halley,
    ps: &mut PointerState,
    node_id: halley_core::field::NodeId,
    started_active: bool,
    world_now: halley_core::field::Vec2,
    now: Instant,
) {
    let now_ms = st.now_ms(now);
    let drag_monitor = st
        .input
        .interaction_state
        .active_drag
        .as_ref()
        .map(|drag| drag.pointer_monitor.clone())
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
    st.input.interaction_state.active_drag = None;
    let joined = st.commit_ready_cluster_join_for_node(node_id, now);
    if !joined {
        let moved_in_cluster = if started_active {
            st.move_active_cluster_member_to_drop_tile(
                drag_monitor.as_str(),
                node_id,
                world_now,
                now_ms,
            )
        } else {
            false
        };
        if started_active {
            crate::compositor::carry::system::finalize_mouse_drag_state(
                st,
                node_id,
                halley_core::field::Vec2 { x: 0.0, y: 0.0 },
                now,
            );
        } else if !moved_in_cluster {
            crate::compositor::carry::system::update_carry_state_preview(st, node_id, now);
        }
    } else {
        st.input.interaction_state.cluster_join_candidate = None;
    }
    crate::compositor::carry::system::set_drag_authority_node(st, None);
    crate::compositor::carry::system::end_carry_state_tracking(st, node_id);
    ps.preview_block_until = Some(now + Duration::from_millis(360));
    ps.drag = None;
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
    if exit_confirm_controller(&*st).active() {
        return;
    }
    if crate::compositor::interaction::state::note_cursor_activity(st, st.now_ms(Instant::now())) {
        ctx.backend.request_redraw();
    }

    let allow_unbounded_screen = {
        let ps = ctx.pointer_state.borrow();
        ps.drag.is_some() || ps.resize.is_some() || ps.panning
    };

    let raw_sx = sx;
    let raw_sy = sy;
    let (sx, sy) = clamp_screen_to_workspace(ws_w, ws_h, raw_sx, raw_sy);
    let now = Instant::now();
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        let target_monitor = st
            .monitor_for_screen(sx, sy)
            .unwrap_or_else(|| st.interaction_monitor().to_string());
        let (sx, sy) = clamp_screen_to_monitor(st, target_monitor.as_str(), sx, sy);
        st.set_interaction_monitor(target_monitor.as_str());
        let _ = st.activate_monitor(target_monitor.as_str());
        let (local_w, local_h, local_sx, local_sy) =
            st.local_screen_in_monitor(target_monitor.as_str(), sx, sy);
        {
            let mut ps = ctx.pointer_state.borrow_mut();
            ps.screen = (sx, sy);
            ps.workspace_size = (local_w, local_h);
            ps.world = screen_to_world(st, local_w, local_h, local_sx, local_sy);
        }
        if let Some(pointer) = st.platform.seat.get_pointer() {
            let focus =
                pointer_focus_for_screen(st, local_w, local_h, local_sx, local_sy, now, None);
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
                    location: (local_sx as f64, local_sy as f64).into(),
                    serial: SERIAL_COUNTER.next_serial(),
                    time: now_millis_u32(),
                },
            );
        }
        return;
    }
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
    let mods = ctx.mod_state.borrow().clone();
    let drag_state = {
        let ps = ctx.pointer_state.borrow();
        ps.drag.map(|drag| {
            let owner = st
                .model
                .monitor_state
                .node_monitor
                .get(&drag.node_id)
                .cloned()
                .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
            let allow_monitor_transfer =
                active_pointer_binding(st, &mods, 0x110) == Some(PointerBindingAction::FieldJump);
            (drag, owner, allow_monitor_transfer)
        })
    };
    let constrained_surface_monitor = constrained_surface_info.as_ref().and_then(|(surface, _)| {
        let node_id = st.model.surface_to_node.get(&surface.id()).copied()?;
        Some(
            st.model
                .monitor_state
                .node_monitor
                .get(&node_id)
                .cloned()
                .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone()),
        )
    });
    let grabbed_layer_surface_monitor = grabbed_layer_surface.as_ref().map(|surface| {
        crate::compositor::monitor::layer_shell::layer_surface_monitor_name(st, surface)
    });
    let locked_drag_monitor = drag_state
        .as_ref()
        .and_then(|(_, owner, allow_monitor_transfer)| {
            (!*allow_monitor_transfer).then(|| owner.clone())
        });
    let locked_resize_monitor = {
        let ps = ctx.pointer_state.borrow();
        ps.resize.and_then(|resize| {
            st.model
                .monitor_state
                .node_monitor
                .get(&resize.node_id)
                .cloned()
                .or_else(|| Some(st.model.monitor_state.current_monitor.clone()))
        })
    };
    let locked_pan_monitor = {
        let ps = ctx.pointer_state.borrow();
        ps.panning.then(|| ps.pan_monitor.clone()).flatten()
    };
    let locked_bloom_monitor = {
        let ps = ctx.pointer_state.borrow();
        ps.bloom_drag.as_ref().map(|drag| drag.monitor.clone())
    };
    let locked_overflow_monitor = {
        let ps = ctx.pointer_state.borrow();
        ps.overflow_drag.as_ref().map(|drag| drag.monitor.clone())
    };
    let (mut effective_sx, mut effective_sy) = if grabbed_layer_surface_monitor.is_some() {
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
    let mut desktop_hover = false;
    let mut hover_focus_blocked = false;
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
            st.monitor_for_screen(effective_sx, effective_sy)
                .unwrap_or_else(|| st.interaction_monitor().to_string())
        }
    };
    if !grabbed_layer_surface_active {
        (effective_sx, effective_sy) =
            clamp_screen_to_monitor(st, target_monitor.as_str(), effective_sx, effective_sy);
    }
    if !grabbed_layer_surface_active {
        st.set_interaction_monitor(target_monitor.as_str());
        let _ = st.activate_monitor(target_monitor.as_str());
    }
    let (local_w, local_h, local_sx, local_sy) =
        st.local_screen_in_monitor(target_monitor.as_str(), effective_sx, effective_sy);

    if let Some(pointer) = st.platform.seat.get_pointer() {
        let resize_preview = ctx.pointer_state.borrow().resize;
        let focus = if let Some(surface) = grabbed_layer_surface.clone() {
            grabbed_layer_surface_focus(st, &surface)
        } else if let Some(surface) = locked_surface.clone() {
            Some((surface, pointer.current_location()))
        } else {
            pointer_focus_for_screen(
                st,
                local_w,
                local_h,
                local_sx,
                local_sy,
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
                (local_sx as f64, local_sy as f64).into()
            } else {
                let cam_scale = st.camera_render_scale() as f64;
                (local_sx as f64 / cam_scale, local_sy as f64 / cam_scale).into()
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
    let p = screen_to_world(st, local_w, local_h, local_sx, local_sy);
    let drag_mod_ok = modifier_active(&mods, st.runtime.tuning.keybinds.modifier)
        || matches!(
            active_pointer_binding(st, &mods, 0x110),
            Some(PointerBindingAction::MoveWindow | PointerBindingAction::FieldJump)
        );

    let mut ps = ctx.pointer_state.borrow_mut();
    let pointer_world = p;
    ps.world = pointer_world;
    ps.screen = (effective_sx, effective_sy);
    ps.workspace_size = (local_w, local_h);
    st.input.interaction_state.last_pointer_screen_global = Some((effective_sx, effective_sy));

    if handle_screenshot_pointer_motion(
        st,
        ctx,
        target_monitor.as_str(),
        local_w,
        local_h,
        local_sx,
        local_sy,
        effective_sx,
        effective_sy,
        now,
    ) {
        return;
    }

    let prompt_monitor = st.model.monitor_state.current_monitor.clone();
    if crate::compositor::clusters::system::cluster_system_controller(&*st)
        .cluster_name_prompt_active_for_monitor(prompt_monitor.as_str())
    {
        let prompt_hit = if target_monitor == prompt_monitor {
            crate::overlay::cluster_naming_dialog_hit_test(st, local_w, local_h, local_sx, local_sy)
        } else {
            None
        };
        st.input.interaction_state.overlay_hover_target = None;
        st.input.interaction_state.pending_core_hover = None;
        ps.hover_node = None;
        ps.hover_started_at = None;
        if let Some(crate::overlay::ClusterNamingDialogHit::InputCaret(caret_char)) = prompt_hit {
            let _ = crate::compositor::clusters::system::cluster_system_controller(&mut *st)
                .drag_cluster_name_prompt_selection_for_monitor(
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

    maybe_begin_core_drag_from_pending_press(
        st,
        &mut ps,
        ctx.backend,
        local_w,
        local_h,
        effective_sx,
        effective_sy,
        local_sx,
        local_sy,
        pointer_world,
    );
    maybe_begin_titlebar_drag_from_pending_press(
        st,
        &mut ps,
        ctx.backend,
        local_w,
        local_h,
        effective_sx,
        effective_sy,
        local_sx,
        local_sy,
        pointer_world,
    );

    if let Some(bloom_drag) = ps.bloom_drag.clone() {
        let (_, _, bloom_local_sx, bloom_local_sy) =
            st.local_screen_in_monitor(bloom_drag.monitor.as_str(), effective_sx, effective_sy);
        let pointer_screen = halley_core::field::Vec2 {
            x: bloom_local_sx,
            y: bloom_local_sy,
        };
        let slot_screen = halley_core::field::Vec2 {
            x: bloom_drag.slot_screen.0,
            y: bloom_drag.slot_screen.1,
        };
        let raw_offset = halley_core::field::Vec2 {
            x: pointer_screen.x - slot_screen.x,
            y: pointer_screen.y - slot_screen.y,
        };
        let pull_dist = raw_offset.x.hypot(raw_offset.y);
        let slop_px = crate::compositor::interaction::state::bloom_pull_slop_px();
        let display_offset =
            crate::compositor::interaction::state::bloom_pull_constrained_offset(raw_offset);
        let now_ms = st.now_ms(now);
        let mut should_detach = false;
        if let Some(preview) = st.input.interaction_state.bloom_pull_preview.as_mut() {
            let outward_axis = halley_core::field::Vec2 {
                x: preview.slot_screen.x - preview.core_screen.x,
                y: preview.slot_screen.y - preview.core_screen.y,
            };
            let outward_len = outward_axis.x.hypot(outward_axis.y);
            let outward_pull = if outward_len > 0.001 {
                (raw_offset.x * (outward_axis.x / outward_len)
                    + raw_offset.y * (outward_axis.y / outward_len))
                    .max(0.0)
            } else {
                pull_dist
            };
            preview.pointer_screen = pointer_screen;
            preview.display_offset = display_offset;
            match preview.phase.clone() {
                crate::compositor::interaction::state::BloomPullPhase::Pressed => {
                    preview.hold_progress = 0.0;
                    if outward_pull >= slop_px {
                        preview.phase =
                            crate::compositor::interaction::state::BloomPullPhase::Tethered {
                                started_at_ms: now_ms,
                            };
                    }
                }
                crate::compositor::interaction::state::BloomPullPhase::Tethered {
                    started_at_ms,
                } => {
                    if outward_pull < slop_px * 0.75 {
                        preview.phase =
                            crate::compositor::interaction::state::BloomPullPhase::Pressed;
                        preview.hold_progress = 0.0;
                    } else {
                        preview.hold_progress = (now_ms.saturating_sub(started_at_ms) as f32
                            / crate::compositor::interaction::state::bloom_detach_hold_ms().max(1)
                                as f32)
                            .clamp(0.0, 1.0);
                        should_detach = preview.hold_progress >= 1.0;
                    }
                }
                crate::compositor::interaction::state::BloomPullPhase::Snapback { .. } => {
                    preview.phase = crate::compositor::interaction::state::BloomPullPhase::Pressed;
                    preview.hold_progress = 0.0;
                }
            }
        }
        st.input.interaction_state.overlay_hover_target = None;
        ps.hover_node = None;
        ps.hover_started_at = None;
        if should_detach {
            ps.bloom_drag = None;
            st.input.interaction_state.bloom_pull_preview = None;
            ps.preview_block_until = Some(now + Duration::from_millis(500));
            detach_bloom_drag_into_pointer_drag(
                st,
                &mut ps,
                ctx.backend,
                bloom_drag,
                effective_sx,
                effective_sy,
            );
            ctx.backend.request_redraw();
        } else {
            st.request_maintenance();
        }
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
                    screen_local: (local_sx, local_sy),
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
        if ps.drag.is_none() && ps.resize.is_none() {
            let queue_hover = crate::overlay::cluster_overflow_icon_hit_test(
                &crate::overlay::OverlayView::from_halley(st),
                monitor.as_str(),
                local_sx,
                local_sy,
                now_ms,
            );
            if local_sx >= local_w as f32 - Halley::CLUSTER_OVERFLOW_REVEAL_EDGE_PX {
                st.reveal_cluster_overflow_for_monitor(monitor.as_str(), now_ms);
            } else if let Some(rect) = st.cluster_overflow_rect_for_monitor(monitor.as_str()) {
                let inside = local_sx >= rect.x
                    && local_sx <= rect.x + rect.w
                    && local_sy >= rect.y
                    && local_sy <= rect.y + rect.h;
                if inside {
                    st.reveal_cluster_overflow_for_monitor(monitor.as_str(), now_ms);
                }
            }
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
                    screen_anchor: (local_sx.round() as i32, local_sy.round() as i32),
                    prefer_left: true,
                }
            });
            crate::compositor::carry::system::set_drag_authority_node(st, None);
            ps.drag = None;
            ps.resize = None;
            if queue_hover.is_some() {
                ctx.backend.request_redraw();
            }
            return;
        }
    }
    st.input.interaction_state.cluster_overflow_drag_preview = None;
    st.input.interaction_state.overlay_hover_target = None;
    crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);

    let _ = handle_drag_motion(
        st,
        ctx.backend,
        &mods,
        &mut ps,
        drag_mod_ok,
        target_monitor.as_str(),
        local_w,
        local_h,
        local_sx,
        local_sy,
        pointer_world,
        now,
    );

    if handle_resize_motion(
        st,
        &mut ps,
        local_w,
        local_h,
        local_sx,
        local_sy,
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
        let dx_px = effective_sx - lsx;
        let dy_px = effective_sy - lsy;
        let camera = camera_controller(&*st).view_size();
        let dx_world = dx_px * camera.x.max(1.0) / (local_w as f32).max(1.0);
        let dy_world = dy_px * camera.y.max(1.0) / (local_h as f32).max(1.0);
        let now = Instant::now();
        st.note_pan_activity(now);
        camera_controller(&mut *st).pan_target(halley_core::field::Vec2 {
            x: -dx_world,
            y: -dy_world,
        });
        st.note_pan_viewport_change(now);
        ps.pan_last_screen = (effective_sx, effective_sy);
        ctx.backend.request_redraw();
    }

    let bloom_hover = if ps.drag.is_none() && ps.resize.is_none() && !ps.panning {
        crate::overlay::bloom_token_hit_test(
            st,
            local_w,
            local_h,
            target_monitor.as_str(),
            local_sx,
            local_sy,
        )
        .map(|layout| {
            (
                layout.member_id,
                crate::compositor::interaction::state::OverlayHoverTarget {
                    node_id: layout.member_id,
                    monitor: target_monitor.clone(),
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
            pick_hit_node_at(st, local_w, local_h, local_sx, local_sy, now, ps.resize)
        } else {
            None
        };

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

    apply_hover_focus_mode(st, hover_hit, hover_focus_blocked, now);

    if next_hover != ps.hover_node {
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
                    let monitor = target_monitor.clone();
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
        && desktop_hover
        && st.input.interaction_state.overlay_hover_target.is_none()
    {
        crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
    }

    ctx.backend.request_redraw();
}

pub(super) fn cluster_join_dwell_ms(st: &Halley) -> u64 {
    st.runtime.tuning.cluster_dwell_ms
}

pub(super) fn update_cluster_join_candidate(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    monitor: &str,
    desired_center: halley_core::field::Vec2,
    now: Instant,
) -> bool {
    if st
        .model
        .field
        .cluster_id_for_member_public(node_id)
        .is_some()
    {
        st.input.interaction_state.cluster_join_candidate = None;
        return false;
    }
    let Some(node) = st.model.field.node(node_id) else {
        st.input.interaction_state.cluster_join_candidate = None;
        return false;
    };
    if node.kind != halley_core::field::NodeKind::Surface {
        st.input.interaction_state.cluster_join_candidate = None;
        return false;
    }

    let mover_ext = if matches!(
        node.state,
        halley_core::field::NodeState::Active | halley_core::field::NodeState::Drifting
    ) {
        st.surface_window_collision_extents(node)
    } else {
        st.collision_extents_for_node(node)
    };
    let candidate = st.cluster_bloom_for_monitor(monitor).and_then(|open_cid| {
        let cluster = st.model.field.cluster(open_cid)?;
        if !cluster.is_collapsed() {
            return None;
        }
        let core_id = cluster.core?;
        let core = st.model.field.node(core_id)?;
        let core_monitor = st
            .model
            .monitor_state
            .node_monitor
            .get(&core_id)
            .map(String::as_str)
            .unwrap_or(monitor);
        if core_monitor != monitor {
            return None;
        }
        let core_ext = st.collision_extents_for_node(core);
        let gap = st.non_overlap_gap_world();
        let mover_left = desired_center.x - mover_ext.left;
        let mover_right = desired_center.x + mover_ext.right;
        let mover_top = desired_center.y - mover_ext.top;
        let mover_bottom = desired_center.y + mover_ext.bottom;
        let core_left = core.pos.x - core_ext.left - gap;
        let core_right = core.pos.x + core_ext.right + gap;
        let core_top = core.pos.y - core_ext.top - gap;
        let core_bottom = core.pos.y + core_ext.bottom + gap;
        let touching_gap = mover_right >= core_left
            && mover_left <= core_right
            && mover_bottom >= core_top
            && mover_top <= core_bottom;
        touching_gap.then_some(open_cid)
    });

    let Some(cluster_id) = candidate else {
        st.input.interaction_state.cluster_join_candidate = None;
        return false;
    };
    let now_ms = st.now_ms(now);
    let keep_started_at = st
        .input
        .interaction_state
        .cluster_join_candidate
        .as_ref()
        .filter(|existing| {
            existing.cluster_id == cluster_id
                && existing.node_id == node_id
                && existing.monitor == monitor
        })
        .map(|existing| existing.started_at_ms)
        .unwrap_or(now_ms);
    let dwell_ms = cluster_join_dwell_ms(st);
    st.input.interaction_state.cluster_join_candidate = Some(
        crate::compositor::interaction::state::ClusterJoinCandidate {
            cluster_id,
            node_id,
            monitor: monitor.to_string(),
            started_at_ms: keep_started_at,
            ready: now_ms.saturating_sub(keep_started_at) >= dwell_ms,
        },
    );
    false
}

pub(super) fn maybe_begin_core_drag_from_pending_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &impl BackendView,
    local_w: i32,
    local_h: i32,
    effective_sx: f32,
    effective_sy: f32,
    local_sx: f32,
    local_sy: f32,
    pointer_world: halley_core::field::Vec2,
) {
    if let Some(pending_press) = st.input.interaction_state.pending_core_press.clone() {
        let dx = effective_sx - pending_press.press_global_sx;
        let dy = effective_sy - pending_press.press_global_sy;
        const CORE_CLICK_DRAG_THRESHOLD_PX: f32 = 8.0;
        if dx.hypot(dy) >= CORE_CLICK_DRAG_THRESHOLD_PX {
            st.input.interaction_state.pending_core_press = None;
            if st.model.field.node(pending_press.node_id).is_some() {
                begin_drag(
                    st,
                    ps,
                    backend,
                    HitNode {
                        node_id: pending_press.node_id,
                        on_titlebar: true,
                        is_core: true,
                    },
                    ButtonFrame {
                        ws_w: local_w,
                        ws_h: local_h,
                        global_sx: effective_sx,
                        global_sy: effective_sy,
                        sx: local_sx,
                        sy: local_sy,
                        world_now: pointer_world,
                        workspace_active: false,
                    },
                    pointer_world,
                    false,
                    false,
                );
                backend.request_redraw();
            }
        }
    }
}

pub(super) fn maybe_begin_titlebar_drag_from_pending_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &impl BackendView,
    local_w: i32,
    local_h: i32,
    effective_sx: f32,
    effective_sy: f32,
    local_sx: f32,
    local_sy: f32,
    pointer_world: halley_core::field::Vec2,
) {
    if let Some(pending_press) = st.input.interaction_state.pending_titlebar_press.clone() {
        if !ps.left_button_down {
            st.input.interaction_state.pending_titlebar_press = None;
            return;
        }
        let dx = effective_sx - pending_press.press_global_sx;
        let dy = effective_sy - pending_press.press_global_sy;
        const TITLEBAR_DRAG_THRESHOLD_PX: f32 = 8.0;
        if dx.hypot(dy) >= TITLEBAR_DRAG_THRESHOLD_PX {
            st.input.interaction_state.pending_titlebar_press = None;
            if node_is_pointer_draggable(st, pending_press.node_id) {
                begin_drag(
                    st,
                    ps,
                    backend,
                    HitNode {
                        node_id: pending_press.node_id,
                        on_titlebar: true,
                        is_core: false,
                    },
                    ButtonFrame {
                        ws_w: local_w,
                        ws_h: local_h,
                        global_sx: effective_sx,
                        global_sy: effective_sy,
                        sx: local_sx,
                        sy: local_sy,
                        world_now: pointer_world,
                        workspace_active: pending_press.workspace_active,
                    },
                    pointer_world,
                    false,
                    false,
                );
                backend.request_redraw();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_drag_motion(
    st: &mut Halley,
    backend: &impl BackendView,
    mods: &ModState,
    ps: &mut PointerState,
    drag_mod_ok: bool,
    target_monitor: &str,
    local_w: i32,
    local_h: i32,
    local_sx: f32,
    local_sy: f32,
    pointer_world: halley_core::field::Vec2,
    now: Instant,
) -> bool {
    let Some(drag) = ps.drag else {
        st.input.interaction_state.cluster_join_candidate = None;
        return false;
    };

    let drag_allowed = !drag.requires_drag_modifier || drag_mod_ok;
    if ps.resize.is_some() || !drag_allowed {
        let joined = !drag_allowed && st.commit_ready_cluster_join_for_node(drag.node_id, now);
        crate::compositor::carry::system::set_drag_authority_node(st, None);
        crate::compositor::carry::system::end_carry_state_tracking(st, drag.node_id);
        ps.drag = None;
        st.input.interaction_state.active_drag = None;
        crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
        if joined {
            backend.request_redraw();
        }
        return joined;
    }

    let mut next_drag = drag;
    let drag_allow_monitor_transfer = super::button::active_pointer_binding(st, mods, 0x110)
        == Some(halley_config::PointerBindingAction::FieldJump);
    next_drag.allow_monitor_transfer = drag_allow_monitor_transfer;
    let dt = now
        .saturating_duration_since(next_drag.last_update_at)
        .as_secs_f32()
        .max(1.0 / 240.0);
    let raw_velocity = halley_core::field::Vec2 {
        x: (pointer_world.x - next_drag.last_pointer_world.x) / dt,
        y: (pointer_world.y - next_drag.last_pointer_world.y) / dt,
    };
    let max_drag_speed = 800.0f32;
    let clamp_axis = |v: f32| v.clamp(-max_drag_speed, max_drag_speed);
    next_drag.release_velocity = halley_core::field::Vec2 {
        x: next_drag.release_velocity.x * 0.35 + clamp_axis(raw_velocity.x) * 0.65,
        y: next_drag.release_velocity.y * 0.35 + clamp_axis(raw_velocity.y) * 0.65,
    };
    next_drag.last_pointer_world = pointer_world;
    next_drag.last_update_at = now;
    let desired_to = halley_core::field::Vec2 {
        x: pointer_world.x - next_drag.current_offset.x,
        y: pointer_world.y - next_drag.current_offset.y,
    };

    update_drag_edge_pan(
        st,
        drag.node_id,
        target_monitor,
        desired_to,
        dt,
        drag_allow_monitor_transfer,
        &mut next_drag,
    );

    let should_center = st.runtime.tuning.center_window_to_mouse
        && (!next_drag.center_latched
            || next_drag.current_offset.x.abs() > f32::EPSILON
            || next_drag.current_offset.y.abs() > f32::EPSILON);
    if should_center {
        next_drag.current_offset = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
        next_drag.center_latched = true;
    }
    st.input.interaction_state.drag_authority_velocity = next_drag.release_velocity;
    st.input.interaction_state.active_drag = Some(ActiveDragState {
        node_id: drag.node_id,
        allow_monitor_transfer: drag_allow_monitor_transfer,
        edge_pan_eligible: next_drag.edge_pan_eligible,
        current_offset: next_drag.current_offset,
        pointer_monitor: target_monitor.to_string(),
        pointer_workspace_size: (local_w, local_h),
        pointer_screen_local: (local_sx, local_sy),
        edge_pan_x: next_drag.edge_pan_x,
        edge_pan_y: next_drag.edge_pan_y,
    });
    ps.drag = Some(next_drag);
    crate::compositor::interaction::pointer::set_cursor_override_icon(
        st,
        Some(smithay::input::pointer::CursorIcon::Grabbing),
    );
    let _ = update_cluster_join_candidate(st, drag.node_id, target_monitor, desired_to, now);
    backend.request_redraw();
    true
}

fn update_drag_edge_pan(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    target_monitor: &str,
    desired_to: halley_core::field::Vec2,
    dt: f32,
    drag_allow_monitor_transfer: bool,
    next_drag: &mut crate::compositor::interaction::DragCtx,
) {
    if !drag_allow_monitor_transfer
        && next_drag.edge_pan_eligible
        && let Some(owner_monitor) = st
            .model
            .monitor_state
            .node_monitor
            .get(&node_id)
            .cloned()
            .or_else(|| Some(target_monitor.to_string()))
    {
        if let Some((clamped_center, edge_contact)) =
            crate::compositor::interaction::state::dragged_node_edge_pan_clamp(
                st,
                owner_monitor.as_str(),
                node_id,
                desired_to,
                halley_core::field::Vec2 {
                    x: next_drag.edge_pan_x.sign(),
                    y: next_drag.edge_pan_y.sign(),
                },
            )
        {
            const EDGE_PAN_PRESSURE_THRESHOLD: f32 = 56.0;
            const EDGE_PAN_PRESSURE_DECAY_PER_SEC: f32 = 44.0;
            const EDGE_PAN_PRESSURE_BUILD_PER_SEC: f32 = 86.0;
            const EDGE_PAN_PRESSURE_DEPTH_NORM: f32 = 18.0;
            const EDGE_PAN_RELEASE_DISTANCE: f32 = 24.0;

            next_drag.edge_pan_pressure.x =
                (next_drag.edge_pan_pressure.x - EDGE_PAN_PRESSURE_DECAY_PER_SEC * dt).max(0.0);
            next_drag.edge_pan_pressure.y =
                (next_drag.edge_pan_pressure.y - EDGE_PAN_PRESSURE_DECAY_PER_SEC * dt).max(0.0);

            if edge_contact.x < 0.0 {
                let depth = (clamped_center.x - desired_to.x).max(0.0);
                let build = (depth / EDGE_PAN_PRESSURE_DEPTH_NORM).clamp(0.0, 1.25);
                next_drag.edge_pan_pressure.x += EDGE_PAN_PRESSURE_BUILD_PER_SEC * build * dt;
            } else if edge_contact.x > 0.0 {
                let depth = (desired_to.x - clamped_center.x).max(0.0);
                let build = (depth / EDGE_PAN_PRESSURE_DEPTH_NORM).clamp(0.0, 1.25);
                next_drag.edge_pan_pressure.x += EDGE_PAN_PRESSURE_BUILD_PER_SEC * build * dt;
            } else {
                next_drag.edge_pan_pressure.x = 0.0;
            }

            if edge_contact.y < 0.0 {
                let depth = (clamped_center.y - desired_to.y).max(0.0);
                let build = (depth / EDGE_PAN_PRESSURE_DEPTH_NORM).clamp(0.0, 1.25);
                next_drag.edge_pan_pressure.y += EDGE_PAN_PRESSURE_BUILD_PER_SEC * build * dt;
            } else if edge_contact.y > 0.0 {
                let depth = (desired_to.y - clamped_center.y).max(0.0);
                let build = (depth / EDGE_PAN_PRESSURE_DEPTH_NORM).clamp(0.0, 1.25);
                next_drag.edge_pan_pressure.y += EDGE_PAN_PRESSURE_BUILD_PER_SEC * build * dt;
            } else {
                next_drag.edge_pan_pressure.y = 0.0;
            }

            next_drag.edge_pan_x = match next_drag.edge_pan_x {
                DragAxisMode::Free => {
                    if edge_contact.x < 0.0
                        && next_drag.edge_pan_pressure.x >= EDGE_PAN_PRESSURE_THRESHOLD
                    {
                        DragAxisMode::EdgePanNeg
                    } else if edge_contact.x > 0.0
                        && next_drag.edge_pan_pressure.x >= EDGE_PAN_PRESSURE_THRESHOLD
                    {
                        DragAxisMode::EdgePanPos
                    } else {
                        DragAxisMode::Free
                    }
                }
                DragAxisMode::EdgePanNeg => {
                    if desired_to.x > clamped_center.x + EDGE_PAN_RELEASE_DISTANCE {
                        next_drag.edge_pan_pressure.x = 0.0;
                        DragAxisMode::Free
                    } else {
                        DragAxisMode::EdgePanNeg
                    }
                }
                DragAxisMode::EdgePanPos => {
                    if desired_to.x < clamped_center.x - EDGE_PAN_RELEASE_DISTANCE {
                        next_drag.edge_pan_pressure.x = 0.0;
                        DragAxisMode::Free
                    } else {
                        DragAxisMode::EdgePanPos
                    }
                }
            };
            next_drag.edge_pan_y = match next_drag.edge_pan_y {
                DragAxisMode::Free => {
                    if edge_contact.y < 0.0
                        && next_drag.edge_pan_pressure.y >= EDGE_PAN_PRESSURE_THRESHOLD
                    {
                        DragAxisMode::EdgePanNeg
                    } else if edge_contact.y > 0.0
                        && next_drag.edge_pan_pressure.y >= EDGE_PAN_PRESSURE_THRESHOLD
                    {
                        DragAxisMode::EdgePanPos
                    } else {
                        DragAxisMode::Free
                    }
                }
                DragAxisMode::EdgePanNeg => {
                    if desired_to.y > clamped_center.y + EDGE_PAN_RELEASE_DISTANCE {
                        next_drag.edge_pan_pressure.y = 0.0;
                        DragAxisMode::Free
                    } else {
                        DragAxisMode::EdgePanNeg
                    }
                }
                DragAxisMode::EdgePanPos => {
                    if desired_to.y < clamped_center.y - EDGE_PAN_RELEASE_DISTANCE {
                        next_drag.edge_pan_pressure.y = 0.0;
                        DragAxisMode::Free
                    } else {
                        DragAxisMode::EdgePanPos
                    }
                }
            };

            let edge_pan_direction = halley_core::field::Vec2 {
                x: next_drag.edge_pan_x.sign(),
                y: next_drag.edge_pan_y.sign(),
            };
            let edge_pan_active = edge_pan_direction.x != 0.0 || edge_pan_direction.y != 0.0;
            let indicator_direction = if edge_pan_active {
                edge_pan_direction
            } else {
                edge_contact
            };

            st.input.interaction_state.grabbed_edge_pan_active = edge_pan_active;
            st.input.interaction_state.grabbed_edge_pan_direction = indicator_direction;
            st.input.interaction_state.grabbed_edge_pan_pressure = next_drag.edge_pan_pressure;
            st.input.interaction_state.grabbed_edge_pan_monitor = ((indicator_direction.x != 0.0
                || indicator_direction.y != 0.0)
                && (next_drag.edge_pan_pressure.x > 0.0 || next_drag.edge_pan_pressure.y > 0.0))
                .then(|| owner_monitor.clone());
            return;
        }
    }

    st.input.interaction_state.grabbed_edge_pan_active = false;
    st.input.interaction_state.grabbed_edge_pan_direction =
        halley_core::field::Vec2 { x: 0.0, y: 0.0 };
    st.input.interaction_state.grabbed_edge_pan_pressure =
        halley_core::field::Vec2 { x: 0.0, y: 0.0 };
    st.input.interaction_state.grabbed_edge_pan_monitor = None;
    next_drag.edge_pan_x = DragAxisMode::Free;
    next_drag.edge_pan_y = DragAxisMode::Free;
    next_drag.edge_pan_pressure = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::reexports::wayland_server::Display;

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
    fn hover_focus_mode_focuses_hovered_surface() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.input.focus_mode = InputFocusMode::Hover;
        let mut st = Halley::new_for_test(&dh, tuning);

        let node_id = st.model.field.spawn_surface(
            "surface",
            halley_core::field::Vec2 { x: 100.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(node_id, "monitor_a");

        apply_hover_focus_mode(
            &mut st,
            Some(HitNode {
                node_id,
                on_titlebar: false,
                is_core: false,
            }),
            false,
            Instant::now(),
        );

        assert_eq!(
            st.model.focus_state.primary_interaction_focus,
            Some(node_id)
        );
    }

    #[test]
    fn click_focus_mode_keeps_hover_focus_disabled() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let node_id = st.model.field.spawn_surface(
            "surface",
            halley_core::field::Vec2 { x: 100.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(node_id, "monitor_a");

        apply_hover_focus_mode(
            &mut st,
            Some(HitNode {
                node_id,
                on_titlebar: false,
                is_core: false,
            }),
            false,
            Instant::now(),
        );

        assert_eq!(st.model.focus_state.primary_interaction_focus, None);
    }

    #[test]
    fn hover_focus_gate_disables_focus_follows_mouse_while_layer_shell_is_active() {
        assert!(!hover_focus_enabled(InputFocusMode::Hover, false, true));
        assert!(hover_focus_enabled(InputFocusMode::Hover, false, false));
    }
}
