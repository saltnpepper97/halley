use super::*;
use crate::compositor::overlap::physics::{
    CONTACT_SKIN, MAX_PHYSICS_SPEED, PHYSICS_REST_EPSILON, POSITION_SOLVER_ITERS,
    resolve_contact_pair,
};
pub(crate) use crate::compositor::overlap::read::CollisionExtents;
use crate::compositor::overlap::read::OverlapReadContext;
use crate::render::active_window_frame_pad_px;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;

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

pub(crate) fn carry_surface_non_overlap(
    st: &mut Halley,
    id: NodeId,
    to: Vec2,
    clamp_only: bool,
) -> bool {
    let carry_direct = |st: &mut Halley, id: NodeId, to: Vec2| {
        if st
            .model
            .field
            .node(id)
            .is_some_and(|node| node.kind == halley_core::field::NodeKind::Core)
        {
            st.model.field.carry_cluster_by_core(id, to)
        } else {
            st.model.field.carry(id, to)
        }
    };
    if !st.runtime.tuning.physics_enabled {
        carry_surface_no_overlap_static(st, id, to)
    } else if clamp_only
        || st.input.interaction_state.suspend_overlap_resolve
        || st.input.interaction_state.suspend_state_checks
    {
        carry_surface_no_overlap_static(st, id, to)
    } else {
        carry_direct(st, id, to)
    }
}

fn carry_surface_no_overlap_static(st: &mut Halley, id: NodeId, to: Vec2) -> bool {
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

    if st
        .model
        .field
        .node(id)
        .is_some_and(|node| node.kind == halley_core::field::NodeKind::Core)
    {
        st.model.field.carry_cluster_by_core(id, mover_pos)
    } else {
        st.model.field.carry(id, mover_pos)
    }
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
    let anim = crate::render::anim_style_for(st, n.id, n.state.clone(), Instant::now());
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
            let ext = overlap_read_context(st).active_surface_overlap_extents(n);

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
            overlap_read_context(st).node_collision_extents(n.intrinsic_size, &n.label, anim.scale)
        }
        halley_core::field::NodeState::Drifting => CollisionExtents::symmetric(n.footprint),
    }
}

pub(crate) fn collision_size_for_node(st: &Halley, n: &halley_core::field::Node) -> Vec2 {
    collision_extents_for_node(st, n).size()
}

fn layout_collision_extents_for_node(
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

pub(crate) fn resolve_surface_overlap(st: &mut Halley) {
    if !st.runtime.tuning.physics_enabled {
        return;
    }
    if st.input.interaction_state.suspend_overlap_resolve {
        return;
    }

    let mut ids: Vec<NodeId> = st
        .model
        .field
        .nodes()
        .keys()
        .copied()
        .filter(|&id| node_participates_in_overlap(st, id))
        .collect();

    if ids.is_empty() {
        return;
    }

    ids.sort_by_key(|id| id.as_u64());

    let now = Instant::now();
    let dt = now
        .saturating_duration_since(st.input.interaction_state.physics_last_tick)
        .as_secs_f32()
        .clamp(1.0 / 240.0, 1.0 / 30.0);
    st.input.interaction_state.physics_last_tick = now;

    let gap = non_overlap_gap_world(st);
    let damping_per_sec = physics_damping_per_sec(st);
    let damping = (-damping_per_sec * dt).exp();
    let mut positions: std::collections::HashMap<NodeId, Vec2> = std::collections::HashMap::new();
    let mut velocities: std::collections::HashMap<NodeId, Vec2> = std::collections::HashMap::new();

    for &id in &ids {
        let Some(node) = st.model.field.node(id) else {
            continue;
        };
        positions.insert(id, node.pos);
        let vel = if st.input.interaction_state.drag_authority_node == Some(id) {
            st.input.interaction_state.drag_authority_velocity
        } else {
            st.input
                .interaction_state
                .physics_velocity
                .get(&id)
                .copied()
                .unwrap_or(Vec2 { x: 0.0, y: 0.0 })
        };
        velocities.insert(id, clamp_speed(vel, MAX_PHYSICS_SPEED));
    }

    for &id in &ids {
        let Some(node) = st.model.field.node(id) else {
            continue;
        };
        let pinned = node.pinned || st.input.interaction_state.resize_static_node == Some(id);
        if physics_inv_mass(st, id, pinned) <= 0.0 {
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

                let Some(na) = st.model.field.node(a) else {
                    continue;
                };
                let Some(nb) = st.model.field.node(b) else {
                    continue;
                };
                if !nodes_share_overlap_group(st, a, b) {
                    continue;
                }

                let a_pinned =
                    na.pinned || st.input.interaction_state.resize_static_node == Some(a);
                let b_pinned =
                    nb.pinned || st.input.interaction_state.resize_static_node == Some(b);
                let inv_mass_a = physics_inv_mass(st, a, a_pinned);
                let inv_mass_b = physics_inv_mass(st, b, b_pinned);
                if inv_mass_a <= 0.0 && inv_mass_b <= 0.0 {
                    continue;
                }

                let Some(a_pos) = positions.get(&a).copied() else {
                    continue;
                };
                let Some(b_pos) = positions.get(&b).copied() else {
                    continue;
                };

                let ea = layout_collision_extents_for_node(st, na);
                let eb = layout_collision_extents_for_node(st, nb);
                let dx = b_pos.x - a_pos.x;
                let dy = b_pos.y - a_pos.y;
                let req_x = required_sep_x(st, a_pos.x, ea, b_pos.x, eb, gap);
                let req_y = required_sep_y(st, a_pos.y, ea, b_pos.y, eb, gap);
                let gap_x = dx.abs() - req_x;
                let gap_y = dy.abs() - req_y;
                if gap_x > CONTACT_SKIN || gap_y > CONTACT_SKIN {
                    continue;
                }

                resolve_contact_pair(
                    &mut positions,
                    &mut velocities,
                    a,
                    b,
                    dx,
                    dy,
                    gap_x,
                    gap_y,
                    inv_mass_a,
                    inv_mass_b,
                );
            }
        }
    }

    for id in ids {
        let Some(node) = st.model.field.node(id) else {
            continue;
        };
        let pinned = node.pinned || st.input.interaction_state.resize_static_node == Some(id);
        // Don't write physics position back to the grabbed window —
        // carry_surface_non_overlap owns its position each frame.
        if st.input.interaction_state.drag_authority_node != Some(id) {
            if let Some(pos) = positions.get(&id).copied() {
                let _ = if node.kind == halley_core::field::NodeKind::Core {
                    st.model.field.carry_cluster_by_core(id, pos)
                } else {
                    st.model.field.carry(id, pos)
                };
            }
        }
        if physics_inv_mass(st, id, pinned) <= 0.0 {
            continue;
        }
        let vel = clamp_speed(
            velocities
                .get(&id)
                .copied()
                .unwrap_or(Vec2 { x: 0.0, y: 0.0 }),
            MAX_PHYSICS_SPEED,
        );
        if vel.x.abs() < PHYSICS_REST_EPSILON && vel.y.abs() < PHYSICS_REST_EPSILON {
            st.input.interaction_state.physics_velocity.remove(&id);
        } else {
            st.input.interaction_state.physics_velocity.insert(id, vel);
        }
    }
}

