use std::ops::{Deref, DerefMut};
use std::sync::mpsc;
use std::time::Instant;

use halley_capit::{CaptureCrop, capture_crop_to_png, default_output_path_in};
use halley_core::field::NodeId;
use halley_ipc::CaptureMode;

use super::root::Halley;
use super::surface_ops::active_stacking_visible_members_for_monitor;

const SCREENSHOT_HANDLE_SIZE: i32 = 12;
const SCREENSHOT_HANDLE_HIT: i32 = 14;
const SCREENSHOT_MIN_W: i32 = 8;
const SCREENSHOT_MIN_H: i32 = 8;

fn screenshot_desktop_bounds(st: &Halley) -> (i32, i32, i32, i32) {
    st.model.monitor_state.monitors.values().fold(
        (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
        |(min_x, min_y, max_x, max_y), space| {
            (
                min_x.min(space.offset_x),
                min_y.min(space.offset_y),
                max_x.max(space.offset_x + space.width),
                max_y.max(space.offset_y + space.height),
            )
        },
    )
}

fn screenshot_window_matches_monitor(st: &Halley, node_id: NodeId, monitor: &str) -> bool {
    st.model.field.node(node_id).is_some_and(|node| {
        node.state == halley_core::field::NodeState::Active
            && st.model.field.is_visible(node_id)
            && st
                .model
                .monitor_state
                .node_monitor
                .get(&node_id)
                .map(|owner| owner.as_str())
                .unwrap_or(st.model.monitor_state.current_monitor.as_str())
                == monitor
    })
}

fn screenshot_window_crop_for_node(
    st: &mut Halley,
    node_id: NodeId,
    monitor: &str,
) -> Option<CaptureCrop> {
    if !screenshot_window_matches_monitor(st, node_id, monitor) {
        return None;
    }
    let (offset_x, offset_y, width, height) = {
        let space = st.model.monitor_state.monitors.get(monitor)?;
        (space.offset_x, space.offset_y, space.width, space.height)
    };
    let previous_monitor = st.begin_temporary_render_monitor(monitor);
    let rect =
        crate::input::active_node_screen_rect(st, width, height, node_id, Instant::now(), None);
    st.end_temporary_render_monitor(previous_monitor);
    let (left, top, right, bottom) = rect?;
    Some(CaptureCrop {
        x: offset_x + left.min(right).round() as i32,
        y: offset_y + top.min(bottom).round() as i32,
        w: (right - left).abs().round().max(1.0) as i32,
        h: (bottom - top).abs().round().max(1.0) as i32,
    })
}

fn screenshot_window_target_for_monitor(st: &Halley, monitor: &str) -> Option<NodeId> {
    [
        st.last_input_surface_node_for_monitor(monitor),
        st.last_focused_surface_node_for_monitor(monitor),
        st.model.focus_state.primary_interaction_focus,
    ]
    .into_iter()
    .flatten()
    .find(|&node_id| screenshot_window_matches_monitor(st, node_id, monitor))
    .or_else(|| {
        active_stacking_visible_members_for_monitor(st, monitor)
            .into_iter()
            .find(|&node_id| screenshot_window_matches_monitor(st, node_id, monitor))
    })
}

fn initial_screenshot_selection(
    st: &mut Halley,
    mode: CaptureMode,
    monitor: &str,
) -> (Option<NodeId>, Option<CaptureCrop>) {
    match mode {
        CaptureMode::Region => {
            let Some(space) = st.model.monitor_state.monitors.get(monitor) else {
                return (None, None);
            };
            (
                None,
                Some(CaptureCrop {
                    x: space.offset_x
                        + (space.width - (space.width / 2).clamp(260, space.width.max(1))) / 2,
                    y: space.offset_y
                        + (space.height - (space.height / 2).clamp(180, space.height.max(1))) / 2,
                    w: (space.width / 2)
                        .clamp(260, space.width.max(1))
                        .max(SCREENSHOT_MIN_W),
                    h: (space.height / 2)
                        .clamp(180, space.height.max(1))
                        .max(SCREENSHOT_MIN_H),
                }),
            )
        }
        CaptureMode::Window => {
            let selected_window = screenshot_window_target_for_monitor(st, monitor);
            let selection_rect = selected_window
                .and_then(|node_id| screenshot_window_crop_for_node(st, node_id, monitor));
            (selected_window, selection_rect)
        }
        CaptureMode::Menu | CaptureMode::Screen => (None, None),
    }
}

fn screenshot_menu_modes() -> [CaptureMode; 3] {
    [
        CaptureMode::Region,
        CaptureMode::Screen,
        CaptureMode::Window,
    ]
}

fn screenshot_menu_index(mode: CaptureMode) -> Option<usize> {
    screenshot_menu_modes()
        .iter()
        .position(|candidate| *candidate == mode)
}

fn screenshot_session_monitor(st: &Halley, output: Option<&str>) -> String {
    output
        .and_then(|name| {
            st.model
                .monitor_state
                .monitors
                .contains_key(name)
                .then_some(name.to_string())
        })
        .or_else(|| {
            st.input
                .interaction_state
                .last_pointer_screen_global
                .and_then(|(sx, sy)| st.monitor_for_screen(sx, sy))
        })
        .or_else(|| {
            st.model
                .monitor_state
                .monitors
                .contains_key(st.interaction_monitor())
                .then(|| st.interaction_monitor().to_string())
        })
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone())
}

