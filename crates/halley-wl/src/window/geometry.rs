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

    if crate::compositor::workspace::state::node_in_maximize_session(st, node_id) {
        return bbox;
    }

    if st.is_fullscreen_active(node_id) {
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

pub(super) fn log_window_render_path(
    _st: &Halley,
    node_id: halley_core::field::NodeId,
    path: &str,
    detail: &str,
) {
    // Only log the expensive paths. `offscreen-compose` (the cheap cached reuse)
    // fires for every window every frame and churned the log into rapid rotation,
    // evicting the very spike lines we needed to see.
    if crate::perf::enabled() && path != "offscreen-compose" {
        eventline::info!(
            "perf render-path node={} path={} {}",
            node_id.as_u64(),
            path,
            detail
        );
    }
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

/// Crop `src` to the destination's aspect ratio (object-fit: cover) so the eventual
/// fill render is a *uniform* scale instead of a squish. Width crops are centered;
/// height crops anchor near the top, matching how tiling slots stack from the top-left.
/// Because callers only enable this while holding a capture at least as large as the
/// footprint, cover only ever crops/downscales — it never upscales.
fn cover_crop_src(
    src_x: f64,
    src_y: f64,
    src_w: f64,
    src_h: f64,
    dst_w: i32,
    dst_h: i32,
) -> (f64, f64, f64, f64) {
    let dst_aspect = dst_w.max(1) as f64 / dst_h.max(1) as f64;
    let src_aspect = src_w / src_h.max(1e-3);
    if (src_aspect - dst_aspect).abs() < 1e-3 {
        return (src_x, src_y, src_w, src_h);
    }
    if src_aspect > dst_aspect {
        // Capture is wider than the slot — crop the sides, keep full height, center.
        let new_w = (src_h * dst_aspect).clamp(1.0, src_w);
        (src_x + (src_w - new_w) * 0.5, src_y, new_w, src_h)
    } else {
        // Capture is taller than the slot — crop the bottom, keep full width, top-anchor.
        let new_h = (src_w / dst_aspect).clamp(1.0, src_h);
        (src_x, src_y, src_w, new_h)
    }
}

#[allow(clippy::too_many_arguments)]
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
    cover_src_to_dst_aspect: bool,
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

    // The src (cached buffer crop) is mapped onto the animated dst box. A straight
    // fill stretches src→dst, which squishes the texture whenever the box's aspect
    // differs from the capture's (e.g. a tiling reflow tweening between two slot
    // shapes). When `cover_src_to_dst_aspect` is set, crop src to the dst aspect first
    // so the fill becomes a uniform scale (object-fit: cover) — no squish, and for a
    // same-width height change it stays a crisp 1:1 vertical crop.
    let (src_x, src_y, src_w, src_h) = if cover_src_to_dst_aspect {
        cover_crop_src(src_x, src_y, src_w, src_h, final_dst_w, final_dst_h)
    } else {
        (src_x, src_y, src_w, src_h)
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

#[cfg(test)]
mod tests {
    use super::cover_crop_src;

    fn aspect(w: f64, h: f64) -> f64 {
        w / h
    }

    #[test]
    fn cover_keeps_matching_aspect_untouched() {
        let (x, y, w, h) = cover_crop_src(0.0, 0.0, 320.0, 240.0, 640, 480);
        assert!((x - 0.0).abs() < 1e-6 && (y - 0.0).abs() < 1e-6);
        assert!((w - 320.0).abs() < 1e-6 && (h - 240.0).abs() < 1e-6);
    }

    #[test]
    fn cover_crops_width_centered_when_capture_too_wide() {
        // Capture 400x200 (2.0) into a 1.0 slot → keep height, crop width to 200, centered.
        let (x, y, w, h) = cover_crop_src(0.0, 0.0, 400.0, 200.0, 100, 100);
        assert!((h - 200.0).abs() < 1e-6, "height kept");
        assert!((w - 200.0).abs() < 1e-6, "width cropped to dst aspect");
        assert!((x - 100.0).abs() < 1e-6, "centered horizontally");
        assert!((y - 0.0).abs() < 1e-6);
        assert!((aspect(w, h) - 1.0).abs() < 1e-3, "src now matches dst aspect");
    }

    #[test]
    fn cover_crops_height_top_anchored_when_capture_too_tall() {
        // Same-width height shrink: capture 200x400 into a 200x100-aspect (2.0) slot →
        // keep full width, crop height to 100, anchored at the top (y unchanged).
        let (x, y, w, h) = cover_crop_src(0.0, 0.0, 200.0, 400.0, 200, 100);
        assert!((w - 200.0).abs() < 1e-6, "full width kept");
        assert!((h - 100.0).abs() < 1e-6, "height cropped to dst aspect");
        assert!((x - 0.0).abs() < 1e-6 && (y - 0.0).abs() < 1e-6, "top-left anchored");
        assert!((aspect(w, h) - 2.0).abs() < 1e-3);
    }
}
