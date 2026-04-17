use halley_config::{NodeBackgroundColorMode, NodeBorderColorMode, RuntimeTuning};
use halley_core::field::Vec2;
use smithay::backend::renderer::Color32F;

use crate::animation::{ease_in_out_cubic, proxy_anim_scale};
use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::root::Halley;

pub(crate) fn world_to_screen(st: &Halley, w: i32, h: i32, x: f32, y: f32) -> (i32, i32) {
    let view = camera_controller(st).view_size();
    let vw = view.x.max(1.0);
    let vh = view.y.max(1.0);

    let nx = ((x - st.model.viewport.center.x) / vw) + 0.5;
    let ny = ((y - st.model.viewport.center.y) / vh) + 0.5;

    let sx = (nx * w as f32).round() as i32;
    let sy = (ny * h as f32).round() as i32;
    (sx, sy)
}

pub(crate) fn preview_proxy_size(_real_w: f32, _real_h: f32) -> (f32, f32) {
    (220.0, 220.0)
}

pub(crate) fn node_render_diameter_px(
    st: &Halley,
    intrinsic_size: Vec2,
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

fn window_active_border_color_for_tuning(tuning: &RuntimeTuning) -> Color32F {
    let color = tuning.decorations.border.color_focused;
    Color32F::new(color.r, color.g, color.b, 1.0)
}

fn window_inactive_border_color_for_tuning(tuning: &RuntimeTuning) -> Color32F {
    let color = tuning.decorations.border.color_unfocused;
    Color32F::new(color.r, color.g, color.b, 1.0)
}

fn node_ring_color_for_tuning(tuning: &RuntimeTuning, hovered: bool, alpha: f32) -> Color32F {
    let mode = if hovered {
        tuning.node_border_color_hover
    } else {
        tuning.node_border_color_inactive
    };
    let base = match mode {
        NodeBorderColorMode::UseWindowActive => window_active_border_color_for_tuning(tuning),
        NodeBorderColorMode::UseWindowInactive => window_inactive_border_color_for_tuning(tuning),
    };
    Color32F::new(base.r(), base.g(), base.b(), alpha)
}

pub(crate) fn themed_node_fill_color(tuning: &RuntimeTuning, hovered: bool) -> Color32F {
    match tuning.node_background_color {
        NodeBackgroundColorMode::Auto | NodeBackgroundColorMode::Theme => {
            let ring = node_ring_color_for_tuning(tuning, hovered, 1.0);
            let base = (0.94, 0.96, 0.985);
            Color32F::new(
                base.0 * 0.86 + ring.r() * 0.14,
                base.1 * 0.86 + ring.g() * 0.14,
                base.2 * 0.86 + ring.b() * 0.14,
                1.0,
            )
        }
        NodeBackgroundColorMode::Light => Color32F::new(0.92, 0.95, 0.98, 1.0),
        NodeBackgroundColorMode::Dark => Color32F::new(0.15, 0.18, 0.22, 1.0),
        NodeBackgroundColorMode::Fixed { r, g, b } => Color32F::new(r, g, b, 1.0),
    }
}

pub(crate) fn themed_node_label_text_color(fill_color: Color32F, alpha: f32) -> Color32F {
    let luminance = fill_color.r() * 0.2126 + fill_color.g() * 0.7152 + fill_color.b() * 0.0722;
    if luminance < 0.45 {
        Color32F::new(0.96, 0.98, 1.0, alpha)
    } else {
        Color32F::new(0.08, 0.10, 0.12, alpha)
    }
}

pub(crate) fn themed_node_label_fill_color(
    tuning: &RuntimeTuning,
    hovered: bool,
    alpha: f32,
) -> Color32F {
    let fill = themed_node_fill_color(tuning, hovered);
    Color32F::new(fill.r(), fill.g(), fill.b(), alpha)
}

pub(crate) fn themed_node_label_colors(
    tuning: &RuntimeTuning,
    hovered: bool,
    fill_alpha: f32,
    text_alpha: f32,
) -> (Color32F, Color32F) {
    let fill = themed_node_label_fill_color(tuning, hovered, fill_alpha);
    let text = themed_node_label_text_color(fill, text_alpha);
    (fill, text)
}
