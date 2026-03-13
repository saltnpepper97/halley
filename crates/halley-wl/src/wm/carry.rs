use super::*;
use halley_core::docking::DockSide;
use halley_core::viewport::{FocusRing, FocusZone};

impl HalleyWlState {
    #[inline]
    pub(crate) fn mark_direct_carry_node(&mut self, id: NodeId) {
        self.carry_direct_nodes.insert(id);
        if let Some(pos) = self.field.node(id).map(|n| n.pos) {
            self.smoothed_render_pos.insert(id, pos);
        }
    }

    #[inline]
    pub(crate) fn clear_direct_carry_nodes(&mut self) {
        self.carry_direct_nodes.clear();
    }

    pub fn enforce_docked_pairs(&mut self) {
        let pairs = self.field.docked_pairs();
        if pairs.is_empty() {
            return;
        }

        let now_ms = self.now_ms(Instant::now());

        for (a, b) in pairs {
            if !self.field.is_visible(a) || !self.field.is_visible(b) {
                continue;
            }
            if self.is_recently_resized_node(a, now_ms) || self.is_recently_resized_node(b, now_ms)
            {
                continue;
            }

            let Some((_, link_b_side)) = self.field.dock_sides_for_pair(a, b) else {
                continue;
            };

            let (mid, sa, sb) = {
                let (Some(na), Some(nb)) = (self.field.node(a), self.field.node(b)) else {
                    continue;
                };
                (
                    Vec2 {
                        x: (na.pos.x + nb.pos.x) * 0.5,
                        y: (na.pos.y + nb.pos.y) * 0.5,
                    },
                    self.collision_extents_for_node(na),
                    self.collision_extents_for_node(nb),
                )
            };

            // Keep docked-pair geometry enforced without auto-resurrecting
            // already-collapsed members.
            if !self.preserve_collapsed_surface(a) {
                let _ = self.field.set_decay_level(a, DecayLevel::Hot);
            }
            if !self.preserve_collapsed_surface(b) {
                let _ = self.field.set_decay_level(b, DecayLevel::Hot);
            }

            if let Some(n) = self.field.node(a)
                && n.state == halley_core::field::NodeState::Active
            {
                self.last_active_size.insert(a, n.intrinsic_size);
            }
            if let Some(n) = self.field.node(b)
                && n.state == halley_core::field::NodeState::Active
            {
                self.last_active_size.insert(b, n.intrinsic_size);
            }

            let gap = self.non_overlap_gap_world();
            match link_b_side {
                DockSide::Left | DockSide::Right => {
                    let sep = if link_b_side == DockSide::Right {
                        (sa.right + sb.left + gap).max(0.0)
                    } else {
                        (sa.left + sb.right + gap).max(0.0)
                    };
                    let half_sep = sep * 0.5;
                    let (ax, bx) = if link_b_side == DockSide::Right {
                        (mid.x - half_sep, mid.x + half_sep)
                    } else {
                        (mid.x + half_sep, mid.x - half_sep)
                    };
                    let _ = self.field.carry(a, Vec2 { x: ax, y: mid.y });
                    let _ = self.field.carry(b, Vec2 { x: bx, y: mid.y });
                }
                DockSide::Top | DockSide::Bottom => {
                    let sep = if link_b_side == DockSide::Top {
                        (sa.top + sb.bottom + gap).max(0.0)
                    } else {
                        (sa.bottom + sb.top + gap).max(0.0)
                    };
                    let half_sep = sep * 0.5;
                    let (ay, by) = if link_b_side == DockSide::Top {
                        (mid.y - half_sep, mid.y + half_sep)
                    } else {
                        (mid.y + half_sep, mid.y - half_sep)
                    };
                    let _ = self.field.carry(a, Vec2 { x: mid.x, y: ay });
                    let _ = self.field.carry(b, Vec2 { x: mid.x, y: by });
                }
            }
        }
    }

    #[inline]
    fn zone_eval_footprint_for(&self, id: NodeId, fallback: Vec2) -> Vec2 {
        if self
            .field
            .node(id)
            .is_some_and(|n| n.state == halley_core::field::NodeState::Active)
        {
            Vec2 { x: 64.0, y: 64.0 }
        } else {
            fallback
        }
    }

    fn focus_ring_coverage_fractions(
        &self,
        pos: Vec2,
        footprint: Vec2,
        focus_ring: FocusRing,
    ) -> (f32, f32) {
        let sample_fp = Vec2 {
            x: footprint.x.max(48.0),
            y: footprint.y.max(48.0),
        };
        let samples = 7usize;
        let mut c_inside = 0usize;
        let mut c_total = 0usize;
        for ix in 0..samples {
            for iy in 0..samples {
                let fx = (ix as f32 / (samples - 1) as f32) - 0.5;
                let fy = (iy as f32 / (samples - 1) as f32) - 0.5;
                let sp = Vec2 {
                    x: pos.x + fx * sample_fp.x,
                    y: pos.y + fy * sample_fp.y,
                };
                match focus_ring.zone(self.viewport.center, sp) {
                    FocusZone::Inside => c_inside += 1,
                    FocusZone::Outside => {}
                }
                c_total += 1;
            }
        }
        if c_total == 0 {
            return (0.0, 1.0);
        }
        let p_inside = c_inside as f32 / c_total as f32;
        let p_outside = (1.0 - p_inside).max(0.0);
        (p_inside, p_outside)
    }

