use std::time::Instant;

use eventline::info;
use halley_config::PanToNewMode;
use halley_core::decay::DecayLevel;
use halley_core::field::{NodeId, Vec2};

use crate::render::ACTIVE_WINDOW_FRAME_PAD_PX;
use crate::state::{FocusState, Halley, MonitorSpawnState, MonitorState, SpawnState};
use crate::wm::overlap::CollisionExtents;

/// Spawn candidates are tried in a deterministic star pattern:
/// center, then right, left, up, down for each ring.
fn spawn_cardinal_dirs() -> [Vec2; 4] {
    [
        Vec2 { x: 1.0, y: 0.0 },  // right
        Vec2 { x: -1.0, y: 0.0 }, // left
        Vec2 { x: 0.0, y: 1.0 },  // up
        Vec2 { x: 0.0, y: -1.0 }, // down
    ]
}

struct SpawnReadContext<'a> {
    field: &'a halley_core::field::Field,
    focus_state: &'a FocusState,
    monitor_state: &'a MonitorState,
    spawn_state: &'a SpawnState,
    viewport: halley_core::viewport::Viewport,
    focused_monitor: &'a str,
    interaction_monitor: &'a str,
    pan_to_new: PanToNewMode,
}

enum RevealNewToplevelPlan {
    AlreadyQueued,
    ActivateNow,
    QueuePan { target_center: Vec2 },
}

impl<'a> SpawnReadContext<'a> {
    fn viewport_center_for_monitor(&self, monitor: &str) -> Vec2 {
        if self.monitor_state.current_monitor == monitor {
            return self.viewport.center;
        }
        self.monitor_state
            .monitors
            .get(monitor)
            .map(|space| space.viewport.center)
            .unwrap_or(self.viewport.center)
    }

    fn resolve_spawn_target_monitor(&self) -> String {
        let focused = self.focused_monitor.to_string();
        if self.monitor_state.monitors.contains_key(focused.as_str()) {
            return focused;
        }
        self.interaction_monitor.to_string()
    }

