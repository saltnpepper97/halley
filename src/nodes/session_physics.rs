//! Coordinate node physics and landmark movement with the live session.

use super::*;

pub fn reconcile_landmarks<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    only_output: Option<&str>,
) {
    reconcile_landmarks_inner(session, only_output);
}

fn reconcile_landmarks_inner<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    only_output: Option<&str>,
) {
    session.nodes.sync_from_space(&session.wayland.space);
    let candidates = session
        .nodes
        .records()
        .filter(|record| {
            record.collapsed
                && record.attached
                && only_output.is_none_or(|output| record.output == output)
        })
        .filter_map(|record| {
            session
                .nodes
                .field
                .node(record.id)
                .map(|node| (record.id, record.output.clone(), node.pos))
        })
        .collect::<Vec<_>>();
    for (id, output, current) in candidates {
        let scale = session
            .cameras
            .get(&output)
            .map(crate::presentation::camera::scale)
            .unwrap_or(1.0);
        let occupied_cores = session
            .clusters
            .collapsed_core_landmarks()
            .into_iter()
            .filter_map(|(_, _, core_output, position, _)| {
                (core_output == output).then_some(position)
            })
            .collect::<Vec<_>>();
        let destination = session.nodes.nearest_free_position(
            id,
            current,
            scale,
            &occupied_cores,
            PlacementContext {
                decorations: &session.settings.decorations,
                font: &session.settings.font,
                fullscreen: &session.fullscreen,
                maximize: &session.maximize,
            },
        );
        if destination == current {
            continue;
        }
        // Placement reflow is a real move, not a reversible zoom offset: the
        // shared path commits this landmark's pre-zoom home as it applies.
        apply_landmark_move(
            session,
            id,
            destination,
            LandmarkMovement::Persistent,
            &output,
        );
    }
}

fn dynamics_bodies<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
) -> Vec<dynamics::Body> {
    dynamics_bodies_at_scale(session, None)
}

fn dynamics_bodies_at_scale<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    scale_override: Option<(&str, f32)>,
) -> Vec<dynamics::Body> {
    session.nodes.sync_from_space(&session.wayland.space);
    super::dynamics_inputs::collect_dynamics_bodies(
        super::dynamics_inputs::DynamicsBodyContext {
            nodes: &session.nodes,
            clusters: &session.clusters,
            cameras: &session.cameras,
            placement: PlacementContext {
                decorations: &session.settings.decorations,
                font: &session.settings.font,
                fullscreen: &session.fullscreen,
                maximize: &session.maximize,
            },
        },
        scale_override,
    )
}

/// Where a landmark move came from. Camera zoom may be reverted while the
/// presented footprint shrinks again; anything physical or user-driven rebases
/// the landmark permanently.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LandmarkMovement {
    ZoomTemporary,
    Persistent,
}

fn landmark_moved(origin: Vec2, destination: Vec2) -> bool {
    (destination.x - origin.x).abs() > dynamics::LANDMARK_POSITION_EPSILON
        || (destination.y - origin.y).abs() > dynamics::LANDMARK_POSITION_EPSILON
}