    fn zone_for_pos_with_hysteresis(
        &mut self,
        id: NodeId,
        pos: Vec2,
        footprint: Vec2,
    ) -> FocusZone {
        let focus_ring = self.active_focus_ring();
        let footprint = self.zone_eval_footprint_for(id, footprint);
        let (p_inside, p_outside) = self.focus_ring_coverage_fractions(pos, footprint, focus_ring);
        let prev = self.carry_zone_hint.get(&id).copied();

        const ACTIVE_RETAIN_FRAC: f32 = 0.04;
        const ACTIVE_ENTER_FRAC: f32 = 0.10;
        const OUTSIDE_ENTER_FRAC: f32 = 0.90;

        let zone = match prev {
            Some(FocusZone::Inside) => {
                if p_inside >= ACTIVE_RETAIN_FRAC {
                    FocusZone::Inside
                } else if p_outside >= OUTSIDE_ENTER_FRAC {
                    FocusZone::Outside
                } else {
                    FocusZone::Inside
                }
            }
            _ => {
                if p_inside >= ACTIVE_ENTER_FRAC {
                    FocusZone::Inside
                } else {
                    FocusZone::Outside
                }
            }
        };

        let now_ms = self.now_ms(Instant::now());
        self.carry_zone_last_change_ms.insert(id, now_ms);
        self.carry_zone_pending.remove(&id);
        self.carry_zone_pending_since_ms.remove(&id);
        self.carry_zone_hint.insert(id, zone);
        zone
    }

    pub fn finalize_mouse_drag_state(&mut self, id: NodeId, _pointer_world: Vec2, _now: Instant) {
        let Some(n) = self.field.node(id) else {
            return;
        };
        if n.kind != halley_core::field::NodeKind::Surface || !self.field.is_visible(id) {}
    }

    pub fn begin_carry_state_tracking(&mut self, id: NodeId, _docking_mode: bool) {
        self.clear_direct_carry_nodes();
        self.mark_direct_carry_node(id);
        if self.resize_static_node == Some(id) {
            self.resize_static_node = None;
            self.resize_static_lock_pos = None;
            self.resize_static_until_ms = 0;
        }
        self.suspend_overlap_resolve = true;
        self.suspend_state_checks = true;
        self.immediate_physics_nodes.clear();
        self.physics_velocity.remove(&id);
        self.release_smoothing_until_ms.remove(&id);
        self.release_axis_lock.remove(&id);
        let _ = self.field.undock_node(id);
        self.field.clear_dock_preview();
        let _ = self.field.set_pinned(id, false);

        if let Some(n) = self.field.node(id) {
            let fp = self.collision_size_for_node(n);
            let z = self.zone_for_pos_with_hysteresis(id, n.pos, fp);
            self.carry_zone_hint.insert(id, z);
            self.carry_zone_last_change_ms
                .insert(id, self.now_ms(Instant::now()));
            self.carry_zone_pending.remove(&id);
            self.carry_zone_pending_since_ms.remove(&id);
            self.carry_activation_anim_armed.insert(id);
        }
    }

    pub fn end_carry_state_tracking(&mut self, id: NodeId) {
        self.mark_direct_carry_node(id);
        self.carry_zone_hint.remove(&id);
        self.carry_zone_last_change_ms.remove(&id);
        self.carry_zone_pending.remove(&id);
        self.carry_zone_pending_since_ms.remove(&id);
        self.carry_activation_anim_armed.remove(&id);
        self.field.clear_dock_preview();
        self.suspend_overlap_resolve = false;
        self.suspend_state_checks = false;
        let immediate_nodes: Vec<NodeId> = self.immediate_physics_nodes.drain().collect();
        for node_id in immediate_nodes {
            self.release_smoothing_until_ms.remove(&node_id);
            self.release_axis_lock.remove(&node_id);
            self.smoothed_render_pos.remove(&node_id);
            self.smoothed_render_vel.remove(&node_id);
        }
        self.enforce_docked_pairs();
    }

    pub fn update_carry_state_preview(&mut self, id: NodeId, now: Instant) {
        let Some(n) = self.field.node(id) else {
            return;
        };
        self.update_carry_state_preview_at(id, n.pos, now);
    }

    pub fn update_carry_state_preview_at(&mut self, id: NodeId, source_pos: Vec2, now: Instant) {
        let Some(n) = self.field.node(id) else {
            return;
        };
        let n_kind = n.kind.clone();
        let was_active = n.state == halley_core::field::NodeState::Active;
        let footprint = self.zone_eval_footprint_for(id, self.collision_size_for_node(n));
        if n_kind != halley_core::field::NodeKind::Surface || !self.field.is_visible(id) {
            return;
        }
        let zone = self.zone_for_pos_with_hysteresis(id, source_pos, footprint);
        let target = match zone {
            FocusZone::Inside if was_active => DecayLevel::Hot,
            _ => DecayLevel::Cold,
        };
        let _ = self.field.set_decay_level(id, target);
        let is_active = self
            .field
            .node(id)
            .is_some_and(|nn| nn.state == halley_core::field::NodeState::Active);
        if is_active {
            if let Some(nn) = self.field.node(id) {
                self.last_active_size.insert(id, nn.intrinsic_size);
            }
            if !was_active
                && self.active_transition_alpha(id, now) <= 0.01
                && self.carry_activation_anim_armed.remove(&id)
            {
                self.mark_active_transition(id, now, 360);
                self.push_neighbors_for_activation(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carry_release_preserves_immediate_physics_velocity() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        let dragged =
            state
                .field
                .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let affected =
            state
                .field
                .spawn_surface("b", Vec2 { x: 430.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });

        state.begin_carry_state_tracking(dragged);
        state.immediate_physics_nodes.insert(affected);
        state
            .physics_velocity
            .insert(affected, Vec2 { x: 180.0, y: -24.0 });

        state.end_carry_state_tracking(dragged);

        let velocity = state.physics_velocity.get(&affected).copied();
        assert_eq!(velocity, Some(Vec2 { x: 180.0, y: -24.0 }));
    }
}
