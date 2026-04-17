use smithay::input::pointer::CursorIcon;

use crate::compositor::interaction::ResizeHandle;

pub(crate) fn cursor_icon_for_resize_handle(handle: ResizeHandle) -> CursorIcon {
    match handle {
        ResizeHandle::Pending => CursorIcon::Crosshair,
        ResizeHandle::Left => CursorIcon::WResize,
        ResizeHandle::Right => CursorIcon::EResize,
        ResizeHandle::Top => CursorIcon::NResize,
        ResizeHandle::Bottom => CursorIcon::SResize,
        ResizeHandle::TopLeft => CursorIcon::NwResize,
        ResizeHandle::TopRight => CursorIcon::NeResize,
        ResizeHandle::BottomLeft => CursorIcon::SwResize,
        ResizeHandle::BottomRight => CursorIcon::SeResize,
    }
}

/// Pick a resize handle from the nearest edge/corner to the press point.
/// Only called for direct border grabs (press within edge slop zone).
#[allow(dead_code)]
pub(crate) fn pick_resize_handle_from_screen(
    rect: (f32, f32, f32, f32),
    p: (f32, f32),
    edge_slop: f32,
) -> ResizeHandle {
    let (l, t, r, b) = rect;
    let dl = (p.0 - l).abs();
    let dr = (r - p.0).abs();
    let dt = (p.1 - t).abs();
    let db = (b - p.1).abs();
    let edge_slop = edge_slop.max(0.0);
    let near_left = dl <= edge_slop;
    let near_right = dr <= edge_slop;
    let near_top = dt <= edge_slop;
    let near_bottom = db <= edge_slop;

    if near_left && near_top {
        return ResizeHandle::TopLeft;
    }
    if near_right && near_top {
        return ResizeHandle::TopRight;
    }
    if near_left && near_bottom {
        return ResizeHandle::BottomLeft;
    }
    if near_right && near_bottom {
        return ResizeHandle::BottomRight;
    }

    let min_d = dl.min(dr).min(dt).min(db);
    if (min_d - dl).abs() <= f32::EPSILON {
        ResizeHandle::Left
    } else if (min_d - dr).abs() <= f32::EPSILON {
        ResizeHandle::Right
    } else if (min_d - dt).abs() <= f32::EPSILON {
        ResizeHandle::Top
    } else {
        ResizeHandle::Bottom
    }
}

/// Commit a resize handle from where the pointer pressed within the window,
/// using a 3×3 grid split at the 1/3 and 2/3 fractional positions.
pub(crate) fn handle_from_press_position(
    rect: (f32, f32, f32, f32),
    p: (f32, f32),
) -> ResizeHandle {
    let (l, t, r, b) = rect;
    let w = (r - l).max(1.0);
    let h = (b - t).max(1.0);
    let fx = ((p.0 - l) / w).clamp(0.0, 1.0);
    let fy = ((p.1 - t) / h).clamp(0.0, 1.0);

    #[derive(PartialEq)]
    enum Z {
        Near,
        Mid,
        Far,
    }
    let hz = if fx < 1.0 / 3.0 {
        Z::Near
    } else if fx < 2.0 / 3.0 {
        Z::Mid
    } else {
        Z::Far
    };
    let vz = if fy < 1.0 / 3.0 {
        Z::Near
    } else if fy < 2.0 / 3.0 {
        Z::Mid
    } else {
        Z::Far
    };

    match (hz, vz) {
        (Z::Near, Z::Near) => ResizeHandle::TopLeft,
        (Z::Mid, Z::Near) => ResizeHandle::Top,
        (Z::Far, Z::Near) => ResizeHandle::TopRight,
        (Z::Near, Z::Mid) => ResizeHandle::Left,
        (Z::Mid, Z::Mid) => {
            let dl = p.0 - l;
            let dr = r - p.0;
            let dt = p.1 - t;
            let db = b - p.1;
            let min_d = dl.min(dr).min(dt).min(db);
            if (min_d - dl).abs() <= f32::EPSILON {
                ResizeHandle::Left
            } else if (min_d - dr).abs() <= f32::EPSILON {
                ResizeHandle::Right
            } else if (min_d - dt).abs() <= f32::EPSILON {
                ResizeHandle::Top
            } else {
                ResizeHandle::Bottom
            }
        }
        (Z::Far, Z::Mid) => ResizeHandle::Right,
        (Z::Near, Z::Far) => ResizeHandle::BottomLeft,
        (Z::Mid, Z::Far) => ResizeHandle::Bottom,
        (Z::Far, Z::Far) => ResizeHandle::BottomRight,
    }
}

#[allow(dead_code)]
pub(crate) fn press_is_near_edge(
    rect: (f32, f32, f32, f32),
    p: (f32, f32),
    edge_slop: f32,
) -> bool {
    let (l, t, r, b) = rect;
    let edge_slop = edge_slop.max(0.0);
    (p.0 - l).abs() <= edge_slop
        || (r - p.0).abs() <= edge_slop
        || (p.1 - t).abs() <= edge_slop
        || (b - p.1).abs() <= edge_slop
}

#[allow(dead_code)]
pub(crate) fn commit_handle_from_drag(dx: f32, dy: f32) -> ResizeHandle {
    let adx = dx.abs();
    let ady = dy.abs();
    let right = dx >= 0.0;
    let down = dy >= 0.0;

    if ady < adx / 2.0 {
        if right {
            ResizeHandle::Right
        } else {
            ResizeHandle::Left
        }
    } else if adx < ady / 2.0 {
        if down {
            ResizeHandle::Bottom
        } else {
            ResizeHandle::Top
        }
    } else {
        match (right, down) {
            (true, true) => ResizeHandle::BottomRight,
            (true, false) => ResizeHandle::TopRight,
            (false, true) => ResizeHandle::BottomLeft,
            (false, false) => ResizeHandle::TopLeft,
        }
    }
}

pub(crate) fn weights_from_handle(handle: ResizeHandle) -> (f32, f32, f32, f32) {
    match handle {
        ResizeHandle::Left => (1.0, 0.0, 0.0, 0.0),
        ResizeHandle::Right => (0.0, 1.0, 0.0, 0.0),
        ResizeHandle::Top => (0.0, 0.0, 1.0, 0.0),
        ResizeHandle::Bottom => (0.0, 0.0, 0.0, 1.0),
        ResizeHandle::TopLeft => (1.0, 0.0, 1.0, 0.0),
        ResizeHandle::TopRight => (0.0, 1.0, 1.0, 0.0),
        ResizeHandle::BottomLeft => (1.0, 0.0, 0.0, 1.0),
        ResizeHandle::BottomRight => (0.0, 1.0, 0.0, 1.0),
        ResizeHandle::Pending => (0.0, 0.0, 0.0, 0.0),
    }
}
