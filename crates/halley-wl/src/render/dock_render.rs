use std::error::Error;
use std::time::Instant;

use halley_core::docking::DockSide;
use smithay::{
    backend::renderer::{Color32F, Frame},
    utils::{Physical, Rectangle, Size},
};

use crate::state::HalleyWlState;

use super::render_utils::{
    draw_outline_rect, draw_rect, node_marker_bounds, node_marker_metrics, world_to_screen,
};

#[inline]
pub(crate) fn rect_side_anchor(x: i32, y: i32, w: i32, h: i32, side: DockSide) -> (i32, i32) {
    let cx = x + (w / 2);
    let cy = y + (h / 2);
    match side {
        DockSide::Left => (x, cy),
        DockSide::Right => (x + w, cy),
        DockSide::Top => (cx, y),
        DockSide::Bottom => (cx, y + h),
    }
}

pub(crate) fn draw_docked_pairs<F>(
    frame: &mut F,
    st: &mut HalleyWlState,
    size: Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    now: Instant,
) -> Result<(), Box<dyn Error>>
where
    F: Frame,
    F::Error: std::error::Error + 'static,
{
    for (a, b) in st.field.docked_pairs() {
        let (Some(na), Some(nb)) = (st.field.node(a), st.field.node(b)) else {
            continue;
        };
        if !st.field.is_visible(a) || !st.field.is_visible(b) {
            continue;
        }

        let both_nodes = matches!(
            na.state,
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
        ) && matches!(
            nb.state,
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
        );

        if !both_nodes && !st.tuning.dev_enabled {
            continue;
        }

        let (ax, ay) = world_to_screen(st, size.w, size.h, na.pos.x, na.pos.y);
        let (bx, by) = world_to_screen(st, size.w, size.h, nb.pos.x, nb.pos.y);

        let c = if both_nodes {
            Color32F::new(0.20, 0.88, 0.78, 0.82)
        } else {
            Color32F::new(0.98, 0.78, 0.22, 0.70)
        };

        let ((x0, y0), (x1, y1)) = if both_nodes {
            let anim_a = st.anim_style_for(a, na.state.clone(), now);
            let anim_b = st.anim_style_for(b, nb.state.clone(), now);
            let (dot_half_a, label_gap_a, label_w_a, label_h_a) =
                node_marker_metrics(st, na.label.len(), anim_a.scale);
            let (dot_half_b, label_gap_b, label_w_b, label_h_b) =
                node_marker_metrics(st, nb.label.len(), anim_b.scale);
            let (ra_x, ra_y, ra_w, ra_h) =
                node_marker_bounds(ax, ay, dot_half_a, label_gap_a, label_w_a, label_h_a, 6);
            let (rb_x, rb_y, rb_w, rb_h) =
                node_marker_bounds(bx, by, dot_half_b, label_gap_b, label_w_b, label_h_b, 6);
            let ra_r = ra_x + ra_w;
            let ra_b = ra_y + ra_h;
            let rb_l = rb_x;
            let rb_r = rb_x + rb_w;
            let rb_t = rb_y;
            let rb_b = rb_y + rb_h;

            if let Some((a_side, b_side)) = st.field.dock_sides_for_pair(a, b) {
                (
                    rect_side_anchor(ra_x, ra_y, ra_w, ra_h, a_side),
                    rect_side_anchor(rb_x, rb_y, rb_w, rb_h, b_side),
                )
            } else if (ax - bx).abs() >= (ay - by).abs() {
                if ax <= bx {
                    ((ra_r, ay), (rb_l, by))
                } else {
                    ((ra_x, ay), (rb_r, by))
                }
            } else if ay <= by {
                ((ax, ra_b), (bx, rb_t))
            } else {
                ((ax, ra_y), (bx, rb_b))
            }
        } else {
            ((ax, ay), (bx, by))
        };

        if (x0 - x1).abs() >= (y0 - y1).abs() {
            let x = x0.min(x1);
            let w = (x0 - x1).abs().max(1);
            draw_rect(frame, x, y0 - 1, w, 3, c, damage)?;
        } else {
            let y = y0.min(y1);
            let h = (y0 - y1).abs().max(1);
            draw_rect(frame, x0 - 1, y, 3, h, c, damage)?;
        }

        let mx = ((x0 + x1) as f32 * 0.5).round() as i32;
        let my = ((y0 + y1) as f32 * 0.5).round() as i32;
        draw_rect(frame, mx - 3, my - 3, 7, 7, c, damage)?;
    }
    Ok(())
}

pub(crate) fn draw_dock_preview<F>(
    frame: &mut F,
    st: &mut HalleyWlState,
    size: Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    now: Instant,
) -> Result<(), Box<dyn Error>>
where
    F: Frame,
    F::Error: std::error::Error + 'static,
{
    let Some(preview) = st.field.dock_preview() else {
        return Ok(());
    };

    let mover_id = preview.mover_id;
    let target_id = preview.target_id;
    let side = preview.side;
    let snap_pos = preview.snap_pos;
    let armed = preview.armed;

    let (sx, sy) = world_to_screen(st, size.w, size.h, snap_pos.x, snap_pos.y);
    let marker = if armed {
        Color32F::new(0.22, 0.95, 0.36, 0.94)
    } else {
        Color32F::new(0.98, 0.78, 0.22, 0.9)
    };

    draw_outline_rect(frame, sx - 12, sy - 12, 24, 24, marker, damage)?;
    draw_rect(frame, sx - 2, sy - 2, 5, 5, marker, damage)?;

    if let Some(m) = st.field.node(mover_id) {
        let (mx, my) = world_to_screen(st, size.w, size.h, m.pos.x, m.pos.y);
        let lx = mx.min(sx);
        let ly = my.min(sy);
        let lw = (mx - sx).abs().max(1);
        let lh = (my - sy).abs().max(1);
        draw_outline_rect(
            frame,
            lx,
            ly,
            lw,
            lh,
            Color32F::new(0.98, 0.78, 0.22, 0.45),
            damage,
        )?;
    }

    if let Some(t) = st.field.node(target_id) {
        let (tx, ty) = world_to_screen(st, size.w, size.h, t.pos.x, t.pos.y);
        let target_is_node = matches!(
            t.state,
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
        );

        if target_is_node {
            let anim_t = st.anim_style_for(target_id, t.state.clone(), now);
            let (dot_half, label_gap, label_w, label_h) =
                node_marker_metrics(st, t.label.len(), anim_t.scale);
            let (rx, ry, rw, rh) =
                node_marker_bounds(tx, ty, dot_half, label_gap, label_w, label_h, 6);

            draw_outline_rect(
                frame,
                rx,
                ry,
                rw,
                rh,
                Color32F::new(0.22, 0.95, 0.36, 0.42),
                damage,
            )?;

            for anchor_side in [
                DockSide::Left,
                DockSide::Right,
                DockSide::Top,
                DockSide::Bottom,
            ] {
                let (ax, ay) = rect_side_anchor(rx, ry, rw, rh, anchor_side);
                let c = if anchor_side == side {
                    Color32F::new(0.22, 0.95, 0.36, 0.94)
                } else {
                    Color32F::new(0.22, 0.95, 0.36, 0.58)
                };
                draw_rect(frame, ax - 3, ay - 3, 7, 7, c, damage)?;
            }
        } else {
            draw_outline_rect(
                frame,
                tx - 8,
                ty - 8,
                16,
                16,
                Color32F::new(0.22, 0.95, 0.36, 0.75),
                damage,
            )?;
        }
    }

    Ok(())
}
