use super::*;

use std::io;
use std::path::Path;

use image::RgbaImage;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, Offscreen, Renderer};
use smithay::utils::{Buffer, Rectangle, Transform};

use crate::render::{
    draw_offscreen_textures, draw_window_borders, ensure_node_circle_resources,
    ensure_window_texture_program,
};

pub(super) fn render_view_for_monitor(st: &Halley, monitor: &str) -> (Vec2, Vec2, Vec2) {
    if st.model.monitor_state.current_monitor == monitor {
        return (
            st.model.viewport.center,
            st.model.viewport.size,
            st.model.zoom_ref_size,
        );
    }

    st.model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| {
            (
                space.viewport.center,
                space.viewport.size,
                space.zoom_ref_size,
            )
        })
        .unwrap_or((
            st.model.viewport.center,
            st.model.viewport.size,
            st.model.zoom_ref_size,
        ))
}

pub(super) fn world_to_screen_for_view(
    view_center: Vec2,
    view_size: Vec2,
    output_w: i32,
    output_h: i32,
    x: f32,
    y: f32,
) -> (i32, i32) {
    let vw = view_size.x.max(1.0);
    let vh = view_size.y.max(1.0);
    let nx = ((x - view_center.x) / vw) + 0.5;
    let ny = ((y - view_center.y) / vh) + 0.5;

    (
        (nx * output_w as f32).round() as i32,
        (ny * output_h as f32).round() as i32,
    )
}

pub(crate) fn capture_closing_window_animation(
    st: &Halley,
    monitor: &str,
    node_id: NodeId,
) -> Option<(Vec<ActiveBorderRect>, Vec<OffscreenNodeTexture>)> {
    let node = st.model.field.node(node_id)?;
    let cache = st
        .ui
        .render_state
        .cache
        .window_offscreen_cache
        .get(&node_id)?;
    let texture = cache.texture.clone()?;
    let ob = cache.bbox?;
    if !cache.has_content {
        return None;
    }

    let output_size = layer_output_size_for_monitor(st, monitor);
    if output_size.w <= 0 || output_size.h <= 0 {
        return None;
    }
    let output_clip = Rectangle::<i32, Physical>::new(
        (0, 0).into(),
        (output_size.w.max(1), output_size.h.max(1)).into(),
    );

    let (view_center, viewport_size, view_size) = render_view_for_monitor(st, monitor);
    let render_scale = (viewport_size.x.max(1.0) / view_size.x.max(1.0)).max(0.01);
    let local_geo = window_geometry_for_node(st, node_id).unwrap_or((
        ob.loc.x as f32,
        ob.loc.y as f32,
        ob.size.w.max(1) as f32,
        ob.size.h.max(1) as f32,
    ));
    let (cx, cy) = world_to_screen_for_view(
        view_center,
        view_size,
        output_size.w,
        output_size.h,
        node.pos.x,
        node.pos.y,
    );
    let gw = (local_geo.2 * render_scale).round().max(1.0) as i32;
    let gh = (local_geo.3 * render_scale).round().max(1.0) as i32;
    let gx = cx - (gw / 2);
    let gy = cy - (gh / 2);
    let fullscreen_on_monitor = st
        .fullscreen_monitor_for_node(node_id)
        .is_some_and(|fullscreen_monitor| fullscreen_monitor == monitor);
    let decoration_metrics = if fullscreen_on_monitor {
        window_decoration_metrics(0, 0, 0, 0)
    } else {
        window_decoration_metrics(
            scaled_window_border_px(st.runtime.tuning.window_border_radius_px(), render_scale),
            scaled_window_border_px(
                st.runtime.tuning.window_primary_border_size_px(),
                render_scale,
            ),
            scaled_window_border_px(
                st.runtime.tuning.window_secondary_border_gap_px(),
                render_scale,
            ),
            scaled_window_border_px(
                st.runtime.tuning.window_secondary_border_size_px(),
                render_scale,
            ),
        )
    };
    let preserve_visual_margin = false;
    let lock_dst_to_geometry = decoration_metrics.content_corner_radius_px > 0;
    let (src_x, src_y, src_w, src_h, dst_x, dst_y, dst_w, dst_h, clip_x, clip_y, clip_w, clip_h) =
        offscreen_visual_crop_and_dst(
            ob.loc.x,
            ob.loc.y,
            ob.size.w.max(1),
            ob.size.h.max(1),
            local_geo.0,
            local_geo.1,
            local_geo.2,
            local_geo.3,
            gx,
            gy,
            gw.max(1),
            gh.max(1),
            render_scale,
            output_clip,
            preserve_visual_margin,
            lock_dst_to_geometry,
        );
    let (geo_offset_x, geo_offset_y, geo_w_px, geo_h_px) = if lock_dst_to_geometry {
        (0.0, 0.0, dst_w.max(1) as f32, dst_h.max(1) as f32)
    } else {
        let src_scale_x = if src_w > 0.0 {
            dst_w as f32 / src_w as f32
        } else {
            1.0
        };
        let src_scale_y = if src_h > 0.0 {
            dst_h as f32 / src_h as f32
        } else {
            1.0
        };
        let geo_local_x = local_geo.0 - ob.loc.x as f32;
        let geo_local_y = local_geo.1 - ob.loc.y as f32;
        let geo_src_x = (geo_local_x - src_x as f32).max(0.0);
        let geo_src_y = (geo_local_y - src_y as f32).max(0.0);
        (
            (geo_src_x * src_scale_x).max(0.0),
            (geo_src_y * src_scale_y).max(0.0),
            (local_geo.2 * src_scale_x).min(dst_w as f32).max(1.0),
            (local_geo.3 * src_scale_y).min(dst_h as f32).max(1.0),
        )
    };
    let offscreen = OffscreenNodeTexture {
        texture,
        alpha: 1.0,
        corner_radius: decoration_metrics.content_corner_radius_px as f32,
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
        geo_offset_x,
        geo_offset_y,
        geo_w: geo_w_px,
        geo_h: geo_h_px,
    };
    let border_rects = build_window_border_rects(
        st,
        node_id,
        gx,
        gy,
        gw.max(1),
        gh.max(1),
        1.0,
        render_scale,
        fullscreen_on_monitor,
    );

    Some((border_rects, vec![offscreen]))
}

