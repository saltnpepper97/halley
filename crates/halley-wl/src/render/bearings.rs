use std::error::Error;

use halley_core::bearings::{Bearing, bearings_for_visible_nodes};
use halley_core::field::NodeId;
use smithay::{
    backend::renderer::{
        Color32F, Texture,
        gles::{GlesFrame, Uniform},
    },
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::render::app_icon::{ensure_app_icon_resources_for_node_ids, node_app_icon_entry};
use crate::state::{Halley, NodeAppIconCacheEntry};

use super::utils::{bitmap_text_size, draw_bitmap_text, draw_rect};

#[derive(Clone, Debug)]
pub(crate) struct BearingChipLayout {
    pub(crate) node_id: NodeId,
    pub(crate) chip_rect: Rectangle<i32, Physical>,
    pub(crate) icon_rect: Option<Rectangle<i32, Physical>>,
    pub(crate) label: String,
    pub(crate) distance_text: Option<String>,
    pub(crate) distance_rect: Option<Rectangle<i32, Physical>>,
    pub(crate) distance_pos: Option<(i32, i32)>,
    pub(crate) alpha: f32,
}

const CHIP_PAD_X: i32 = 10;
const CHIP_PAD_Y: i32 = 7;
const CHIP_GAP: i32 = 10;
const EDGE_PAD: i32 = 16;
const LABEL_SCALE: i32 = 2;
const META_SCALE: i32 = 1;
const ICON_SIZE: i32 = 16;
const DISTANCE_GAP: i32 = 4;
const META_PAD_X: i32 = 7;
const META_PAD_Y: i32 = 4;
const MAX_LABEL_CHARS: usize = 24;
const MIN_DISTANCE_ALPHA: f32 = 0.34;

pub(crate) fn ensure_bearing_icon_resources(
    renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
    st: &mut Halley,
    monitor: &str,
) -> Result<(), Box<dyn Error>> {
    let viewport = st.viewport_for_monitor(monitor);
    let node_ids = bearings_for_visible_nodes(&st.field, &viewport)
        .into_iter()
        .filter_map(|(id, _)| {
            let node = st.field.node(id)?;
            (node.kind == halley_core::field::NodeKind::Surface
                && st.monitor_state.node_monitor.get(&id).map(String::as_str) == Some(monitor))
            .then_some(id)
        })
        .collect::<Vec<_>>();
    ensure_app_icon_resources_for_node_ids(renderer, st, node_ids.into_iter())
}

pub(crate) fn collect_bearing_layouts(
    st: &Halley,
    screen_w: i32,
    screen_h: i32,
    monitor: &str,
    ui_mix: f32,
) -> Vec<BearingChipLayout> {
    if ui_mix <= 0.002 {
        return Vec::new();
    }

    let viewport = st.viewport_for_monitor(monitor);
    let mut groups: [Vec<(NodeId, f32)>; 8] = std::array::from_fn(|_| Vec::new());
    for (id, bearing) in bearings_for_visible_nodes(&st.field, &viewport) {
        let Some(node) = st.field.node(id) else {
            continue;
        };
        if node.kind != halley_core::field::NodeKind::Surface {
            continue;
        }
        if st.monitor_state.node_monitor.get(&id).map(String::as_str) != Some(monitor) {
            continue;
        }
        if st.node_intersects_viewport_on_monitor(monitor, id) {
            continue;
        }
        let distance = offscreen_distance_from_view_edge(st, monitor, id).unwrap_or(0.0);
        groups[bearing_index(bearing)].push((id, distance));
    }

    for entries in &mut groups {
        entries.sort_by(|(a_id, a_dist), (b_id, b_dist)| {
            a_dist
                .partial_cmp(b_dist)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a_id.as_u64().cmp(&b_id.as_u64()))
        });
    }

    let mut out = Vec::new();
    for bearing in [
        Bearing::NW,
        Bearing::N,
        Bearing::NE,
        Bearing::W,
        Bearing::E,
        Bearing::SW,
        Bearing::S,
        Bearing::SE,
    ] {
        let entries = &groups[bearing_index(bearing)];
        if entries.is_empty() {
            continue;
        }
        for (index, &(node_id, distance)) in entries.iter().enumerate() {
            let Some(node) = st.field.node(node_id) else {
                continue;
            };
            let label = bearing_label(st, node_id, node.label.as_str());
            let (label_w, label_h) = bitmap_text_size(label.as_str(), LABEL_SCALE);
            let show_icon = st.tuning.bearings.show_icons;
            let chip_w = CHIP_PAD_X * 2
                + label_w
                + if show_icon {
                    ICON_SIZE + CHIP_PAD_X.saturating_sub(4)
                } else {
                    0
                };
            let chip_h = (CHIP_PAD_Y * 2 + label_h.max(ICON_SIZE)).max(24);
            let distance_text = st
                .tuning
                .bearings
                .show_distance
                .then(|| format!("{:.0}px", distance.round()));
            let distance_h = distance_text
                .as_ref()
                .map(|text| bitmap_text_size(text, META_SCALE).1 + META_PAD_Y * 2 + DISTANCE_GAP)
                .unwrap_or(0);
            let stack_h = chip_h + distance_h + CHIP_GAP;
            let (chip_x, chip_y) =
                bearing_origin(bearing, screen_w, screen_h, chip_w, chip_h, distance_h, index, stack_h);
            let icon_rect = show_icon.then(|| {
                Rectangle::new(
                    (chip_x + CHIP_PAD_X, chip_y + (chip_h - ICON_SIZE) / 2).into(),
                    (ICON_SIZE, ICON_SIZE).into(),
                )
            });
            let distance_rect = distance_text.as_ref().map(|text| {
                let (distance_w, distance_h_px) = bitmap_text_size(text, META_SCALE);
                let meta_w = distance_w + META_PAD_X * 2;
                let meta_h = distance_h_px + META_PAD_Y * 2;
                Rectangle::new(
                    (chip_x + ((chip_w - meta_w) / 2), chip_y - DISTANCE_GAP - meta_h).into(),
                    (meta_w, meta_h).into(),
                )
            });
            let distance_pos = distance_text
                .as_ref()
                .zip(distance_rect)
                .map(|(text, rect)| {
                    let (_, distance_h_px) = bitmap_text_size(text, META_SCALE);
                    (
                        rect.loc.x + META_PAD_X,
                        rect.loc.y + (rect.size.h - distance_h_px) / 2,
                    )
                });
            let distance_fade = if st.tuning.bearings.fade_distance <= f32::EPSILON {
                1.0
            } else {
                let t = (distance / st.tuning.bearings.fade_distance).clamp(0.0, 1.0);
                MIN_DISTANCE_ALPHA + (1.0 - t) * (1.0 - MIN_DISTANCE_ALPHA)
            };
            out.push(BearingChipLayout {
                node_id,
                chip_rect: Rectangle::new((chip_x, chip_y).into(), (chip_w, chip_h).into()),
                icon_rect,
                label,
                distance_text,
                distance_rect,
                distance_pos,
                alpha: (ui_mix * distance_fade).clamp(0.0, 1.0),
            });
        }
    }

    out
}

