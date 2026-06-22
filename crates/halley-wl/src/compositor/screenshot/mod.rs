mod region;
pub(crate) mod state;

use std::path::Path;
use std::sync::mpsc;
use std::time::Instant;

use halley_api::CaptureMode;
use halley_capit::{capture_crop_to_png, default_output_path_in};

use super::root::Halley;
pub(crate) use region::has_screenshot_window_target_for_monitor;
use region::{
    initial_screenshot_selection, screenshot_desktop_bounds, screenshot_menu_index,
    screenshot_menu_modes, screenshot_region_apply_drag, screenshot_region_hit_test,
    screenshot_session_monitor, screenshot_window_crop_for_node,
};
use state::{
    InflightScreenshotCapture, PendingScreenshotCapture, PendingScreenshotKind,
    ScreenshotCaptureResult, ScreenshotRegionDragMode, ScreenshotSessionState,
};

pub(crate) fn screenshot_session_active(st: &Halley) -> bool {
    st.input.interaction_state.screenshot_session.is_some()
}

pub(crate) fn start_screenshot_session(
    st: &mut Halley,
    mode: CaptureMode,
    output: Option<&str>,
    _now: Instant,
) -> bool {
    if screenshot_session_active(st) {
        return false;
    }
    let monitor = screenshot_session_monitor(st, output);
    let serial = st.input.interaction_state.screenshot_next_serial;
    st.input.interaction_state.screenshot_next_serial = serial.saturating_add(1);
    st.input.interaction_state.last_screenshot_result = None;
    let (selected_window, initial_selection) =
        initial_screenshot_selection(st, mode, monitor.as_str());
    let keyboard_captured = true;
    st.begin_modal_keyboard_capture();
    if let Some(enter) = halley_config::keybinds::key_name_to_evdev("return") {
        crate::compositor::interaction::state::trap_modal_key_release(st, enter + 8);
    }
    st.input.interaction_state.screenshot_session = Some(ScreenshotSessionState {
        mode,
        monitor: monitor.clone(),
        selected_window,
        keyboard_captured,
        menu_selected: 0,
        menu_hovered: None,
        drag_anchor: None,
        drag_current: None,
        selection_rect: initial_selection,
        region_drag_mode: ScreenshotRegionDragMode::None,
        region_grab_cursor: (0, 0),
        region_grab_rect: initial_selection,
    });
    st.request_maintenance();
    true
}

pub(crate) fn move_screenshot_menu_selection(st: &mut Halley, delta: i32) -> bool {
    let Some(session) = st.input.interaction_state.screenshot_session.as_mut() else {
        return false;
    };
    if session.mode != CaptureMode::Menu {
        return false;
    }
    let start = session.menu_selected as i32;
    let modes = screenshot_menu_modes();
    let len = modes.len() as i32;
    let step = if delta >= 0 { 1 } else { -1 };
    let session_monitor = session.monitor.clone();
    // Step over any disabled (blanked) entries, e.g. Window with no targets.
    let mut next = start;
    for _ in 0..len {
        next = (next + step).rem_euclid(len);
        let mode = modes[next as usize];
        let enabled = mode != CaptureMode::Window
            || has_screenshot_window_target_for_monitor(st, session_monitor.as_str());
        if enabled {
            break;
        }
    }
    let next = next as usize;
    let Some(session) = st.input.interaction_state.screenshot_session.as_mut() else {
        return false;
    };
    session.menu_selected = next;
    session.menu_hovered = Some(next);
    st.request_maintenance();
    true
}