    fn last_input_surface_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        let primary = self.focus_state.primary_interaction_focus.and_then(|id| {
            self.field.node(id).and_then(|n| {
                (self.field.is_visible(id)
                    && n.kind == halley_core::field::NodeKind::Surface
                    && self
                        .monitor_state
                        .node_monitor
                        .get(&id)
                        .is_some_and(|m| m == monitor))
                .then_some((id, u64::MAX))
            })
        });
        let monitor_focus = self
            .focus_state
            .monitor_focus
            .get(monitor)
            .copied()
            .and_then(|id| {
                self.field.node(id).and_then(|n| {
                    (self.field.is_visible(id)
                        && n.kind == halley_core::field::NodeKind::Surface
                        && self
                            .monitor_state
                            .node_monitor
                            .get(&id)
                            .is_some_and(|m| m == monitor))
                    .then_some((
                        id,
                        self.focus_state
                            .last_surface_focus_ms
                            .get(&id)
                            .copied()
                            .unwrap_or(0),
                    ))
                })
            });
        primary
            .into_iter()
            .chain(monitor_focus)
            .chain(
                self.focus_state
                    .last_surface_focus_ms
                    .iter()
                    .filter_map(|(&id, &at)| {
                        self.field.node(id).and_then(|n| {
                            (self.field.is_visible(id)
                                && n.kind == halley_core::field::NodeKind::Surface
                                && self
                                    .monitor_state
                                    .node_monitor
                                    .get(&id)
                                    .is_some_and(|m| m == monitor))
                            .then_some((id, at))
                        })
                    }),
            )
            .max_by_key(|entry: &(NodeId, u64)| (entry.1, entry.0.as_u64()))
            .map(|(id, _)| id)
    }

    fn current_spawn_focus(&self, monitor: &str) -> (Option<NodeId>, Vec2) {
        let spawn = self.spawn_monitor_state(monitor);
        let viewport_center = self.viewport_center_for_monitor(monitor);
        if spawn.spawn_anchor_mode == crate::state::SpawnAnchorMode::View {
            return (None, spawn.spawn_view_anchor);
        }
        if let Some(id) = self.last_input_surface_node_for_monitor(monitor)
            && let Some(node) = self.field.node(id)
        {
            return (Some(id), node.pos);
        }
        (None, viewport_center)
    }

    fn spawn_monitor_state(&self, monitor: &str) -> MonitorSpawnState {
        self.spawn_state
            .per_monitor
            .get(monitor)
            .cloned()
            .unwrap_or_else(|| MonitorSpawnState::new(self.viewport_center_for_monitor(monitor)))
    }

    fn viewport_fully_contains_surface_on_monitor(
        &self,
        st: &Halley,
        monitor: &str,
        id: NodeId,
    ) -> bool {
        let Some(node) = self.field.node(id) else {
            return false;
        };
        let ext = st.spawn_obstacle_extents_for_node(node);
        let viewport = if self.monitor_state.current_monitor == monitor {
            self.viewport
        } else if let Some(space) = self.monitor_state.monitors.get(monitor) {
            space.viewport
        } else {
            self.viewport
        };
        let min_x = viewport.center.x - viewport.size.x * 0.5;
        let max_x = viewport.center.x + viewport.size.x * 0.5;
        let min_y = viewport.center.y - viewport.size.y * 0.5;
        let max_y = viewport.center.y + viewport.size.y * 0.5;

        node.pos.x - ext.left >= min_x
            && node.pos.x + ext.right <= max_x
            && node.pos.y - ext.top >= min_y
            && node.pos.y + ext.bottom <= max_y
    }

    fn reveal_new_toplevel_plan(
        &self,
        st: &Halley,
        id: NodeId,
        is_transient: bool,
    ) -> RevealNewToplevelPlan {
        if is_transient {
            return RevealNewToplevelPlan::ActivateNow;
        }
        if self
            .spawn_state
            .active_spawn_pan
            .is_some_and(|active| active.node_id == id)
            || self
                .spawn_state
                .pending_spawn_pan_queue
                .iter()
                .any(|pending| pending.node_id == id)
        {
            return RevealNewToplevelPlan::AlreadyQueued;
        }

        let monitor = self
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| self.focused_monitor.to_string());
        if st
            .active_cluster_workspace_for_monitor(monitor.as_str())
            .is_some()
        {
            return RevealNewToplevelPlan::ActivateNow;
        }
        let target_center = match self.pan_to_new {
            PanToNewMode::Never => return RevealNewToplevelPlan::ActivateNow,
            PanToNewMode::Always => match self.field.node(id) {
                Some(node) => node.pos,
                None => return RevealNewToplevelPlan::ActivateNow,
            },
            PanToNewMode::IfNeeded => {
                if st.surface_is_sufficiently_visible_on_monitor(monitor.as_str(), id) {
                    return RevealNewToplevelPlan::ActivateNow;
                }
                match st.minimal_reveal_center_for_surface_on_monitor(monitor.as_str(), id) {
                    Some(center) => center,
                    None => return RevealNewToplevelPlan::ActivateNow,
                }
            }
        };
        RevealNewToplevelPlan::QueuePan { target_center }
    }
}

impl Halley {
    const SPAWN_STAR_RINGS: usize = 24;

