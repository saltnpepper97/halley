use super::super::button::ButtonFrame;
use super::drag::{begin_drag, node_is_pointer_draggable};
use crate::backend::interface::BackendView;
use crate::compositor::interaction::{HitNode, PointerState};
use crate::compositor::root::Halley;

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
                    false,
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
                backend.request_redraw();
            }
        }
    }
}
