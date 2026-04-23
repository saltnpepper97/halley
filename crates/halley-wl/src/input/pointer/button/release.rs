use std::time::Instant;

use crate::backend::interface::BackendView;
use crate::compositor::interaction::PointerState;
use crate::compositor::root::Halley;
use halley_config::PointerBindingAction;

use crate::input::pointer::motion::finish_pointer_drag;
use crate::input::pointer::resize::finalize_resize;

pub(crate) fn handle_button_release(
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
            st.input.interaction_state.pending_move_press = None;
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
            st.input.interaction_state.pending_move_press = None;
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
                    st.input.interaction_state.pending_move_press = None;
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

pub(crate) fn clear_pointer_activity(st: &mut Halley, ps: &mut PointerState) {
    if let Some(drag) = ps.drag {
        crate::compositor::carry::system::set_drag_authority_node(st, None);
        crate::compositor::carry::system::end_carry_state_tracking(st, drag.node_id);
    }
    crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
    st.input.interaction_state.active_drag = None;
    st.input.interaction_state.pending_core_press = None;
    st.input.interaction_state.pending_collapsed_node_press = None;
    st.input.interaction_state.pending_move_press = None;
    st.input.interaction_state.cluster_overflow_drag_preview = None;
    crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
    ps.drag = None;
    ps.overflow_drag = None;
    ps.resize = None;
    ps.panning = false;
    ps.pan_monitor = None;
}

pub(crate) fn collapse_bloom_for_core_if_open(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
) -> bool {
    let Some(cid) = st.model.field.cluster_id_for_core_public(node_id) else {
        return false;
    };
    let monitor = st.monitor_for_node_or_current(node_id);
    if st.cluster_bloom_for_monitor(monitor.as_str()) != Some(cid) {
        return false;
    }
    st.close_cluster_bloom_for_monitor(monitor.as_str())
}

pub(crate) fn restore_fullscreen_click_focus(
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
        .unwrap_or_else(|| st.monitor_for_node_or_current(node_id));

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
