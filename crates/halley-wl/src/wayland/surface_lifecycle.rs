use std::time::Instant;

use halley_core::decay::DecayLevel;
use halley_core::field::{NodeId, Vec2};
use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};

use crate::activity::CommitActivity;
use crate::state::HalleyWlState;
use crate::wm::overlap::CollisionExtents;

/// Generates candidate spawn positions using a Fermat spiral (golden-angle
/// phyllotaxis). Placing candidates at successive golden-angle increments
/// produces the most isotropic possible coverage — the same sunflower-seed
/// packing used in botany — with no preferred axis to lock growth onto.
///
/// Ring is `ceil(sqrt(n))` so points at roughly equal radii share a ring,
/// preserving the existing ring-threshold semantics for frontier tracking.
fn spawn_fermat_candidates(step: f32, count: usize) -> Vec<(Vec2, u32)> {
    // 2π(1 - 1/φ) ≈ 137.508° — the golden angle in radians.
    const GOLDEN_ANGLE: f32 = 2.399_963_2_f32;
    let mut out = Vec::with_capacity(count);
    out.push((Vec2 { x: 0.0, y: 0.0 }, 0u32));
    for n in 1..count {
        let angle = n as f32 * GOLDEN_ANGLE;
        let radius = (n as f32).sqrt() * step;
        let ring = (n as f32).sqrt().ceil() as u32;
        out.push((
            Vec2 {
                x: angle.cos() * radius,
                y: angle.sin() * radius,
            },
            ring,
        ));
    }
    out
}

impl HalleyWlState {
    /// Number of Fermat spiral candidates tried when nothing adjacent fits.
    /// 89 is a Fibonacci number and gives decent radial coverage (~10 rings).
    const SPAWN_FERMAT_COUNT: usize = 89;
    /// How far to jump (in window diagonals) when seeding a fresh island.
    const SPAWN_ISLAND_JUMP_DIAGONALS: f32 = 5.5;

    #[inline]
    fn surface_key(surface: &WlSurface) -> ObjectId {
        surface.id()
    }

    /// Returns the currently focused surface node's id, center position, and
    /// intrinsic size — the anchor for adjacent placement.
    fn focused_surface(&self) -> Option<(NodeId, Vec2, Vec2)> {
        let id = self.last_input_surface_node()?;
        let node = self.field.node(id)?;
        Some((id, node.pos, node.intrinsic_size))
    }

    fn viewport_contains_point(&self, pos: Vec2) -> bool {
        let half_w = self.viewport.size.x * 0.5;
        let half_h = self.viewport.size.y * 0.5;
        pos.x >= self.viewport.center.x - half_w
            && pos.x <= self.viewport.center.x + half_w
            && pos.y >= self.viewport.center.y - half_h
            && pos.y <= self.viewport.center.y + half_h
    }

    fn current_spawn_focus(&self) -> (Option<NodeId>, Vec2) {
        if self.spawn_anchor_mode == crate::state::SpawnAnchorMode::View {
            return (None, self.spawn_view_anchor);
        }
        if let Some(id) = self.last_input_surface_node()
            && let Some(node) = self.field.node(id)
        {
            if !self.viewport_contains_point(node.pos) {
                return (None, self.viewport.center);
            }
            return (Some(id), node.pos);
        }
        (None, self.viewport.center)
    }

    /// Try to place a window of `size` directly adjacent to the currently
    /// focused window. Checks right → left → below → above and returns the
    /// first position that fits, or `None` if all four are blocked.
    ///
    /// In view-anchor mode (user panned away from focused window) this step is
    /// skipped so new windows open near the viewport rather than off-screen.
    fn try_spawn_adjacent(&self, size: Vec2) -> Option<Vec2> {
        if self.spawn_anchor_mode == crate::state::SpawnAnchorMode::View {
            return None;
        }
        let (focus_id, focus_pos, focus_size) = self.focused_surface()?;
        if !self.viewport_contains_point(focus_pos) {
            return None;
        }
        let gap = self.non_overlap_gap_world();
        // Half-extents of the focus and new window, used to compute
        // edge-to-edge offsets so the new window sits flush with a gap.
        let fx = focus_size.x * 0.5;
        let fy = focus_size.y * 0.5;
        let nx = size.x * 0.5;
        let ny = size.y * 0.5;
        let candidates = [
            Vec2 {
                x: focus_pos.x + fx + gap + nx,
                y: focus_pos.y,
            }, // right
            Vec2 {
                x: focus_pos.x - fx - gap - nx,
                y: focus_pos.y,
            }, // left
            Vec2 {
                x: focus_pos.x,
                y: focus_pos.y + fy + gap + ny,
            }, // below
            Vec2 {
                x: focus_pos.x,
                y: focus_pos.y - fy - gap - ny,
            }, // above
        ];
        candidates
            .into_iter()
            .find(|&pos| self.spawn_candidate_fits(pos, size, Some(focus_id)))
    }

