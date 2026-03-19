use std::collections::HashSet;

use super::*;
use halley_core::viewport::{FocusRing, FocusZone};

impl HalleyWlState {
    pub(crate) fn enforce_single_primary_active_unit(&mut self, focus_ring: FocusRing) {
        let now_ms = self.now_ms(Instant::now());
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

        if active_ids.len() <= 2 {
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

        if keep_set.len() < 2 {
            let mut ranked = active_ids.clone();
            ranked.sort_by_key(|id| {
                let pos = self
                    .field
                    .node(*id)
                    .map(|n| n.pos)
                    .unwrap_or(self.viewport.center);
                let preferred_rank = u8::from(preferred_surface == Some(*id));
                let focus_rank = u8::from(self.interaction_focus == Some(*id));
                let companion_rank = u8::from(companion == Some(*id));
                let inside_rank =
                    u8::from(focus_ring.zone(self.viewport.center, pos) == FocusZone::Inside);
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
                if keep_set.len() >= 2 {
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

            let target = match zone {
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
        let primary_outside_ring_delay_ms = self.tuning.primary_outside_ring_delay_ms;
        let secondary_outside_ring_delay_ms = self.tuning.secondary_outside_ring_delay_ms;

        let ids: Vec<NodeId> = self.field.nodes().keys().copied().collect();

        for id in ids {
            self.apply_single_surface_decay_policy(
                id,
                focus_ring,
                now_ms,
                primary_outside_ring_delay_ms,
                secondary_outside_ring_delay_ms,
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
        primary_delay_ms: u64,
        secondary_delay_ms: u64,
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

        let outside_ring = focus_ring.zone(self.viewport.center, n.pos) == FocusZone::Outside;
        if !outside_ring {
            self.dock_decay_offscreen_since_ms.remove(&id);
            let _ = self.field.set_decay_level(id, DecayLevel::Hot);
            return;
        }

        let is_primary = self.interaction_focus == Some(id);
        let is_secondary = self.companion_surface_node(now_ms) == Some(id);
        let delay_ms = if is_primary {
            primary_delay_ms
        } else {
            let _ = is_secondary;
            secondary_delay_ms
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
