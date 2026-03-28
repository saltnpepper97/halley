use std::error::Error;

use halley_core::{
    bearings::{Bearing, bearings_for_visible_nodes},
    field::{NodeId, Vec2},
    viewport::Viewport,
};
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
const EDGE_PAD: i32 = 16;
const LABEL_SCALE: i32 = 2;
const META_SCALE: i32 = 1;
const ICON_SIZE: i32 = 16;
const ICON_TEXT_GAP: i32 = 6;
const DISTANCE_GAP: i32 = 4;
const META_PAD_X: i32 = 7;
const META_PAD_Y: i32 = 4;
const GROUP_GAP: i32 = 10;
const MAX_LABEL_CHARS: usize = 24;
const MIN_DISTANCE_ALPHA: f32 = 0.34;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BearingLane {
    NW,
    N,
    NE,
    W,
    E,
    SW,
    S,
    SE,
}

impl BearingLane {
    fn all() -> [Self; 8] {
        [
            Self::NW,
            Self::N,
            Self::NE,
            Self::W,
            Self::E,
            Self::SW,
            Self::S,
            Self::SE,
        ]
    }

    fn from_bearing(bearing: Bearing) -> Self {
        match bearing {
            Bearing::NW => Self::NW,
            Bearing::N => Self::N,
            Bearing::NE => Self::NE,
            Bearing::W => Self::W,
            Bearing::E => Self::E,
            Bearing::SW => Self::SW,
            Bearing::S => Self::S,
            Bearing::SE => Self::SE,
        }
    }

    fn uses_horizontal_axis(self) -> bool {
        matches!(self, Self::N | Self::S)
    }
}

#[derive(Clone, Copy, Debug)]
struct BearingSize {
    chip_w: i32,
    chip_h: i32,
    show_icon: bool,
    distance_rect_w: i32,
    distance_rect_h: i32,
    distance_block_h: i32,
}

impl BearingSize {
    fn total_height(self) -> i32 {
        self.distance_block_h + self.chip_h
    }

    fn primary_extent(self, lane: BearingLane) -> i32 {
        if lane.uses_horizontal_axis() {
            self.chip_w
        } else {
            self.total_height()
        }
    }
}

#[derive(Clone, Debug)]
struct BearingCandidate {
    node_id: NodeId,
    lane: BearingLane,
    projected: f32,
    distance: f32,
    label: String,
    size: BearingSize,
}

#[derive(Clone, Debug)]
struct BearingGroup {
    lane: BearingLane,
    projected: f32,
    node_id: NodeId,
    label: String,
    distance: f32,
    size: BearingSize,
    alpha: f32,
}

