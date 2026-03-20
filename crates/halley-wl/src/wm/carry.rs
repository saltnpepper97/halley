use super::*;
use halley_core::viewport::{FocusRing, FocusZone};

impl HalleyWlState {
    #[inline]
    pub(crate) fn mark_direct_carry_node(&mut self, id: NodeId) {
        self.carry_direct_nodes.insert(id);
    }

    #[inline]
    pub(crate) fn clear_direct_carry_nodes(&mut self) {
        self.carry_direct_nodes.clear();
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
        self.suspend_overlap_resolve = false;
        self.suspend_state_checks = false;
        let _ = self.field.set_pinned(id, false);

        if let Some(n) = self.field.node(id) {
            self.carry_state_hold.insert(id, n.state.clone());
            let fp = self.collision_size_for_node(n);
            let z = self.zone_for_pos_with_hysteresis(id, n.pos, fp);
            self.carry_zone_hint.insert(id, z);
            self.carry_zone_last_change_ms
                .insert(id, self.now_ms(Instant::now()));
            self.carry_zone_pending.remove(&id);
            self.carry_zone_pending_since_ms.remove(&id);
            self.carry_activation_anim_armed.insert(id);
        }
        self.request_maintenance();
    }

    pub fn end_carry_state_tracking(&mut self, id: NodeId) {
        self.mark_direct_carry_node(id);
        self.carry_zone_hint.remove(&id);
        self.carry_zone_last_change_ms.remove(&id);
        self.carry_zone_pending.remove(&id);
        self.carry_zone_pending_since_ms.remove(&id);
        self.carry_activation_anim_armed.remove(&id);
        self.carry_state_hold.remove(&id);
        self.dock_decay_offscreen_since_ms.remove(&id);
        self.suspend_overlap_resolve = false;
        self.suspend_state_checks = false;
        self.clear_direct_carry_nodes();
        self.request_maintenance();
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
        let held_state = self.carry_state_hold.get(&id);
        let target = match held_state {
            Some(halley_core::field::NodeState::Active) => DecayLevel::Hot,
            Some(halley_core::field::NodeState::Node | halley_core::field::NodeState::Core) => {
                DecayLevel::Cold
            }
            _ => match zone {
                FocusZone::Inside if was_active => DecayLevel::Hot,
                _ => DecayLevel::Cold,
            },
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
            }
        }
        self.request_maintenance();
    }
}
