use super::super::button::ButtonFrame;
use super::drag::{begin_drag, node_is_pointer_draggable};
use crate::backend::interface::BackendView;
use crate::compositor::interaction::{HitNode, PointerState};
use crate::compositor::root::Halley;

fn apply_restore_drag_offset(
    st: &mut Halley,
    ps: &mut PointerState,
    node_id: halley_core::field::NodeId,
    pointer_world: halley_core::field::Vec2,
    restore_drag_offset: halley_core::field::Vec2,
) {
    if st.runtime.tuning.center_window_to_mouse {
        return;
    }
    let Some(drag) = ps.drag.as_mut().filter(|drag| drag.node_id == node_id) else {
        return;
    };

    drag.current_offset = restore_drag_offset;
    drag.center_latched = false;
    if let Some(active_drag) = st
        .input
        .interaction_state
        .active_drag
        .as_mut()
        .filter(|drag| drag.node_id == node_id)
    {
        active_drag.current_offset = restore_drag_offset;
    }
    let to = halley_core::field::Vec2 {
        x: pointer_world.x - restore_drag_offset.x,
        y: pointer_world.y - restore_drag_offset.y,
    };
    let _ = st.carry_surface_non_overlap(node_id, to, false);
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
                        move_surface: true,
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
                    true,
                    false,
                );
                backend.request_redraw();
            }
        }
    }
}

pub(super) fn maybe_begin_move_drag_from_pending_press(
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
    if let Some(pending_press) = st.input.interaction_state.pending_move_press.clone() {
        if !ps.left_button_down {
            st.input.interaction_state.pending_move_press = None;
            return;
        }
        let dx = effective_sx - pending_press.press_global_sx;
        let dy = effective_sy - pending_press.press_global_sy;
        const MOVE_DRAG_THRESHOLD_PX: f32 = 8.0;
        if dx.hypot(dy) >= MOVE_DRAG_THRESHOLD_PX {
            st.input.interaction_state.pending_move_press = None;
            if node_is_pointer_draggable(st, pending_press.node_id) {
                begin_drag(
                    st,
                    ps,
                    backend,
                    HitNode {
                        node_id: pending_press.node_id,
                        move_surface: true,
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
                if let Some(restore_drag_offset) = pending_press.restore_drag_offset {
                    apply_restore_drag_offset(
                        st,
                        ps,
                        pending_press.node_id,
                        pointer_world,
                        restore_drag_offset,
                    );
                }
                backend.request_redraw();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::interface::BackendView;
    use crate::compositor::interaction::PointerState;
    use crate::compositor::interaction::state::PendingMovePress;
    use halley_core::field::Vec2;
    use smithay::reexports::wayland_server::Display;

    struct TestBackend;

    impl BackendView for TestBackend {
        fn window_size_i32(&self) -> (i32, i32) {
            (1600, 1200)
        }

        fn request_redraw(&self) {}
    }

    #[test]
    fn restored_maximize_pending_move_anchors_drag_under_pointer() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let node_id = st.model.field.spawn_surface(
            "dragged",
            Vec2 {
                x: -1200.0,
                y: -900.0,
            },
            Vec2 { x: 400.0, y: 260.0 },
        );
        st.assign_node_to_current_monitor(node_id);
        let mut ps = PointerState {
            left_button_down: true,
            ..PointerState::default()
        };
        let pointer_world = Vec2 { x: 500.0, y: 300.0 };
        let restore_drag_offset = Vec2 {
            x: 120.0,
            y: -115.0,
        };
        st.input.interaction_state.pending_move_press = Some(PendingMovePress {
            node_id,
            press_global_sx: 100.0,
            press_global_sy: 100.0,
            workspace_active: false,
            restore_drag_offset: Some(restore_drag_offset),
        });

        maybe_begin_move_drag_from_pending_press(
            &mut st,
            &mut ps,
            &TestBackend,
            1600,
            1200,
            112.0,
            100.0,
            112.0,
            100.0,
            pointer_world,
        );

        let drag = ps.drag.expect("active drag");
        assert_eq!(drag.current_offset, restore_drag_offset);
        assert!(
            st.input
                .interaction_state
                .active_drag
                .as_ref()
                .is_some_and(|active_drag| active_drag.current_offset == restore_drag_offset)
        );
        let pos = st.model.field.node(node_id).expect("node").pos;
        assert!((pos.x - (pointer_world.x - restore_drag_offset.x)).abs() <= 0.5);
        assert!((pos.y - (pointer_world.y - restore_drag_offset.y)).abs() <= 0.5);
    }
}
