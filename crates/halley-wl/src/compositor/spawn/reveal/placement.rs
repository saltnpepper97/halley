use std::time::Instant;

use crate::compositor::overlap::system::CollisionExtents;
use crate::compositor::root::Halley;
use crate::compositor::spawn::read;
use crate::compositor::spawn::rules::{InitialWindowIntent, ResolvedInitialWindowRule};
use crate::compositor::spawn::state::{InitialSpawnPlacement, SpawnPlacementExtents};
use crate::window::active_window_frame_pad_px;
use eventline::debug;
use halley_config::{InitialWindowOverlapPolicy, InitialWindowSpawnPlacement};
use halley_core::field::{NodeId, Vec2};
use halley_core::viewport::Viewport;

const SPAWN_COLLISION_SAFETY_SCALE: f32 = 1.08;
const SPAWN_CONTACT_MARGIN: f32 = 4.0;

#[derive(Clone, Copy, Debug)]
struct SpawnAnchor {
    node: Option<NodeId>,
    pos: Vec2,
    ext: Option<CollisionExtents>,
}

#[derive(Clone, Copy, Debug)]
struct SpawnCandidate {
    pos: Vec2,
    dir: Option<Vec2>,
}

/// Spawn candidates are tried in a deterministic star pattern:
/// center, then right, left, up, down for each ring.
fn spawn_cardinal_dirs() -> [Vec2; 4] {
    [
        Vec2 { x: 1.0, y: 0.0 },
        Vec2 { x: -1.0, y: 0.0 },
        Vec2 { x: 0.0, y: -1.0 },
        Vec2 { x: 0.0, y: 1.0 },
    ]
}

fn spawn_candidate_extents(size: Vec2, frame_pad: f32) -> CollisionExtents {
    let half_w = (size.x * 0.5 + frame_pad) * SPAWN_COLLISION_SAFETY_SCALE + SPAWN_CONTACT_MARGIN;
    let half_h = (size.y * 0.5 + frame_pad) * SPAWN_COLLISION_SAFETY_SCALE + SPAWN_CONTACT_MARGIN;
    CollisionExtents {
        left: half_w,
        right: half_w,
        top: half_h,
        bottom: half_h,
    }
}

fn spawn_safe_obstacle_extents(ext: CollisionExtents) -> CollisionExtents {
    CollisionExtents {
        left: ext.left * SPAWN_COLLISION_SAFETY_SCALE + SPAWN_CONTACT_MARGIN,
        right: ext.right * SPAWN_COLLISION_SAFETY_SCALE + SPAWN_CONTACT_MARGIN,
        top: ext.top * SPAWN_COLLISION_SAFETY_SCALE + SPAWN_CONTACT_MARGIN,
        bottom: ext.bottom * SPAWN_COLLISION_SAFETY_SCALE + SPAWN_CONTACT_MARGIN,
    }
}

fn spawn_record_extents(ext: CollisionExtents) -> SpawnPlacementExtents {
    SpawnPlacementExtents {
        left: ext.left,
        right: ext.right,
        top: ext.top,
        bottom: ext.bottom,
    }
}

fn spawn_dir_from_delta(delta: Vec2) -> Option<Vec2> {
    if delta.x.abs() <= 0.5 && delta.y.abs() <= 0.5 {
        return None;
    }
    if delta.x.abs() >= delta.y.abs() {
        Some(Vec2 {
            x: delta.x.signum(),
            y: 0.0,
        })
    } else {
        Some(Vec2 {
            x: 0.0,
            y: delta.y.signum(),
        })
    }
}

fn spawn_candidate_for_snapshot_dir(
    focus_pos: Vec2,
    focus_size: Vec2,
    size: Vec2,
    dir: Vec2,
    gap: f32,
    frame_pad: f32,
) -> Vec2 {
    let focus_ext = spawn_safe_obstacle_extents(CollisionExtents {
        left: focus_size.x * 0.5 + frame_pad,
        right: focus_size.x * 0.5 + frame_pad,
        top: focus_size.y * 0.5 + frame_pad,
        bottom: focus_size.y * 0.5 + frame_pad,
    });
    let candidate_ext = spawn_candidate_extents(size, frame_pad);
    if dir.x > 0.0 {
        Vec2 {
            x: focus_pos.x + focus_ext.right + candidate_ext.left + gap,
            y: focus_pos.y,
        }
    } else if dir.x < 0.0 {
        Vec2 {
            x: focus_pos.x - focus_ext.left - candidate_ext.right - gap,
            y: focus_pos.y,
        }
    } else if dir.y > 0.0 {
        Vec2 {
            x: focus_pos.x,
            y: focus_pos.y + focus_ext.bottom + candidate_ext.top + gap,
        }
    } else {
        Vec2 {
            x: focus_pos.x,
            y: focus_pos.y - focus_ext.top - candidate_ext.bottom - gap,
        }
    }
}

const SPAWN_STAR_RINGS: usize = 24;

pub(crate) fn default_window_rule() -> ResolvedInitialWindowRule {
    ResolvedInitialWindowRule::default()
}