pub(crate) fn ensure_bearing_icon_resources(
    renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
    st: &mut Halley,
    monitor: &str,
) -> Result<(), Box<dyn Error>> {
    let viewport = bearings_viewport_for_monitor(st, monitor);
    let node_ids = bearings_for_visible_nodes(&st.model.field, &viewport)
        .into_iter()
        .filter_map(|(id, _)| {
            let node = st.model.field.node(id)?;
            (node.kind == halley_core::field::NodeKind::Surface
                && st
                    .model
                    .monitor_state
                    .node_monitor
                    .get(&id)
                    .map(String::as_str)
                    == Some(monitor)
                && !node_intersects_bearings_view(st, monitor, id))
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

    let viewport = bearings_viewport_for_monitor(st, monitor);
    let mut candidates = Vec::new();
    for (id, bearing) in bearings_for_visible_nodes(&st.model.field, &viewport) {
        let Some(node) = st.model.field.node(id) else {
            continue;
        };
        if node.kind != halley_core::field::NodeKind::Surface {
            continue;
        }
        if st
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .map(String::as_str)
            != Some(monitor)
        {
            continue;
        }
        if node_intersects_bearings_view(st, monitor, id) {
            continue;
        }

        let lane = BearingLane::from_bearing(bearing);
        let distance = offscreen_distance_from_monitor_edge(st, monitor, id).unwrap_or(0.0);
        let label = bearing_label(st, id, node.label.as_str());
        let projected = projected_anchor_for_lane(st, monitor, id, lane, screen_w, screen_h);
        let distance_text = st
            .runtime
            .tuning
            .bearings
            .show_distance
            .then(|| format!("{:.0}px", distance.round()));
        let size = bearing_size(
            label.as_str(),
            st.runtime.tuning.bearings.show_icons,
            distance_text.as_deref(),
        );

        candidates.push(BearingCandidate {
            node_id: id,
            lane,
            projected,
            distance,
            label,
            size,
        });
    }

    let mut layouts = Vec::new();
    for lane in BearingLane::all() {
        let mut lane_candidates = candidates
            .iter()
            .filter(|candidate| candidate.lane == lane)
            .cloned()
            .collect::<Vec<_>>();
        if lane_candidates.is_empty() {
            continue;
        }
        lane_candidates.sort_by(|a, b| {
            a.projected
                .partial_cmp(&b.projected)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.node_id.as_u64().cmp(&b.node_id.as_u64()))
        });

        let groups = group_lane_candidates(st, lane_candidates, ui_mix);
        layouts.extend(layout_lane_groups(st, lane, groups, screen_w, screen_h));
    }

    layouts
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
        .ui
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
            Color32F::new(0.92, 0.95, 0.98, 0.92 * layout.alpha),
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
                ICON_SIZE + ICON_TEXT_GAP
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

fn group_lane_candidates(
    st: &Halley,
    candidates: Vec<BearingCandidate>,
    ui_mix: f32,
) -> Vec<BearingGroup> {
    let mut groups = Vec::new();
    let mut current: Vec<BearingCandidate> = Vec::new();

    for candidate in candidates {
        if let Some(previous) = current.last() {
            let min_gap = crowding_threshold(previous.lane, previous.size, candidate.size);
            if candidate.projected - previous.projected > min_gap {
                groups.push(finalize_group(st, std::mem::take(&mut current), ui_mix));
            }
        }
        current.push(candidate);
    }

    if !current.is_empty() {
        groups.push(finalize_group(st, current, ui_mix));
    }

    groups
}

fn finalize_group(st: &Halley, members: Vec<BearingCandidate>, ui_mix: f32) -> BearingGroup {
    let member_count = members.len();
    let lane = members[0].lane;
    let nearest = members
        .iter()
        .min_by(|a, b| {
            a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.node_id.as_u64().cmp(&b.node_id.as_u64()))
        })
        .expect("bearing group should not be empty");
    let projected = members
        .iter()
        .map(|candidate| candidate.projected)
        .sum::<f32>()
        / member_count as f32;
    let label = if member_count == 1 {
        nearest.label.clone()
    } else {
        format!("{member_count} nodes")
    };
    let distance = nearest.distance;
    let distance_text = st
        .runtime
        .tuning
        .bearings
        .show_distance
        .then(|| format!("{:.0}px", distance.round()));
    let size = bearing_size(
        label.as_str(),
        member_count == 1 && st.runtime.tuning.bearings.show_icons,
        distance_text.as_deref(),
    );
    let distance_fade = if st.runtime.tuning.bearings.fade_distance <= f32::EPSILON {
        1.0
    } else {
        let t = (distance / st.runtime.tuning.bearings.fade_distance).clamp(0.0, 1.0);
        MIN_DISTANCE_ALPHA + (1.0 - t) * (1.0 - MIN_DISTANCE_ALPHA)
    };

    BearingGroup {
        lane,
        projected,
        node_id: nearest.node_id,
        label,
        distance,
        size,
        alpha: (ui_mix * distance_fade).clamp(0.0, 1.0),
    }
}

fn layout_lane_groups(
    st: &Halley,
    lane: BearingLane,
    groups: Vec<BearingGroup>,
    screen_w: i32,
    screen_h: i32,
) -> Vec<BearingChipLayout> {
    let mut groups = groups;
    if groups.is_empty() {
        return Vec::new();
    }

    groups.sort_by(|a, b| {
        a.projected
            .partial_cmp(&b.projected)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.node_id.as_u64().cmp(&b.node_id.as_u64()))
    });

    let mut centers = groups
        .iter()
        .map(|group| {
            let (min_center, max_center) = lane_center_bounds(lane, group.size, screen_w, screen_h);
            group.projected.clamp(min_center, max_center)
        })
        .collect::<Vec<_>>();

    for index in 1..centers.len() {
        let min_sep = crowding_threshold(lane, groups[index - 1].size, groups[index].size);
        let min_center = centers[index - 1] + min_sep;
        if centers[index] < min_center {
            centers[index] = min_center;
        }
    }
    for index in (0..centers.len().saturating_sub(1)).rev() {
        let (_, max_center) = lane_center_bounds(lane, groups[index].size, screen_w, screen_h);
        centers[index] = centers[index].min(max_center);
        let max_prev = centers[index + 1]
            - crowding_threshold(lane, groups[index].size, groups[index + 1].size);
        if centers[index] > max_prev {
            centers[index] = max_prev;
        }
    }
    for index in 1..centers.len() {
        let min_sep = crowding_threshold(lane, groups[index - 1].size, groups[index].size);
        let min_center = centers[index - 1] + min_sep;
        if centers[index] < min_center {
            centers[index] = min_center;
        }
    }

    groups
        .into_iter()
        .zip(centers)
        .map(|(group, center)| build_layout_from_group(st, group, center, screen_w, screen_h))
        .collect()
}

