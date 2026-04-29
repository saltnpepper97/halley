mod carry;
mod resolve;
#[cfg(test)]
mod tests;

use super::*;
use crate::compositor::overlap::read::OverlapReadContext;
use crate::frame_loop::anim_style_for;

pub(crate) use crate::compositor::overlap::read::CollisionExtents;
pub(crate) use carry::carry_surface_non_overlap;
pub(crate) use resolve::{request_toplevel_resize, resolve_surface_overlap};

fn overlap_read_context(st: &Halley) -> OverlapReadContext<'_> {
    OverlapReadContext {
        field: &st.model.field,
        monitor_state: &st.model.monitor_state,
        interaction_state: &st.input.interaction_state,
        spawn_state: &st.model.spawn_state,
        render_state: &st.ui.render_state,
        workspace_state: &st.model.workspace_state,
        tuning: &st.runtime.tuning,
        viewport: st.model.viewport,
        camera_render_scale: st.camera_render_scale(),
    }
}

#[inline]
fn clamp_speed(v: Vec2, max_speed: f32) -> Vec2 {
    OverlapReadContext::clamp_speed(v, max_speed)
}

#[inline]
fn physics_damping_per_sec(st: &Halley) -> f32 {
    overlap_read_context(st).physics_damping_per_sec()
}

#[inline]
fn physics_inv_mass(st: &Halley, id: NodeId, pinned: bool) -> f32 {
    overlap_read_context(st).physics_inv_mass(id, pinned)
}

#[inline]
fn node_participates_in_overlap(st: &Halley, id: NodeId) -> bool {
    overlap_read_context(st).node_participates_in_overlap(id)
}

pub(crate) fn non_overlap_gap_world(st: &Halley) -> f32 {
    overlap_read_context(st).non_overlap_gap_world()
}

#[inline]
pub(crate) fn required_sep_x(
    st: &Halley,
    a_pos_x: f32,
    a_ext: CollisionExtents,
    b_pos_x: f32,
    b_ext: CollisionExtents,
    gap: f32,
) -> f32 {
    overlap_read_context(st).required_sep_x(a_pos_x, a_ext, b_pos_x, b_ext, gap)
}

#[inline]
fn nodes_share_overlap_group(st: &Halley, a: NodeId, b: NodeId) -> bool {
    overlap_read_context(st).nodes_share_overlap_group(a, b)
}

#[inline]
pub(crate) fn required_sep_y(
    st: &Halley,
    a_pos_y: f32,
    a_ext: CollisionExtents,
    b_pos_y: f32,
    b_ext: CollisionExtents,
    gap: f32,
) -> f32 {
    overlap_read_context(st).required_sep_y(a_pos_y, a_ext, b_pos_y, b_ext, gap)
}

pub(super) fn carry_overlap_node_direct(st: &mut Halley, id: NodeId, to: Vec2) -> bool {
    let deliberate_pointer_move = st.input.interaction_state.drag_authority_node == Some(id);
    let was_pinned = st.model.field.node(id).is_some_and(|node| node.pinned);
    if deliberate_pointer_move && was_pinned {
        let _ = st.model.field.set_pinned(id, false);
    }

    let moved = if st
        .model
        .field
        .node(id)
        .is_some_and(|node| node.kind == halley_core::field::NodeKind::Core)
    {
        st.model.field.carry_cluster_by_core(id, to)
    } else {
        st.model.field.carry(id, to)
    };

    if deliberate_pointer_move && was_pinned {
        let _ = st.model.field.set_pinned(id, true);
    }

    moved
}

pub(crate) fn surface_window_collision_extents(
    st: &Halley,
    n: &halley_core::field::Node,
) -> CollisionExtents {
    overlap_read_context(st).surface_window_collision_extents(n)
}

pub(crate) fn spawn_obstacle_extents_for_node(
    st: &Halley,
    n: &halley_core::field::Node,
) -> CollisionExtents {
    if n.kind == halley_core::field::NodeKind::Surface {
        overlap_read_context(st).spawn_obstacle_extents_for_node(n)
    } else {
        collision_extents_for_node(st, n)
    }
}

pub(crate) fn collision_extents_for_node(
    st: &Halley,
    n: &halley_core::field::Node,
) -> CollisionExtents {
    let anim = anim_style_for(st, n.id, n.state.clone(), Instant::now());
    match n.state {
        halley_core::field::NodeState::Active => {
            let basis = st
                .model
                .workspace_state
                .last_active_size
                .get(&n.id)
                .copied()
                .unwrap_or(n.intrinsic_size);
            let s = OverlapReadContext::active_collision_scale(anim.scale, basis.x, basis.y);
            let ext = overlap_read_context(st).surface_window_collision_extents(n);

            CollisionExtents {
                left: ext.left * s,
                right: ext.right * s,
                top: ext.top * s,
                bottom: ext.bottom * s,
            }
        }
        halley_core::field::NodeState::Node => {
            overlap_read_context(st).node_collision_extents(n.intrinsic_size, &n.label, anim.scale)
        }
        halley_core::field::NodeState::Core => {
            overlap_read_context(st).core_node_collision_extents()
        }
        halley_core::field::NodeState::Drifting => CollisionExtents::symmetric(n.footprint),
    }
}

pub(crate) fn collision_size_for_node(st: &Halley, n: &halley_core::field::Node) -> Vec2 {
    collision_extents_for_node(st, n).size()
}

pub(super) fn layout_collision_extents_for_node(
    st: &Halley,
    n: &halley_core::field::Node,
) -> CollisionExtents {
    match n.state {
        halley_core::field::NodeState::Node | halley_core::field::NodeState::Core => {
            collision_extents_for_node(st, n)
        }
        _ => collision_extents_for_node(st, n),
    }
}
