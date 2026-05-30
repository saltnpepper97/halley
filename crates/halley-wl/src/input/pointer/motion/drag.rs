use std::time::{Duration, Instant};

use super::super::button::{ButtonFrame, active_pointer_binding};
use crate::backend::interface::BackendView;
use crate::compositor::interaction::state::ActiveDragState;
use crate::compositor::interaction::{DragAxisMode, DragCtx, HitNode, ModState, PointerState};
use crate::compositor::root::Halley;
use crate::compositor::surface::{
    is_active_stacking_workspace_member, node_blocks_interactive_transform,
};

pub(crate) fn node_is_pointer_draggable(st: &Halley, node_id: halley_core::field::NodeId) -> bool {
    if st.is_fullscreen_active(node_id) {
        return false;
    }
    if node_blocks_interactive_transform(st, node_id) {
        return false;
    }
    if crate::compositor::workspace::state::node_in_maximize_session(st, node_id) {
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

fn drag_edge_pan_eligible(
    st: &Halley,
    node_id: halley_core::field::NodeId,
    drag_monitor: &str,
    allow_monitor_transfer: bool,
) -> bool {
    !allow_monitor_transfer
        && st.model.field.node(node_id).is_some_and(|n| {
            n.kind == halley_core::field::NodeKind::Surface
                && n.state == halley_core::field::NodeState::Active
                && st.model.field.is_visible(node_id)
        })
        && crate::compositor::interaction::state::node_fully_visible_on_monitor(
            st,
            drag_monitor,
            node_id,
        )
        .unwrap_or(false)
}

fn drag_allows_monitor_transfer_for_mods(st: &Halley, mods: &ModState, drag: DragCtx) -> bool {
    if !drag.requires_drag_modifier {
        return drag.allow_monitor_transfer;
    }

    match active_pointer_binding(st, mods, 0x110) {
        Some(halley_config::PointerBindingAction::PanField) => false,
        Some(halley_config::PointerBindingAction::MoveWindow) => true,
        _ => drag.allow_monitor_transfer,
    }
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
    let now = Instant::now();
    st.input.interaction_state.pending_core_press = None;
    st.input.interaction_state.pending_core_click = None;
    st.input.interaction_state.pending_collapsed_node_press = None;
    st.input.interaction_state.pending_collapsed_node_click = None;
    let drag_monitor = st.monitor_for_node_or_current(hit.node_id);
    let edge_pan_eligible = drag_edge_pan_eligible(
        st,
        hit.node_id,
        drag_monitor.as_str(),
        allow_monitor_transfer,
    );
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
    if !node_is_pointer_draggable(st, hit.node_id) {
        return;
    }
    ps.drag = Some(drag_ctx);
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
        last_edge_pan_at: now,
    });
    crate::compositor::carry::system::set_drag_authority_node(st, Some(hit.node_id));
    crate::compositor::carry::system::begin_carry_state_tracking(st, hit.node_id);
    crate::compositor::interaction::pointer::set_cursor_override_icon(
        st,
        Some(smithay::input::pointer::CursorIcon::Grabbing),
    );
    if !hit.is_core {
        st.focus_pointer_target(hit.node_id, 30_000, now);
    }
    let to = halley_core::field::Vec2 {
        x: world_now.x - drag_ctx.current_offset.x,
        y: world_now.y - drag_ctx.current_offset.y,
    };
    let _ = st.carry_surface_non_overlap(hit.node_id, to, false);
    backend.request_redraw();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::interface::BackendView;
    use crate::compositor::interaction::PointerState;
    use crate::compositor::root::Halley;
    use smithay::reexports::wayland_server::Display;

    struct TestBackend;

    impl BackendView for TestBackend {
        fn window_size_i32(&self) -> (i32, i32) {
            (1600, 1200)
        }

        fn request_redraw(&self) {}
    }

    fn move_window_mods() -> ModState {
        ModState {
            alt_down: true,
            left_alt_down: true,
            ..ModState::default()
        }
    }

    fn pan_field_mods() -> ModState {
        ModState {
            alt_down: true,
            left_alt_down: true,
            shift_down: true,
            left_shift_down: true,
            ..ModState::default()
        }
    }

    #[test]
    fn move_window_drag_does_not_enable_edge_pan() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = st.model.field.spawn_surface(
            "dragged",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        let hit = HitNode {
            node_id: id,
            move_surface: false,
            is_core: false,
        };
        let world_now = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
        let frame = ButtonFrame {
            global_sx: 200.0,
            global_sy: 120.0,
            sx: 200.0,
            sy: 120.0,
            ws_w: 1600,
            ws_h: 1200,
            world_now,
            workspace_active: true,
        };
        let mut ps = PointerState::default();

        begin_drag(
            &mut st,
            &mut ps,
            &TestBackend,
            hit,
            frame,
            world_now,
            true,
            false,
        );

        assert!(ps.drag.is_some_and(|drag| !drag.edge_pan_eligible));
        assert!(!st.input.interaction_state.grabbed_edge_pan_active);
    }

    #[test]
    fn shift_pressed_during_move_window_drag_enables_edge_pan_when_visible() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = st.model.field.spawn_surface(
            "dragged",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        let hit = HitNode {
            node_id: id,
            move_surface: false,
            is_core: false,
        };
        let world_now = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
        let frame = ButtonFrame {
            global_sx: 200.0,
            global_sy: 120.0,
            sx: 200.0,
            sy: 120.0,
            ws_w: 1600,
            ws_h: 1200,
            world_now,
            workspace_active: true,
        };
        let mut ps = PointerState::default();
        begin_drag(
            &mut st,
            &mut ps,
            &TestBackend,
            hit,
            frame,
            world_now,
            true,
            true,
        );

        let monitor = st.monitor_for_node_or_current(id);
        assert!(handle_drag_motion(
            &mut st,
            &TestBackend,
            &pan_field_mods(),
            &mut ps,
            true,
            monitor.as_str(),
            1600,
            1200,
            200.0,
            120.0,
            world_now,
            Instant::now(),
        ));

        let drag = ps.drag.expect("active drag");
        assert!(!drag.allow_monitor_transfer);
        assert!(drag.edge_pan_eligible);
        assert!(
            st.input
                .interaction_state
                .active_drag
                .as_ref()
                .is_some_and(|drag| !drag.allow_monitor_transfer && drag.edge_pan_eligible)
        );
    }

    #[test]
    fn releasing_shift_during_pan_field_drag_returns_to_move_window_mode() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = st.model.field.spawn_surface(
            "dragged",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        let hit = HitNode {
            node_id: id,
            move_surface: false,
            is_core: false,
        };
        let world_now = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
        let frame = ButtonFrame {
            global_sx: 200.0,
            global_sy: 120.0,
            sx: 200.0,
            sy: 120.0,
            ws_w: 1600,
            ws_h: 1200,
            world_now,
            workspace_active: true,
        };
        let mut ps = PointerState::default();
        begin_drag(
            &mut st,
            &mut ps,
            &TestBackend,
            hit,
            frame,
            world_now,
            false,
            true,
        );

        let monitor = st.monitor_for_node_or_current(id);
        assert!(handle_drag_motion(
            &mut st,
            &TestBackend,
            &move_window_mods(),
            &mut ps,
            true,
            monitor.as_str(),
            1600,
            1200,
            200.0,
            120.0,
            world_now,
            Instant::now(),
        ));

        let drag = ps.drag.expect("active drag");
        assert!(drag.allow_monitor_transfer);
        assert!(!drag.edge_pan_eligible);
        assert!(
            st.input
                .interaction_state
                .active_drag
                .as_ref()
                .is_some_and(|drag| drag.allow_monitor_transfer && !drag.edge_pan_eligible)
        );
    }

    #[test]
    fn partially_offscreen_active_surface_drag_does_not_enable_edge_pan() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = st.model.field.spawn_surface(
            "dragged",
            halley_core::field::Vec2 { x: 930.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        let hit = HitNode {
            node_id: id,
            move_surface: false,
            is_core: false,
        };
        let world_now = halley_core::field::Vec2 { x: 930.0, y: 0.0 };
        let frame = ButtonFrame {
            global_sx: 200.0,
            global_sy: 120.0,
            sx: 200.0,
            sy: 120.0,
            ws_w: 1600,
            ws_h: 1200,
            world_now,
            workspace_active: true,
        };
        let mut ps = PointerState::default();

        begin_drag(
            &mut st,
            &mut ps,
            &TestBackend,
            hit,
            frame,
            world_now,
            false,
            false,
        );

        assert!(ps.drag.is_some_and(|drag| !drag.edge_pan_eligible));
        assert!(!st.input.interaction_state.grabbed_edge_pan_active);
    }

    #[test]
    fn pinned_surface_drag_is_allowed_as_deliberate_anchor_move() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = st.model.field.spawn_surface(
            "dragged",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        assert!(st.set_node_user_pinned(id, true));
        let hit = HitNode {
            node_id: id,
            move_surface: false,
            is_core: false,
        };
        let world_now = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
        let frame = ButtonFrame {
            global_sx: 200.0,
            global_sy: 120.0,
            sx: 200.0,
            sy: 120.0,
            ws_w: 1600,
            ws_h: 1200,
            world_now,
            workspace_active: true,
        };
        let mut ps = PointerState::default();

        begin_drag(
            &mut st,
            &mut ps,
            &TestBackend,
            hit,
            frame,
            world_now,
            false,
            false,
        );

        assert!(ps.drag.is_some());
        assert!(st.input.interaction_state.active_drag.is_some());
        assert_eq!(st.input.interaction_state.drag_authority_node, Some(id));
        assert!(st.model.field.node(id).expect("node").pinned);
        assert!(st.carry_surface_non_overlap(
            id,
            halley_core::field::Vec2 { x: 40.0, y: 20.0 },
            false,
        ));
        assert_eq!(
            st.model.field.node(id).expect("node").pos,
            halley_core::field::Vec2 { x: 40.0, y: 20.0 }
        );
        assert!(st.model.field.node(id).expect("node").pinned);

        let drag = ps.drag.expect("drag in progress");
        finish_pointer_drag(
            &mut st,
            &mut ps,
            id,
            drag.started_active,
            world_now,
            Instant::now(),
        );

        assert!(st.node_user_pinned(id));
        assert!(st.model.field.node(id).expect("node").pinned);
    }

    #[test]
    fn maximized_session_surface_is_not_pointer_draggable() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.animations.maximize.enabled = false;
        let mut st = Halley::new_for_test(&dh, tuning);
        let id = st.model.field.spawn_surface(
            "dragged",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        st.assign_node_to_current_monitor(id);
        let monitor = st.focused_monitor().to_string();

        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut st,
                id,
                Instant::now(),
                monitor.as_str(),
            )
        );

        assert!(!node_is_pointer_draggable(&st, id));
    }
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
    st.resolve_surface_overlap();
    ps.preview_block_until = Some(now + Duration::from_millis(360));
    ps.drag = None;
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
    let drag_allow_monitor_transfer = drag_allows_monitor_transfer_for_mods(st, mods, next_drag);
    next_drag.allow_monitor_transfer = drag_allow_monitor_transfer;
    let edge_pan_monitor = st.monitor_for_node_or_current(drag.node_id);
    next_drag.edge_pan_eligible = drag_edge_pan_eligible(
        st,
        drag.node_id,
        edge_pan_monitor.as_str(),
        drag_allow_monitor_transfer,
    );
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
        last_edge_pan_at: st
            .input
            .interaction_state
            .active_drag
            .as_ref()
            .map(|drag| drag.last_edge_pan_at)
            .unwrap_or(now),
    });
    ps.drag = Some(next_drag);
    crate::compositor::interaction::pointer::set_cursor_override_icon(
        st,
        Some(smithay::input::pointer::CursorIcon::Grabbing),
    );
    let _ = super::cluster::update_cluster_join_candidate(
        st,
        drag.node_id,
        target_monitor,
        desired_to,
        now,
    );
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
