use std::collections::HashMap;
use std::error::Error;
use std::time::Instant;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
        Color32F, Texture,
        element::{Kind, surface::render_elements_from_surface_tree, utils::CropRenderElement},
        gles::{GlesFrame, GlesRenderer, GlesTexture, Uniform, UniformName, UniformType},
        ImportMem,
    },
    },
    desktop::{PopupManager, utils::bbox_from_surface_tree},
    reexports::wayland_server::Resource,
    utils::{Buffer, Physical, Rectangle, Size, Transform},
};

use crate::input::active_resize_geometry_screen;
use crate::interaction::types::ResizeCtx;
use crate::state::HalleyWlState;
use crate::surface::window_geometry_for_node;
use halley_config::{NodeBackgroundColorMode, NodeBorderColorMode, NodeDisplayPolicy};

use crate::animation::{
    active_surface_render_scale, ease_in_out_cubic, ease_out_back,
};
use super::offscreen::render_surface_tree_to_texture;
use super::render_utils::{
    bitmap_text_size, draw_bitmap_text, draw_rounded_rect, node_marker_bounds,
    node_marker_metrics, node_render_diameter_px, sync_node_size_from_surface, world_to_screen,
};

type SurfaceElement =
    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>;
type CroppedSurfaceElement = CropRenderElement<SurfaceElement>;

const NODE_CIRCLE_SHADER: &str = r#"
precision mediump float;
//_DEFINES

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform vec4 node_color;
uniform vec4 fill_color;

void main() {
    vec2 p = v_coords * 2.0 - 1.0;
    vec2 a = abs(p);
    float dist = pow(pow(a.x, 4.0) + pow(a.y, 4.0), 0.25);
    if (dist > 1.0) { discard; }

    // node_color.a encodes border width as a fraction of radius.
    float border_w    = node_color.a;
    float border_edge = 1.0 - border_w;

    // Hard binary split — no blending between fill and border zones.
    // Any mix() here bleeds fill color into the border and vice-versa,
    // which shows as a lighter inner rim on dark borders especially at
    // small sizes where border_w is a large fraction of the disc.
    float in_border = step(border_edge, dist);

    // Shared light direction for both zones.
    vec2 light_dir = normalize(vec2(-0.55, -0.65));

    // --- fill ---
    float light = dot(p, light_dir) * 0.5 + 0.5;
    light = light * 0.55 + 0.225;
    vec3 shaded_fill = mix(
        mix(fill_color.rgb, vec3(0.0), 0.10),
        mix(fill_color.rgb, vec3(1.0), 0.12),
        light
    );
    // Inner shadow: fixed pixel-width ramp so it stays a thin rim at all sizes.
    float shadow_w     = min(border_w * 0.5, 0.06);
    float shadow_t     = smoothstep(border_edge - shadow_w, border_edge, dist);
    // Mask strictly to fill zone with step — not (1-in_border) which is 0..1.
    float fill_mask    = 1.0 - in_border;
    shaded_fill = mix(shaded_fill, vec3(0.0), shadow_t * fill_mask * 0.13);

    // --- border ---
    float border_light = dot(p, light_dir) * 0.5 + 0.5;
    border_light = border_light * 0.55 + 0.225;
    vec3 shaded_border = mix(
        mix(node_color.rgb, vec3(0.0), 0.10),
        mix(node_color.rgb, vec3(1.0), 0.10),
        border_light
    );

    // Select zone with hard step, AA only at the outer disc edge.
    vec3 color = mix(shaded_fill, shaded_border, in_border);
    float edge_aa = 1.0 - smoothstep(0.96, 1.0, dist);
    float final_alpha = alpha * edge_aa;
    gl_FragColor = vec4(color * final_alpha, final_alpha);
}
"#;

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

pub(crate) fn ensure_node_circle_resources(
    renderer: &mut GlesRenderer,
    st: &mut HalleyWlState,
) -> Result<(), Box<dyn Error>> {
    if st.node_circle_texture.is_none() {
        const TEX_SIZE: usize = 4;
        let pixel = vec![255u8; TEX_SIZE * TEX_SIZE * 4];
        st.node_circle_texture = Some(renderer.import_memory(
            &pixel,
            Fourcc::Abgr8888,
            (TEX_SIZE as i32, TEX_SIZE as i32).into(),
            false,
        )?);
    }

    if st.node_circle_program.is_none() {
        st.node_circle_program = Some(renderer.compile_custom_texture_shader(
            NODE_CIRCLE_SHADER,
            &[
                UniformName::new("node_color", UniformType::_4f),
                UniformName::new("fill_color", UniformType::_4f),
            ],
        )?);
    }

    Ok(())
}