pub(crate) fn hover_screenshot_menu_item(st: &mut Halley, index: Option<usize>) {
    let monitor = match st.input.interaction_state.screenshot_session.as_ref() {
        Some(session) if session.mode == CaptureMode::Menu => session.monitor.clone(),
        _ => return,
    };
    // Ignore hover on a blanked (disabled) entry, e.g. Window with no targets.
    let index = index.filter(|&idx| {
        screenshot_menu_modes().get(idx).is_some_and(|&mode| {
            mode != CaptureMode::Window
                || has_screenshot_window_target_for_monitor(st, monitor.as_str())
        })
    });
    if let Some(session) = st.input.interaction_state.screenshot_session.as_mut() {
        session.menu_hovered = index;
        if let Some(index) = index {
            session.menu_selected = index;
        }
        st.request_maintenance();
    }
}

pub(crate) fn activate_screenshot_menu_item(st: &mut Halley, index: usize) -> bool {
    let monitor = match st
        .input
        .interaction_state
        .screenshot_session
        .as_ref()
        .map(|session| session.monitor.clone())
    {
        Some(monitor) => monitor,
        None => return false,
    };
    let modes = screenshot_menu_modes();
    let Some(&mode) = modes.get(index) else {
        return false;
    };
    // Window capture is disabled (blanked) when there is nothing to target.
    if mode == CaptureMode::Window
        && !has_screenshot_window_target_for_monitor(st, monitor.as_str())
    {
        return false;
    }
    let (selected_window, initial_selection) =
        initial_screenshot_selection(st, mode, monitor.as_str());
    let Some(session) = st.input.interaction_state.screenshot_session.as_mut() else {
        return false;
    };
    if session.mode != CaptureMode::Menu {
        return false;
    }
    session.mode = mode;
    session.selected_window = selected_window;
    session.menu_selected = index;
    session.menu_hovered = Some(index);
    session.selection_rect = initial_selection;
    session.region_drag_mode = ScreenshotRegionDragMode::None;
    session.region_grab_rect = session.selection_rect;
    session.drag_anchor = None;
    session.drag_current = None;
    st.request_maintenance();
    true
}

pub(crate) fn return_screenshot_session_to_menu(st: &mut Halley) -> bool {
    let (mode, monitor) = match st
        .input
        .interaction_state
        .screenshot_session
        .as_ref()
        .map(|session| (session.mode, session.monitor.clone()))
    {
        Some(state) => state,
        None => return false,
    };
    if mode == CaptureMode::Menu {
        return false;
    }

    let menu_selected = screenshot_menu_index(mode).unwrap_or(0);
    let (selected_window, selection_rect) =
        initial_screenshot_selection(st, CaptureMode::Menu, monitor.as_str());
    let Some(session) = st.input.interaction_state.screenshot_session.as_mut() else {
        return false;
    };
    session.mode = CaptureMode::Menu;
    session.monitor = monitor;
    session.selected_window = selected_window;
    session.menu_selected = menu_selected;
    session.menu_hovered = Some(menu_selected);
    session.selection_rect = selection_rect;
    session.region_drag_mode = ScreenshotRegionDragMode::None;
    session.region_grab_rect = selection_rect;
    session.drag_anchor = None;
    session.drag_current = None;
    st.request_maintenance();
    true
}

pub(crate) fn clear_screenshot_session_state(st: &mut Halley) -> bool {
    let Some(session) = st.input.interaction_state.screenshot_session.take() else {
        return false;
    };
    if session.keyboard_captured {
        let restore_focus = st
            .last_input_surface_node_for_monitor(st.model.monitor_state.current_monitor.as_str())
            .or(st.last_input_surface_node());
        st.schedule_modal_focus_restore_after(restore_focus, Instant::now(), 260);
    }
    st.runtime.screenshot_full_repaint_until_ms = st.now_ms(Instant::now()).saturating_add(120);
    st.request_maintenance();
    true
}

pub(crate) fn cancel_screenshot_session(st: &mut Halley) -> bool {
    if !clear_screenshot_session_state(st) {
        return false;
    }
    st.input.interaction_state.last_screenshot_result = Some(ScreenshotCaptureResult {
        serial: st
            .input
            .interaction_state
            .screenshot_next_serial
            .saturating_sub(1),
        saved_path: None,
        error: Some("cancelled".to_string()),
    });
    true
}

