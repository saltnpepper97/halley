use std::time::Instant;

use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use crate::animation::active_surface_render_scale;
use crate::interaction::types::{ResizeCtx, ResizeHandle};
use crate::render::world_to_screen;
use crate::state::Halley;

#[derive(Clone, Copy)]
pub(crate) struct ActiveNodeSurfaceTransformScreen {
    pub(crate) origin_x: f32,
    pub(crate) origin_y: f32,
    pub(crate) scale: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct ActiveResizeGeometryScreen {
    pub(crate) frame_left: f32,
    pub(crate) frame_top: f32,
    pub(crate) frame_right: f32,
    pub(crate) frame_bottom: f32,
    pub(crate) surface_origin_x: f32,
    pub(crate) surface_origin_y: f32,

    /// Live committed window geometry from st.window_geometry, updated on
    /// every client commit during resize. Zero means not yet committed.
    pub(crate) live_geo_lx: f32,
    pub(crate) live_geo_ly: f32,
    pub(crate) live_geo_w: f32,
    pub(crate) live_geo_h: f32,
}

impl ActiveResizeGeometryScreen {
    pub(crate) fn frame_rect_px(self) -> (i32, i32, i32, i32) {
        let left = self.frame_left.round() as i32;
        let top = self.frame_top.round() as i32;
        let right = self.frame_right.round() as i32;
        let bottom = self.frame_bottom.round() as i32;
        (left, top, (right - left).max(1), (bottom - top).max(1))
    }

    pub(crate) fn surface_origin_px(self) -> (i32, i32) {
        (
            self.surface_origin_x.round() as i32,
            self.surface_origin_y.round() as i32,
        )
    }

