use std::f32::consts::TAU;
use std::time::Instant;

use crate::animation::{ease_in_out_cubic, proxy_anim_scale};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;
use smithay::{
    backend::renderer::{Color32F, Frame},
    desktop::utils::bbox_from_surface_tree,
    utils::{Logical, Physical, Rectangle},
};

use crate::compositor::root::Halley;

/// Draw an elliptical ring at a fixed screen-space position and radius.
///
/// All coordinates are in physical screen pixels.  This function is
/// intentionally decoupled from world-space and camera zoom so that HUD
/// elements like the focus ring do not scale when the camera zooms.
pub(crate) fn draw_ring<F: Frame>(
    frame: &mut F,
    center_sx: f32,
    center_sy: f32,
    rx: f32,
    ry: f32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), F::Error> {
    let samples = 224;
    let thickness = 2.0f32;
    let mut prev: Option<(f32, f32)> = None;
    for i in 0..=samples {
        let t = (i as f32 / samples as f32) * TAU;
        let x = center_sx + t.cos() * rx;
        let y = center_sy + t.sin() * ry;
        if let Some((px, py)) = prev {
            let dx = x - px;
            let dy = y - py;
            let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as i32;
            for step in 0..=steps {
                let frac = step as f32 / steps as f32;
                let sx = px + dx * frac;
                let sy = py + dy * frac;
                draw_rect(
                    frame,
                    (sx - thickness * 0.5).round() as i32,
                    (sy - thickness * 0.5).round() as i32,
                    thickness.round().max(1.0) as i32,
                    thickness.round().max(1.0) as i32,
                    color,
                    damage,
                )?;
            }
        }
        prev = Some((x, y));
    }
    Ok(())
}

pub(crate) fn world_to_screen(st: &Halley, w: i32, h: i32, x: f32, y: f32) -> (i32, i32) {
    let view = st.camera_view_size();
    let vw = view.x.max(1.0);
    let vh = view.y.max(1.0);

    let nx = ((x - st.model.viewport.center.x) / vw) + 0.5;
    let ny = ((y - st.model.viewport.center.y) / vh) + 0.5;

    let sx = (nx * w as f32).round() as i32;
    let sy = (ny * h as f32).round() as i32;
    (sx, sy)
}

pub(crate) fn draw_rect<F: Frame>(
    frame: &mut F,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), F::Error> {
    if w <= 0 || h <= 0 {
        return Ok(());
    }
    let dst = Rectangle::new((x, y).into(), (w, h).into());
    frame.draw_solid(dst, &[damage], color)
}

pub(crate) fn draw_outline_rect<F: Frame>(
    frame: &mut F,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), F::Error> {
    if w <= 1 || h <= 1 {
        return Ok(());
    }
    draw_rect(frame, x, y, w, 2, color, damage)?;
    draw_rect(frame, x, y + h - 2, w, 2, color, damage)?;
    draw_rect(frame, x, y, 2, h, color, damage)?;
    draw_rect(frame, x + w - 2, y, 2, h, color, damage)
}

pub(crate) fn sync_node_size_from_surface(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    wl: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> Rectangle<i32, Logical> {
    let bbox = snapshot_surface_geometry(st, node_id, wl);

    if crate::compositor::surface_ops::is_active_cluster_workspace_member(st, node_id) {
        return bbox;
    }

    let (bw, bh) = crate::compositor::surface_ops::window_geometry_for_node(st, node_id)
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
    if !changed {
        return bbox;
    }

    if resize_static_active {
        return bbox;
    }

    node.intrinsic_size = halley_core::field::Vec2 { x: bw, y: bh };
    if matches!(node.state, halley_core::field::NodeState::Active) {
        node.footprint = node.intrinsic_size;
    }

    bbox
}

pub(crate) fn snapshot_surface_geometry(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    wl: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> Rectangle<i32, Logical> {
    let bbox = bbox_from_surface_tree(wl, (0, 0));

    st.ui
        .render_state
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
        st.ui.render_state.window_geometry.insert(
            node_id,
            (
                g.loc.x as f32,
                g.loc.y as f32,
                g.size.w as f32,
                g.size.h as f32,
            ),
        );
    } else {
        st.ui.render_state.window_geometry.insert(
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

pub(crate) fn preview_proxy_size(_real_w: f32, _real_h: f32) -> (f32, f32) {
    (220.0, 220.0)
}

pub(crate) fn node_render_diameter_px(
    st: &Halley,
    intrinsic_size: halley_core::field::Vec2,
    label_len: usize,
    anim_scale: f32,
) -> f32 {
    const PROXY_TO_MARKER_START: f32 = 0.50;
    const PROXY_TO_MARKER_END: f32 = 0.20;

    let marker_mix_lin = ((PROXY_TO_MARKER_START - anim_scale)
        / (PROXY_TO_MARKER_START - PROXY_TO_MARKER_END))
        .clamp(0.0, 1.0);
    let marker_mix = ease_in_out_cubic(marker_mix_lin);

    let (dot_half, _, _, _) = node_marker_metrics(st, label_len, anim_scale);
    let marker_diameter = ((dot_half as f32 * 1.5).round().max(1.0)) * 2.0;

    let (pw, ph) = preview_proxy_size(intrinsic_size.x, intrinsic_size.y);
    let proxy_diameter = pw.min(ph) * proxy_anim_scale(anim_scale);

    (proxy_diameter + (marker_diameter - proxy_diameter) * marker_mix).max(marker_diameter)
}

pub(crate) fn node_marker_metrics(
    _st: &Halley,
    label_len: usize,
    _anim_scale: f32,
) -> (i32, i32, i32, i32) {
    let dot_half = 17i32;
    let label_h = 26i32;
    let label_gap = 14i32;
    let label_w = ((label_len as f32) * 9.5).round().clamp(72.0, 420.0) as i32;
    (dot_half, label_gap, label_w, label_h)
}

pub(crate) fn node_marker_bounds(
    cx: i32,
    cy: i32,
    dot_half: i32,
    label_gap: i32,
    label_w: i32,
    label_h: i32,
    pad: i32,
) -> (i32, i32, i32, i32) {
    let pad = pad.max(0);
    let dot_d = (dot_half * 2).max(1);

    let content_w = (dot_d + label_gap.max(0) + label_w.max(0)).max(dot_d);
    let content_h = dot_d.max(label_h).max(1);

    let x0 = cx - dot_half - pad;
    let y0 = cy - (content_h / 2) - pad;
    let w = (content_w + pad * 2).max(8);
    let h = (content_h + pad * 2).max(8);

    (x0, y0, w, h)
}
