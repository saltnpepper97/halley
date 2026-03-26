use std::time::Instant;

use halley_core::decay::DecayLevel;
use halley_core::field::{NodeId, Vec2};

use crate::render::ACTIVE_WINDOW_FRAME_PAD_PX;
use crate::state::Halley;
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

impl Halley {
    const SPAWN_STAR_RINGS: usize = 24;

    fn spawn_anchor_on_monitor(&self, anchor: Vec2, monitor: &str) -> bool {
        self.monitor_for_screen(anchor.x, anchor.y).as_deref() == Some(monitor)
    }

    fn viewport_center_for_monitor(&self, monitor: &str) -> Vec2 {
        self.monitor_state
            .monitors
            .get(monitor)
            .map(|space| space.viewport.center)
            .unwrap_or(self.viewport.center)
    }

    fn resolve_spawn_target_monitor(&self) -> String {
        let fallback = self.interaction_monitor().to_string();
        if self
            .spawn_monitor_state(fallback.as_str())
            .spawn_anchor_mode
            == crate::state::SpawnAnchorMode::View
        {
            return fallback;
        }
        if let Some(id) = self
            .focus_state
            .primary_interaction_focus
            .or_else(|| self.last_input_surface_node())
            && let Some(monitor) = self.monitor_state.node_monitor.get(&id)
        {
            return monitor.clone();
        }
        fallback
    }

