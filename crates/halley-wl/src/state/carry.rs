use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DockSide {
    Left,
    Right,
    Top,
    Bottom,
}

impl DockSide {
    fn opposite(self) -> Self {
        match self {
            DockSide::Left => DockSide::Right,
            DockSide::Right => DockSide::Left,
            DockSide::Top => DockSide::Bottom,
            DockSide::Bottom => DockSide::Top,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct DockLink {
    pub(super) partner: NodeId,
    pub(super) side: DockSide,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct DockPending {
    pub(super) mover: NodeId,
    pub(super) target: NodeId,
    pub(super) side: DockSide,
    pub(super) snap_pos: Vec2,
    pub(super) since_ms: u64,
}

impl HalleyWlState {
    const DOCK_SNAP_DIST: f32 = 84.0;
    const DOCK_DWELL_MS: u64 = 360;

    fn dock_partner(&self, id: NodeId) -> Option<NodeId> {
        self.docked_links.get(&id).map(|l| l.partner)
    }

    fn dock_link(&self, id: NodeId) -> Option<DockLink> {
        self.docked_links.get(&id).copied()
    }

    pub(crate) fn dock_sides_for_pair(&self, a: NodeId, b: NodeId) -> Option<(DockSide, DockSide)> {
        let a_link = self.docked_links.get(&a).copied()?;
        let b_link = self.docked_links.get(&b).copied()?;
        if a_link.partner == b && b_link.partner == a {
            Some((a_link.side, b_link.side))
        } else {
            None
        }
    }

    fn dock_side_snap_pos(&self, mover: NodeId, target: NodeId, side: DockSide) -> Option<Vec2> {
        let mover_n = self.field.node(mover)?;
        let target_n = self.field.node(target)?;
        let mover_size = self.collision_size_for_node(mover_n);
        let target_size = self.collision_size_for_node(target_n);
        let gap = self.non_overlap_gap_world();
        let half_x = mover_size.x * 0.5 + target_size.x * 0.5 + gap;
        let half_y = mover_size.y * 0.5 + target_size.y * 0.5 + gap;
        let p = match side {
            DockSide::Left => Vec2 {
                x: target_n.pos.x - half_x,
                y: target_n.pos.y,
            },
            DockSide::Right => Vec2 {
                x: target_n.pos.x + half_x,
                y: target_n.pos.y,
            },
            DockSide::Top => Vec2 {
                x: target_n.pos.x,
                y: target_n.pos.y + half_y,
            },
            DockSide::Bottom => Vec2 {
                x: target_n.pos.x,
                y: target_n.pos.y - half_y,
            },
        };
        Some(p)
    }

    fn best_dock_candidate(&self, mover: NodeId) -> Option<(NodeId, DockSide, Vec2, f32)> {
        let mover_n = self.field.node(mover)?;
        if mover_n.kind != halley_core::field::NodeKind::Surface || !self.field.is_visible(mover) {
            return None;
        }
        if self.dock_partner(mover).is_some() {
            return None;
        }
        let mover_pos = mover_n.pos;
        let mut best: Option<(NodeId, DockSide, Vec2, f32)> = None;
        for (&id, n) in self.field.nodes() {
            if id == mover || !self.field.is_visible(id) {
                continue;
            }
            if n.kind != halley_core::field::NodeKind::Surface {
                continue;
            }
            if self.dock_partner(id).is_some() {
                continue;
            }
            for side in [
                DockSide::Left,
                DockSide::Right,
                DockSide::Top,
                DockSide::Bottom,
            ] {
                let Some(snap_pos) = self.dock_side_snap_pos(mover, id, side) else {
                    continue;
                };
                let dx = mover_pos.x - snap_pos.x;
                let dy = mover_pos.y - snap_pos.y;
                let d = (dx * dx + dy * dy).sqrt();
                if best.is_none_or(|(_, _, _, bd)| d < bd) {
                    best = Some((id, side, snap_pos, d));
                }
            }
        }
        best
    }

    pub fn update_dock_preview(&mut self, mover: NodeId, now: Instant) {
        let now_ms = self.now_ms(now);
        let next = self
            .best_dock_candidate(mover)
            .and_then(|(target, side, snap_pos, dist)| {
                (dist <= Self::DOCK_SNAP_DIST).then_some(DockPending {
                    mover,
                    target,
                    side,
                    snap_pos,
                    since_ms: now_ms,
                })
            });
        match (self.dock_pending, next) {
            (Some(cur), Some(mut n))
                if cur.mover == n.mover && cur.target == n.target && cur.side == n.side =>
            {
                n.since_ms = cur.since_ms;
                self.dock_pending = Some(n);
            }
            (_, n) => {
                self.dock_pending = n;
            }
        }
    }

    pub(crate) fn dock_preview(
        &self,
        now: Instant,
    ) -> Option<(NodeId, NodeId, DockSide, Vec2, bool)> {
        let p = self.dock_pending?;
        let armed = self.now_ms(now).saturating_sub(p.since_ms) >= Self::DOCK_DWELL_MS;
        Some((p.mover, p.target, p.side, p.snap_pos, armed))
    }

    fn insert_dock_pair(&mut self, a: NodeId, b: NodeId) {
        let side_a = self.dock_pending.map(|p| p.side).unwrap_or(DockSide::Left);
        let side_b = side_a.opposite();
        self.docked_links.insert(
            a,
            DockLink {
                partner: b,
                side: side_a,
            },
        );
        self.docked_links.insert(
            b,
            DockLink {
                partner: a,
                side: side_b,
            },
        );
    }

    pub fn clear_docking_for_node(&mut self, id: NodeId) {
        if let Some(link) = self.docked_links.remove(&id) {
            self.docked_links.remove(&link.partner);
        }
        if self
            .dock_pending
            .is_some_and(|p| p.mover == id || p.target == id)
        {
            self.dock_pending = None;
        }
    }

    fn undock_for_drag(&mut self, id: NodeId) {
        if let Some(partner) = self.dock_partner(id) {
            self.docked_links.remove(&id);
            self.docked_links.remove(&partner);
        }
        if self
            .dock_pending
            .is_some_and(|p| p.mover == id || p.target == id)
        {
            self.dock_pending = None;
        }
    }

    pub fn finalize_dock_on_drag_release(&mut self, mover: NodeId, now: Instant) -> bool {
        let now_ms = self.now_ms(now);
        let Some(pending) = self.dock_pending else {
            return false;
        };
        if pending.mover != mover || now_ms.saturating_sub(pending.since_ms) < Self::DOCK_DWELL_MS {
            return false;
        }
        let mover_ok = self.field.node(pending.mover).is_some_and(|n| {
            n.kind == halley_core::field::NodeKind::Surface && self.field.is_visible(pending.mover)
        });
        let target_ok = self.field.node(pending.target).is_some_and(|n| {
            n.kind == halley_core::field::NodeKind::Surface && self.field.is_visible(pending.target)
        });
        if !mover_ok || !target_ok {
            self.dock_pending = None;
            return false;
        }
        if self.dock_partner(pending.mover).is_some() || self.dock_partner(pending.target).is_some()
        {
            self.dock_pending = None;
            return false;
        }
        let either_active = self
            .field
            .node(pending.mover)
            .is_some_and(|n| n.state == halley_core::field::NodeState::Active)
            || self
                .field
                .node(pending.target)
                .is_some_and(|n| n.state == halley_core::field::NodeState::Active);
        let _ = self.field.carry(pending.mover, pending.snap_pos);
        self.insert_dock_pair(pending.mover, pending.target);
        if either_active {
            let _ = self.field.set_decay_level(pending.mover, DecayLevel::Hot);
            let _ = self.field.set_decay_level(pending.target, DecayLevel::Hot);
            self.mark_active_transition(pending.mover, now, 280);
            self.mark_active_transition(pending.target, now, 280);
            if let Some(n) = self.field.node(pending.mover) {
                self.last_active_size
                    .insert(pending.mover, n.intrinsic_size);
            }
            if let Some(n) = self.field.node(pending.target) {
                self.last_active_size
                    .insert(pending.target, n.intrinsic_size);
            }
        }
        self.set_interaction_focus(Some(pending.target), 700, now);
        self.dock_pending = None;
        true
    }

    pub fn docked_pairs(&self) -> Vec<(NodeId, NodeId)> {
        self.docked_links
            .iter()
            .filter_map(|(&id, link)| {
                (id.as_u64() < link.partner.as_u64()).then_some((id, link.partner))
            })
            .collect()
    }

    #[inline]
    fn dock_node_target_decay(
        &self,
        p_primary: f32,
        p_outside: f32,
        was_active: bool,
    ) -> DecayLevel {
        // Docked edge behavior:
        // - if a node is clearly offscreen, allow it to collapse to Node
        // - if it comes back on-screen enough, reopen to Active
        // This avoids forcing both sides open while panning at edges.
        const ACTIVE_RETAIN_FRAC: f32 = 0.04;
        const ACTIVE_ENTER_FRAC: f32 = 0.10;
        const OUTSIDE_ENTER_FRAC: f32 = 0.90;
        if was_active {
            if p_primary >= ACTIVE_RETAIN_FRAC || p_outside < OUTSIDE_ENTER_FRAC {
                DecayLevel::Hot
            } else {
                DecayLevel::Cold
            }
        } else if p_primary >= ACTIVE_ENTER_FRAC {
            DecayLevel::Hot
        } else {
            DecayLevel::Cold
        }
    }

    #[inline]
    fn dock_outward_edge_overflow(
        &self,
        pos: Vec2,
        footprint: Vec2,
        outward_side: DockSide,
    ) -> f32 {
        let vp = self.viewport.rect();
        let half = Vec2 {
            x: footprint.x * 0.5,
            y: footprint.y * 0.5,
        };
        let min_x = pos.x - half.x;
        let max_x = pos.x + half.x;
        let min_y = pos.y - half.y;
        let max_y = pos.y + half.y;
        match outward_side {
            DockSide::Left => vp.min.x - min_x,
            DockSide::Right => max_x - vp.max.x,
            DockSide::Top => max_y - vp.max.y,
            DockSide::Bottom => vp.min.y - min_y,
        }
    }

    #[inline]
    fn dock_state_eval_footprint(&self, id: NodeId, live: Vec2) -> Vec2 {
        match self.last_active_size.get(&id).copied() {
            Some(last) => Vec2 {
                x: live.x.max(last.x),
                y: live.y.max(last.y),
            },
            None => live,
        }
    }

    pub fn enforce_docked_pairs(&mut self) {
        if self.docked_links.is_empty() {
            return;
        }
        let now_ms = self.now_ms(Instant::now());
        let rings = self.active_rings();
        let pairs = self.docked_pairs();
        for (a, b) in pairs {
            if !self.field.is_visible(a) || !self.field.is_visible(b) {
                continue;
            }
            if self.is_recently_resized_node(a, now_ms) || self.is_recently_resized_node(b, now_ms)
            {
                // Recently resized node(s): do not apply dock snap correction yet.
                continue;
            }
            let (Some(link_a), Some(link_b)) = (self.dock_link(a), self.dock_link(b)) else {
                continue;
            };
            if link_a.partner != b || link_b.partner != a {
                continue;
            }
            let (mid, sa, sb, a_state, b_state, a_pos, b_pos) = {
                let (Some(na), Some(nb)) = (self.field.node(a), self.field.node(b)) else {
                    continue;
                };
                (
                    Vec2 {
                        x: (na.pos.x + nb.pos.x) * 0.5,
                        y: (na.pos.y + nb.pos.y) * 0.5,
                    },
                    self.collision_size_for_node(na),
                    self.collision_size_for_node(nb),
                    na.state.clone(),
                    nb.state.clone(),
                    na.pos,
                    nb.pos,
                )
            };
            let (a_primary, _a_secondary, a_outside) =
                self.ring_coverage_fractions(a_pos, sa, rings);
            let (b_primary, _b_secondary, b_outside) =
                self.ring_coverage_fractions(b_pos, sb, rings);
            let mut decay_a = self.dock_node_target_decay(
                a_primary,
                a_outside,
                a_state == halley_core::field::NodeState::Active,
            );
            let mut decay_b = self.dock_node_target_decay(
                b_primary,
                b_outside,
                b_state == halley_core::field::NodeState::Active,
            );
            // Docked edge rule:
            // - Collapse side immediately when its outward edge crosses viewport boundary.
            // - Reopen only when that same outward edge is back inside the viewport.
            let a_outward_overflow = self.dock_outward_edge_overflow(a_pos, sa, link_a.side);
            let b_outward_overflow = self.dock_outward_edge_overflow(b_pos, sb, link_b.side);
            let a_outward_outside = a_outward_overflow > 0.0;
            let b_outward_outside = b_outward_overflow > 0.0;
            if a_outward_outside || b_outward_outside {
                if a_outward_outside {
                    decay_a = DecayLevel::Cold;
                }
                if b_outward_outside {
                    decay_b = DecayLevel::Cold;
                }
                if a_outward_outside && !b_outward_outside {
                    decay_b = DecayLevel::Hot;
                } else if b_outward_outside && !a_outward_outside {
                    decay_a = DecayLevel::Hot;
                }
            } else {
                let either_hot = matches!(decay_a, DecayLevel::Hot)
                    || matches!(decay_b, DecayLevel::Hot)
                    || a_state == halley_core::field::NodeState::Active
                    || b_state == halley_core::field::NodeState::Active;
                let coupled = if either_hot {
                    DecayLevel::Hot
                } else {
                    DecayLevel::Cold
                };
                decay_a = coupled;
                decay_b = coupled;
            }

            let _ = self.field.set_decay_level(a, decay_a);
            let _ = self.field.set_decay_level(b, decay_b);
            if decay_a == DecayLevel::Hot {
                if let Some(n) = self.field.node(a) {
                    self.last_active_size.insert(a, n.intrinsic_size);
                }
            }
            if decay_b == DecayLevel::Hot {
                if let Some(n) = self.field.node(b) {
                    self.last_active_size.insert(b, n.intrinsic_size);
                }
            }
            // Keep docked pairs centered on their shared anchor in both minimized
            // and reopened states, while preserving orientation and non-overlap.
            // This prevents midpoint drift when footprints change during reopen.
            let gap = self.non_overlap_gap_world();
            match link_b.side {
                DockSide::Left | DockSide::Right => {
                    let sep = (sa.x * 0.5 + sb.x * 0.5 + gap).max(0.0);
                    let half_sep = sep * 0.5;
                    let (ax, bx) = if link_b.side == DockSide::Right {
                        (mid.x - half_sep, mid.x + half_sep)
                    } else {
                        (mid.x + half_sep, mid.x - half_sep)
                    };
                    let _ = self.field.carry(a, Vec2 { x: ax, y: mid.y });
                    let _ = self.field.carry(b, Vec2 { x: bx, y: mid.y });
                }
                DockSide::Top | DockSide::Bottom => {
                    let sep = (sa.y * 0.5 + sb.y * 0.5 + gap).max(0.0);
                    let half_sep = sep * 0.5;
                    let (ay, by) = if link_b.side == DockSide::Top {
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
        // When Active, state transitions should be driven by pointer/zone intent,
        // not by large window footprint overlap. Use a compact proxy footprint
        // until the node is noded again.
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

    fn ring_coverage_fractions(
        &self,
        pos: Vec2,
        footprint: Vec2,
        rings: FocusRings,
    ) -> (f32, f32, f32) {
        let sample_fp = Vec2 {
            x: footprint.x.max(48.0),
            y: footprint.y.max(48.0),
        };
        let samples = 7usize;
        let mut c_primary = 0usize;
        let mut c_secondary = 0usize;
        let mut c_total = 0usize;
        for ix in 0..samples {
            for iy in 0..samples {
                let fx = (ix as f32 / (samples - 1) as f32) - 0.5;
                let fy = (iy as f32 / (samples - 1) as f32) - 0.5;
                let sp = Vec2 {
                    x: pos.x + fx * sample_fp.x,
                    y: pos.y + fy * sample_fp.y,
                };
                match rings.zone(self.viewport.center, sp) {
                    RingZone::Primary => c_primary += 1,
                    RingZone::Secondary => c_secondary += 1,
                    RingZone::Outside => {}
                }
                c_total += 1;
            }
        }
        if c_total == 0 {
            return (0.0, 0.0, 1.0);
        }
        let p_primary = c_primary as f32 / c_total as f32;
        let p_secondary = c_secondary as f32 / c_total as f32;
        let p_outside = (1.0 - p_primary - p_secondary).max(0.0);
        (p_primary, p_secondary, p_outside)
    }

    fn zone_for_pos_with_hysteresis(&mut self, id: NodeId, pos: Vec2, footprint: Vec2) -> RingZone {
        let rings = self.active_rings();
        let footprint = self.zone_eval_footprint_for(id, footprint);
        let (p_primary, _p_secondary, p_outside) =
            self.ring_coverage_fractions(pos, footprint, rings);
        let prev = self.carry_zone_hint.get(&id).copied();

        // Two-state carry rule with strong hysteresis to prevent edge thrash:
        // - Active is sticky: retain at very low primary overlap
        // - collapse only when clearly outside
        // - Node -> Active needs higher overlap to re-enter
        const ACTIVE_RETAIN_FRAC: f32 = 0.04;
        const ACTIVE_ENTER_FRAC: f32 = 0.10;
        const OUTSIDE_ENTER_FRAC: f32 = 0.90;
        let direct = match prev {
            Some(RingZone::Primary) => {
                if p_primary >= ACTIVE_RETAIN_FRAC {
                    RingZone::Primary
                } else if p_outside >= OUTSIDE_ENTER_FRAC {
                    RingZone::Outside
                } else {
                    RingZone::Primary
                }
            }
            Some(RingZone::Secondary) => {
                if p_primary >= ACTIVE_ENTER_FRAC {
                    RingZone::Primary
                } else {
                    RingZone::Outside
                }
            }
            Some(RingZone::Outside) => {
                if p_primary >= ACTIVE_ENTER_FRAC {
                    RingZone::Primary
                } else {
                    RingZone::Outside
                }
            }
            _ => {
                if p_primary >= ACTIVE_ENTER_FRAC {
                    RingZone::Primary
                } else {
                    RingZone::Outside
                }
            }
        };
        let zone = direct;
        let now_ms = self.now_ms(Instant::now());
        self.carry_zone_last_change_ms.insert(id, now_ms);
        self.carry_zone_pending.remove(&id);
        self.carry_zone_pending_since_ms.remove(&id);

        self.carry_zone_hint.insert(id, zone);
        zone
    }

    pub fn finalize_mouse_drag_state(&mut self, id: NodeId, pointer_world: Vec2, _now: Instant) {
        let Some(n) = self.field.node(id) else {
            return;
        };
        if n.kind != halley_core::field::NodeKind::Surface || !self.field.is_visible(id) {
            return;
        }
        let rings = self.active_rings();
        let pointer_zone = rings.zone(self.viewport.center, pointer_world);
        let target = if pointer_zone != RingZone::Primary {
            DecayLevel::Cold
        } else {
            DecayLevel::Hot
        };
        // Keep keyboard focus sticky across drag/finalize operations.
        let _ = self.field.set_decay_level(id, target);
    }

    pub fn begin_carry_state_tracking(&mut self, id: NodeId) {
        // If user starts dragging immediately after resize, cancel the short
        // post-resize lock/cooldown so drag is not fighting a stale lock target.
        if self.resize_static_node == Some(id) {
            self.resize_static_node = None;
            self.resize_static_lock_pos = None;
            self.resize_static_until_ms = 0;
        }
        self.suspend_overlap_resolve = true;
        self.suspend_state_checks = true;
        self.undock_for_drag(id);
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
        self.carry_zone_hint.remove(&id);
        self.carry_zone_last_change_ms.remove(&id);
        self.carry_zone_pending.remove(&id);
        self.carry_zone_pending_since_ms.remove(&id);
        self.carry_activation_anim_armed.remove(&id);
        if self
            .dock_pending
            .is_some_and(|p| p.mover == id || p.target == id)
        {
            self.dock_pending = None;
        }
        self.suspend_overlap_resolve = false;
        self.suspend_state_checks = false;
        self.enforce_docked_pairs();
        self.resolve_overlap_now();
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
            // Dragging a Node into center should not auto-promote it to Active.
            RingZone::Primary if was_active => DecayLevel::Hot,
            RingZone::Primary => DecayLevel::Cold,
            RingZone::Secondary => DecayLevel::Cold,
            RingZone::Outside => DecayLevel::Cold,
        };
        // Keep keyboard focus sticky across drag/zone transitions.
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