    pub(crate) fn center_px(self) -> (i32, i32) {
        (
            ((self.frame_left + self.frame_right) * 0.5).round() as i32,
            ((self.frame_top + self.frame_bottom) * 0.5).round() as i32,
        )
    }
}

/// Pick a resize handle from the nearest edge/corner to the press point.
/// Only called for direct border grabs (press within edge slop zone).
#[allow(dead_code)]
pub(crate) fn pick_resize_handle_from_screen(
    rect: (f32, f32, f32, f32),
    p: (f32, f32),
) -> ResizeHandle {
    let (l, t, r, b) = rect;
    let dl = (p.0 - l).abs();
    let dr = (r - p.0).abs();
    let dt = (p.1 - t).abs();
    let db = (b - p.1).abs();
    let edge_slop = 28.0f32;
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
/// using a 3×3 grid split at the 1/3 and 2/3 fractional positions:
///
///   fx:   0..1/3     1/3..2/3    2/3..1
///        ┌──────────┬──────────┬──────────┐
///  0..   │ TopLeft  │   Top    │ TopRight │
/// 1/3    ├──────────┼──────────┼──────────┤
///  1/3.. │  Left    │ nearest  │  Right   │
///  2/3   ├──────────┼──────────┼──────────┤
///  2/3.. │BotLeft   │  Bottom  │ BotRight │
///  1     └──────────┴──────────┴──────────┘
///
/// Pressing near top-left and dragging any direction pulls the top-left corner.
/// The centre cell falls back to whichever edge is nearest.
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
            // Centre: nearest edge
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

/// Returns `true` if the press point is within the edge slop zone of `rect`.
#[allow(dead_code)]
pub(crate) fn press_is_near_edge(rect: (f32, f32, f32, f32), p: (f32, f32)) -> bool {
    let (l, t, r, b) = rect;
    let edge_slop = 28.0f32;
    (p.0 - l).abs() <= edge_slop
        || (r - p.0).abs() <= edge_slop
        || (p.1 - t).abs() <= edge_slop
        || (b - p.1).abs() <= edge_slop
}

/// Commit a resize handle from the drag vector `(dx, dy)` using an octant
/// split with a 2:1 aspect-ratio threshold:
///
///   |dy| < |dx| / 2  →  Left or Right   (wide horizontal band)
///   |dx| < |dy| / 2  →  Top or Bottom   (wide vertical band)
///   otherwise         →  corner quadrant
///
/// `dx` positive = rightward, `dy` positive = downward (screen space).
/// Never returns `Pending`.
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

/// Map a committed handle to its four signed edge weights
/// `(h_weight_left, h_weight_right, v_weight_top, v_weight_bottom)`.
///
/// The preview rect is updated each frame as:
///
///   new_left   = start_left   + h_weight_left  * dx
///   new_right  = start_right  + h_weight_right * dx
///   new_top    = start_top    + v_weight_top   * dy
///   new_bottom = start_bottom + v_weight_bottom * dy
///
/// Weight semantics:
///   +1.0  — this edge tracks the pointer directly (right/bottom moving edges)
///   -1.0  — this edge moves opposite to the pointer (left/top moving edges,
///            so that dragging right on the left border moves it right = shrink,
///            and dragging left moves it left = grow, as expected)
///    0.0  — this edge is anchored and does not move
///
/// Both weights being 0.0 on an axis means that axis is not resized at all
/// (e.g. a pure Left/Right grab does not change the window height).
pub(crate) fn weights_from_handle(handle: ResizeHandle) -> (f32, f32, f32, f32) {
    // (h_left, h_right, v_top, v_bottom)
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

pub(crate) fn active_node_screen_rect(
    st: &Halley,
    w: i32,
    h: i32,
    node_id: halley_core::field::NodeId,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Option<(f32, f32, f32, f32)> {
    if let Some(active_resize) = active_resize_geometry_screen(st, node_id, resize_preview) {
        return Some((
            active_resize.frame_left,
            active_resize.frame_top,
            active_resize.frame_right,
            active_resize.frame_bottom,
        ));
    }

    // Mirror the render path exactly: center on local_geo, derive geometry_rect.
    let xform = active_node_surface_transform_screen_details(st, w, h, node_id, now, None)?;
    let local_geo = active_node_visual_local_rect(st, node_id).or_else(|| {
        st.model.field.node(node_id).map(|n| {
            (
                0.0,
                0.0,
                n.intrinsic_size.x.max(1.0),
                n.intrinsic_size.y.max(1.0),
            )
        })
    })?;

    let (gx, gy, gw, gh) = local_geo;
    let rw = (gw * xform.scale).round().max(1.0);
    let rh = (gh * xform.scale).round().max(1.0);
    let rx = xform.origin_x + (gx * xform.scale).round();
    let ry = xform.origin_y + (gy * xform.scale).round();
    Some((rx, ry, rx + rw, ry + rh))
}

/// Compute the screen-space surface-tree origin and scale for an active node,
/// matching exactly the placement used by the render path.
pub(crate) fn active_node_surface_transform_screen_details(
    st: &Halley,
    w: i32,
    h: i32,
    node_id: halley_core::field::NodeId,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Option<ActiveNodeSurfaceTransformScreen> {
    let n = st.model.field.node(node_id)?;
    if n.state != halley_core::field::NodeState::Active {
        return None;
    }

    let anim = st.anim_style_for(node_id, n.state.clone(), now);
    let transition_alpha = st.active_transition_alpha(node_id, now);
    let cam_scale = st.camera_render_scale();
    let anim_scale = active_surface_render_scale(
        anim.scale,
        st.active_zoom_lock_scale(),
        n.intrinsic_size.x,
        n.intrinsic_size.y,
        transition_alpha,
    ) * st.fullscreen_entry_scale(node_id, st.now_ms(now))
        * cam_scale;

    let (origin_x, origin_y, scale) =
        if let Some(active_resize) = active_resize_geometry_screen(st, node_id, resize_preview) {
            (
                active_resize.surface_origin_x,
                active_resize.surface_origin_y,
                1.0f32,
            )
        } else {
            let p = n.pos;
            let (cx, cy) = world_to_screen(st, w, h, p.x, p.y);

            let bbox_lx = st.ui.render_state
                .bbox_loc
                .get(&node_id)
                .copied()
                .unwrap_or((0.0, 0.0))
                .0;
            let bbox_ly = st.ui.render_state
                .bbox_loc
                .get(&node_id)
                .copied()
                .unwrap_or((0.0, 0.0))
                .1;
            let bbox_w = n.intrinsic_size.x.max(1.0);
            let bbox_h = n.intrinsic_size.y.max(1.0);
            let local_bbox = (bbox_lx, bbox_ly, bbox_w, bbox_h);
            let (gx, gy, gw, gh) = st.ui.render_state
                .window_geometry
                .get(&node_id)
                .copied()
                .map(|(x, y, w, h)| (x, y, w.max(1.0), h.max(1.0)))
                .unwrap_or(local_bbox);

            let rw = (gw * anim_scale).round() as i32;
            let rh = (gh * anim_scale).round() as i32;
            let rx = cx - (rw / 2);
            let ry = cy - (rh / 2);
            let origin_x = (rx as f32) - (gx * anim_scale).round();
            let origin_y = (ry as f32) - (gy * anim_scale).round();

            (origin_x, origin_y, anim_scale)
        };

    Some(ActiveNodeSurfaceTransformScreen {
        origin_x,
        origin_y,
        scale: scale.max(0.001),
    })
}

pub(crate) fn active_resize_geometry_screen(
    st: &Halley,
    node_id: halley_core::field::NodeId,
    resize_preview: Option<ResizeCtx>,
) -> Option<ActiveResizeGeometryScreen> {
    let rz = resize_preview.filter(|rz| rz.node_id == node_id)?;
    // While Pending the window hasn't moved yet — don't produce a preview rect.
    if rz.handle == ResizeHandle::Pending {
        return None;
    }
    let frame_left = rz.preview_left_px;
    let frame_top = rz.preview_top_px;
    let frame_right = rz.preview_right_px;
    let frame_bottom = rz.preview_bottom_px;
    let (live_geo_lx, live_geo_ly, live_geo_w, live_geo_h) = st.ui.render_state
        .window_geometry
        .get(&node_id)
        .copied()
        .unwrap_or((0.0, 0.0, 0.0, 0.0));
    let geo_lx = if live_geo_w > 0.0 {
        live_geo_lx
    } else {
        rz.start_geo_lx
    };
    let geo_ly = if live_geo_h > 0.0 {
        live_geo_ly
    } else {
        rz.start_geo_ly
    };

    Some(ActiveResizeGeometryScreen {
        frame_left,
        frame_top,
        frame_right,
        frame_bottom,
        surface_origin_x: frame_left - geo_lx.round(),
        surface_origin_y: frame_top - geo_ly.round(),
        live_geo_lx,
        live_geo_ly,
        live_geo_w,
        live_geo_h,
    })
}

fn active_node_visual_local_rect(
    st: &Halley,
    node_id: halley_core::field::NodeId,
) -> Option<(f32, f32, f32, f32)> {
    if let Some(&(x, y, w, h)) = st.ui.render_state.window_geometry.get(&node_id) {
        return Some((x, y, w.max(1.0), h.max(1.0)));
    }

    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.model.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
        }

        let geo = with_states(wl, |states| {
            states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .geometry
        });
        if let Some(g) = geo {
            return Some((
                g.loc.x as f32,
                g.loc.y as f32,
                g.size.w.max(1) as f32,
                g.size.h.max(1) as f32,
            ));
        }

        let bbox = bbox_from_surface_tree(wl, (0, 0));
        return Some((
            bbox.loc.x as f32,
            bbox.loc.y as f32,
            bbox.size.w.max(1) as f32,
            bbox.size.h.max(1) as f32,
        ));
    }

    None
}