    fn current_spawn_focus(&self, monitor: &str) -> (Option<NodeId>, Vec2) {
        let spawn = self.spawn_monitor_state(monitor);
        let viewport_center = self.viewport_center_for_monitor(monitor);
        if spawn.spawn_anchor_mode == crate::state::SpawnAnchorMode::View {
            let anchor = if self.spawn_anchor_on_monitor(spawn.spawn_view_anchor, monitor) {
                spawn.spawn_view_anchor
            } else {
                viewport_center
            };
            return (None, anchor);
        }
        if let Some(id) = self.last_input_surface_node_for_monitor(monitor)
            && let Some(node) = self.field.node(id)
        {
            return (Some(id), node.pos);
        }
        (None, viewport_center)
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
                .monitor_state
                .node_monitor
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

    fn pick_cluster_growth_dir(&self, monitor: &str, center: Vec2) -> Vec2 {
        let dirs = spawn_cardinal_dirs();
        let local = self
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
        let target_monitor = self.resolve_spawn_target_monitor();
        self.spawn_monitor_state_mut(target_monitor.as_str()).spawn_cursor += 1;
        let monitor_spawn = self.spawn_monitor_state(target_monitor.as_str());
        let viewport_center = self.viewport_center_for_monitor(target_monitor.as_str());
        let (focus_id, focus_pos) = self.current_spawn_focus(target_monitor.as_str());
        let use_view_patch = monitor_spawn.spawn_anchor_mode == crate::state::SpawnAnchorMode::View;
        let patch_focus_continues = monitor_spawn.spawn_patch.as_ref().is_some_and(|patch| {
            self.spawn_anchor_on_monitor(patch.anchor, target_monitor.as_str())
                && focus_id.is_some()
                && (patch.focus_pos.x - focus_pos.x).hypot(patch.focus_pos.y - focus_pos.y)
                    <= self.spawn_star_step(size) + 1.0
        });

        let anchor = if use_view_patch {
            monitor_spawn
                .spawn_patch
                .as_ref()
                .filter(|patch| {
                    patch.focus_node.is_none()
                        && self.spawn_anchor_on_monitor(patch.anchor, target_monitor.as_str())
                })
                .map(|patch| patch.anchor)
                .unwrap_or(focus_pos)
        } else if let Some(patch) = &monitor_spawn.spawn_patch {
            let same_focus = patch.focus_node == focus_id;
            let same_focus_pos = (patch.focus_pos.x - focus_pos.x).abs() < 0.01
                && (patch.focus_pos.y - focus_pos.y).abs() < 0.01;
            if same_focus
                && same_focus_pos
                && self.spawn_anchor_on_monitor(patch.anchor, target_monitor.as_str())
            {
                patch.anchor
            } else if patch_focus_continues {
                patch.anchor
            } else {
                focus_pos
            }
        } else {
            focus_pos
        };

        if let Some(pos) = self.try_spawn_star(anchor, size) {
            let growth_dir = self.pick_cluster_growth_dir(target_monitor.as_str(), anchor);
            let patch_focus = if use_view_patch { None } else { focus_id };
            let patch_focus_pos = if use_view_patch {
                viewport_center
            } else {
                focus_pos
            };
            self.update_spawn_patch(
                target_monitor.as_str(),
                anchor,
                patch_focus,
                patch_focus_pos,
                growth_dir,
            );
            if use_view_patch {
                self.spawn_monitor_state_mut(target_monitor.as_str())
                    .spawn_view_anchor = anchor;
            }
            return (target_monitor, pos, false);
        }

        let fallback_anchor = if use_view_patch {
            viewport_center
        } else {
            focus_pos
        };
        let growth_dir = self.pick_cluster_growth_dir(target_monitor.as_str(), fallback_anchor);
        let patch_focus = if use_view_patch { None } else { focus_id };
        let patch_focus_pos = if use_view_patch {
            viewport_center
        } else {
            focus_pos
        };
        self.update_spawn_patch(
            target_monitor.as_str(),
            fallback_anchor,
            patch_focus,
            patch_focus_pos,
            growth_dir,
        );
        if use_view_patch {
            self.spawn_monitor_state_mut(target_monitor.as_str())
                .spawn_view_anchor = fallback_anchor;
        }
        (target_monitor, fallback_anchor, false)
    }

    pub(crate) fn queue_spawn_pan_to_node(&mut self, id: NodeId, now: Instant) {
        let Some(target_center) = self.field.node(id).map(|node| node.pos) else {
            return;
        };
        let _ = self.field.set_detached(id, true);
        self.spawn_state.pending_spawn_activate_at_ms.remove(&id);
        self.spawn_state.pending_spawn_pan_queue
            .push_back(crate::state::PendingSpawnPan {
                node_id: id,
                target_center,
            });
        self.maybe_start_pending_spawn_pan(now);
    }

    pub(crate) fn maybe_start_pending_spawn_pan(&mut self, now: Instant) {
        if self.spawn_state.active_spawn_pan.is_some() {
            return;
        }

        let now_ms = self.now_ms(now);
        while let Some(next) = self.spawn_state.pending_spawn_pan_queue.pop_front() {
            if self.field.node(next.node_id).is_none() {
                continue;
            }

            let did_pan = self.animate_viewport_center_to_delayed(
                next.target_center,
                now,
                Self::VIEWPORT_PAN_PRELOAD_MS,
            );
            self.spawn_state.active_spawn_pan = Some(crate::state::ActiveSpawnPan {
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
        let Some(active) = self.spawn_state.active_spawn_pan else {
            self.maybe_start_pending_spawn_pan(now);
            return;
        };

        if self.field.node(active.node_id).is_none() {
            self.spawn_state.active_spawn_pan = None;
            self.maybe_start_pending_spawn_pan(now);
            return;
        }

        let pan_finished = now_ms >= active.reveal_at_ms
            || (now_ms >= active.pan_start_at_ms
                && self.interaction_state.viewport_pan_anim.is_none());
        if !pan_finished {
            return;
        }

        let _ = self.field.set_detached(active.node_id, false);
        let _ = self.field.set_decay_level(active.node_id, DecayLevel::Hot);
        if let Some(node) = self.field.node(active.node_id) {
            self.workspace_state
                .last_active_size
                .insert(active.node_id, node.intrinsic_size);
        }
        self.mark_active_transition(active.node_id, now, 620);
        self.record_focus_trail_visit(active.node_id);
        self.focus_state.suppress_trail_record_once = true;
        self.set_interaction_focus(Some(active.node_id), 30_000, now);
        self.spawn_state.active_spawn_pan = None;
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
            self.spawn_state.pending_spawn_activate_at_ms.remove(&id);
            self.mark_active_transition(id, now, 620);
            return;
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
    fn star_offsets_are_center_then_right_left_up_down() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let state = Halley::new_for_test(&dh, tuning);

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
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.viewport.size = Vec2 {
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
        let current_monitor = state.monitor_state.current_monitor.clone();
        state.update_spawn_patch(
            current_monitor.as_str(),
            Vec2 { x: 0.0, y: 0.0 },
            Some(first),
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 1.0, y: 0.0 },
        );

        let (_, pos, needs_pan) = state.pick_spawn_position(size);
        let step = state.spawn_star_step(size);
        assert_eq!(pos, Vec2 { x: step, y: 0.0 });
        assert!(!needs_pan);
    }

    #[test]
    fn current_spawn_focus_keeps_focused_window_anchor() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 1200.0, y: 0.0 };
        state.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let focused = state.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.focus_state.last_surface_focus_ms.insert(focused, 1);
        state.focus_state.primary_interaction_focus = Some(focused);
        state.assign_node_to_current_monitor(focused);
        state.focus_state
            .monitor_focus
            .insert(state.monitor_state.current_monitor.clone(), focused);

        assert_eq!(
            state.current_spawn_focus(state.monitor_state.current_monitor.as_str()),
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
        state.assign_node_to_current_monitor(focused);
        {
            let current_monitor = state.monitor_state.current_monitor.clone();
            let viewport_center = state.viewport.center;
            let spawn = state.spawn_monitor_state_mut(current_monitor.as_str());
            spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
            spawn.spawn_view_anchor = viewport_center;
        }

        let (_, pos, needs_pan) = state.pick_spawn_position(Vec2 { x: 100.0, y: 80.0 });
        assert!(!needs_pan);
        assert_eq!(pos, state.viewport.center);
    }

    #[test]
    fn focus_mode_keeps_building_around_last_focus() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
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
        state.assign_node_to_current_monitor(focused);
        state.focus_state
            .monitor_focus
            .insert(state.monitor_state.current_monitor.clone(), focused);
        let current_monitor = state.monitor_state.current_monitor.clone();
        state.update_spawn_patch(
            current_monitor.as_str(),
            Vec2 { x: 0.0, y: 0.0 },
            Some(focused),
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 1.0, y: 0.0 },
        );

        let size = Vec2 { x: 120.0, y: 90.0 };
        let _ = state
            .field
            .spawn_surface("existing", Vec2 { x: 0.0, y: 0.0 }, size);
        let (_, pos, needs_pan) = state.pick_spawn_position(size);
        let step = state.spawn_star_step(size);
        assert_eq!(pos, Vec2 { x: step, y: 0.0 });
        assert!(!needs_pan);
    }

    #[test]
    fn view_mode_continues_local_build_up_around_new_area() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 1200.0, y: 0.0 };
        state.viewport.size = Vec2 { x: 800.0, y: 600.0 };
        {
            let current_monitor = state.monitor_state.current_monitor.clone();
            let viewport_center = state.viewport.center;
            let spawn = state.spawn_monitor_state_mut(current_monitor.as_str());
            spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
            spawn.spawn_view_anchor = viewport_center;
        }

