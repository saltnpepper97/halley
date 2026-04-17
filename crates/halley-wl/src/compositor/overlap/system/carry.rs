use halley_core::field::{NodeId, Vec2};

use crate::compositor::root::Halley;

use super::{
    CollisionExtents, carry_overlap_node_direct, collision_extents_for_node,
    node_participates_in_overlap, nodes_share_overlap_group, non_overlap_gap_world, required_sep_x,
    required_sep_y,
};

pub(crate) fn carry_surface_non_overlap(
    st: &mut Halley,
    id: NodeId,
    to: Vec2,
    clamp_only: bool,
) -> bool {
    if clamp_only
        || st.input.interaction_state.suspend_overlap_resolve
        || st.input.interaction_state.suspend_state_checks
    {
        carry_surface_no_overlap_clamped(st, id, to)
    } else if !st.runtime.tuning.physics_enabled {
        carry_surface_no_overlap_split(st, id, to)
    } else {
        carry_overlap_node_direct(st, id, to)
    }
}

fn carry_surface_no_overlap_split(st: &mut Halley, id: NodeId, to: Vec2) -> bool {
    let Some(n) = st.model.field.node(id) else {
        return false;
    };

    let mover_ext = collision_extents_for_node(st, n);
    let gap = non_overlap_gap_world(st);
    let mut mover_pos = to;

    for _ in 0..24 {
        let others: Vec<(NodeId, Vec2, CollisionExtents, bool)> = st
            .model
            .field
            .nodes()
            .iter()
            .filter_map(|(&oid, other)| {
                if oid == id
                    || !node_participates_in_overlap(st, oid)
                    || !nodes_share_overlap_group(st, id, oid)
                {
                    return None;
                }
                Some((
                    oid,
                    other.pos,
                    collision_extents_for_node(st, other),
                    other.pinned || st.input.interaction_state.resize_static_node == Some(oid),
                ))
            })
            .collect();

        let mut changed = false;

        for (oid, opos, oext, other_locked) in others {
            let dx = mover_pos.x - opos.x;
            let dy = mover_pos.y - opos.y;
            let req_x = required_sep_x(st, mover_pos.x, mover_ext, opos.x, oext, gap);
            let req_y = required_sep_y(st, mover_pos.y, mover_ext, opos.y, oext, gap);
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
                let step = ox + 0.3;
                if other_locked {
                    mover_pos.x += s * step;
                } else {
                    let half = s * (step * 0.5);
                    mover_pos.x += half;
                    let _ = carry_overlap_node_direct(
                        st,
                        oid,
                        Vec2 {
                            x: opos.x - half,
                            y: opos.y,
                        },
                    );
                }
            } else {
                let s = if dy.abs() > f32::EPSILON {
                    dy.signum()
                } else {
                    1.0
                };
                let step = oy + 0.3;
                if other_locked {
                    mover_pos.y += s * step;
                } else {
                    let half = s * (step * 0.5);
                    mover_pos.y += half;
                    let _ = carry_overlap_node_direct(
                        st,
                        oid,
                        Vec2 {
                            x: opos.x,
                            y: opos.y - half,
                        },
                    );
                }
            }

            changed = true;
        }

        if !changed {
            break;
        }
    }

    carry_overlap_node_direct(st, id, mover_pos)
}

fn carry_surface_no_overlap_clamped(st: &mut Halley, id: NodeId, to: Vec2) -> bool {
    let Some(n) = st.model.field.node(id) else {
        return false;
    };

    let mover_ext = collision_extents_for_node(st, n);
    let gap = non_overlap_gap_world(st);
    let mut mover_pos = to;

    for _ in 0..24 {
        let others: Vec<(NodeId, Vec2, CollisionExtents)> = st
            .model
            .field
            .nodes()
            .iter()
            .filter_map(|(&oid, other)| {
                if oid == id
                    || !node_participates_in_overlap(st, oid)
                    || !nodes_share_overlap_group(st, id, oid)
                {
                    return None;
                }
                Some((oid, other.pos, collision_extents_for_node(st, other)))
            })
            .collect();

        let mut changed = false;

        for (oid, opos, oext) in others {
            let dx = mover_pos.x - opos.x;
            let dy = mover_pos.y - opos.y;
            let req_x = required_sep_x(st, mover_pos.x, mover_ext, opos.x, oext, gap);
            let req_y = required_sep_y(st, mover_pos.y, mover_ext, opos.y, oext, gap);
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

    carry_overlap_node_direct(st, id, mover_pos)
}