/// Applies one landmark destination to both cluster metadata and the Field
/// scene, preserving the slide animation and retargeting it from the position
/// actually on screen. Temporary zoom movement remembers the pre-zoom home on
/// the first displacement; persistent movement discards it, so anything a
/// physical or user-driven move touches is rebased together.
fn apply_landmark_move<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    destination: Vec2,
    movement: LandmarkMovement,
    output: &str,
) -> bool {
    let now = crate::frame_clock::monotonic_now();
    if let Some(cluster) = session.clusters.cluster_for_core(id) {
        let Some((current, core_output)) = session
            .clusters
            .metadata(cluster)
            .map(|metadata| (metadata.core_position, metadata.output.clone()))
        else {
            return false;
        };
        if !landmark_moved(destination, current) {
            return false;
        }
        let from = session.nodes.landmark_position(id, current, now);
        if !session
            .clusters
            .move_core(cluster, &core_output, destination)
        {
            return false;
        }
        if movement == LandmarkMovement::ZoomTemporary {
            session.nodes.remember_zoom_home(id, current);
        } else {
            session.nodes.commit_zoom_home(id);
        }
        if let Some(node) = session.nodes.field.node_mut(id) {
            node.pos = destination;
        }
        session.nodes.physics_velocity.remove(&id);
        session
            .nodes
            .start_landmark_slide(id, from, destination, now);
        return true;
    }

    let Some((current, collapsed, attached, node_output)) =
        session.nodes.record(id).and_then(|record| {
            session.nodes.field.node(id).map(|node| {
                (
                    node.pos,
                    record.collapsed,
                    record.attached,
                    record.output.clone(),
                )
            })
        })
    else {
        return false;
    };
    if !collapsed || !attached || node_output != output {
        return false;
    }
    if !landmark_moved(destination, current) {
        return false;
    }
    if movement == LandmarkMovement::ZoomTemporary {
        session.nodes.remember_zoom_home(id, current);
    } else {
        session.nodes.commit_zoom_home(id);
    }
    let from = session.nodes.landmark_position(id, current, now);
    if let Some(node) = session.nodes.field.node_mut(id) {
        node.pos = destination;
    }
    session.nodes.physics_velocity.remove(&id);
    session
        .nodes
        .start_landmark_slide(id, from, destination, now);
    true
}

/// Reconcile the fixed-pixel footprint of collapsed landmarks at a newly
/// presented zoom scale. This is the zoom-out direction: active windows are
/// stationary blockers, while ordinary nodes and collapsed cluster cores move
/// together under the same collision policy. The first displacement of each
/// landmark remembers its pre-zoom home so zooming back in can revert it.
pub(crate) fn reconcile_landmarks_for_zoom<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    output: &str,
    scale: f32,
) -> bool {
    let bodies = dynamics_bodies_at_scale(session, Some((output, scale)))
        .into_iter()
        .filter(|body| body.output == output)
        .collect::<Vec<_>>();
    let positions = dynamics::solve_zoom_reflow(&bodies);
    let mut targets = positions.into_iter().collect::<Vec<_>>();
    targets.sort_by_key(|(id, _)| id.as_u64());
    let mut changed = false;
    for (id, destination) in targets {
        changed |= apply_landmark_move(
            session,
            id,
            destination,
            LandmarkMovement::ZoomTemporary,
            output,
        );
    }
    session.nodes.retire_zoom_homes_without_field_node();
    changed
}

/// Returns temporarily displaced landmarks toward their remembered pre-zoom
/// homes as the presented footprint grows again. This is the zoom-in direction
/// of the same reconciliation: a home is forgotten once its landmark reaches
/// it, and a still-blocked landmark keeps its home for a later attempt.
pub(crate) fn restore_landmarks_for_zoom<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    output: &str,
    scale: f32,
) -> bool {
    let bodies = dynamics_bodies_at_scale(session, Some((output, scale)))
        .into_iter()
        .filter(|body| body.output == output)
        .collect::<Vec<_>>();
    let homes = bodies
        .iter()
        .filter_map(|body| session.nodes.zoom_home(body.id).map(|home| (body.id, home)))
        .collect::<HashMap<_, _>>();
    if homes.is_empty() {
        return false;
    }
    let positions = dynamics::solve_zoom_return(&bodies, &homes);
    let mut targets = positions
        .into_iter()
        .filter(|(id, _)| homes.contains_key(id))
        .collect::<Vec<_>>();
    targets.sort_by_key(|(id, _)| id.as_u64());
    let mut changed = false;
    for (id, destination) in targets {
        let home = homes[&id];
        if !landmark_moved(destination, home) {
            // The landmark is home again, so the temporary displacement is over.
            changed |=
                apply_landmark_move(session, id, home, LandmarkMovement::ZoomTemporary, output);
            session.nodes.commit_zoom_home(id);
            continue;
        }
        changed |= apply_landmark_move(
            session,
            id,
            destination,
            LandmarkMovement::ZoomTemporary,
            output,
        );
    }
    session.nodes.retire_zoom_homes_without_field_node();
    changed
}