pub(crate) fn bearing_hit_test(
    st: &Halley,
    screen_w: i32,
    screen_h: i32,
    monitor: &str,
    sx: f32,
    sy: f32,
) -> Option<NodeId> {
    let ui_mix = st
        .render_state
        .bearings_mix
        .get(monitor)
        .copied()
        .unwrap_or_else(|| if st.bearings_visible() { 1.0 } else { 0.0 });
    if ui_mix <= 0.05 {
        return None;
    }
    let px = sx.round() as i32;
    let py = sy.round() as i32;
    collect_bearing_layouts(st, screen_w, screen_h, monitor, ui_mix)
        .into_iter()
        .find(|layout| rect_contains(layout.chip_rect, px, py))
        .map(|layout| layout.node_id)
}

pub(crate) fn draw_bearings(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    damage: Rectangle<i32, Physical>,
    layouts: &[BearingChipLayout],
) -> Result<(), Box<dyn Error>> {
    for layout in layouts {
        if layout.alpha <= 0.002 {
            continue;
        }

        let fill = Color32F::new(0.92, 0.95, 0.98, 0.92 * layout.alpha);
        draw_shader_label(
            frame,
            st,
            layout.chip_rect.loc.x,
            layout.chip_rect.loc.y,
            layout.chip_rect.size.w,
            layout.chip_rect.size.h,
            11.0,
            0.0,
            layout.alpha,
            Color32F::new(0.92, 0.95, 0.98, 0.0),
            fill,
            damage,
        )?;

        if let Some(distance_rect) = layout.distance_rect
            && let Some((distance_x, distance_y)) = layout.distance_pos
            && let Some(distance_text) = layout.distance_text.as_ref()
        {
            draw_shader_label(
                frame,
                st,
                distance_rect.loc.x,
                distance_rect.loc.y,
                distance_rect.size.w,
                distance_rect.size.h,
                8.0,
                0.0,
                layout.alpha,
                Color32F::new(0.10, 0.14, 0.18, 0.0),
                Color32F::new(0.10, 0.14, 0.18, 0.84 * layout.alpha),
                damage,
            )?;
            draw_bitmap_text(
                frame,
                distance_x,
                distance_y,
                distance_text,
                META_SCALE,
                Color32F::new(0.86, 0.92, 0.98, layout.alpha * 0.96),
                damage,
            )?;
        }

        let text_x = layout.chip_rect.loc.x
            + CHIP_PAD_X
            + if layout.icon_rect.is_some() {
                ICON_SIZE + CHIP_PAD_X.saturating_sub(4)
            } else {
                0
            };
        let (_, label_h) = bitmap_text_size(layout.label.as_str(), LABEL_SCALE);
        let text_y = layout.chip_rect.loc.y + (layout.chip_rect.size.h - label_h) / 2;
        draw_bitmap_text(
            frame,
            text_x,
            text_y,
            layout.label.as_str(),
            LABEL_SCALE,
            Color32F::new(0.08, 0.10, 0.12, layout.alpha),
            damage,
        )?;

        if let Some(icon_rect) = layout.icon_rect {
            draw_bearing_icon(frame, st, layout.node_id, icon_rect, damage, layout.alpha)?;
        }
    }

    Ok(())
}

