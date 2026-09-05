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
    let now = crate::frame_clock::monotonic_now();
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
            PlacementChrome {
                decorations: &session.settings.decorations,
                font: &session.settings.font,
            },
        );
        if destination == current {
            continue;
        }
        if let Some(node) = session.nodes.field.node_mut(id) {
            node.pos = destination;
        }
        session
            .nodes
            .start_landmark_slide(id, current, destination, now);
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
    let mut bodies = session
        .nodes
        .records()
        .filter(|record| {
            record.attached
                && !session.clusters.is_member(record.id)
                && (record.collapsed
                    || (!session.fullscreen.is_fullscreen_or_pending(&record.surface)
                        && !session.maximize.contains(&record.surface)))
        })
        .filter_map(|record| {
            let node = session.nodes.field.node(record.id)?;
            let scale = scale_override
                .filter(|(output, _)| *output == record.output)
                .map(|(_, scale)| scale)
                .or_else(|| {
                    session
                        .cameras
                        .get(&record.output)
                        .map(crate::presentation::camera::scale)
                })
                .unwrap_or(1.0)
                .max(0.05);
            let (kind, extents) = if record.collapsed {
                (
                    dynamics::BodyKind::Node,
                    dynamics::CollisionExtents::symmetric(Vec2 {
                        x: NODE_DIAMETER_PX * 0.5 / scale,
                        y: NODE_DIAMETER_PX * 0.5 / scale,
                    }),
                )
            } else {
                let outer = crate::titlebar::outer_rect_for_client(
                    &record.window,
                    record.geometry,
                    &session.settings.decorations,
                    &session.settings.font,
                );
                let center_x = record.geometry.loc.x as f32 + record.geometry.size.w as f32 * 0.5;
                let center_y = record.geometry.loc.y as f32 + record.geometry.size.h as f32 * 0.5;
                (
                    dynamics::BodyKind::Window,
                    dynamics::CollisionExtents {
                        left: center_x - outer.loc.x as f32,
                        right: outer.loc.x as f32 + outer.size.w as f32 - center_x,
                        top: center_y - outer.loc.y as f32,
                        bottom: outer.loc.y as f32 + outer.size.h as f32 - center_y,
                    },
                )
            };
            Some(dynamics::Body {
                id: record.id,
                kind,
                pos: node.pos,
                extents,
                gap: session.nodes.landmarks.gap_px / scale,
                pinned: node.pinned || session.nodes.release_locked(record.id),
                output: record.output.clone(),
            })
        })
        .collect::<Vec<_>>();
    bodies.extend(session.clusters.collapsed_core_landmarks().into_iter().map(
        |(_, id, output, pos, pinned)| {
            let scale = scale_override
                .filter(|(override_output, _)| *override_output == output)
                .map(|(_, scale)| scale)
                .or_else(|| {
                    session
                        .cameras
                        .get(&output)
                        .map(crate::presentation::camera::scale)
                })
                .unwrap_or(1.0)
                .max(0.05);
            dynamics::Body {
                id,
                kind: dynamics::BodyKind::Node,
                pos,
                extents: dynamics::CollisionExtents::symmetric(Vec2 {
                    x: crate::clusters::CORE_DIAMETER_PX * 0.5 / scale,
                    y: crate::clusters::CORE_DIAMETER_PX * 0.5 / scale,
                }),
                gap: session.nodes.landmarks.gap_px / scale,
                pinned,
                output,
            }
        },
    ));
    bodies
}

