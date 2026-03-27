use super::*;
use crate::animation::{ease_in_out_cubic, proxy_anim_scale};
use crate::render::ACTIVE_WINDOW_FRAME_PAD_PX;
use crate::render::preview_proxy_size;
use crate::state::{InteractionState, MonitorState, RenderState, WorkspaceState};
use halley_core::viewport::Viewport;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;

const CONTACT_SLOP: f32 = 0.5;
const CONTACT_SKIN: f32 = 1.5;
const MAX_PHYSICS_SPEED: f32 = 1600.0;
const LINEAR_DAMPING_FALLBACK_PER_SEC: f32 = 4.5;
const USER_DAMPING_MIN: f32 = 0.0;
const USER_DAMPING_MAX: f32 = 1.0;
const INTERNAL_DAMPING_MIN_PER_SEC: f32 = 3.0;
const INTERNAL_DAMPING_MAX_PER_SEC: f32 = 8.0;
const CONTACT_RESTITUTION: f32 = 0.02;
const CONTACT_FRICTION: f32 = 0.22;
const MAX_CONTACT_IMPULSE: f32 = 380.0;
const MAX_POSITION_CORRECTION: f32 = 48.0;
const POSITION_SOLVER_ITERS: usize = 6;
const PHYSICS_REST_EPSILON: f32 = 4.0;

#[inline]
fn node_render_diameter_px_for_viewport(
    viewport: Viewport,
    intrinsic_size: Vec2,
    _label_len: usize,
    anim_scale: f32,
) -> f32 {
    const PROXY_TO_MARKER_START: f32 = 0.50;
    const PROXY_TO_MARKER_END: f32 = 0.20;

    let marker_mix_lin = ((PROXY_TO_MARKER_START - anim_scale)
        / (PROXY_TO_MARKER_START - PROXY_TO_MARKER_END))
        .clamp(0.0, 1.0);
    let marker_mix = ease_in_out_cubic(marker_mix_lin);

    let marker_diameter = ((17.0f32 * 1.5).round().max(1.0)) * 2.0;
    let (pw, ph) = preview_proxy_size(intrinsic_size.x, intrinsic_size.y);
    let _ = viewport;
    let proxy_diameter = pw.min(ph) * proxy_anim_scale(anim_scale);

    (proxy_diameter + (marker_diameter - proxy_diameter) * marker_mix).max(marker_diameter)
}

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

struct OverlapReadContext<'a> {
    field: &'a Field,
    monitor_state: &'a MonitorState,
    interaction_state: &'a InteractionState,
    render_state: &'a RenderState,
    workspace_state: &'a WorkspaceState,
    tuning: &'a RuntimeTuning,
    viewport: Viewport,
    camera_render_scale: f32,
}

impl<'a> OverlapReadContext<'a> {
    #[inline]
    fn clamp_speed(v: Vec2, max_speed: f32) -> Vec2 {
        let speed_sq = v.x * v.x + v.y * v.y;
        if speed_sq <= max_speed * max_speed {
            return v;
        }
        let speed = speed_sq.sqrt().max(f32::EPSILON);
        let scale = max_speed / speed;
        Vec2 {
            x: v.x * scale,
            y: v.y * scale,
        }
    }

    #[inline]
    fn physics_damping_per_sec(&self) -> f32 {
        let user = self.tuning.non_overlap_bump_damping;
        if !user.is_finite() {
            return LINEAR_DAMPING_FALLBACK_PER_SEC;
        }
        let x = user.clamp(USER_DAMPING_MIN, USER_DAMPING_MAX);
        let t = 1.0 - (1.0 - x) * (1.0 - x);
        INTERNAL_DAMPING_MIN_PER_SEC
            + t * (INTERNAL_DAMPING_MAX_PER_SEC - INTERNAL_DAMPING_MIN_PER_SEC)
    }

    #[inline]
    fn physics_inv_mass(&self, id: NodeId, pinned: bool) -> f32 {
        if pinned
            || self.interaction_state.drag_authority_node == Some(id)
            || self.interaction_state.resize_active == Some(id)
        {
            0.0
        } else {
            1.0
        }
    }

    #[inline]
    fn node_participates_in_overlap(&self, id: NodeId) -> bool {
        if !self.field.participates_in_field_dynamics(id) {
            return false;
        }
        self.field.node(id).is_some_and(|n| {
            self.field.is_visible(id)
                && matches!(
                    n.state,
                    halley_core::field::NodeState::Active
                        | halley_core::field::NodeState::Node
                        | halley_core::field::NodeState::Core
                        | halley_core::field::NodeState::Drifting
                )
        })
    }