pub(crate) fn request_toplevel_resize(st: &mut Halley, node_id: NodeId, width: i32, height: i32) {
    let (min_w, min_h) = crate::compositor::surface_ops::toplevel_min_size_for_node(st, node_id);
    let width = width.max(min_w).max(96);
    let height = height.max(min_h).max(72);
    let focused_node = st.last_input_surface_node();

    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();

        if st.model.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
        }

        top.with_pending_state(|s| {
            s.size = Some((width, height).into());
            if focused_node == Some(node_id) {
                s.states.set(xdg_toplevel::State::Activated);
            } else {
                s.states.unset(xdg_toplevel::State::Activated);
            }
            st.apply_toplevel_tiled_hint(s);
        });
        top.send_configure();
        break;
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
        let anim = crate::render::anim_style_for(&state, id, node.state.clone(), Instant::now());
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

        crate::compositor::carry::system::set_drag_authority_node(&mut state, Some(node));
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

        crate::compositor::carry::system::set_drag_authority_node(&mut state, Some(dragged));
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
        let expected_half_w =
            node.intrinsic_size.x * 0.5 + active_window_frame_pad_px(&state.runtime.tuning) as f32;
        let expected_half_h =
            node.intrinsic_size.y * 0.5 + active_window_frame_pad_px(&state.runtime.tuning) as f32;

        assert_eq!(ext.left, expected_half_w);
        assert_eq!(ext.right, expected_half_w);
        assert_eq!(ext.top, expected_half_h);
        assert_eq!(ext.bottom, expected_half_h);
    }

    #[test]
    fn surface_collision_extents_ignore_asymmetric_bbox_offsets() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let id = state.model.field.spawn_surface(
            "gtk-like",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 1200.0,
                y: 920.0,
            },
        );
        state.ui.render_state.bbox_loc.insert(id, (4.0, 6.0));
        state
            .ui
            .render_state
            .window_geometry
            .insert(id, (12.0, 18.0, 840.0, 620.0));

        let node = state.model.field.node(id).expect("surface node");
        let ext = state.surface_window_collision_extents(node);
        let expected_half_w = 420.0 + active_window_frame_pad_px(&state.runtime.tuning) as f32;
        let expected_half_h = 310.0 + active_window_frame_pad_px(&state.runtime.tuning) as f32;

        assert_eq!(ext.left, expected_half_w);
        assert_eq!(ext.right, expected_half_w);
        assert_eq!(ext.top, expected_half_h);
        assert_eq!(ext.bottom, expected_half_h);
    }

    #[test]
    fn active_overlap_extents_preserve_asymmetric_bbox_offsets() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let id = state.model.field.spawn_surface(
            "gtk-like",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 1200.0,
                y: 920.0,
            },
        );
        state.ui.render_state.bbox_loc.insert(id, (4.0, 6.0));
        state
            .ui
            .render_state
            .window_geometry
            .insert(id, (12.0, 18.0, 840.0, 620.0));

        let node = state.model.field.node(id).expect("surface node");
        let ext = state.collision_extents_for_node(node);

        assert!(
            ext.left > ext.right,
            "expected asymmetric x extents: {ext:?}"
        );
        assert!(
            ext.top > ext.bottom,
            "expected asymmetric y extents: {ext:?}"
        );
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

        crate::compositor::carry::system::set_drag_authority_node(&mut state, Some(dragged));
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
        crate::compositor::carry::system::finalize_mouse_drag_state(
            &mut state,
            id,
            Vec2 { x: 0.0, y: 0.0 },
            Instant::now(),
        );

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

        crate::compositor::carry::system::set_drag_authority_node(&mut state, Some(dragged));
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