pub(crate) fn update_screenshot_session_monitor(st: &mut Halley, monitor: String) {
    if let Some(session) = st.input.interaction_state.screenshot_session.as_mut() {
        session.monitor = monitor;
    }
}

pub(crate) fn update_screenshot_window_selection_from_pointer(
    st: &mut Halley,
    monitor: &str,
    screen_w: i32,
    screen_h: i32,
    sx: f32,
    sy: f32,
    now: Instant,
) {
    let previous_monitor = st.begin_temporary_render_monitor(monitor);
    let selected_window = crate::spatial::pick_hit_node_at(
        st, screen_w, screen_h, sx, sy, now, None,
    )
    .and_then(|hit| screenshot_window_crop_for_node(st, hit.node_id, monitor).map(|_| hit.node_id));
    st.end_temporary_render_monitor(previous_monitor);
    let selection_rect =
        selected_window.and_then(|node_id| screenshot_window_crop_for_node(st, node_id, monitor));
    if let Some(session) = st.input.interaction_state.screenshot_session.as_mut()
        && session.mode == CaptureMode::Window
    {
        session.monitor = monitor.to_string();
        session.selected_window = selected_window;
        session.selection_rect = selection_rect;
        session.region_grab_rect = selection_rect;
        session.drag_anchor = None;
        session.drag_current = None;
        st.request_maintenance();
    }
}

pub(crate) fn begin_screenshot_region_drag(st: &mut Halley, x: i32, y: i32) -> bool {
    let Some(session) = st.input.interaction_state.screenshot_session.as_mut() else {
        return false;
    };
    if session.mode != CaptureMode::Region {
        return false;
    }
    let selection = session.selection_rect.unwrap_or(halley_capit::CaptureCrop {
        x,
        y,
        w: 320,
        h: 220,
    });
    session.region_drag_mode = screenshot_region_hit_test(selection, x, y);
    session.region_grab_cursor = (x, y);
    session.region_grab_rect = Some(selection);
    session.drag_anchor = Some((x, y));
    session.drag_current = Some((x, y));
    st.request_maintenance();
    true
}

pub(crate) fn update_screenshot_region_drag(st: &mut Halley, x: i32, y: i32) {
    let desktop_bounds = screenshot_desktop_bounds(st);
    let Some(session) = st.input.interaction_state.screenshot_session.as_mut() else {
        return;
    };
    if session.mode != CaptureMode::Region {
        return;
    }
    let Some((ax, ay)) = session.drag_anchor else {
        return;
    };
    session.drag_current = Some((x, y));
    let grab_rect = session
        .region_grab_rect
        .unwrap_or(halley_capit::CaptureCrop {
            x: ax,
            y: ay,
            w: 1,
            h: 1,
        });
    session.selection_rect = Some(screenshot_region_apply_drag(
        session.region_drag_mode,
        (x, y),
        session.region_grab_cursor,
        grab_rect,
        desktop_bounds,
    ));
    st.request_maintenance();
}

pub(crate) fn end_screenshot_region_drag(st: &mut Halley) {
    if let Some(session) = st.input.interaction_state.screenshot_session.as_mut()
        && session.mode == CaptureMode::Region
    {
        session.drag_anchor = None;
        session.region_drag_mode = ScreenshotRegionDragMode::None;
    }
    st.request_maintenance();
}

