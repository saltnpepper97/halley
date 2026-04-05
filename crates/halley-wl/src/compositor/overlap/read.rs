use super::*;
use crate::animation::{ease_in_out_cubic, proxy_anim_scale};
use crate::compositor::interaction::state::InteractionState;
use crate::compositor::monitor::state::MonitorState;
use crate::compositor::spawn::state::SpawnState;
use crate::compositor::workspace::state::WorkspaceState;
use crate::render::state::RenderState;
use crate::render::{active_window_frame_pad_px, preview_proxy_size};
use halley_core::viewport::Viewport;

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
    pub(crate) fn size(self) -> Vec2 {
        Vec2 {
            x: (self.left + self.right).max(0.0),
            y: (self.top + self.bottom).max(0.0),
        }
    }
}

pub(super) struct OverlapReadContext<'a> {
    pub(super) field: &'a Field,
    pub(super) monitor_state: &'a MonitorState,
    pub(super) interaction_state: &'a InteractionState,
    pub(super) spawn_state: &'a SpawnState,
    pub(super) render_state: &'a RenderState,
    pub(super) workspace_state: &'a WorkspaceState,
    pub(super) tuning: &'a RuntimeTuning,
    pub(super) viewport: Viewport,
    pub(super) camera_render_scale: f32,
}

impl<'a> OverlapReadContext<'a> {
    #[inline]
    pub(crate) fn clamp_speed(v: Vec2, max_speed: f32) -> Vec2 {
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
    pub(crate) fn physics_damping_per_sec(&self) -> f32 {
        const LINEAR_DAMPING_FALLBACK_PER_SEC: f32 = 4.5;
        const USER_DAMPING_MIN: f32 = 0.0;
        const USER_DAMPING_MAX: f32 = 1.0;
        const INTERNAL_DAMPING_MIN_PER_SEC: f32 = 3.0;
        const INTERNAL_DAMPING_MAX_PER_SEC: f32 = 8.0;

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
    pub(crate) fn physics_inv_mass(&self, id: NodeId, pinned: bool) -> f32 {
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
    pub(crate) fn node_participates_in_overlap(&self, id: NodeId) -> bool {
        if !self.field.participates_in_field_dynamics(id) {
            return false;
        }
        if self.spawn_state.pending_initial_reveal.contains(&id) {
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
    pub(crate) fn nodes_share_overlap_group(&self, a: NodeId, b: NodeId) -> bool {
        if self.node_allows_overlap_with(a, b) || self.node_allows_overlap_with(b, a) {
            return false;
        }
        match (
            self.monitor_state.node_monitor.get(&a),
            self.monitor_state.node_monitor.get(&b),
        ) {
            (Some(a_monitor), Some(b_monitor)) => a_monitor == b_monitor,
            _ => true,
        }
    }

    fn node_allows_overlap_with(&self, id: NodeId, other: NodeId) -> bool {
        if matches!(
            self.tuning.cluster_layout_kind(),
            halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Stacking
        ) {
            return false;
        }
        let Some(rule) = self.spawn_state.applied_window_rules.get(&id) else {
            return false;
        };
        match rule.overlap_policy {
            halley_config::InitialWindowOverlapPolicy::None => false,
            halley_config::InitialWindowOverlapPolicy::ParentOnly => {
                rule.parent_node == Some(other)
            }
            halley_config::InitialWindowOverlapPolicy::All => true,
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

    pub(crate) fn active_collision_scale(anim_scale: f32, real_w: f32, real_h: f32) -> f32 {
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
        let frame_pad = active_window_frame_pad_px(self.tuning) as f32;
        let half_w = basis.x.max(1.0) * 0.5 + frame_pad;
        let half_h = basis.y.max(1.0) * 0.5 + frame_pad;

        CollisionExtents {
            left: half_w,
            right: half_w,
            top: half_h,
            bottom: half_h,
        }
    }

    pub(crate) fn active_surface_overlap_extents(
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
        let frame_pad = active_window_frame_pad_px(self.tuning) as f32;

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
