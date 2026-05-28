use std::ops::{Deref, DerefMut};
use std::time::Instant;

use eventline::debug;
use halley_config::{InitialWindowOverlapPolicy, InitialWindowSpawnPlacement};
use halley_core::field::{NodeId, Vec2};
use halley_core::viewport::{FocusZone, Viewport};

use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::overlap::system::CollisionExtents;
use crate::compositor::root::Halley;
use crate::compositor::spawn::read;
use crate::compositor::spawn::rules::{InitialWindowIntent, ResolvedInitialWindowRule};
use crate::compositor::spawn::state::{
    InitialSpawnAuthority, InitialSpawnPlacement, SpawnPlacementExtents,
};
use crate::window::active_window_frame_pad_px;

use super::SpawnRevealController;

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

impl<T: Deref<Target = Halley>> SpawnRevealController<T> {
    const SPAWN_STAR_RINGS: usize = 24;

    fn default_window_rule() -> ResolvedInitialWindowRule {
        ResolvedInitialWindowRule::default()
    }

    fn has_default_window_rule(intent: &InitialWindowIntent) -> bool {
        intent.rule == Self::default_window_rule()
            && intent.parent_node.is_none()
            && !intent.prefer_app_intent
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn viewport_center_for_monitor(&self, monitor: &str) -> Vec2 {
        read::spawn_read_context(self).viewport_center_for_monitor(monitor)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn resolve_spawn_target_monitor(&self) -> String {
        read::spawn_read_context(self).resolve_spawn_target_monitor()
    }

    #[cfg(test)]
    pub(crate) fn current_spawn_focus(&self, monitor: &str) -> (Option<NodeId>, Vec2) {
        read::spawn_read_context(self).current_spawn_focus(monitor)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn viewport_fully_contains_surface_on_monitor(
        &self,
        monitor: &str,
        id: NodeId,
    ) -> bool {
        self.surface_is_fully_visible_on_monitor(monitor, id)
    }

    #[cfg(test)]
    pub(crate) fn right_spawn_candidate_for_focus(&self, id: NodeId, size: Vec2) -> Option<Vec2> {
        self.spawn_candidate_for_focus_dir(id, size, Vec2 { x: 1.0, y: 0.0 })
    }

    pub(crate) fn spawn_candidate_for_focus_dir(
        &self,
        id: NodeId,
        size: Vec2,
        dir: Vec2,
    ) -> Option<Vec2> {
        let node = self.model.field.node(id)?;
        let focus_ext = self.spawn_safe_obstacle_extents_for_node(node);
        let candidate_ext = spawn_candidate_extents(
            size,
            active_window_frame_pad_px(&self.runtime.tuning) as f32,
        );
        let gap = self.non_overlap_gap_world() + SPAWN_CONTACT_MARGIN;
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
            let monitor = self
                .model
                .monitor_state
                .node_monitor
                .get(&id)
                .cloned()
                .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone());
            self.adjust_vertical_candidate_to_row(monitor.as_str(), node.pos, pos, size)
        } else {
            pos
        };
        Some(pos)
    }

    pub(crate) fn spawn_star_step_x(&self, size: Vec2) -> f32 {
        spawn_candidate_extents(
            size,
            active_window_frame_pad_px(&self.runtime.tuning) as f32,
        )
        .size()
        .x + self.non_overlap_gap_world()
            + SPAWN_CONTACT_MARGIN
    }

    pub(crate) fn spawn_star_step_y(&self, size: Vec2) -> f32 {
        spawn_candidate_extents(
            size,
            active_window_frame_pad_px(&self.runtime.tuning) as f32,
        )
        .size()
        .y + self.non_overlap_gap_world()
            + SPAWN_CONTACT_MARGIN
    }

    #[cfg(test)]
    pub(crate) fn spawn_star_step(&self, size: Vec2) -> f32 {
        self.spawn_star_step_x(size)
            .max(self.spawn_star_step_y(size))
    }

    pub(crate) fn star_candidate_offsets(&self, size: Vec2) -> Vec<Vec2> {
        let step_x = self.spawn_star_step_x(size);
        let step_y = self.spawn_star_step_y(size);
        let mut out = Vec::with_capacity(1 + Self::SPAWN_STAR_RINGS * spawn_cardinal_dirs().len());

        out.push(Vec2 { x: 0.0, y: 0.0 });

        for ring in 1..=Self::SPAWN_STAR_RINGS {
            for dir in spawn_cardinal_dirs() {
                out.push(Vec2 {
                    x: dir.x * step_x * ring as f32,
                    y: dir.y * step_y * ring as f32,
                });
            }
        }

        out
    }

    fn viewport_for_monitor(&self, monitor: &str) -> Option<Viewport> {
        if self.model.monitor_state.current_monitor == monitor {
            return Some(Viewport::new(
                self.model.viewport.center,
                camera_controller(&**self).view_size(),
            ));
        }
        self.model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| Viewport::new(space.viewport.center, space.zoom_ref_size))
    }

