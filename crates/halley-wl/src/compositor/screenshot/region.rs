use std::time::Instant;

use halley_capit::CaptureCrop;
use halley_core::field::NodeId;
use halley_ipc::CaptureMode;

use crate::compositor::root::Halley;
use crate::compositor::screenshot::state::{ScreenshotRegionDragMode, ScreenshotRegionResizeDir};
use crate::compositor::surface::active_stacking_visible_members_for_monitor;

pub(super) const SCREENSHOT_HANDLE_SIZE: i32 = 12;
pub(super) const SCREENSHOT_HANDLE_HIT: i32 = 14;
pub(super) const SCREENSHOT_MIN_W: i32 = 8;
pub(super) const SCREENSHOT_MIN_H: i32 = 8;

pub(super) fn screenshot_desktop_bounds(st: &Halley) -> (i32, i32, i32, i32) {
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

pub(super) fn screenshot_window_crop_for_node(
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

pub(super) fn initial_screenshot_selection(
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

pub(super) fn screenshot_menu_modes() -> [CaptureMode; 3] {
    [
        CaptureMode::Region,
        CaptureMode::Screen,
        CaptureMode::Window,
    ]
}

pub(super) fn screenshot_menu_index(mode: CaptureMode) -> Option<usize> {
    screenshot_menu_modes()
        .iter()
        .position(|candidate| *candidate == mode)
}

pub(super) fn screenshot_session_monitor(st: &Halley, output: Option<&str>) -> String {
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
) -> Option<ScreenshotRegionResizeDir> {
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
        0 => ScreenshotRegionResizeDir {
            left: true,
            right: false,
            top: true,
            bottom: false,
        },
        1 => ScreenshotRegionResizeDir {
            left: false,
            right: true,
            top: true,
            bottom: false,
        },
        2 => ScreenshotRegionResizeDir {
            left: true,
            right: false,
            top: false,
            bottom: true,
        },
        _ => ScreenshotRegionResizeDir {
            left: false,
            right: true,
            top: false,
            bottom: true,
        },
    })
}

pub(super) fn screenshot_region_hit_test(
    selection: CaptureCrop,
    px: i32,
    py: i32,
) -> ScreenshotRegionDragMode {
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

pub(super) fn screenshot_region_apply_drag(
    drag_mode: ScreenshotRegionDragMode,
    cursor: (i32, i32),
    grab_cursor: (i32, i32),
    grab_rect: CaptureCrop,
    bounds: (i32, i32, i32, i32),
) -> CaptureCrop {
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
