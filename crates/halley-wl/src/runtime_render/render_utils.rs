use std::f32::consts::TAU;
use std::time::Instant;

use smithay::{
    backend::renderer::{Color32F, Frame},
    desktop::utils::bbox_from_surface_tree,
    utils::{Logical, Physical, Rectangle},
};

use super::anim_utils::proxy_anim_scale;
use crate::state::HalleyWlState;

pub(crate) fn draw_ring<F: Frame>(
    frame: &mut F,
    st: &HalleyWlState,
    w: i32,
    h: i32,
    rx: f32,
    ry: f32,
    offset_x: f32,
    offset_y: f32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), F::Error> {
    let samples = 96;
    let center_x = st.viewport.center.x + offset_x;
    let center_y = st.viewport.center.y + offset_y;

    for i in 0..samples {
        let t = (i as f32 / samples as f32) * TAU;
        let x = center_x + t.cos() * rx;
        let y = center_y + t.sin() * ry;
        let (sx, sy) = world_to_screen(st, w, h, x, y);
        draw_rect(frame, sx - 1, sy - 1, 3, 3, color, damage)?;
    }
    Ok(())
}

pub(crate) fn world_to_screen(st: &HalleyWlState, w: i32, h: i32, x: f32, y: f32) -> (i32, i32) {
    let vw = st.viewport.size.x.max(1.0);
    let vh = st.viewport.size.y.max(1.0);

    let nx = ((x - st.viewport.center.x) / vw) + 0.5;
    let ny = 0.5 - ((y - st.viewport.center.y) / vh);

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
    st: &mut HalleyWlState,
    node_id: halley_core::field::NodeId,
    wl: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> Rectangle<i32, Logical> {
    let bbox = bbox_from_surface_tree(wl, (0, 0));

    st.bbox_loc
        .insert(node_id, (bbox.loc.x as f32, bbox.loc.y as f32));

    let bw = bbox.size.w.max(1) as f32;
    let bh = bbox.size.h.max(1) as f32;

    let now_ms = st.now_ms(Instant::now());
    let resize_static_active = st.resize_static_active_for(node_id, now_ms);

    let Some(node) = st.field.node_mut(node_id) else {
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

pub(crate) fn preview_proxy_size(_real_w: f32, _real_h: f32) -> (f32, f32) {
    (220.0, 220.0)
}

pub(crate) fn node_marker_metrics(
    st: &HalleyWlState,
    label_len: usize,
    anim_scale: f32,
) -> (i32, i32, i32, i32) {
    let zx = st.viewport.size.x / st.zoom_ref_size.x.max(1.0);
    let zy = st.viewport.size.y / st.zoom_ref_size.y.max(1.0);
    let z = ((zx + zy) * 0.5).clamp(1.0, 8.0);
    let g = z.sqrt() * proxy_anim_scale(anim_scale);

    let dot_half = (4.0 * g).round().clamp(4.0, 18.0) as i32;
    let label_h = (4.0 * g).round().clamp(4.0, 14.0) as i32;
    let label_gap = (8.0 + (g - 1.0) * 8.0).round().clamp(8.0, 28.0) as i32;
    let label_w = ((label_len as f32 * 6.0) * (0.9 + 0.6 * g))
        .round()
        .clamp(24.0, 320.0) as i32;
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
    let dot_x0 = cx - dot_half;
    let dot_y0 = cy - dot_half;
    let dot_x1 = cx + dot_half;
    let dot_y1 = cy + dot_half;
    let lbl_x0 = cx + label_gap;
    let lbl_y0 = cy - (label_h / 2);
    let lbl_x1 = lbl_x0 + label_w;
    let lbl_y1 = lbl_y0 + label_h;
    let x0 = dot_x0.min(lbl_x0) - pad.max(0);
    let y0 = dot_y0.min(lbl_y0) - pad.max(0);
    let x1 = dot_x1.max(lbl_x1) + pad.max(0);
    let y1 = dot_y1.max(lbl_y1) + pad.max(0);
    let w = (x1 - x0).max(8);
    let h = (y1 - y0).max(8);
    (x0, y0, w, h)
}
