use std::time::Instant;

use halley_core::decay::DecayLevel;
use halley_core::field::{NodeId, Vec2};

use crate::render::ACTIVE_WINDOW_FRAME_PAD_PX;
use crate::state::HalleyWlState;
use crate::wm::overlap::CollisionExtents;

/// Spawn candidates are tried in a deterministic star pattern:
/// center, then left, right, up, down for each ring.
fn spawn_cardinal_dirs() -> [Vec2; 4] {
    [
        Vec2 { x: 1.0, y: 0.0 },  // right
        Vec2 { x: -1.0, y: 0.0 }, // left
        Vec2 { x: 0.0, y: 1.0 },  // up
        Vec2 { x: 0.0, y: -1.0 }, // down
    ]
}

impl HalleyWlState {
    const SPAWN_STAR_RINGS: usize = 24;

    fn spawn_anchor_on_current_monitor(&self, anchor: Vec2) -> bool {
        self.monitor_for_screen(anchor.x, anchor.y).as_deref() == Some(self.monitor_state.current_monitor.as_str())
    }

    fn current_spawn_focus(&self) -> (Option<NodeId>, Vec2) {
        if self.spawn_anchor_mode == crate::state::SpawnAnchorMode::View {
            let anchor = if self.spawn_anchor_on_current_monitor(self.spawn_view_anchor) {
                self.spawn_view_anchor
            } else {
                self.viewport.center
            };
            return (None, anchor);
        }
        if let Some(id) = self.last_input_surface_node()
            && let Some(node) = self.field.node(id)
            && self
                .monitor_state.node_monitor
                .get(&id)
                .is_none_or(|monitor| monitor == &self.monitor_state.current_monitor)
        {
            return (Some(id), node.pos);
        }
        (None, self.viewport.center)
    }

    fn spawn_star_step(&self, size: Vec2) -> f32 {
        size.x.max(size.y)
            + (ACTIVE_WINDOW_FRAME_PAD_PX.max(0) as f32 * 2.0)
            + self.non_overlap_gap_world()
    }

    fn star_candidate_offsets(&self, size: Vec2) -> Vec<Vec2> {
        let step = self.spawn_star_step(size);
        let mut out = Vec::with_capacity(1 + Self::SPAWN_STAR_RINGS * spawn_cardinal_dirs().len());

        out.push(Vec2 { x: 0.0, y: 0.0 });

        for ring in 1..=Self::SPAWN_STAR_RINGS {
            let d = step * ring as f32;
            for dir in spawn_cardinal_dirs() {
                out.push(Vec2 {
                    x: dir.x * d,
                    y: dir.y * d,
                });
            }
        }

        out
    }

    fn spawn_candidate_fits(&self, pos: Vec2, size: Vec2, skip_node: Option<NodeId>) -> bool {
        let pair_gap = self.non_overlap_gap_world();
        let candidate = CollisionExtents::symmetric(size);
        let candidate_monitor = self
            .monitor_for_screen(pos.x, pos.y)
            .unwrap_or_else(|| self.monitor_state.current_monitor.clone());
        !self.field.nodes().values().any(|other| {
            if Some(other.id) == skip_node
                || other.kind != halley_core::field::NodeKind::Surface
                || !self.field.is_visible(other.id)
            {
                return false;
            }
            if self
                .monitor_state.node_monitor
                .get(&other.id)
                .is_some_and(|monitor| monitor != &candidate_monitor)
            {
                return false;
            }
            let other_ext = self.spawn_obstacle_extents_for_node(other);
            let req_x = self.required_sep_x(pos.x, candidate, other.pos.x, other_ext, pair_gap);
            let req_y = self.required_sep_y(pos.y, candidate, other.pos.y, other_ext, pair_gap);
            (pos.x - other.pos.x).abs() < req_x && (pos.y - other.pos.y).abs() < req_y
        })
    }

    fn try_spawn_star(&self, center: Vec2, size: Vec2) -> Option<Vec2> {
        for offset in self.star_candidate_offsets(size) {
            let pos = Vec2 {
                x: center.x + offset.x,
                y: center.y + offset.y,
            };
            if self.spawn_candidate_fits(pos, size, None) {
                return Some(pos);
            }
        }
        None
    }