fn apply_cluster_core_dynamics_positions(
    nodes: &mut NodesState,
    clusters: &mut crate::clusters::ClusterSystem,
    positions: &HashMap<NodeId, Vec2>,
) -> HashSet<String> {
    let core_changes = positions
        .iter()
        .filter_map(|(id, position)| {
            let cluster = clusters.cluster_for_core(*id)?;
            let metadata = clusters.metadata(cluster)?;
            ((position.x - metadata.core_position.x).abs() > 0.001
                || (position.y - metadata.core_position.y).abs() > 0.001)
                .then(|| (cluster, *id, *position, metadata.output.clone()))
        })
        .collect::<Vec<_>>();
    let mut outputs = HashSet::new();
    for (cluster, core, position, output) in core_changes {
        if clusters.move_core(cluster, &output, position) {
            if let Some(node) = nodes.field.node_mut(core) {
                node.pos = position;
            }
            // Physics and rigid moves are physical truth: whatever they move,
            // including a collision-chain neighbour, is no longer a zoom offset.
            nodes.commit_zoom_home(core);
            outputs.insert(output);
        }
    }
    outputs
}

pub(super) fn apply_dynamics_positions<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    positions: HashMap<NodeId, Vec2>,
    authority: Option<NodeId>,
) -> HashSet<String> {
    let changes = positions
        .iter()
        .filter_map(|(id, position)| {
            let record = session.nodes.record(*id)?;
            let current = session.nodes.field.node(*id)?.pos;
            ((position.x - current.x).abs() > 0.001 || (position.y - current.y).abs() > 0.001).then(
                || {
                    (
                        *id,
                        *position,
                        record.collapsed,
                        record.output.clone(),
                        record.window.clone(),
                        record.geometry.size,
                    )
                },
            )
        })
        .collect::<Vec<_>>();
    let mut outputs = apply_cluster_core_dynamics_positions(
        &mut session.nodes,
        &mut session.clusters,
        &positions,
    );
    for (id, position, collapsed, output, window, size) in changes {
        outputs.insert(output);
        session.nodes.commit_zoom_home(id);
        if let Some(node) = session.nodes.field.node_mut(id) {
            node.pos = position;
        }
        session.nodes.landmark_slides.borrow_mut().remove(&id);
        if collapsed {
            continue;
        }
        let location = Point::<i32, Logical>::from((
            (position.x - size.w as f32 * 0.5).round() as i32,
            (position.y - size.h as f32 * 0.5).round() as i32,
        ));
        // `Space::map_element` always moves an existing element to the top of
        // its z-index, even with `activate = false`. Physics must only change
        // position: otherwise a node pushing a window makes that window jump
        // layers on every solver frame.
        session.wayland.space.relocate_element(&window, location);
        if let Some(record) = session.nodes.record_mut(id) {
            record.geometry = Rectangle::new(location, size);
        }
        if crate::xwayland::is_x11(&window) {
            crate::xwayland::configure_window(session, &window, Rectangle::new(location, size));
        }
        if authority != Some(id) {
            // A physically displaced Wayland window needs only compositor
            // placement; its client-controlled size remains unchanged.
            crate::wayland::popup::update_reactive_for_window(
                &session.wayland,
                crate::session::popup_unconstrain_context!(session),
                &window,
            );
        }
    }
    outputs
}

pub(super) fn physics_frame_delta(last: Duration, now: Duration) -> f32 {
    now.saturating_sub(last).as_secs_f32().min(1.0 / 30.0)
}

pub(crate) fn move_grabbed_body_rigid<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    desired: Vec2,
) -> bool {
    let bodies = dynamics_bodies(session);
    if !bodies.iter().any(|body| body.id == id) {
        return false;
    }
    session.nodes.physics_velocity.clear();
    let positions = dynamics::solve_static_swept(bodies, id, desired);
    let changed = !apply_dynamics_positions(session, positions, Some(id)).is_empty();
    if changed {
        session.request_redraw();
    }
    changed
}