    pub(crate) fn non_overlap_gap_world(&self) -> f32 {
        // Overlap resolution must live purely in stable world-space. Camera
        // zoom must never change the required separation between nodes.
        self.tuning.non_overlap_gap_px.max(0.0)
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
    fn nodes_share_overlap_group(&self, a: NodeId, b: NodeId) -> bool {
        match (
            self.monitor_state.node_monitor.get(&a),
            self.monitor_state.node_monitor.get(&b),
        ) {
            (Some(a_monitor), Some(b_monitor)) => a_monitor == b_monitor,
            _ => true,
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

    pub(crate) fn node_collision_extents_stable(
        &self,
        intrinsic_size: Vec2,
        label: &str,
        anim_scale: f32,
    ) -> CollisionExtents {
        let diameter_px = node_render_diameter_px_for_viewport(
            self.viewport,
            intrinsic_size,
            label.len(),
            anim_scale,
        );
        let radius_px = (diameter_px * 0.5).round().max(1.0);

        CollisionExtents::symmetric(Vec2 {
            x: radius_px * 2.0,
            y: radius_px * 2.0,
        })
    }

    pub(crate) fn node_collision_extents(
        &self,
        intrinsic_size: Vec2,
        label: &str,
        anim_scale: f32,
    ) -> CollisionExtents {
        let stable = self.node_collision_extents_stable(intrinsic_size, label, anim_scale);
        let cam_scale = self.camera_render_scale.max(0.01);

        CollisionExtents::symmetric(Vec2 {
            x: stable.size().x / cam_scale,
            y: stable.size().y / cam_scale,
        })
    }

    pub(crate) fn surface_window_collision_extents(
        &self,
        n: &halley_core::field::Node,
    ) -> CollisionExtents {
        let basis = self
            .workspace_state
            .last_active_size
            .get(&n.id)
            .copied()
            .or_else(|| {
                self.render_state
                    .window_geometry
                    .get(&n.id)
                    .map(|(_, _, w, h)| Vec2 { x: *w, y: *h })
            })
            .unwrap_or(n.intrinsic_size);
        let bbox_w = n.intrinsic_size.x.max(1.0);
        let bbox_h = n.intrinsic_size.y.max(1.0);
        let (bbox_lx, bbox_ly) = self
            .render_state
            .bbox_loc
            .get(&n.id)
            .copied()
            .unwrap_or((0.0, 0.0));
        let (geo_lx, geo_ly, geo_w, geo_h) = self
            .render_state
            .window_geometry
            .get(&n.id)
            .copied()
            .unwrap_or((bbox_lx, bbox_ly, bbox_w, bbox_h));

        let left = (bbox_w * 0.5 + bbox_lx - geo_lx).max(16.0);
        let right = (geo_lx + geo_w - bbox_lx - bbox_w * 0.5).max(16.0);
        let top = (bbox_h * 0.5 + bbox_ly - geo_ly).max(16.0);
        let bottom = (geo_ly + geo_h - bbox_ly - bbox_h * 0.5).max(16.0);
        let frame_pad = ACTIVE_WINDOW_FRAME_PAD_PX.max(0) as f32;

        CollisionExtents {
            left: left * basis.x.max(1.0) / bbox_w + frame_pad,
            right: right * basis.x.max(1.0) / bbox_w + frame_pad,
            top: top * basis.y.max(1.0) / bbox_h + frame_pad,
            bottom: bottom * basis.y.max(1.0) / bbox_h + frame_pad,
        }
    }

    pub(crate) fn spawn_obstacle_extents_for_node(
        &self,
        n: &halley_core::field::Node,
    ) -> CollisionExtents {
        debug_assert!(n.kind == halley_core::field::NodeKind::Surface);
        self.surface_window_collision_extents(n)
    }
}

impl Halley {
    fn overlap_read_context(&self) -> OverlapReadContext<'_> {
        OverlapReadContext {
            field: &self.model.field,
            monitor_state: &self.model.monitor_state,
            interaction_state: &self.input.interaction_state,
            render_state: &self.ui.render_state,
            workspace_state: &self.model.workspace_state,
            tuning: &self.runtime.tuning,
            viewport: self.model.viewport,
            camera_render_scale: self.camera_render_scale(),
        }
    }

    #[inline]
    fn clamp_speed(v: Vec2, max_speed: f32) -> Vec2 {
        OverlapReadContext::clamp_speed(v, max_speed)
    }

    #[inline]
    fn physics_damping_per_sec(&self) -> f32 {
        self.overlap_read_context().physics_damping_per_sec()
    }

    #[inline]
    fn physics_inv_mass(&self, id: NodeId, pinned: bool) -> f32 {
        self.overlap_read_context().physics_inv_mass(id, pinned)
    }

    #[inline]
    fn node_participates_in_overlap(&self, id: NodeId) -> bool {
        self.overlap_read_context().node_participates_in_overlap(id)
    }

    pub(crate) fn non_overlap_gap_world(&self) -> f32 {
        self.overlap_read_context().non_overlap_gap_world()
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
        self.overlap_read_context()
            .required_sep_x(a_pos_x, a_ext, b_pos_x, b_ext, gap)
    }

    #[inline]
    fn nodes_share_overlap_group(&self, a: NodeId, b: NodeId) -> bool {
        self.overlap_read_context().nodes_share_overlap_group(a, b)
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
        self.overlap_read_context()
            .required_sep_y(a_pos_y, a_ext, b_pos_y, b_ext, gap)
    }

    pub(crate) fn carry_surface_non_overlap(
        &mut self,
        id: NodeId,
        to: Vec2,
        clamp_only: bool,
    ) -> bool {
        let carry_direct = |this: &mut Self, id: NodeId, to: Vec2| {
            if this
                .model
                .field
                .node(id)
                .is_some_and(|node| node.kind == halley_core::field::NodeKind::Core)
            {
                this.model.field.carry_cluster_by_core(id, to)
            } else {
                this.model.field.carry(id, to)
            }
        };
        let moved = if !self.runtime.tuning.physics_enabled {
            self.carry_surface_no_overlap_static(id, to)
        } else if clamp_only
            || self.input.interaction_state.suspend_overlap_resolve
            || self.input.interaction_state.suspend_state_checks
        {
            self.carry_surface_no_overlap_static(id, to)
        } else {
            carry_direct(self, id, to)
        };
        moved
    }

    fn carry_surface_no_overlap_static(&mut self, id: NodeId, to: Vec2) -> bool {
        let Some(n) = self.model.field.node(id) else {
            return false;
        };

        let mover_ext = self.collision_extents_for_node(n);
        let gap = self.non_overlap_gap_world();
        let mut mover_pos = to;

        for _ in 0..24 {
            let others: Vec<(NodeId, Vec2, CollisionExtents)> = self
                .model
                .field
                .nodes()
                .iter()
                .filter_map(|(&oid, other)| {
                    if oid == id
                        || !self.node_participates_in_overlap(oid)
                        || !self.nodes_share_overlap_group(id, oid)
                    {
                        return None;
                    }
                    Some((oid, other.pos, self.collision_extents_for_node(other)))
                })
                .collect();

            let mut changed = false;

            for (oid, opos, oext) in others {
                let dx = mover_pos.x - opos.x;
                let dy = mover_pos.y - opos.y;
                let req_x = self.required_sep_x(mover_pos.x, mover_ext, opos.x, oext, gap);
                let req_y = self.required_sep_y(mover_pos.y, mover_ext, opos.y, oext, gap);
                let ox = req_x - dx.abs();
                let oy = req_y - dy.abs();

                if ox <= 0.0 || oy <= 0.0 {
                    continue;
                }

                if ox < oy {
                    let s = if dx.abs() > f32::EPSILON {
                        dx.signum()
                    } else if oid.as_u64() < id.as_u64() {
                        1.0
                    } else {
                        -1.0
                    };
                    mover_pos.x += s * (ox + 0.3);
                } else {
                    let s = if dy.abs() > f32::EPSILON {
                        dy.signum()
                    } else {
                        1.0
                    };
                    mover_pos.y += s * (oy + 0.3);
                }

                changed = true;
            }

            if !changed {
                break;
            }
        }

        if self
            .model
            .field
            .node(id)
            .is_some_and(|node| node.kind == halley_core::field::NodeKind::Core)
        {
            self.model.field.carry_cluster_by_core(id, mover_pos)
        } else {
            self.model.field.carry(id, mover_pos)
        }
    }

    pub(crate) fn surface_window_collision_extents(
        &self,
        n: &halley_core::field::Node,
    ) -> CollisionExtents {
        self.overlap_read_context()
            .surface_window_collision_extents(n)
    }

    pub(crate) fn spawn_obstacle_extents_for_node(
        &self,
        n: &halley_core::field::Node,
    ) -> CollisionExtents {
        if n.kind == halley_core::field::NodeKind::Surface {
            self.overlap_read_context()
                .spawn_obstacle_extents_for_node(n)
        } else {
            self.collision_extents_for_node(n)
        }
    }

    pub(crate) fn collision_extents_for_node(
        &self,
        n: &halley_core::field::Node,
    ) -> CollisionExtents {
        let anim = self.anim_style_for(n.id, n.state.clone(), Instant::now());
        match n.state {
            halley_core::field::NodeState::Active => {
                let basis = self
                    .model
                    .workspace_state
                    .last_active_size
                    .get(&n.id)
                    .copied()
                    .unwrap_or(n.intrinsic_size);
                let s = OverlapReadContext::active_collision_scale(anim.scale, basis.x, basis.y);
                let ext = self
                    .overlap_read_context()
                    .surface_window_collision_extents(n);

                CollisionExtents {
                    left: ext.left * s,
                    right: ext.right * s,
                    top: ext.top * s,
                    bottom: ext.bottom * s,
                }
            }
            halley_core::field::NodeState::Node => self
                .overlap_read_context()
                .node_collision_extents(n.intrinsic_size, &n.label, anim.scale),
            halley_core::field::NodeState::Core => self
                .overlap_read_context()
                .node_collision_extents(n.intrinsic_size, &n.label, anim.scale),
            halley_core::field::NodeState::Drifting => CollisionExtents::symmetric(n.footprint),
        }
    }

    pub(super) fn collision_size_for_node(&self, n: &halley_core::field::Node) -> Vec2 {
        self.collision_extents_for_node(n).size()
    }

    fn layout_collision_extents_for_node(&self, n: &halley_core::field::Node) -> CollisionExtents {
        match n.state {
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core => {
                self.collision_extents_for_node(n)
            }
            _ => self.collision_extents_for_node(n),
        }
    }

    pub(crate) fn resolve_surface_overlap(&mut self) {
        if !self.runtime.tuning.physics_enabled {
            return;
        }
        if self.input.interaction_state.suspend_overlap_resolve {
            return;
        }

        let mut ids: Vec<NodeId> = self
            .model
            .field
            .nodes()
            .keys()
            .copied()
            .filter(|&id| self.node_participates_in_overlap(id))
            .collect();

        if ids.is_empty() {
            return;
        }

        ids.sort_by_key(|id| id.as_u64());

        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.input.interaction_state.physics_last_tick)
            .as_secs_f32()
            .clamp(1.0 / 240.0, 1.0 / 30.0);
        self.input.interaction_state.physics_last_tick = now;

        let gap = self.non_overlap_gap_world();
        let damping_per_sec = self.physics_damping_per_sec();
        let damping = (-damping_per_sec * dt).exp();
        let mut positions: std::collections::HashMap<NodeId, Vec2> =
            std::collections::HashMap::new();
        let mut velocities: std::collections::HashMap<NodeId, Vec2> =
            std::collections::HashMap::new();

        for &id in &ids {
            let Some(node) = self.model.field.node(id) else {
                continue;
            };
            positions.insert(id, node.pos);
            let vel = if self.input.interaction_state.drag_authority_node == Some(id) {
                self.input.interaction_state.drag_authority_velocity
            } else {
                self.input
                    .interaction_state
                    .physics_velocity
                    .get(&id)
                    .copied()
                    .unwrap_or(Vec2 { x: 0.0, y: 0.0 })
            };
            velocities.insert(id, Self::clamp_speed(vel, MAX_PHYSICS_SPEED));
        }

        for &id in &ids {
            let Some(node) = self.model.field.node(id) else {
                continue;
            };
            let pinned = node.pinned || self.input.interaction_state.resize_static_node == Some(id);
            if self.physics_inv_mass(id, pinned) <= 0.0 {
                continue;
            }
            if let (Some(pos), Some(vel)) = (positions.get_mut(&id), velocities.get_mut(&id)) {
                pos.x += vel.x * dt;
                pos.y += vel.y * dt;
                vel.x *= damping;
                vel.y *= damping;
            }
        }

        for _ in 0..POSITION_SOLVER_ITERS {
            for i in 0..ids.len() {
                for j in (i + 1)..ids.len() {
                    let a = ids[i];
                    let b = ids[j];

                    let Some(na) = self.model.field.node(a) else {
                        continue;
                    };
                    let Some(nb) = self.model.field.node(b) else {
                        continue;
                    };
                    if !self.nodes_share_overlap_group(a, b) {
                        continue;
                    }

                    let a_pinned =
                        na.pinned || self.input.interaction_state.resize_static_node == Some(a);
                    let b_pinned =
                        nb.pinned || self.input.interaction_state.resize_static_node == Some(b);
                    let inv_mass_a = self.physics_inv_mass(a, a_pinned);
                    let inv_mass_b = self.physics_inv_mass(b, b_pinned);
                    if inv_mass_a <= 0.0 && inv_mass_b <= 0.0 {
                        continue;
                    }

                    let Some(a_pos) = positions.get(&a).copied() else {
                        continue;
                    };
                    let Some(b_pos) = positions.get(&b).copied() else {
                        continue;
                    };

                    let ea = self.layout_collision_extents_for_node(na);
                    let eb = self.layout_collision_extents_for_node(nb);
                    let dx = b_pos.x - a_pos.x;
                    let dy = b_pos.y - a_pos.y;
                    let req_x = self.required_sep_x(a_pos.x, ea, b_pos.x, eb, gap);
                    let req_y = self.required_sep_y(a_pos.y, ea, b_pos.y, eb, gap);
                    let gap_x = dx.abs() - req_x;
                    let gap_y = dy.abs() - req_y;
                    if gap_x > CONTACT_SKIN || gap_y > CONTACT_SKIN {
                        continue;
                    }

                    let solve_x = gap_x >= gap_y;
                    let normal = if solve_x {
                        Vec2 {
                            x: if dx.abs() > f32::EPSILON {
                                dx.signum()
                            } else if a.as_u64() < b.as_u64() {
                                -1.0
                            } else {
                                1.0
                            },
                            y: 0.0,
                        }
                    } else {
                        Vec2 {
                            x: 0.0,
                            y: if dy.abs() > f32::EPSILON {
                                dy.signum()
                            } else {
                                1.0
                            },
                        }
                    };

                    let penetration = if solve_x {
                        (-gap_x).max(0.0)
                    } else {
                        (-gap_y).max(0.0)
                    };
                    if penetration > 0.0 {
                        let correction = (penetration + CONTACT_SLOP).min(MAX_POSITION_CORRECTION);
                        let total_inv = inv_mass_a + inv_mass_b;
                        if total_inv > 0.0 {
                            let move_a = correction * (inv_mass_a / total_inv);
                            let move_b = correction * (inv_mass_b / total_inv);
                            if let Some(pos) = positions.get_mut(&a) {
                                pos.x -= normal.x * move_a;
                                pos.y -= normal.y * move_a;
                            }
                            if let Some(pos) = positions.get_mut(&b) {
                                pos.x += normal.x * move_b;
                                pos.y += normal.y * move_b;
                            }
                        }
                    }

                    let Some(va) = velocities.get(&a).copied() else {
                        continue;
                    };
                    let Some(vb) = velocities.get(&b).copied() else {
                        continue;
                    };
                    let rel_x = vb.x - va.x;
                    let rel_y = vb.y - va.y;
                    let rel_normal = rel_x * normal.x + rel_y * normal.y;
                    if rel_normal >= 0.0 {
                        continue;
                    }

                    let total_inv = inv_mass_a + inv_mass_b;
                    if total_inv <= 0.0 {
                        continue;
                    }

                    let normal_impulse = (-(1.0 + CONTACT_RESTITUTION) * rel_normal / total_inv)
                        .min(MAX_CONTACT_IMPULSE)
                        .max(0.0);
                    let impulse_x = normal.x * normal_impulse;
                    let impulse_y = normal.y * normal_impulse;

                    if let Some(vel) = velocities.get_mut(&a) {
                        vel.x -= impulse_x * inv_mass_a;
                        vel.y -= impulse_y * inv_mass_a;
                    }
                    if let Some(vel) = velocities.get_mut(&b) {
                        vel.x += impulse_x * inv_mass_b;
                        vel.y += impulse_y * inv_mass_b;
                    }

                    let tangent_x = rel_x - normal.x * rel_normal;
                    let tangent_y = rel_y - normal.y * rel_normal;
                    let tangent_len = (tangent_x * tangent_x + tangent_y * tangent_y).sqrt();
                    if tangent_len <= f32::EPSILON {
                        continue;
                    }
                    let tx = tangent_x / tangent_len;
                    let ty = tangent_y / tangent_len;
                    let rel_tangent = rel_x * tx + rel_y * ty;
                    let friction_impulse = (-rel_tangent / total_inv).clamp(
                        -CONTACT_FRICTION * normal_impulse,
                        CONTACT_FRICTION * normal_impulse,
                    );
                    let friction_x = tx * friction_impulse;
                    let friction_y = ty * friction_impulse;

                    if let Some(vel) = velocities.get_mut(&a) {
                        vel.x -= friction_x * inv_mass_a;
                        vel.y -= friction_y * inv_mass_a;
                    }
                    if let Some(vel) = velocities.get_mut(&b) {
                        vel.x += friction_x * inv_mass_b;
                        vel.y += friction_y * inv_mass_b;
                    }
                }
            }
        }

        for id in ids {
            let Some(node) = self.model.field.node(id) else {
                continue;
            };
            let pinned = node.pinned || self.input.interaction_state.resize_static_node == Some(id);
            // Don't write physics position back to the grabbed window —
            // carry_surface_non_overlap owns its position each frame.
            if self.input.interaction_state.drag_authority_node != Some(id) {
                if let Some(pos) = positions.get(&id).copied() {
                    let _ = if node.kind == halley_core::field::NodeKind::Core {
                        self.model.field.carry_cluster_by_core(id, pos)
                    } else {
                        self.model.field.carry(id, pos)
                    };
                }
            }
            if self.physics_inv_mass(id, pinned) <= 0.0 {
                continue;
            }
            let vel = Self::clamp_speed(
                velocities
                    .get(&id)
                    .copied()
                    .unwrap_or(Vec2 { x: 0.0, y: 0.0 }),
                MAX_PHYSICS_SPEED,
            );
            if vel.x.abs() < PHYSICS_REST_EPSILON && vel.y.abs() < PHYSICS_REST_EPSILON {
                self.input.interaction_state.physics_velocity.remove(&id);
            } else {
                self.input
                    .interaction_state
                    .physics_velocity
                    .insert(id, vel);
            }
        }
    }

    pub(super) fn request_toplevel_resize(&mut self, node_id: NodeId, width: i32, height: i32) {
        let width = width.max(96);
        let height = height.max(72);
        let focused_node = self.last_input_surface_node();

        for top in self.platform.xdg_shell_state.toplevel_surfaces() {
            let wl = top.wl_surface();
            let key = wl.id();

            if self.model.surface_to_node.get(&key).copied() != Some(node_id) {
                continue;
            }

            top.with_pending_state(|s| {
                s.size = Some((width, height).into());
                if focused_node == Some(node_id) {
                    s.states.set(xdg_toplevel::State::Activated);
                } else {
                    s.states.unset(xdg_toplevel::State::Activated);
                }
                self.apply_toplevel_tiled_hint(s);
            });
            top.send_configure();
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::utils::node_render_diameter_px;

    fn overlap_metrics(state: &Halley, a: NodeId, b: NodeId) -> (f32, f32, f32, f32) {
        let na = state.model.field.node(a).expect("node a");
        let nb = state.model.field.node(b).expect("node b");
        let ea = state.collision_extents_for_node(na);
        let eb = state.collision_extents_for_node(nb);
        let gap = state.non_overlap_gap_world();
        let dx = (nb.pos.x - na.pos.x).abs();
        let dy = (nb.pos.y - na.pos.y).abs();
        let req_x = state.required_sep_x(na.pos.x, ea, nb.pos.x, eb, gap);
        let req_y = state.required_sep_y(na.pos.y, ea, nb.pos.y, eb, gap);
        (dx, dy, req_x, req_y)
    }

    fn nodes_overlap(state: &Halley, a: NodeId, b: NodeId) -> bool {
        let (dx, dy, req_x, req_y) = overlap_metrics(state, a, b);
        dx < req_x && dy < req_y
    }

    fn tick_overlap_frames(state: &mut Halley, frames: usize) {
        for _ in 0..frames {
            state.resolve_surface_overlap();
        }
    }

    #[test]
    fn collapsed_surface_nodes_use_marker_collision_extents() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.model.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let id = state.model.field.spawn_surface(
            "collapsed-firefox",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 1200.0,
                y: 900.0,
            },
        );
        let _ = state
            .model
            .field
            .set_state(id, halley_core::field::NodeState::Node);

        let node = state.model.field.node(id).expect("node");
        let ext = state.collision_extents_for_node(node);

        assert!(
            ext.left + ext.right < 300.0,
            "collapsed node collision width should stay marker-sized, got {:?}",
            ext
        );
        assert!(
            ext.top + ext.bottom < 120.0,
            "collapsed node collision height should stay marker-sized, got {:?}",
            ext
        );
    }

    #[test]
    fn collapsed_surface_nodes_match_rendered_node_diameter() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.model.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let id = state.model.field.spawn_surface(
            "collapsed-firefox",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 1200.0,
                y: 900.0,
            },
        );
        let _ = state
            .model
            .field
            .set_state(id, halley_core::field::NodeState::Node);