fn draw_shader_circle(
    frame: &mut GlesFrame<'_, '_>,
    st: &HalleyWlState,
    cx: i32,
    cy: i32,
    radius: i32,
    alpha: f32,
    border_color: Color32F,
    fill_color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let Some(texture) = st.node_circle_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = st.node_circle_program.as_ref() else {
        return Ok(());
    };

    let radius = radius.max(1);
    let diameter = (radius * 2).max(1);
    let dest =
        Rectangle::<i32, Physical>::new((cx - radius, cy - radius).into(), (diameter, diameter).into());
    let tex_size = texture.size();
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (tex_size.w as f64, tex_size.h as f64).into(),
    );
    let uniforms = [
        Uniform::new("node_color", (border_color.r(), border_color.g(), border_color.b(), border_color.a())),
        Uniform::new("fill_color",  (fill_color.r(),  fill_color.g(),  fill_color.b(),  fill_color.a())),
    ];

    frame.render_texture_from_to(
        texture,
        src,
        dest,
        &[damage],
        &[],
        Transform::Normal,
        alpha.clamp(0.0, 1.0),
        Some(program),
        &uniforms,
    )?;

    Ok(())
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

fn window_active_border_color() -> Color32F {
    Color32F::new(0.22, 0.82, 0.92, 1.0)
}

fn window_inactive_border_color() -> Color32F {
    Color32F::new(0.28, 0.30, 0.35, 1.0)
}

fn node_ring_color(st: &HalleyWlState, hovered: bool, alpha: f32) -> Color32F {
    let mode = if hovered {
        st.tuning.node_border_color_hover
    } else {
        st.tuning.node_border_color_inactive
    };
    let base = match mode {
        NodeBorderColorMode::UseWindowActive => window_active_border_color(),
        NodeBorderColorMode::UseWindowInactive => window_inactive_border_color(),
    };
    Color32F::new(base.r(), base.g(), base.b(), alpha)
}

fn node_fill_color(st: &HalleyWlState, hovered: bool) -> Color32F {
    match st.tuning.node_background_color {
        NodeBackgroundColorMode::Auto | NodeBackgroundColorMode::Theme => {
            let ring = node_ring_color(st, hovered, 1.0);
            let base = (0.94, 0.96, 0.985);
            Color32F::new(
                base.0 * 0.86 + ring.r() * 0.14,
                base.1 * 0.86 + ring.g() * 0.14,
                base.2 * 0.86 + ring.b() * 0.14,
                1.0,
            )
        }
        NodeBackgroundColorMode::Fixed { r, g, b } => Color32F::new(r, g, b, 1.0),
    }
}

