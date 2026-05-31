use halley_core::field::{NodeId, Vec2};

use crate::compositor::root::Halley;

use super::{
    CollisionExtents, carry_overlap_node_direct, collision_extents_for_node,
    mixed_expanded_landmark_locks, node_is_expanded_window, node_is_landmark,
    node_participates_in_drag_overlap, node_participates_in_overlap, nodes_share_overlap_group,
    non_overlap_gap_world, required_sep_x, required_sep_y,
};

fn carry_candidate_ids(st: &Halley) -> Vec<NodeId> {
    if st.input.interaction_state.drag_authority_node.is_some() {
        st.model.field.node_ids_all()
    } else {
        st.model.field.nodes().keys().copied().collect()
    }
}

fn candidate_participates(st: &Halley, id: NodeId) -> bool {
    if st.input.interaction_state.drag_authority_node.is_some() {
        node_participates_in_drag_overlap(st, id)
    } else {
        node_participates_in_overlap(st, id)
    }
}

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
        carry_overlap_node_direct(st, id, clamp_against_locked_neighbors(st, id, to))
    }
}

fn clamp_against_locked_neighbors(st: &Halley, id: NodeId, to: Vec2) -> Vec2 {
    let Some(n) = st.model.field.node(id) else {
        return to;
    };

    let mover_ext = collision_extents_for_node(st, n);
    let gap = non_overlap_gap_world(st);
    let physics_dragged_landmark = st.runtime.tuning.physics_enabled
        && st.input.interaction_state.drag_authority_node == Some(id)
        && node_is_landmark(st, id);
    let mut mover_pos = if physics_dragged_landmark {
        to
    } else {
        clamp_landmark_sweep_against_expanded(st, id, n.pos, to, mover_ext, gap)
    };

    for _ in 0..24 {
        let locked_others: Vec<(NodeId, Vec2, CollisionExtents)> = carry_candidate_ids(st)
            .into_iter()
            .filter_map(|oid| {
                let other = st.model.field.node(oid)?;
                if oid == id
                    || !candidate_participates(st, oid)
                    || !nodes_share_overlap_group(st, id, oid)
                    || !(other.pinned
                        || st.input.interaction_state.resize_static_node == Some(oid)
                        || (!physics_dragged_landmark
                            && node_is_landmark(st, id)
                            && node_is_expanded_window(st, oid)))
                {
                    return None;
                }
                Some((oid, other.pos, collision_extents_for_node(st, other)))
            })
            .collect();

        let mut changed = false;
        for (oid, opos, oext) in locked_others {
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

    mover_pos
}

fn clamp_landmark_sweep_against_expanded(
    st: &Halley,
    id: NodeId,
    from: Vec2,
    to: Vec2,
    mover_ext: CollisionExtents,
    gap: f32,
) -> Vec2 {
    if !node_is_landmark(st, id) {
        return to;
    }
    let mut out = to;
    for oid in carry_candidate_ids(st) {
        let Some(other) = st.model.field.node(oid) else {
            continue;
        };
        if oid == id
            || !candidate_participates(st, oid)
            || !nodes_share_overlap_group(st, id, oid)
            || !node_is_expanded_window(st, oid)
        {
            continue;
        }
        let oext = collision_extents_for_node(st, other);
        let req_x = required_sep_x(st, out.x, mover_ext, other.pos.x, oext, gap);
        let req_y = required_sep_y(st, out.y, mover_ext, other.pos.y, oext, gap);
        let vertical_overlap = (out.y - other.pos.y).abs() < req_y;
        let horizontal_overlap = (out.x - other.pos.x).abs() < req_x;

        if !(vertical_overlap && horizontal_overlap) {
            continue;
        }

        match landmark_contact_side(from, to, other.pos, req_x, req_y) {
            ContactSide::Left => out.x = other.pos.x - req_x - 0.3,
            ContactSide::Right => out.x = other.pos.x + req_x + 0.3,
            ContactSide::Top => out.y = other.pos.y - req_y - 0.3,
            ContactSide::Bottom => out.y = other.pos.y + req_y + 0.3,
        }
    }
    out
}

#[derive(Clone, Copy)]
enum ContactSide {
    Left,
    Right,
    Top,
    Bottom,
}

fn landmark_contact_side(
    from: Vec2,
    to: Vec2,
    other_pos: Vec2,
    req_x: f32,
    req_y: f32,
) -> ContactSide {
    let from_dx = from.x - other_pos.x;
    let from_dy = from.y - other_pos.y;
    let x_outside = from_dx.abs() >= req_x;
    let y_outside = from_dy.abs() >= req_y;

    if y_outside && (!x_outside || from_dy.abs() / req_y.max(1.0) >= from_dx.abs() / req_x.max(1.0))
    {
        if to.y > other_pos.y {
            ContactSide::Bottom
        } else {
            ContactSide::Top
        }
    } else if x_outside {
        if to.x > other_pos.x {
            ContactSide::Right
        } else {
            ContactSide::Left
        }
    } else if (to.y - other_pos.y).abs() >= (to.x - other_pos.x).abs() {
        if to.y > other_pos.y {
            ContactSide::Bottom
        } else {
            ContactSide::Top
        }
    } else if to.x > other_pos.x {
        ContactSide::Right
    } else {
        ContactSide::Left
    }
}

fn carry_surface_no_overlap_split(st: &mut Halley, id: NodeId, to: Vec2) -> bool {
    let Some(n) = st.model.field.node(id) else {
        return false;
    };

    let mover_ext = collision_extents_for_node(st, n);
    let gap = non_overlap_gap_world(st);
    let mut mover_pos = clamp_landmark_sweep_against_expanded(st, id, n.pos, to, mover_ext, gap);

    for _ in 0..24 {
        let others: Vec<(NodeId, Vec2, CollisionExtents, bool)> = carry_candidate_ids(st)
            .into_iter()
            .filter_map(|oid| {
                let other = st.model.field.node(oid)?;
                if oid == id
                    || !candidate_participates(st, oid)
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
                let mover_expanded = node_is_expanded_window(st, id);
                let other_landmark = node_is_landmark(st, oid);
                let other_expanded = node_is_expanded_window(st, oid);
                let mover_landmark = node_is_landmark(st, id);
                let (mover_locked, other_locked) =
                    mixed_expanded_landmark_locks(st, id, oid, false, other_locked);
                if other_locked {
                    mover_pos.x += s * step;
                } else if mover_locked || (mover_expanded && other_landmark) {
                    let _ = carry_overlap_node_direct(
                        st,
                        oid,
                        Vec2 {
                            x: opos.x - s * step,
                            y: opos.y,
                        },
                    );
                } else if mover_landmark && other_expanded {
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
                let mover_expanded = node_is_expanded_window(st, id);
                let other_landmark = node_is_landmark(st, oid);
                let other_expanded = node_is_expanded_window(st, oid);
                let mover_landmark = node_is_landmark(st, id);
                let (mover_locked, other_locked) =
                    mixed_expanded_landmark_locks(st, id, oid, false, other_locked);
                if other_locked {
                    mover_pos.y += s * step;
                } else if mover_locked || (mover_expanded && other_landmark) {
                    let _ = carry_overlap_node_direct(
                        st,
                        oid,
                        Vec2 {
                            x: opos.x,
                            y: opos.y - s * step,
                        },
                    );
                } else if mover_landmark && other_expanded {
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
    let mut mover_pos = clamp_landmark_sweep_against_expanded(st, id, n.pos, to, mover_ext, gap);

    for _ in 0..24 {
        let others: Vec<(NodeId, Vec2, CollisionExtents)> = carry_candidate_ids(st)
            .into_iter()
            .filter_map(|oid| {
                let other = st.model.field.node(oid)?;
                if oid == id
                    || !candidate_participates(st, oid)
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
