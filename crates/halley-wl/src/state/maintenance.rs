use std::collections::HashSet;

use super::*;
use halley_core::viewport::{FocusRing, FocusZone};

impl HalleyWlState {
    pub(crate) fn enforce_single_primary_active_unit(&mut self, focus_ring: FocusRing) {
        let mut inside_ids: Vec<NodeId> = self
            .field
            .nodes()
            .iter()
            .filter_map(|(&id, n)| {
                (self.field.is_visible(id)
                    && n.kind == halley_core::field::NodeKind::Surface
                    && focus_ring.zone(self.viewport.center, n.pos) == FocusZone::Inside)
                    .then_some(id)
            })
            .collect();

        if inside_ids.is_empty() {
            return;
        }

        inside_ids.sort_by_key(|id| id.as_u64());

        let mut units: Vec<Vec<NodeId>> = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::new();

        for id in inside_ids {
            if seen.contains(&id) {
                continue;
            }
            let mut unit = vec![id];
            if let Some(link) = self.docked_links.get(&id) {
                let partner = link.partner;
                if self
                    .docked_links
                    .get(&partner)
                    .is_some_and(|back| back.partner == id)
                    && self.field.node(partner).is_some_and(|pn| {
                        self.field.is_visible(partner)
                            && pn.kind == halley_core::field::NodeKind::Surface
                    })
                {
                    unit.push(partner);
                    seen.insert(partner);
                }
            }
            seen.insert(id);
            units.push(unit);
        }

        if units.is_empty() {
            return;
        }

        let active_unit_indices: Vec<usize> = units
            .iter()
            .enumerate()
            .filter_map(|(idx, unit)| {
                unit.iter()
                    .any(|id| {
                        self.field.node(*id).is_some_and(|n| {
                            n.kind == halley_core::field::NodeKind::Surface
                                && n.state == halley_core::field::NodeState::Active
                        })
                    })
                    .then_some(idx)
            })
            .collect();

        if active_unit_indices.len() <= 1 {
            return;
        }

        let preferred_surface = self.last_input_surface_node();
        let selected_idx = active_unit_indices.iter().copied().max_by_key(|idx| {
            let unit = &units[*idx];
            let preferred_rank = u8::from(
                preferred_surface.is_some_and(|preferred| unit.iter().any(|id| *id == preferred)),
            );
            let focused = self
                .interaction_focus
                .is_some_and(|fid| unit.iter().any(|id| *id == fid));
            let focus_rank = u8::from(focused);
            let latest_focus = unit
                .iter()
                .filter_map(|id| self.last_surface_focus_ms.get(id).copied())
                .max()
                .unwrap_or(0);
            let latest_id = unit.iter().map(|id| id.as_u64()).max().unwrap_or(0);
            (preferred_rank, focus_rank, latest_focus, latest_id)
        });

        let Some(selected_idx) = selected_idx else {
            return;
        };

        let mut losing_ids: HashSet<NodeId> = HashSet::new();
        for idx in active_unit_indices {
            if idx == selected_idx {
                continue;
            }
            let unit = &units[idx];
            for id in unit {
                losing_ids.insert(*id);
            }
        }

        for id in losing_ids {
            if !self.field.is_visible(id) {
                continue;
            }
            if self.field.node(id).is_some_and(|n| {
                n.kind == halley_core::field::NodeKind::Surface
                    && n.state == halley_core::field::NodeState::Active
            }) {
                let _ = self.field.set_decay_level(id, DecayLevel::Cold);
            }
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
            let _ = self.field.set_decay_level(id, DecayLevel::Hot);
            if let Some(nn) = self.field.node(id) {
                self.last_active_size.insert(id, nn.intrinsic_size);
            }
            self.mark_active_transition(id, now, 620);
            self.set_interaction_focus(Some(id), 30_000, now);
            self.push_neighbors_for_activation(id);
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

    pub(crate) fn enforce_pan_dominant_zone_states(&mut self, focus_ring: FocusRing, _now_ms: u64) {
        let ids: Vec<NodeId> = self.field.nodes().keys().copied().collect();

        for id in ids {
            if !self.field.is_visible(id) {
                continue;
            }
            let Some(n) = self.field.node(id) else {
                continue;
            };
            if n.kind != halley_core::field::NodeKind::Surface {
                continue;
            }

            let n_state = n.state.clone();
            let pos = n.pos;
            let fp_raw = self.collision_size_for_node(n);
            let fp = if n.state == halley_core::field::NodeState::Active {
                Vec2 { x: 64.0, y: 64.0 }
            } else {
                Vec2 {
                    x: fp_raw.x.max(48.0),
                    y: fp_raw.y.max(48.0),
                }
            };

            let samples = 7usize;
            let mut c_inside = 0usize;
            let mut c_total = 0usize;

            for ix in 0..samples {
                for iy in 0..samples {
                    let fx = (ix as f32 / (samples - 1) as f32) - 0.5;
                    let fy = (iy as f32 / (samples - 1) as f32) - 0.5;
                    let sp = Vec2 {
                        x: pos.x + fx * fp.x,
                        y: pos.y + fy * fp.y,
                    };
                    match focus_ring.zone(self.viewport.center, sp) {
                        FocusZone::Inside => c_inside += 1,
                        FocusZone::Outside => {}
                    }
                    c_total += 1;
                }
            }

            let p_inside = if c_total > 0 {
                c_inside as f32 / c_total as f32
            } else {
                0.0
            };
            let p_outside = (1.0 - p_inside).max(0.0);

            let was_active = n_state == halley_core::field::NodeState::Active;
            let pair_gap = self.non_overlap_gap_world();

            let overlap_active = self
                .field
                .nodes()
                .iter()
                .filter_map(|(&oid, on)| {
                    if oid == id
                        || !self.field.is_visible(oid)
                        || on.kind != halley_core::field::NodeKind::Surface
                    {
                        return None;
                    }
                    Some((oid, on))
                })
                .any(|(_, on)| {
                    let os = self.collision_size_for_node(on);
                    let req_x = fp_raw.x * 0.5 + os.x * 0.5 + pair_gap;
                    let req_y = fp_raw.y * 0.5 + os.y * 0.5 + pair_gap;
                    let dx = (on.pos.x - pos.x).abs();
                    let dy = (on.pos.y - pos.y).abs();
                    dx < req_x && dy < req_y
                });

            const ACTIVE_RETAIN_FRAC: f32 = 0.04;
            const ACTIVE_OVERLAP_RETAIN_FRAC: f32 = 0.22;
            const OUTSIDE_ENTER_FRAC: f32 = 0.90;

            let target = if was_active {
                let retain_frac = if overlap_active {
                    ACTIVE_OVERLAP_RETAIN_FRAC
                } else {
                    ACTIVE_RETAIN_FRAC
                };

                if p_inside >= retain_frac {
                    DecayLevel::Hot
                } else if p_outside >= OUTSIDE_ENTER_FRAC {
                    DecayLevel::Cold
                } else if overlap_active {
                    DecayLevel::Cold
                } else {
                    DecayLevel::Hot
                }
            } else {
                DecayLevel::Cold
            };

            let _ = self.field.set_decay_level(id, target);
        }
    }

    pub(crate) fn push_neighbors_for_activation(&mut self, activated: NodeId) {
        if !self.tuning.physics_enabled {
            return;
        }
        let now_ms = self.now_ms(Instant::now());
        let Some(a) = self.field.node(activated) else {
            return;
        };
        if !self.field.is_visible(activated) {
            return;
        }
        let apos = a.pos;
        let asize = self.collision_size_for_node(a);
        let pair_gap = self.non_overlap_gap_world();

        let mut others: Vec<(NodeId, Vec2, Vec2, f32)> = self
            .field
            .nodes()
            .iter()
            .filter_map(|(&id, n)| {
                if id == activated
                    || !self.field.is_visible(id)
                    || n.kind != halley_core::field::NodeKind::Surface
                    || n.pinned
                    || self.is_recently_resized_node(id, now_ms)
                {
                    return None;
                }
                let osize = self.collision_size_for_node(n);
                let d2 = (n.pos.x - apos.x).powi(2) + (n.pos.y - apos.y).powi(2);
                Some((id, n.pos, osize, d2))
            })
            .collect();

        others.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));

        let mut moved = 0usize;
        for (id, opos, osize, _) in others {
            if moved >= 3 {
                break;
            }
            let dx = opos.x - apos.x;
            let dy = opos.y - apos.y;
            let req_x = asize.x * 0.5 + osize.x * 0.5 + pair_gap;
            let req_y = asize.y * 0.5 + osize.y * 0.5 + pair_gap;
            let ox = req_x - dx.abs();
            let oy = req_y - dy.abs();
            if ox <= 0.0 || oy <= 0.0 {
                continue;
            }
            let target = if ox < oy {
                let s = if dx >= 0.0 { 1.0 } else { -1.0 };
                Vec2 {
                    x: apos.x + s * (req_x + 1.0),
                    y: opos.y,
                }
            } else {
                let s = if dy >= 0.0 { 1.0 } else { -1.0 };
                Vec2 {
                    x: opos.x,
                    y: apos.y + s * (req_y + 1.0),
                }
            };
            if self.field.carry(id, target) {
                moved += 1;
            }
        }
    }

    pub(crate) fn reconcile_surface_bindings(&mut self) {
        const STALE_SURFACE_GRACE_MS: u64 = 1500;
        let now = Instant::now();

        let alive: HashSet<ObjectId> = self
            .xdg_shell_state
            .toplevel_surfaces()
            .into_iter()
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
                self.clear_docking_for_node(id);
                self.zoom_nominal_size.remove(&id);
                self.zoom_resize_fallback.remove(&id);
                self.zoom_resize_reject_streak.remove(&id);
                self.zoom_last_observed_size.remove(&id);
                self.zoom_resize_static_streak.remove(&id);
                self.last_active_size.remove(&id);
                self.pending_spawn_activate_at_ms.remove(&id);
                self.active_transition_until_ms.remove(&id);
                self.primary_promote_cooldown_until_ms.remove(&id);
                self.last_surface_focus_ms.remove(&id);
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
