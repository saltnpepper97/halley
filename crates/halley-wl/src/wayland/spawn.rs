use std::time::Instant;

use halley_core::decay::DecayLevel;
use halley_core::field::{NodeId, Vec2};

use crate::render::ACTIVE_WINDOW_FRAME_PAD_PX;
use crate::state::HalleyWlState;
use crate::wm::overlap::CollisionExtents;

/// Spawn candidates are tried in a deterministic star pattern:
/// center, then right, left, top, bottom for each ring.
fn spawn_cardinal_dirs() -> [Vec2; 4] {
    [
        Vec2 { x: 1.0, y: 0.0 },  // right
        Vec2 { x: -1.0, y: 0.0 }, // left
        Vec2 { x: 0.0, y: 1.0 },  // top
        Vec2 { x: 0.0, y: -1.0 }, // bottom
    ]
}

impl HalleyWlState {
    const SPAWN_STAR_RINGS: usize = 4;
    const SPAWN_CLUSTER_JUMP_DIAGONALS: f32 = 4.25;

    fn current_spawn_focus(&self) -> (Option<NodeId>, Vec2) {
        if self.spawn_anchor_mode == crate::state::SpawnAnchorMode::View {
            return (None, self.spawn_view_anchor);
        }
        if let Some(id) = self.last_input_surface_node()
            && let Some(node) = self.field.node(id)
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
        !self.field.nodes().values().any(|other| {
            if Some(other.id) == skip_node
                || other.kind != halley_core::field::NodeKind::Surface
                || !self.field.is_visible(other.id)
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

    fn star_has_any_room(&self, center: Vec2, size: Vec2) -> bool {
        self.star_candidate_offsets(size).into_iter().any(|offset| {
            self.spawn_candidate_fits(
                Vec2 {
                    x: center.x + offset.x,
                    y: center.y + offset.y,
                },
                size,
                None,
            )
        })
    }

    fn pick_cluster_growth_dir(&self, center: Vec2) -> Vec2 {
        let dirs = spawn_cardinal_dirs();
        let idx = ((self.spawn_cursor as usize)
            .wrapping_add(center.x.abs() as usize)
            .wrapping_add((center.y.abs() * 3.0) as usize))
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

    fn find_nearby_star_center(&self, base: Vec2, size: Vec2) -> Option<(Vec2, Vec2)> {
        let step = self.spawn_star_step(size);
        let star_radius = step * Self::SPAWN_STAR_RINGS as f32;
        let jump = star_radius + step * Self::SPAWN_CLUSTER_JUMP_DIAGONALS;

        let base_dir = self
            .spawn_patch
            .as_ref()
            .map(|p| p.growth_dir)
            .unwrap_or_else(|| self.pick_cluster_growth_dir(base));

        let dirs = [
            base_dir,
            Vec2 {
                x: -base_dir.y,
                y: base_dir.x,
            },
            Vec2 {
                x: base_dir.y,
                y: -base_dir.x,
            },
            Vec2 {
                x: -base_dir.x,
                y: -base_dir.y,
            },
        ];

        for mul in [1.0_f32, 1.35, 1.75, 2.25, 2.9] {
            for dir in dirs {
                let center = Vec2 {
                    x: base.x + dir.x * jump * mul,
                    y: base.y + dir.y * jump * mul,
                };
                if self.star_has_any_room(center, size) {
                    return Some((center, dir));
                }
            }
        }

        None
    }

    /// Returns `(position, needs_pan)`.
    pub(super) fn pick_spawn_position(&mut self, size: Vec2) -> (Vec2, bool) {
        let (focus_id, focus_pos) = self.current_spawn_focus();

        let focus_intersects_view = focus_id
            .map(|id| self.surface_intersects_viewport(id))
            .unwrap_or(false);

        let focus_center_in_view = focus_id
            .and_then(|id| self.field.node(id))
            .map(|node| self.viewport_contains_point(node.pos))
            .unwrap_or(false);

        let local_mode = focus_id.is_some() && focus_intersects_view && focus_center_in_view;
        let remote_focus_mode = focus_id.is_some() && !local_mode;

        let star_center = if let Some(patch) = &self.spawn_patch {
            let same_focus = patch.focus_node == focus_id;
            let same_focus_pos = (patch.focus_pos.x - focus_pos.x).abs() < 0.01
                && (patch.focus_pos.y - focus_pos.y).abs() < 0.01;

            if same_focus || same_focus_pos {
                patch.anchor
            } else {
                focus_pos
            }
        } else {
            focus_pos
        };

        if remote_focus_mode {
            if let Some((center, growth_dir)) = self.find_nearby_star_center(star_center, size)
                && let Some(pos) = self.try_spawn_star(center, size)
            {
                self.update_spawn_patch(center, focus_id, focus_pos, growth_dir);
                return (pos, true);
            }
        }

        if let Some(pos) = self.try_spawn_star(star_center, size) {
            let growth_dir = self
                .spawn_patch
                .as_ref()
                .filter(|patch| {
                    (patch.anchor.x - star_center.x).abs() < 0.01
                        && (patch.anchor.y - star_center.y).abs() < 0.01
                })
                .map(|patch| patch.growth_dir)
                .unwrap_or_else(|| self.pick_cluster_growth_dir(star_center));

            self.update_spawn_patch(star_center, focus_id, focus_pos, growth_dir);

            let is_center =
                (pos.x - star_center.x).abs() < 0.01 && (pos.y - star_center.y).abs() < 0.01;

            return (pos, local_mode && !is_center);
        }

        if let Some((center, growth_dir)) = self.find_nearby_star_center(star_center, size)
            && let Some(pos) = self.try_spawn_star(center, size)
        {
            self.update_spawn_patch(center, focus_id, focus_pos, growth_dir);
            return (pos, true);
        }

        let requested = self.spawn_view_anchor;
        if self.spawn_anchor_mode == crate::state::SpawnAnchorMode::View {
            if let Some(pos) = self.try_spawn_star(requested, size) {
                let growth_dir = self.pick_cluster_growth_dir(requested);
                self.update_spawn_patch(requested, None, requested, growth_dir);
                self.spawn_view_anchor = requested;
                return (pos, false);
            }

            if let Some((center, growth_dir)) = self.find_nearby_star_center(requested, size)
                && let Some(pos) = self.try_spawn_star(center, size)
            {
                self.spawn_view_anchor = center;
                self.update_spawn_patch(center, None, center, growth_dir);
                return (pos, true);
            }
        }

        if let Some((center, growth_dir)) = self.find_nearby_star_center(self.viewport.center, size)
            && let Some(pos) = self.try_spawn_star(center, size)
        {
            self.spawn_view_anchor = center;
            self.update_spawn_patch(center, None, center, growth_dir);
            return (pos, true);
        }

        let fallback = self.viewport.center;
        let growth_dir = self.pick_cluster_growth_dir(fallback);
        self.update_spawn_patch(fallback, None, fallback, growth_dir);
        (fallback, false)
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
            || (now_ms >= active.pan_start_at_ms && self.viewport_pan_anim.is_none());
        if !pan_finished {
            return;
        }

        let _ = self.field.set_detached(active.node_id, false);
        let _ = self.field.set_decay_level(active.node_id, DecayLevel::Hot);
        if let Some(node) = self.field.node(active.node_id) {
            self.last_active_size
                .insert(active.node_id, node.intrinsic_size);
        }
        self.mark_active_transition(active.node_id, now, 620);
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

        let visible_in_view = self
            .field
            .node(id)
            .is_some_and(|node| self.viewport_contains_point(node.pos));
        if visible_in_view {
            self.mark_active_transition(id, now, 620);
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
    fn star_offsets_are_center_then_right_left_top_bottom() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let state = HalleyWlState::new(&dh, tuning);

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
        let mut state = HalleyWlState::new(&dh, tuning);
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
        let mut state = HalleyWlState::new(&dh, tuning);
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
        state.last_surface_focus_ms.insert(first, 1);
        state.interaction_focus = Some(first);
        state.update_spawn_patch(
            Vec2 { x: 0.0, y: 0.0 },
            Some(first),
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 1.0, y: 0.0 },
        );

        let (pos, needs_pan) = state.pick_spawn_position(size);
        let step = state.spawn_star_step(size);
        assert_eq!(pos, Vec2 { x: step, y: 0.0 });
        assert!(needs_pan);
    }

    #[test]
    fn current_spawn_focus_keeps_offscreen_focus_anchor() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        state.viewport.center = Vec2 { x: 1200.0, y: 0.0 };
        state.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let focused = state.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.last_surface_focus_ms.insert(focused, 1);
        state.interaction_focus = Some(focused);

        assert_eq!(
            state.current_spawn_focus(),
            (Some(focused), Vec2 { x: 0.0, y: 0.0 })
        );
    }

    #[test]
    fn offscreen_focused_surface_spawns_to_new_star_and_pans() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
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
        state.last_surface_focus_ms.insert(focused, 1);
        state.interaction_focus = Some(focused);

        let (pos, needs_pan) = state.pick_spawn_position(Vec2 { x: 100.0, y: 80.0 });
        assert!(needs_pan);
        assert_ne!(pos, Vec2 { x: 0.0, y: 0.0 });
    }

    #[test]
    fn partially_visible_focused_surface_spawns_to_new_star_and_pans() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        state.viewport.center = Vec2 { x: 500.0, y: 0.0 };
        state.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let focused = state.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 500.0, y: 300.0 },
        );
        let _ = state
            .field
            .set_state(focused, halley_core::field::NodeState::Active);
        state.last_surface_focus_ms.insert(focused, 1);
        state.interaction_focus = Some(focused);

        assert!(state.surface_intersects_viewport(focused));
        assert!(!state.viewport_contains_point(Vec2 { x: 0.0, y: 0.0 }));

        let (pos, needs_pan) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        assert!(needs_pan);
        assert_ne!(pos, Vec2 { x: 0.0, y: 0.0 });
    }

    #[test]
    fn reveal_new_toplevel_skips_pan_when_spawn_is_already_visible() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
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
        assert!(state.viewport_pan_anim.is_none());
        assert_eq!(state.interaction_focus, Some(id));
    }

    #[test]
    fn reveal_new_toplevel_pans_when_spawn_is_offscreen() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
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