/// Reconcile the fixed-pixel footprint of collapsed landmarks at a newly
/// presented zoom scale. This runs only while zooming out: active windows are
/// stationary blockers, while ordinary nodes and collapsed cluster cores move
/// together under the same collision policy.
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
    let now = crate::frame_clock::monotonic_now();
    let mut changed = false;

    for (id, destination) in positions {
        if let Some(cluster) = session.clusters.cluster_for_core(id) {
            let Some((current, core_output)) = session
                .clusters
                .metadata(cluster)
                .map(|metadata| (metadata.core_position, metadata.output.clone()))
            else {
                continue;
            };
            if (destination.x - current.x).abs() <= 0.001
                && (destination.y - current.y).abs() <= 0.001
            {
                continue;
            }
            let from = session.nodes.landmark_position(id, current, now);
            if !session
                .clusters
                .move_core(cluster, &core_output, destination)
            {
                continue;
            }
            if let Some(node) = session.nodes.field.node_mut(id) {
                node.pos = destination;
            }
            session.nodes.physics_velocity.remove(&id);
            session
                .nodes
                .start_landmark_slide(id, from, destination, now);
            changed = true;
            continue;
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
            continue;
        };
        if !collapsed
            || !attached
            || node_output != output
            || ((destination.x - current.x).abs() <= 0.001
                && (destination.y - current.y).abs() <= 0.001)
        {
            continue;
        }
        let from = session.nodes.landmark_position(id, current, now);
        if let Some(node) = session.nodes.field.node_mut(id) {
            node.pos = destination;
        }
        session.nodes.physics_velocity.remove(&id);
        session
            .nodes
            .start_landmark_slide(id, from, destination, now);
        changed = true;
    }

    changed
}

fn apply_dynamics_positions<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    positions: HashMap<NodeId, Vec2>,
    authority: Option<NodeId>,
) -> HashSet<String> {
    let core_changes = positions
        .iter()
        .filter_map(|(id, position)| {
            let cluster = session.clusters.cluster_for_core(*id)?;
            let metadata = session.clusters.metadata(cluster)?;
            ((position.x - metadata.core_position.x).abs() > 0.001
                || (position.y - metadata.core_position.y).abs() > 0.001)
                .then(|| (cluster, *id, *position, metadata.output.clone()))
        })
        .collect::<Vec<_>>();
    let changes = positions
        .into_iter()
        .filter_map(|(id, position)| {
            let record = session.nodes.record(id)?;
            let current = session.nodes.field.node(id)?.pos;
            ((position.x - current.x).abs() > 0.001 || (position.y - current.y).abs() > 0.001).then(
                || {
                    (
                        id,
                        position,
                        record.collapsed,
                        record.output.clone(),
                        record.window.clone(),
                        record.geometry.size,
                    )
                },
            )
        })
        .collect::<Vec<_>>();
    let mut outputs = HashSet::new();
    for (cluster, core, position, output) in core_changes {
        if session.clusters.move_core(cluster, &output, position) {
            if let Some(node) = session.nodes.field.node_mut(core) {
                node.pos = position;
            }
            outputs.insert(output);
        }
    }
    for (id, position, collapsed, output, window, size) in changes {
        outputs.insert(output);
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

pub(crate) fn set_collapsed_output<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    output: &Output,
) {
    let changed = session
        .nodes
        .record(id)
        .is_some_and(|record| record.output != output.name());
    if !changed {
        return;
    }
    let window = session.nodes.record(id).map(|record| record.window.clone());
    if let Some(record) = session.nodes.record_mut(id) {
        record.output = output.name();
    }
    if let Some(window) = window {
        crate::wayland::set_window_output(&window, output);
    }
    session.nodes.clear_direct_motion(id);
}

pub fn toggle_focused_on_output<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    output: Option<&str>,
    serial: smithay::utils::Serial,
) {
    let id = match output {
        Some(output) => session.nodes.focused_on_output(output),
        None => session.nodes.focused(),
    };
    let Some(id) = id else {
        return;
    };
    if session
        .nodes
        .record(id)
        .is_some_and(|record| record.collapsed)
    {
        let _ = restore(session, id, serial);
    } else {
        let _ = collapse(session, id, serial);
    }
}