pub(crate) fn confirm_screenshot_session(st: &mut Halley, now: Instant) -> bool {
    let Some(session) = st.input.interaction_state.screenshot_session.clone() else {
        return false;
    };
    let kind = match session.mode {
        CaptureMode::Menu => {
            return activate_screenshot_menu_item(st, session.menu_selected);
        }
        CaptureMode::Region => match session.selection_rect {
            Some(rect) if rect.w > 0 && rect.h > 0 => PendingScreenshotKind::Crop(rect),
            _ => return false,
        },
        CaptureMode::Screen => {
            let Some(space) = st
                .model
                .monitor_state
                .monitors
                .get(session.monitor.as_str())
            else {
                return false;
            };
            PendingScreenshotKind::Crop(halley_capit::CaptureCrop {
                x: space.offset_x,
                y: space.offset_y,
                w: space.width.max(1),
                h: space.height.max(1),
            })
        }
        CaptureMode::Window => match session.selected_window {
            Some(node_id) => PendingScreenshotKind::Window { node_id },
            None => return false,
        },
    };
    let output_path = default_output_path_in(
        expand_screenshot_directory(st.runtime.tuning.screenshot.directory.as_str()),
        match session.mode {
            CaptureMode::Menu => "halley-capture",
            CaptureMode::Region => "halley-region",
            CaptureMode::Screen => "halley-screen",
            CaptureMode::Window => "halley-window",
        },
    );
    if let Err(err) = ensure_screenshot_output_directory(output_path.as_path()) {
        st.input.interaction_state.last_screenshot_result = Some(ScreenshotCaptureResult {
            serial: st
                .input
                .interaction_state
                .screenshot_next_serial
                .saturating_sub(1),
            saved_path: None,
            error: Some(err),
        });
        return false;
    }
    let monitor = session.monitor.clone();
    let serial = st
        .input
        .interaction_state
        .screenshot_next_serial
        .saturating_sub(1);
    let _ = clear_screenshot_session_state(st);
    st.input.interaction_state.pending_screenshot_capture = Some(PendingScreenshotCapture {
        monitor,
        serial,
        kind,
        output_path,
        execute_at_ms: st.now_ms(now).saturating_add(24),
    });
    st.request_maintenance();
    true
}

pub(crate) fn run_pending_screenshot_capture_if_due(st: &mut Halley, now_ms: u64) {
    if let Some(pending) = st
        .input
        .interaction_state
        .pending_screenshot_capture
        .clone()
        && now_ms >= pending.execute_at_ms
    {
        st.input.interaction_state.pending_screenshot_capture = None;
        match pending.kind {
            PendingScreenshotKind::Crop(crop) => {
                let (tx, rx) = mpsc::channel();
                let output_path = pending.output_path.clone();
                std::thread::spawn(move || {
                    let result = capture_crop_to_png(output_path.as_path(), crop)
                        .map(|_| output_path.clone());
                    let _ = tx.send(result);
                });
                st.input.interaction_state.inflight_screenshot_capture =
                    Some(InflightScreenshotCapture {
                        monitor: pending.monitor,
                        serial: pending.serial,
                        rx,
                    });
            }
            PendingScreenshotKind::Window { node_id } => {
                let result = st
                    .portal
                    .capture_backend
                    .clone()
                    .ok_or_else(|| "no capture backend configured".to_string())
                    .and_then(|backend| {
                        backend
                            .capture_window_png(
                                st,
                                pending.monitor.as_str(),
                                node_id,
                                pending.output_path.as_path(),
                            )
                            .map_err(|err| err.to_string())
                    });
                match &result {
                    Ok(_) => {
                        let message =
                            format!("Saved screenshot\n{}", pending.output_path.display());
                        st.ui.render_state.show_overlay_toast(
                            pending.monitor.as_str(),
                            message.as_str(),
                            4200,
                            now_ms,
                        );
                    }
                    Err(err) => {
                        let message = format!("Capture failed\n{err}");
                        st.ui.render_state.show_overlay_toast(
                            pending.monitor.as_str(),
                            message.as_str(),
                            5000,
                            now_ms,
                        );
                    }
                }
                st.input.interaction_state.last_screenshot_result = Some(ScreenshotCaptureResult {
                    serial: pending.serial,
                    saved_path: result.as_ref().ok().map(|_| pending.output_path.clone()),
                    error: result.err(),
                });
            }
        }
        st.request_maintenance();
    }

    if let Some(inflight) = st
        .input
        .interaction_state
        .inflight_screenshot_capture
        .take()
    {
        match inflight.rx.try_recv() {
            Ok(Ok(path)) => {
                st.input.interaction_state.last_screenshot_result = Some(ScreenshotCaptureResult {
                    serial: inflight.serial,
                    saved_path: Some(path.clone()),
                    error: None,
                });
                let message = format!("Saved screenshot\n{}", path.display());
                st.ui.render_state.show_overlay_toast(
                    inflight.monitor.as_str(),
                    message.as_str(),
                    4200,
                    now_ms,
                );
            }
            Ok(Err(err)) => {
                st.input.interaction_state.last_screenshot_result = Some(ScreenshotCaptureResult {
                    serial: inflight.serial,
                    saved_path: None,
                    error: Some(err.clone()),
                });
                let message = format!("Capture failed\n{err}");
                st.ui.render_state.show_overlay_toast(
                    inflight.monitor.as_str(),
                    message.as_str(),
                    5000,
                    now_ms,
                );
            }
            Err(mpsc::TryRecvError::Empty) => {
                st.input.interaction_state.inflight_screenshot_capture = Some(inflight);
                st.request_maintenance();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                st.input.interaction_state.last_screenshot_result = Some(ScreenshotCaptureResult {
                    serial: inflight.serial,
                    saved_path: None,
                    error: Some("capture worker disconnected".to_string()),
                });
            }
        }
    }
}