fn screenshot_contains(rect: CaptureCrop, px: i32, py: i32) -> bool {
    px >= rect.x && py >= rect.y && px < rect.x + rect.w && py < rect.y + rect.h
}

fn screenshot_dist2(ax: i32, ay: i32, bx: i32, by: i32) -> i64 {
    let dx = i64::from(ax - bx);
    let dy = i64::from(ay - by);
    dx * dx + dy * dy
}

fn screenshot_corner_hit(
    selection: CaptureCrop,
    px: i32,
    py: i32,
) -> Option<crate::compositor::interaction::state::ScreenshotRegionResizeDir> {
    let rad = SCREENSHOT_HANDLE_HIT.max(SCREENSHOT_HANDLE_SIZE / 2);
    let rad2 = (rad as i64) * (rad as i64);
    let tl = screenshot_dist2(px, py, selection.x, selection.y);
    let tr = screenshot_dist2(px, py, selection.x + selection.w, selection.y);
    let bl = screenshot_dist2(px, py, selection.x, selection.y + selection.h);
    let br = screenshot_dist2(px, py, selection.x + selection.w, selection.y + selection.h);
    let mut best = (i64::MAX, 0);
    for (d, idx) in [(tl, 0), (tr, 1), (bl, 2), (br, 3)] {
        if d < best.0 {
            best = (d, idx);
        }
    }
    if best.0 > rad2 {
        return None;
    }
    Some(match best.1 {
        0 => crate::compositor::interaction::state::ScreenshotRegionResizeDir {
            left: true,
            right: false,
            top: true,
            bottom: false,
        },
        1 => crate::compositor::interaction::state::ScreenshotRegionResizeDir {
            left: false,
            right: true,
            top: true,
            bottom: false,
        },
        2 => crate::compositor::interaction::state::ScreenshotRegionResizeDir {
            left: true,
            right: false,
            top: false,
            bottom: true,
        },
        _ => crate::compositor::interaction::state::ScreenshotRegionResizeDir {
            left: false,
            right: true,
            top: false,
            bottom: true,
        },
    })
}

