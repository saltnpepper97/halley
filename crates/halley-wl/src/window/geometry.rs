use super::*;

pub(super) fn sync_node_size_from_surface(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    wl: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> Rectangle<i32, Logical> {
    let restoring_fullscreen = st
        .model
        .fullscreen_state
        .fullscreen_restore
        .contains_key(&node_id)
        && !st.is_fullscreen_active(node_id);
    let bbox = if restoring_fullscreen {
        bbox_from_surface_tree(wl, (0, 0))
    } else {
        snapshot_surface_geometry(st, node_id, wl)
    };

    if crate::compositor::surface::is_active_cluster_workspace_member(st, node_id) {
        return bbox;
    }

    if restoring_fullscreen {
        return bbox;
    }

    let (bw, bh) = crate::compositor::surface::window_geometry_for_node(st, node_id)
        .map(|(_, _, w, h)| (w.max(1.0), h.max(1.0)))
        .unwrap_or((bbox.size.w.max(1) as f32, bbox.size.h.max(1) as f32));

    let now_ms = st.now_ms(Instant::now());
    let resize_static_active =
        crate::compositor::interaction::state::resize_static_active_for(st, node_id, now_ms);

    let Some(node) = st.model.field.node_mut(node_id) else {
        return bbox;
    };

    let changed =
        (node.intrinsic_size.x - bw).abs() > 0.5 || (node.intrinsic_size.y - bh).abs() > 0.5;
    if !changed || resize_static_active {
        return bbox;
    }

    node.intrinsic_size = halley_core::field::Vec2 { x: bw, y: bh };
    if matches!(node.state, halley_core::field::NodeState::Active) {
        node.footprint = node.intrinsic_size;
    }

    bbox
}

fn snapshot_surface_geometry(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    wl: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> Rectangle<i32, Logical> {
    let bbox = bbox_from_surface_tree(wl, (0, 0));

    st.ui
        .render_state
        .cache
        .bbox_loc
        .insert(node_id, (bbox.loc.x as f32, bbox.loc.y as f32));
    let geometry = with_states(wl, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
    });
    if let Some(g) = geometry {
        st.ui.render_state.cache.window_geometry.insert(
            node_id,
            (
                g.loc.x as f32,
                g.loc.y as f32,
                g.size.w as f32,
                g.size.h as f32,
            ),
        );
    } else {
        st.ui.render_state.cache.window_geometry.insert(
            node_id,
            (
                bbox.loc.x as f32,
                bbox.loc.y as f32,
                bbox.size.w.max(1) as f32,
                bbox.size.h.max(1) as f32,
            ),
        );
    }

    bbox
}

pub(super) fn should_draw_resize_overlap_overlay(
    resize_rect_px: Option<(i32, i32, i32, i32, NodeId)>,
    node_id: NodeId,
    geometry_rect: (i32, i32, i32, i32),
    resizing_node_has_overlap_policy: bool,
) -> bool {
    let Some((rl, rt, rr, rb, rid)) = resize_rect_px else {
        return false;
    };
    if resizing_node_has_overlap_policy || node_id == rid {
        return false;
    }
    let (gx, gy, gw, gh) = geometry_rect;
    let wl = gx;
    let wt = gy;
    let wr = gx + gw.max(1);
    let wb = gy + gh.max(1);
    wl < rr && rl < wr && wt < rb && rt < wb
}

pub(super) fn log_window_render_path(
    _st: &Halley,
    _node_id: halley_core::field::NodeId,
    _path: &str,
    _detail: &str,
) {
}

pub(super) fn rect4_str(x: i32, y: i32, w: i32, h: i32) -> String {
    format!("({},{} {}x{})", x, y, w, h)
}

pub(super) fn rect4f_str(x: f32, y: f32, w: f32, h: f32) -> String {
    format!("({:.1},{:.1} {:.1}x{:.1})", x, y, w, h)
}

pub(super) fn rect_from_local_geometry(
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

pub(super) fn wrap_direct_surface_elements(
    elems: Vec<SurfaceElement>,
    display_clip: Rectangle<i32, Physical>,
    surface_clip_program: Option<&smithay::backend::renderer::gles::GlesTexProgram>,
    geo_rect: Rectangle<i32, Physical>,
    corner_radius: f32,
) -> Vec<CroppedClippedSurfaceElement> {
    elems
        .into_iter()
        .filter_map(|e| {
            let wrapped: DirectSurfaceElement = if let Some(program) = surface_clip_program
                && ClippedSurfaceRenderElement::will_clip(&e, geo_rect, corner_radius)
            {
                ClippedSurfaceRenderElement::new(e, program.clone(), geo_rect, corner_radius).into()
            } else {
                e.into()
            };
            CropRenderElement::from_element(wrapped, 1.0, display_clip)
        })
        .collect()
}

pub(super) fn offscreen_visual_crop_and_dst(
    bbox_loc_x: i32,
    bbox_loc_y: i32,
    bbox_w: i32,
    bbox_h: i32,
    geo_lx: f32,
    geo_ly: f32,
    geo_w: f32,
    geo_h: f32,
    dst_x: i32,
    dst_y: i32,
    dst_w: i32,
    dst_h: i32,
    scale: f32,
    clip: Rectangle<i32, Physical>,
    preserve_visual_margin: bool,
    lock_dst_to_geometry: bool,
) -> (f64, f64, f64, f64, i32, i32, i32, i32, i32, i32, i32, i32) {
    const VISUAL_MARGIN_CAP: f32 = 4.0;

    let geo_x = geo_lx - bbox_loc_x as f32;
    let geo_y = geo_ly - bbox_loc_y as f32;
    let geo_w_f = geo_w.max(1.0);
    let geo_h_f = geo_h.max(1.0);

    let bbox_right = (bbox_loc_x + bbox_w) as f32;
    let bbox_bottom = (bbox_loc_y + bbox_h) as f32;
    let geo_right_abs = geo_lx + geo_w;
    let geo_bottom_abs = geo_ly + geo_h;

    let (left_extra, top_extra, right_extra, bottom_extra) = if preserve_visual_margin {
        (
            geo_x.clamp(0.0, VISUAL_MARGIN_CAP),
            geo_y.clamp(0.0, VISUAL_MARGIN_CAP),
            (bbox_right - geo_right_abs).clamp(0.0, VISUAL_MARGIN_CAP),
            (bbox_bottom - geo_bottom_abs).clamp(0.0, VISUAL_MARGIN_CAP),
        )
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };

    let src_x = (geo_x - left_extra).max(0.0) as f64;
    let src_y = (geo_y - top_extra).max(0.0) as f64;
    let src_w = (geo_w_f + left_extra + right_extra) as f64;
    let src_w = src_w.min(bbox_w as f64 - src_x).max(1.0);
    let src_h = (geo_h_f + top_extra + bottom_extra) as f64;
    let src_h = src_h.min(bbox_h as f64 - src_y).max(1.0);

    let dst_expand_l = (left_extra * scale).round() as i32;
    let dst_expand_t = (top_extra * scale).round() as i32;
    let dst_expand_r = (right_extra * scale).round() as i32;
    let dst_expand_b = (bottom_extra * scale).round() as i32;

    let (final_dst_x, final_dst_y, final_dst_w, final_dst_h) = if lock_dst_to_geometry {
        (dst_x, dst_y, dst_w.max(1), dst_h.max(1))
    } else {
        (
            dst_x - dst_expand_l,
            dst_y - dst_expand_t,
            dst_w.max(1) + dst_expand_l + dst_expand_r,
            dst_h.max(1) + dst_expand_t + dst_expand_b,
        )
    };

    (
        src_x,
        src_y,
        src_w,
        src_h,
        final_dst_x,
        final_dst_y,
        final_dst_w,
        final_dst_h,
        clip.loc.x,
        clip.loc.y,
        clip.size.w,
        clip.size.h,
    )
}