fn build_layout_from_group(
    st: &Halley,
    group: BearingGroup,
    center: f32,
    screen_w: i32,
    screen_h: i32,
) -> BearingChipLayout {
    let chip_w = group.size.chip_w;
    let chip_h = group.size.chip_h;
    let total_h = group.size.total_height();
    let total_top = (center.round() as i32) - total_h / 2;
    let chip_y_vertical = total_top + group.size.distance_block_h;
    let distance_text = st
        .runtime
        .tuning
        .bearings
        .show_distance
        .then(|| format!("{:.0}px", group.distance.round()));

    let (chip_x, chip_y) = match group.lane {
        BearingLane::N => (
            (center.round() as i32) - chip_w / 2,
            EDGE_PAD + group.size.distance_block_h,
        ),
        BearingLane::S => (
            (center.round() as i32) - chip_w / 2,
            screen_h - EDGE_PAD - chip_h,
        ),
        BearingLane::W => (EDGE_PAD, chip_y_vertical),
        BearingLane::E => (screen_w - EDGE_PAD - chip_w, chip_y_vertical),
        BearingLane::NW => (EDGE_PAD, chip_y_vertical),
        BearingLane::NE => (screen_w - EDGE_PAD - chip_w, chip_y_vertical),
        BearingLane::SW => (EDGE_PAD, chip_y_vertical),
        BearingLane::SE => (screen_w - EDGE_PAD - chip_w, chip_y_vertical),
    };

    let icon_rect = group.size.show_icon.then(|| {
        Rectangle::new(
            (chip_x + CHIP_PAD_X, chip_y + (chip_h - ICON_SIZE) / 2).into(),
            (ICON_SIZE, ICON_SIZE).into(),
        )
    });
    let distance_rect = distance_text.as_ref().map(|_| {
        Rectangle::new(
            (
                chip_x + ((chip_w - group.size.distance_rect_w) / 2),
                chip_y - DISTANCE_GAP - group.size.distance_rect_h,
            )
                .into(),
            (group.size.distance_rect_w, group.size.distance_rect_h).into(),
        )
    });
    let distance_pos = distance_text
        .as_ref()
        .zip(distance_rect)
        .map(|(text, rect)| {
            let (_, text_h) = bitmap_text_size(text, META_SCALE);
            (
                rect.loc.x + META_PAD_X,
                rect.loc.y + (rect.size.h - text_h) / 2,
            )
        });

    BearingChipLayout {
        node_id: group.node_id,
        chip_rect: Rectangle::new((chip_x, chip_y).into(), (chip_w, chip_h).into()),
        icon_rect,
        label: group.label,
        distance_text,
        distance_rect,
        distance_pos,
        alpha: group.alpha,
    }
}

fn bearing_size(label: &str, show_icon: bool, distance_text: Option<&str>) -> BearingSize {
    let (label_w, label_h) = bitmap_text_size(label, LABEL_SCALE);
    let icon_gap = if show_icon {
        ICON_SIZE + ICON_TEXT_GAP
    } else {
        0
    };
    let chip_w = (CHIP_PAD_X * 2 + icon_gap + label_w).max(44);
    let chip_h = (CHIP_PAD_Y * 2 + label_h.max(if show_icon { ICON_SIZE } else { 0 })).max(24);
    let (distance_rect_w, distance_rect_h, distance_block_h) = distance_text
        .map(|text| {
            let (text_w, text_h) = bitmap_text_size(text, META_SCALE);
            let rect_w = text_w + META_PAD_X * 2;
            let rect_h = text_h + META_PAD_Y * 2;
            (rect_w, rect_h, rect_h + DISTANCE_GAP)
        })
        .unwrap_or((0, 0, 0));

    BearingSize {
        chip_w,
        chip_h,
        show_icon,
        distance_rect_w,
        distance_rect_h,
        distance_block_h,
    }
}

fn crowding_threshold(lane: BearingLane, left: BearingSize, right: BearingSize) -> f32 {
    ((left.primary_extent(lane) + right.primary_extent(lane)) as f32 * 0.5) + GROUP_GAP as f32
}

fn lane_center_bounds(
    lane: BearingLane,
    size: BearingSize,
    screen_w: i32,
    screen_h: i32,
) -> (f32, f32) {
    if lane.uses_horizontal_axis() {
        let half = size.chip_w as f32 * 0.5;
        (
            EDGE_PAD as f32 + half,
            screen_w as f32 - EDGE_PAD as f32 - half,
        )
    } else {
        let half = size.total_height() as f32 * 0.5;
        (
            EDGE_PAD as f32 + half,
            screen_h as f32 - EDGE_PAD as f32 - half,
        )
    }
}

fn bearings_viewport_for_monitor(st: &Halley, monitor: &str) -> Viewport {
    let (center, size) = monitor_view_center_size(st, monitor);
    Viewport::new(center, size)
}

