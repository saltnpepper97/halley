use std::collections::HashMap;
use std::error::Error;
use std::time::Instant;

use smithay::{
    backend::renderer::{
        Color32F, Frame,
        element::{Kind, surface::render_elements_from_surface_tree, utils::CropRenderElement},
        gles::{GlesRenderer, GlesTexture},
    },
    desktop::{PopupManager, utils::bbox_from_surface_tree},
    reexports::wayland_server::Resource,
    utils::{Physical, Rectangle, Size},
};

use crate::input::active_resize_geometry_screen;
use crate::interaction::types::ResizeCtx;
use crate::state::HalleyWlState;
use crate::surface::window_geometry_for_node;

use super::anim_utils::{
    active_surface_render_scale, ease_in_out_cubic, ease_out_back, proxy_anim_scale,
};
use super::offscreen::render_surface_tree_to_texture;
use super::render_utils::{
    draw_outline_rect, draw_rect, node_marker_bounds, node_marker_metrics, preview_proxy_size,
    sync_node_size_from_surface, world_to_screen,
};

type SurfaceElement =
    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>;
type CroppedSurfaceElement = CropRenderElement<SurfaceElement>;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Snapshot of per-node data captured before any mutable frame calls so that
/// node iteration and drawing stay in separate, borrow-clean passes.
pub(crate) struct NodeSnapshot {
    pub id: halley_core::field::NodeId,
    pub state: halley_core::field::NodeState,
    pub pos: halley_core::field::Vec2,
    pub intrinsic_size: halley_core::field::Vec2,
    pub label: String,
}

pub(crate) struct ActiveBorderRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub focused: bool,
}

pub(crate) struct OffscreenNodeTexture {
    pub texture: GlesTexture,
    pub src_x: i32,
    pub src_y: i32,
    pub src_w: i32,
    pub src_h: i32,
    pub dst_x: i32,
    pub dst_y: i32,
    pub dst_w: i32,
    pub dst_h: i32,
    pub clip_x: i32,
    pub clip_y: i32,
    pub clip_w: i32,
    pub clip_h: i32,
    pub focused: bool,
}

fn rect_from_local_geometry(
    origin_x: i32,
    origin_y: i32,
    scale: f32,
    local_rect: (f32, f32, f32, f32),
) -> (i32, i32, i32, i32) {
    let (local_x, local_y, local_w, local_h) = local_rect;
    (
        origin_x + (local_x * scale).round() as i32,
        origin_y + (local_y * scale).round() as i32,
        (local_w * scale).round().max(1.0) as i32,
        (local_h * scale).round().max(1.0) as i32,
    )
}

