use std::ops::{Deref, DerefMut};

use eventline::debug;
use halley_config::{InitialWindowOverlapPolicy, InitialWindowSpawnPlacement};
use halley_core::field::{NodeId, Vec2};
use halley_core::viewport::Viewport;

use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::overlap::system::CollisionExtents;
use crate::compositor::root::Halley;
use crate::compositor::spawn::read;
use crate::compositor::spawn::rules::{InitialWindowIntent, ResolvedInitialWindowRule};
use crate::window::active_window_frame_pad_px;

use super::SpawnRevealController;

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
        let focus_ext = self.spawn_obstacle_extents_for_node(node);
        let candidate_ext = CollisionExtents::symmetric(size);
        let gap = self.non_overlap_gap_world();
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
        Some(pos)
    }

    pub(crate) fn spawn_star_step_x(&self, size: Vec2) -> f32 {
        size.x
            + (active_window_frame_pad_px(&self.runtime.tuning) as f32 * 2.0)
            + self.non_overlap_gap_world()
    }

    pub(crate) fn spawn_star_step_y(&self, size: Vec2) -> f32 {
        size.y
            + (active_window_frame_pad_px(&self.runtime.tuning) as f32 * 2.0)
            + self.non_overlap_gap_world()
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
        let candidate = CollisionExtents::symmetric(size);
        !self.model.field.nodes().values().any(|other| {
            if Some(other.id) == skip_node
                || other.kind != halley_core::field::NodeKind::Surface
                || !self.model.field.is_visible(other.id)
            {
                return false;
            }
            if self
                .model
                .monitor_state
                .node_monitor
                .get(&other.id)
                .is_some_and(|other_monitor| other_monitor != monitor)
            {
                return false;
            }
            if overlap_policy == InitialWindowOverlapPolicy::ParentOnly
                && parent_node == Some(other.id)
            {
                return false;
            }
            let (other_pos, other_ext) = if let Some(session) =
                crate::compositor::workspace::state::maximize_session_for_monitor(self, monitor)
                    .filter(|session| {
                        session.state
                            == crate::compositor::workspace::state::MaximizeSessionState::SpawnRestoring
                    })
                && let Some(snapshot) = session.node_snapshots.get(&other.id)
            {
                let half_w = snapshot.size.x.max(1.0) * 0.5
                    + active_window_frame_pad_px(&self.runtime.tuning) as f32;
                let half_h = snapshot.size.y.max(1.0) * 0.5
                    + active_window_frame_pad_px(&self.runtime.tuning) as f32;
                (
                    snapshot.pos,
                    CollisionExtents {
                        left: half_w,
                        right: half_w,
                        top: half_h,
                        bottom: half_h,
                    },
                )
            } else {
                (other.pos, self.spawn_obstacle_extents_for_node(other))
            };
            let req_x = self.required_sep_x(pos.x, candidate, other_pos.x, other_ext, pair_gap);
            let req_y = self.required_sep_y(pos.y, candidate, other_pos.y, other_ext, pair_gap);
            (pos.x - other_pos.x).abs() < req_x && (pos.y - other_pos.y).abs() < req_y
        })
    }

    fn try_spawn_star(&self, monitor: &str, center: Vec2, size: Vec2) -> Option<Vec2> {
        self.try_spawn_star_with_policy(
            monitor,
            center,
            size,
            InitialWindowOverlapPolicy::None,
            None,
        )
    }

    fn try_spawn_star_with_policy(
        &self,
        monitor: &str,
        center: Vec2,
        size: Vec2,
        overlap_policy: InitialWindowOverlapPolicy,
        parent_node: Option<NodeId>,
    ) -> Option<Vec2> {
        for offset in self.star_candidate_offsets(size) {
            let pos = Vec2 {
                x: center.x + offset.x,
                y: center.y + offset.y,
            };
            if self.spawn_candidate_fits_with_policy(
                monitor,
                pos,
                size,
                None,
                overlap_policy,
                parent_node,
            ) {
                return Some(pos);
            }
        }
        None
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
        let parent_anchor = intent
            .parent_node
            .and_then(|id| self.model.field.node(id).map(|node| node.pos));
        let cursor_anchor = self
            .input
            .interaction_state
            .last_pointer_screen_global
            .and_then(|(sx, sy)| self.world_from_monitor_screen(target_monitor.as_str(), sx, sy));

        let chosen = match placement {
            InitialWindowSpawnPlacement::Adjacent => {
                if let Some(parent_id) = intent.parent_node {
                    if overlap_policy != InitialWindowOverlapPolicy::None
                        && fullscreen_anchor
                            .is_some_and(|(fullscreen_id, _)| fullscreen_id == parent_id)
                    {
                        return (
                            target_monitor,
                            parent_anchor.unwrap_or(viewport_center),
                            false,
                        );
                    }
                    for dir in spawn_cardinal_dirs() {
                        if let Some(pos) = self.spawn_candidate_for_focus_dir(parent_id, size, dir)
                            && self.spawn_candidate_fits_with_policy(
                                target_monitor.as_str(),
                                pos,
                                size,
                                None,
                                overlap_policy,
                                intent.parent_node,
                            )
                        {
                            return (target_monitor, pos, false);
                        }
                    }
                    return self.default_pick_spawn_position(size);
                }
                if overlap_policy == InitialWindowOverlapPolicy::All {
                    if let Some((_, pos)) = fullscreen_anchor {
                        return (target_monitor, pos, false);
                    }
                    if let Some(id) = focus_id {
                        for dir in spawn_cardinal_dirs() {
                            if let Some(pos) = self.spawn_candidate_for_focus_dir(id, size, dir) {
                                return (target_monitor, pos, false);
                            }
                        }
                    }
                    return (target_monitor, focus_pos, false);
                }
                return self.default_pick_spawn_position(size);
            }
            InitialWindowSpawnPlacement::Center => parent_anchor
                .or_else(|| fullscreen_anchor.map(|(_, pos)| pos))
                .unwrap_or(viewport_center),
            InitialWindowSpawnPlacement::ViewportCenter => viewport_center,
            InitialWindowSpawnPlacement::Cursor => cursor_anchor.unwrap_or(viewport_center),
            InitialWindowSpawnPlacement::App => parent_anchor
                .or_else(|| fullscreen_anchor.map(|(_, pos)| pos))
                .unwrap_or(viewport_center),
        };

        let pos = if overlap_policy == InitialWindowOverlapPolicy::All {
            chosen
        } else {
            self.try_spawn_star_with_policy(
                target_monitor.as_str(),
                chosen,
                size,
                overlap_policy,
                intent.parent_node,
            )
            .unwrap_or(chosen)
        };
        let growth_dir = self.pick_cluster_growth_dir(target_monitor.as_str(), chosen);
        self.update_spawn_patch(
            target_monitor.as_str(),
            chosen,
            intent.parent_node,
            chosen,
            growth_dir,
        );
        self.spawn_monitor_state_mut(target_monitor.as_str())
            .spawn_view_anchor = chosen;
        (target_monitor, pos, false)
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
        let monitor_spawn = self.spawn_monitor_state(target_monitor.as_str());
        let viewport_center =
            read::spawn_read_context(self).viewport_center_for_monitor(target_monitor.as_str());
        let (focus_id, focus_pos) =
            read::spawn_read_context(self).current_spawn_focus(target_monitor.as_str());
        debug!(
            "spawn target resolved: target_monitor={} focused_monitor={} interaction_monitor={} anchor_mode={:?} focus_id={:?}",
            target_monitor,
            self.focused_monitor(),
            self.interaction_monitor(),
            monitor_spawn.spawn_anchor_mode,
            focus_id.map(|id| id.as_u64())
        );
        if let Some(anchor) = focus_override
            && let Some(pos) = self.try_spawn_star(target_monitor.as_str(), anchor, size)
        {
            let growth_dir = self.pick_cluster_growth_dir(target_monitor.as_str(), anchor);
            self.update_spawn_patch(target_monitor.as_str(), anchor, None, anchor, growth_dir);
            self.spawn_monitor_state_mut(target_monitor.as_str())
                .spawn_view_anchor = anchor;
            debug!(
                "spawn position picked from override: target_monitor={} anchor=({:.1},{:.1}) chosen=({:.1},{:.1}) size=({:.1},{:.1})",
                target_monitor, anchor.x, anchor.y, pos.x, pos.y, size.x, size.y
            );
            return (target_monitor, pos, false);
        }
        let focus_visible = focus_id.is_some_and(|id| {
            self.surface_is_fully_visible_on_monitor(target_monitor.as_str(), id)
        });

        if let Some(id) = focus_id {
            for dir in spawn_cardinal_dirs() {
                if let Some(pos) = self.spawn_candidate_for_focus_dir(id, size, dir)
                    && self.spawn_candidate_fits(target_monitor.as_str(), pos, size, None)
                {
                    self.update_spawn_patch(
                        target_monitor.as_str(),
                        focus_pos,
                        Some(id),
                        focus_pos,
                        dir,
                    );
                    debug!(
                        "spawn position picked: target_monitor={} anchor=({:.1},{:.1}) focus_pos=({:.1},{:.1}) chosen=({:.1},{:.1}) size=({:.1},{:.1})",
                        target_monitor,
                        focus_pos.x,
                        focus_pos.y,
                        focus_pos.x,
                        focus_pos.y,
                        pos.x,
                        pos.y,
                        size.x,
                        size.y
                    );
                    return (target_monitor, pos, false);
                }
            }
        }

        let anchor = if focus_visible {
            focus_pos
        } else {
            viewport_center
        };
        if let Some(pos) = self.try_spawn_star(target_monitor.as_str(), anchor, size) {
            let growth_dir = self.pick_cluster_growth_dir(target_monitor.as_str(), anchor);
            self.update_spawn_patch(
                target_monitor.as_str(),
                anchor,
                None,
                viewport_center,
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
            return (target_monitor, pos, false);
        }

        let fallback_anchor = viewport_center;
        let growth_dir = self.pick_cluster_growth_dir(target_monitor.as_str(), fallback_anchor);
        self.update_spawn_patch(
            target_monitor.as_str(),
            fallback_anchor,
            None,
            viewport_center,
            growth_dir,
        );
        self.spawn_monitor_state_mut(target_monitor.as_str())
            .spawn_view_anchor = fallback_anchor;
        debug!(
            "spawn fallback used: target_monitor={} anchor=({:.1},{:.1}) focus_pos=({:.1},{:.1}) chosen=({:.1},{:.1}) size=({:.1},{:.1})",
            target_monitor,
            fallback_anchor.x,
            fallback_anchor.y,
            focus_pos.x,
            focus_pos.y,
            fallback_anchor.x,
            fallback_anchor.y,
            size.x,
            size.y
        );
        (target_monitor, fallback_anchor, false)
    }
}