fn bearing_origin(
    bearing: Bearing,
    screen_w: i32,
    screen_h: i32,
    chip_w: i32,
    chip_h: i32,
    distance_h: i32,
    index: usize,
    stack_h: i32,
) -> (i32, i32) {
    let idx = index as i32;
    match bearing {
        Bearing::N => ((screen_w - chip_w) / 2, EDGE_PAD + idx * stack_h + distance_h),
        Bearing::S => (
            (screen_w - chip_w) / 2,
            screen_h - EDGE_PAD - chip_h - idx * stack_h,
        ),
        Bearing::W => (
            EDGE_PAD,
            (screen_h - chip_h) / 2 + idx * stack_h - distance_h / 2,
        ),
        Bearing::E => (
            screen_w - EDGE_PAD - chip_w,
            (screen_h - chip_h) / 2 + idx * stack_h - distance_h / 2,
        ),
        Bearing::NW => (EDGE_PAD, EDGE_PAD + idx * stack_h + distance_h),
        Bearing::NE => (
            screen_w - EDGE_PAD - chip_w,
            EDGE_PAD + idx * stack_h + distance_h,
        ),
        Bearing::SW => (EDGE_PAD, screen_h - EDGE_PAD - chip_h - idx * stack_h),
        Bearing::SE => (
            screen_w - EDGE_PAD - chip_w,
            screen_h - EDGE_PAD - chip_h - idx * stack_h,
        ),
    }
}

fn bearing_index(bearing: Bearing) -> usize {
    match bearing {
        Bearing::NW => 0,
        Bearing::N => 1,
        Bearing::NE => 2,
        Bearing::W => 3,
        Bearing::E => 4,
        Bearing::SW => 5,
        Bearing::S => 6,
        Bearing::SE => 7,
    }
}

