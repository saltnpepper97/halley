use std::time::Instant;

use halley_api::CaptureMode;
use smithay::backend::input::ButtonState;
use smithay::input::pointer::CursorIcon;

use crate::backend::interface::BackendView;
use crate::compositor::interaction::PointerState;
use crate::compositor::root::Halley;
use crate::compositor::screenshot;
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
    if !screenshot::screenshot_session_active(&mut *st) {
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
            Some(CaptureMode::Menu) => {
                if let Some(crate::overlay::ScreenshotMenuHit::Item(idx)) =
                    crate::overlay::screenshot_menu_hit_test(local_w, local_h, local_sx, local_sy)
                {
                    screenshot::hover_screenshot_menu_item(&mut *st, Some(idx));
                    let _ = screenshot::activate_screenshot_menu_item(&mut *st, idx);
                }
            }
            Some(CaptureMode::Region) => {
                let _ = screenshot::begin_screenshot_region_drag(
                    &mut *st,
                    frame.global_sx.round() as i32,
                    frame.global_sy.round() as i32,
                );
            }
            Some(CaptureMode::Screen) => {
                let _ = screenshot::confirm_screenshot_session(&mut *st, Instant::now());
            }
            Some(CaptureMode::Window) => {
                let now = Instant::now();
                screenshot::update_screenshot_window_selection_from_pointer(
                    &mut *st,
                    target_monitor,
                    local_w,
                    local_h,
                    local_sx,
                    local_sy,
                    now,
                );
                let _ = screenshot::confirm_screenshot_session(&mut *st, now);
            }
            _ => {}
        }
    }
    if left
        && matches!(button_state, ButtonState::Released)
        && matches!(session_mode, Some(CaptureMode::Region))
    {
        screenshot::end_screenshot_region_drag(&mut *st);
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
                    session.drag_anchor.is_some() && session.mode == CaptureMode::Region,
                    session.mode,
                )
            })
    else {
        return false;
    };

    screenshot::update_screenshot_session_monitor(&mut *st, target_monitor.to_string());
    let menu_mode = st
        .input
        .interaction_state
        .screenshot_session
        .as_ref()
        .is_some_and(|session| session.mode == CaptureMode::Menu);
    if menu_mode {
        let hit = crate::overlay::screenshot_menu_hit_test(local_w, local_h, local_sx, local_sy);
        let idx = match hit {
            Some(crate::overlay::ScreenshotMenuHit::Item(idx)) => Some(idx),
            None => None,
        };
        screenshot::hover_screenshot_menu_item(&mut *st, idx);
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
        .is_some_and(|session| session.mode == CaptureMode::Window);
    if dragging_region {
        screenshot::update_screenshot_region_drag(
            &mut *st,
            effective_sx.round() as i32,
            effective_sy.round() as i32,
        );
    } else if window_mode {
        screenshot::update_screenshot_window_selection_from_pointer(
            &mut *st,
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