// ---------------------------------------------------------------------------
// Active surface collection
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
pub(crate) fn collect_active_surfaces(
    renderer: &mut GlesRenderer,
    st: &mut HalleyWlState,
    size: Size<i32, Physical>,
    resize_preview: Option<ResizeCtx>,
    now: Instant,
) -> (
    Vec<CroppedSurfaceElement>,
    Vec<CroppedSurfaceElement>,
    Vec<OffscreenNodeTexture>,
    Vec<CroppedSurfaceElement>,
    HashMap<
        halley_core::field::NodeId,
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    >,
    Vec<ActiveBorderRect>,
    Vec<(i32, i32, i32, i32, Color32F)>,
    Vec<(i32, i32, Color32F)>,
    Vec<(i32, i32, i32, i32)>,
) {
    let mut active_elements: Vec<CroppedSurfaceElement> = Vec::new();
    let mut resized_active_elements: Vec<CroppedSurfaceElement> = Vec::new();
    let mut offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut popup_elements: Vec<CroppedSurfaceElement> = Vec::new();
    let mut node_surface_map = HashMap::new();
    let mut border_rects: Vec<ActiveBorderRect> = Vec::new();
    let mut overlay_rects: Vec<(i32, i32, i32, i32, Color32F)> = Vec::new();
    let mut overlay_points: Vec<(i32, i32, Color32F)> = Vec::new();
    let mut overlap_overlay_rects: Vec<(i32, i32, i32, i32)> = Vec::new();

    let recent_top_node = st.recent_top_node_active(now);
    let output_clip = Rectangle::<i32, Physical>::new((0, 0).into(), size);

    let resize_rect_px = resize_preview.map(|rz| {
        (
            rz.preview_left_px.min(rz.preview_right_px).round() as i32,
            rz.preview_top_px.min(rz.preview_bottom_px).round() as i32,
            rz.preview_left_px.max(rz.preview_right_px).round() as i32,
            rz.preview_top_px.max(rz.preview_bottom_px).round() as i32,
            rz.node_id,
        )
    });

    let mut wl_surfaces: Vec<_> = st
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .filter_map(|t| {
            let wl = t.wl_surface().clone();
            let key = wl.id();
            let node_id = st.surface_to_node.get(&key).copied()?;
            node_surface_map.insert(node_id, wl.clone());
            Some((node_id, wl))
        })
        .collect();

    wl_surfaces.sort_by_key(|(id, _)| std::cmp::Reverse(id.as_u64()));

    for (node_id, wl) in wl_surfaces {
        let bbox = if resize_preview.is_some_and(|rz| rz.node_id == node_id) {
            bbox_from_surface_tree(&wl, (0, 0))
        } else {
            sync_node_size_from_surface(st, node_id, &wl)
        };

        let Some(node) = st.field.node(node_id) else {
            continue;
        };
        if node.state != halley_core::field::NodeState::Active || !st.field.is_visible(node_id) {
            continue;
        }

        let node_pos = node.pos;
        let node_state = node.state.clone();
        let node_intrinsic = node.intrinsic_size;
        let transition_alpha = st.active_transition_alpha(node_id, now);
        let anim = st.anim_style_for(node_id, node_state, now);
        let active_resize = active_resize_geometry_screen(st, node_id, resize_preview);
        let resizing_this_node = active_resize.is_some();
        let draw_top_this_node = resizing_this_node || recent_top_node == Some(node_id);

        let (scale, live_ramp) = if draw_top_this_node {
            (1.0f32, 1.0f32)
        } else {
            let s = active_surface_render_scale(
                anim.scale,
                st.active_zoom_lock_scale(),
                node_intrinsic.x,
                node_intrinsic.y,
                transition_alpha,
            );
            let live_t = ((anim.scale - 0.44) / (1.0 - 0.44)).clamp(0.0, 1.0);
            let live_ramp = if transition_alpha > 0.0 {
                ease_out_back((1.0 - transition_alpha).clamp(0.0, 1.0), 1.42).clamp(0.0, 1.08)
            } else {
                ease_in_out_cubic(live_t).clamp(0.0, 1.0)
            };
            (s, live_ramp)
        };

        let cam_scale = st.camera_render_scale();
        let render_scale = scale * cam_scale;

        let p = node_pos;
        let local_bbox = (
            bbox.loc.x as f32,
            bbox.loc.y as f32,
            bbox.size.w.max(1) as f32,
            bbox.size.h.max(1) as f32,
        );

        let local_geo = window_geometry_for_node(st, node_id).unwrap_or(local_bbox);

        let (cx, cy, sx, sy, texture_rect, geometry_rect) =
            if let Some(active_resize) = active_resize {
                let (cx, cy) = active_resize.center_px();
                let (surface_origin_x, surface_origin_y) = active_resize.surface_origin_px();
                let frame = active_resize.frame_rect_px();
                (cx, cy, surface_origin_x, surface_origin_y, frame, frame)
            } else {
                let (cx, cy) = world_to_screen(st, size.w, size.h, p.x, p.y);

                let (gx, gy, gw, gh) = local_geo;
                let rw = (gw * render_scale).round().max(1.0) as i32;
                let rh = (gh * render_scale).round().max(1.0) as i32;
                let rx = cx - (rw / 2);
                let ry = cy - (rh / 2);

                let sx = rx - (gx * render_scale).round() as i32;
                let sy = ry - (gy * render_scale).round() as i32;

                let texture_rect = rect_from_local_geometry(sx, sy, render_scale, local_bbox);
                let geometry_rect = (rx, ry, rw, rh);

                (cx, cy, sx, sy, texture_rect, geometry_rect)
            };

        let element_scale = if active_resize.is_some() {
            scale
        } else {
            render_scale
        };

        if st.tuning.dev_enabled && st.tuning.dev_show_geometry_overlay {
            let (nx0, ny0, nw, nh) = geometry_rect;
            overlay_rects.push((nx0, ny0, nw, nh, Color32F::new(0.15, 0.85, 0.85, 0.95)));
            overlay_rects.push((nx0, ny0, nw, nh, Color32F::new(0.95, 0.25, 0.85, 0.95)));
            overlay_points.push((cx, cy, Color32F::new(0.98, 0.92, 0.22, 0.95)));
        }

        let (gx, gy, gw, gh) = geometry_rect;
        // During resize: border follows the committed texture size, not the
        // preview frame. This keeps border and texture in sync at all zoom levels.
        // live_geo_w/h is in logical px; scale to screen px with cam_scale.
        let (border_x, border_y, border_w, border_h) = if let Some(rz) = active_resize
            && rz.live_geo_w > 0.0
        {
            let lw = (rz.live_geo_w * cam_scale).round() as i32;
            let lh = (rz.live_geo_h * cam_scale).round() as i32;
            (gx, gy, lw.max(1), lh.max(1))
        } else {
            (gx, gy, gw.max(1), gh.max(1))
        };
        border_rects.push(ActiveBorderRect {
            x: border_x,
            y: border_y,
            w: border_w,
            h: border_h,
            focused: st.interaction_focus == Some(node_id),
        });

        if let Some((rl, rt, rr, rb, rid)) = resize_rect_px
            && node_id != rid
        {
            let wl2 = gx;
            let wt = gy;
            let wr = gx + gw.max(1);
            let wb = gy + gh.max(1);
            if wl2 < rr && rl < wr && wt < rb && rt < wb {
                overlap_overlay_rects.push((gx, gy, gw.max(1), gh.max(1)));
            }
        }

        let alpha = (anim.alpha * live_ramp).clamp(0.0, 1.0);
        let use_offscreen_zoom = (cam_scale - 1.0).abs() > 0.001;

        if use_offscreen_zoom {
            match render_surface_tree_to_texture(renderer, &wl, alpha) {
                Ok(offscreen) => {
                    let ob = offscreen.bbox;

                    // src = full bbox, dst = bbox scaled to screen positioned so geo
                    // lands on frame, clip = frame rect to discard CSD shadow bleed.
                    let (src_x, src_y, src_w, src_h, dst_x, dst_y, dst_w, dst_h, clip_x, clip_y, clip_w, clip_h) =
                        if let Some(active_resize) = active_resize {
                            let (fx, fy, fw, fh) = active_resize.frame_rect_px();
                            // Use live committed geo (updated on every client commit)
                            // as the single source of truth. Falls back to frozen
                            // local_geo before the first commit after resize starts.
                            let (live_lx, live_ly, live_gw, live_gh): (f32, f32, f32, f32) =
                                if active_resize.live_geo_w > 0.0 {
                                    (
                                        active_resize.live_geo_lx,
                                        active_resize.live_geo_ly,
                                        active_resize.live_geo_w,
                                        active_resize.live_geo_h,
                                    )
                                } else {
                                    // Before first commit: use frozen start geo from ResizeCtx.
                                    // local_geo may have stale data; start_geo_lx/ly is reliable.
                                    let rz = resize_preview.unwrap();
                                    (rz.start_geo_lx, rz.start_geo_ly,
                                     local_geo.2, local_geo.3)
                                };
                            // Crop src to just the geo region — excludes the CSD
                            // shadow pixels (transparent black) at bbox edges which
                            // would otherwise blit over the border strips.
                            // src coords are in logical texture pixels (1:1 with surface).
                            let src_x = (live_lx.round() as i32) - ob.loc.x;
                            let src_y = (live_ly.round() as i32) - ob.loc.y;
                            let src_w = live_gw.round() as i32;
                            let src_h = live_gh.round() as i32;
                            let clip_w = (live_gw * cam_scale).round() as i32;
                            let clip_h = (live_gh * cam_scale).round() as i32;
                            (
                                src_x.max(0), src_y.max(0), src_w.max(1), src_h.max(1),
                                fx, fy, clip_w.max(1).min(fw), clip_h.max(1).min(fh),
                                fx, fy, clip_w.max(1).min(fw), clip_h.max(1).min(fh),
                            )
                        } else {
                            let src_x = (local_geo.0.round() as i32) - ob.loc.x;
                            let src_y = (local_geo.1.round() as i32) - ob.loc.y;
                            let src_w = local_geo.2.round().max(1.0) as i32;
                            let src_h = local_geo.3.round().max(1.0) as i32;
                            (src_x, src_y, src_w, src_h, gx, gy, gw.max(1), gh.max(1),
                             gx, gy, gw.max(1), gh.max(1))
                        };

                    offscreen_textures.push(OffscreenNodeTexture {
                        texture: offscreen.texture,
                        src_x, src_y, src_w, src_h,
                        dst_x, dst_y, dst_w, dst_h,
                        clip_x, clip_y, clip_w, clip_h,
                        focused: st.interaction_focus == Some(node_id),
                    });
                }
                Err(_) => {
                    let elems = render_elements_from_surface_tree(
                        renderer,
                        &wl,
                        (sx, sy),
                        element_scale as f64,
                        alpha,
                        Kind::Unspecified,
                    );

                    let (tx, ty, tw, th) = texture_rect;
                    let display_clip = Rectangle::<i32, Physical>::new(
                        (tx, ty).into(),
                        (tw.max(1), th.max(1)).into(),
                    );

                    let cropped: Vec<_> = elems
                        .into_iter()
                        .filter_map(|e| CropRenderElement::from_element(e, 1.0, display_clip))
                        .collect();

                    if draw_top_this_node {
                        resized_active_elements.extend(cropped);
                    } else {
                        active_elements.extend(cropped);
                    }
                }
            }
        } else {
            let elems = render_elements_from_surface_tree(
                renderer,
                &wl,
                (sx, sy),
                element_scale as f64,
                alpha,
                Kind::Unspecified,
            );

            let (tx, ty, tw, th) = texture_rect;
            let display_clip =
                Rectangle::<i32, Physical>::new((tx, ty).into(), (tw.max(1), th.max(1)).into());

            let cropped: Vec<_> = elems
                .into_iter()
                .filter_map(|e| CropRenderElement::from_element(e, 1.0, display_clip))
                .collect();

            if draw_top_this_node {
                resized_active_elements.extend(cropped);
            } else {
                active_elements.extend(cropped);
            }
        }

        let parent_geo = window_geometry_for_node(st, node_id).unwrap_or((
            0.0,
            0.0,
            node_intrinsic.x.max(1.0),
            node_intrinsic.y.max(1.0),
        ));
        let parent_geo_loc = (parent_geo.0.round() as i32, parent_geo.1.round() as i32);
        let mut popup_cropped = Vec::new();
        let mut popups: Vec<_> = PopupManager::popups_for_surface(&wl).collect();
        popups.reverse();
        for (popup, popup_offset) in popups {
            let popup_geo = popup.geometry();
            let popup_sx = sx
                + ((parent_geo_loc.0 + popup_offset.x - popup_geo.loc.x) as f32 * element_scale)
                    .round() as i32;
            let popup_sy = sy
                + ((parent_geo_loc.1 + popup_offset.y - popup_geo.loc.y) as f32 * element_scale)
                    .round() as i32;
            let popup_elems = render_elements_from_surface_tree(
                renderer,
                popup.wl_surface(),
                (popup_sx, popup_sy),
                element_scale as f64,
                alpha,
                Kind::Unspecified,
            );
            popup_cropped.extend(
                popup_elems
                    .into_iter()
                    .filter_map(|e| CropRenderElement::from_element(e, 1.0, output_clip)),
            );
        }

        popup_elements.extend(popup_cropped);
    }

    (
        active_elements,
        resized_active_elements,
        offscreen_textures,
        popup_elements,
        node_surface_map,
        border_rects,
        overlay_rects,
        overlay_points,
        overlap_overlay_rects,
    )
}