fn bearing_label(st: &Halley, node_id: NodeId, title: &str) -> String {
    let base = if !title.trim().is_empty() {
        title.trim().to_string()
    } else if let Some(app_id) = st.node_app_ids.get(&node_id) {
        app_id.clone()
    } else {
        format!("Node {}", node_id.as_u64())
    };
    truncate_label(base.as_str())
}

fn offscreen_distance_from_view_edge(st: &Halley, monitor: &str, node_id: NodeId) -> Option<f32> {
    let node = st.field.node(node_id)?;
    let ext = st.spawn_obstacle_extents_for_node(node);
    let viewport = st.viewport_for_monitor(monitor);
    let min_x = viewport.center.x - viewport.size.x * 0.5;
    let max_x = viewport.center.x + viewport.size.x * 0.5;
    let min_y = viewport.center.y - viewport.size.y * 0.5;
    let max_y = viewport.center.y + viewport.size.y * 0.5;

    let node_min_x = node.pos.x - ext.left;
    let node_max_x = node.pos.x + ext.right;
    let node_min_y = node.pos.y - ext.top;
    let node_max_y = node.pos.y + ext.bottom;

    let overflow_x = if node_max_x <= min_x {
        min_x - node_max_x
    } else if node_min_x >= max_x {
        node_min_x - max_x
    } else {
        0.0
    };
    let overflow_y = if node_max_y <= min_y {
        min_y - node_max_y
    } else if node_min_y >= max_y {
        node_min_y - max_y
    } else {
        0.0
    };

    Some((overflow_x * overflow_x + overflow_y * overflow_y).sqrt())
}

fn truncate_label(label: &str) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for ch in label.chars() {
        if count >= MAX_LABEL_CHARS {
            break;
        }
        out.push(ch);
        count += 1;
    }
    if label.chars().count() > MAX_LABEL_CHARS {
        out.push_str("...");
    }
    out
}

fn rect_contains(rect: Rectangle<i32, Physical>, x: i32, y: i32) -> bool {
    x >= rect.loc.x
        && x < rect.loc.x + rect.size.w
        && y >= rect.loc.y
        && y < rect.loc.y + rect.size.h
}

fn draw_bearing_icon(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    node_id: NodeId,
    rect: Rectangle<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    match node_app_icon_entry(st, node_id) {
        Some(NodeAppIconCacheEntry::Ready(icon)) => {
            let src = Rectangle::<f64, Buffer>::new(
                (0.0, 0.0).into(),
                (icon.width as f64, icon.height as f64).into(),
            );
            frame.render_texture_from_to(
                &icon.texture,
                src,
                rect,
                &[damage],
                &[],
                Transform::Normal,
                alpha,
                None,
                &[],
            )?;
        }
        _ => {
            draw_rect(
                frame,
                rect.loc.x,
                rect.loc.y,
                rect.size.w,
                rect.size.h,
                Color32F::new(0.22, 0.82, 0.92, alpha * 0.30),
                damage,
            )?;
        }
    }
    Ok(())
}

fn draw_shader_label(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    corner_radius: f32,
    border_px: f32,
    alpha: f32,
    border_color: Color32F,
    fill_color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let Some(texture) = st.render_state.node_circle_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = st.render_state.node_label_program.as_ref() else {
        return Ok(());
    };

    let dest = Rectangle::<i32, Physical>::new((x, y).into(), (w.max(1), h.max(1)).into());
    let tex_size = texture.size();
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (tex_size.w as f64, tex_size.h as f64).into(),
    );
    let uniforms = [
        Uniform::new(
            "node_color",
            (
                border_color.r(),
                border_color.g(),
                border_color.b(),
                border_color.a(),
            ),
        ),
        Uniform::new(
            "fill_color",
            (
                fill_color.r(),
                fill_color.g(),
                fill_color.b(),
                fill_color.a(),
            ),
        ),
        Uniform::new("rect_size", (w as f32, h as f32)),
        Uniform::new("corner_radius", corner_radius),
        Uniform::new("border_px", border_px),
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