fn screenshot_region_hit_test(
    selection: CaptureCrop,
    px: i32,
    py: i32,
) -> crate::compositor::interaction::state::ScreenshotRegionDragMode {
    use crate::compositor::interaction::state::{
        ScreenshotRegionDragMode, ScreenshotRegionResizeDir,
    };

    if let Some(dir) = screenshot_corner_hit(selection, px, py) {
        return ScreenshotRegionDragMode::Resize(dir);
    }

    let left = (px - selection.x).abs() <= SCREENSHOT_HANDLE_HIT
        && py >= selection.y - SCREENSHOT_HANDLE_HIT
        && py <= selection.y + selection.h + SCREENSHOT_HANDLE_HIT;
    let right = (px - (selection.x + selection.w)).abs() <= SCREENSHOT_HANDLE_HIT
        && py >= selection.y - SCREENSHOT_HANDLE_HIT
        && py <= selection.y + selection.h + SCREENSHOT_HANDLE_HIT;
    let top = (py - selection.y).abs() <= SCREENSHOT_HANDLE_HIT
        && px >= selection.x - SCREENSHOT_HANDLE_HIT
        && px <= selection.x + selection.w + SCREENSHOT_HANDLE_HIT;
    let bottom = (py - (selection.y + selection.h)).abs() <= SCREENSHOT_HANDLE_HIT
        && px >= selection.x - SCREENSHOT_HANDLE_HIT
        && px <= selection.x + selection.w + SCREENSHOT_HANDLE_HIT;

    let dir = ScreenshotRegionResizeDir {
        left,
        right,
        top,
        bottom,
    };
    if left || right || top || bottom {
        return ScreenshotRegionDragMode::Resize(dir);
    }
    if screenshot_contains(selection, px, py) {
        ScreenshotRegionDragMode::Move
    } else {
        ScreenshotRegionDragMode::Resize(screenshot_corner_hit(selection, px, py).unwrap_or(
            ScreenshotRegionResizeDir {
                left: px < selection.x + selection.w / 2,
                right: px >= selection.x + selection.w / 2,
                top: py < selection.y + selection.h / 2,
                bottom: py >= selection.y + selection.h / 2,
            },
        ))
    }
}

fn screenshot_crop_clamp_to(rect: &mut CaptureCrop, bounds: (i32, i32, i32, i32)) {
    let (min_x, min_y, max_x, max_y) = bounds;
    rect.w = rect.w.max(SCREENSHOT_MIN_W);
    rect.h = rect.h.max(SCREENSHOT_MIN_H);
    if rect.x < min_x {
        rect.x = min_x;
    }
    if rect.y < min_y {
        rect.y = min_y;
    }
    if rect.x + rect.w > max_x {
        rect.x = (max_x - rect.w).max(min_x);
    }
    if rect.y + rect.h > max_y {
        rect.y = (max_y - rect.h).max(min_y);
    }
}