pub(crate) fn has_default_window_rule(intent: &InitialWindowIntent) -> bool {
    intent.rule == default_window_rule()
        && intent.parent_node.is_none()
        && !intent.prefer_app_intent
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn viewport_center_for_monitor(st: &Halley, monitor: &str) -> Vec2 {
    read::spawn_read_context(st).viewport_center_for_monitor(monitor)
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn resolve_spawn_target_monitor(st: &Halley) -> String {
    read::spawn_read_context(st).resolve_spawn_target_monitor()
}

#[cfg(test)]
pub(crate) fn current_spawn_focus(st: &Halley, monitor: &str) -> (Option<NodeId>, Vec2) {
    read::spawn_read_context(st).current_spawn_focus(monitor)
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn viewport_fully_contains_surface_on_monitor(
    st: &Halley,
    monitor: &str,
    id: NodeId,
) -> bool {
    st.surface_is_fully_visible_on_monitor(monitor, id)
}

#[cfg(test)]
pub(crate) fn right_spawn_candidate_for_focus(st: &Halley, id: NodeId, size: Vec2) -> Option<Vec2> {
    spawn_candidate_for_focus_dir(st, id, size, Vec2 { x: 1.0, y: 0.0 })
}

pub(crate) fn spawn_candidate_for_focus_dir(
    st: &Halley,
    id: NodeId,
    size: Vec2,
    dir: Vec2,
) -> Option<Vec2> {
    let node = st.model.field.node(id)?;
    let focus_ext = spawn_safe_obstacle_extents_for_node(st, node);
    let candidate_ext =
        spawn_candidate_extents(size, active_window_frame_pad_px(&st.runtime.tuning) as f32);
    let gap = st.non_overlap_gap_world() + SPAWN_CONTACT_MARGIN;
    let pos = if dir.x > 0.0 {
        Vec2 {
            x: node.pos.x + focus_ext.right + candidate_ext.left + gap,
            y: node.pos.y,
        }
    } else if dir.x < 0.0 {
        Vec2 {
            x: node.pos.x - focus_ext.left - candidate_ext.right - gap,
            y: node.pos.y,
        }
    } else if dir.y > 0.0 {
        Vec2 {
            x: node.pos.x,
            y: node.pos.y + focus_ext.bottom + candidate_ext.top + gap,
        }
    } else {
        Vec2 {
            x: node.pos.x,
            y: node.pos.y - focus_ext.top - candidate_ext.bottom - gap,
        }
    };
    let pos = if dir.x == 0.0 && dir.y != 0.0 {
        let monitor = st
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
        adjust_vertical_candidate_to_row(st, monitor.as_str(), node.pos, pos, size)
    } else {
        pos
    };
    Some(pos)
}

pub(crate) fn spawn_star_step_x(st: &Halley, size: Vec2) -> f32 {
    spawn_candidate_extents(size, active_window_frame_pad_px(&st.runtime.tuning) as f32)
        .size()
        .x
        + st.non_overlap_gap_world()
        + SPAWN_CONTACT_MARGIN
}

pub(crate) fn spawn_star_step_y(st: &Halley, size: Vec2) -> f32 {
    spawn_candidate_extents(size, active_window_frame_pad_px(&st.runtime.tuning) as f32)
        .size()
        .y
        + st.non_overlap_gap_world()
        + SPAWN_CONTACT_MARGIN
}

pub(crate) fn star_candidate_offsets(st: &Halley, size: Vec2) -> Vec<Vec2> {
    let step_x = spawn_star_step_x(st, size);
    let step_y = spawn_star_step_y(st, size);
    let mut out = Vec::with_capacity(1 + SPAWN_STAR_RINGS * spawn_cardinal_dirs().len());

    out.push(Vec2 { x: 0.0, y: 0.0 });

    for ring in 1..=SPAWN_STAR_RINGS {
        for dir in spawn_cardinal_dirs() {
            out.push(Vec2 {
                x: dir.x * step_x * ring as f32,
                y: dir.y * step_y * ring as f32,
            });
        }
    }

    out
}

pub(crate) fn viewport_for_monitor(st: &Halley, monitor: &str) -> Option<Viewport> {
    if st.model.monitor_state.current_monitor == monitor {
        return Some(Viewport::new(
            st.model.viewport.center,
            crate::compositor::monitor::camera::camera_view_size(st),
        ));
    }
    st.model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| Viewport::new(space.viewport.center, space.zoom_ref_size))
}

pub(crate) fn world_from_monitor_screen(
    st: &Halley,
    monitor: &str,
    sx: f32,
    sy: f32,
) -> Option<Vec2> {
    let (w, h, local_sx, local_sy) = st.local_screen_in_monitor(monitor, sx, sy);
    let viewport = viewport_for_monitor(st, monitor)?;
    let w = (w as f32).max(1.0);
    let h = (h as f32).max(1.0);
    let nx = (local_sx / w) - 0.5;
    let ny = (local_sy / h) - 0.5;
    Some(Vec2 {
        x: viewport.center.x + nx * viewport.size.x.max(1.0),
        y: viewport.center.y + ny * viewport.size.y.max(1.0),
    })
}

pub(crate) fn spawn_candidate_fits(
    st: &Halley,
    monitor: &str,
    pos: Vec2,
    size: Vec2,
    skip_node: Option<NodeId>,
) -> bool {
    spawn_candidate_fits_with_policy(
        st,
        monitor,
        pos,
        size,
        skip_node,
        InitialWindowOverlapPolicy::None,
        None,
    )
}

pub(crate) fn spawn_candidate_fits_with_policy(
    st: &Halley,
    monitor: &str,
    pos: Vec2,
    size: Vec2,
    skip_node: Option<NodeId>,
    _overlap_policy: InitialWindowOverlapPolicy,
    _parent_node: Option<NodeId>,
) -> bool {
    let pair_gap = st.non_overlap_gap_world();
    let candidate =
        spawn_candidate_extents(size, active_window_frame_pad_px(&st.runtime.tuning) as f32);
    !st.model.field.nodes().values().any(|other| {
        if Some(other.id) == skip_node {
            return false;
        }
        let Some((other_pos, other_ext)) = visible_spawn_obstacle(st, monitor, other.id) else {
            return false;
        };
        let req_x = st.required_sep_x(pos.x, candidate, other_pos.x, other_ext, pair_gap);
        let req_y = st.required_sep_y(pos.y, candidate, other_pos.y, other_ext, pair_gap);
        (pos.x - other_pos.x).abs() < req_x && (pos.y - other_pos.y).abs() < req_y
    })
}

pub(crate) fn spawn_candidate_fits_with_view_obstacles(
    st: &Halley,
    monitor: &str,
    pos: Vec2,
    size: Vec2,
    skip_node: Option<NodeId>,
) -> bool {
    let pair_gap = st.non_overlap_gap_world();
    let candidate =
        spawn_candidate_extents(size, active_window_frame_pad_px(&st.runtime.tuning) as f32);
    !st.model.field.nodes().values().any(|other| {
        if Some(other.id) == skip_node {
            return false;
        }
        let Some((other_pos, other_ext)) = visible_spawn_obstacle(st, monitor, other.id) else {
            return false;
        };
        if !obstacle_intersects_current_view(st, monitor, other_pos, other_ext) {
            return false;
        }
        let req_x = st.required_sep_x(pos.x, candidate, other_pos.x, other_ext, pair_gap);
        let req_y = st.required_sep_y(pos.y, candidate, other_pos.y, other_ext, pair_gap);
        (pos.x - other_pos.x).abs() < req_x && (pos.y - other_pos.y).abs() < req_y
    })
}

pub(crate) fn obstacle_intersects_current_view(
    st: &Halley,
    monitor: &str,
    pos: Vec2,
    ext: CollisionExtents,
) -> bool {
    let view = st.usable_viewport_for_monitor(monitor);
    let left = view.center.x - view.size.x * 0.5;
    let right = view.center.x + view.size.x * 0.5;
    let top = view.center.y - view.size.y * 0.5;
    let bottom = view.center.y + view.size.y * 0.5;
    let obstacle_left = pos.x - ext.left;
    let obstacle_right = pos.x + ext.right;
    let obstacle_top = pos.y - ext.top;
    let obstacle_bottom = pos.y + ext.bottom;

    obstacle_right >= left
        && obstacle_left <= right
        && obstacle_bottom >= top
        && obstacle_top <= bottom
}

pub(crate) fn visible_spawn_obstacle(
    st: &Halley,
    monitor: &str,
    id: NodeId,
) -> Option<(Vec2, CollisionExtents)> {
    let other = st.model.field.node(id)?;
    let is_pinned_landmark = other.pinned
        && matches!(
            other.state,
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
        );
    if !is_pinned_landmark
        || !st.model.field.is_visible(id)
        || st
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .is_some_and(|other_monitor| other_monitor != monitor)
    {
        return None;
    }

    Some((other.pos, spawn_safe_obstacle_extents_for_node(st, other)))
}

fn spawn_anchor_for_node(st: &Halley, id: NodeId) -> Option<SpawnAnchor> {
    st.model.field.node(id).map(|node| SpawnAnchor {
        node: Some(id),
        pos: node.pos,
        ext: Some(spawn_safe_obstacle_extents_for_node(st, node)),
    })
}

fn spawn_anchor_at(pos: Vec2) -> SpawnAnchor {
    SpawnAnchor {
        node: None,
        pos,
        ext: None,
    }
}

pub(crate) fn spawn_safe_obstacle_extents_for_node(
    st: &Halley,
    node: &halley_core::field::Node,
) -> CollisionExtents {
    spawn_safe_obstacle_extents(st.spawn_obstacle_extents_for_node(node))
}

pub(crate) fn view_center_hits_spawn_node(st: &Halley, monitor: &str, id: NodeId) -> bool {
    let Some(node) = st.model.field.node(id) else {
        return false;
    };
    let center = st.usable_viewport_for_monitor(monitor).center;
    let ext = spawn_safe_obstacle_extents_for_node(st, node);
    center.x >= node.pos.x - ext.left
        && center.x <= node.pos.x + ext.right
        && center.y >= node.pos.y - ext.top
        && center.y <= node.pos.y + ext.bottom
}

pub(crate) fn view_center_hits_spawn_snapshot(
    st: &Halley,
    monitor: &str,
    pos: Vec2,
    size: Vec2,
) -> bool {
    let center = st.usable_viewport_for_monitor(monitor).center;
    let frame_pad = active_window_frame_pad_px(&st.runtime.tuning) as f32;
    let ext = spawn_safe_obstacle_extents(CollisionExtents {
        left: size.x * 0.5 + frame_pad,
        right: size.x * 0.5 + frame_pad,
        top: size.y * 0.5 + frame_pad,
        bottom: size.y * 0.5 + frame_pad,
    });
    center.x >= pos.x - ext.left
        && center.x <= pos.x + ext.right
        && center.y >= pos.y - ext.top
        && center.y <= pos.y + ext.bottom
}

pub(crate) fn point_is_spawn_view_center(st: &Halley, monitor: &str, pos: Vec2) -> bool {
    let center = st.usable_viewport_for_monitor(monitor).center;
    (pos.x - center.x).abs() <= 0.5 && (pos.y - center.y).abs() <= 0.5
}

pub(crate) fn monitor_has_visible_spawn_surface(st: &Halley, monitor: &str) -> bool {
    st.model.field.nodes().values().any(|node| {
        node.kind == halley_core::field::NodeKind::Surface
            && st.model.field.is_visible(node.id)
            && st
                .model
                .monitor_state
                .node_monitor
                .get(&node.id)
                .is_some_and(|node_monitor| node_monitor == monitor)
    })
}

pub(crate) fn occupied_spawn_bounds(
    st: &Halley,
    monitor: &str,
    skip_node: Option<NodeId>,
) -> Option<(f32, f32, f32, f32)> {
    let mut bounds: Option<(f32, f32, f32, f32)> = None;
    for other in st.model.field.nodes().values() {
        if Some(other.id) == skip_node {
            continue;
        }
        let Some((pos, ext)) = visible_spawn_obstacle(st, monitor, other.id) else {
            continue;
        };
        let left = pos.x - ext.left;
        let right = pos.x + ext.right;
        let top = pos.y - ext.top;
        let bottom = pos.y + ext.bottom;
        bounds = Some(match bounds {
            Some((current_left, current_right, current_top, current_bottom)) => (
                current_left.min(left),
                current_right.max(right),
                current_top.min(top),
                current_bottom.max(bottom),
            ),
            None => (left, right, top, bottom),
        });
    }
    bounds
}

pub(crate) fn safe_spawn_fallback_outside_occupied(
    st: &Halley,
    monitor: &str,
    anchor: Vec2,
    size: Vec2,
    skip_node: Option<NodeId>,
    overlap_policy: InitialWindowOverlapPolicy,
    parent_node: Option<NodeId>,
) -> Vec2 {
    let Some((left, right, top, bottom)) = occupied_spawn_bounds(st, monitor, skip_node) else {
        return anchor;
    };
    let candidate =
        spawn_candidate_extents(size, active_window_frame_pad_px(&st.runtime.tuning) as f32);
    let gap = st.non_overlap_gap_world() + SPAWN_CONTACT_MARGIN;
    let candidates = [
        Vec2 {
            x: right + candidate.left + gap,
            y: anchor.y,
        },
        Vec2 {
            x: left - candidate.right - gap,
            y: anchor.y,
        },
        Vec2 {
            x: anchor.x,
            y: top - candidate.bottom - gap,
        },
        Vec2 {
            x: anchor.x,
            y: bottom + candidate.top + gap,
        },
    ];

    candidates
        .into_iter()
        .find(|pos| {
            spawn_candidate_fits_with_policy(
                st,
                monitor,
                *pos,
                size,
                skip_node,
                overlap_policy,
                parent_node,
            )
        })
        .unwrap_or(candidates[0])
}

pub(crate) fn adjust_vertical_candidate_to_row(
    st: &Halley,
    monitor: &str,
    center: Vec2,
    mut pos: Vec2,
    size: Vec2,
) -> Vec2 {
    let dy = pos.y - center.y;
    let dir_y = if dy > 0.0 {
        1.0
    } else if dy < 0.0 {
        -1.0
    } else {
        0.0
    };
    if dir_y == 0.0 || pos.x != center.x {
        return pos;
    }

    let candidate =
        spawn_candidate_extents(size, active_window_frame_pad_px(&st.runtime.tuning) as f32);
    let gap = st.non_overlap_gap_world() * 2.0 + SPAWN_CONTACT_MARGIN;
    let mut row_top: Option<f32> = None;
    let mut row_bottom: Option<f32> = None;

    for other in st.model.field.nodes().values() {
        let Some((other_pos, other_ext)) = visible_spawn_obstacle(st, monitor, other.id) else {
            continue;
        };
        let top = other_pos.y - other_ext.top;
        let bottom = other_pos.y + other_ext.bottom;
        if center.y < top || center.y > bottom {
            continue;
        }

        row_top = Some(row_top.map_or(top, |current| current.min(top)));
        row_bottom = Some(row_bottom.map_or(bottom, |current| current.max(bottom)));
    }

    if dir_y < 0.0 {
        if let Some(row_top) = row_top {
            let y = row_top - gap - candidate.bottom;
            if pos.y > y {
                pos.y = y;
            }
        }
    } else if let Some(row_bottom) = row_bottom {
        let y = row_bottom + gap + candidate.top;
        if pos.y < y {
            pos.y = y;
        }
    }

    pos
}

pub(crate) fn vertical_row_center_for_candidate(
    st: &Halley,
    monitor: &str,
    anchor_pos: Vec2,
    dir_y: f32,
    size: Vec2,
    skip_node: Option<NodeId>,
) -> Option<f32> {
    let candidate =
        spawn_candidate_extents(size, active_window_frame_pad_px(&st.runtime.tuning) as f32);
    let gap = st.non_overlap_gap_world() * 2.0 + SPAWN_CONTACT_MARGIN;
    let mut row_top: Option<f32> = None;
    let mut row_bottom: Option<f32> = None;

    for other in st.model.field.nodes().values() {
        if Some(other.id) == skip_node {
            continue;
        }
        let Some((other_pos, other_ext)) = visible_spawn_obstacle(st, monitor, other.id) else {
            continue;
        };
        let top = other_pos.y - other_ext.top;
        let bottom = other_pos.y + other_ext.bottom;
        if anchor_pos.y < top || anchor_pos.y > bottom {
            continue;
        }

        row_top = Some(row_top.map_or(top, |current| current.min(top)));
        row_bottom = Some(row_bottom.map_or(bottom, |current| current.max(bottom)));
    }

    if dir_y < 0.0 {
        row_top.map(|top| top - gap - candidate.bottom)
    } else if dir_y > 0.0 {
        row_bottom.map(|bottom| bottom + gap + candidate.top)
    } else {
        None
    }
}

pub(crate) fn try_spawn_star_with_policy(
    st: &Halley,
    monitor: &str,
    center: Vec2,
    size: Vec2,
    overlap_policy: InitialWindowOverlapPolicy,
    parent_node: Option<NodeId>,
) -> Option<Vec2> {
    try_spawn_star_with_policy_skip(st, monitor, center, size, overlap_policy, parent_node, None)
}

pub(crate) fn try_spawn_star_with_policy_skip(
    st: &Halley,
    monitor: &str,
    center: Vec2,
    size: Vec2,
    overlap_policy: InitialWindowOverlapPolicy,
    parent_node: Option<NodeId>,
    skip_node: Option<NodeId>,
) -> Option<Vec2> {
    for offset in star_candidate_offsets(st, size) {
        let pos = Vec2 {
            x: center.x + offset.x,
            y: center.y + offset.y,
        };
        let pos = adjust_vertical_candidate_to_row(st, monitor, center, pos, size);
        if spawn_candidate_fits_with_policy(
            st,
            monitor,
            pos,
            size,
            skip_node,
            overlap_policy,
            parent_node,
        ) {
            return Some(pos);
        }
    }

    Some(safe_spawn_fallback_outside_occupied(
        st,
        monitor,
        center,
        size,
        skip_node,
        overlap_policy,
        parent_node,
    ))
}

pub(crate) fn try_strict_spawn_star_with_policy_skip(
    st: &Halley,
    monitor: &str,
    center: Vec2,
    size: Vec2,
    overlap_policy: InitialWindowOverlapPolicy,
    parent_node: Option<NodeId>,
    skip_node: Option<NodeId>,
) -> Option<Vec2> {
    if spawn_candidate_fits_with_policy(
        st,
        monitor,
        center,
        size,
        skip_node,
        overlap_policy,
        parent_node,
    ) {
        return Some(center);
    }

    for ring in 1..=SPAWN_STAR_RINGS {
        for dir in spawn_cardinal_dirs() {
            let pos = strict_spawn_arm_candidate(st, monitor, center, size, dir, ring);
            if spawn_candidate_fits_with_policy(
                st,
                monitor,
                pos,
                size,
                skip_node,
                overlap_policy,
                parent_node,
            ) {
                return Some(pos);
            }
        }
    }

    Some(safe_spawn_fallback_outside_occupied(
        st,
        monitor,
        center,
        size,
        skip_node,
        overlap_policy,
        parent_node,
    ))
}

pub(crate) fn try_view_center_spawn_star(
    st: &Halley,
    monitor: &str,
    center: Vec2,
    size: Vec2,
    skip_node: Option<NodeId>,
) -> Option<Vec2> {
    for offset in star_candidate_offsets(st, size) {
        let pos = Vec2 {
            x: center.x + offset.x,
            y: center.y + offset.y,
        };
        if spawn_candidate_fits_with_view_obstacles(st, monitor, pos, size, skip_node) {
            return Some(pos);
        }
    }

    None
}

pub(crate) fn strict_spawn_arm_candidate(
    st: &Halley,
    monitor: &str,
    center: Vec2,
    size: Vec2,
    dir: Vec2,
    occurrence: usize,
) -> Vec2 {
    let candidate =
        spawn_candidate_extents(size, active_window_frame_pad_px(&st.runtime.tuning) as f32);
    let gap = st.non_overlap_gap_world() + SPAWN_CONTACT_MARGIN;
    let mut bases: Vec<(Vec2, CollisionExtents)> = st
        .model
        .field
        .nodes()
        .values()
        .filter_map(|other| visible_spawn_obstacle(st, monitor, other.id))
        .filter(|(pos, ext)| {
            if dir.x > 0.0 {
                pos.x >= center.x - 0.5
                    && center.y >= pos.y - ext.top
                    && center.y <= pos.y + ext.bottom
            } else if dir.x < 0.0 {
                pos.x <= center.x + 0.5
                    && center.y >= pos.y - ext.top
                    && center.y <= pos.y + ext.bottom
            } else if dir.y < 0.0 {
                pos.y <= center.y + 0.5
                    && center.x >= pos.x - ext.left
                    && center.x <= pos.x + ext.right
            } else {
                pos.y >= center.y - 0.5
                    && center.x >= pos.x - ext.left
                    && center.x <= pos.x + ext.right
            }
        })
        .collect();

    bases.sort_by(|(a, _), (b, _)| {
        let ordering = if dir.x > 0.0 {
            a.x.partial_cmp(&b.x)
        } else if dir.x < 0.0 {
            b.x.partial_cmp(&a.x)
        } else if dir.y < 0.0 {
            b.y.partial_cmp(&a.y)
        } else {
            a.y.partial_cmp(&b.y)
        };
        ordering.unwrap_or(std::cmp::Ordering::Equal)
    });

    if let Some((base_pos, base_ext)) = bases.get(occurrence.saturating_sub(1)).copied() {
        if dir.x > 0.0 {
            return Vec2 {
                x: base_pos.x + base_ext.right + candidate.left + gap,
                y: center.y,
            };
        }
        if dir.x < 0.0 {
            return Vec2 {
                x: base_pos.x - base_ext.left - candidate.right - gap,
                y: center.y,
            };
        }
        if dir.y < 0.0 {
            return Vec2 {
                x: center.x,
                y: base_pos.y - base_ext.top - candidate.bottom - gap,
            };
        }
        return Vec2 {
            x: center.x,
            y: base_pos.y + base_ext.bottom + candidate.top + gap,
        };
    }

    let offset = Vec2 {
        x: dir.x * spawn_star_step_x(st, size) * occurrence as f32,
        y: dir.y * spawn_star_step_y(st, size) * occurrence as f32,
    };
    Vec2 {
        x: center.x + offset.x,
        y: center.y + offset.y,
    }
}

pub(crate) fn try_strict_spawn_star(
    st: &Halley,
    monitor: &str,
    center: Vec2,
    size: Vec2,
) -> Option<Vec2> {
    try_strict_spawn_star_with_policy_skip(
        st,
        monitor,
        center,
        size,
        InitialWindowOverlapPolicy::None,
        None,
        None,
    )
}

pub(crate) fn resolve_parent_monitor(st: &Halley, parent_node: Option<NodeId>) -> Option<String> {
    parent_node.and_then(|id| st.model.monitor_state.node_monitor.get(&id).cloned())
}

pub(crate) fn fullscreen_anchor_for_monitor(st: &Halley, monitor: &str) -> Option<(NodeId, Vec2)> {
    let fullscreen_id = st
        .model
        .fullscreen_state
        .fullscreen_active_node
        .get(monitor)
        .copied()?;
    let pos = st.model.field.node(fullscreen_id).map(|node| node.pos)?;
    Some((fullscreen_id, pos))
}

pub(crate) fn spawn_target_monitor_for_intent(st: &Halley, intent: &InitialWindowIntent) -> String {
    let default_monitor = read::spawn_read_context(st).resolve_spawn_target_monitor();
    match intent.effective_spawn_placement() {
        InitialWindowSpawnPlacement::Default
        | InitialWindowSpawnPlacement::Center
        | InitialWindowSpawnPlacement::Adjacent
        | InitialWindowSpawnPlacement::App => {
            resolve_parent_monitor(st, intent.parent_node).unwrap_or(default_monitor)
        }
        InitialWindowSpawnPlacement::Cursor => {
            if let Some((sx, sy)) = st.input.interaction_state.last_pointer_screen_global {
                st.monitor_for_screen(sx, sy).unwrap_or(default_monitor)
            } else {
                default_monitor
            }
        }
        InitialWindowSpawnPlacement::ViewportCenter => default_monitor,
    }
}

pub(crate) fn pick_cluster_growth_dir(st: &Halley, monitor: &str, center: Vec2) -> Vec2 {
    let dirs = spawn_cardinal_dirs();
    let local = st
        .model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|monitor| Vec2 {
            x: center.x - monitor.offset_x as f32,
            y: center.y - monitor.offset_y as f32,
        })
        .unwrap_or(center);
    let idx = ((st.spawn_monitor_state(monitor).spawn_cursor as usize)
        .wrapping_add(local.x.abs() as usize)
        .wrapping_add((local.y.abs() * 3.0) as usize))
        % dirs.len();
    dirs[idx]
}

pub(crate) fn set_pending_initial_spawn_placement(
    st: &mut Halley,
    monitor: &str,
    anchor_pos: Vec2,
    anchor_ext: Option<CollisionExtents>,
    chosen_pos: Vec2,
    dir: Option<Vec2>,
    preserve_chosen_pos: bool,
    view_center_reset: bool,
    _overlap_policy: InitialWindowOverlapPolicy,
) {
    st.model.spawn_state.pending_initial_spawn_placement = Some(InitialSpawnPlacement {
        monitor: monitor.to_string(),
        anchor_pos,
        anchor_ext: anchor_ext.map(spawn_record_extents),
        chosen_pos,
        dir,
        preserve_chosen_pos,
        view_center_reset,
    });
}

pub(crate) fn finalize_initial_spawn_position(st: &mut Halley, id: NodeId, size: Vec2) -> bool {
    let Some(record) = st.model.spawn_state.initial_spawn_placements.remove(&id) else {
        return false;
    };
    let mut pos = record.chosen_pos;

    if record.view_center_reset {
        pos = try_view_center_spawn_star(
            st,
            record.monitor.as_str(),
            record.anchor_pos,
            size,
            Some(id),
        )
        .unwrap_or(record.anchor_pos);
    } else if !record.preserve_chosen_pos {
        let Some(dir) = record.dir else {
            return false;
        };
        let candidate =
            spawn_candidate_extents(size, active_window_frame_pad_px(&st.runtime.tuning) as f32);
        let pair_gap = st.non_overlap_gap_world() + SPAWN_CONTACT_MARGIN;
        let row_gap = pair_gap * 2.0;

        if dir.x > 0.0 {
            if let Some(anchor) = record.anchor_ext {
                pos.x = record.anchor_pos.x + anchor.right + candidate.left + pair_gap;
                pos.y = record.anchor_pos.y;
            }
        } else if dir.x < 0.0 {
            if let Some(anchor) = record.anchor_ext {
                pos.x = record.anchor_pos.x - anchor.left - candidate.right - pair_gap;
                pos.y = record.anchor_pos.y;
            }
        } else if dir.y != 0.0 {
            pos.x = record.anchor_pos.x;
            if let Some(y) = vertical_row_center_for_candidate(
                st,
                record.monitor.as_str(),
                record.anchor_pos,
                dir.y,
                size,
                Some(id),
            ) {
                pos.y = y;
            } else if let Some(anchor) = record.anchor_ext {
                pos.y = if dir.y < 0.0 {
                    record.anchor_pos.y - anchor.top - row_gap - candidate.bottom
                } else {
                    record.anchor_pos.y + anchor.bottom + row_gap + candidate.top
                };
            }
        }
    }

    if !record.view_center_reset
        && !spawn_candidate_fits_with_policy(
            st,
            record.monitor.as_str(),
            pos,
            size,
            Some(id),
            InitialWindowOverlapPolicy::None,
            None,
        )
        && let Some(fallback) = try_strict_spawn_star_with_policy_skip(
            st,
            record.monitor.as_str(),
            record.anchor_pos,
            size,
            InitialWindowOverlapPolicy::None,
            None,
            Some(id),
        )
    {
        pos = fallback;
    }

    let _ = st.model.field.carry(id, pos);
    true
}

pub(crate) fn update_spawn_patch(
    st: &mut Halley,
    monitor: &str,
    anchor: Vec2,
    focus_node: Option<NodeId>,
    focus_pos: Vec2,
    growth_dir: Vec2,
) {
    st.spawn_monitor_state_mut(monitor).spawn_patch =
        Some(crate::compositor::spawn::state::SpawnPatch {
            anchor,
            focus_node,
            focus_pos,
            growth_dir,
            placements_in_patch: 0,
            frontier: Vec::new(),
        });
}

fn commit_spawn_plan(
    st: &mut Halley,
    monitor: &str,
    anchor: SpawnAnchor,
    candidate: SpawnCandidate,
    overlap_policy: InitialWindowOverlapPolicy,
) {
    set_pending_initial_spawn_placement(
        st,
        monitor,
        anchor.pos,
        anchor.ext,
        candidate.pos,
        candidate.dir,
        false,
        false,
        overlap_policy,
    );
    let growth_dir = candidate
        .dir
        .unwrap_or_else(|| pick_cluster_growth_dir(st, monitor, anchor.pos));
    update_spawn_patch(st, monitor, anchor.pos, anchor.node, anchor.pos, growth_dir);
    st.spawn_monitor_state_mut(monitor).spawn_view_anchor = anchor.pos;
}

pub(crate) fn default_pick_spawn_position(st: &mut Halley, size: Vec2) -> (String, Vec2, bool) {
    pick_spawn_position_impl(st, size)
}

#[allow(dead_code)]
pub(crate) fn pick_spawn_position(st: &mut Halley, size: Vec2) -> (String, Vec2, bool) {
    default_pick_spawn_position(st, size)
}

pub(crate) fn pick_spawn_position_with_intent(
    st: &mut Halley,
    size: Vec2,
    intent: &InitialWindowIntent,
) -> (String, Vec2, bool) {
    if has_default_window_rule(intent) {
        return default_pick_spawn_position(st, size);
    }

    let target_monitor = spawn_target_monitor_for_intent(st, intent);
    let overlap_policy = intent.effective_overlap_policy();
    let placement = intent.effective_spawn_placement();
    st.spawn_monitor_state_mut(target_monitor.as_str())
        .spawn_cursor += 1;
    let viewport_center =
        read::spawn_read_context(st).viewport_center_for_monitor(target_monitor.as_str());
    let fullscreen_anchor = if overlap_policy != InitialWindowOverlapPolicy::None {
        fullscreen_anchor_for_monitor(st, target_monitor.as_str())
    } else {
        None
    };
    let cursor_anchor = st
        .input
        .interaction_state
        .last_pointer_screen_global
        .and_then(|(sx, sy)| world_from_monitor_screen(st, target_monitor.as_str(), sx, sy));

    match placement {
        InitialWindowSpawnPlacement::Adjacent => {
            if let Some(parent_id) = intent.parent_node
                && let Some(anchor) = spawn_anchor_for_node(st, parent_id)
            {
                if overlap_policy != InitialWindowOverlapPolicy::None
                    && fullscreen_anchor
                        .is_some_and(|(fullscreen_id, _)| fullscreen_id == parent_id)
                {
                    let candidate = SpawnCandidate {
                        pos: anchor.pos,
                        dir: None,
                    };
                    commit_spawn_plan(
                        st,
                        target_monitor.as_str(),
                        anchor,
                        candidate,
                        overlap_policy,
                    );
                    return (target_monitor, candidate.pos, false);
                }

                for dir in spawn_cardinal_dirs() {
                    if let Some(pos) =
                        spawn_candidate_for_focus_dir(st, parent_id, size, dir).map(|pos| {
                            adjust_vertical_candidate_to_row(
                                st,
                                target_monitor.as_str(),
                                anchor.pos,
                                pos,
                                size,
                            )
                        })
                        && spawn_candidate_fits_with_policy(
                            st,
                            target_monitor.as_str(),
                            pos,
                            size,
                            None,
                            overlap_policy,
                            intent.parent_node,
                        )
                    {
                        let candidate = SpawnCandidate {
                            pos,
                            dir: Some(dir),
                        };
                        commit_spawn_plan(
                            st,
                            target_monitor.as_str(),
                            anchor,
                            candidate,
                            overlap_policy,
                        );
                        return (target_monitor, candidate.pos, false);
                    }
                }

                if let Some(pos) = try_spawn_star_with_policy(
                    st,
                    target_monitor.as_str(),
                    anchor.pos,
                    size,
                    overlap_policy,
                    intent.parent_node,
                ) {
                    let candidate = SpawnCandidate {
                        pos,
                        dir: spawn_dir_from_delta(Vec2 {
                            x: pos.x - anchor.pos.x,
                            y: pos.y - anchor.pos.y,
                        }),
                    };
                    commit_spawn_plan(
                        st,
                        target_monitor.as_str(),
                        anchor,
                        candidate,
                        overlap_policy,
                    );
                    return (target_monitor, candidate.pos, false);
                }

                return default_pick_spawn_position(st, size);
            }
            return default_pick_spawn_position(st, size);
        }
        InitialWindowSpawnPlacement::Default
        | InitialWindowSpawnPlacement::Center
        | InitialWindowSpawnPlacement::ViewportCenter
        | InitialWindowSpawnPlacement::Cursor
        | InitialWindowSpawnPlacement::App => {}
    }

    let anchor = match placement {
        InitialWindowSpawnPlacement::Default
        | InitialWindowSpawnPlacement::Center
        | InitialWindowSpawnPlacement::App => intent
            .parent_node
            .and_then(|id| spawn_anchor_for_node(st, id))
            .or_else(|| {
                fullscreen_anchor.and_then(|(id, pos)| {
                    spawn_anchor_for_node(st, id).or_else(|| Some(spawn_anchor_at(pos)))
                })
            })
            .unwrap_or_else(|| spawn_anchor_at(viewport_center)),
        InitialWindowSpawnPlacement::ViewportCenter => spawn_anchor_at(viewport_center),
        InitialWindowSpawnPlacement::Cursor => {
            spawn_anchor_at(cursor_anchor.unwrap_or(viewport_center))
        }
        InitialWindowSpawnPlacement::Adjacent => unreachable!("adjacent handled above"),
    };

    let candidate = try_spawn_star_with_policy(
        st,
        target_monitor.as_str(),
        anchor.pos,
        size,
        overlap_policy,
        intent.parent_node,
    )
    .map(|pos| SpawnCandidate {
        pos,
        dir: spawn_dir_from_delta(Vec2 {
            x: pos.x - anchor.pos.x,
            y: pos.y - anchor.pos.y,
        }),
    })
    .unwrap_or(SpawnCandidate {
        pos: anchor.pos,
        dir: None,
    });
    commit_spawn_plan(
        st,
        target_monitor.as_str(),
        anchor,
        candidate,
        overlap_policy,
    );
    (target_monitor, candidate.pos, false)
}

pub(crate) fn pick_spawn_position_impl(st: &mut Halley, size: Vec2) -> (String, Vec2, bool) {
    let target_monitor = st
        .model
        .spawn_state
        .pending_spawn_monitor
        .take()
        .filter(|monitor| st.model.monitor_state.monitors.contains_key(monitor))
        .unwrap_or_else(|| read::spawn_read_context(st).resolve_spawn_target_monitor());
    let focus_override = st
        .spawn_monitor_state_mut(target_monitor.as_str())
        .spawn_focus_override
        .take();
    st.spawn_monitor_state_mut(target_monitor.as_str())
        .spawn_cursor += 1;
    let viewport_center =
        read::spawn_read_context(st).viewport_center_for_monitor(target_monitor.as_str());
    let (focus_id, focus_pos) =
        read::spawn_read_context(st).current_spawn_focus(target_monitor.as_str());
    if !monitor_has_visible_spawn_surface(st, target_monitor.as_str()) {
        st.spawn_monitor_state_mut(target_monitor.as_str())
            .spawn_patch = None;
    }
    let monitor_spawn = st.spawn_monitor_state(target_monitor.as_str());
    debug!(
        "spawn target resolved: target_monitor={} focused_monitor={} interaction_monitor={} anchor_mode={:?} focus_id={:?}",
        target_monitor,
        st.focused_monitor(),
        st.interaction_monitor(),
        monitor_spawn.spawn_anchor_mode,
        focus_id.map(|id| id.as_u64())
    );
    if let Some(override_focus) = focus_override.filter(|override_focus| {
        view_center_hits_spawn_snapshot(
            st,
            target_monitor.as_str(),
            override_focus.pos,
            override_focus.size,
        )
    }) {
        let gap = st.non_overlap_gap_world() + SPAWN_CONTACT_MARGIN;
        let frame_pad = active_window_frame_pad_px(&st.runtime.tuning) as f32;
        let override_ext = spawn_safe_obstacle_extents(CollisionExtents {
            left: override_focus.size.x * 0.5 + frame_pad,
            right: override_focus.size.x * 0.5 + frame_pad,
            top: override_focus.size.y * 0.5 + frame_pad,
            bottom: override_focus.size.y * 0.5 + frame_pad,
        });
        for dir in spawn_cardinal_dirs() {
            let pos = spawn_candidate_for_snapshot_dir(
                override_focus.pos,
                override_focus.size,
                size,
                dir,
                gap,
                frame_pad,
            );
            let pos = adjust_vertical_candidate_to_row(
                st,
                target_monitor.as_str(),
                override_focus.pos,
                pos,
                size,
            );
            if spawn_candidate_fits(st, target_monitor.as_str(), pos, size, None) {
                set_pending_initial_spawn_placement(
                    st,
                    target_monitor.as_str(),
                    override_focus.pos,
                    Some(override_ext),
                    pos,
                    Some(dir),
                    false,
                    false,
                    InitialWindowOverlapPolicy::None,
                );
                update_spawn_patch(
                    st,
                    target_monitor.as_str(),
                    override_focus.pos,
                    None,
                    override_focus.pos,
                    dir,
                );
                st.spawn_monitor_state_mut(target_monitor.as_str())
                    .spawn_view_anchor = override_focus.pos;
                debug!(
                    "spawn position picked from override: target_monitor={} anchor=({:.1},{:.1}) chosen=({:.1},{:.1}) size=({:.1},{:.1})",
                    target_monitor,
                    override_focus.pos.x,
                    override_focus.pos.y,
                    pos.x,
                    pos.y,
                    size.x,
                    size.y
                );
                return (target_monitor, pos, false);
            }
        }
        if let Some(pos) =
            try_strict_spawn_star(st, target_monitor.as_str(), override_focus.pos, size)
        {
            set_pending_initial_spawn_placement(
                st,
                target_monitor.as_str(),
                override_focus.pos,
                Some(override_ext),
                pos,
                spawn_dir_from_delta(Vec2 {
                    x: pos.x - override_focus.pos.x,
                    y: pos.y - override_focus.pos.y,
                }),
                true,
                false,
                InitialWindowOverlapPolicy::None,
            );
            let growth_dir =
                pick_cluster_growth_dir(st, target_monitor.as_str(), override_focus.pos);
            update_spawn_patch(
                st,
                target_monitor.as_str(),
                override_focus.pos,
                None,
                override_focus.pos,
                growth_dir,
            );
            st.spawn_monitor_state_mut(target_monitor.as_str())
                .spawn_view_anchor = override_focus.pos;
            debug!(
                "spawn position picked from override fallback: target_monitor={} anchor=({:.1},{:.1}) chosen=({:.1},{:.1}) size=({:.1},{:.1})",
                target_monitor,
                override_focus.pos.x,
                override_focus.pos.y,
                pos.x,
                pos.y,
                size.x,
                size.y
            );
            return (target_monitor, pos, false);
        }
    }

    let patch = monitor_spawn.spawn_patch.as_ref();
    let patch_anchor_active = patch.is_some_and(|patch| {
        patch
            .focus_node
            .is_some_and(|id| view_center_hits_spawn_node(st, target_monitor.as_str(), id))
            || (patch.focus_node.is_none()
                && point_is_spawn_view_center(st, target_monitor.as_str(), patch.anchor))
    });
    let focus_anchor_id = focus_id.filter(|id| {
        let focus_continues_active_patch = patch.is_some_and(|patch| {
            patch_anchor_active && patch.focus_node.is_some() && patch.focus_node != Some(*id)
        });
        view_center_hits_spawn_node(st, target_monitor.as_str(), *id)
            || focus_continues_active_patch
    });
    let focus_moved_from_patch = patch.is_some_and(|patch| {
        focus_anchor_id.is_some()
            && ((focus_pos.x - patch.anchor.x).abs() > 0.5
                || (focus_pos.y - patch.anchor.y).abs() > 0.5)
    });
    let use_patch_anchor = patch.is_some()
        && monitor_has_visible_spawn_surface(st, target_monitor.as_str())
        && patch_anchor_active
        && !focus_moved_from_patch;
    let anchor = if use_patch_anchor {
        patch.map(|patch| patch.anchor).unwrap_or(viewport_center)
    } else if focus_anchor_id.is_some() {
        focus_pos
    } else if focus_id.is_some() {
        viewport_center
    } else {
        focus_pos
    };
    let reset_to_view_center = !use_patch_anchor && focus_id.is_some() && focus_anchor_id.is_none();
    let anchor_node = if use_patch_anchor {
        patch.and_then(|patch| patch.focus_node).or(focus_anchor_id)
    } else {
        focus_anchor_id
    };
    let view_center_reset = reset_to_view_center || anchor_node.is_none();
    let anchor_ext = anchor_node.and_then(|id| {
        st.model.field.node(id).and_then(|node| {
            ((node.pos.x - anchor.x).abs() <= 0.5 && (node.pos.y - anchor.y).abs() <= 0.5)
                .then(|| spawn_safe_obstacle_extents_for_node(st, node))
        })
    });
    let pos = if reset_to_view_center {
        try_view_center_spawn_star(st, target_monitor.as_str(), anchor, size, None)
    } else {
        try_strict_spawn_star(st, target_monitor.as_str(), anchor, size)
    }
    .unwrap_or(anchor);
    set_pending_initial_spawn_placement(
        st,
        target_monitor.as_str(),
        anchor,
        anchor_ext,
        pos,
        spawn_dir_from_delta(Vec2 {
            x: pos.x - anchor.x,
            y: pos.y - anchor.y,
        }),
        true,
        view_center_reset,
        InitialWindowOverlapPolicy::None,
    );
    let growth_dir = pick_cluster_growth_dir(st, target_monitor.as_str(), anchor);
    update_spawn_patch(
        st,
        target_monitor.as_str(),
        anchor,
        anchor_node,
        anchor,
        growth_dir,
    );
    st.spawn_monitor_state_mut(target_monitor.as_str())
        .spawn_view_anchor = anchor;
    debug!(
        "spawn position picked: target_monitor={} anchor=({:.1},{:.1}) focus_pos=({:.1},{:.1}) chosen=({:.1},{:.1}) size=({:.1},{:.1})",
        target_monitor, anchor.x, anchor.y, focus_pos.x, focus_pos.y, pos.x, pos.y, size.x, size.y
    );
    (target_monitor, pos, false)
}