fn monitor_view_center_size(st: &Halley, monitor: &str) -> (Vec2, Vec2) {
    if st.model.monitor_state.current_monitor == monitor {
        (st.model.viewport.center, st.model.zoom_ref_size)
    } else {
        st.model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| (space.viewport.center, space.zoom_ref_size))
            .unwrap_or((st.model.viewport.center, st.model.zoom_ref_size))
    }
}

fn node_intersects_bearings_view(st: &Halley, monitor: &str, node_id: NodeId) -> bool {
    let Some(node) = st.model.field.node(node_id) else {
        return false;
    };
    let ext = bearings_collision_extents(st, node);
    let (center, size) = monitor_view_center_size(st, monitor);
    let min_x = center.x - size.x * 0.5;
    let max_x = center.x + size.x * 0.5;
    let min_y = center.y - size.y * 0.5;
    let max_y = center.y + size.y * 0.5;

    let node_min_x = node.pos.x - ext.left;
    let node_max_x = node.pos.x + ext.right;
    let node_min_y = node.pos.y - ext.top;
    let node_max_y = node.pos.y + ext.bottom;

    node_max_x > min_x && node_min_x < max_x && node_max_y > min_y && node_min_y < max_y
}

fn projected_anchor_for_lane(
    st: &Halley,
    monitor: &str,
    node_id: NodeId,
    lane: BearingLane,
    screen_w: i32,
    screen_h: i32,
) -> f32 {
    let Some(node) = st.model.field.node(node_id) else {
        return if lane.uses_horizontal_axis() {
            screen_w as f32 * 0.5
        } else {
            screen_h as f32 * 0.5
        };
    };
    let (center, size) = monitor_view_center_size(st, monitor);
    let dx = node.pos.x - center.x;
    let dy = node.pos.y - center.y;
    let half_w = size.x * 0.5;
    let half_h = size.y * 0.5;
    let tx = if dx.abs() <= f32::EPSILON {
        f32::INFINITY
    } else {
        half_w / dx.abs()
    };
    let ty = if dy.abs() <= f32::EPSILON {
        f32::INFINITY
    } else {
        half_h / dy.abs()
    };
    let t = tx.min(ty);
    let edge_x = center.x + dx * t;
    let edge_y = center.y + dy * t;

    let scalar = if lane.uses_horizontal_axis() {
        ((edge_x - (center.x - half_w)) / size.x.max(1.0)) * screen_w as f32
    } else {
        (((center.y + half_h) - edge_y) / size.y.max(1.0)) * screen_h as f32
    };

    scalar.clamp(
        EDGE_PAD as f32,
        if lane.uses_horizontal_axis() {
            screen_w as f32 - EDGE_PAD as f32
        } else {
            screen_h as f32 - EDGE_PAD as f32
        },
    )
}

fn bearing_label(st: &Halley, node_id: NodeId, title: &str) -> String {
    let base = if !title.trim().is_empty() {
        title.trim().to_string()
    } else if let Some(app_id) = st.model.node_app_ids.get(&node_id) {
        app_id.clone()
    } else {
        format!("Node {}", node_id.as_u64())
    };
    truncate_label(base.as_str())
}

fn offscreen_distance_from_monitor_edge(
    st: &Halley,
    monitor: &str,
    node_id: NodeId,
) -> Option<f32> {
    let node = st.model.field.node(node_id)?;
    let ext = bearings_collision_extents(st, node);
    let (center, size) = monitor_view_center_size(st, monitor);
    let min_x = center.x - size.x * 0.5;
    let max_x = center.x + size.x * 0.5;
    let min_y = center.y - size.y * 0.5;
    let max_y = center.y + size.y * 0.5;

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

fn bearings_collision_extents(
    st: &Halley,
    node: &halley_core::field::Node,
) -> crate::wm::overlap::CollisionExtents {
    match node.state {
        halley_core::field::NodeState::Node | halley_core::field::NodeState::Core => {
            st.collision_extents_for_node(node)
        }
        halley_core::field::NodeState::Active | halley_core::field::NodeState::Drifting => {
            st.surface_window_collision_extents(node)
        }
    }
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
    let Some(texture) = st.ui.render_state.node_circle_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = st.ui.render_state.node_label_program.as_ref() else {
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
        Uniform::new(
            "inner_rect_size",
            ((w as f32 - border_px * 2.0).max(1.0), (h as f32 - border_px * 2.0).max(1.0)),
        ),
        Uniform::new("inner_rect_offset", (border_px.max(0.0), border_px.max(0.0))),
        Uniform::new("corner_radius", corner_radius),
        Uniform::new("inner_corner_radius", (corner_radius - border_px).max(0.0)),
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
