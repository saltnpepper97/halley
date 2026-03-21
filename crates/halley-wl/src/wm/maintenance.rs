use std::collections::HashSet;

use super::*;
use crate::wm::overlap::CollisionExtents;
use halley_core::viewport::{FocusRing, FocusZone};

impl HalleyWlState {
    const ACTIVE_RING_OUTSIDE_DECAY_FRAC: f32 = 0.98;

    fn focus_ring_coverage_for_extents(
        &self,
        pos: Vec2,
        ext: CollisionExtents,
        focus_ring: FocusRing,
    ) -> (f32, f32) {
        let samples = 9usize;
        let width = (ext.left + ext.right).max(1.0);
        let height = (ext.top + ext.bottom).max(1.0);
        let left = pos.x - ext.left;
        let top = pos.y - ext.top;
        let mut inside = 0usize;
        let mut total = 0usize;

        for ix in 0..samples {
            for iy in 0..samples {
                let fx = ix as f32 / (samples - 1) as f32;
                let fy = iy as f32 / (samples - 1) as f32;
                let sample = Vec2 {
                    x: left + fx * width,
                    y: top + fy * height,
                };
                if focus_ring.zone(self.viewport.center, sample) == FocusZone::Inside {
                    inside += 1;
                }
                total += 1;
            }
        }

        if total == 0 {
            return (0.0, 1.0);
        }

        let inside_frac = inside as f32 / total as f32;
        (inside_frac, (1.0 - inside_frac).max(0.0))
    }

    fn surface_ring_coverage(&self, id: NodeId, focus_ring: FocusRing) -> (f32, f32) {
        let Some(node) = self.field.node(id) else {
            return (0.0, 1.0);
        };

        let ext = match node.state {
            halley_core::field::NodeState::Active => self.surface_window_collision_extents(node),
            _ => self.collision_extents_for_node(node),
        };

        self.focus_ring_coverage_for_extents(node.pos, ext, focus_ring)
    }

    fn surface_is_definitively_outside_focus_ring(
        &self,
        id: NodeId,
        focus_ring: FocusRing,
    ) -> bool {
        let (_, outside_frac) = self.surface_ring_coverage(id, focus_ring);
        outside_frac >= Self::ACTIVE_RING_OUTSIDE_DECAY_FRAC
    }

    pub(crate) fn enforce_single_primary_active_unit(&mut self, focus_ring: FocusRing) {
        let now_ms = self.now_ms(Instant::now());
        let active_windows_allowed = self.tuning.active_windows_allowed.max(1);
        let companion = self.companion_surface_node(now_ms);
        let preferred_surface = self.last_input_surface_node();

        let active_ids: Vec<NodeId> = self
            .field
            .nodes()
            .iter()
            .filter_map(|(&id, n)| {
                (self.field.is_visible(id)
                    && n.kind == halley_core::field::NodeKind::Surface
                    && n.state == halley_core::field::NodeState::Active)
                    .then_some(id)
            })
            .collect();

        if active_ids.len() <= active_windows_allowed {
            return;
        }

        let mut keep_set: HashSet<NodeId> = HashSet::new();

        let focused_breakout: Option<NodeId> = active_ids
            .iter()
            .copied()
            .find(|&id| self.interaction_focus == Some(id))
            .or_else(|| {
                active_ids
                    .iter()
                    .copied()
                    .max_by_key(|id| self.last_surface_focus_ms.get(id).copied().unwrap_or(0))
            });

        if let Some(fid) = focused_breakout {
            keep_set.insert(fid);
        }

        if keep_set.len() < active_windows_allowed {
            let mut ranked = active_ids.clone();
            ranked.sort_by_key(|id| {
                let preferred_rank = u8::from(preferred_surface == Some(*id));
                let focus_rank = u8::from(self.interaction_focus == Some(*id));
                let companion_rank = u8::from(companion == Some(*id));
                let inside_rank =
                    u8::from(!self.surface_is_definitively_outside_focus_ring(*id, focus_ring));
                let latest_focus = self.last_surface_focus_ms.get(id).copied().unwrap_or(0);
                (
                    preferred_rank,
                    focus_rank,
                    companion_rank,
                    inside_rank,
                    latest_focus,
                    id.as_u64(),
                )
            });

            for id in ranked.iter().rev().copied() {
                keep_set.insert(id);
                if keep_set.len() >= active_windows_allowed {
                    break;
                }
            }
        }

        for id in active_ids {
            if keep_set.contains(&id) {
                continue;
            }
            let _ = self.field.set_decay_level(id, DecayLevel::Cold);
        }
    }