fn node_icon_glyph(st: &HalleyWlState, id: halley_core::field::NodeId, fallback: &str) -> Option<char> {
    st.node_app_ids
        .get(&id)
        .map(String::as_str)
        .unwrap_or(fallback)
        .chars()
        .find(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase())
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

                    offscreen_textures.push(OffscreenNodeTexture {
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
                    });
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

    let (dot_half, _, _, _) = node_marker_metrics(st, label_len, anim.scale);
    let render_pad = 8;
    let (bx, by, bw, bh) = node_marker_bounds(cx, cy, dot_half, 0, 0, dot_half * 2, render_pad);

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

pub(crate) fn draw_node_markers(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut HalleyWlState,
    size: Size<i32, Physical>,
    render_nodes: &[NodeSnapshot],
    hover_node: Option<halley_core::field::NodeId>,
    damage: Rectangle<i32, Physical>,
    now: Instant,
) -> Result<(), Box<dyn Error>>
{
    const NODE_ICON_FADE_DELAY_MS: u64 = 1000;
    const NODE_ICON_FADE_MS: u64 = 220;

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
        let hovered = hover_node == Some(id);
        let hover_mix = ease_in_out_cubic(st.node_label_hover_mix(id, hovered));
        let border_mix = ease_in_out_cubic(((0.304 - anim.scale) / 0.004).clamp(0.0, 1.0));
        let icon_mix = st
            .anim_track_elapsed_for(id, node_state.clone(), now)
            .map(|elapsed| {
                let elapsed_ms = elapsed.as_millis() as u64;
                let fade_t = elapsed_ms.saturating_sub(NODE_ICON_FADE_DELAY_MS) as f32
                    / NODE_ICON_FADE_MS as f32;
                ease_in_out_cubic(fade_t.clamp(0.0, 1.0))
            })
            .unwrap_or(0.0);

        let (dot_half, _, _, _) = node_marker_metrics(st, node_label.len(), anim.scale);
        let render_radius = (dot_half as f32 * 1.5).round() as i32;

        if proxy_mix > 0.01 && border_mix < 0.99 {
            let diameter =
                node_render_diameter_px(st, intrinsic_size, node_label.len(), anim.scale).round()
                    as i32;
            let proxy_radius = (diameter / 2).max(dot_half);
            let proxy_col = Color32F::new(0.84, 0.89, 0.95, 0.0);
            draw_shader_circle(
                frame, st,
                sx, sy, proxy_radius, 1.0 - border_mix,
                proxy_col, proxy_col,
                damage,
            )?;
        }

        let dot_alpha = (anim.alpha * marker_mix).clamp(0.0, 1.0);
        if dot_alpha <= 0.01 {
            continue;
        }

        if border_mix > 0.01 {
            // border_frac = 3px border expressed as a fraction of the radius
            let border_frac = (3.0 / render_radius as f32).clamp(0.01, 0.5);
            let nc = node_ring_color(st, hover_mix > 0.02, 1.0);
            // node_color.rgb = border ring colour; .a = border_px / radius
            // fill_color.rgb  = node fill colour (inner fill + outer halo)
            let node_color  = Color32F::new(nc.r(), nc.g(), nc.b(), border_frac);
            let fill_color  = node_fill_color(st, hovered);
            draw_shader_circle(
                frame,
                st,
                sx,
                sy,
                render_radius,
                border_mix,
                node_color,
                fill_color,
                damage,
            )?;
        }

        let show_icon = match st.tuning.node_show_app_icons {
            NodeDisplayPolicy::Off => false,
            NodeDisplayPolicy::Hover => hovered,
            NodeDisplayPolicy::Always => true,
        };
        if show_icon
        {
            let icon_alpha = (dot_alpha * icon_mix).clamp(0.0, 1.0);
            let mut drew_real_icon = false;
            if icon_alpha > 0.01
                && let Some(app_id) = st.node_app_ids.get(&id)
                && let Some(crate::state::NodeAppIconCacheEntry::Ready(icon)) =
                    st.node_app_icon_cache.get(app_id)
            {
                let side = ((render_radius * 2) as f32 * st.tuning.node_icon_size)
                    .round() as i32;
                let side = side.clamp(16, 42);
                let dest = Rectangle::<i32, Physical>::new(
                    (sx - side / 2, sy - side / 2).into(),
                    (side, side).into(),
                );
                let src = Rectangle::<f64, Buffer>::new(
                    (0.0, 0.0).into(),
                    (icon.width as f64, icon.height as f64).into(),
                );
                frame.render_texture_from_to(
                    &icon.texture,
                    src,
                    dest,
                    &[damage],
                    &[],
                    Transform::Normal,
                    icon_alpha,
                    None,
                    &[],
                )?;
                drew_real_icon = true;
            }

            if !drew_real_icon
                && icon_alpha > 0.01
                && let Some(icon) = node_icon_glyph(st, id, node_label)
            {
                let scale = if render_radius >= 24 { 3 } else { 2 };
                let icon_text = icon.to_string();
                let (tw, th) = bitmap_text_size(&icon_text, scale);
                let text_x = sx - (tw / 2);
                let text_y = sy - (th / 2);
                draw_bitmap_text(
                    frame,
                    text_x,
                    text_y,
                    &icon_text,
                    scale,
                    Color32F::new(0.18, 0.21, 0.26, 0.92 * icon_alpha),
                    damage,
                )?;
            }
        }
    }
    Ok(())
}