// ---------------------------------------------------------------------------
// Hover preview collection
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
pub(crate) fn collect_hover_preview(
    renderer: &mut GlesRenderer,
    st: &mut HalleyWlState,
    size: Size<i32, Physical>,
    node_surface_map: &HashMap<
        halley_core::field::NodeId,
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    >,
    hovered_preview_id: Option<halley_core::field::NodeId>,
    hover_node: Option<halley_core::field::NodeId>,
    now: Instant,
) -> (
    Option<(i32, i32, i32, i32)>,
    Vec<smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>>,
) {
    let _ = hover_node;

    let Some((preview_id, preview_mix_raw)) = st.node_preview_hover_anim(hovered_preview_id) else {
        return (None, Vec::new());
    };
    let Some(wl) = node_surface_map.get(&preview_id) else {
        return (None, Vec::new());
    };
    let Some((node_state, node_pos, label_len)) = st
        .field
        .node(preview_id)
        .map(|n| (n.state.clone(), n.pos, n.label.len()))
    else {
        return (None, Vec::new());
    };

    if !matches!(
        node_state,
        halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
    ) {
        return (None, Vec::new());
    }

    let bbox = bbox_from_surface_tree(wl, (0, 0));
    if bbox.size.w <= 0 || bbox.size.h <= 0 {
        return (None, Vec::new());
    }

    let preview_mix = ease_in_out_cubic(preview_mix_raw.clamp(0.0, 1.0));
    let anim = st.anim_style_for(preview_id, node_state.clone(), now);

    const PROXY_TO_MARKER_START: f32 = 0.50;
    const PROXY_TO_MARKER_END: f32 = 0.20;
    let marker_mix_lin = ((PROXY_TO_MARKER_START - anim.scale)
        / (PROXY_TO_MARKER_START - PROXY_TO_MARKER_END))
        .clamp(0.0, 1.0);
    let marker_mix = ease_in_out_cubic(marker_mix_lin);

    let p = node_pos;
    let _ = marker_mix;
    let (cx, cy) = world_to_screen(st, size.w, size.h, p.x, p.y);

    let (dot_half_raw, label_gap, mut label_w, mut label_h) =
        node_marker_metrics(st, label_len, anim.scale);
    let dot_half = ((dot_half_raw as f32) * 1.36).round() as i32;
    label_w = ((label_w as f32) * 1.22).round() as i32;
    label_h = ((label_h as f32) * 1.22).round() as i32;
    let min_h = dot_half * 2;
    label_h = label_h.max(min_h);
    let visual_h = (dot_half * 2).max(label_h);
    let render_pad = 8;
    let (bx, by, bw, bh) =
        node_marker_bounds(cx, cy, dot_half, label_gap, label_w, visual_h, render_pad);

    let mut preview_size_base = ((size.w.min(size.h) as f32) * 0.30).round() as i32;
    preview_size_base = preview_size_base.clamp(220, 360);
    let inset = 10i32;
    let source_side = bbox.size.w.max(bbox.size.h).max(1);
    let base_side = (source_side + inset * 2).clamp(120, preview_size_base);
    let preview_size = ((base_side as f32) * (0.94 + 0.06 * preview_mix))
        .round()
        .max(120.0) as i32;

    let anchor_cx = bx + (bw / 2);
    let anchor_cy = by + (bh / 2);
    let mut preview_x = anchor_cx - (preview_size / 2);
    let mut preview_y = anchor_cy - (preview_size / 2);
    preview_x = preview_x.clamp(10, (size.w - preview_size - 10).max(10));
    preview_y = preview_y.clamp(10, (size.h - preview_size - 10).max(10));

    let sx = preview_x + inset - bbox.loc.x;
    let sy = preview_y + inset - bbox.loc.y;
    let alpha = (preview_mix * preview_mix).clamp(0.0, 1.0);

    let elements =
        render_elements_from_surface_tree(renderer, wl, (sx, sy), 1.0f64, alpha, Kind::Unspecified);

    (
        Some((preview_x, preview_y, preview_size, preview_size)),
        elements,
    )
}

