use super::*;

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
    let cache = st.ui.render_state.window_offscreen_cache.get(&node_id)?;
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