pub fn close_focused_on_output<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    output: Option<&str>,
) {
    let belongs_to_output = |id: NodeId| {
        output.is_none_or(|output| {
            session
                .nodes
                .record(id)
                .is_some_and(|record| record.output == output)
                || session
                    .clusters
                    .cluster_for_core(id)
                    .and_then(|cluster| session.clusters.metadata(cluster))
                    .is_some_and(|metadata| metadata.output == output)
        })
    };

    // Active cluster members are intentionally hidden from the free-field
    // node scene.  That made `NodesState::focused()` reject them even while
    // their Wayland surface held keyboard focus, so CloseFocusedWindow could
    // become a no-op -- most visibly for the final member of a cluster.  The
    // live client focus is authoritative for real windows; logical focus is
    // still authoritative for compositor-only core nodes.
    let client_focused = session
        .wayland
        .focused_window
        .as_ref()
        .and_then(|surface| session.nodes.id_for_surface(surface))
        .filter(|id| belongs_to_output(*id));
    let logical_focused = session.nodes.focused().filter(|id| belongs_to_output(*id));
    let output_focused = output.and_then(|output| session.nodes.focused_on_output(output));
    let Some(id) = preferred_close_candidate(client_focused, logical_focused, output_focused)
    else {
        return;
    };
    let _ = close(session, id);
}

/// Requests closure for a managed node through the compositor's single close
/// path. Logical cluster cores expand to their member windows; regular and
/// cluster-member nodes close only their own canonical managed window.
pub fn close<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
) -> bool {
    let targets = session.clusters.close_targets_for_node(id);
    let windows = targets
        .into_iter()
        .filter_map(|target| {
            session
                .nodes
                .record(target)
                .map(|record| record.window.clone())
        })
        .collect::<Vec<_>>();
    for window in &windows {
        crate::session::request_window_close(session, window);
    }
    !windows.is_empty()
}

fn preferred_close_candidate(
    client_focused: Option<NodeId>,
    logical_focused: Option<NodeId>,
    output_focused: Option<NodeId>,
) -> Option<NodeId> {
    client_focused.or(logical_focused).or(output_focused)
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

pub fn collapse<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
) -> bool {
    collapse_inner(session, id, serial, false)
}

fn collapse_for_decay<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
) -> bool {
    collapse_inner(session, id, serial, true)
}

