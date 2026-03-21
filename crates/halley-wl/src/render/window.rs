use std::collections::HashMap;
use std::time::Instant;

use smithay::{
    backend::renderer::{
        Color32F,
        element::{Kind, surface::render_elements_from_surface_tree, utils::CropRenderElement},
        gles::{GlesRenderer, GlesTexture},
    },
    desktop::{PopupManager, utils::bbox_from_surface_tree},
    reexports::wayland_server::Resource,
    utils::{Physical, Rectangle, Size},
};

use crate::animation::{active_surface_render_scale, ease_in_out_cubic, ease_out_back};
use crate::input::active_resize_geometry_screen;
use crate::interaction::types::ResizeCtx;
use crate::state::HalleyWlState;
use crate::surface::window_geometry_for_node;

use super::offscreen::render_surface_tree_to_texture;
use super::utils::{sync_node_size_from_surface, world_to_screen};

type SurfaceElement =
    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>;
type CroppedSurfaceElement = CropRenderElement<SurfaceElement>;
pub(crate) struct ActiveBorderRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub focused: bool,
}

pub(crate) struct OffscreenNodeTexture {
    pub texture: GlesTexture,
    pub alpha: f32,
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
    Vec<OffscreenNodeTexture>,
    Vec<OffscreenNodeTexture>,
    Vec<CroppedSurfaceElement>,
    HashMap<
        halley_core::field::NodeId,
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    >,
    Vec<ActiveBorderRect>,
    Vec<ActiveBorderRect>,
    Vec<(i32, i32, i32, i32, Color32F)>,
    Vec<(i32, i32, Color32F)>,
    Vec<(i32, i32, i32, i32)>,
) {
    let mut active_elements: Vec<CroppedSurfaceElement> = Vec::new();
    let mut resized_active_elements: Vec<CroppedSurfaceElement> = Vec::new();
    let mut offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut resized_offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut popup_offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut popup_elements: Vec<CroppedSurfaceElement> = Vec::new();
    let mut node_surface_map = HashMap::new();
    let mut border_rects: Vec<ActiveBorderRect> = Vec::new();
    let mut resized_border_rects: Vec<ActiveBorderRect> = Vec::new();
    let mut overlay_rects: Vec<(i32, i32, i32, i32, Color32F)> = Vec::new();
    let mut overlay_points: Vec<(i32, i32, Color32F)> = Vec::new();
    let mut overlap_overlay_rects: Vec<(i32, i32, i32, i32)> = Vec::new();

    let recent_top_node = st.recent_top_node_active(now);
    let output_clip = Rectangle::<i32, Physical>::new((0, 0).into(), size);

    let resize_rect_px = resize_preview.and_then(|rz| {
        if !st.node_visible_on_current_monitor(rz.node_id) {
            return None;
        }
        Some((
            rz.preview_left_px.min(rz.preview_right_px).round() as i32,
            rz.preview_top_px.min(rz.preview_bottom_px).round() as i32,
            rz.preview_left_px.max(rz.preview_right_px).round() as i32,
            rz.preview_top_px.max(rz.preview_bottom_px).round() as i32,
            rz.node_id,
        ))
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
        if node.state != halley_core::field::NodeState::Active
            || !st.field.is_visible(node_id)
            || !st.node_visible_on_current_monitor(node_id)
        {
            continue;
        }

        let node_pos = node.pos;
        let node_state = node.state.clone();
        let node_intrinsic = node.intrinsic_size;
        let transition_alpha = st.active_transition_alpha(node_id, now);
        let anim = st.anim_style_for(node_id, node_state, now);
        let fullscreen_entry_scale = st.fullscreen_entry_scale(node_id, st.now_ms(now));
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
            (s * fullscreen_entry_scale, live_ramp)
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
        let border_rect = ActiveBorderRect {
            x: border_x,
            y: border_y,
            w: border_w,
            h: border_h,
            focused: st.interaction_focus == Some(node_id),
        };
        if draw_top_this_node {
            resized_border_rects.push(border_rect);
        } else {
            border_rects.push(border_rect);
        }

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
            let cache_miss = {
                let cache =
                    st.ensure_window_offscreen_cache(node_id, bbox.size.w, bbox.size.h, now);
                cache.dirty || cache.texture.is_none() || cache.bbox.is_none()
            };

            if cache_miss {
                match render_surface_tree_to_texture(renderer, &wl, 1.0) {
                    Ok(offscreen) => {
                        let cache = st
                            .window_offscreen_cache
                            .get_mut(&node_id)
                            .expect("offscreen cache should exist after ensure");
                        cache.texture = Some(offscreen.texture);
                        cache.bbox = Some(offscreen.bbox);
                        cache.mark_clean(now);
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
                        continue;
                    }
                }
            }

            if let Some(cache) = st.window_offscreen_cache.get_mut(&node_id) {
                cache.touch(now);
            }

            match st.window_offscreen_cache.get(&node_id) {
                Some(cache) => {
                    let Some(texture) = cache.texture.as_ref() else {
                        continue;
                    };
                    let Some(ob) = cache.bbox else {
                        continue;
                    };

                    // src = full bbox, dst = bbox scaled to screen positioned so geo
                    // lands on frame, clip = frame rect to discard CSD shadow bleed.
                    let (
                        src_x,
                        src_y,
                        src_w,
                        src_h,
                        dst_x,
                        dst_y,
                        dst_w,
                        dst_h,
                        clip_x,
                        clip_y,
                        clip_w,
                        clip_h,
                    ) = if let Some(active_resize) = active_resize {
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
                                (rz.start_geo_lx, rz.start_geo_ly, local_geo.2, local_geo.3)
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
                            src_x.max(0),
                            src_y.max(0),
                            src_w.max(1),
                            src_h.max(1),
                            fx,
                            fy,
                            clip_w.max(1).min(fw),
                            clip_h.max(1).min(fh),
                            fx,
                            fy,
                            clip_w.max(1).min(fw),
                            clip_h.max(1).min(fh),
                        )
                    } else {
                        let src_x = (local_geo.0.round() as i32) - ob.loc.x;
                        let src_y = (local_geo.1.round() as i32) - ob.loc.y;
                        let src_w = local_geo.2.round().max(1.0) as i32;
                        let src_h = local_geo.3.round().max(1.0) as i32;
                        (
                            src_x,
                            src_y,
                            src_w,
                            src_h,
                            gx,
                            gy,
                            gw.max(1),
                            gh.max(1),
                            gx,
                            gy,
                            gw.max(1),
                            gh.max(1),
                        )
                    };

                    let offscreen = OffscreenNodeTexture {
                        texture: texture.clone(),
                        alpha,
                        src_x,
                        src_y,
                        src_w,
                        src_h,
                        dst_x,
                        dst_y,
                        dst_w,
                        dst_h,
                        clip_x,
                        clip_y,
                        clip_w,
                        clip_h,
                    };
                    if draw_top_this_node {
                        resized_offscreen_textures.push(offscreen);
                    } else {
                        offscreen_textures.push(offscreen);
                    }
                }
                None => continue,
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
            if use_offscreen_zoom {
                match render_surface_tree_to_texture(renderer, popup.wl_surface(), alpha) {
                    Ok(offscreen) => {
                        let src_x = 0;
                        let src_y = 0;
                        let src_w = offscreen.bbox.size.w.max(1);
                        let src_h = offscreen.bbox.size.h.max(1);
                        let dst_x =
                            popup_sx + (offscreen.bbox.loc.x as f32 * element_scale).round() as i32;
                        let dst_y =
                            popup_sy + (offscreen.bbox.loc.y as f32 * element_scale).round() as i32;
                        let dst_w = (offscreen.bbox.size.w as f32 * element_scale)
                            .round()
                            .max(1.0) as i32;
                        let dst_h = (offscreen.bbox.size.h as f32 * element_scale)
                            .round()
                            .max(1.0) as i32;
                        popup_offscreen_textures.push(OffscreenNodeTexture {
                            texture: offscreen.texture,
                            alpha,
                            src_x,
                            src_y,
                            src_w,
                            src_h,
                            dst_x,
                            dst_y,
                            dst_w,
                            dst_h,
                            clip_x: output_clip.loc.x,
                            clip_y: output_clip.loc.y,
                            clip_w: output_clip.size.w,
                            clip_h: output_clip.size.h,
                        });
                    }
                    Err(_) => {
                        let popup_elems = render_elements_from_surface_tree(
                            renderer,
                            popup.wl_surface(),
                            (popup_sx, popup_sy),
                            element_scale as f64,
                            alpha,
                            Kind::Unspecified,
                        );
                        popup_cropped.extend(
                            popup_elems.into_iter().filter_map(|e| {
                                CropRenderElement::from_element(e, 1.0, output_clip)
                            }),
                        );
                    }
                }
            } else {
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
        }

        popup_elements.extend(popup_cropped);
    }

    (
        active_elements,
        resized_active_elements,
        offscreen_textures,
        resized_offscreen_textures,
        popup_offscreen_textures,
        popup_elements,
        node_surface_map,
        border_rects,
        resized_border_rects,
        overlay_rects,
        overlay_points,
        overlap_overlay_rects,
    )
}