pub(crate) fn move_cluster_core_rigid<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    cluster: halley_core::cluster::ClusterId,
    desired: Vec2,
) -> bool {
    let Some(core) = session.clusters.core_node(cluster) else {
        return false;
    };
    move_grabbed_body_rigid(session, core, desired)
}

pub(crate) fn resolve_new_cluster_core<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    cluster: halley_core::cluster::ClusterId,
) -> bool {
    let Some(core) = session.clusters.core_node(cluster) else {
        return false;
    };
    // Members leave the free Field scene as the core takes over, so a home
    // remembered for one of them can no longer be honoured.
    for member in session.clusters.member_ids(cluster) {
        session.nodes.commit_zoom_home(member);
    }
    let Some(origin) = session
        .clusters
        .metadata(cluster)
        .map(|metadata| metadata.core_position)
    else {
        return false;
    };
    let bodies = dynamics_bodies(session);
    if !bodies.iter().any(|body| body.id == core) {
        return false;
    }
    let positions = dynamics::solve_new_landmark(bodies, core, origin);
    let Some(destination) = positions.get(&core).copied() else {
        return false;
    };
    if (destination.x - origin.x).abs() <= 0.001 && (destination.y - origin.y).abs() <= 0.001 {
        return false;
    }
    let changed = !apply_dynamics_positions(session, positions, Some(core)).is_empty();
    if !changed {
        return false;
    }
    session.nodes.start_landmark_slide(
        core,
        origin,
        destination,
        crate::frame_clock::monotonic_now(),
    );
    session.request_redraw();
    true
}

pub(crate) fn tick_physics<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    now: Duration,
) -> bool {
    if !session.nodes.physics.enabled {
        session.nodes.physics_velocity.clear();
        session.nodes.release_locks.clear();
        session.nodes.physics_last_tick = now;
        return false;
    }
    let authority = match &session.interactions.grab {
        crate::input::grab::Grab::MoveWindow {
            id: Some(id),
            cluster_drag,
            last_world,
            velocity,
            ..
        } if cluster_drag.is_none() => Some((*id, *last_world, *velocity)),
        crate::input::grab::Grab::MoveNode {
            id,
            last_world,
            velocity,
            ..
        } => Some((*id, *last_world, *velocity)),
        _ => None,
    };
    let expired_lock = session.nodes.expire_release_locks(now);
    if session.nodes.physics_velocity.is_empty()
        && authority.is_none()
        && session.nodes.release_locks.is_empty()
        && !expired_lock
    {
        session.nodes.physics_last_tick = now;
        return false;
    }
    let bodies = dynamics_bodies(session);
    let live = bodies.iter().map(|body| body.id).collect::<HashSet<_>>();
    session
        .nodes
        .physics_velocity
        .retain(|id, _| live.contains(id));
    let dt = physics_frame_delta(session.nodes.physics_last_tick, now);
    session.nodes.physics_last_tick = now;
    let positions = if let Some(authority) = authority {
        dynamics::solve_physics_swept(
            bodies,
            &mut session.nodes.physics_velocity,
            authority,
            dt,
            session.nodes.physics.damping,
        )
    } else {
        if dt <= f32::EPSILON && !expired_lock {
            return !session.nodes.physics_velocity.is_empty()
                || !session.nodes.release_locks.is_empty();
        }
        dynamics::solve_physics(
            &bodies,
            &mut session.nodes.physics_velocity,
            None,
            dt,
            session.nodes.physics.damping,
        )
    };
    let _ = apply_dynamics_positions(session, positions, authority.map(|(id, _, _)| id));
    authority.is_some()
        || !session.nodes.physics_velocity.is_empty()
        || !session.nodes.release_locks.is_empty()
}