fn collapse_inner<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
    decay: bool,
) -> bool {
    // Cluster workspaces own member visibility as a unit.  Collapsing one
    // member would tear it out of the workspace without updating cluster
    // membership, so minimize requests (server-titlebar, xdg-shell, or X11)
    // are intentional no-ops while the window belongs to a cluster.
    if !collapse_allowed(session.clusters.is_member(id)) {
        return false;
    }
    let Some(record) = session.nodes.record(id).cloned() else {
        return false;
    };
    if record.collapsed
        || !record.attached
        || session.fullscreen.is_fullscreen_or_pending(&record.surface)
    {
        return false;
    }
    let maximize_restore = session.maximize.restore(&record.surface);
    let Some(geometry) = maximize_restore
        .as_ref()
        .map(|restore| restore.geometry)
        .or_else(|| session.wayland.space.element_geometry(&record.window))
    else {
        return false;
    };
    let Some(stack_index) = session
        .wayland
        .space
        .elements()
        .position(|candidate| candidate == &record.window)
    else {
        return false;
    };
    let client_was_focused = session.wayland.focused_window.as_ref() == Some(&record.surface);
    let logical_focus =
        logical_focus_after_collapse(session.nodes.focused(), id, client_was_focused);

    let _ = if decay {
        crate::session::closing::capture_window_for_decay(session, &record.window)
    } else {
        crate::session::closing::capture_window(session, &record.window)
    };
    if let Some(restore) = session.maximize.take_restore(&record.surface) {
        session.render.fullscreen_textures.remove(&restore.surface);
        crate::session::configure_field_geometry(session, &restore);
        let _ = session.cameras.apply_field_maximize(&record.output, None);
        if let Some(record) = session.nodes.record_mut(id) {
            record.geometry = restore.geometry;
        }
    }
    crate::session::cancel_grab_for_surface(session, &record.surface);
    if client_was_focused {
        // A collapsed surface must not keep receiving keyboard input, but the
        // node remains Halley's logical command/focus target.
        crate::window::clear_focus(&mut session.wayland);
    }
    session.wayland.space.unmap_elem(&record.window);
    if record.window.toplevel().is_some() {
        session
            .wayland
            .collapsed
            .insert(record.surface.clone(), record.window.clone());
    } else {
        crate::xwayland::set_hidden(&record.window, true);
    }
    session.xwayland.set_window_iconic(&record.window);
    let collapse_origin = rect_center(geometry);
    let scale = session
        .cameras
        .get(&record.output)
        .map(crate::presentation::camera::scale)
        .unwrap_or(1.0);
    let occupied_cores = session
        .clusters
        .collapsed_core_landmarks()
        .into_iter()
        .filter_map(|(_, _, output, position, _)| (output == record.output).then_some(position))
        .collect::<Vec<_>>();
    let node_position = session.nodes.nearest_free_position(
        id,
        collapse_origin,
        scale,
        &occupied_cores,
        PlacementChrome {
            decorations: &session.settings.decorations,
            font: &session.settings.font,
        },
    );
    if let Some(node) = session.nodes.field.node_mut(id) {
        node.pos = node_position;
        node.intrinsic_size = vec_size(geometry);
    }
    if let Some(record) = session.nodes.record_mut(id) {
        record.geometry = geometry;
        record.collapsed_stack_index = Some(stack_index);
    }
    let now_ms = session.start_time.elapsed().as_millis() as u64;
    if !session.nodes.set_collapsed(id, true, now_ms) {
        return false;
    }
    if let Some(record) = session.nodes.record_mut(id) {
        record.collapsed_at = crate::frame_clock::monotonic_now();
    }
    session.nodes.start_landmark_slide(
        id,
        collapse_origin,
        node_position,
        crate::frame_clock::monotonic_now(),
    );
    session
        .render
        .window_close_animations
        .retarget_pending_to_node(&record.surface, node_position);
    let _ = crate::session::closing::start(session, &record.surface);
    let focus_changed = session.nodes.focus(logical_focus, now_ms);
    if focus_changed && let Some(id) = logical_focus {
        session.record_trail_focus(id);
    }
    crate::session::sync_keyboard_focus(session, serial);
    crate::session::reconcile_pointer_constraints(session);
    session.request_redraw();
    true
}

fn collapse_allowed(cluster_member: bool) -> bool {
    !cluster_member
}

pub fn restore<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
) -> bool {
    restore_with_centering(session, id, serial, None)
}

pub fn restore_for_close<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
) -> bool {
    restore_without_centering(session, id, serial)
}

pub(crate) fn restore_for_cluster_join<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
) -> bool {
    restore_without_centering(session, id, serial)
}

fn restore_without_centering<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
) -> bool {
    restore_with_centering(
        session,
        id,
        serial,
        Some(halley_config::RestoreCentering::Never),
    )
}

