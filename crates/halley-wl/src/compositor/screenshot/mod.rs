mod region;
pub(crate) mod state;

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::mpsc;
use std::time::Instant;

use halley_capit::{capture_crop_to_png, default_output_path_in};
use halley_ipc::CaptureMode;

use super::root::Halley;
use region::{
    initial_screenshot_selection, screenshot_desktop_bounds, screenshot_menu_index,
    screenshot_menu_modes, screenshot_region_apply_drag, screenshot_region_hit_test,
    screenshot_session_monitor, screenshot_window_crop_for_node,
};
use state::{
    InflightScreenshotCapture, PendingScreenshotCapture, PendingScreenshotKind,
    ScreenshotCaptureResult, ScreenshotRegionDragMode, ScreenshotSessionState,
};

pub(crate) struct ScreenshotController<T> {
    st: T,
}

pub(crate) fn screenshot_controller<T>(st: T) -> ScreenshotController<T> {
    ScreenshotController { st }
}

impl<T: Deref<Target = Halley>> Deref for ScreenshotController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for ScreenshotController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

impl<T: Deref<Target = Halley>> ScreenshotController<T> {
    pub(crate) fn screenshot_session_active(&self) -> bool {
        self.input.interaction_state.screenshot_session.is_some()
    }
}

impl<T: DerefMut<Target = Halley>> ScreenshotController<T> {
    pub(crate) fn start_screenshot_session(
        &mut self,
        mode: CaptureMode,
        output: Option<&str>,
        _now: Instant,
    ) -> bool {
        if self.screenshot_session_active() {
            return false;
        }
        let monitor = screenshot_session_monitor(self, output);
        let serial = self.input.interaction_state.screenshot_next_serial;
        self.input.interaction_state.screenshot_next_serial = serial.saturating_add(1);
        self.input.interaction_state.last_screenshot_result = None;
        let (selected_window, initial_selection) =
            initial_screenshot_selection(self, mode, monitor.as_str());
        let keyboard_captured = true;
        self.begin_modal_keyboard_capture();
        if let Some(enter) = halley_config::keybinds::key_name_to_evdev("return") {
            crate::compositor::interaction::state::trap_modal_key_release(self, enter + 8);
        }
        self.input.interaction_state.screenshot_session = Some(ScreenshotSessionState {
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
        self.request_maintenance();
        true
    }

    pub(crate) fn move_screenshot_menu_selection(&mut self, delta: i32) -> bool {
        let Some(session) = self.input.interaction_state.screenshot_session.as_mut() else {
            return false;
        };
        if session.mode != CaptureMode::Menu {
            return false;
        }
        let len = screenshot_menu_modes().len() as i32;
        let next = (session.menu_selected as i32 + delta).rem_euclid(len) as usize;
        session.menu_selected = next;
        session.menu_hovered = Some(next);
        self.request_maintenance();
        true
    }

    pub(crate) fn hover_screenshot_menu_item(&mut self, index: Option<usize>) {
        if let Some(session) = self.input.interaction_state.screenshot_session.as_mut()
            && session.mode == CaptureMode::Menu
        {
            session.menu_hovered = index;
            if let Some(index) = index {
                session.menu_selected = index;
            }
            self.request_maintenance();
        }
    }

    pub(crate) fn activate_screenshot_menu_item(&mut self, index: usize) -> bool {
        let monitor = match self
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
        let (selected_window, initial_selection) =
            initial_screenshot_selection(self, mode, monitor.as_str());
        let Some(session) = self.input.interaction_state.screenshot_session.as_mut() else {
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
        self.request_maintenance();
        true
    }

    pub(crate) fn return_screenshot_session_to_menu(&mut self) -> bool {
        let (mode, monitor) = match self
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
            initial_screenshot_selection(self, CaptureMode::Menu, monitor.as_str());
        let Some(session) = self.input.interaction_state.screenshot_session.as_mut() else {
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
        self.request_maintenance();
        true
    }

    fn clear_screenshot_session_state(&mut self) -> bool {
        let Some(session) = self.input.interaction_state.screenshot_session.take() else {
            return false;
        };
        if session.keyboard_captured {
            let restore_focus = self
                .last_input_surface_node_for_monitor(
                    self.model.monitor_state.current_monitor.as_str(),
                )
                .or(self.last_input_surface_node());
            self.schedule_modal_focus_restore_after(restore_focus, Instant::now(), 260);
        }
        self.runtime.screenshot_full_repaint_until_ms =
            self.now_ms(Instant::now()).saturating_add(120);
        self.request_maintenance();
        true
    }

    pub(crate) fn cancel_screenshot_session(&mut self) -> bool {
        if !self.clear_screenshot_session_state() {
            return false;
        }
        self.input.interaction_state.last_screenshot_result = Some(ScreenshotCaptureResult {
            serial: self
                .input
                .interaction_state
                .screenshot_next_serial
                .saturating_sub(1),
            saved_path: None,
            error: Some("cancelled".to_string()),
        });
        true
    }

    pub(crate) fn update_screenshot_session_monitor(&mut self, monitor: String) {
        if let Some(session) = self.input.interaction_state.screenshot_session.as_mut() {
            session.monitor = monitor;
        }
    }

    pub(crate) fn update_screenshot_window_selection_from_pointer(
        &mut self,
        monitor: &str,
        screen_w: i32,
        screen_h: i32,
        sx: f32,
        sy: f32,
        now: Instant,
    ) {
        let previous_monitor = self.begin_temporary_render_monitor(monitor);
        let selected_window = crate::spatial::pick_hit_node_at(
            self, screen_w, screen_h, sx, sy, now, None,
        )
        .and_then(|hit| {
            screenshot_window_crop_for_node(self, hit.node_id, monitor).map(|_| hit.node_id)
        });
        self.end_temporary_render_monitor(previous_monitor);
        let selection_rect = selected_window
            .and_then(|node_id| screenshot_window_crop_for_node(self, node_id, monitor));
        if let Some(session) = self.input.interaction_state.screenshot_session.as_mut()
            && session.mode == CaptureMode::Window
        {
            session.monitor = monitor.to_string();
            session.selected_window = selected_window;
            session.selection_rect = selection_rect;
            session.region_grab_rect = selection_rect;
            session.drag_anchor = None;
            session.drag_current = None;
            self.request_maintenance();
        }
    }

    pub(crate) fn begin_screenshot_region_drag(&mut self, x: i32, y: i32) -> bool {
        let Some(session) = self.input.interaction_state.screenshot_session.as_mut() else {
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
        self.request_maintenance();
        true
    }

    pub(crate) fn update_screenshot_region_drag(&mut self, x: i32, y: i32) {
        let desktop_bounds = screenshot_desktop_bounds(self);
        let Some(session) = self.input.interaction_state.screenshot_session.as_mut() else {
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
        self.request_maintenance();
    }

    pub(crate) fn end_screenshot_region_drag(&mut self) {
        if let Some(session) = self.input.interaction_state.screenshot_session.as_mut()
            && session.mode == CaptureMode::Region
        {
            session.drag_anchor = None;
            session.region_drag_mode = ScreenshotRegionDragMode::None;
        }
        self.request_maintenance();
    }

    pub(crate) fn confirm_screenshot_session(&mut self, now: Instant) -> bool {
        let Some(session) = self.input.interaction_state.screenshot_session.clone() else {
            return false;
        };
        let kind = match session.mode {
            CaptureMode::Menu => {
                return self.activate_screenshot_menu_item(session.menu_selected);
            }
            CaptureMode::Region => match session.selection_rect {
                Some(rect) if rect.w > 0 && rect.h > 0 => PendingScreenshotKind::Crop(rect),
                _ => return false,
            },
            CaptureMode::Screen => {
                let Some(space) = self
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
            expand_screenshot_directory(self.runtime.tuning.screenshot.directory.as_str()),
            match session.mode {
                CaptureMode::Menu => "halley-capture",
                CaptureMode::Region => "halley-region",
                CaptureMode::Screen => "halley-screen",
                CaptureMode::Window => "halley-window",
            },
        );
        if let Err(err) = ensure_screenshot_output_directory(output_path.as_path()) {
            self.input.interaction_state.last_screenshot_result = Some(ScreenshotCaptureResult {
                serial: self
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
        let serial = self
            .input
            .interaction_state
            .screenshot_next_serial
            .saturating_sub(1);
        let _ = self.clear_screenshot_session_state();
        self.input.interaction_state.pending_screenshot_capture = Some(PendingScreenshotCapture {
            monitor,
            serial,
            kind,
            output_path,
            execute_at_ms: self.now_ms(now).saturating_add(24),
        });
        self.request_maintenance();
        true
    }

    pub(crate) fn run_pending_screenshot_capture_if_due(&mut self, now_ms: u64) {
        if let Some(pending) = self
            .input
            .interaction_state
            .pending_screenshot_capture
            .clone()
            && now_ms >= pending.execute_at_ms
        {
            self.input.interaction_state.pending_screenshot_capture = None;
            match pending.kind {
                PendingScreenshotKind::Crop(crop) => {
                    let (tx, rx) = mpsc::channel();
                    let output_path = pending.output_path.clone();
                    std::thread::spawn(move || {
                        let result = capture_crop_to_png(output_path.as_path(), crop)
                            .map(|_| output_path.clone());
                        let _ = tx.send(result);
                    });
                    self.input.interaction_state.inflight_screenshot_capture =
                        Some(InflightScreenshotCapture {
                            monitor: pending.monitor,
                            serial: pending.serial,
                            rx,
                        });
                }
                PendingScreenshotKind::Window { node_id } => {
                    let result = self
                        .portal
                        .capture_backend
                        .clone()
                        .ok_or_else(|| "no capture backend configured".to_string())
                        .and_then(|backend| {
                            backend
                                .capture_window_png(
                                    self,
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
                            self.ui.render_state.show_overlay_toast(
                                pending.monitor.as_str(),
                                message.as_str(),
                                4200,
                                now_ms,
                            );
                        }
                        Err(err) => {
                            let message = format!("Capture failed\n{err}");
                            self.ui.render_state.show_overlay_toast(
                                pending.monitor.as_str(),
                                message.as_str(),
                                5000,
                                now_ms,
                            );
                        }
                    }
                    self.input.interaction_state.last_screenshot_result =
                        Some(ScreenshotCaptureResult {
                            serial: pending.serial,
                            saved_path: result.as_ref().ok().map(|_| pending.output_path.clone()),
                            error: result.err(),
                        });
                }
            }
            self.request_maintenance();
        }

        if let Some(inflight) = self
            .input
            .interaction_state
            .inflight_screenshot_capture
            .take()
        {
            match inflight.rx.try_recv() {
                Ok(Ok(path)) => {
                    self.input.interaction_state.last_screenshot_result =
                        Some(ScreenshotCaptureResult {
                            serial: inflight.serial,
                            saved_path: Some(path.clone()),
                            error: None,
                        });
                    let message = format!("Saved screenshot\n{}", path.display());
                    self.ui.render_state.show_overlay_toast(
                        inflight.monitor.as_str(),
                        message.as_str(),
                        4200,
                        now_ms,
                    );
                }
                Ok(Err(err)) => {
                    self.input.interaction_state.last_screenshot_result =
                        Some(ScreenshotCaptureResult {
                            serial: inflight.serial,
                            saved_path: None,
                            error: Some(err.clone()),
                        });
                    let message = format!("Capture failed\n{err}");
                    self.ui.render_state.show_overlay_toast(
                        inflight.monitor.as_str(),
                        message.as_str(),
                        5000,
                        now_ms,
                    );
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.input.interaction_state.inflight_screenshot_capture = Some(inflight);
                    self.request_maintenance();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.input.interaction_state.last_screenshot_result =
                        Some(ScreenshotCaptureResult {
                            serial: inflight.serial,
                            saved_path: None,
                            error: Some("capture worker disconnected".to_string()),
                        });
                }
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

        assert!(screenshot_controller(&mut state).start_screenshot_session(
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

        assert!(screenshot_controller(&mut state).start_screenshot_session(
            CaptureMode::Menu,
            None,
            Instant::now()
        ));
        assert!(screenshot_controller(&mut state).activate_screenshot_menu_item(0));
        assert_eq!(
            state
                .input
                .interaction_state
                .screenshot_session
                .as_ref()
                .map(|session| session.mode),
            Some(CaptureMode::Region)
        );

        assert!(screenshot_controller(&mut state).return_screenshot_session_to_menu());
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