pub(crate) fn capture_window_to_png_via_renderer(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
    monitor: &str,
    node_id: NodeId,
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let previous_monitor = st.begin_temporary_render_monitor(monitor);
    let result = (|| {
        let now = Instant::now();
        ensure_node_circle_resources(renderer, st)?;
        ensure_window_texture_program(renderer, st);
        prewarm_visible_active_window_offscreen_caches(renderer, st, now);

        let (mut border_rects, mut offscreen_textures) =
            capture_closing_window_animation(st, monitor, node_id).ok_or_else(|| {
                io::Error::other(format!(
                    "unable to prepare window capture for node {} on {monitor}",
                    node_id.as_u64()
                ))
            })?;
        let bounds = window_capture_bounds(&border_rects, &offscreen_textures)
            .ok_or_else(|| io::Error::other("window capture bounds are empty"))?;

        translate_window_capture_primitives(
            &mut border_rects,
            &mut offscreen_textures,
            bounds.loc.x,
            bounds.loc.y,
            bounds.size.w,
            bounds.size.h,
        );
        let buffer_size: smithay::utils::Size<i32, Buffer> =
            (bounds.size.w.max(1), bounds.size.h.max(1)).into();

        let mut texture = <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
            renderer,
            Fourcc::Abgr8888,
            buffer_size,
        )?;
        let damage = Rectangle::from_size(bounds.size);
        {
            let mut target = renderer.bind(&mut texture)?;
            let mut frame = renderer.render(&mut target, bounds.size, Transform::Normal)?;
            frame.clear(Color32F::TRANSPARENT, &[damage])?;
            draw_offscreen_textures(
                &mut frame,
                damage,
                &offscreen_textures,
                st.ui.render_state.gpu.window_texture_program.as_ref(),
            )?;
            draw_window_borders(&mut frame, bounds.size, damage, &border_rects, st)?;
            let _ = frame.finish()?;
        }

        let capture_region = Rectangle::<i32, Buffer>::from_size(buffer_size);
        let mapping = renderer.copy_texture(&texture, capture_region, Fourcc::Abgr8888)?;
        let bytes = renderer.map_texture(&mapping)?.to_vec();
        save_window_capture_png(
            output_path,
            bounds.size.w as u32,
            bounds.size.h as u32,
            bytes,
        )?;
        Ok(())
    })();
    st.end_temporary_render_monitor(previous_monitor);
    result
}