fn restore_with_centering<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
    centering: Option<halley_config::RestoreCentering>,
) -> bool {
    let Some(record) = session.nodes.record(id).cloned() else {
        return false;
    };
    if !record.collapsed || !record.attached {
        return false;
    }
    let Some(node) = session.nodes.field.node(id).cloned() else {
        return false;
    };
    let size = record.geometry.size;
    let location = Point::<i32, Logical>::from((
        (node.pos.x - size.w as f32 / 2.0).round() as i32,
        (node.pos.y - size.h as f32 / 2.0).round() as i32,
    ));
    session
        .wayland
        .space
        .map_element(record.window.clone(), location, true);
    crate::xwayland::set_hidden(&record.window, false);
    session.xwayland.set_window_normal(&record.window);
    // A collapsed window is not in the space, so a decoration reload while it
    // was collapsed never reached it.
    session.xwayland.sync_frame_extents(
        &record.window,
        &session.settings.decorations,
        &session.settings.font,
    );
    session.wayland.collapsed.remove(&record.surface);
    let now = crate::frame_clock::monotonic_now();
    let now_ms = session.start_time.elapsed().as_millis() as u64;
    let _ = session.nodes.set_collapsed(id, false, now_ms);
    if let Some(record) = session.nodes.record_mut(id) {
        record.collapsed_stack_index = None;
    }
    reconcile_landmarks(session, Some(&record.output));
    crate::session::closing::mapped(session, &record.surface);
    crate::window::focus_and_raise(&mut session.wayland, &record.window);
    session.xwayland.raise_window(&record.window);

    let output = {
        session
            .wayland
            .space
            .outputs()
            .find(|output| output.name() == record.output)
            .cloned()
    };
    if let Some(output) = output {
        let _ = crate::session::opening::start(session, record.surface.clone(), &output, now);
        let should_center = match centering.unwrap_or(session.nodes.config.restore_centering) {
            halley_config::RestoreCentering::Never => false,
            halley_config::RestoreCentering::Always => true,
            halley_config::RestoreCentering::IfOffscreen => {
                let Some(output_geometry) = session.wayland.space.output_geometry(&output) else {
                    return true;
                };
                let Some(camera) = session.cameras.get(&record.output) else {
                    return true;
                };
                !output_geometry.contains(screen_from_world(node.pos, camera, output_geometry))
            }
        };
        if should_center
            && let Some(output_geometry) = session.wayland.space.output_geometry(&output)
            && let Some(camera) = session.cameras.get_mut(&record.output)
        {
            camera.pan_vel = Vec2 { x: 0.0, y: 0.0 };
            camera.target_center = Vec2 {
                x: node.pos.x - output_geometry.loc.x as f32,
                y: node.pos.y - output_geometry.loc.y as f32,
            };
        }
    }
    crate::session::sync_keyboard_focus(session, serial);
    crate::session::reconcile_pointer_constraints(session);
    session.request_redraw();
    true
}

pub fn pan_after_close_restore<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    policy: halley_config::CloseRestorePan,
) {
    if policy == halley_config::CloseRestorePan::Never {
        return;
    }
    let Some(record) = session.nodes.record(id).cloned() else {
        return;
    };
    if session.fullscreen.is_fullscreen_or_pending(&record.surface)
        || session.maximize.contains(&record.surface)
        || session.clusters.is_member_floating(id)
    {
        return;
    }
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == record.output)
        .cloned()
    else {
        return;
    };
    let Some(output_geometry) = session.wayland.space.output_geometry(&output) else {
        return;
    };
    let Some(view) = session.cameras.view(&record.output) else {
        return;
    };
    let geometry = session
        .wayland
        .space
        .element_geometry(&record.window)
        .unwrap_or(record.geometry);
    let viewport = crate::presentation::camera::world_viewport(view, output_geometry);
    let delta = match policy {
        halley_config::CloseRestorePan::Never => return,
        halley_config::CloseRestorePan::IfOffscreen => {
            if viewport.intersection(geometry).is_some() {
                return;
            }
            minimal_reveal_delta(viewport, geometry, 24)
        }
        halley_config::CloseRestorePan::Always => Vec2 {
            x: geometry.loc.x as f32 + geometry.size.w as f32 * 0.5
                - (viewport.loc.x as f32 + viewport.size.w as f32 * 0.5),
            y: geometry.loc.y as f32 + geometry.size.h as f32 * 0.5
                - (viewport.loc.y as f32 + viewport.size.h as f32 * 0.5),
        },
    };
    if let Some(camera) = session.cameras.get_mut(&record.output) {
        camera.pan_vel = Vec2 { x: 0.0, y: 0.0 };
        camera.target_center = Vec2 {
            x: camera.center.x + delta.x,
            y: camera.center.y + delta.y,
        };
    }
}