    fn world_from_monitor_screen(&self, monitor: &str, sx: f32, sy: f32) -> Option<Vec2> {
        let (w, h, local_sx, local_sy) = self.local_screen_in_monitor(monitor, sx, sy);
        let viewport = self.viewport_for_monitor(monitor)?;
        let w = (w as f32).max(1.0);
        let h = (h as f32).max(1.0);
        let nx = (local_sx / w) - 0.5;
        let ny = (local_sy / h) - 0.5;
        Some(Vec2 {
            x: viewport.center.x + nx * viewport.size.x.max(1.0),
            y: viewport.center.y + ny * viewport.size.y.max(1.0),
        })
    }

    fn spawn_candidate_fits(
        &self,
        monitor: &str,
        pos: Vec2,
        size: Vec2,
        skip_node: Option<NodeId>,
    ) -> bool {
        self.spawn_candidate_fits_with_policy(
            monitor,
            pos,
            size,
            skip_node,
            InitialWindowOverlapPolicy::None,
            None,
        )
    }

    fn spawn_candidate_fits_with_policy(
        &self,
        monitor: &str,
        pos: Vec2,
        size: Vec2,
        skip_node: Option<NodeId>,
        overlap_policy: InitialWindowOverlapPolicy,
        parent_node: Option<NodeId>,
    ) -> bool {
        if overlap_policy == InitialWindowOverlapPolicy::All {
            return true;
        }
        let pair_gap = self.non_overlap_gap_world();
        let candidate = spawn_candidate_extents(
            size,
            active_window_frame_pad_px(&self.runtime.tuning) as f32,
        );
        !self.model.field.nodes().values().any(|other| {
            if Some(other.id) == skip_node {
                return false;
            }
            if overlap_policy == InitialWindowOverlapPolicy::ParentOnly
                && parent_node == Some(other.id)
            {
                return false;
            }
            let Some((other_pos, other_ext)) = self.visible_spawn_obstacle(monitor, other.id)
            else {
                return false;
            };
            let req_x = self.required_sep_x(pos.x, candidate, other_pos.x, other_ext, pair_gap);
            let req_y = self.required_sep_y(pos.y, candidate, other_pos.y, other_ext, pair_gap);
            (pos.x - other_pos.x).abs() < req_x && (pos.y - other_pos.y).abs() < req_y
        })
    }

    fn visible_spawn_obstacle(
        &self,
        monitor: &str,
        id: NodeId,
    ) -> Option<(Vec2, CollisionExtents)> {
        let other = self.model.field.node(id)?;
        if other.kind != halley_core::field::NodeKind::Surface
            || !self.model.field.is_visible(id)
            || self
                .model
                .monitor_state
                .node_monitor
                .get(&id)
                .is_some_and(|other_monitor| other_monitor != monitor)
        {
            return None;
        }

        if let Some(session) = crate::compositor::workspace::state::maximize_session_for_monitor(
            self, monitor,
        )
        .filter(|session| {
            session.state
                == crate::compositor::workspace::state::MaximizeSessionState::SpawnRestoring
        }) && let Some(snapshot) = session.node_snapshots.get(&id)
        {
            let half_w = snapshot.size.x.max(1.0) * 0.5
                + active_window_frame_pad_px(&self.runtime.tuning) as f32;
            let half_h = snapshot.size.y.max(1.0) * 0.5
                + active_window_frame_pad_px(&self.runtime.tuning) as f32;
            return Some((
                snapshot.pos,
                spawn_safe_obstacle_extents(CollisionExtents {
                    left: half_w,
                    right: half_w,
                    top: half_h,
                    bottom: half_h,
                }),
            ));
        }

        Some((other.pos, self.spawn_safe_obstacle_extents_for_node(other)))
    }