pub(crate) fn draw_node_hover_labels(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut HalleyWlState,
    size: Size<i32, Physical>,
    render_nodes: &[NodeSnapshot],
    hover_node: Option<halley_core::field::NodeId>,
    damage: Rectangle<i32, Physical>,
    now: Instant,
) -> Result<(), Box<dyn Error>>
{
    if st.tuning.node_show_labels == NodeDisplayPolicy::Off {
        return Ok(());
    }

    for node in render_nodes {
        if !matches!(
            node.state,
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
        ) {
            continue;
        }

        let anim = st.anim_style_for(node.id, node.state.clone(), now);
        let dot_alpha = (anim.alpha
            * ease_in_out_cubic(
                ((0.50 - anim.scale) / (0.50 - 0.20)).clamp(0.0, 1.0),
            ))
        .clamp(0.0, 1.0);
        if dot_alpha <= 0.01 {
            continue;
        }

        let hover_mix = match st.tuning.node_show_labels {
            NodeDisplayPolicy::Off => 0.0,
            NodeDisplayPolicy::Hover => {
                st.node_label_hover_mix(node.id, hover_node == Some(node.id))
            }
            NodeDisplayPolicy::Always => 1.0,
        };
        // cube the hover_mix so the whole animation is back-loaded — nothing
        // happens until well into the hover, then it rushes in
        let reveal_mix = ease_in_out_cubic(hover_mix * hover_mix * hover_mix);
        let label_fade = ((reveal_mix - 0.30) / 0.55).clamp(0.0, 1.0);
        if label_fade <= 0.01 {
            continue;
        }
        let label_slide = ((reveal_mix - 0.15) / 0.65).clamp(0.0, 1.0);
        let label_grow = ((reveal_mix - 0.40) / 0.55).clamp(0.0, 1.0);

        let (sx, sy) = world_to_screen(st, size.w, size.h, node.pos.x, node.pos.y);
        let (dot_half, base_label_gap, base_label_w, base_label_h) =
            node_marker_metrics(st, node.label.len(), anim.scale);
        let label_gap = ((base_label_gap as f32) * (1.0 + 0.45 * label_grow)).round() as i32;
        let label_w_target =
            ((((base_label_w as f32) * 1.80).round() as i32 + 1) & !1).clamp(72, 240);
        // Round to even so label_h / 2 centering is always exact — odd dims cause
        // a 0.5px vertical drift that steps jaggedly each animation frame.
        let label_w = ((((base_label_w as f32) * (1.0 + 0.80 * label_grow)).round() as i32 + 1) & !1)
            .clamp(72, 240);
        let label_h = (((base_label_h as f32) * (1.0 + 0.55 * label_grow)).round() as i32 + 1) & !1;

        let margin = 12;
        let side_gap = dot_half + label_gap.max(10);
        let prefer_left = sx + side_gap + label_w_target + margin > size.w;
        let label_x_target = if prefer_left {
            sx - side_gap - label_w
        } else {
            sx + side_gap
        };
        let label_x_start = if prefer_left {
            label_x_target + 44
        } else {
            label_x_target - 44
        };
        let label_x = ((label_x_start as f32)
            + ((label_x_target - label_x_start) as f32) * label_slide)
            .round() as i32;
        let label_y_target = sy - (label_h / 2);
        let label_y = (label_y_target as f32 + (1.0 - label_slide) * 10.0).round() as i32;

        let final_x = label_x.clamp(margin, (size.w - label_w - margin).max(margin));
        let final_y = label_y.clamp(margin, (size.h - label_h - margin).max(margin));

        draw_rounded_rect(
            frame,
            final_x,
            final_y,
            label_w.max(1),
            label_h.max(1),
            8,
            Color32F::new(0.96, 0.98, 1.0, 0.88 * dot_alpha * label_fade),
            damage,
        )?;

        let text_scale = 2;
        let char_advance = 5 * text_scale + text_scale;
        let max_chars = ((label_w - 20).max(0) / char_advance).max(1) as usize;
        let mut text = node.label.to_ascii_uppercase();
        if text.chars().count() > max_chars {
            let keep = max_chars.saturating_sub(3);
            text = text.chars().take(keep).collect::<String>();
            text.push_str("...");
        }
        let (text_w, text_h) = bitmap_text_size(&text, text_scale);
        let text_x = final_x + ((label_w - text_w).max(0) / 2);
        let text_y = final_y + ((label_h - text_h).max(0) / 2);
        draw_bitmap_text(
            frame,
            text_x,
            text_y,
            &text,
            text_scale,
            Color32F::new(0.16, 0.18, 0.22, 0.94 * dot_alpha * label_fade),
            damage,
        )?;
    }

    Ok(())
}