/// Focus and raise a presentation-navigation target, then smoothly place its
/// center at the output center. Apogee and Alt+Tab are explicit spatial jumps:
/// they should land on the chosen window rather than merely reveal an edge.
pub fn focus_and_center_node<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
) -> bool {
    let Some(record) = session.nodes.record(id).cloned() else {
        return false;
    };
    let Some(node) = session.nodes.field.node(id).cloned() else {
        return false;
    };
    let selected_output = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == record.output)
        .cloned();
    if session
        .fullscreen
        .pause_presentation_on_output_except(&record.output, &record.surface)
        && let Some(output) = selected_output.as_ref()
    {
        session.sync_fullscreen_camera(output, crate::frame_clock::monotonic_now());
    }
    let activated = if record.collapsed {
        restore_with_centering(
            session,
            id,
            serial,
            Some(halley_config::RestoreCentering::Always),
        )
    } else if record.attached {
        crate::session::focus_window(session, &record.window, serial);
        true
    } else {
        false
    };
    if !activated {
        return false;
    }

    // Presentation owners already place their window at the output center.
    // Rewriting the parked Field camera underneath them would make their later
    // restore jump to an unrelated Apogee selection.
    if !session.fullscreen.is_fullscreen_or_pending(&record.surface)
        && !session.maximize.contains(&record.surface)
        && let Some(output) = selected_output
        && let Some(output_geometry) = session.wayland.space.output_geometry(&output)
    {
        let _ = session.cameras.center_field_on(
            &record.output,
            Vec2 {
                x: node.pos.x - output_geometry.loc.x as f32,
                y: node.pos.y - output_geometry.loc.y as f32,
            },
        );
    }
    let _ = crate::session::center_pointer_on_output(session, &record.output);
    session.request_redraw();
    true
}

/// Activate a node and make it visible in one operation. Collapsed nodes
/// follow the configured restore-centering policy; live windows are focused
/// immediately. When `pan` is true, the camera only moves far enough to
/// reveal their bounds.
pub fn focus_or_reveal_node<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
    pan: bool,
) -> bool {
    let Some(record) = session.nodes.record(id).cloned() else {
        return false;
    };
    if record.collapsed {
        return restore(session, id, serial);
    }
    if !record.attached {
        return false;
    }

    crate::session::focus_window(session, &record.window, serial);
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == record.output)
        .cloned()
    else {
        session.request_redraw();
        return true;
    };
    let Some(output_geometry) = session.wayland.space.output_geometry(&output) else {
        session.request_redraw();
        return true;
    };
    let Some(view) = session.cameras.view(&record.output) else {
        session.request_redraw();
        return true;
    };
    let geometry = session
        .wayland
        .space
        .element_geometry(&record.window)
        .unwrap_or(record.geometry);
    if pan {
        let delta = minimal_reveal_delta(
            crate::presentation::camera::world_viewport(view, output_geometry),
            geometry,
            24,
        );
        apply_camera_reveal_delta(session, &record.output, delta);
    }
    session.request_redraw();
    true
}

/// Select a collapsed Field node. When `pan` is true, the camera moves only
/// far enough to fit its restored decorated-window bounds. This intentionally
/// leaves the node collapsed.
pub fn reveal_collapsed_node<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
    pan: bool,
) -> bool {
    let Some(record) = session.nodes.record(id).cloned() else {
        return false;
    };
    if !record.attached || !record.collapsed {
        return false;
    }
    let Some(node_position) = session.nodes.field.node(id).map(|node| node.pos) else {
        return false;
    };
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == record.output)
        .cloned()
    else {
        return false;
    };
    crate::wayland::focus::select_output(&mut session.wayland, &output);
    crate::window::clear_focus(&mut session.wayland);
    session
        .nodes
        .focus(Some(id), session.start_time.elapsed().as_millis() as u64);
    crate::session::sync_keyboard_focus(session, serial);

    if let (Some(output_geometry), Some(view)) = (
        session.wayland.space.output_geometry(&output),
        session.cameras.view(&record.output),
    ) {
        let restored_client = centered_rect(node_position, record.geometry.size);
        let restored_outer = crate::titlebar::outer_rect_for_client(
            &record.window,
            restored_client,
            &session.settings.decorations,
            &session.settings.font,
        );
        if pan {
            let delta = minimal_reveal_delta(
                crate::presentation::camera::world_viewport(view, output_geometry),
                restored_outer,
                24,
            );
            apply_camera_reveal_delta(session, &record.output, delta);
        }
    }
    session.request_output_redraw(&output);
    true
}