    fn spawn_read_context(&self) -> SpawnReadContext<'_> {
        SpawnReadContext {
            field: &self.model.field,
            focus_state: &self.model.focus_state,
            monitor_state: &self.model.monitor_state,
            spawn_state: &self.model.spawn_state,
            viewport: self.model.viewport,
            focused_monitor: self.focused_monitor(),
            interaction_monitor: self.interaction_monitor(),
            pan_to_new: self.runtime.tuning.pan_to_new,
        }
    }

    fn viewport_center_for_monitor(&self, monitor: &str) -> Vec2 {
        self.spawn_read_context()
            .viewport_center_for_monitor(monitor)
    }

    fn resolve_spawn_target_monitor(&self) -> String {
        self.spawn_read_context().resolve_spawn_target_monitor()
    }

    fn current_spawn_focus(&self, monitor: &str) -> (Option<NodeId>, Vec2) {
        self.spawn_read_context().current_spawn_focus(monitor)
    }

    fn viewport_fully_contains_surface_on_monitor(&self, monitor: &str, id: NodeId) -> bool {
        self.spawn_read_context()
            .viewport_fully_contains_surface_on_monitor(self, monitor, id)
    }

    #[cfg(test)]
    fn right_spawn_candidate_for_focus(&self, id: NodeId, size: Vec2) -> Option<Vec2> {
        self.spawn_candidate_for_focus_dir(id, size, Vec2 { x: 1.0, y: 0.0 })
    }

    fn spawn_candidate_for_focus_dir(&self, id: NodeId, size: Vec2, dir: Vec2) -> Option<Vec2> {
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

    fn spawn_star_step_x(&self, size: Vec2) -> f32 {
        size.x + (ACTIVE_WINDOW_FRAME_PAD_PX.max(0) as f32 * 2.0) + self.non_overlap_gap_world()
    }

    fn spawn_star_step_y(&self, size: Vec2) -> f32 {
        size.y + (ACTIVE_WINDOW_FRAME_PAD_PX.max(0) as f32 * 2.0) + self.non_overlap_gap_world()
    }

    #[cfg(test)]
    fn spawn_star_step(&self, size: Vec2) -> f32 {
        self.spawn_star_step_x(size)
            .max(self.spawn_star_step_y(size))
    }

    fn star_candidate_offsets(&self, size: Vec2) -> Vec<Vec2> {
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

    fn spawn_candidate_fits(
        &self,
        monitor: &str,
        pos: Vec2,
        size: Vec2,
        skip_node: Option<NodeId>,
    ) -> bool {
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
            let other_ext = self.spawn_obstacle_extents_for_node(other);
            let req_x = self.required_sep_x(pos.x, candidate, other.pos.x, other_ext, pair_gap);
            let req_y = self.required_sep_y(pos.y, candidate, other.pos.y, other_ext, pair_gap);
            (pos.x - other.pos.x).abs() < req_x && (pos.y - other.pos.y).abs() < req_y
        })
    }

    fn try_spawn_star(&self, monitor: &str, center: Vec2, size: Vec2) -> Option<Vec2> {
        for offset in self.star_candidate_offsets(size) {
            let pos = Vec2 {
                x: center.x + offset.x,
                y: center.y + offset.y,
            };
            if self.spawn_candidate_fits(monitor, pos, size, None) {
                return Some(pos);
            }
        }
        None
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

    fn update_spawn_patch(
        &mut self,
        monitor: &str,
        anchor: Vec2,
        focus_node: Option<NodeId>,
        focus_pos: Vec2,
        growth_dir: Vec2,
    ) {
        self.spawn_monitor_state_mut(monitor).spawn_patch = Some(crate::state::SpawnPatch {
            anchor,
            focus_node,
            focus_pos,
            growth_dir,
            placements_in_patch: 0,
            frontier: Vec::new(),
        });
    }

    /// Returns `(monitor, position, needs_pan)`.
    pub(super) fn pick_spawn_position(&mut self, size: Vec2) -> (String, Vec2, bool) {
        let target_monitor = self
            .model
            .spawn_state
            .pending_spawn_monitor
            .take()
            .filter(|monitor| self.model.monitor_state.monitors.contains_key(monitor))
            .unwrap_or_else(|| self.resolve_spawn_target_monitor());
        self.spawn_monitor_state_mut(target_monitor.as_str())
            .spawn_cursor += 1;
        let monitor_spawn = self.spawn_monitor_state(target_monitor.as_str());
        let viewport_center = self.viewport_center_for_monitor(target_monitor.as_str());
        let (focus_id, focus_pos) = self.current_spawn_focus(target_monitor.as_str());
        info!(
            "spawn target resolved: target_monitor={} focused_monitor={} interaction_monitor={} anchor_mode={:?} focus_id={:?}",
            target_monitor,
            self.focused_monitor(),
            self.interaction_monitor(),
            monitor_spawn.spawn_anchor_mode,
            focus_id.map(|id| id.as_u64())
        );
        let focus_visible = focus_id.is_some_and(|id| {
            self.viewport_fully_contains_surface_on_monitor(target_monitor.as_str(), id)
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
                    info!(
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
            info!(
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
        info!(
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

    pub(crate) fn queue_spawn_pan_to_node(&mut self, id: NodeId, now: Instant) {
        let monitor = self
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| self.focused_monitor().to_string());
        let Some(target_center) = (match self.runtime.tuning.pan_to_new {
            PanToNewMode::Always => self.model.field.node(id).map(|node| node.pos),
            PanToNewMode::IfNeeded => {
                self.minimal_reveal_center_for_surface_on_monitor(monitor.as_str(), id)
            }
            PanToNewMode::Never => None,
        }) else {
            return;
        };
        let _ = self.model.field.set_detached(id, true);
        self.model
            .spawn_state
            .pending_spawn_activate_at_ms
            .remove(&id);
        self.model
            .spawn_state
            .pending_spawn_pan_queue
            .push_back(crate::state::PendingSpawnPan {
                node_id: id,
                target_center,
            });
        self.maybe_start_pending_spawn_pan(now);
    }

    pub(crate) fn maybe_start_pending_spawn_pan(&mut self, now: Instant) {
        if self.model.spawn_state.active_spawn_pan.is_some() {
            return;
        }

        let now_ms = self.now_ms(now);
        while let Some(next) = self.model.spawn_state.pending_spawn_pan_queue.pop_front() {
            if self.model.field.node(next.node_id).is_none() {
                continue;
            }

            let did_pan = self.animate_viewport_center_to_delayed(
                next.target_center,
                now,
                Self::VIEWPORT_PAN_PRELOAD_MS,
            );
            self.model.spawn_state.active_spawn_pan = Some(crate::state::ActiveSpawnPan {
                node_id: next.node_id,
                pan_start_at_ms: now_ms.saturating_add(if did_pan {
                    Self::VIEWPORT_PAN_PRELOAD_MS
                } else {
                    0
                }),
                reveal_at_ms: now_ms.saturating_add(if did_pan {
                    Self::VIEWPORT_PAN_PRELOAD_MS + Self::VIEWPORT_PAN_DURATION_MS
                } else {
                    0
                }),
            });
            break;
        }
    }

    pub(crate) fn tick_pending_spawn_pan(&mut self, now: Instant, now_ms: u64) {
        let Some(active) = self.model.spawn_state.active_spawn_pan else {
            self.maybe_start_pending_spawn_pan(now);
            return;
        };

        if self.model.field.node(active.node_id).is_none() {
            self.model.spawn_state.active_spawn_pan = None;
            self.maybe_start_pending_spawn_pan(now);
            return;
        }

        let pan_finished = now_ms >= active.reveal_at_ms
            || (now_ms >= active.pan_start_at_ms
                && self.input.interaction_state.viewport_pan_anim.is_none());
        if !pan_finished {
            return;
        }

        let _ = self.model.field.set_detached(active.node_id, false);
        let _ = self
            .model
            .field
            .set_decay_level(active.node_id, DecayLevel::Hot);
        if let Some(node) = self.model.field.node(active.node_id) {
            self.model
                .workspace_state
                .last_active_size
                .insert(active.node_id, node.intrinsic_size);
        }
        self.mark_active_transition(active.node_id, now, 620);
        self.record_focus_trail_visit(active.node_id);
        self.model.focus_state.suppress_trail_record_once = true;
        self.set_interaction_focus(Some(active.node_id), 30_000, now);
        self.model.spawn_state.active_spawn_pan = None;
        self.maybe_start_pending_spawn_pan(now);
    }

    pub(crate) fn reveal_new_toplevel_node(
        &mut self,
        id: NodeId,
        is_transient: bool,
        now: Instant,
    ) {
        match self
            .spawn_read_context()
            .reveal_new_toplevel_plan(self, id, is_transient)
        {
            RevealNewToplevelPlan::AlreadyQueued => {}
            RevealNewToplevelPlan::ActivateNow => {
                self.record_focus_trail_visit(id);
                self.model.focus_state.suppress_trail_record_once = true;
                self.set_interaction_focus(Some(id), 30_000, now);
                self.model
                    .spawn_state
                    .pending_spawn_activate_at_ms
                    .remove(&id);
                self.mark_active_transition(id, now, 620);
            }
            RevealNewToplevelPlan::QueuePan { target_center } => {
                let _ = self.model.field.set_detached(id, true);
                self.model
                    .spawn_state
                    .pending_spawn_activate_at_ms
                    .remove(&id);
                self.model.spawn_state.pending_spawn_pan_queue.push_back(
                    crate::state::PendingSpawnPan {
                        node_id: id,
                        target_center,
                    },
                );
                self.maybe_start_pending_spawn_pan(now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_offsets_are_center_then_right_left_up_down() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let state = Halley::new_for_test(&dh, tuning);

        let offsets = state.star_candidate_offsets(Vec2 { x: 100.0, y: 80.0 });
        assert_eq!(offsets[0], Vec2 { x: 0.0, y: 0.0 });

        let step_x = state.spawn_star_step_x(Vec2 { x: 100.0, y: 80.0 });
        let step_y = state.spawn_star_step_y(Vec2 { x: 100.0, y: 80.0 });
        assert_eq!(offsets[1], Vec2 { x: step_x, y: 0.0 });
        assert_eq!(offsets[2], Vec2 { x: -step_x, y: 0.0 });
        assert_eq!(offsets[3], Vec2 { x: 0.0, y: step_y });
        assert_eq!(offsets[4], Vec2 { x: 0.0, y: -step_y });
    }

    #[test]
    fn first_spawn_in_star_is_center() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let (_, pos, needs_pan) = state.pick_spawn_position(Vec2 { x: 100.0, y: 80.0 });
        assert_eq!(pos, Vec2 { x: 0.0, y: 0.0 });
        assert!(!needs_pan);
    }

    #[test]
    fn second_spawn_uses_first_available_star_slot() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let size = Vec2 { x: 100.0, y: 80.0 };
        let first = state
            .model
            .field
            .spawn_surface("first", Vec2 { x: 0.0, y: 0.0 }, size);
        let _ = state
            .model
            .field
            .set_state(first, halley_core::field::NodeState::Active);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(first, 1);
        state.model.focus_state.primary_interaction_focus = Some(first);
        state.assign_node_to_current_monitor(first);
        let current_monitor = state.model.monitor_state.current_monitor.clone();
        state.update_spawn_patch(
            current_monitor.as_str(),
            Vec2 { x: 0.0, y: 0.0 },
            Some(first),
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 1.0, y: 0.0 },
        );

        let (_, pos, needs_pan) = state.pick_spawn_position(size);
        let expected = state
            .right_spawn_candidate_for_focus(first, size)
            .expect("right spawn candidate");
        assert_eq!(pos, expected);
        assert!(!needs_pan);
    }

    #[test]
    fn current_spawn_focus_keeps_focused_window_anchor() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 1200.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let focused = state.model.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(focused, 1);
        state.model.focus_state.primary_interaction_focus = Some(focused);
        state.assign_node_to_current_monitor(focused);
        state
            .model
            .focus_state
            .monitor_focus
            .insert(state.model.monitor_state.current_monitor.clone(), focused);

        assert_eq!(
            state.current_spawn_focus(state.model.monitor_state.current_monitor.as_str()),
            (Some(focused), Vec2 { x: 0.0, y: 0.0 })
        );
    }

    #[test]
    fn view_mode_spawns_near_viewport_center_without_pan() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 1200.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let focused = state.model.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let _ = state
            .model
            .field
            .set_state(focused, halley_core::field::NodeState::Active);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(focused, 1);
        state.model.focus_state.primary_interaction_focus = Some(focused);
        state.assign_node_to_current_monitor(focused);
        {
            let current_monitor = state.model.monitor_state.current_monitor.clone();
            let viewport_center = state.model.viewport.center;
            let spawn = state.spawn_monitor_state_mut(current_monitor.as_str());
            spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
            spawn.spawn_view_anchor = viewport_center;
        }

        let (_, pos, needs_pan) = state.pick_spawn_position(Vec2 { x: 100.0, y: 80.0 });
        assert!(!needs_pan);
        assert_eq!(pos, state.model.viewport.center);
    }

    #[test]
    fn focus_mode_uses_next_free_neighbor_around_last_focus() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 500.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let focused = state.model.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        let _ = state
            .model
            .field
            .set_state(focused, halley_core::field::NodeState::Active);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(focused, 1);
        state.model.focus_state.primary_interaction_focus = Some(focused);
        state.assign_node_to_current_monitor(focused);
        state
            .model
            .focus_state
            .monitor_focus
            .insert(state.model.monitor_state.current_monitor.clone(), focused);
        let current_monitor = state.model.monitor_state.current_monitor.clone();
        state.update_spawn_patch(
            current_monitor.as_str(),
            Vec2 { x: 0.0, y: 0.0 },
            Some(focused),
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 1.0, y: 0.0 },
        );

        let size = Vec2 { x: 120.0, y: 90.0 };
        let existing = state
            .model
            .field
            .spawn_surface("existing", Vec2 { x: 143.0, y: 0.0 }, size);
        state.assign_node_to_current_monitor(existing);
        let (_, pos, needs_pan) = state.pick_spawn_position(size);
        assert_eq!(pos, Vec2 { x: -143.0, y: 0.0 });
        assert!(!needs_pan);
    }

    #[test]
    fn view_mode_continues_local_build_up_around_new_area() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 1200.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };
        {
            let current_monitor = state.model.monitor_state.current_monitor.clone();
            let viewport_center = state.model.viewport.center;
            let spawn = state.spawn_monitor_state_mut(current_monitor.as_str());
            spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
            spawn.spawn_view_anchor = viewport_center;
        }

        let size = Vec2 { x: 100.0, y: 80.0 };
        let first = state.pick_spawn_position(size).1;
        let first_id = state.model.field.spawn_surface("first", first, size);
        state.assign_node_to_current_monitor(first_id);
        let second = state.pick_spawn_position(size).1;
        let step = state.spawn_star_step(size);
        assert_eq!(first, Vec2 { x: 1200.0, y: 0.0 });
        assert_eq!(
            second,
            Vec2 {
                x: 1200.0 + step,
                y: 0.0
            }
        );
    }

    #[test]
    fn focused_monitor_drives_spawn_even_when_current_monitor_differs() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let _ = state.activate_monitor("left");

        let focused = state.model.field.spawn_surface(
            "focused",
            Vec2 {
                x: 1200.0,
                y: 300.0,
            },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(focused, "right");
        state.set_interaction_focus(Some(focused), 30_000, Instant::now());

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        let expected = state
            .right_spawn_candidate_for_focus(focused, Vec2 { x: 120.0, y: 90.0 })
            .expect("right spawn candidate");
        assert_eq!(monitor, "right");
        assert_eq!(pos, expected);
    }

    #[test]
    fn monitor_local_last_input_beats_stale_monitor_focus_for_spawn_anchor() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let stale = state.model.field.spawn_surface(
            "stale",
            Vec2 {
                x: 1040.0,
                y: 300.0,
            },
            Vec2 { x: 120.0, y: 90.0 },
        );
        let latest = state.model.field.spawn_surface(
            "latest",
            Vec2 {
                x: 1320.0,
                y: 300.0,
            },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(stale, "right");
        state.assign_node_to_monitor(latest, "right");
        state
            .model
            .focus_state
            .monitor_focus
            .insert("right".to_string(), stale);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(stale, 1);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(latest, 2);
        state.set_interaction_monitor("right");
        state.set_focused_monitor("right");

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        let expected = state
            .right_spawn_candidate_for_focus(latest, Vec2 { x: 120.0, y: 90.0 })
            .expect("right spawn candidate");
        assert_eq!(monitor, "right");
        assert_eq!(pos, expected);
    }

    #[test]
    fn spawn_buildup_stays_isolated_per_monitor() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let size = Vec2 { x: 100.0, y: 80.0 };
        let step = state.spawn_star_step(size);

        let _ = state.activate_monitor("left");
        state.set_interaction_monitor("left");
        state.set_focused_monitor("left");
        {
            let spawn = state.spawn_monitor_state_mut("left");
            spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
            spawn.spawn_view_anchor = Vec2 { x: 400.0, y: 300.0 };
        }
        let first_left = state.pick_spawn_position(size).1;
        let left_id = state.model.field.spawn_surface("left-1", first_left, size);
        state.assign_node_to_monitor(left_id, "left");

        let _ = state.activate_monitor("right");
        state.set_interaction_monitor("right");
        state.set_focused_monitor("right");
        {
            let spawn = state.spawn_monitor_state_mut("right");
            spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
            spawn.spawn_view_anchor = Vec2 {
                x: 1200.0,
                y: 300.0,
            };
        }
        let first_right = state.pick_spawn_position(size).1;
        let right_id = state
            .model
            .field
            .spawn_surface("right-1", first_right, size);
        state.assign_node_to_monitor(right_id, "right");

        let _ = state.activate_monitor("left");
        state.set_interaction_monitor("left");
        state.set_focused_monitor("left");
        let second_left = state.pick_spawn_position(size).1;

        assert_eq!(first_left, Vec2 { x: 400.0, y: 300.0 });
        assert_eq!(
            first_right,
            Vec2 {
                x: 1200.0,
                y: 300.0
            }
        );
        assert_eq!(
            second_left,
            Vec2 {
                x: 400.0 + step,
                y: 300.0,
            }
        );
    }

    #[test]
    fn focus_mode_keeps_monitor_local_patch_after_auto_focusing_new_spawn() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let size = Vec2 { x: 120.0, y: 90.0 };
        let anchor = state
            .model
            .field
            .spawn_surface("anchor", Vec2 { x: 0.0, y: 0.0 }, size);
        let _ = state
            .model
            .field
            .set_state(anchor, halley_core::field::NodeState::Active);
        state.assign_node_to_current_monitor(anchor);
        state.model.focus_state.primary_interaction_focus = Some(anchor);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(anchor, 1);
        state
            .model
            .focus_state
            .monitor_focus
            .insert(state.model.monitor_state.current_monitor.clone(), anchor);

        let first = state.pick_spawn_position(size).1;
        let first_id = state.model.field.spawn_surface("first", first, size);
        state.assign_node_to_current_monitor(first_id);
        state.set_interaction_focus(Some(first_id), 30_000, Instant::now());

        let second = state.pick_spawn_position(size).1;
        let first_expected = state
            .right_spawn_candidate_for_focus(anchor, size)
            .expect("right spawn candidate");
        let second_expected = state
            .right_spawn_candidate_for_focus(first_id, size)
            .expect("right spawn candidate");

        assert_eq!(first, first_expected);
        assert_eq!(second, second_expected);
    }

    #[test]
    fn focus_monitor_view_switches_spawn_to_clicked_monitor() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let left = state.model.field.spawn_surface(
            "left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(left, "left");
        state.set_interaction_focus(Some(left), 30_000, Instant::now());

        state.focus_monitor_view("right", Instant::now());

        assert_eq!(state.interaction_monitor(), "right");
        assert_eq!(state.focused_monitor(), "right");
        assert_eq!(state.model.focus_state.primary_interaction_focus, None);
        assert_eq!(
            state.spawn_monitor_state("right").spawn_anchor_mode,
            crate::state::SpawnAnchorMode::View
        );

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        assert_eq!(monitor, "right");
        assert_eq!(
            pos,
            Vec2 {
                x: 1200.0,
                y: 300.0
            }
        );
    }

    #[test]
    fn focused_monitor_beats_interaction_monitor_drift_for_spawn_target() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let left = state.model.field.spawn_surface(
            "left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(left, "left");
        state.set_interaction_focus(Some(left), 30_000, Instant::now());
        state.focus_monitor_view("right", Instant::now());

        state.set_interaction_monitor("left");
        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });

        assert_eq!(state.focused_monitor(), "right");
        assert_eq!(monitor, "right");
        assert_eq!(
            pos,
            Vec2 {
                x: 1200.0,
                y: 300.0
            }
        );
    }

    #[test]
    fn focused_monitor_beats_stale_primary_focus_monitor_for_spawn_target() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let left = state.model.field.spawn_surface(
            "left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(left, "left");
        state.model.focus_state.primary_interaction_focus = Some(left);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(left, 1);

        state.focus_monitor_view("right", Instant::now());

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        assert_eq!(monitor, "right");
        assert_eq!(
            pos,
            Vec2 {
                x: 1200.0,
                y: 300.0
            }
        );
    }

    #[test]
    fn pending_spawn_monitor_beats_focus_churn_for_next_toplevel() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let left = state.model.field.spawn_surface(
            "left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(left, "left");
        state.set_interaction_focus(Some(left), 30_000, Instant::now());

        state.model.spawn_state.pending_spawn_monitor = Some("right".to_string());

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        assert_eq!(monitor, "right");
        assert_eq!(
            pos,
            Vec2 {
                x: 1200.0,
                y: 300.0
            }
        );
        assert!(state.model.spawn_state.pending_spawn_monitor.is_none());
    }

    #[test]
    fn shorter_secondary_keeps_building_to_the_right_offscreen() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 2560,
                height: 1440,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 2560,
                offset_y: 0,
                width: 1920,
                height: 1200,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.focus_monitor_view("right", Instant::now());

        let size = Vec2 { x: 883.0, y: 504.0 };
        let first = state.pick_spawn_position(size).1;
        let first_id = state.model.field.spawn_surface("first", first, size);
        state.assign_node_to_monitor(first_id, "right");
        state.set_interaction_focus(Some(first_id), 30_000, Instant::now());

        let second = state.pick_spawn_position(size).1;
        let second_id = state.model.field.spawn_surface("second", second, size);
        state.assign_node_to_monitor(second_id, "right");
        state.set_interaction_focus(Some(second_id), 30_000, Instant::now());
        let third = state.pick_spawn_position(size).1;

        let second_expected = state
            .right_spawn_candidate_for_focus(first_id, size)
            .expect("right spawn candidate");
        let third_expected = state
            .right_spawn_candidate_for_focus(second_id, size)
            .expect("right spawn candidate");
        assert_eq!(
            first,
            Vec2 {
                x: 3520.0,
                y: 600.0
            }
        );
        assert_eq!(second, second_expected);
        assert_eq!(third, third_expected);
    }

    #[test]
    fn focus_mode_checks_neighbors_in_right_left_up_down_order() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let size = Vec2 { x: 120.0, y: 90.0 };
        let focused = state
            .model
            .field
            .spawn_surface("focused", Vec2 { x: 0.0, y: 0.0 }, size);
        state.assign_node_to_current_monitor(focused);
        state.set_interaction_focus(Some(focused), 30_000, Instant::now());

        let right = state
            .spawn_candidate_for_focus_dir(focused, size, Vec2 { x: 1.0, y: 0.0 })
            .expect("right");
        let left = state
            .spawn_candidate_for_focus_dir(focused, size, Vec2 { x: -1.0, y: 0.0 })
            .expect("left");
        let up = state
            .spawn_candidate_for_focus_dir(focused, size, Vec2 { x: 0.0, y: 1.0 })
            .expect("up");
        let down = state
            .spawn_candidate_for_focus_dir(focused, size, Vec2 { x: 0.0, y: -1.0 })
            .expect("down");

        let right_id = state.model.field.spawn_surface("right", right, size);
        let left_id = state.model.field.spawn_surface("left", left, size);
        let up_id = state.model.field.spawn_surface("up", up, size);
        state.assign_node_to_current_monitor(right_id);
        state.assign_node_to_current_monitor(left_id);
        state.assign_node_to_current_monitor(up_id);

        let chosen = state.pick_spawn_position(size).1;
        assert_eq!(chosen, down);
    }

    #[test]
    fn reveal_new_toplevel_skips_pan_when_spawn_is_already_visible() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 700.0, y: 0.0 };
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let id = state.model.field.spawn_surface(
            "new",
            Vec2 { x: 920.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        state.reveal_new_toplevel_node(id, false, Instant::now());

        assert!(state.model.spawn_state.active_spawn_pan.is_none());
        assert!(state.model.spawn_state.pending_spawn_pan_queue.is_empty());
        assert!(state.input.interaction_state.viewport_pan_anim.is_none());
        assert_eq!(state.model.focus_state.primary_interaction_focus, Some(id));
    }

    #[test]
    fn reveal_new_toplevel_pans_when_spawn_is_partially_offscreen() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 700.0, y: 0.0 };
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let id = state.model.field.spawn_surface(
            "partial",
            Vec2 { x: 1460.0, y: 0.0 },
            Vec2 { x: 240.0, y: 160.0 },
        );

        state.reveal_new_toplevel_node(id, false, Instant::now());

        assert_eq!(
            state
                .model
                .spawn_state
                .active_spawn_pan
                .map(|pan| pan.node_id),
            Some(id)
        );
    }

    #[test]
    fn reveal_new_toplevel_pans_when_spawn_is_offscreen() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let id = state.model.field.spawn_surface(
            "new",
            Vec2 { x: 1200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        state.reveal_new_toplevel_node(id, false, Instant::now());

        assert_eq!(
            state
                .model
                .spawn_state
                .active_spawn_pan
                .map(|pan| pan.node_id),
            Some(id)
        );
    }
}