fn screenshot_region_apply_drag(
    drag_mode: crate::compositor::interaction::state::ScreenshotRegionDragMode,
    cursor: (i32, i32),
    grab_cursor: (i32, i32),
    grab_rect: CaptureCrop,
    bounds: (i32, i32, i32, i32),
) -> CaptureCrop {
    use crate::compositor::interaction::state::ScreenshotRegionDragMode;

    let (cx, cy) = cursor;
    let (gx, gy) = grab_cursor;
    match drag_mode {
        ScreenshotRegionDragMode::None => grab_rect,
        ScreenshotRegionDragMode::Move => {
            let mut r = CaptureCrop {
                x: grab_rect.x + (cx - gx),
                y: grab_rect.y + (cy - gy),
                w: grab_rect.w.max(SCREENSHOT_MIN_W),
                h: grab_rect.h.max(SCREENSHOT_MIN_H),
            };
            screenshot_crop_clamp_to(&mut r, bounds);
            r
        }
        ScreenshotRegionDragMode::Resize(dir) => {
            let mut left = grab_rect.x;
            let mut top = grab_rect.y;
            let mut right = grab_rect.x + grab_rect.w;
            let mut bottom = grab_rect.y + grab_rect.h;
            if dir.left {
                left = cx;
            }
            if dir.right {
                right = cx;
            }
            if dir.top {
                top = cy;
            }
            if dir.bottom {
                bottom = cy;
            }
            if left > right {
                std::mem::swap(&mut left, &mut right);
            }
            if top > bottom {
                std::mem::swap(&mut top, &mut bottom);
            }
            let mut r = CaptureCrop {
                x: left,
                y: top,
                w: (right - left).max(SCREENSHOT_MIN_W),
                h: (bottom - top).max(SCREENSHOT_MIN_H),
            };
            screenshot_crop_clamp_to(&mut r, bounds);
            r
        }
    }
}

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
        self.input.interaction_state.screenshot_session = Some(
            crate::compositor::interaction::state::ScreenshotSessionState {
                mode,
                monitor: monitor.clone(),
                selected_window,
                keyboard_captured,
                menu_selected: 0,
                menu_hovered: None,
                drag_anchor: None,
                drag_current: None,
                selection_rect: initial_selection,
                region_drag_mode:
                    crate::compositor::interaction::state::ScreenshotRegionDragMode::None,
                region_grab_cursor: (0, 0),
                region_grab_rect: initial_selection,
            },
        );
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
        session.region_drag_mode =
            crate::compositor::interaction::state::ScreenshotRegionDragMode::None;
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
        session.region_drag_mode =
            crate::compositor::interaction::state::ScreenshotRegionDragMode::None;
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
        self.input.interaction_state.last_screenshot_result = Some(
            crate::compositor::interaction::state::ScreenshotCaptureResult {
                serial: self
                    .input
                    .interaction_state
                    .screenshot_next_serial
                    .saturating_sub(1),
                saved_path: None,
                error: Some("cancelled".to_string()),
            },
        );
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
            screenshot_window_matches_monitor(self, hit.node_id, monitor).then_some(hit.node_id)
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
        let selection = session.selection_rect.unwrap_or(CaptureCrop {
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
        let grab_rect = session.region_grab_rect.unwrap_or(CaptureCrop {
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
            session.region_drag_mode =
                crate::compositor::interaction::state::ScreenshotRegionDragMode::None;
        }
        self.request_maintenance();
    }

    pub(crate) fn confirm_screenshot_session(&mut self, now: Instant) -> bool {
        let Some(session) = self.input.interaction_state.screenshot_session.clone() else {
            return false;
        };
        let crop = match session.mode {
            CaptureMode::Menu => {
                return self.activate_screenshot_menu_item(session.menu_selected);
            }
            CaptureMode::Region => match session.selection_rect {
                Some(rect) if rect.w > 0 && rect.h > 0 => rect,
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
                CaptureCrop {
                    x: space.offset_x,
                    y: space.offset_y,
                    w: space.width.max(1),
                    h: space.height.max(1),
                }
            }
            CaptureMode::Window => match session
                .selected_window
                .and_then(|node_id| {
                    screenshot_window_crop_for_node(self, node_id, session.monitor.as_str())
                })
                .or(session.selection_rect)
            {
                Some(rect) if rect.w > 0 && rect.h > 0 => rect,
                _ => return false,
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
        let monitor = session.monitor.clone();
        let serial = self
            .input
            .interaction_state
            .screenshot_next_serial
            .saturating_sub(1);
        let _ = self.clear_screenshot_session_state();
        self.input.interaction_state.pending_screenshot_capture = Some(
            crate::compositor::interaction::state::PendingScreenshotCapture {
                monitor,
                serial,
                crop,
                output_path,
                execute_at_ms: self.now_ms(now).saturating_add(24),
            },
        );
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
            let (tx, rx) = mpsc::channel();
            let output_path = pending.output_path.clone();
            let crop = pending.crop;
            std::thread::spawn(move || {
                let result =
                    capture_crop_to_png(output_path.as_path(), crop).map(|_| output_path.clone());
                let _ = tx.send(result);
            });
            self.input.interaction_state.inflight_screenshot_capture = Some(
                crate::compositor::interaction::state::InflightScreenshotCapture {
                    monitor: pending.monitor,
                    serial: pending.serial,
                    rx,
                },
            );
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
                    self.input.interaction_state.last_screenshot_result = Some(
                        crate::compositor::interaction::state::ScreenshotCaptureResult {
                            serial: inflight.serial,
                            saved_path: Some(path.clone()),
                            error: None,
                        },
                    );
                    let message = format!("Saved screenshot\n{}", path.display());
                    self.ui.render_state.show_overlay_toast(
                        inflight.monitor.as_str(),
                        message.as_str(),
                        4200,
                        now_ms,
                    );
                }
                Ok(Err(err)) => {
                    self.input.interaction_state.last_screenshot_result = Some(
                        crate::compositor::interaction::state::ScreenshotCaptureResult {
                            serial: inflight.serial,
                            saved_path: None,
                            error: Some(err.clone()),
                        },
                    );
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
                    self.input.interaction_state.last_screenshot_result = Some(
                        crate::compositor::interaction::state::ScreenshotCaptureResult {
                            serial: inflight.serial,
                            saved_path: None,
                            error: Some("capture worker disconnected".to_string()),
                        },
                    );
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
}
