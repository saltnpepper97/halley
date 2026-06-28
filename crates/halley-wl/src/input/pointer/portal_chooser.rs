use std::time::Instant;

use smithay::backend::input::ButtonState;
use smithay::input::pointer::CursorIcon;

use crate::backend::interface::BackendView;
use crate::compositor::interaction::PointerState;
use crate::compositor::portal_chooser::{
    PortalChooserPhase, activate_portal_chooser, hover_portal_chooser_item,
    hover_portal_chooser_monitor, hover_portal_chooser_window, pick_portal_screen,
    pick_portal_window_at, portal_chooser_active, portal_chooser_entries,
    update_portal_chooser_monitor,
};
use crate::compositor::root::Halley;
use crate::input::ctx::InputCtx;

use super::button::{ButtonFrame, handle_button_release};

pub(super) fn handle_portal_chooser_pointer_button<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    ps: &mut PointerState,
    button_code: u32,
    button_state: ButtonState,
    left: bool,
    _frame: ButtonFrame,
    target_monitor: &str,
    local_w: i32,
    local_h: i32,
    local_sx: f32,
    local_sy: f32,
    world_now: halley_core::field::Vec2,
) -> bool {
    if !portal_chooser_active(st) {
        return false;
    }

    ps.world = world_now;
    let phase = st
        .input
        .interaction_state
        .portal_chooser
        .as_ref()
        .map(|s| s.phase);
    if left && matches!(button_state, ButtonState::Pressed) {
        match phase {
            Some(PortalChooserPhase::Menu) => {
                let count = portal_chooser_entries(st).len();
                if let Some(idx) = crate::overlay::portal_chooser_menu_hit_test(
                    local_w, local_h, local_sx, local_sy, count,
                ) && portal_chooser_entries(st)[idx].enabled
                {
                    hover_portal_chooser_item(st, Some(idx));
                    let _ = activate_portal_chooser(st, Instant::now());
                }
            }
            Some(PortalChooserPhase::ScreenPick) => {
                let _ = pick_portal_screen(st, target_monitor, Instant::now());
            }
            Some(PortalChooserPhase::WindowPick) => {
                let _ = pick_portal_window_at(
                    st,
                    target_monitor,
                    local_w,
                    local_h,
                    local_sx,
                    local_sy,
                    Instant::now(),
                );
            }
            _ => {}
        }
    }
    if matches!(button_state, ButtonState::Released) {
        handle_button_release(st, ps, ctx.backend, button_code, None, world_now);
    }
    ctx.backend.request_redraw();
    true
}

pub(super) fn handle_portal_chooser_pointer_motion<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    target_monitor: &str,
    local_w: i32,
    local_h: i32,
    local_sx: f32,
    local_sy: f32,
) -> bool {
    if !portal_chooser_active(st) {
        return false;
    }
    let phase = st
        .input
        .interaction_state
        .portal_chooser
        .as_ref()
        .map(|s| s.phase);
    update_portal_chooser_monitor(st, target_monitor.to_string());
    if matches!(phase, Some(PortalChooserPhase::Menu)) {
        let count = portal_chooser_entries(st).len();
        let idx = crate::overlay::portal_chooser_menu_hit_test(
            local_w, local_h, local_sx, local_sy, count,
        );
        hover_portal_chooser_item(st, idx);
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(CursorIcon::Pointer),
        );
    } else if matches!(phase, Some(PortalChooserPhase::ScreenPick)) {
        hover_portal_chooser_monitor(st, target_monitor.to_string());
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(CursorIcon::Pointer),
        );
    } else {
        let now = Instant::now();
        let previous_monitor = st.begin_temporary_render_monitor(target_monitor);
        let hit =
            crate::spatial::pick_hit_node_at(st, local_w, local_h, local_sx, local_sy, now, None);
        st.end_temporary_render_monitor(previous_monitor);
        hover_portal_chooser_window(st, hit.map(|hit| hit.node_id));
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(CursorIcon::Pointer),
        );
    }
    ctx.backend.request_redraw();
    true
}
