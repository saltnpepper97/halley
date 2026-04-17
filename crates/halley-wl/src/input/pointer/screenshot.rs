use std::time::Instant;

use smithay::backend::input::ButtonState;
use smithay::input::pointer::CursorIcon;

use crate::backend::interface::BackendView;
use crate::compositor::interaction::PointerState;
use crate::compositor::root::Halley;
use crate::compositor::screenshot::screenshot_controller;
use crate::input::ctx::InputCtx;

use super::button::{ButtonFrame, handle_button_release};

pub(super) fn handle_screenshot_pointer_button<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    ps: &mut PointerState,
    button_code: u32,
    button_state: ButtonState,
    left: bool,
    frame: ButtonFrame,
    target_monitor: &str,
    local_w: i32,
    local_h: i32,
    local_sx: f32,
    local_sy: f32,
    world_now: halley_core::field::Vec2,
) -> bool {
    if !screenshot_controller(&mut *st).screenshot_session_active() {
        return false;
    }

    ps.world = world_now;
    let session_mode = st
        .input
        .interaction_state
        .screenshot_session
        .as_ref()
        .map(|session| session.mode);
    if left && matches!(button_state, ButtonState::Pressed) {
        match session_mode {
            Some(halley_ipc::CaptureMode::Menu) => {
                if let Some(crate::overlay::ScreenshotMenuHit::Item(idx)) =
                    crate::overlay::screenshot_menu_hit_test(local_w, local_h, local_sx, local_sy)
                {
                    screenshot_controller(&mut *st).hover_screenshot_menu_item(Some(idx));
                    let _ = screenshot_controller(&mut *st).activate_screenshot_menu_item(idx);
                }
            }
            Some(halley_ipc::CaptureMode::Region) => {
                let _ = screenshot_controller(&mut *st).begin_screenshot_region_drag(
                    frame.global_sx.round() as i32,
                    frame.global_sy.round() as i32,
                );
            }
            Some(halley_ipc::CaptureMode::Screen) => {
                let _ = screenshot_controller(&mut *st).confirm_screenshot_session(Instant::now());
            }
            Some(halley_ipc::CaptureMode::Window) => {
                let now = Instant::now();
                screenshot_controller(&mut *st).update_screenshot_window_selection_from_pointer(
                    target_monitor,
                    local_w,
                    local_h,
                    local_sx,
                    local_sy,
                    now,
                );
                let _ = screenshot_controller(&mut *st).confirm_screenshot_session(now);
            }
            _ => {}
        }
    }
    if left
        && matches!(button_state, ButtonState::Released)
        && matches!(session_mode, Some(halley_ipc::CaptureMode::Region))
    {
        screenshot_controller(&mut *st).end_screenshot_region_drag();
    }
    if matches!(button_state, ButtonState::Released) {
        handle_button_release(st, ps, ctx.backend, button_code, None, world_now);
    }
    ctx.backend.request_redraw();
    true
}

pub(super) fn handle_screenshot_pointer_motion<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    target_monitor: &str,
    local_w: i32,
    local_h: i32,
    local_sx: f32,
    local_sy: f32,
    effective_sx: f32,
    effective_sy: f32,
    now: Instant,
) -> bool {
    let Some((dragging_region, _)) =
        st.input
            .interaction_state
            .screenshot_session
            .as_ref()
            .map(|session| {
                (
                    session.drag_anchor.is_some()
                        && session.mode == halley_ipc::CaptureMode::Region,
                    session.mode,
                )
            })
    else {
        return false;
    };

    screenshot_controller(&mut *st).update_screenshot_session_monitor(target_monitor.to_string());
    let menu_mode = st
        .input
        .interaction_state
        .screenshot_session
        .as_ref()
        .is_some_and(|session| session.mode == halley_ipc::CaptureMode::Menu);
    if menu_mode {
        let hit = crate::overlay::screenshot_menu_hit_test(local_w, local_h, local_sx, local_sy);
        let idx = match hit {
            Some(crate::overlay::ScreenshotMenuHit::Item(idx)) => Some(idx),
            None => None,
        };
        screenshot_controller(&mut *st).hover_screenshot_menu_item(idx);
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(CursorIcon::Pointer),
        );
        ctx.backend.request_redraw();
        return true;
    }
    let window_mode = st
        .input
        .interaction_state
        .screenshot_session
        .as_ref()
        .is_some_and(|session| session.mode == halley_ipc::CaptureMode::Window);
    if dragging_region {
        screenshot_controller(&mut *st).update_screenshot_region_drag(
            effective_sx.round() as i32,
            effective_sy.round() as i32,
        );
    } else if window_mode {
        screenshot_controller(&mut *st).update_screenshot_window_selection_from_pointer(
            target_monitor,
            local_w,
            local_h,
            local_sx,
            local_sy,
            now,
        );
    }
    crate::compositor::interaction::pointer::set_cursor_override_icon(
        st,
        Some(if window_mode {
            CursorIcon::Pointer
        } else {
            CursorIcon::Crosshair
        }),
    );
    ctx.backend.request_redraw();
    true
}