        let size = Vec2 { x: 100.0, y: 80.0 };
        let first = state.pick_spawn_position(size).1;
        let first_id = state.field.spawn_surface("first", first, size);
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

        let focused = state.field.spawn_surface(
            "focused",
            Vec2 { x: 1200.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(focused, "right");
        state.focus_state.primary_interaction_focus = Some(focused);
        state.focus_state.last_surface_focus_ms.insert(focused, 1);
        state.focus_state.monitor_focus.insert("right".to_string(), focused);

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        let step = state.spawn_star_step(Vec2 { x: 120.0, y: 90.0 });
        assert_eq!(monitor, "right");
        assert_eq!(
            pos,
            Vec2 {
                x: 1200.0 + step,
                y: 300.0,
            }
        );
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

        let stale = state.field.spawn_surface(
            "stale",
            Vec2 { x: 1040.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        let latest = state.field.spawn_surface(
            "latest",
            Vec2 { x: 1320.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(stale, "right");
        state.assign_node_to_monitor(latest, "right");
        state.focus_state
            .monitor_focus
            .insert("right".to_string(), stale);
        state.focus_state.last_surface_focus_ms.insert(stale, 1);
        state.focus_state.last_surface_focus_ms.insert(latest, 2);
        state.set_interaction_monitor("right");

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        let step = state.spawn_star_step(Vec2 { x: 120.0, y: 90.0 });
        assert_eq!(monitor, "right");
        assert_eq!(
            pos,
            Vec2 {
                x: 1320.0 + step,
                y: 300.0,
            }
        );
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
        {
            let spawn = state.spawn_monitor_state_mut("left");
            spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
            spawn.spawn_view_anchor = Vec2 { x: 400.0, y: 300.0 };
        }
        let first_left = state.pick_spawn_position(size).1;
        let left_id = state.field.spawn_surface("left-1", first_left, size);
        state.assign_node_to_monitor(left_id, "left");

        let _ = state.activate_monitor("right");
        state.set_interaction_monitor("right");
        {
            let spawn = state.spawn_monitor_state_mut("right");
            spawn.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
            spawn.spawn_view_anchor = Vec2 { x: 1200.0, y: 300.0 };
        }
        let first_right = state.pick_spawn_position(size).1;
        let right_id = state.field.spawn_surface("right-1", first_right, size);
        state.assign_node_to_monitor(right_id, "right");

        let _ = state.activate_monitor("left");
        state.set_interaction_monitor("left");
        let second_left = state.pick_spawn_position(size).1;

        assert_eq!(first_left, Vec2 { x: 400.0, y: 300.0 });
        assert_eq!(first_right, Vec2 { x: 1200.0, y: 300.0 });
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
        state.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.viewport.size = Vec2 { x: 1600.0, y: 1200.0 };

        let size = Vec2 { x: 120.0, y: 90.0 };
        let anchor = state.field.spawn_surface("anchor", Vec2 { x: 0.0, y: 0.0 }, size);
        let _ = state
            .field
            .set_state(anchor, halley_core::field::NodeState::Active);
        state.assign_node_to_current_monitor(anchor);
        state.focus_state.primary_interaction_focus = Some(anchor);
        state.focus_state.last_surface_focus_ms.insert(anchor, 1);
        state.focus_state
            .monitor_focus
            .insert(state.monitor_state.current_monitor.clone(), anchor);

        let first = state.pick_spawn_position(size).1;
        let first_id = state.field.spawn_surface("first", first, size);
        state.assign_node_to_current_monitor(first_id);
        state.set_interaction_focus(Some(first_id), 30_000, Instant::now());

        let second = state.pick_spawn_position(size).1;
        let step = state.spawn_star_step(size);

        assert_eq!(first, Vec2 { x: step, y: 0.0 });
        assert_eq!(second, Vec2 { x: -step, y: 0.0 });
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

        let left = state.field.spawn_surface(
            "left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(left, "left");
        state.set_interaction_focus(Some(left), 30_000, Instant::now());

        state.focus_monitor_view("right", Instant::now());

        assert_eq!(state.interaction_monitor(), "right");
        assert_eq!(state.focus_state.primary_interaction_focus, None);
        assert_eq!(
            state.spawn_monitor_state("right").spawn_anchor_mode,
            crate::state::SpawnAnchorMode::View
        );

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        assert_eq!(monitor, "right");
        assert_eq!(pos, Vec2 { x: 1200.0, y: 300.0 });
    }

    #[test]
    fn reveal_new_toplevel_skips_pan_when_spawn_is_already_visible() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
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

        assert!(state.spawn_state.active_spawn_pan.is_none());
        assert!(state.spawn_state.pending_spawn_pan_queue.is_empty());
        assert!(state.interaction_state.viewport_pan_anim.is_none());
        assert_eq!(state.focus_state.primary_interaction_focus, Some(id));
    }

    #[test]
    fn reveal_new_toplevel_pans_when_spawn_is_partially_offscreen() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
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

        assert_eq!(state.spawn_state.active_spawn_pan.map(|pan| pan.node_id), Some(id));
    }

    #[test]
    fn reveal_new_toplevel_pans_when_spawn_is_offscreen() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let id = state.field.spawn_surface(
            "new",
            Vec2 { x: 1200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        state.reveal_new_toplevel_node(id, false, Instant::now());

        assert_eq!(state.spawn_state.active_spawn_pan.map(|pan| pan.node_id), Some(id));
    }
}