        let node = state.model.field.node(id).expect("node");
        let ext = state.collision_extents_for_node(node);
        let anim = state.anim_style_for(id, node.state.clone(), Instant::now());
        let expected =
            node_render_diameter_px(&state, node.intrinsic_size, node.label.len(), anim.scale);

        assert_eq!(ext.left + ext.right, expected.round());
        assert_eq!(ext.top + ext.bottom, expected.round());
    }

    #[test]
    fn resolve_overlap_settles_collapsed_nodes_when_zoomed_out() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.model.zoom_ref_size = Vec2 {
            x: 3200.0,
            y: 2400.0,
        };

        let a = state.model.field.spawn_surface(
            "alpha",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        let b = state.model.field.spawn_surface(
            "beta",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        let _ = state
            .model
            .field
            .set_state(a, halley_core::field::NodeState::Node);
        let _ = state
            .model
            .field
            .set_state(b, halley_core::field::NodeState::Node);

        tick_overlap_frames(&mut state, 64);

        let (dx, dy, req_x, req_y) = overlap_metrics(&state, a, b);

        assert!(
            dx >= req_x || dy >= req_y,
            "collapsed nodes still overlap after zoomed-out settle: a={:?} b={:?} req=({}, {})",
            state.model.field.node(a).expect("node a").pos,
            state.model.field.node(b).expect("node b").pos,
            req_x,
            req_y
        );
    }

    #[test]
    fn overlap_resolution_is_not_limited_to_current_monitor() {
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

        let a = state.model.field.spawn_surface(
            "right-a",
            Vec2 {
                x: 1200.0,
                y: 300.0,
            },
            Vec2 { x: 320.0, y: 220.0 },
        );
        let b = state.model.field.spawn_surface(
            "right-b",
            Vec2 {
                x: 1200.0,
                y: 300.0,
            },
            Vec2 { x: 320.0, y: 220.0 },
        );
        state.assign_node_to_monitor(a, "right");
        state.assign_node_to_monitor(b, "right");
        let _ = state
            .model
            .field
            .set_state(a, halley_core::field::NodeState::Node);
        let _ = state
            .model
            .field
            .set_state(b, halley_core::field::NodeState::Node);

        tick_overlap_frames(&mut state, 64);

        assert!(
            !nodes_overlap(&state, a, b),
            "right-monitor overlap should resolve even while current monitor is left"
        );
    }

    #[test]
    fn dragged_window_is_authoritative_while_neighbor_yields() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.model.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let active = state.model.field.spawn_surface(
            "active",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 400.0, y: 260.0 },
        );
        let node = state.model.field.spawn_surface(
            "collapsed",
            Vec2 { x: 600.0, y: 0.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        let _ = state
            .model
            .field
            .set_state(node, halley_core::field::NodeState::Node);

        state.set_drag_authority_node(Some(node));
        assert!(state.carry_surface_non_overlap(node, Vec2 { x: 0.0, y: 0.0 }, false));
        state.resolve_surface_overlap();

        let active_node = state.model.field.node(active).expect("active surface");
        let collapsed_node = state.model.field.node(node).expect("collapsed node");

        assert!(
            collapsed_node.pos == Vec2 { x: 0.0, y: 0.0 },
            "dragged window moved away from the cursor-driven position: {:?}",
            collapsed_node.pos
        );
        assert!(
            active_node.pos != Vec2 { x: 0.0, y: 0.0 },
            "passive neighbor did not yield while dragged window remained authoritative"
        );
    }

    #[test]
    fn dragged_window_pushes_collapsed_core_and_members_follow() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.model.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let dragged = state.model.field.spawn_surface(
            "dragged",
            Vec2 { x: 400.0, y: 0.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        let a = state.model.field.spawn_surface(
            "a",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        let b = state.model.field.spawn_surface(
            "b",
            Vec2 { x: 20.0, y: 0.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        let cid = state
            .model
            .field
            .create_cluster(vec![a, b])
            .expect("cluster");
        let core = state.model.field.collapse_cluster(cid).expect("core");

        let core_before = state.model.field.node(core).expect("core before").pos;
        let a_before = state.model.field.node(a).expect("a before").pos;
        let b_before = state.model.field.node(b).expect("b before").pos;

        state.set_drag_authority_node(Some(dragged));
        assert!(state.carry_surface_non_overlap(dragged, Vec2 { x: 0.0, y: 0.0 }, false));
        state.resolve_surface_overlap();

        let dragged_after = state.model.field.node(dragged).expect("dragged after");
        let core_after = state.model.field.node(core).expect("core after");
        let a_after = state.model.field.node(a).expect("a after");
        let b_after = state.model.field.node(b).expect("b after");

        assert_eq!(
            dragged_after.pos,
            Vec2 { x: 0.0, y: 0.0 },
            "dragged window should stay authoritative"
        );
        assert!(
            core_after.pos != core_before,
            "collapsed core did not yield under physics"
        );
        assert_eq!(
            a_after.pos, a_before,
            "collapsed cluster members should not be repositioned by field physics"
        );
        assert_eq!(
            b_after.pos, b_before,
            "collapsed cluster members should not be repositioned by field physics"
        );
    }

    #[test]
    fn active_surface_collision_extents_include_frame_pad() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let id = state.model.field.spawn_surface(
            "active",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 400.0, y: 260.0 },
        );
        let node = state.model.field.node(id).expect("active node");
        let ext = state.surface_window_collision_extents(node);
        let expected_half_w = node.intrinsic_size.x * 0.5 + ACTIVE_WINDOW_FRAME_PAD_PX as f32;
        let expected_half_h = node.intrinsic_size.y * 0.5 + ACTIVE_WINDOW_FRAME_PAD_PX as f32;

        assert_eq!(ext.left, expected_half_w);
        assert_eq!(ext.right, expected_half_w);
        assert_eq!(ext.top, expected_half_h);
        assert_eq!(ext.bottom, expected_half_h);
    }

    #[test]
    fn resolve_overlap_settles_collapsed_nodes() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.model.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let a = state.model.field.spawn_surface(
            "alpha",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        let b = state.model.field.spawn_surface(
            "beta",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        let _ = state
            .model
            .field
            .set_state(a, halley_core::field::NodeState::Node);
        let _ = state
            .model
            .field
            .set_state(b, halley_core::field::NodeState::Node);

        tick_overlap_frames(&mut state, 64);

        let (dx, dy, req_x, req_y) = overlap_metrics(&state, a, b);

        assert!(
            dx >= req_x || dy >= req_y,
            "collapsed nodes still overlap after settle: a={:?} b={:?} req=({}, {})",
            state.model.field.node(a).expect("node a").pos,
            state.model.field.node(b).expect("node b").pos,
            req_x,
            req_y
        );
    }

    #[test]
    fn resolve_overlap_settles_active_surface_and_node() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.model.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let active = state.model.field.spawn_surface(
            "active",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        let node = state.model.field.spawn_surface(
            "node",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 300.0, y: 200.0 },
        );
        let _ = state
            .model
            .field
            .set_state(node, halley_core::field::NodeState::Node);

        tick_overlap_frames(&mut state, 96);

        let (dx, dy, req_x, req_y) = overlap_metrics(&state, active, node);

        assert!(
            dx >= req_x || dy >= req_y,
            "active surface and node still overlap after settle: active={:?} node={:?} req=({}, {})",
            state.model.field.node(active).expect("active").pos,
            state.model.field.node(node).expect("node").pos,
            req_x,
            req_y
        );
    }

    #[test]
    fn resolve_overlap_settles_two_active_surfaces() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.model.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let a = state.model.field.spawn_surface(
            "a",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        let b = state.model.field.spawn_surface(
            "b",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );

        tick_overlap_frames(&mut state, 128);

        let (dx, dy, req_x, req_y) = overlap_metrics(&state, a, b);

        assert!(
            dx >= req_x || dy >= req_y,
            "active surfaces still overlap after settle: a={:?} b={:?} req=({}, {})",
            state.model.field.node(a).expect("a").pos,
            state.model.field.node(b).expect("b").pos,
            req_x,
            req_y
        );
    }

    #[test]
    fn body_velocity_is_bounded_under_contact() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let a = state.model.field.spawn_surface(
            "a",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        let b = state.model.field.spawn_surface(
            "b",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );

        for _ in 0..12 {
            state.resolve_surface_overlap();
            let vel_a = state
                .input
                .interaction_state
                .physics_velocity
                .get(&a)
                .copied()
                .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
            let vel_b = state
                .input
                .interaction_state
                .physics_velocity
                .get(&b)
                .copied()
                .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
            assert!(
                vel_a.x.abs() <= MAX_PHYSICS_SPEED
                    && vel_a.y.abs() <= MAX_PHYSICS_SPEED
                    && vel_b.x.abs() <= MAX_PHYSICS_SPEED
                    && vel_b.y.abs() <= MAX_PHYSICS_SPEED,
                "contact solver exceeded the velocity bound: vel_a={vel_a:?} vel_b={vel_b:?}"
            );
        }
    }

    #[test]
    fn angled_drag_contact_does_not_create_unbounded_velocity() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let passive = state.model.field.spawn_surface(
            "passive",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        let dragged = state.model.field.spawn_surface(
            "dragged",
            Vec2 {
                x: -420.0,
                y: -280.0,
            },
            Vec2 { x: 320.0, y: 220.0 },
        );

        state.set_drag_authority_node(Some(dragged));
        for step in 0..48 {
            let to = Vec2 {
                x: -180.0 + step as f32 * 9.0,
                y: -120.0 + step as f32 * 5.5,
            };
            let _ = state.carry_surface_non_overlap(dragged, to, false);
            state.resolve_surface_overlap();
            let vel = state
                .input
                .interaction_state
                .physics_velocity
                .get(&passive)
                .copied()
                .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
            assert!(
                vel.x.abs() <= MAX_PHYSICS_SPEED && vel.y.abs() <= MAX_PHYSICS_SPEED,
                "passive window velocity exceeded the configured cap during angled drag: {vel:?}"
            );
        }
    }

    #[test]
    fn release_clears_grabbed_window_momentum() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let id = state.model.field.spawn_surface(
            "release",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        state
            .input
            .interaction_state
            .physics_velocity
            .insert(id, Vec2 { x: 480.0, y: 120.0 });
        state.finalize_mouse_drag_state(id, Vec2 { x: 0.0, y: 0.0 }, Instant::now());

        assert!(
            !state
                .input
                .interaction_state
                .physics_velocity
                .contains_key(&id),
            "grabbed window should not retain momentum after release"
        );
    }

    #[test]
    fn direct_border_hit_triggers_physics_response() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let a = state.model.field.spawn_surface(
            "a",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        let b = state.model.field.spawn_surface(
            "b",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        let ea = state.collision_extents_for_node(state.model.field.node(a).expect("a"));
        let eb = state.collision_extents_for_node(state.model.field.node(b).expect("b"));
        let req_x = state.required_sep_x(0.0, ea, 1.0, eb, state.non_overlap_gap_world());
        let _ = state.model.field.carry(b, Vec2 { x: req_x, y: 0.0 });
        state
            .input
            .interaction_state
            .physics_velocity
            .insert(a, Vec2 { x: 320.0, y: 0.0 });
        state
            .input
            .interaction_state
            .physics_velocity
            .insert(b, Vec2 { x: 0.0, y: 0.0 });

        state.resolve_surface_overlap();

        let vb = state
            .input
            .interaction_state
            .physics_velocity
            .get(&b)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
        assert!(
            vb.x > 0.0,
            "gap==0 border contact failed to produce a physics response: vb={vb:?}"
        );
    }

    #[test]
    fn grabbed_window_kinematic_velocity_pushes_neighbor_without_retaining_momentum() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let dragged = state.model.field.spawn_surface(
            "dragged",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        let passive = state.model.field.spawn_surface(
            "passive",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        let ea =
            state.collision_extents_for_node(state.model.field.node(dragged).expect("dragged"));
        let eb =
            state.collision_extents_for_node(state.model.field.node(passive).expect("passive"));
        let req_x = state.required_sep_x(0.0, ea, 1.0, eb, state.non_overlap_gap_world());
        let _ = state.model.field.carry(
            passive,
            Vec2 {
                x: req_x - 1.0,
                y: 0.0,
            },
        );

        state.set_drag_authority_node(Some(dragged));
        state.input.interaction_state.drag_authority_velocity = Vec2 { x: 420.0, y: 0.0 };

        state.resolve_surface_overlap();

        let passive_velocity = state
            .input
            .interaction_state
            .physics_velocity
            .get(&passive)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
        assert!(
            passive_velocity.x > 0.0,
            "passive window should receive physics from a grabbed kinematic collider: {passive_velocity:?}"
        );
        assert!(
            !state
                .input
                .interaction_state
                .physics_velocity
                .contains_key(&dragged),
            "grabbed window should not retain physics momentum"
        );
    }

    #[test]
    fn windows_settle_back_to_rest_after_contact_clears() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let a = state.model.field.spawn_surface(
            "a",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        let b = state.model.field.spawn_surface(
            "b",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );

        tick_overlap_frames(&mut state, 12);
        let _ = state.carry_surface_non_overlap(b, Vec2 { x: 700.0, y: 0.0 }, false);
        tick_overlap_frames(&mut state, 24);

        let va = state
            .input
            .interaction_state
            .physics_velocity
            .get(&a)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
        let vb = state
            .input
            .interaction_state
            .physics_velocity
            .get(&b)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 });

        assert!(
            va.x.abs() <= PHYSICS_REST_EPSILON
                && va.y.abs() <= PHYSICS_REST_EPSILON
                && vb.x.abs() <= PHYSICS_REST_EPSILON
                && vb.y.abs() <= PHYSICS_REST_EPSILON,
            "windows failed to settle back to rest after overlap cleared: va={va:?} vb={vb:?}"
        );
        assert!(
            !nodes_overlap(&state, a, b),
            "windows still overlap after the settling phase"
        );
    }
}