    fn pick_cluster_growth_dir(&self, center: Vec2) -> Vec2 {
        let dirs = spawn_cardinal_dirs();
        let local = self
            .monitor_for_screen(center.x, center.y)
            .and_then(|monitor| self.monitor_state.monitors.get(monitor.as_str()))
            .map(|monitor| {
                Vec2 {
                    x: center.x - monitor.offset_x as f32,
                    y: center.y - monitor.offset_y as f32,
                }
            })
            .unwrap_or(center);
        let idx = ((self.spawn_cursor as usize)
            .wrapping_add(local.x.abs() as usize)
            .wrapping_add((local.y.abs() * 3.0) as usize))
            % dirs.len();
        dirs[idx]
    }

    fn update_spawn_patch(
        &mut self,
        anchor: Vec2,
        focus_node: Option<NodeId>,
        focus_pos: Vec2,
        growth_dir: Vec2,
    ) {
        self.spawn_patch = Some(crate::state::SpawnPatch {
            anchor,
            focus_node,
            focus_pos,
            growth_dir,
            placements_in_patch: 0,
            frontier: Vec::new(),
        });
    }

    /// Returns `(position, needs_pan)`.
    pub(super) fn pick_spawn_position(&mut self, size: Vec2) -> (Vec2, bool) {
        let (focus_id, focus_pos) = self.current_spawn_focus();
        let use_view_patch = self.spawn_anchor_mode == crate::state::SpawnAnchorMode::View;

        let anchor = if use_view_patch {
            self.spawn_patch
                .as_ref()
                .filter(|patch| {
                    patch.focus_node.is_none() && self.spawn_anchor_on_current_monitor(patch.anchor)
                })
                .map(|patch| patch.anchor)
                .unwrap_or(self.viewport.center)
        } else if let Some(patch) = &self.spawn_patch {
            let same_focus = patch.focus_node == focus_id;
            let same_focus_pos = (patch.focus_pos.x - focus_pos.x).abs() < 0.01
                && (patch.focus_pos.y - focus_pos.y).abs() < 0.01;
            if same_focus && same_focus_pos && self.spawn_anchor_on_current_monitor(patch.anchor) {
                patch.anchor
            } else {
                focus_pos
            }
        } else {
            focus_pos
        };

        if let Some(pos) = self.try_spawn_star(anchor, size) {
            let growth_dir = self.pick_cluster_growth_dir(anchor);
            let patch_focus = if use_view_patch { None } else { focus_id };
            let patch_focus_pos = if use_view_patch {
                self.viewport.center
            } else {
                focus_pos
            };
            self.update_spawn_patch(anchor, patch_focus, patch_focus_pos, growth_dir);
            if use_view_patch {
                self.spawn_view_anchor = anchor;
            }
            return (pos, false);
        }

        let fallback_anchor = if use_view_patch {
            self.viewport.center
        } else {
            focus_pos
        };
        let growth_dir = self.pick_cluster_growth_dir(fallback_anchor);
        let patch_focus = if use_view_patch { None } else { focus_id };
        let patch_focus_pos = if use_view_patch {
            self.viewport.center
        } else {
            focus_pos
        };
        self.update_spawn_patch(fallback_anchor, patch_focus, patch_focus_pos, growth_dir);
        if use_view_patch {
            self.spawn_view_anchor = fallback_anchor;
        }
        (fallback_anchor, false)
    }

    pub(crate) fn queue_spawn_pan_to_node(&mut self, id: NodeId, now: Instant) {
        let Some(target_center) = self.field.node(id).map(|node| node.pos) else {
            return;
        };
        let _ = self.field.set_detached(id, true);
        self.pending_spawn_activate_at_ms.remove(&id);
        self.pending_spawn_pan_queue
            .push_back(crate::state::PendingSpawnPan {
                node_id: id,
                target_center,
            });
        self.maybe_start_pending_spawn_pan(now);
    }