fn window_capture_bounds(
    border_rects: &[ActiveBorderRect],
    offscreen_textures: &[OffscreenNodeTexture],
) -> Option<Rectangle<i32, Physical>> {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;

    for rect in border_rects {
        let border_px = rect.border_px.max(0.0).round() as i32;
        min_x = min_x.min(rect.x - border_px);
        min_y = min_y.min(rect.y - border_px);
        max_x = max_x.max(rect.x + rect.w + border_px);
        max_y = max_y.max(rect.y + rect.h + border_px);
    }
    for tex in offscreen_textures {
        min_x = min_x.min(tex.dst_x);
        min_y = min_y.min(tex.dst_y);
        max_x = max_x.max(tex.dst_x + tex.dst_w.max(1));
        max_y = max_y.max(tex.dst_y + tex.dst_h.max(1));
    }

    (min_x < max_x && min_y < max_y).then(|| {
        Rectangle::<i32, Physical>::new(
            (min_x, min_y).into(),
            ((max_x - min_x).max(1), (max_y - min_y).max(1)).into(),
        )
    })
}

fn translate_window_capture_primitives(
    border_rects: &mut [ActiveBorderRect],
    offscreen_textures: &mut [OffscreenNodeTexture],
    offset_x: i32,
    offset_y: i32,
    clip_w: i32,
    clip_h: i32,
) {
    for rect in border_rects {
        rect.x -= offset_x;
        rect.y -= offset_y;
    }
    for tex in offscreen_textures {
        tex.dst_x -= offset_x;
        tex.dst_y -= offset_y;
        tex.clip_x = 0;
        tex.clip_y = 0;
        tex.clip_w = clip_w.max(1);
        tex.clip_h = clip_h.max(1);
    }
}

fn save_window_capture_png(
    output_path: &Path,
    width: u32,
    height: u32,
    bytes: Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let image = RgbaImage::from_vec(width.max(1), height.max(1), bytes)
        .ok_or_else(|| io::Error::other("failed to build RGBA image for window capture"))?;
    image.save(output_path)?;
    Ok(())
}

pub(crate) fn prewarm_visible_active_window_offscreen_caches(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
    now: Instant,
) {
    let mut wl_surfaces: Vec<_> = st
        .platform
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .filter_map(|toplevel| {
            let wl = toplevel.wl_surface().clone();
            let node_id = st.model.surface_to_node.get(&wl.id()).copied()?;
            Some((node_id, wl))
        })
        .collect();

    wl_surfaces.sort_by_key(|(id, _)| std::cmp::Reverse(id.as_u64()));

    for (node_id, wl) in wl_surfaces {
        let bbox = sync_node_size_from_surface(st, node_id, &wl);
        let Some(node) = st.model.field.node(node_id) else {
            continue;
        };
        if node.state != halley_core::field::NodeState::Active
            || !st.model.field.is_visible(node_id)
            || !st.node_visible_on_current_monitor(node_id)
        {
            continue;
        }

        let cache_missing = st
            .ui
            .render_state
            .cache
            .window_offscreen_cache
            .get(&node_id)
            .is_none_or(|cache| {
                !cache.matches_size(bbox.size.w, bbox.size.h)
                    || cache.texture.is_none()
                    || cache.bbox.is_none()
                    || !cache.has_content
            });
        if !cache_missing {
            continue;
        }

        let cache = st.ui.render_state.ensure_window_offscreen_cache(
            node_id,
            bbox.size.w,
            bbox.size.h,
            now,
        );
        if !cache.dirty && cache.texture.is_some() && cache.bbox.is_some() && cache.has_content {
            continue;
        }

        if let Ok(offscreen) = render_surface_tree_to_texture(renderer, &wl, 1.0, None) {
            let cache = st
                .ui
                .render_state
                .cache
                .window_offscreen_cache
                .get_mut(&node_id)
                .expect("offscreen cache should exist after prewarm ensure");
            cache.texture = Some(offscreen.texture);
            cache.bbox = Some(offscreen.bbox);
            cache.has_content = offscreen.has_content;
            cache.mark_clean(now);
        }
    }
}