/// Select a collapsed cluster's logical core. When `pan` is true, the camera
/// moves only far enough to make that core visible. This intentionally does
/// not activate the workspace.
pub fn reveal_cluster_core<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    core: NodeId,
    serial: smithay::utils::Serial,
    pan: bool,
) -> bool {
    let Some(cluster) = session.clusters.cluster_for_core(core) else {
        return false;
    };
    let Some(metadata) = session.clusters.metadata(cluster).cloned() else {
        return false;
    };
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == metadata.output)
        .cloned()
    else {
        return false;
    };
    crate::window::clear_focus(&mut session.wayland);
    session
        .nodes
        .focus(Some(core), session.start_time.elapsed().as_millis() as u64);
    crate::session::sync_keyboard_focus(session, serial);

    if let (Some(output_geometry), Some(view)) = (
        session.wayland.space.output_geometry(&output),
        session.cameras.view(&metadata.output),
    ) && pan
    {
        let delta = landmark_reveal_delta(
            crate::presentation::camera::world_viewport(view, output_geometry),
            metadata.core_position,
            crate::clusters::CORE_DIAMETER_PX,
            view.scale,
        );
        apply_camera_reveal_delta(session, &metadata.output, delta);
    }
    session.request_redraw();
    true
}

pub(super) fn centered_rect(
    position: Vec2,
    size: smithay::utils::Size<i32, Logical>,
) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (
            (position.x - size.w as f32 * 0.5).round() as i32,
            (position.y - size.h as f32 * 0.5).round() as i32,
        )
            .into(),
        size,
    )
}

pub(super) fn landmark_reveal_delta(
    viewport: Rectangle<i32, Logical>,
    position: Vec2,
    diameter_px: f32,
    scale: f32,
) -> Vec2 {
    let scale = scale.max(0.05);
    let side = (diameter_px / scale).round().max(1.0) as i32;
    let landmark = Rectangle::<i32, Logical>::new(
        (
            (position.x - side as f32 * 0.5).round() as i32,
            (position.y - side as f32 * 0.5).round() as i32,
        )
            .into(),
        (side, side).into(),
    );
    minimal_reveal_delta(viewport, landmark, (24.0 / scale).round() as i32)
}

fn apply_camera_reveal_delta<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    output: &str,
    delta: Vec2,
) {
    if (delta.x != 0.0 || delta.y != 0.0)
        && let Some(camera) = session.cameras.get_mut(output)
    {
        camera.pan_vel = Vec2 { x: 0.0, y: 0.0 };
        camera.target_center = Vec2 {
            x: camera.center.x + delta.x,
            y: camera.center.y + delta.y,
        };
    }
}

pub(super) fn minimal_reveal_delta(
    viewport: Rectangle<i32, Logical>,
    target: Rectangle<i32, Logical>,
    margin: i32,
) -> Vec2 {
    fn axis_delta(
        view_start: i32,
        view_extent: i32,
        target_start: i32,
        target_extent: i32,
        margin: i32,
    ) -> f32 {
        let available = (view_extent - margin.saturating_mul(2)).max(1);
        if target_extent > available {
            return (target_start as f32 + target_extent as f32 * 0.5)
                - (view_start as f32 + view_extent as f32 * 0.5);
        }
        let minimum = view_start + margin;
        let maximum = view_start + view_extent - margin;
        if target_start < minimum {
            (target_start - minimum) as f32
        } else if target_start + target_extent > maximum {
            (target_start + target_extent - maximum) as f32
        } else {
            0.0
        }
    }

    Vec2 {
        x: axis_delta(
            viewport.loc.x,
            viewport.size.w,
            target.loc.x,
            target.size.w,
            margin,
        ),
        y: axis_delta(
            viewport.loc.y,
            viewport.size.h,
            target.loc.y,
            target.size.h,
            margin,
        ),
    }
}