    pub(crate) fn process_pending_spawn_activations(&mut self, now: Instant, now_ms: u64) {
        let due: Vec<NodeId> = self
            .pending_spawn_activate_at_ms
            .iter()
            .filter_map(|(&id, &at)| (now_ms >= at).then_some(id))
            .collect();

        for id in due {
            self.pending_spawn_activate_at_ms.remove(&id);
            if !self.field.is_visible(id) {
                continue;
            }
            let Some(n) = self.field.node(id) else {
                continue;
            };
            if n.kind != halley_core::field::NodeKind::Surface {
                continue;
            }
            if self.preserve_collapsed_surface(id) {
                continue;
            }
            let _ = self.field.set_decay_level(id, DecayLevel::Hot);
            if let Some(nn) = self.field.node(id) {
                self.last_active_size.insert(id, nn.intrinsic_size);
            }
            self.mark_active_transition(id, now, 620);
            self.record_focus_trail_visit(id);
            self.suppress_trail_record_once = true;
            self.set_interaction_focus(Some(id), 30_000, now);
        }
    }

    pub(crate) fn enforce_carry_zone_states(&mut self) {
        let tracked: Vec<(NodeId, FocusZone)> = self
            .carry_zone_hint
            .iter()
            .map(|(&id, &z)| (id, z))
            .collect();

        for (id, zone) in tracked {
            if !self.field.is_visible(id) {
                continue;
            }
            let Some(n) = self.field.node(id) else {
                continue;
            };
            if n.kind != halley_core::field::NodeKind::Surface {
                continue;
            }
            if self.preserve_collapsed_surface(id) {
                continue;
            }

            let held_state = self.carry_state_hold.get(&id);
            let target = match zone {
                _ if matches!(held_state, Some(halley_core::field::NodeState::Active)) => {
                    DecayLevel::Hot
                }
                _ if matches!(
                    held_state,
                    Some(halley_core::field::NodeState::Node | halley_core::field::NodeState::Core)
                ) =>
                {
                    DecayLevel::Cold
                }
                FocusZone::Inside if n.state == halley_core::field::NodeState::Active => {
                    DecayLevel::Hot
                }
                FocusZone::Inside => DecayLevel::Cold,
                FocusZone::Outside => DecayLevel::Cold,
            };
            let _ = self.field.set_decay_level(id, target);
        }
    }

    pub(crate) fn enforce_pan_dominant_zone_states(&mut self, focus_ring: FocusRing, now_ms: u64) {
        let active_outside_ring_delay_ms = self.tuning.active_outside_ring_delay_ms;
        let inactive_outside_ring_delay_ms = self.tuning.inactive_outside_ring_delay_ms;

        let ids: Vec<NodeId> = self.field.nodes().keys().copied().collect();

        for id in ids {
            self.apply_single_surface_decay_policy(
                id,
                focus_ring,
                now_ms,
                active_outside_ring_delay_ms,
                inactive_outside_ring_delay_ms,
            );
        }

        self.dock_decay_offscreen_since_ms.retain(|id, _| {
            self.field.node(*id).is_some_and(|n| {
                self.field.is_visible(*id) && n.kind == halley_core::field::NodeKind::Surface
            })
        });
    }

    fn apply_single_surface_decay_policy(
        &mut self,
        id: NodeId,
        focus_ring: FocusRing,
        now_ms: u64,
        active_delay_ms: u64,
        inactive_delay_ms: u64,
    ) {
        let Some(n) = self.field.node(id) else {
            self.dock_decay_offscreen_since_ms.remove(&id);
            return;
        };
        if !self.field.is_visible(id) || n.kind != halley_core::field::NodeKind::Surface {
            self.dock_decay_offscreen_since_ms.remove(&id);
            return;
        }

        if self.preserve_collapsed_surface(id) {
            self.dock_decay_offscreen_since_ms.remove(&id);
            return;
        }

        if self.is_hard_decay_protected(id, now_ms) {
            self.dock_decay_offscreen_since_ms.remove(&id);
            let _ = self.field.set_decay_level(id, DecayLevel::Hot);
            return;
        }

        let outside_ring = self.surface_is_definitively_outside_focus_ring(id, focus_ring);
        if !outside_ring {
            self.dock_decay_offscreen_since_ms.remove(&id);
            let _ = self.field.set_decay_level(id, DecayLevel::Hot);
            return;
        }

        let is_primary = self.interaction_focus == Some(id);
        let delay_ms = if is_primary {
            active_delay_ms
        } else {
            inactive_delay_ms
        };

        let since = self
            .dock_decay_offscreen_since_ms
            .get(&id)
            .copied()
            .unwrap_or(now_ms);

        self.dock_decay_offscreen_since_ms.insert(id, since);

        if now_ms.saturating_sub(since) >= delay_ms {
            let _ = self.field.set_decay_level(id, DecayLevel::Cold);
        } else {
            let _ = self.field.set_decay_level(id, DecayLevel::Hot);
        }
    }