// ---------------------------------------------------------------------------
// Node marker drawing
// ---------------------------------------------------------------------------

pub(crate) fn draw_node_markers<F>(
    frame: &mut F,
    st: &mut HalleyWlState,
    size: Size<i32, Physical>,
    render_nodes: &[NodeSnapshot],
    hover_node: Option<halley_core::field::NodeId>,
    damage: Rectangle<i32, Physical>,
    now: Instant,
) -> Result<(), Box<dyn Error>>
where
    F: Frame,
    F::Error: std::error::Error + 'static,
{
    for NodeSnapshot {
        id,
        state: node_state,
        pos: node_pos,
        intrinsic_size,
        label: node_label,
    } in render_nodes
    {
        let id = *id;
        let node_pos = *node_pos;
        let intrinsic_size = *intrinsic_size;

        let anim = st.anim_style_for(id, node_state.clone(), now);

        if !matches!(
            node_state,
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
        ) {
            continue;
        }

        let p_smooth = node_pos;

        const PROXY_TO_MARKER_START: f32 = 0.50;
        const PROXY_TO_MARKER_END: f32 = 0.20;
        let marker_mix_lin = ((PROXY_TO_MARKER_START - anim.scale)
            / (PROXY_TO_MARKER_START - PROXY_TO_MARKER_END))
            .clamp(0.0, 1.0);
        let marker_mix = ease_in_out_cubic(marker_mix_lin);
        let proxy_mix = 1.0 - marker_mix;

        let p = halley_core::field::Vec2 {
            x: p_smooth.x + (node_pos.x - p_smooth.x) * marker_mix,
            y: p_smooth.y + (node_pos.y - p_smooth.y) * marker_mix,
        };
        let (sx, sy) = world_to_screen(st, size.w, size.h, p.x, p.y);

        let (dot_half_raw, mut label_gap, mut label_w, mut label_h) =
            node_marker_metrics(st, node_label.len(), anim.scale);

        let hover_mix = ease_in_out_cubic(st.node_label_hover_mix(id, hover_node == Some(id)));
        label_gap += (8.0 * hover_mix).round() as i32;
        label_w = ((label_w as f32) * (1.0 + 1.25 * hover_mix)).round() as i32;
        label_h = ((label_h as f32) * (1.0 + 0.95 * hover_mix)).round() as i32;
        let dot_half = ((dot_half_raw as f32) * 1.36).round() as i32;
        label_w = ((label_w as f32) * 1.22).round() as i32;
        label_h = ((label_h as f32) * 1.22).round() as i32;
        let min_h = ((dot_half * 2) as f32 + 8.0 * hover_mix).round() as i32;
        label_h = label_h.max(min_h);
        let visual_h = (dot_half * 2).max(label_h);
        let render_pad = (8.0 + 6.0 * hover_mix).round() as i32;
        let (bx, by, bw, bh) =
            node_marker_bounds(sx, sy, dot_half, label_gap, label_w, visual_h, render_pad);

        let mut container_x = bx;
        let mut container_y = by;
        let mut container_w = bw;
        let mut container_h = bh;
        let label_w_px = label_w;
        let label_h_px = visual_h;

        if proxy_mix > 0.01 {
            let proxy_t = ((anim.scale - 0.30) / (0.62 - 0.30)).clamp(0.0, 1.0);
            let proxy_alpha = (anim.alpha * proxy_t * proxy_t * proxy_mix * proxy_mix * proxy_mix)
                .clamp(0.0, 1.0);

            let (pw, ph) = preview_proxy_size(intrinsic_size.x, intrinsic_size.y);
            let s = proxy_anim_scale(anim.scale);
            let pw = pw * s;
            let ph = ph * s;
            let px = p.x - pw * 0.5;
            let py = p.y - ph * 0.5;
            let qx = p.x + pw * 0.5;
            let qy = p.y + ph * 0.5;
            let (sx0, sy0) = world_to_screen(st, size.w, size.h, px, py);
            let (sx1, sy1) = world_to_screen(st, size.w, size.h, qx, qy);
            let p0x = sx0.min(sx1);
            let p0y = sy0.min(sy1);
            let p0w = (sx0.max(sx1) - p0x).max(8);
            let p0h = (sy0.max(sy1) - p0y).max(8);

            let x = ((p0x as f32) + ((bx - p0x) as f32) * marker_mix).round() as i32;
            let y = ((p0y as f32) + ((by - p0y) as f32) * marker_mix).round() as i32;
            let w = ((p0w as f32) + ((bw - p0w) as f32) * marker_mix)
                .round()
                .max(8.0) as i32;
            let h = ((p0h as f32) + ((bh - p0h) as f32) * marker_mix)
                .round()
                .max(8.0) as i32;

            container_x = x;
            container_y = y;
            container_w = w;
            container_h = h;

            draw_rect(
                frame,
                x,
                y,
                w,
                h,
                Color32F::new(0.18, 0.18, 0.18, 0.58 * proxy_alpha),
                damage,
            )?;
            draw_outline_rect(
                frame,
                x,
                y,
                w,
                h,
                Color32F::new(1.0, 1.0, 1.0, 0.62 * proxy_alpha),
                damage,
            )?;
        }

        let dot_alpha = (anim.alpha * marker_mix).clamp(0.0, 1.0);
        if dot_alpha <= 0.01 {
            continue;
        }

        let inner = (8.0 + 3.0 * hover_mix).round() as i32;
        let dot_d = dot_half * 2;
        let mut gap_px = label_gap.max(8);
        let min_label_w = 10;

        let content_left_limit = container_x + inner;
        let content_right_limit = container_x + container_w - inner;
        let content_available_w =
            (content_right_limit - content_left_limit).max(dot_d + min_label_w);
        let content_cx = container_x + (container_w / 2);

        let desired_content_w = dot_d + gap_px + label_w_px;
        if desired_content_w > content_available_w {
            let overflow = desired_content_w - content_available_w;
            let reducible_gap = gap_px.saturating_sub(4);
            let gap_cut = overflow.min(reducible_gap);
            gap_px -= gap_cut;
        }

        let max_label_w = (content_available_w - dot_d - gap_px).max(min_label_w);
        let label_draw_w = label_w_px.min(max_label_w);

        let final_content_w = dot_d + gap_px + label_draw_w;
        let content_x = content_cx - (final_content_w / 2);

        let dot_x = content_x;
        let dot_y = container_y + ((container_h - dot_d) / 2);

        let label_x = dot_x + dot_d + gap_px;
        let label_y = container_y + ((container_h - label_h_px) / 2);

        draw_rect(
            frame,
            label_x,
            label_y,
            label_draw_w,
            label_h_px,
            Color32F::new(1.0, 1.0, 1.0, 0.88 * dot_alpha),
            damage,
        )?;
        draw_rect(
            frame,
            dot_x,
            dot_y,
            dot_d,
            dot_d,
            Color32F::new(0.88, 0.88, 0.88, 0.80 * dot_alpha),
            damage,
        )?;
    }
    Ok(())
}
