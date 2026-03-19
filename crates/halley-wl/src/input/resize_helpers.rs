use std::time::Instant;

use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use crate::animation::active_surface_render_scale;
use crate::interaction::types::{ResizeCtx, ResizeHandle};
use crate::render::world_to_screen;
use crate::state::HalleyWlState;

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

pub(crate) fn active_node_screen_rect(
    st: &HalleyWlState,
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
    // This gives us (rx, ry, rw, rh) which is the screen rect of the visible
    // window content — identical to what the render path puts in geometry_rect.
    let xform = active_node_surface_transform_screen_details(st, w, h, node_id, now, None)?;
    let local_geo = active_node_visual_local_rect(st, node_id).or_else(|| {
        st.field.node(node_id).map(|n| {
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
    // Replicate the integer-division centering from the render path:
    //   rx = cx - (rw / 2)  where rw is already i32 there.
    // xform.origin_x = sx = rx - (gx * scale).round(), so rx = origin_x + (gx * scale).round()
    let rx = xform.origin_x + (gx * xform.scale).round();
    let ry = xform.origin_y + (gy * xform.scale).round();
    Some((rx, ry, rx + rw, ry + rh))
}

/// Compute the screen-space surface-tree origin and scale for an active node,
/// matching exactly the placement used by the render path.
///
/// The render path centers on `local_geo` (window geometry, excluding CSD
/// shadows) using integer division for the half-width, then derives the
/// surface-tree origin as:
///
///   sx = rx - (gx * render_scale).round()
///
/// We replicate that here so hit-testing and focus-ring placement are pixel-
/// perfect at any zoom level.
///
/// `resize_preview` must be forwarded from the caller so that during
/// interactive resize the focus origin stays in sync with where the window is
/// actually rendered (which uses preview coordinates and scale=1.0).
///
pub(crate) fn active_node_surface_transform_screen_details(
    st: &HalleyWlState,
    w: i32,
    h: i32,
    node_id: halley_core::field::NodeId,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Option<ActiveNodeSurfaceTransformScreen> {
    let n = st.field.node(node_id)?;
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
    ) * cam_scale;

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

            // Use the same local_geo the render path uses: window_geometry if
            // present, otherwise the full bbox. Fall back to intrinsic_size so
            // we always have something sensible.
            let bbox_lx = st.bbox_loc.get(&node_id).copied().unwrap_or((0.0, 0.0)).0;
            let bbox_ly = st.bbox_loc.get(&node_id).copied().unwrap_or((0.0, 0.0)).1;
            let bbox_w = n.intrinsic_size.x.max(1.0);
            let bbox_h = n.intrinsic_size.y.max(1.0);
            let local_bbox = (bbox_lx, bbox_ly, bbox_w, bbox_h);
            let (gx, gy, gw, gh) = st
                .window_geometry
                .get(&node_id)
                .copied()
                .map(|(x, y, w, h)| (x, y, w.max(1.0), h.max(1.0)))
                .unwrap_or(local_bbox);

            // Mirror the render path exactly:
            //   rw = (gw * render_scale).round()  (already .max(1) but doesn't
            //        affect the center calculation meaningfully)
            //   rx = cx - (rw / 2)                <- integer division
            //   sx = rx - (gx * render_scale).round()
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
    st: &HalleyWlState,
    node_id: halley_core::field::NodeId,
    resize_preview: Option<ResizeCtx>,
) -> Option<ActiveResizeGeometryScreen> {
    let rz = resize_preview.filter(|rz| rz.node_id == node_id)?;
    let frame_left = rz.preview_left_px;
    let frame_top = rz.preview_top_px;
    let frame_right = rz.preview_right_px;
    let frame_bottom = rz.preview_bottom_px;
    // Read live committed geo from window_geometry — updated on every client
    // commit during resize via note_commit. Zero = not yet committed, callers
    // fall back to frozen local_geo in that case.
    let (live_geo_lx, live_geo_ly, live_geo_w, live_geo_h) = st
        .window_geometry
        .get(&node_id)
        .copied()
        .unwrap_or((0.0, 0.0, 0.0, 0.0));
    Some(ActiveResizeGeometryScreen {
        frame_left,
        frame_top,
        frame_right,
        frame_bottom,
        surface_origin_x: frame_left - rz.start_geo_lx.round(),
        surface_origin_y: frame_top - rz.start_geo_ly.round(),
        live_geo_lx,
        live_geo_ly,
        live_geo_w,
        live_geo_h,
    })
}

fn active_node_visual_local_rect(
    st: &HalleyWlState,
    node_id: halley_core::field::NodeId,
) -> Option<(f32, f32, f32, f32)> {
    if let Some(&(x, y, w, h)) = st.window_geometry.get(&node_id) {
        return Some((x, y, w.max(1.0), h.max(1.0)));
    }

    for top in st.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.surface_to_node.get(&key).copied() != Some(node_id) {
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