pub(crate) fn displace_landmarks_for_new_window<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
) -> bool {
    if session.clusters.is_member(id) {
        return false;
    }
    let Some(position) = session.nodes.field.node(id).map(|node| node.pos) else {
        return false;
    };
    let bodies = dynamics_bodies(session);
    if !bodies.iter().any(|body| body.id == id) {
        return false;
    }
    let now = crate::frame_clock::monotonic_now();
    let landmarks = bodies
        .iter()
        .filter(|body| body.kind == dynamics::BodyKind::Node)
        .map(|body| {
            (
                body.id,
                body.pos,
                session.nodes.landmark_position(body.id, body.pos, now),
            )
        })
        .collect::<Vec<_>>();
    session.nodes.physics_velocity.clear();
    let positions = dynamics::solve_static_swept(bodies, id, position);
    let changed = !apply_dynamics_positions(session, positions, Some(id)).is_empty();
    if !changed {
        return false;
    }

    for (landmark, old_target, from) in landmarks {
        let Some(to) = session.nodes.field.node(landmark).map(|node| node.pos) else {
            continue;
        };
        if (to.x - old_target.x).abs() <= 0.001 && (to.y - old_target.y).abs() <= 0.001 {
            continue;
        }
        session.nodes.start_landmark_slide(landmark, from, to, now);
    }
    session.request_redraw();
    true
}

#[cfg(test)]
mod tests {
    use super::{apply_cluster_core_dynamics_positions, dynamics};
    use halley_core::cluster::layout::ClusterWorkspaceLayoutKind;
    use halley_core::field::Vec2;

    #[test]
    fn core_drag_commits_zoom_homes_for_core_and_physically_pushed_neighbour() {
        let config = halley_config::RuntimeConfig::default();
        let mut nodes = crate::nodes::NodesState::new(&config);
        let mut clusters =
            crate::clusters::ClusterSystem::new(config.clusters, config.animations.cluster);
        let first_member = nodes.field.spawn_surface(
            "first-member",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 200.0, y: 150.0 },
        );
        let neighbour_member = nodes.field.spawn_surface(
            "neighbour-member",
            Vec2 { x: 100.0, y: 0.0 },
            Vec2 { x: 200.0, y: 150.0 },
        );
        let first = clusters
            .create_collapsed_cluster(
                &mut nodes.field,
                "first".into(),
                "DP-1".into(),
                ClusterWorkspaceLayoutKind::Tiling,
                vec![first_member],
                Vec2 { x: 0.0, y: 0.0 },
            )
            .expect("first cluster");
        let neighbour = clusters
            .create_collapsed_cluster(
                &mut nodes.field,
                "neighbour".into(),
                "DP-1".into(),
                ClusterWorkspaceLayoutKind::Tiling,
                vec![neighbour_member],
                Vec2 { x: 100.0, y: 0.0 },
            )
            .expect("neighbour cluster");
        let core = clusters.core_node(first).expect("first core");
        let neighbour_core = clusters.core_node(neighbour).expect("neighbour core");
        nodes.remember_zoom_home(core, Vec2 { x: -20.0, y: 0.0 });
        nodes.remember_zoom_home(neighbour_core, Vec2 { x: 80.0, y: 0.0 });

        let extent = crate::clusters::CORE_DIAMETER_PX * 0.5;
        let bodies = [
            (core, Vec2 { x: 0.0, y: 0.0 }),
            (neighbour_core, Vec2 { x: 100.0, y: 0.0 }),
        ]
        .into_iter()
        .map(|(id, pos)| dynamics::Body {
            id,
            kind: dynamics::BodyKind::Node,
            pos,
            extents: dynamics::CollisionExtents::symmetric(Vec2 {
                x: extent,
                y: extent,
            }),
            gap: 0.0,
            pinned: false,
            output: "DP-1".into(),
        })
        .collect();

        // This matches the rigid core-drag seam: solve the swept collision chain,
        // then apply every changed core through cluster metadata and NodesState.
        let positions = dynamics::solve_static_swept(bodies, core, Vec2 { x: 100.0, y: 0.0 });
        let outputs = apply_cluster_core_dynamics_positions(&mut nodes, &mut clusters, &positions);

        assert_eq!(outputs, std::collections::HashSet::from(["DP-1".into()]));
        assert!(clusters.metadata(first).unwrap().core_position.x > 0.0);
        assert!(clusters.metadata(neighbour).unwrap().core_position.x > 100.0);
        assert_eq!(nodes.zoom_home(core), None);
        assert_eq!(nodes.zoom_home(neighbour_core), None);
    }
}