    pub(crate) fn maybe_start_pending_spawn_pan(&mut self, now: Instant) {
        if self.active_spawn_pan.is_some() {
            return;
        }

        let now_ms = self.now_ms(now);
        while let Some(next) = self.pending_spawn_pan_queue.pop_front() {
            if self.field.node(next.node_id).is_none() {
                continue;
            }

            let did_pan = self.animate_viewport_center_to_delayed(
                next.target_center,
                now,
                Self::VIEWPORT_PAN_PRELOAD_MS,
            );
            self.active_spawn_pan = Some(crate::state::ActiveSpawnPan {
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
        let Some(active) = self.active_spawn_pan else {
            self.maybe_start_pending_spawn_pan(now);
            return;
        };

        if self.field.node(active.node_id).is_none() {
            self.active_spawn_pan = None;
            self.maybe_start_pending_spawn_pan(now);
            return;
        }

        let pan_finished = now_ms >= active.reveal_at_ms
            || (now_ms >= active.pan_start_at_ms && self.interaction_state.viewport_pan_anim.is_none());
        if !pan_finished {
            return;
        }

        let _ = self.field.set_detached(active.node_id, false);
        let _ = self.field.set_decay_level(active.node_id, DecayLevel::Hot);
        if let Some(node) = self.field.node(active.node_id) {
            self.workspace_state.last_active_size
                .insert(active.node_id, node.intrinsic_size);
        }
        self.mark_active_transition(active.node_id, now, 620);
        self.record_focus_trail_visit(active.node_id);
        self.focus_state.suppress_trail_record_once = true;
        self.set_interaction_focus(Some(active.node_id), 30_000, now);
        self.active_spawn_pan = None;
        self.maybe_start_pending_spawn_pan(now);
    }

    pub(crate) fn reveal_new_toplevel_node(
        &mut self,
        id: NodeId,
        is_transient: bool,
        now: Instant,
    ) {
        if is_transient {
            self.record_focus_trail_visit(id);
            self.focus_state.suppress_trail_record_once = true;
            self.set_interaction_focus(Some(id), 30_000, now);
            self.pending_spawn_activate_at_ms.remove(&id);
            self.mark_active_transition(id, now, 620);
            return;
        }

        if self
            .active_spawn_pan
            .is_some_and(|active| active.node_id == id)
            || self
                .pending_spawn_pan_queue
                .iter()
                .any(|pending| pending.node_id == id)
        {
            return;
        }

        let fully_visible_in_view = self.viewport_fully_contains_surface(id);
        if fully_visible_in_view || !self.tuning.pan_to_new {
            self.mark_active_transition(id, now, 620);
            self.record_focus_trail_visit(id);
            self.focus_state.suppress_trail_record_once = true;
            self.set_interaction_focus(Some(id), 30_000, now);
        } else {
            self.queue_spawn_pan_to_node(id, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_offsets_are_center_then_left_right_up_down() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let state = HalleyWlState::new_for_test(&dh, tuning);

        let offsets = state.star_candidate_offsets(Vec2 { x: 100.0, y: 80.0 });
        assert_eq!(offsets[0], Vec2 { x: 0.0, y: 0.0 });

        let step = state.spawn_star_step(Vec2 { x: 100.0, y: 80.0 });
        assert_eq!(offsets[1], Vec2 { x: step, y: 0.0 });
        assert_eq!(offsets[2], Vec2 { x: -step, y: 0.0 });
        assert_eq!(offsets[3], Vec2 { x: 0.0, y: step });
        assert_eq!(offsets[4], Vec2 { x: 0.0, y: -step });
    }

    #[test]
    fn first_spawn_in_star_is_center() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let (pos, needs_pan) = state.pick_spawn_position(Vec2 { x: 100.0, y: 80.0 });
        assert_eq!(pos, Vec2 { x: 0.0, y: 0.0 });
        assert!(!needs_pan);
    }

    #[test]
    fn second_spawn_uses_first_available_star_slot() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let size = Vec2 { x: 100.0, y: 80.0 };
        let first = state
            .field
            .spawn_surface("first", Vec2 { x: 0.0, y: 0.0 }, size);
        let _ = state
            .field
            .set_state(first, halley_core::field::NodeState::Active);
        state.focus_state.last_surface_focus_ms.insert(first, 1);
        state.focus_state.primary_interaction_focus = Some(first);
        state.update_spawn_patch(
            Vec2 { x: 0.0, y: 0.0 },
            Some(first),
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 1.0, y: 0.0 },
        );

        let (pos, needs_pan) = state.pick_spawn_position(size);
        let step = state.spawn_star_step(size);
        assert_eq!(pos, Vec2 { x: -step, y: 0.0 });
        assert!(!needs_pan);
    }

    #[test]
    fn current_spawn_focus_keeps_focused_window_anchor() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 1200.0, y: 0.0 };
        state.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let focused = state.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.focus_state.last_surface_focus_ms.insert(focused, 1);
        state.focus_state.primary_interaction_focus = Some(focused);

        assert_eq!(
            state.current_spawn_focus(),
            (Some(focused), Vec2 { x: 0.0, y: 0.0 })
        );
    }