    fn is_hard_decay_protected(&self, id: NodeId, now_ms: u64) -> bool {
        self.interaction_focus == Some(id)
            || self.resize_active == Some(id)
            || self.is_recently_resized_node(id, now_ms)
            || self.carry_zone_hint.contains_key(&id)
            || self.active_transition_until_ms.contains_key(&id)
    }

    pub fn surface_intersects_viewport(&self, id: NodeId) -> bool {
        let Some(n) = self.field.node(id) else {
            return false;
        };
        if n.kind != halley_core::field::NodeKind::Surface || !self.field.is_visible(id) {
            return false;
        }

        let ext = self.collision_extents_for_node(n);
        let half_vw = self.viewport.size.x * 0.5;
        let half_vh = self.viewport.size.y * 0.5;

        let view_left = self.viewport.center.x - half_vw;
        let view_right = self.viewport.center.x + half_vw;
        let view_top = self.viewport.center.y - half_vh;
        let view_bottom = self.viewport.center.y + half_vh;

        let node_left = n.pos.x - ext.left;
        let node_right = n.pos.x + ext.right;
        let node_top = n.pos.y - ext.top;
        let node_bottom = n.pos.y + ext.bottom;

        node_right > view_left
            && node_left < view_right
            && node_bottom > view_top
            && node_top < view_bottom
    }

    pub(crate) fn reconcile_surface_bindings(&mut self) {
        const STALE_SURFACE_GRACE_MS: u64 = 1500;
        let now = Instant::now();

        let alive: HashSet<ObjectId> = self
            .xdg_shell_state
            .toplevel_surfaces()
            .iter()
            .map(|t| t.wl_surface().id())
            .collect();

        let stale: Vec<ObjectId> = self
            .surface_to_node
            .keys()
            .filter(|k| !alive.contains(*k))
            .filter(|k| {
                let Some(activity) = self.surface_activity.get(*k) else {
                    return true;
                };
                now.duration_since(activity.last_commit_at()).as_millis() as u64
                    >= STALE_SURFACE_GRACE_MS
            })
            .cloned()
            .collect();

        for key in stale {
            self.surface_activity.remove(&key);
            if let Some(id) = self.surface_to_node.remove(&key) {
                if self.pan_restore_active_focus == Some(id) {
                    self.pan_restore_active_focus = None;
                }
                self.manual_collapsed_nodes.remove(&id);
                self.zoom_nominal_size.remove(&id);
                self.zoom_resize_fallback.remove(&id);
                self.zoom_resize_reject_streak.remove(&id);
                self.zoom_last_observed_size.remove(&id);
                self.zoom_resize_static_streak.remove(&id);
                self.node_app_ids.remove(&id);
                self.last_active_size.remove(&id);
                self.bbox_loc.remove(&id);
                self.window_geometry.remove(&id);
                self.pending_spawn_activate_at_ms.remove(&id);
                self.active_transition_until_ms.remove(&id);
                self.primary_promote_cooldown_until_ms.remove(&id);
                self.last_surface_focus_ms.remove(&id);
                self.dock_decay_offscreen_since_ms.remove(&id);
                self.carry_zone_hint.remove(&id);
                self.carry_zone_last_change_ms.remove(&id);
                self.carry_zone_pending.remove(&id);
                self.carry_zone_pending_since_ms.remove(&id);
                self.carry_activation_anim_armed.remove(&id);
                self.carry_state_hold.remove(&id);
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

        self.surface_activity.retain(|k, _| alive.contains(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_surface_with_small_ring_overlap_is_not_treated_as_outside() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.focus_ring_rx = 100.0;
        tuning.focus_ring_ry = 100.0;
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new_for_test(&dh, tuning);

        let id = state.field.spawn_surface(
            "edge-overlap",
            Vec2 { x: 145.0, y: 0.0 },
            Vec2 { x: 100.0, y: 100.0 },
        );
        state
            .last_active_size
            .insert(id, Vec2 { x: 100.0, y: 100.0 });
        state
            .window_geometry
            .insert(id, (-50.0, -50.0, 100.0, 100.0));
        state.bbox_loc.insert(id, (0.0, 0.0));

        assert!(!state.surface_is_definitively_outside_focus_ring(id, state.active_focus_ring()));
    }

    #[test]
    fn active_surface_fully_clear_of_ring_is_treated_as_outside() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.focus_ring_rx = 100.0;
        tuning.focus_ring_ry = 100.0;
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new_for_test(&dh, tuning);

        let id = state.field.spawn_surface(
            "outside",
            Vec2 { x: 260.0, y: 0.0 },
            Vec2 { x: 100.0, y: 100.0 },
        );
        state
            .last_active_size
            .insert(id, Vec2 { x: 100.0, y: 100.0 });
        state
            .window_geometry
            .insert(id, (-50.0, -50.0, 100.0, 100.0));
        state.bbox_loc.insert(id, (0.0, 0.0));

        assert!(state.surface_is_definitively_outside_focus_ring(id, state.active_focus_ring()));
    }
}