    fn spawn_anchor_for_node(&self, id: NodeId) -> Option<SpawnAnchor> {
        self.model.field.node(id).map(|node| SpawnAnchor {
            node: Some(id),
            pos: node.pos,
            ext: Some(self.spawn_safe_obstacle_extents_for_node(node)),
        })
    }

    fn spawn_anchor_at(&self, pos: Vec2) -> SpawnAnchor {
        SpawnAnchor {
            node: None,
            pos,
            ext: None,
        }
    }

    fn spawn_safe_obstacle_extents_for_node(
        &self,
        node: &halley_core::field::Node,
    ) -> CollisionExtents {
        spawn_safe_obstacle_extents(self.spawn_obstacle_extents_for_node(node))
    }

    fn node_is_in_spawn_active_area(&self, monitor: &str, id: NodeId) -> bool {
        let Some(node) = self.model.field.node(id) else {
            return false;
        };
        self.pos_is_in_spawn_active_area(monitor, node.pos)
    }

    fn pos_is_in_spawn_active_area(&self, monitor: &str, pos: Vec2) -> bool {
        let center = self.view_center_for_monitor(monitor);
        self.focus_ring_for_monitor(monitor).zone(center, pos) == FocusZone::Inside
    }

    fn monitor_has_visible_spawn_surface(&self, monitor: &str) -> bool {
        self.model.field.nodes().values().any(|node| {
            node.kind == halley_core::field::NodeKind::Surface
                && self.model.field.is_visible(node.id)
                && self
                    .model
                    .monitor_state
                    .node_monitor
                    .get(&node.id)
                    .is_some_and(|node_monitor| node_monitor == monitor)
        })
    }

