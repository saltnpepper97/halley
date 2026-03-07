use std::time::Instant;

use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_server::Resource;

use crate::interaction::types::{ResizeCtx, ResizeHandle};
use crate::runtime_render::{active_surface_render_scale, world_to_screen};
use crate::state::HalleyWlState;

#[derive(Clone, Copy)]
pub(crate) struct ActiveNodeSurfaceTransformScreen {
    pub(crate) origin_x: f32,
    pub(crate) origin_y: f32,
    pub(crate) scale: f32,
    pub(crate) bbox_offset_x: f32,
    pub(crate) bbox_offset_y: f32,
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
    st: &mut HalleyWlState,
    w: i32,
    h: i32,
    node_id: halley_core::field::NodeId,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Option<(f32, f32, f32, f32)> {
    let xform =
        active_node_surface_transform_screen_details(st, w, h, node_id, now, resize_preview)?;
    let n = st.field.node(node_id)?;
    let mut bbox_w = n.intrinsic_size.x;
    let mut bbox_h = n.intrinsic_size.y;
    for top in st.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
        }
        let bbox = bbox_from_surface_tree(&wl, (0, 0));
        bbox_w = bbox.size.w.max(1) as f32;
        bbox_h = bbox.size.h.max(1) as f32;
        break;
    }
    let sw = (bbox_w * xform.scale).round();
    let sh = (bbox_h * xform.scale).round();
    let left = xform.origin_x + xform.bbox_offset_x;
    let top = xform.origin_y + xform.bbox_offset_y;
    Some((left, top, left + sw, top + sh))
}

/// Compute the screen-space surface-tree origin and scale for an active node,
/// matching exactly the placement used by the render path.
///
/// `resize_preview` must be forwarded from the caller so that during
/// interactive resize the focus origin stays in sync with where the window is
/// actually rendered (which uses preview coordinates and scale=1.0), rather
/// than diverging from the smoothed-position + anim-scale path.
///
/// TODO: bbox_loc (the cached bbox.loc from the last render frame) should be
/// read from `st.bbox_loc.get(&node_id)` instead of re-calling
/// `bbox_from_surface_tree` here, to prevent mid-frame subsurface commits
/// (e.g. a video layer) from shifting origin_x/y between render time and
/// input-handling time. Add `pub bbox_loc: HashMap<NodeId, (f32, f32)>` to
/// HalleyWlState and populate it in `collect_active_surfaces`.
pub(crate) fn active_node_surface_transform_screen_details(
    st: &mut HalleyWlState,
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
    let anim_scale = active_surface_render_scale(
        anim.scale,
        st.active_zoom_lock_scale(),
        n.intrinsic_size.x,
        n.intrinsic_size.y,
        transition_alpha,
    );

    let mut bbox_w = n.intrinsic_size.x;
    let mut bbox_h = n.intrinsic_size.y;
    let mut bbox_lx = 0.0f32;
    let mut bbox_ly = 0.0f32;
    for top in st.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
        }
        let bbox = bbox_from_surface_tree(&wl, (0, 0));
        bbox_w = bbox.size.w.max(1) as f32;
        bbox_h = bbox.size.h.max(1) as f32;
        bbox_lx = bbox.loc.x as f32;
        bbox_ly = bbox.loc.y as f32;
        break;
    }

    // During interactive resize the render path uses preview edge coordinates
    // with scale=1.0 — mirror that here exactly so focus_origin stays in sync.
    let (origin_x, origin_y, scale) =
        if let Some(rz) = resize_preview.filter(|rz| rz.node_id == node_id) {
            let origin_x = match rz.handle {
                ResizeHandle::Left | ResizeHandle::TopLeft | ResizeHandle::BottomLeft => {
                    rz.preview_right_px.round() - bbox_w - bbox_lx
                }
                ResizeHandle::Right
                | ResizeHandle::TopRight
                | ResizeHandle::BottomRight
                | ResizeHandle::Top
                | ResizeHandle::Bottom => rz.preview_left_px.round() - bbox_lx,
            };
            let origin_y = match rz.handle {
                ResizeHandle::Top | ResizeHandle::TopLeft | ResizeHandle::TopRight => {
                    rz.preview_bottom_px.round() - bbox_h - bbox_ly
                }
                ResizeHandle::Bottom
                | ResizeHandle::BottomLeft
                | ResizeHandle::BottomRight
                | ResizeHandle::Left
                | ResizeHandle::Right => rz.preview_top_px.round() - bbox_ly,
            };
            (origin_x, origin_y, 1.0f32)
        } else {
            let p = st.smoothed_render_pos_read(node_id, n.pos, now); // read-only: keeps focus origin in sync with hit-test position
            let (cx, cy) = world_to_screen(st, w, h, p.x, p.y);
            let sw = (bbox_w * anim_scale).round();
            let sh = (bbox_h * anim_scale).round();
            let lx = (bbox_lx * anim_scale).round();
            let ly = (bbox_ly * anim_scale).round();
            (
                (cx as f32) - sw * 0.5 - lx,
                (cy as f32) - sh * 0.5 - ly,
                anim_scale,
            )
        };

    Some(ActiveNodeSurfaceTransformScreen {
        origin_x,
        origin_y,
        scale: scale.max(0.001),
        bbox_offset_x: (bbox_lx * scale).round(),
        bbox_offset_y: (bbox_ly * scale).round(),
    })
}