    /// Try to place a window of `size` using a Fermat spiral expanding
    /// outward from `center`. Returns the first position that fits.
    fn try_spawn_fermat(&self, center: Vec2, size: Vec2) -> Option<Vec2> {
        let gap = self.non_overlap_gap_world();
        let step = ((size.x + gap) * (size.y + gap)).sqrt();
        for (offset, _ring) in spawn_fermat_candidates(step, Self::SPAWN_FERMAT_COUNT) {
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

    /// Last-resort placement: jump to a fresh island anchor and pan there.
    /// Tries the stored growth direction first, then rotations of it.
    /// Returns `(position, needs_pan=true)` on success, or falls back to
    /// viewport center with `needs_pan=false` if even the jump fails.
    fn try_island_jump(&mut self, size: Vec2) -> (Vec2, bool) {
        let jump_dist = size.x.hypot(size.y) * Self::SPAWN_ISLAND_JUMP_DIAGONALS;
        let base = self
            .spawn_patch
            .as_ref()
            .map(|p| p.anchor)
            .unwrap_or(self.viewport.center);
        let base_dir = self
            .spawn_patch
            .as_ref()
            .map(|p| p.growth_dir)
            .unwrap_or(Vec2 { x: 1.0, y: 0.0 });

        // Try the current growth direction, then 90° rotations.
        let rotations: [Vec2; 4] = [
            base_dir,
            Vec2 {
                x: -base_dir.y,
                y: base_dir.x,
            },
            Vec2 {
                x: -base_dir.x,
                y: -base_dir.y,
            },
            Vec2 {
                x: base_dir.y,
                y: -base_dir.x,
            },
        ];
        for dir in rotations {
            let island_center = Vec2 {
                x: base.x + dir.x * jump_dist,
                y: base.y + dir.y * jump_dist,
            };
            if let Some(pos) = self.try_spawn_fermat(island_center, size) {
                self.spawn_patch = Some(crate::state::SpawnPatch {
                    anchor: island_center,
                    focus_node: None,
                    focus_pos: island_center,
                    growth_dir: dir,
                    placements_in_patch: 0,
                    frontier: Vec::new(),
                });
                return (pos, true);
            }
        }
        // Absolute fallback — nothing fit anywhere.
        (self.viewport.center, false)
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

    /// Returns `(position, needs_pan)`. `needs_pan` is `true` when an island
    /// jump occurred and the caller should pan the viewport to the new window.
    ///
    /// Priority order:
    ///   1. Directly adjacent to focused window  (right → left → below → above)
    ///   2. Fermat spiral outward from focused position  (first fit)
    ///   3. Island jump: new anchor in growth direction  (+ pan)
    fn pick_spawn_position(&mut self, size: Vec2) -> (Vec2, bool) {
        // Step 1: adjacent to focused window.
        if let Some(pos) = self.try_spawn_adjacent(size) {
            return (pos, false);
        }

        // Step 2: Fermat spiral from focus / view anchor.
        let focus_pos = self.current_spawn_focus().1;
        if let Some(pos) = self.try_spawn_fermat(focus_pos, size) {
            return (pos, false);
        }

        // Step 3: everything near focus is packed — jump to a fresh island.
        self.try_island_jump(size)
    }

    pub fn note_commit(&mut self, surface: &WlSurface, now: Instant) {
        let key = Self::surface_key(surface);
        self.surface_activity
            .entry(key)
            .or_insert_with(|| CommitActivity::new(now))
            .on_commit(now);
        if let Some(output) = &self.primary_output {
            output.enter(surface);
        }

        // Grant keyboard focus to layer surfaces (e.g. fuzzel) on their first
        // real commit, when keyboard_interactivity is now populated.
        self.maybe_grant_layer_surface_focus_on_commit(surface);
    }

    pub fn ensure_node_for_surface(
        &mut self,
        surface: &WlSurface,
        label: &str,
        size_px: (i32, i32),
    ) -> NodeId {
        let key = Self::surface_key(surface);
        if let Some(id) = self.surface_to_node.get(&key).copied() {
            return id;
        }

        self.spawn_cursor += 1;
        let size = Vec2 {
            x: size_px.0.max(64) as f32,
            y: size_px.1.max(64) as f32,
        };
        let (pos, needs_pan) = self.pick_spawn_position(size);

        let id = self.field.spawn_surface(label.to_string(), pos, size);
        let _ = self
            .field
            .set_state(id, halley_core::field::NodeState::Active);
        let _ = self.field.set_decay_level(id, DecayLevel::Hot);

        self.surface_to_node.insert(key, id);
        self.zoom_nominal_size.insert(id, size);
        self.last_active_size.insert(id, size);
        if self.tuning.dev_anim_enabled {
            self.animator.observe_field(&self.field, Instant::now());
        }
        // Island jump: pan the viewport to the new window so the spatial move
        // feels intentional rather than silently spawning off-screen.
        if needs_pan {
            self.queue_spawn_pan_to_node(id, Instant::now());
        }
        id
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
        self.push_neighbors_for_activation(active.node_id);
        self.active_spawn_pan = None;
        self.maybe_start_pending_spawn_pan(now);
    }

    pub fn drop_surface(&mut self, surface: &WlSurface) {
        if let Some(output) = &self.primary_output {
            output.leave(surface);
        }
        let key = Self::surface_key(surface);
        self.surface_activity.remove(&key);
        if let Some(id) = self.surface_to_node.remove(&key) {
            if self.pan_restore_active_focus == Some(id) {
                self.pan_restore_active_focus = None;
            }
            let _ = self.field.undock_node(id);
            self.field.clear_dock_preview();
            self.zoom_nominal_size.remove(&id);
            self.zoom_resize_fallback.remove(&id);
            self.zoom_resize_reject_streak.remove(&id);
            self.zoom_last_observed_size.remove(&id);
            self.zoom_resize_static_streak.remove(&id);
            self.last_active_size.remove(&id);
            self.bbox_loc.remove(&id);
            self.window_geometry.remove(&id);
            self.pending_spawn_activate_at_ms.remove(&id);
            self.active_transition_until_ms.remove(&id);
            self.primary_promote_cooldown_until_ms.remove(&id);
            self.last_surface_focus_ms.remove(&id);
            self.carry_zone_hint.remove(&id);
            self.carry_zone_last_change_ms.remove(&id);
            self.carry_zone_pending.remove(&id);
            self.carry_zone_pending_since_ms.remove(&id);
            self.carry_activation_anim_armed.remove(&id);
            self.release_smoothing_until_ms.remove(&id);
            self.release_axis_lock.remove(&id);
            self.physics_velocity.remove(&id);
            if self.resize_active == Some(id) {
                self.resize_active = None;
            }
            if self.resize_static_node == Some(id) {
                self.resize_static_node = None;
                self.resize_static_lock_pos = None;
                self.resize_static_until_ms = 0;
            }
            if self.interaction_focus == Some(id) {
                self.interaction_focus = None;
                self.interaction_focus_until_ms = 0;
            }
            self.smoothed_render_pos.remove(&id);
            let _ = self.field.remove(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fermat_candidates_cover_all_octants() {
        let candidates = spawn_fermat_candidates(100.0, 89);
        let mut octants = [false; 8];
        for (pos, _) in candidates.iter().skip(1) {
            let angle = pos.y.atan2(pos.x);
            let octant = ((angle / (std::f32::consts::PI / 4.0)).rem_euclid(8.0)) as usize;
            octants[octant.min(7)] = true;
        }
        assert!(
            octants.iter().all(|&covered| covered),
            "some octants not covered: {:?}",
            octants
        );
    }

    #[test]
    fn try_spawn_adjacent_prefers_right_then_left() {
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
        let focus = state
            .field
            .spawn_surface("focus", Vec2 { x: 0.0, y: 0.0 }, size);
        let _ = state
            .field
            .set_state(focus, halley_core::field::NodeState::Active);
        state.last_surface_focus_ms.insert(focus, 1);
        state.interaction_focus = Some(focus);

        // With nothing to the right, adjacent placement should go right.
        let pos = state.try_spawn_adjacent(size).expect("should fit right");
        assert!(pos.x > 0.0, "expected right placement, got {:?}", pos);
        assert_eq!(pos.y, 0.0, "y should match focus");
    }

    #[test]
    fn try_spawn_adjacent_falls_back_to_left_when_right_blocked() {
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
        let focus = state
            .field
            .spawn_surface("focus", Vec2 { x: 0.0, y: 0.0 }, size);
        let _ = state
            .field
            .set_state(focus, halley_core::field::NodeState::Active);
        state.last_surface_focus_ms.insert(focus, 1);
        state.interaction_focus = Some(focus);

        // Block the right slot.
        let gap = state.non_overlap_gap_world();
        let right_x = size.x + gap + size.x * 0.5;
        let _ = state
            .field
            .spawn_surface("blocker", Vec2 { x: right_x, y: 0.0 }, size);

        let pos = state.try_spawn_adjacent(size).expect("should fit left");
        assert!(pos.x < 0.0, "expected left placement, got {:?}", pos);
    }

    #[test]
    fn try_spawn_fermat_returns_none_when_fully_packed() {
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
        let gap = state.non_overlap_gap_world();
        let step = ((size.x + gap) * (size.y + gap)).sqrt();
        // Place blockers at every Fermat candidate position.
        for (offset, _) in spawn_fermat_candidates(step, HalleyWlState::SPAWN_FERMAT_COUNT) {
            let pos = Vec2 {
                x: offset.x,
                y: offset.y,
            };
            state.field.spawn_surface("blocker", pos, size);
        }

        assert!(
            state
                .try_spawn_fermat(Vec2 { x: 0.0, y: 0.0 }, size)
                .is_none(),
            "should return None when all candidates are blocked"
        );
    }

    #[test]
    fn try_spawn_adjacent_skipped_in_view_anchor_mode() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        state.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let size = Vec2 { x: 100.0, y: 80.0 };
        let focus = state
            .field
            .spawn_surface("focus", Vec2 { x: 0.0, y: 0.0 }, size);
        state.last_surface_focus_ms.insert(focus, 1);
        state.interaction_focus = Some(focus);

        // Trigger view-anchor mode via a meaningful pan.
        let now = Instant::now();
        state.note_pan_activity(now);
        state.viewport.center = Vec2 { x: 700.0, y: 0.0 };
        state.note_pan_viewport_change(now);

        assert_eq!(state.spawn_anchor_mode, crate::state::SpawnAnchorMode::View);
        assert!(
            state.try_spawn_adjacent(size).is_none(),
            "adjacent placement should be skipped in view-anchor mode"
        );
    }

    #[test]
    fn current_spawn_focus_uses_view_anchor_after_meaningful_pan_handoff() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        state.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        let focused = state.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.last_surface_focus_ms.insert(focused, 1);
        state.interaction_focus = Some(focused);

        let now = Instant::now();
        state.note_pan_activity(now);
        state.viewport.center = Vec2 { x: 700.0, y: 0.0 };
        state.note_pan_viewport_change(now);

        assert_eq!(state.spawn_anchor_mode, crate::state::SpawnAnchorMode::View);
        assert_eq!(
            state.current_spawn_focus(),
            (None, Vec2 { x: 700.0, y: 0.0 })
        );
    }

    #[test]
    fn current_spawn_focus_uses_viewport_when_focused_surface_is_offscreen() {
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

        assert_eq!(state.current_spawn_focus(), (None, state.viewport.center));
        assert!(
            state
                .try_spawn_adjacent(Vec2 { x: 100.0, y: 80.0 })
                .is_none(),
            "adjacent placement should be skipped when focused surface is off-screen"
        );
    }
}
