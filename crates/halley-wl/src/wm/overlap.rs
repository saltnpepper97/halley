use super::*;
use std::collections::VecDeque;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
#[derive(Clone, Copy, Debug)]
pub(crate) struct CollisionExtents {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}
impl CollisionExtents {
    #[inline]
    pub(crate) fn symmetric(size: Vec2) -> Self {
        Self {
            left: size.x * 0.5,
            right: size.x * 0.5,
            top: size.y * 0.5,
            bottom: size.y * 0.5,
        }
    }
    #[inline]
    fn size(self) -> Vec2 {
        Vec2 {
            x: (self.left + self.right).max(0.0),
            y: (self.top + self.bottom).max(0.0),
        }
    }
}
impl HalleyWlState {
    const PHYSICS_DEPTH_LIMIT: usize = 6;
    const DRAG_AXIS_DOMINANCE: f32 = 1.35;
    const PHYSICS_MIN_SPEED: f32 = 12.0;
    const NODE_BODY_MASS: f32 = 1.3;
    const WINDOW_BODY_MASS: f32 = 1.0;
    const PHYSICS_MIN_IMPULSE_LIMIT: f32 = 320.0;
    const PHYSICS_MAX_IMPULSE_LIMIT: f32 = 2200.0;
    const PHYSICS_MIN_BOUNCE_LIMIT: f32 = 420.0;
    const PHYSICS_MAX_BOUNCE_LIMIT: f32 = 2600.0;
    #[inline]
    fn bump_response(&self) -> f32 {
        self.tuning.non_overlap_bump_damping.clamp(0.05, 1.0)
    }
    #[inline]
    fn physics_damping_per_sec(&self) -> f32 {
        1.8 + self.bump_response() * 4.2
    }
    #[inline]
    fn drag_impulse_gain(&self) -> f32 {
        2.6 + self.bump_response() * 2.4
    }
    #[inline]
    fn collision_push_gain(&self) -> f32 {
        5.5 + self.bump_response() * 7.5
    }
    #[inline]
    fn collision_bounce(&self) -> f32 {
        0.05 + self.bump_response() * 0.17
    }
    #[inline]
    pub(crate) fn carry_for_physics(&mut self, id: NodeId, to: Vec2) -> bool {
        if self.resize_static_node == Some(id) {
            return false;
        }
        if !self.tuning.physics_enabled {
            return self.field.carry(id, to);
        }
        let immediate_physics = self.suspend_overlap_resolve && self.suspend_state_checks;
        let should_prime_render_smoothing = !self.carry_zone_hint.contains_key(&id)
            && !immediate_physics
            && self.resize_active != Some(id)
            && self.resize_static_node != Some(id);
        let Some(n) = self.field.node_mut(id) else {
            return false;
        };
        let from = n.pos;
        if should_prime_render_smoothing
            && ((to.x - from.x).abs() > 0.05 || (to.y - from.y).abs() > 0.05)
        {
            self.smoothed_render_pos.entry(id).or_insert(from);
            self.smoothed_render_vel
                .entry(id)
                .or_insert(Vec2 { x: 0.0, y: 0.0 });
        }
        n.pos = to;
        if immediate_physics {
            self.immediate_physics_nodes.insert(id);
            self.smoothed_render_pos.remove(&id);
            self.smoothed_render_vel.remove(&id);
        }
        true
    }
    #[inline]
    fn world_units_per_px_xy(&self) -> (f32, f32) {
        let wx = self.viewport.size.x / self.zoom_ref_size.x.max(1.0);
        let wy = self.viewport.size.y / self.zoom_ref_size.y.max(1.0);
        (wx.max(0.01), wy.max(0.01))
    }
    pub(crate) fn non_overlap_gap_world(&self) -> f32 {
        let (wx, wy) = self.world_units_per_px_xy();
        self.tuning.non_overlap_gap_px.max(0.0) * ((wx + wy) * 0.5)
    }
    #[inline]
    pub(crate) fn required_sep_x(
        &self,
        a_pos_x: f32,
        a_ext: CollisionExtents,
        b_pos_x: f32,
        b_ext: CollisionExtents,
        gap: f32,
    ) -> f32 {
        if b_pos_x >= a_pos_x {
            a_ext.right + b_ext.left + gap
        } else {
            a_ext.left + b_ext.right + gap
        }
    }
    #[inline]
    pub(crate) fn required_sep_y(
        &self,
        a_pos_y: f32,
        a_ext: CollisionExtents,
        b_pos_y: f32,
        b_ext: CollisionExtents,
        gap: f32,
    ) -> f32 {
        if b_pos_y >= a_pos_y {
            a_ext.bottom + b_ext.top + gap
        } else {
            a_ext.top + b_ext.bottom + gap
        }
    }
    #[inline]
    pub(crate) fn update_release_axis_from_motion(&mut self, id: NodeId, motion: Vec2) {
        if motion.x.abs() > motion.y.abs() * Self::DRAG_AXIS_DOMINANCE {
            self.release_axis_lock.insert(id, true);
        } else if motion.y.abs() > motion.x.abs() * Self::DRAG_AXIS_DOMINANCE {
            self.release_axis_lock.insert(id, false);
        }
    }
    #[inline]
    pub(crate) fn release_smoothing_active_for(&self, id: NodeId, now_ms: u64) -> bool {
        self.release_smoothing_until_ms
            .get(&id)
            .is_some_and(|&until| until > now_ms)
    }
    #[inline]
    fn collision_axis(motion: Vec2, overlap_x: f32, overlap_y: f32) -> bool {
        if motion.x.abs() > motion.y.abs() * Self::DRAG_AXIS_DOMINANCE {
            true
        } else if motion.y.abs() > motion.x.abs() * Self::DRAG_AXIS_DOMINANCE {
            false
        } else {
            overlap_x <= overlap_y
        }
    }
    #[inline]
    fn push_direction(delta: f32, motion: f32) -> f32 {
        if motion.abs() > 0.01 {
            motion.signum()
        } else if delta >= 0.0 {
            1.0
        } else {
            -1.0
        }
    }
    #[inline]
    fn body_locked(&self, id: NodeId) -> bool {
        self.carry_zone_hint.contains_key(&id)
            || self.resize_active == Some(id)
            || self.resize_static_node == Some(id)
    }
    #[inline]
    fn add_physics_velocity(&mut self, id: NodeId, delta: Vec2) {
        let inv_mass = self.body_inverse_mass(id);
        let limit = self
            .field
            .node(id)
            .map(|node| {
                let ext = self.collision_extents_for_node(node);
                self.collision_velocity_limit(ext, ext, true)
            })
            .unwrap_or(Self::PHYSICS_MAX_BOUNCE_LIMIT);
        let vel = self
            .physics_velocity
            .entry(id)
            .or_insert(Vec2 { x: 0.0, y: 0.0 });
        vel.x = (vel.x + delta.x * inv_mass).clamp(-limit, limit);
        vel.y = (vel.y + delta.y * inv_mass).clamp(-limit, limit);
    }
    #[inline]
    fn body_mass_for_node(&self, node: &halley_core::field::Node) -> f32 {
        match node.state {
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core => {
                Self::NODE_BODY_MASS
            }
            halley_core::field::NodeState::Active | halley_core::field::NodeState::Drifting => {
                Self::WINDOW_BODY_MASS
            }
        }
    }
    #[inline]
    fn body_inverse_mass(&self, id: NodeId) -> f32 {
        self.field
            .node(id)
            .map(|node| 1.0 / self.body_mass_for_node(node).max(0.001))
            .unwrap_or(1.0)
    }
    #[inline]
    fn collision_velocity_limit(
        &self,
        a_ext: CollisionExtents,
        b_ext: CollisionExtents,
        bounce: bool,
    ) -> f32 {
        let a_size = a_ext.size();
        let b_size = b_ext.size();
        let scale = a_size
            .x
            .max(a_size.y)
            .max(b_size.x.max(b_size.y))
            .max(self.non_overlap_gap_world() * 10.0);
        let response_scale = 0.85 + self.bump_response() * 0.9;
        let raw = if bounce { scale * 5.0 } else { scale * 4.0 } * response_scale;
        if bounce {
            raw.clamp(
                Self::PHYSICS_MIN_BOUNCE_LIMIT,
                Self::PHYSICS_MAX_BOUNCE_LIMIT,
            )
        } else {
            raw.clamp(
                Self::PHYSICS_MIN_IMPULSE_LIMIT,
                Self::PHYSICS_MAX_IMPULSE_LIMIT,
            )
        }
    }
    fn carry_surface_docking_clamped(&mut self, id: NodeId, to: Vec2) -> bool {
        let Some(node) = self.field.node(id) else {
            return false;
        };
        if self.is_fullscreen_node(id) {
            return false;
        }
        let from = node.pos;
        let mover_ext = self.collision_extents_for_node(node);
        let gap = self.non_overlap_gap_world();
        let motion = Vec2 {
            x: to.x - from.x,
            y: to.y - from.y,
        };
        let mut mover_pos = to;
        for _ in 0..12 {
            let others: Vec<(Vec2, CollisionExtents)> = self
                .field
                .nodes()
                .iter()
                .filter_map(|(&oid, other)| {
                    if oid == id || !self.field.is_visible(oid) {
                        return None;
                    }
                    if self.is_fullscreen_node(oid) {
                        return None;
                    }
                    if self.field.dock_partner(id) == Some(oid)
                        || self.field.dock_partner(oid) == Some(id)
                    {
                        return None;
                    }
                    Some((other.pos, self.collision_extents_for_node(other)))
                })
                .collect();
            let mut changed = false;
            for (opos, oext) in others {
                let dx = opos.x - mover_pos.x;
                let dy = opos.y - mover_pos.y;
                let req_x = self.required_sep_x(mover_pos.x, mover_ext, opos.x, oext, gap);
                let req_y = self.required_sep_y(mover_pos.y, mover_ext, opos.y, oext, gap);
                let ox = req_x - dx.abs();
                let oy = req_y - dy.abs();
                if ox <= 0.0 || oy <= 0.0 {
                    continue;
                }
                if Self::collision_axis(motion, ox, oy) {
                    let dir = Self::push_direction(dx, motion.x);
                    mover_pos.x = opos.x - dir * req_x;
                } else {
                    let dir = Self::push_direction(dy, motion.y);
                    mover_pos.y = opos.y - dir * req_y;
                }
                changed = true;
            }
            if !changed {
                break;
            }
        }
        self.carry_for_physics(id, mover_pos)
    }
    fn apply_drag_impulses(&mut self, source_id: NodeId, drag_motion: Vec2) {
        // Use total drag speed for impulse magnitude so a diagonal approach at
        // speed V gives the same kick as a direct axis-aligned hit at speed V.
        // Previously motion.x.abs()/motion.y.abs() was used per-branch, which
        // gave only ~70% of the correct impulse on a 45-degree approach, making
        // the neighbor escape too slowly and getting hit again next frame.
        let drag_speed = (drag_motion.x * drag_motion.x + drag_motion.y * drag_motion.y).sqrt();
        let mut queue = VecDeque::from([(source_id, drag_motion, drag_speed, 0usize)]);
        while let Some((mover_id, motion, motion_speed, depth)) = queue.pop_front() {
            if depth >= Self::PHYSICS_DEPTH_LIMIT {
                continue;
            }
            let Some(mover) = self.field.node(mover_id) else {
                continue;
            };
            if self.is_fullscreen_node(mover_id) {
                continue;
            }
            let mover_pos = mover.pos;
            let mover_ext = self.collision_extents_for_node(mover);
            let gap = self.non_overlap_gap_world();
            let others: Vec<(NodeId, Vec2, CollisionExtents, bool)> = self
                .field
                .nodes()
                .iter()
                .filter_map(|(&oid, other)| {
                    if oid == mover_id || !self.field.is_visible(oid) {
                        return None;
                    }
                    if self.is_fullscreen_node(oid) {
                        return None;
                    }
                    if self.field.dock_partner(mover_id) == Some(oid)
                        || self.field.dock_partner(oid) == Some(mover_id)
                    {
                        return None;
                    }
                    Some((
                        oid,
                        other.pos,
                        self.collision_extents_for_node(other),
                        self.resize_static_node == Some(oid) || other.pinned,
                    ))
                })
                .collect();
            for (other_id, other_pos, other_ext, other_locked) in others {
                let dx = other_pos.x - mover_pos.x;
                let dy = other_pos.y - mover_pos.y;
                let req_x =
                    self.required_sep_x(mover_pos.x, mover_ext, other_pos.x, other_ext, gap);
                let req_y =
                    self.required_sep_y(mover_pos.y, mover_ext, other_pos.y, other_ext, gap);
                let overlap_x = req_x - dx.abs();
                let overlap_y = req_y - dy.abs();
                if overlap_x <= 0.0 || overlap_y <= 0.0 {
                    continue;
                }
                if Self::collision_axis(motion, overlap_x, overlap_y) {
                    let dir = Self::push_direction(dx, motion.x);
                    if other_locked {
                        let _ = self.carry_for_physics(
                            mover_id,
                            Vec2 {
                                x: other_pos.x - dir * req_x,
                                y: mover_pos.y,
                            },
                        );
                        continue;
                    }
                    let correction = overlap_x + gap * 0.04;
                    let target = Vec2 {
                        x: other_pos.x + dir * correction,
                        y: other_pos.y,
                    };
                    let impulse_limit = self.collision_velocity_limit(mover_ext, other_ext, false);
                    let impulse = (dir
                        * (motion_speed * self.drag_impulse_gain()
                            + correction * self.collision_push_gain()))
                    .clamp(-impulse_limit, impulse_limit);
                    let _ = self.carry_for_physics(other_id, target);
                    self.add_physics_velocity(other_id, Vec2 { x: impulse, y: 0.0 });
                    queue.push_back((
                        other_id,
                        Vec2 {
                            x: impulse * 0.014,
                            y: 0.0,
                        },
                        impulse.abs() * 0.014,
                        depth + 1,
                    ));
                } else {
                    let dir = Self::push_direction(dy, motion.y);
                    if other_locked {
                        let _ = self.carry_for_physics(
                            mover_id,
                            Vec2 {
                                x: mover_pos.x,
                                y: other_pos.y - dir * req_y,
                            },
                        );
                        continue;
                    }
                    let correction = overlap_y + gap * 0.04;
                    let target = Vec2 {
                        x: other_pos.x,
                        y: other_pos.y + dir * correction,
                    };
                    let impulse_limit = self.collision_velocity_limit(mover_ext, other_ext, false);
                    let impulse = (dir
                        * (motion_speed * self.drag_impulse_gain()
                            + correction * self.collision_push_gain()))
                    .clamp(-impulse_limit, impulse_limit);
                    let _ = self.carry_for_physics(other_id, target);
                    self.add_physics_velocity(other_id, Vec2 { x: 0.0, y: impulse });
                    queue.push_back((
                        other_id,
                        Vec2 {
                            x: 0.0,
                            y: impulse * 0.014,
                        },
                        impulse.abs() * 0.014,
                        depth + 1,
                    ));
                }
            }
        }
    }
    fn resolve_static_surface_collisions(&mut self) {
        let mut ids: Vec<NodeId> = self
            .field
            .nodes()
            .keys()
            .copied()
            .filter(|&id| self.field.is_visible(id))
            .filter(|&id| {
                self.field
                    .node(id)
                    .is_some_and(|n| {
                        n.kind == halley_core::field::NodeKind::Surface && !self.is_fullscreen_node(id)
                    })
            })
            .collect();
        ids.sort_by_key(|id| id.as_u64());
        let gap = self.non_overlap_gap_world();
        for _ in 0..24 {
            let mut changed = false;
            for i in 0..ids.len() {
                for j in (i + 1)..ids.len() {
                    let a = ids[i];
                    let b = ids[j];
                    if self.field.dock_partner(a) == Some(b)
                        || self.field.dock_partner(b) == Some(a)
                    {
                        continue;
                    }
                    let (Some(na), Some(nb)) = (self.field.node(a), self.field.node(b)) else {
                        continue;
                    };
                    let apos = na.pos;
                    let bpos = nb.pos;
                    let aext = self.collision_extents_for_node(na);
                    let bext = self.collision_extents_for_node(nb);
                    let dx = bpos.x - apos.x;
                    let dy = bpos.y - apos.y;
                    let req_x = self.required_sep_x(apos.x, aext, bpos.x, bext, gap);
                    let req_y = self.required_sep_y(apos.y, aext, bpos.y, bext, gap);
                    let overlap_x = req_x - dx.abs();
                    let overlap_y = req_y - dy.abs();
                    if overlap_x <= 0.0 || overlap_y <= 0.0 {
                        continue;
                    }
                    let a_locked = self.resize_static_node == Some(a) || na.pinned;
                    let b_locked = self.resize_static_node == Some(b) || nb.pinned;
                    if a_locked && b_locked {
                        continue;
                    }
                    if overlap_x <= overlap_y {
                        let dir = if dx >= 0.0 { 1.0 } else { -1.0 };
                        let correction = overlap_x + 0.1;
                        if a_locked {
                            if self.carry_for_physics(
                                b,
                                Vec2 {
                                    x: bpos.x + dir * correction,
                                    y: bpos.y,
                                },
                            ) {
                                changed = true;
                            }
                        } else if b_locked {
                            if self.carry_for_physics(
                                a,
                                Vec2 {
                                    x: apos.x - dir * correction,
                                    y: apos.y,
                                },
                            ) {
                                changed = true;
                            }
                        } else {
                            let half = correction * 0.5;
                            let moved_a = self.carry_for_physics(
                                a,
                                Vec2 {
                                    x: apos.x - dir * half,
                                    y: apos.y,
                                },
                            );
                            let moved_b = self.carry_for_physics(
                                b,
                                Vec2 {
                                    x: bpos.x + dir * half,
                                    y: bpos.y,
                                },
                            );
                            changed |= moved_a || moved_b;
                        }
                    } else {
                        let dir = if dy >= 0.0 { 1.0 } else { -1.0 };
                        let correction = overlap_y + 0.1;
                        if a_locked {
                            if self.carry_for_physics(
                                b,
                                Vec2 {
                                    x: bpos.x,
                                    y: bpos.y + dir * correction,
                                },
                            ) {
                                changed = true;
                            }
                        } else if b_locked {
                            if self.carry_for_physics(
                                a,
                                Vec2 {
                                    x: apos.x,
                                    y: apos.y - dir * correction,
                                },
                            ) {
                                changed = true;
                            }
                        } else {
                            let half = correction * 0.5;
                            let moved_a = self.carry_for_physics(
                                a,
                                Vec2 {
                                    x: apos.x,
                                    y: apos.y - dir * half,
                                },
                            );
                            let moved_b = self.carry_for_physics(
                                b,
                                Vec2 {
                                    x: bpos.x,
                                    y: bpos.y + dir * half,
                                },
                            );
                            changed |= moved_a || moved_b;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
    pub(crate) fn carry_surface_non_overlap(&mut self, id: NodeId, to: Vec2) -> bool {
        if self.is_fullscreen_node(id) {
            return false;
        }
        if self.docking_active {
            return self.carry_surface_docking_clamped(id, to);
        }
        if !self.tuning.physics_enabled {
            if !self.carry_for_physics(id, to) {
                return false;
            }
            self.resolve_static_surface_collisions();
            return true;
        }
        let from = self.field.node(id).map(|n| n.pos).unwrap_or(to);
        let motion = Vec2 {
            x: to.x - from.x,
            y: to.y - from.y,
        };
        self.update_release_axis_from_motion(id, motion);
        self.physics_velocity.remove(&id);
        if !self.carry_for_physics(id, to) {
            return false;
        }
        self.apply_drag_impulses(id, motion);
        true
    }
    fn resolve_dynamic_collisions(&mut self, dt: f32) {
        let gap = self.non_overlap_gap_world();
        let ids: Vec<NodeId> = self
            .field
            .nodes()
            .keys()
            .copied()
            .filter(|&id| self.field.is_visible(id))
            .filter(|&id| !self.is_fullscreen_node(id))
            .collect();
        for _ in 0..10 {
            let mut changed = false;
            for i in 0..ids.len() {
                for j in (i + 1)..ids.len() {
                    let a = ids[i];
                    let b = ids[j];
                    if self.field.dock_partner(a) == Some(b)
                        || self.field.dock_partner(b) == Some(a)
                    {
                        continue;
                    }
                    let (Some(na), Some(nb)) = (self.field.node(a), self.field.node(b)) else {
                        continue;
                    };
                    let apos = na.pos;
                    let bpos = nb.pos;
                    let aext = self.collision_extents_for_node(na);
                    let bext = self.collision_extents_for_node(nb);
                    let dx = bpos.x - apos.x;
                    let dy = bpos.y - apos.y;
                    let req_x = self.required_sep_x(apos.x, aext, bpos.x, bext, gap);
                    let req_y = self.required_sep_y(apos.y, aext, bpos.y, bext, gap);
                    let overlap_x = req_x - dx.abs();
                    let overlap_y = req_y - dy.abs();
                    if overlap_x <= 0.0 || overlap_y <= 0.0 {
                        continue;
                    }
                    let a_locked = self.body_locked(a) || na.pinned;
                    let b_locked = self.body_locked(b) || nb.pinned;
                    if a_locked && b_locked {
                        continue;
                    }
                    let va = self
                        .physics_velocity
                        .get(&a)
                        .copied()
                        .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
                    let vb = self
                        .physics_velocity
                        .get(&b)
                        .copied()
                        .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
                    let rel = Vec2 {
                        x: vb.x - va.x,
                        y: vb.y - va.y,
                    };
                    if Self::collision_axis(rel, overlap_x, overlap_y) {
                        let dir = if dx >= 0.0 { 1.0f32 } else { -1.0 };
                        let correction = overlap_x + gap * 0.02;
                        let bounce_limit = self.collision_velocity_limit(aext, bext, true);
                        let speed = (correction / dt.max(1.0 / 240.0)).clamp(0.0, bounce_limit);
                        if a_locked {
                            let _ = self.carry_for_physics(
                                b,
                                Vec2 {
                                    x: bpos.x + dir * correction,
                                    y: bpos.y,
                                },
                            );
                            self.add_physics_velocity(
                                b,
                                Vec2 {
                                    x: dir * speed * self.collision_bounce(),
                                    y: 0.0,
                                },
                            );
                        } else if b_locked {
                            let _ = self.carry_for_physics(
                                a,
                                Vec2 {
                                    x: apos.x - dir * correction,
                                    y: apos.y,
                                },
                            );
                            self.add_physics_velocity(
                                a,
                                Vec2 {
                                    x: -dir * speed * self.collision_bounce(),
                                    y: 0.0,
                                },
                            );
                        } else {
                            let total_inv_mass =
                                self.body_inverse_mass(a) + self.body_inverse_mass(b);
                            let a_share = (self.body_inverse_mass(a) / total_inv_mass.max(0.001))
                                .clamp(0.0, 1.0);
                            let b_share = (self.body_inverse_mass(b) / total_inv_mass.max(0.001))
                                .clamp(0.0, 1.0);
                            let _ = self.carry_for_physics(
                                a,
                                Vec2 {
                                    x: apos.x - dir * correction * a_share,
                                    y: apos.y,
                                },
                            );
                            let _ = self.carry_for_physics(
                                b,
                                Vec2 {
                                    x: bpos.x + dir * correction * b_share,
                                    y: bpos.y,
                                },
                            );
                            self.add_physics_velocity(
                                a,
                                Vec2 {
                                    x: -dir * speed * self.collision_bounce() * a_share,
                                    y: 0.0,
                                },
                            );
                            self.add_physics_velocity(
                                b,
                                Vec2 {
                                    x: dir * speed * self.collision_bounce() * b_share,
                                    y: 0.0,
                                },
                            );
                        }
                    } else {
                        let dir = if dy >= 0.0 { 1.0_f32 } else { -1.0 };
                        let correction = overlap_y + gap * 0.02;
                        let bounce_limit = self.collision_velocity_limit(aext, bext, true);
                        let speed = (correction / dt.max(1.0 / 240.0)).clamp(0.0, bounce_limit);
                        if a_locked {
                            let _ = self.carry_for_physics(
                                b,
                                Vec2 {
                                    x: bpos.x,
                                    y: bpos.y + dir * correction,
                                },
                            );
                            self.add_physics_velocity(
                                b,
                                Vec2 {
                                    x: 0.0,
                                    y: dir * speed * self.collision_bounce(),
                                },
                            );
                        } else if b_locked {
                            let _ = self.carry_for_physics(
                                a,
                                Vec2 {
                                    x: apos.x,
                                    y: apos.y - dir * correction,
                                },
                            );
                            self.add_physics_velocity(
                                a,
                                Vec2 {
                                    x: 0.0,
                                    y: -dir * speed * self.collision_bounce(),
                                },
                            );
                        } else {
                            let total_inv_mass =
                                self.body_inverse_mass(a) + self.body_inverse_mass(b);
                            let a_share = (self.body_inverse_mass(a) / total_inv_mass.max(0.001))
                                .clamp(0.0, 1.0);
                            let b_share = (self.body_inverse_mass(b) / total_inv_mass.max(0.001))
                                .clamp(0.0, 1.0);
                            let _ = self.carry_for_physics(
                                a,
                                Vec2 {
                                    x: apos.x,
                                    y: apos.y - dir * correction * a_share,
                                },
                            );
                            let _ = self.carry_for_physics(
                                b,
                                Vec2 {
                                    x: bpos.x,
                                    y: bpos.y + dir * correction * b_share,
                                },
                            );
                            self.add_physics_velocity(
                                a,
                                Vec2 {
                                    x: 0.0,
                                    y: -dir * speed * self.collision_bounce() * a_share,
                                },
                            );
                            self.add_physics_velocity(
                                b,
                                Vec2 {
                                    x: 0.0,
                                    y: dir * speed * self.collision_bounce() * b_share,
                                },
                            );
                        }
                    }
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }
    pub(crate) fn tick_passive_physics(&mut self) {
        if !self.tuning.physics_enabled {
            return;
        }
        let dt = (1.0_f32 / 60.0).clamp(1.0 / 240.0, 1.0 / 20.0);
        let ids: Vec<NodeId> = self.physics_velocity.keys().copied().collect();
        for id in ids {
            if self.body_locked(id) || !self.field.is_visible(id) {
                self.physics_velocity.remove(&id);
                continue;
            }
            let vel = self
                .physics_velocity
                .get(&id)
                .copied()
                .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
            if vel.x.abs() < Self::PHYSICS_MIN_SPEED && vel.y.abs() < Self::PHYSICS_MIN_SPEED {
                self.physics_velocity.remove(&id);
                continue;
            }
            if let Some(node) = self.field.node(id) {
                let next = Vec2 {
                    x: node.pos.x + vel.x * dt,
                    y: node.pos.y + vel.y * dt,
                };
                let _ = self.carry_for_physics(id, next);
            }
            let damping = (1.0 - self.physics_damping_per_sec() * dt).clamp(0.0, 1.0);
            if let Some(v) = self.physics_velocity.get_mut(&id) {
                v.x *= damping;
                v.y *= damping;
            }
        }
        self.resolve_dynamic_collisions(dt);
    }
    fn preview_collision_size(real_w: f32, real_h: f32) -> Vec2 {
        let w = real_w.max(1.0);
        let h = real_h.max(1.0);
        let aspect = w / h;
        let base_h = 160.0f32;
        let mut out_w = base_h * aspect;
        let mut out_h = base_h;
        if out_w < 180.0 {
            out_w = 180.0;
            out_h = out_w / aspect.max(0.1);
        }
        if out_w > 360.0 {
            out_w = 360.0;
            out_h = out_w / aspect.max(0.1);
        }
        out_h = out_h.clamp(100.0, 220.0);
        Vec2 { x: out_w, y: out_h }
    }
    #[inline]
    fn active_collision_scale(anim_scale: f32, real_w: f32, real_h: f32) -> f32 {
        let base = Self::preview_collision_size(real_w, real_h);
        let start = (base.x / real_w.max(1.0))
            .min(base.y / real_h.max(1.0))
            .clamp(0.24, 1.0);
        let t = ((anim_scale - 0.30) / (1.0 - 0.30)).clamp(0.0, 1.0);
        let e = t * t * (3.0 - 2.0 * t);
        let mut out = start + (1.0 - start) * e;
        if anim_scale > 1.0 {
            out += (anim_scale - 1.0) * 0.30;
        }
        out.clamp(0.24, 1.08)
    }
    #[inline]
    fn proxy_collision_scale(anim_scale: f32) -> f32 {
        anim_scale.clamp(0.22, 1.4)
    }
    fn node_collision_extents(&self, label: &str, anim_scale: f32) -> CollisionExtents {
        let zx = self.viewport.size.x / self.zoom_ref_size.x.max(1.0);
        let zy = self.viewport.size.y / self.zoom_ref_size.y.max(1.0);
        let z = ((zx + zy) * 0.5).clamp(1.0, 8.0);
        let g = z.sqrt() * Self::proxy_collision_scale(anim_scale);
        let dot_half_px = (4.0 * g).round().clamp(4.0, 18.0);
        let label_h_px = (4.0 * g).round().clamp(4.0, 14.0);
        let label_gap_px = (8.0 + (g - 1.0) * 8.0).round().clamp(8.0, 28.0);
        let label_w_px = ((label.len() as f32 * 6.0) * (0.9 + 0.6 * g))
            .round()
            .clamp(24.0, 320.0);
        let pad_px = 6.0;
        let dot_d_px = (dot_half_px * 2.0).max(1.0);
        let marker_w_px = (dot_d_px + label_gap_px + label_w_px + pad_px * 2.0).max(8.0);
        let marker_h_px = (dot_d_px.max(label_h_px) + pad_px * 2.0).max(8.0);
        let world_per_px_x = self.viewport.size.x / self.zoom_ref_size.x.max(1.0);
        let world_per_px_y = self.viewport.size.y / self.zoom_ref_size.y.max(1.0);
        CollisionExtents::symmetric(Vec2 {
            x: marker_w_px * world_per_px_x.max(0.01),
            y: marker_h_px * world_per_px_y.max(0.01),
        })
    }
    fn surface_window_collision_extents(&self, n: &halley_core::field::Node) -> CollisionExtents {
        let basis = self
            .last_active_size
            .get(&n.id)
            .copied()
            .unwrap_or(n.intrinsic_size);
        let (world_per_px_x, world_per_px_y) = self.world_units_per_px_xy();
        let bbox_w = n.intrinsic_size.x.max(1.0);
        let bbox_h = n.intrinsic_size.y.max(1.0);
        let (bbox_lx, bbox_ly) = self.bbox_loc.get(&n.id).copied().unwrap_or((0.0, 0.0));
        let (geo_lx, geo_ly, geo_w, geo_h) = self
            .window_geometry
            .get(&n.id)
            .copied()
            .unwrap_or((bbox_lx, bbox_ly, bbox_w, bbox_h));
        let left = (bbox_w * 0.5 + bbox_lx - geo_lx).max(16.0);
        let right = (geo_lx + geo_w - bbox_lx - bbox_w * 0.5).max(16.0);
        let top = (bbox_h * 0.5 + bbox_ly - geo_ly).max(16.0);
        let bottom = (geo_ly + geo_h - bbox_ly - bbox_h * 0.5).max(16.0);
        CollisionExtents {
            left: left * basis.x.max(1.0) / bbox_w * world_per_px_x,
            right: right * basis.x.max(1.0) / bbox_w * world_per_px_x,
            top: top * basis.y.max(1.0) / bbox_h * world_per_px_y,
            bottom: bottom * basis.y.max(1.0) / bbox_h * world_per_px_y,
        }
    }
    pub(crate) fn spawn_obstacle_extents_for_node(
        &self,
        n: &halley_core::field::Node,
    ) -> CollisionExtents {
        if n.kind == halley_core::field::NodeKind::Surface {
            self.surface_window_collision_extents(n)
        } else {
            self.collision_extents_for_node(n)
        }
    }
    pub(crate) fn collision_extents_for_node(
        &self,
        n: &halley_core::field::Node,
    ) -> CollisionExtents {
        if self.is_fullscreen_node(n.id) {
            return CollisionExtents::symmetric(Vec2 { x: 0.0, y: 0.0 });
        }
        let now = Instant::now();
        let anim = self.anim_style_for(n.id, n.state.clone(), now);
        match n.state {
            halley_core::field::NodeState::Active => {
                let basis = self
                    .last_active_size
                    .get(&n.id)
                    .copied()
                    .unwrap_or(n.intrinsic_size);
                let s = Self::active_collision_scale(anim.scale, basis.x, basis.y);
                let ext = self.surface_window_collision_extents(n);
                CollisionExtents {
                    left: ext.left * s,
                    right: ext.right * s,
                    top: ext.top * s,
                    bottom: ext.bottom * s,
                }
            }
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core => {
                self.node_collision_extents(&n.label, anim.scale)
            }
            halley_core::field::NodeState::Drifting => CollisionExtents::symmetric(n.footprint),
        }
    }
    pub(super) fn collision_size_for_node(&self, n: &halley_core::field::Node) -> Vec2 {
        self.collision_extents_for_node(n).size()
    }
    pub(crate) fn resolve_surface_overlap(&mut self) {
        if self.tuning.physics_enabled || self.suspend_overlap_resolve {
            return;
        }
        self.resolve_static_surface_collisions();
    }
    pub(super) fn request_toplevel_resize(&mut self, node_id: NodeId, width: i32, height: i32) {
        let width = width.max(96);
        let height = height.max(72);
        let focused_node = self.last_input_surface_node();
        for top in self.xdg_shell_state.toplevel_surfaces() {
            let wl = top.wl_surface();
            let key = wl.id();
            if self.surface_to_node.get(&key).copied() != Some(node_id) {
                continue;
            }
            top.with_pending_state(|s| {
                s.size = Some((width, height).into());
                if focused_node == Some(node_id) {
                    s.states.set(xdg_toplevel::State::Activated);
                } else {
                    s.states.unset(xdg_toplevel::State::Activated);
                }
                if self.is_fullscreen_node(node_id) {
                    s.states.set(xdg_toplevel::State::Fullscreen);
                } else {
                    s.states.unset(xdg_toplevel::State::Fullscreen);
                }
            });
            top.send_configure();
            break;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn test_state(tuning: halley_config::RuntimeTuning) -> HalleyWlState {
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        state.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state
    }
    #[test]
    fn collapsed_surface_nodes_use_marker_collision_extents() {
        let mut state = test_state(halley_config::RuntimeTuning::default());
        let id = state.field.spawn_surface(
            "collapsed-firefox",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 1200.0,
                y: 900.0,
            },
        );
        let _ = state
            .field
            .set_state(id, halley_core::field::NodeState::Node);
        let node = state.field.node(id).expect("node");
        let ext = state.collision_extents_for_node(node);
        assert!(ext.left + ext.right < 300.0);
        assert!(ext.top + ext.bottom < 120.0);
    }
    #[test]
    fn resolve_surface_overlap_enforces_configured_gap() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.physics_enabled = false;
        let mut state = test_state(tuning);
        let a =
            state
                .field
                .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let b =
            state
                .field
                .spawn_surface("b", Vec2 { x: 410.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        state.resolve_surface_overlap();
        let na = state.field.node(a).expect("first node");
        let nb = state.field.node(b).expect("second node");
        let ea = state.collision_extents_for_node(na);
        let eb = state.collision_extents_for_node(nb);
        let gap = state.non_overlap_gap_world();
        let dx = (nb.pos.x - na.pos.x).abs();
        let req_x = state.required_sep_x(na.pos.x, ea, nb.pos.x, eb, gap);
        assert!(dx >= req_x - 0.5);
    }
    #[test]
    fn static_drag_resolution_keeps_surfaces_non_overlapping_when_physics_is_disabled() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.physics_enabled = false;
        let mut state = test_state(tuning);
        let a =
            state
                .field
                .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let b =
            state
                .field
                .spawn_surface("b", Vec2 { x: 430.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        state.begin_carry_state_tracking(a);
        assert!(state.carry_surface_non_overlap(a, Vec2 { x: 280.0, y: 0.0 }));
        let na = state.field.node(a).expect("first node");
        let nb = state.field.node(b).expect("second node");
        let ea = state.collision_extents_for_node(na);
        let eb = state.collision_extents_for_node(nb);
        let gap = state.non_overlap_gap_world();
        let dx = (nb.pos.x - na.pos.x).abs();
        let req_x = state.required_sep_x(na.pos.x, ea, nb.pos.x, eb, gap);
        assert!(
            dx >= req_x - 0.5,
            "expected static carry path to keep surfaces separated: dx={dx}, req_x={req_x}"
        );
    }
    #[test]
    fn static_drag_resolution_splits_overlap_correction_across_windows() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.physics_enabled = false;
        let mut state = test_state(tuning);
        let a =
            state
                .field
                .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let b =
            state
                .field
                .spawn_surface("b", Vec2 { x: 430.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        state.begin_carry_state_tracking(a);
        assert!(state.carry_surface_non_overlap(a, Vec2 { x: 280.0, y: 0.0 }));
        let na = state.field.node(a).expect("first node");
        let nb = state.field.node(b).expect("second node");
        let moved_a = 280.0 - na.pos.x;
        let moved_b = nb.pos.x - 430.0;
        assert!(
            moved_a > 0.0 && moved_b > 0.0,
            "expected both windows to move during static resolution: a={moved_a}, b={moved_b}"
        );
        assert!(
            (moved_a - moved_b).abs() < 0.6,
            "expected static resolution to split correction evenly: a={moved_a}, b={moved_b}"
        );
    }
    #[test]
    fn physics_drag_adds_extra_velocity_to_hit_window() {
        let mut state = test_state(halley_config::RuntimeTuning::default());
        let a =
            state
                .field
                .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let b =
            state
                .field
                .spawn_surface("b", Vec2 { x: 430.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        state.begin_carry_state_tracking(a);
        assert!(state.carry_surface_non_overlap(a, Vec2 { x: 280.0, y: 0.0 }));
        let vb = state
            .physics_velocity
            .get(&b)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
        assert!(
            vb.x > 100.0,
            "expected hit window to receive horizontal velocity, got {:?}",
            vb
        );
    }
    #[test]
    fn collapsed_nodes_receive_less_velocity_than_active_windows() {
        let mut state = test_state(halley_config::RuntimeTuning::default());
        let window = state.field.spawn_surface(
            "window",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 400.0, y: 300.0 },
        );
        let node =
            state
                .field
                .spawn_surface("node", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let _ = state
            .field
            .set_state(node, halley_core::field::NodeState::Node);
        state.add_physics_velocity(window, Vec2 { x: 100.0, y: 0.0 });
        state.add_physics_velocity(node, Vec2 { x: 100.0, y: 0.0 });
        let window_vx = state
            .physics_velocity
            .get(&window)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 })
            .x;
        let node_vx = state
            .physics_velocity
            .get(&node)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 })
            .x;
        assert!(
            node_vx > 0.0 && node_vx < window_vx,
            "expected collapsed node to pick up less velocity than a window: node={node_vx}, window={window_vx}"
        );
    }
    #[test]
    fn passive_window_keeps_moving_after_drag_impulse() {
        let mut state = test_state(halley_config::RuntimeTuning::default());
        let a =
            state
                .field
                .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let b =
            state
                .field
                .spawn_surface("b", Vec2 { x: 430.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        state.begin_carry_state_tracking(a);
        assert!(state.carry_surface_non_overlap(a, Vec2 { x: 280.0, y: 0.0 }));
        let before = state.field.node(b).expect("hit window").pos;
        state.tick_passive_physics();
        state.tick_passive_physics();
        let after = state.field.node(b).expect("hit window after").pos;
        assert!(
            after.x > before.x + 1.0,
            "expected passive window to keep moving after contact: before={before:?}, after={after:?}"
        );
    }
    #[test]
    fn chained_window_node_collision_caps_runaway_velocity() {
        let mut state = test_state(halley_config::RuntimeTuning::default());
        let dragged = state.field.spawn_surface(
            "dragged",
            Vec2 { x: -520.0, y: 0.0 },
            Vec2 { x: 400.0, y: 300.0 },
        );
        let node =
            state
                .field
                .spawn_surface("node", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let other = state.field.spawn_surface(
            "other",
            Vec2 { x: 120.0, y: 0.0 },
            Vec2 { x: 400.0, y: 300.0 },
        );
        let _ = state
            .field
            .set_state(node, halley_core::field::NodeState::Node);
        state.begin_carry_state_tracking(dragged);
        assert!(state.carry_surface_non_overlap(dragged, Vec2 { x: -40.0, y: 0.0 }));
        let node_vx = state
            .physics_velocity
            .get(&node)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 })
            .x
            .abs();
        let other_vx = state
            .physics_velocity
            .get(&other)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 })
            .x
            .abs();
        let node_limit = state
            .field
            .node(node)
            .map(|n| {
                let ext = state.collision_extents_for_node(n);
                state.collision_velocity_limit(ext, ext, true)
            })
            .expect("node limit");
        let other_limit = state
            .field
            .node(other)
            .map(|n| {
                let ext = state.collision_extents_for_node(n);
                state.collision_velocity_limit(ext, ext, true)
            })
            .expect("other limit");
        assert!(
            node_vx <= node_limit + 0.1,
            "expected node velocity to stay bounded after chained overlap: {node_vx}"
        );
        assert!(
            other_vx <= other_limit + 0.1,
            "expected downstream window velocity to stay bounded after chained overlap: {other_vx}"
        );
        for _ in 0..8 {
            state.tick_passive_physics();
        }
        let node_vx_after = state
            .physics_velocity
            .get(&node)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 })
            .x
            .abs();
        let other_vx_after = state
            .physics_velocity
            .get(&other)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 })
            .x
            .abs();
        assert!(
            node_vx_after <= node_limit + 0.1,
            "expected passive node bounce to stay bounded: {node_vx_after}"
        );
        assert!(
            other_vx_after <= other_limit + 0.1,
            "expected passive window bounce to stay bounded: {other_vx_after}"
        );
    }
    #[test]
    fn lower_damping_reduces_collision_impulse_strength() {
        let mut soft_tuning = halley_config::RuntimeTuning::default();
        soft_tuning.non_overlap_bump_damping = 0.1;
        let mut soft = test_state(soft_tuning);
        let a = soft
            .field
            .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let b =
            soft.field
                .spawn_surface("b", Vec2 { x: 430.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        soft.begin_carry_state_tracking(a);
        assert!(soft.carry_surface_non_overlap(a, Vec2 { x: 280.0, y: 0.0 }));
        let soft_vx = soft
            .physics_velocity
            .get(&b)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 })
            .x;
        let mut firm_tuning = halley_config::RuntimeTuning::default();
        firm_tuning.non_overlap_bump_damping = 0.9;
        let mut firm = test_state(firm_tuning);
        let a = firm
            .field
            .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let b =
            firm.field
                .spawn_surface("b", Vec2 { x: 430.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        firm.begin_carry_state_tracking(a);
        assert!(firm.carry_surface_non_overlap(a, Vec2 { x: 280.0, y: 0.0 }));
        let firm_vx = firm
            .physics_velocity
            .get(&b)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 })
            .x;
        assert!(
            soft_vx > 0.0 && firm_vx > soft_vx,
            "expected firmer damping to yield a stronger impulse: soft={soft_vx}, firm={firm_vx}"
        );
    }
    #[test]
    fn docking_mode_clamps_the_dragged_window_instead_of_pushing_neighbors() {
        let mut state = test_state(halley_config::RuntimeTuning::default());
        state.docking_active = true;
        let a =
            state
                .field
                .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let b =
            state
                .field
                .spawn_surface("b", Vec2 { x: 20.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        assert!(state.carry_surface_non_overlap(a, Vec2 { x: 20.0, y: 0.0 }));
        let na = state.field.node(a).expect("first window");
        let nb = state.field.node(b).expect("second window");
        assert!((nb.pos.x - 20.0).abs() < 0.01);
        assert!(na.pos.x < 20.0);
    }
}