fn hard_protected_from_decay(
    fullscreen: bool,
    maximized: bool,
    grabbed: bool,
    arranged: bool,
) -> bool {
    fullscreen || maximized || grabbed || arranged
}

pub fn tick_decay<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
) -> bool {
    session.nodes.sync_from_space(&session.wayland.space);
    let mut centers = HashMap::new();
    for record in session.nodes.records() {
        if centers.contains_key(&record.output) {
            continue;
        }
        let Some(output) = session
            .wayland
            .space
            .outputs()
            .find(|output| output.name() == record.output)
        else {
            continue;
        };
        let Some(output_geometry) = session.wayland.space.output_geometry(output) else {
            continue;
        };
        let Some(view) = session.cameras.view(&record.output) else {
            continue;
        };
        let global = crate::presentation::camera::global_center(view.center, output_geometry);
        centers.insert(
            record.output.clone(),
            Vec2 {
                x: global.x,
                y: global.y,
            },
        );
    }
    let focused = session.wayland.focused_window.clone();
    let cluster_members = session
        .nodes
        .records()
        .filter(|record| session.clusters.is_member(record.id))
        .map(|record| record.id)
        .collect::<HashSet<_>>();
    let protected = session
        .nodes
        .records()
        .filter(|record| {
            hard_protected_from_decay(
                session.fullscreen.is_fullscreen_or_pending(&record.surface),
                session.maximize.contains(&record.surface),
                crate::input::grab::belongs_to_surface(&session.interactions.grab, &record.surface),
                session
                    .interactions
                    .field_arrange
                    .contains_surface(&record.surface),
            )
        })
        .map(|record| record.surface.clone())
        .collect::<Vec<_>>();
    let now_ms = session.start_time.elapsed().as_millis() as u64;
    let ready = session.nodes.decay_candidates(
        &centers,
        focused.as_ref(),
        |id| cluster_members.contains(&id),
        |surface| protected.iter().any(|candidate| candidate == surface),
        now_ms,
    );
    let mut changed = false;
    // Snapshotting and collapsing several full-size windows in one calloop
    // callback blocks keyboard dispatch across all outputs. Deadlines are
    // already checked once per second, so drain overdue nodes incrementally.
    for id in ready.into_iter().take(1) {
        changed |= collapse_for_decay(session, id, smithay::utils::SERIAL_COUNTER.next_serial());
    }
    changed
}

#[cfg(test)]
mod close_tests {
    use super::{collapse_allowed, hard_protected_from_decay, preferred_close_candidate};
    use halley_core::field::NodeId;

    #[test]
    fn arranged_windows_are_hard_protected_from_decay() {
        assert!(hard_protected_from_decay(false, false, false, true));
        assert!(!hard_protected_from_decay(false, false, false, false));
    }

    #[test]
    fn live_client_focus_wins_over_stale_logical_focus_for_cluster_members() {
        let active_member = NodeId::new(1);
        let stale_logical = NodeId::new(2);
        let output_history = NodeId::new(3);

        assert_eq!(
            preferred_close_candidate(
                Some(active_member),
                Some(stale_logical),
                Some(output_history),
            ),
            Some(active_member)
        );
    }

    #[test]
    fn logical_core_focus_is_used_when_no_client_surface_is_focused() {
        let core = NodeId::new(4);
        assert_eq!(
            preferred_close_candidate(None, Some(core), Some(NodeId::new(5))),
            Some(core)
        );
    }

    #[test]
    fn cluster_members_reject_individual_minimize_requests() {
        assert!(!collapse_allowed(true));
        assert!(collapse_allowed(false));
    }
}