fn expand_screenshot_directory(raw: &str) -> std::path::PathBuf {
    if let Some(rest) = raw.strip_prefix("$HOME/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return std::path::PathBuf::from(home).join(rest);
    }
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return std::path::PathBuf::from(home).join(rest);
    }
    std::path::PathBuf::from(raw)
}

fn ensure_screenshot_output_directory(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("create screenshot directory {}: {err}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use halley_config::RuntimeTuning;
    use smithay::reexports::wayland_server::Display;

    use super::*;

    fn two_monitor_tuning() -> RuntimeTuning {
        let mut tuning = RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        tuning
    }

    #[test]
    fn screenshot_session_uses_pointer_monitor_by_default() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, two_monitor_tuning());
        state.input.interaction_state.last_pointer_screen_global = Some((1200.0, 300.0));

        assert!(super::start_screenshot_session(
            &mut state,
            CaptureMode::Menu,
            None,
            Instant::now()
        ));
        assert_eq!(
            state
                .input
                .interaction_state
                .screenshot_session
                .as_ref()
                .map(|session| session.monitor.as_str()),
            Some("right")
        );
    }

    #[test]
    fn screenshot_escape_target_returns_to_menu_before_cancel() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, RuntimeTuning::default());

        assert!(super::start_screenshot_session(
            &mut state,
            CaptureMode::Menu,
            None,
            Instant::now()
        ));
        assert!(super::activate_screenshot_menu_item(&mut state, 0));
        assert_eq!(
            state
                .input
                .interaction_state
                .screenshot_session
                .as_ref()
                .map(|session| session.mode),
            Some(CaptureMode::Region)
        );

        assert!(super::return_screenshot_session_to_menu(&mut state));
        let session = state
            .input
            .interaction_state
            .screenshot_session
            .as_ref()
            .expect("session");
        assert_eq!(session.mode, CaptureMode::Menu);
        assert_eq!(session.menu_selected, 0);
        assert_eq!(session.menu_hovered, Some(0));
    }

    #[test]
    fn ensure_screenshot_output_directory_creates_missing_parent() {
        let base = std::env::temp_dir().join(format!(
            "halley-screenshot-dir-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let out = base.join("nested").join("capture.png");

        ensure_screenshot_output_directory(out.as_path()).expect("directory should be created");

        assert!(out.parent().is_some_and(|parent| parent.exists()));

        let _ = std::fs::remove_dir_all(base);
    }
}