    fn occupied_spawn_bounds(
        &self,
        monitor: &str,
        skip_node: Option<NodeId>,
    ) -> Option<(f32, f32, f32, f32)> {
        let mut bounds: Option<(f32, f32, f32, f32)> = None;
        for other in self.model.field.nodes().values() {
            if Some(other.id) == skip_node {
                continue;
            }
            let Some((pos, ext)) = self.visible_spawn_obstacle(monitor, other.id) else {
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

    fn safe_spawn_fallback_outside_occupied(
        &self,
        monitor: &str,
        anchor: Vec2,
        size: Vec2,
        skip_node: Option<NodeId>,
        overlap_policy: InitialWindowOverlapPolicy,
        parent_node: Option<NodeId>,
    ) -> Vec2 {
        let Some((left, right, top, bottom)) = self.occupied_spawn_bounds(monitor, skip_node)
        else {
            return anchor;
        };
        let candidate = spawn_candidate_extents(
            size,
            active_window_frame_pad_px(&self.runtime.tuning) as f32,
        );
        let gap = self.non_overlap_gap_world() + SPAWN_CONTACT_MARGIN;
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
                self.spawn_candidate_fits_with_policy(
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

    fn adjust_vertical_candidate_to_row(
        &self,
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

        let candidate = spawn_candidate_extents(
            size,
            active_window_frame_pad_px(&self.runtime.tuning) as f32,
        );
        let gap = self.non_overlap_gap_world() * 2.0 + SPAWN_CONTACT_MARGIN;
        let mut row_top: Option<f32> = None;
        let mut row_bottom: Option<f32> = None;

        for other in self.model.field.nodes().values() {
            let Some((other_pos, other_ext)) = self.visible_spawn_obstacle(monitor, other.id)
            else {
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

    fn vertical_row_center_for_candidate(
        &self,
        monitor: &str,
        anchor_pos: Vec2,
        dir_y: f32,
        size: Vec2,
        skip_node: Option<NodeId>,
    ) -> Option<f32> {
        let candidate = spawn_candidate_extents(
            size,
            active_window_frame_pad_px(&self.runtime.tuning) as f32,
        );
        let gap = self.non_overlap_gap_world() * 2.0 + SPAWN_CONTACT_MARGIN;
        let mut row_top: Option<f32> = None;
        let mut row_bottom: Option<f32> = None;

        for other in self.model.field.nodes().values() {
            if Some(other.id) == skip_node {
                continue;
            }
            let Some((other_pos, other_ext)) = self.visible_spawn_obstacle(monitor, other.id)
            else {
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

    fn try_spawn_star_with_policy(
        &self,
        monitor: &str,
        center: Vec2,
        size: Vec2,
        overlap_policy: InitialWindowOverlapPolicy,
        parent_node: Option<NodeId>,
    ) -> Option<Vec2> {
        self.try_spawn_star_with_policy_skip(
            monitor,
            center,
            size,
            overlap_policy,
            parent_node,
            None,
        )
    }

    fn try_spawn_star_with_policy_skip(
        &self,
        monitor: &str,
        center: Vec2,
        size: Vec2,
        overlap_policy: InitialWindowOverlapPolicy,
        parent_node: Option<NodeId>,
        skip_node: Option<NodeId>,
    ) -> Option<Vec2> {
        for offset in self.star_candidate_offsets(size) {
            let pos = Vec2 {
                x: center.x + offset.x,
                y: center.y + offset.y,
            };
            let pos = self.adjust_vertical_candidate_to_row(monitor, center, pos, size);
            if self.spawn_candidate_fits_with_policy(
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

        Some(self.safe_spawn_fallback_outside_occupied(
            monitor,
            center,
            size,
            skip_node,
            overlap_policy,
            parent_node,
        ))
    }

    fn try_strict_spawn_star_with_policy_skip(
        &self,
        monitor: &str,
        center: Vec2,
        size: Vec2,
        overlap_policy: InitialWindowOverlapPolicy,
        parent_node: Option<NodeId>,
        skip_node: Option<NodeId>,
    ) -> Option<Vec2> {
        if self.spawn_candidate_fits_with_policy(
            monitor,
            center,
            size,
            skip_node,
            overlap_policy,
            parent_node,
        ) {
            return Some(center);
        }

        for ring in 1..=Self::SPAWN_STAR_RINGS {
            for dir in spawn_cardinal_dirs() {
                let pos = self.strict_spawn_arm_candidate(monitor, center, size, dir, ring);
                if self.spawn_candidate_fits_with_policy(
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

        Some(self.safe_spawn_fallback_outside_occupied(
            monitor,
            center,
            size,
            skip_node,
            overlap_policy,
            parent_node,
        ))
    }

    fn strict_spawn_arm_candidate(
        &self,
        monitor: &str,
        center: Vec2,
        size: Vec2,
        dir: Vec2,
        occurrence: usize,
    ) -> Vec2 {
        let candidate = spawn_candidate_extents(
            size,
            active_window_frame_pad_px(&self.runtime.tuning) as f32,
        );
        let gap = self.non_overlap_gap_world() + SPAWN_CONTACT_MARGIN;
        let mut bases: Vec<(Vec2, CollisionExtents)> = self
            .model
            .field
            .nodes()
            .values()
            .filter_map(|other| self.visible_spawn_obstacle(monitor, other.id))
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
            x: dir.x * self.spawn_star_step_x(size) * occurrence as f32,
            y: dir.y * self.spawn_star_step_y(size) * occurrence as f32,
        };
        Vec2 {
            x: center.x + offset.x,
            y: center.y + offset.y,
        }
    }

    fn try_strict_spawn_star(&self, monitor: &str, center: Vec2, size: Vec2) -> Option<Vec2> {
        self.try_strict_spawn_star_with_policy_skip(
            monitor,
            center,
            size,
            InitialWindowOverlapPolicy::None,
            None,
            None,
        )
    }

    fn resolve_parent_monitor(&self, parent_node: Option<NodeId>) -> Option<String> {
        parent_node.and_then(|id| self.model.monitor_state.node_monitor.get(&id).cloned())
    }

    fn fullscreen_anchor_for_monitor(&self, monitor: &str) -> Option<(NodeId, Vec2)> {
        let fullscreen_id = self
            .model
            .fullscreen_state
            .fullscreen_active_node
            .get(monitor)
            .copied()?;
        let pos = self.model.field.node(fullscreen_id).map(|node| node.pos)?;
        Some((fullscreen_id, pos))
    }

    pub(crate) fn spawn_target_monitor_for_intent(&self, intent: &InitialWindowIntent) -> String {
        let default_monitor = read::spawn_read_context(self).resolve_spawn_target_monitor();
        match intent.effective_spawn_placement() {
            InitialWindowSpawnPlacement::Center
            | InitialWindowSpawnPlacement::Adjacent
            | InitialWindowSpawnPlacement::App => self
                .resolve_parent_monitor(intent.parent_node)
                .unwrap_or(default_monitor),
            InitialWindowSpawnPlacement::Cursor => {
                if let Some((sx, sy)) = self.input.interaction_state.last_pointer_screen_global {
                    self.monitor_for_screen(sx, sy).unwrap_or(default_monitor)
                } else {
                    default_monitor
                }
            }
            InitialWindowSpawnPlacement::ViewportCenter => default_monitor,
        }
    }

    fn pick_cluster_growth_dir(&self, monitor: &str, center: Vec2) -> Vec2 {
        let dirs = spawn_cardinal_dirs();
        let local = self
            .model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|monitor| Vec2 {
                x: center.x - monitor.offset_x as f32,
                y: center.y - monitor.offset_y as f32,
            })
            .unwrap_or(center);
        let idx = ((self.spawn_monitor_state(monitor).spawn_cursor as usize)
            .wrapping_add(local.x.abs() as usize)
            .wrapping_add((local.y.abs() * 3.0) as usize))
            % dirs.len();
        dirs[idx]
    }
}

impl<T: DerefMut<Target = Halley>> SpawnRevealController<T> {
    fn set_pending_initial_spawn_placement(
        &mut self,
        monitor: &str,
        anchor_node: Option<NodeId>,
        anchor_pos: Vec2,
        anchor_ext: Option<CollisionExtents>,
        chosen_pos: Vec2,
        dir: Option<Vec2>,
        preserve_chosen_pos: bool,
        overlap_policy: InitialWindowOverlapPolicy,
    ) {
        self.model.spawn_state.pending_initial_spawn_placement = Some(InitialSpawnPlacement {
            monitor: monitor.to_string(),
            anchor_node,
            anchor_pos,
            anchor_ext: anchor_ext.map(spawn_record_extents),
            chosen_pos,
            dir,
            preserve_chosen_pos,
            overlap_policy,
        });
    }

    pub(crate) fn finalize_initial_spawn_position(&mut self, id: NodeId, size: Vec2) -> bool {
        let Some(record) = self.model.spawn_state.initial_spawn_placements.remove(&id) else {
            return false;
        };
        if record.overlap_policy != InitialWindowOverlapPolicy::None {
            return false;
        }
        let mut pos = record.chosen_pos;

        if !record.preserve_chosen_pos {
            let Some(dir) = record.dir else {
                return false;
            };
            let candidate = spawn_candidate_extents(
                size,
                active_window_frame_pad_px(&self.runtime.tuning) as f32,
            );
            let pair_gap = self.non_overlap_gap_world() + SPAWN_CONTACT_MARGIN;
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
                if let Some(y) = self.vertical_row_center_for_candidate(
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

        if !self.spawn_candidate_fits_with_policy(
            record.monitor.as_str(),
            pos,
            size,
            Some(id),
            InitialWindowOverlapPolicy::None,
            None,
        ) && let Some(fallback) = self.try_strict_spawn_star_with_policy_skip(
            record.monitor.as_str(),
            record.anchor_pos,
            size,
            InitialWindowOverlapPolicy::None,
            None,
            Some(id),
        ) {
            pos = fallback;
        }

        let _ = self.model.field.carry(id, pos);
        if let Some(anchor_node) = record.anchor_node
            && anchor_node != id
            && self.model.field.node(anchor_node).is_some()
        {
            let duration_ms = self
                .runtime
                .tuning
                .window_open_duration_ms()
                .saturating_add(500)
                .max(900);
            let until_ms = self.now_ms(Instant::now()).saturating_add(duration_ms);
            self.model.spawn_state.initial_spawn_authority.insert(
                id,
                InitialSpawnAuthority {
                    anchor_node,
                    until_ms,
                },
            );
            self.input.interaction_state.physics_velocity.remove(&id);
            self.input
                .interaction_state
                .physics_velocity
                .remove(&anchor_node);
        }
        true
    }

    pub(crate) fn update_spawn_patch(
        &mut self,
        monitor: &str,
        anchor: Vec2,
        focus_node: Option<NodeId>,
        focus_pos: Vec2,
        growth_dir: Vec2,
    ) {
        self.spawn_monitor_state_mut(monitor).spawn_patch =
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
        &mut self,
        monitor: &str,
        anchor: SpawnAnchor,
        candidate: SpawnCandidate,
        overlap_policy: InitialWindowOverlapPolicy,
    ) {
        self.set_pending_initial_spawn_placement(
            monitor,
            anchor.node,
            anchor.pos,
            anchor.ext,
            candidate.pos,
            candidate.dir,
            false,
            overlap_policy,
        );
        let growth_dir = candidate
            .dir
            .unwrap_or_else(|| self.pick_cluster_growth_dir(monitor, anchor.pos));
        self.update_spawn_patch(monitor, anchor.pos, anchor.node, anchor.pos, growth_dir);
        self.spawn_monitor_state_mut(monitor).spawn_view_anchor = anchor.pos;
    }

    fn default_pick_spawn_position(&mut self, size: Vec2) -> (String, Vec2, bool) {
        self.pick_spawn_position_impl(size)
    }

    #[allow(dead_code)]
    pub(crate) fn pick_spawn_position(&mut self, size: Vec2) -> (String, Vec2, bool) {
        self.default_pick_spawn_position(size)
    }

    pub(crate) fn pick_spawn_position_with_intent(
        &mut self,
        size: Vec2,
        intent: &InitialWindowIntent,
    ) -> (String, Vec2, bool) {
        if Self::has_default_window_rule(intent) {
            return self.default_pick_spawn_position(size);
        }

        let target_monitor = self.spawn_target_monitor_for_intent(intent);
        let overlap_policy = intent.effective_overlap_policy();
        let placement = intent.effective_spawn_placement();
        self.spawn_monitor_state_mut(target_monitor.as_str())
            .spawn_cursor += 1;
        let viewport_center =
            read::spawn_read_context(self).viewport_center_for_monitor(target_monitor.as_str());
        let (focus_id, focus_pos) =
            read::spawn_read_context(self).current_spawn_focus(target_monitor.as_str());
        let fullscreen_anchor = if overlap_policy != InitialWindowOverlapPolicy::None {
            self.fullscreen_anchor_for_monitor(target_monitor.as_str())
        } else {
            None
        };
        let cursor_anchor = self
            .input
            .interaction_state
            .last_pointer_screen_global
            .and_then(|(sx, sy)| self.world_from_monitor_screen(target_monitor.as_str(), sx, sy));

        match placement {
            InitialWindowSpawnPlacement::Adjacent => {
                if let Some(parent_id) = intent.parent_node
                    && let Some(anchor) = self.spawn_anchor_for_node(parent_id)
                {
                    if overlap_policy != InitialWindowOverlapPolicy::None
                        && fullscreen_anchor
                            .is_some_and(|(fullscreen_id, _)| fullscreen_id == parent_id)
                    {
                        let candidate = SpawnCandidate {
                            pos: anchor.pos,
                            dir: None,
                        };
                        self.commit_spawn_plan(
                            target_monitor.as_str(),
                            anchor,
                            candidate,
                            overlap_policy,
                        );
                        return (target_monitor, candidate.pos, false);
                    }

                    for dir in spawn_cardinal_dirs() {
                        if let Some(pos) = self
                            .spawn_candidate_for_focus_dir(parent_id, size, dir)
                            .map(|pos| {
                                self.adjust_vertical_candidate_to_row(
                                    target_monitor.as_str(),
                                    anchor.pos,
                                    pos,
                                    size,
                                )
                            })
                            && self.spawn_candidate_fits_with_policy(
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
                            self.commit_spawn_plan(
                                target_monitor.as_str(),
                                anchor,
                                candidate,
                                overlap_policy,
                            );
                            return (target_monitor, candidate.pos, false);
                        }
                    }

                    if let Some(pos) = self.try_spawn_star_with_policy(
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
                        self.commit_spawn_plan(
                            target_monitor.as_str(),
                            anchor,
                            candidate,
                            overlap_policy,
                        );
                        return (target_monitor, candidate.pos, false);
                    }

                    return self.default_pick_spawn_position(size);
                }
                if overlap_policy == InitialWindowOverlapPolicy::All {
                    if let Some((fullscreen_id, pos)) = fullscreen_anchor {
                        let anchor = self
                            .spawn_anchor_for_node(fullscreen_id)
                            .unwrap_or_else(|| self.spawn_anchor_at(pos));
                        let candidate = SpawnCandidate {
                            pos: anchor.pos,
                            dir: None,
                        };
                        self.commit_spawn_plan(
                            target_monitor.as_str(),
                            anchor,
                            candidate,
                            overlap_policy,
                        );
                        return (target_monitor, candidate.pos, false);
                    }
                    if let Some(id) = focus_id
                        && let Some(anchor) = self.spawn_anchor_for_node(id)
                    {
                        for dir in spawn_cardinal_dirs() {
                            if let Some(pos) = self.spawn_candidate_for_focus_dir(id, size, dir) {
                                let candidate = SpawnCandidate {
                                    pos,
                                    dir: Some(dir),
                                };
                                self.commit_spawn_plan(
                                    target_monitor.as_str(),
                                    anchor,
                                    candidate,
                                    overlap_policy,
                                );
                                return (target_monitor, candidate.pos, false);
                            }
                        }
                    }
                    let anchor = self.spawn_anchor_at(focus_pos);
                    let candidate = SpawnCandidate {
                        pos: focus_pos,
                        dir: None,
                    };
                    self.commit_spawn_plan(
                        target_monitor.as_str(),
                        anchor,
                        candidate,
                        overlap_policy,
                    );
                    return (target_monitor, candidate.pos, false);
                }
                return self.default_pick_spawn_position(size);
            }
            InitialWindowSpawnPlacement::Center
            | InitialWindowSpawnPlacement::ViewportCenter
            | InitialWindowSpawnPlacement::Cursor
            | InitialWindowSpawnPlacement::App => {}
        }

        let anchor = match placement {
            InitialWindowSpawnPlacement::Center | InitialWindowSpawnPlacement::App => intent
                .parent_node
                .and_then(|id| self.spawn_anchor_for_node(id))
                .or_else(|| {
                    fullscreen_anchor.and_then(|(id, pos)| {
                        self.spawn_anchor_for_node(id)
                            .or_else(|| Some(self.spawn_anchor_at(pos)))
                    })
                })
                .unwrap_or_else(|| self.spawn_anchor_at(viewport_center)),
            InitialWindowSpawnPlacement::ViewportCenter => self.spawn_anchor_at(viewport_center),
            InitialWindowSpawnPlacement::Cursor => {
                self.spawn_anchor_at(cursor_anchor.unwrap_or(viewport_center))
            }
            InitialWindowSpawnPlacement::Adjacent => unreachable!("adjacent handled above"),
        };

        let candidate = if overlap_policy == InitialWindowOverlapPolicy::All {
            SpawnCandidate {
                pos: anchor.pos,
                dir: None,
            }
        } else {
            self.try_spawn_star_with_policy(
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
            })
        };
        self.commit_spawn_plan(target_monitor.as_str(), anchor, candidate, overlap_policy);
        (target_monitor, candidate.pos, false)
    }

    fn pick_spawn_position_impl(&mut self, size: Vec2) -> (String, Vec2, bool) {
        let target_monitor = self
            .model
            .spawn_state
            .pending_spawn_monitor
            .take()
            .filter(|monitor| self.model.monitor_state.monitors.contains_key(monitor))
            .unwrap_or_else(|| read::spawn_read_context(self).resolve_spawn_target_monitor());
        let focus_override = self
            .spawn_monitor_state_mut(target_monitor.as_str())
            .spawn_focus_override
            .take();
        self.spawn_monitor_state_mut(target_monitor.as_str())
            .spawn_cursor += 1;
        let viewport_center =
            read::spawn_read_context(self).viewport_center_for_monitor(target_monitor.as_str());
        let (focus_id, focus_pos) =
            read::spawn_read_context(self).current_spawn_focus(target_monitor.as_str());
        if !self.monitor_has_visible_spawn_surface(target_monitor.as_str()) {
            self.spawn_monitor_state_mut(target_monitor.as_str())
                .spawn_patch = None;
        }
        let monitor_spawn = self.spawn_monitor_state(target_monitor.as_str());
        debug!(
            "spawn target resolved: target_monitor={} focused_monitor={} interaction_monitor={} anchor_mode={:?} focus_id={:?}",
            target_monitor,
            self.focused_monitor(),
            self.interaction_monitor(),
            monitor_spawn.spawn_anchor_mode,
            focus_id.map(|id| id.as_u64())
        );
        if let Some(override_focus) = focus_override {
            let gap = self.non_overlap_gap_world() + SPAWN_CONTACT_MARGIN;
            let frame_pad = active_window_frame_pad_px(&self.runtime.tuning) as f32;
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
                let pos = self.adjust_vertical_candidate_to_row(
                    target_monitor.as_str(),
                    override_focus.pos,
                    pos,
                    size,
                );
                if self.spawn_candidate_fits(target_monitor.as_str(), pos, size, None) {
                    self.set_pending_initial_spawn_placement(
                        target_monitor.as_str(),
                        None,
                        override_focus.pos,
                        Some(override_ext),
                        pos,
                        Some(dir),
                        false,
                        InitialWindowOverlapPolicy::None,
                    );
                    self.update_spawn_patch(
                        target_monitor.as_str(),
                        override_focus.pos,
                        None,
                        override_focus.pos,
                        dir,
                    );
                    self.spawn_monitor_state_mut(target_monitor.as_str())
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
                self.try_strict_spawn_star(target_monitor.as_str(), override_focus.pos, size)
            {
                self.set_pending_initial_spawn_placement(
                    target_monitor.as_str(),
                    None,
                    override_focus.pos,
                    Some(override_ext),
                    pos,
                    spawn_dir_from_delta(Vec2 {
                        x: pos.x - override_focus.pos.x,
                        y: pos.y - override_focus.pos.y,
                    }),
                    true,
                    InitialWindowOverlapPolicy::None,
                );
                let growth_dir =
                    self.pick_cluster_growth_dir(target_monitor.as_str(), override_focus.pos);
                self.update_spawn_patch(
                    target_monitor.as_str(),
                    override_focus.pos,
                    None,
                    override_focus.pos,
                    growth_dir,
                );
                self.spawn_monitor_state_mut(target_monitor.as_str())
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
            self.pos_is_in_spawn_active_area(target_monitor.as_str(), patch.anchor)
        });
        let focus_anchor_id = focus_id.filter(|id| {
            self.node_is_in_spawn_active_area(target_monitor.as_str(), *id)
                || (patch_anchor_active
                    && !self.surface_is_fully_visible_on_monitor(target_monitor.as_str(), *id))
        });
        let focus_moved_from_patch = patch.is_some_and(|patch| {
            focus_anchor_id.is_some()
                && ((focus_pos.x - patch.anchor.x).abs() > 0.5
                    || (focus_pos.y - patch.anchor.y).abs() > 0.5)
        });
        let use_patch_anchor = patch.is_some()
            && self.monitor_has_visible_spawn_surface(target_monitor.as_str())
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
        let anchor_node = if use_patch_anchor {
            patch.and_then(|patch| patch.focus_node).or(focus_anchor_id)
        } else {
            focus_anchor_id
        };
        let anchor_ext = anchor_node.and_then(|id| {
            self.model.field.node(id).and_then(|node| {
                ((node.pos.x - anchor.x).abs() <= 0.5 && (node.pos.y - anchor.y).abs() <= 0.5)
                    .then(|| self.spawn_safe_obstacle_extents_for_node(node))
            })
        });
        let pos = self
            .try_strict_spawn_star(target_monitor.as_str(), anchor, size)
            .unwrap_or(anchor);
        self.set_pending_initial_spawn_placement(
            target_monitor.as_str(),
            anchor_node,
            anchor,
            anchor_ext,
            pos,
            spawn_dir_from_delta(Vec2 {
                x: pos.x - anchor.x,
                y: pos.y - anchor.y,
            }),
            true,
            InitialWindowOverlapPolicy::None,
        );
        let growth_dir = self.pick_cluster_growth_dir(target_monitor.as_str(), anchor);
        self.update_spawn_patch(
            target_monitor.as_str(),
            anchor,
            anchor_node,
            anchor,
            growth_dir,
        );
        self.spawn_monitor_state_mut(target_monitor.as_str())
            .spawn_view_anchor = anchor;
        debug!(
            "spawn position picked: target_monitor={} anchor=({:.1},{:.1}) focus_pos=({:.1},{:.1}) chosen=({:.1},{:.1}) size=({:.1},{:.1})",
            target_monitor,
            anchor.x,
            anchor.y,
            focus_pos.x,
            focus_pos.y,
            pos.x,
            pos.y,
            size.x,
            size.y
        );
        (target_monitor, pos, false)
    }
}
