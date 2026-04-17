use super::*;

pub(crate) fn active_window_frame_pad_px(tuning: &halley_config::RuntimeTuning) -> i32 {
    tuning.total_window_border_footprint_px()
}

pub(super) fn scaled_window_border_px(base: i32, render_scale: f32) -> i32 {
    let base = base.max(0) as f32;
    let scaled = (base * render_scale).round();
    if base > 0.0 {
        scaled.max(1.0) as i32
    } else {
        0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct WindowDecorationMetrics {
    pub(super) content_corner_radius_px: i32,
    pub(super) primary_border_px: i32,
    pub(super) primary_outer_corner_radius_px: i32,
    pub(super) secondary_gap_px: i32,
    pub(super) secondary_border_px: i32,
    pub(super) secondary_inner_corner_radius_px: i32,
    pub(super) secondary_outer_corner_radius_px: i32,
}

const DECORATION_JOIN_OVERLAP_PX: f32 = 0.75;

pub(super) fn window_decoration_metrics(
    content_corner_radius_px: i32,
    primary_border_px: i32,
    secondary_gap_px: i32,
    secondary_border_px: i32,
) -> WindowDecorationMetrics {
    let content_corner_radius_px = content_corner_radius_px.max(0);
    let primary_border_px = primary_border_px.max(0);
    let secondary_gap_px = secondary_gap_px.max(0);
    let secondary_border_px = secondary_border_px.max(0);
    let (
        primary_outer_corner_radius_px,
        secondary_inner_corner_radius_px,
        secondary_outer_corner_radius_px,
    ) = if content_corner_radius_px > 0 {
        let primary_outer_corner_radius_px = content_corner_radius_px + primary_border_px;
        let secondary_inner_corner_radius_px = primary_outer_corner_radius_px + secondary_gap_px;
        let secondary_outer_corner_radius_px =
            secondary_inner_corner_radius_px + secondary_border_px;
        (
            primary_outer_corner_radius_px,
            secondary_inner_corner_radius_px,
            secondary_outer_corner_radius_px,
        )
    } else {
        (0, 0, 0)
    };

    WindowDecorationMetrics {
        content_corner_radius_px,
        primary_border_px,
        primary_outer_corner_radius_px,
        secondary_gap_px,
        secondary_border_px,
        secondary_inner_corner_radius_px,
        secondary_outer_corner_radius_px,
    }
}

fn overlap_joined_inner_boundary(
    inner_offset_px: f32,
    inner_w_px: f32,
    inner_h_px: f32,
    inner_corner_radius_px: f32,
    overlap_px: f32,
) -> (f32, f32, f32, f32) {
    let overlap_px = overlap_px.max(0.0);
    if overlap_px <= 0.0 {
        return (
            inner_offset_px,
            inner_w_px.max(1.0),
            inner_h_px.max(1.0),
            inner_corner_radius_px.max(0.0),
        );
    }

    (
        inner_offset_px + overlap_px,
        (inner_w_px - overlap_px * 2.0).max(1.0),
        (inner_h_px - overlap_px * 2.0).max(1.0),
        (inner_corner_radius_px - overlap_px).max(0.0),
    )
}

fn border_color(color: halley_config::DecorationBorderColor) -> Color32F {
    Color32F::new(color.r, color.g, color.b, 1.0)
}

pub(super) fn build_window_border_rects(
    st: &Halley,
    node_id: NodeId,
    gx: i32,
    gy: i32,
    gw: i32,
    gh: i32,
    alpha: f32,
    render_scale: f32,
    fullscreen_on_current_monitor: bool,
) -> Vec<ActiveBorderRect> {
    if fullscreen_on_current_monitor {
        return Vec::new();
    }

    let metrics = window_decoration_metrics(
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
    );
    let focused = st.model.focus_state.primary_interaction_focus == Some(node_id);
    let mut rects = Vec::with_capacity(2);

    if metrics.primary_border_px > 0 {
        let (inner_offset_x, inner_w, inner_h, inner_corner_radius) = overlap_joined_inner_boundary(
            metrics.primary_border_px as f32,
            gw.max(1) as f32,
            gh.max(1) as f32,
            metrics.content_corner_radius_px as f32,
            DECORATION_JOIN_OVERLAP_PX,
        );
        let border_color = if focused {
            border_color(st.runtime.tuning.decorations.border.color_focused)
        } else {
            border_color(st.runtime.tuning.decorations.border.color_unfocused)
        };
        rects.push(ActiveBorderRect {
            x: gx,
            y: gy,
            w: gw.max(1),
            h: gh.max(1),
            inner_offset_x,
            inner_offset_y: inner_offset_x,
            inner_w,
            inner_h,
            alpha,
            border_px: metrics.primary_border_px as f32,
            corner_radius: metrics.primary_outer_corner_radius_px as f32,
            inner_corner_radius,
            border_color,
        });
    }

    if metrics.secondary_border_px > 0 {
        let secondary_inset_px = metrics.primary_border_px + metrics.secondary_gap_px;
        let secondary_overlap_px = if metrics.secondary_gap_px == 0 {
            DECORATION_JOIN_OVERLAP_PX
        } else {
            0.0
        };
        let (inner_offset_x, inner_w, inner_h, inner_corner_radius) = overlap_joined_inner_boundary(
            metrics.secondary_border_px as f32,
            (gw + secondary_inset_px * 2).max(1) as f32,
            (gh + secondary_inset_px * 2).max(1) as f32,
            metrics.secondary_inner_corner_radius_px as f32,
            secondary_overlap_px,
        );
        let border_color = if focused {
            border_color(st.runtime.tuning.decorations.secondary_border.color_focused)
        } else {
            border_color(
                st.runtime
                    .tuning
                    .decorations
                    .secondary_border
                    .color_unfocused,
            )
        };
        rects.push(ActiveBorderRect {
            x: gx - secondary_inset_px,
            y: gy - secondary_inset_px,
            w: (gw + secondary_inset_px * 2).max(1),
            h: (gh + secondary_inset_px * 2).max(1),
            inner_offset_x,
            inner_offset_y: inner_offset_x,
            inner_w,
            inner_h,
            alpha,
            border_px: metrics.secondary_border_px as f32,
            corner_radius: metrics.secondary_outer_corner_radius_px as f32,
            inner_corner_radius,
            border_color,
        });
    }

    rects
}