    #[test]
    fn view_mode_spawns_near_viewport_center_without_pan() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 1200.0, y: 0.0 };
        state.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let focused = state.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let _ = state
            .field
            .set_state(focused, halley_core::field::NodeState::Active);
        state.focus_state.last_surface_focus_ms.insert(focused, 1);
        state.focus_state.primary_interaction_focus = Some(focused);
        state.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
        state.spawn_view_anchor = state.viewport.center;

        let (pos, needs_pan) = state.pick_spawn_position(Vec2 { x: 100.0, y: 80.0 });
        assert!(!needs_pan);
        assert_eq!(pos, state.viewport.center);
    }

    #[test]
    fn focus_mode_keeps_building_around_last_focus() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 500.0, y: 0.0 };
        state.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let focused = state.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        let _ = state
            .field
            .set_state(focused, halley_core::field::NodeState::Active);
        state.focus_state.last_surface_focus_ms.insert(focused, 1);
        state.focus_state.primary_interaction_focus = Some(focused);
        state.update_spawn_patch(
            Vec2 { x: 0.0, y: 0.0 },
            Some(focused),
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 1.0, y: 0.0 },
        );

        let size = Vec2 { x: 120.0, y: 90.0 };
        let _ = state
            .field
            .spawn_surface("existing", Vec2 { x: 0.0, y: 0.0 }, size);
        let (pos, needs_pan) = state.pick_spawn_position(size);
        let step = state.spawn_star_step(size);
        assert_eq!(pos, Vec2 { x: -step, y: 0.0 });
        assert!(!needs_pan);
    }

    #[test]
    fn view_mode_continues_local_build_up_around_new_area() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 1200.0, y: 0.0 };
        state.viewport.size = Vec2 { x: 800.0, y: 600.0 };
        state.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
        state.spawn_view_anchor = state.viewport.center;

        let size = Vec2 { x: 100.0, y: 80.0 };
        let first = state.pick_spawn_position(size).0;
        let _ = state.field.spawn_surface("first", first, size);
        let second = state.pick_spawn_position(size).0;
        let step = state.spawn_star_step(size);
        assert_eq!(first, Vec2 { x: 1200.0, y: 0.0 });
        assert_eq!(
            second,
            Vec2 {
                x: 1200.0 - step,
                y: 0.0
            }
        );
    }

    #[test]
    fn reveal_new_toplevel_skips_pan_when_spawn_is_already_visible() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 700.0, y: 0.0 };
        state.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let id =
            state
                .field
                .spawn_surface("new", Vec2 { x: 920.0, y: 0.0 }, Vec2 { x: 100.0, y: 80.0 });

        state.reveal_new_toplevel_node(id, false, Instant::now());

        assert!(state.active_spawn_pan.is_none());
        assert!(state.pending_spawn_pan_queue.is_empty());
        assert!(state.interaction_state.viewport_pan_anim.is_none());
        assert_eq!(state.focus_state.primary_interaction_focus, Some(id));
    }

    #[test]
    fn reveal_new_toplevel_pans_when_spawn_is_partially_offscreen() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 700.0, y: 0.0 };
        state.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let id = state.field.spawn_surface(
            "partial",
            Vec2 { x: 1460.0, y: 0.0 },
            Vec2 { x: 240.0, y: 160.0 },
        );

        state.reveal_new_toplevel_node(id, false, Instant::now());

        assert_eq!(state.active_spawn_pan.map(|pan| pan.node_id), Some(id));
    }

    #[test]
    fn reveal_new_toplevel_pans_when_spawn_is_offscreen() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let id = state.field.spawn_surface(
            "new",
            Vec2 { x: 1200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        state.reveal_new_toplevel_node(id, false, Instant::now());

        assert_eq!(state.active_spawn_pan.map(|pan| pan.node_id), Some(id));
    }
}
