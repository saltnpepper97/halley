//! Reconcile cluster membership, surfaces, and output presentation.

use super::*;

pub(crate) fn reconcile_cluster_surfaces<D: SessionDriver>(
    session: &mut Session<D>,
    output_name: &str,
) {
    let now = crate::frame_clock::monotonic_now();
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)
        .cloned()
    else {
        return;
    };
    let Some(output_geometry) = session.wayland.space.output_geometry(&output) else {
        return;
    };
    let work_area = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
    let targets =
        session
            .clusters
            .workspace_surface_targets(output_name, work_area, output_geometry);
    for target in targets {
        if session
            .clusters
            .surface_layout_is_deferred(target.node_id, now)
        {
            continue;
        }
        let Some((window, surface, current)) = session.nodes.record(target.node_id).map(|record| {
            (
                record.window.clone(),
                record.surface.clone(),
                session
                    .wayland
                    .space
                    .element_geometry(&record.window)
                    .unwrap_or(record.geometry),
            )
        }) else {
            continue;
        };
        if session.fullscreen.is_fullscreen_or_pending(&surface)
            || session.maximize.is_maximized_or_pending(&surface)
        {
            continue;
        }
        if session
            .xwayland
            .client_geometry_guarded_for_window(&window, now)
        {
            continue;
        }
        let geometry = if session.clusters.member_layout(target.node_id)
            == Some(halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Tiling)
        {
            crate::titlebar::client_rect_for_outer(
                &window,
                target.geometry,
                &session.settings.decorations,
                &session.settings.font,
            )
        } else {
            target.geometry
        };
        if !session
            .clusters
            .prepare_surface_target(target.node_id, current, geometry)
        {
            continue;
        }
        crate::wayland::set_window_output(&window, &output);
        session
            .wayland
            .space
            .relocate_element(&window, geometry.loc);
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|pending| {
                pending.size = Some(geometry.size);
                pending.bounds = Some(work_area.size);
                if session.clusters.is_member_floating(target.node_id) {
                    crate::wayland::decoration::clear_tiled_hint(pending);
                } else {
                    crate::wayland::decoration::apply_tiled_hint(pending);
                }
            });
            if toplevel.is_initial_configure_sent() {
                toplevel.send_configure();
            }
        } else {
            crate::xwayland::configure_window(session, &window, geometry);
        }
    }
}

/// Removes one runtime workspace, restores member client geometry, and keeps
/// the member applications alive as ordinary Field windows.
pub(crate) fn dissolve_cluster<D: SessionDriver>(
    session: &mut Session<D>,
    cluster_id: halley_core::cluster::ClusterId,
) -> bool {
    let focused_core = session
        .clusters
        .core_node(cluster_id)
        .filter(|core| session.nodes.focused() == Some(*core));
    let dissolving_core = session.clusters.core_node(cluster_id);
    let Some(dissolution) = session
        .clusters
        .dissolve_cluster(&mut session.nodes.field, cluster_id)
    else {
        return false;
    };
    // The core node and its hidden members leave the free Field scene, so no
    // remembered zoom home can be honoured for them any more.
    if let Some(core) = dissolving_core {
        session.nodes.commit_zoom_home(core);
    }
    for member in &dissolution.members {
        session.nodes.commit_zoom_home(*member);
    }

    for (member, geometry) in &dissolution.surface_restores {
        let Some((window, surface)) = session
            .nodes
            .record(*member)
            .map(|record| (record.window.clone(), record.surface.clone()))
        else {
            continue;
        };
        if session.fullscreen.is_fullscreen_or_pending(&surface)
            || session.maximize.is_maximized_or_pending(&surface)
        {
            continue;
        }
        let center = geometry.loc
            + smithay::utils::Point::<i32, smithay::utils::Logical>::from((
                geometry.size.w / 2,
                geometry.size.h / 2,
            ));
        let output = session
            .wayland
            .space
            .outputs()
            .find(|output| {
                session
                    .wayland
                    .space
                    .output_geometry(output)
                    .is_some_and(|output_geometry| output_geometry.contains(center))
            })
            .cloned()
            .or_else(|| {
                session
                    .wayland
                    .space
                    .outputs()
                    .find(|output| output.name() == dissolution.output)
                    .cloned()
            });
        let restored_output = output.as_ref().map(smithay::output::Output::name);
        if let Some(output) = output.as_ref() {
            crate::wayland::set_window_output(&window, output);
        }
        session
            .wayland
            .space
            .relocate_element(&window, geometry.loc);
        if let Some(record) = session.nodes.record_mut(*member) {
            record.geometry = *geometry;
            if let Some(output) = restored_output.as_ref() {
                record.output.clone_from(output);
            }
        }
        if let Some(toplevel) = window.toplevel() {
            let bounds = output.as_ref().map(|output| {
                smithay::desktop::layer_map_for_output(output)
                    .non_exclusive_zone()
                    .size
            });
            toplevel.with_pending_state(|pending| {
                pending.size = Some(geometry.size);
                pending.bounds = bounds;
                crate::wayland::decoration::clear_tiled_hint(pending);
            });
            if toplevel.is_initial_configure_sent() {
                toplevel.send_configure();
            }
        } else {
            crate::xwayland::configure_window(session, &window, *geometry);
        }
    }

    if focused_core.is_some() {
        let successor = dissolution.members.first().copied();
        session
            .nodes
            .focus(successor, session.start_time.elapsed().as_millis() as u64);
        if let Some(successor) = successor {
            session.record_trail_focus(successor);
        }
        sync_keyboard_focus(session, smithay::utils::SERIAL_COUNTER.next_serial());
        reconcile_pointer_constraints(session);
    }
    sync_cluster_camera(
        session,
        &dissolution.output,
        crate::frame_clock::monotonic_now(),
    );
    session.request_redraw();
    true
}

/// Copies an acknowledged interactive-resize result back into the
/// cluster-local floating layer. Clients may quantize the requested size, so
/// the committed Space geometry is authoritative rather than the last pointer
/// target sent to them.
///
/// Fullscreen and maximize own Space for the duration of their presentation,
/// including the enter commit that remaps the window to the output and the
/// unmaximize/unfullscreen handshake whose first commits still carry the
/// output-sized buffer. Copying that rectangle here would replace the
/// remembered windowed size, so exit would restore to an output-sized float
/// and skip the shrink. The same boundary keeps `reconcile_cluster_surfaces`
/// from driving those windows.
pub(crate) fn sync_cluster_floating_geometry<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) {
    let Some((member, window, output_name)) = session
        .nodes
        .id_for_surface(surface)
        .and_then(|member| {
            session
                .clusters
                .is_member_floating(member)
                .then_some(member)
        })
        .and_then(|member| {
            let record = session.nodes.record(member)?;
            let cluster = session.clusters.cluster_for_member(member)?;
            let metadata = session.clusters.metadata(cluster)?;
            (session.clusters.active_on(&metadata.output) == Some(cluster))
                .then(|| {
                    let output = session.clusters.member_floating_output(member)?;
                    Some((member, record.window.clone(), output.to_string()))
                })
                .flatten()
        })
    else {
        return;
    };
    let now = crate::frame_clock::monotonic_now();
    if session.fullscreen.is_fullscreen_or_pending(surface)
        || session.maximize.is_maximized_or_pending(surface)
        || session.fullscreen.awaits_external_configure(surface)
        || session
            .xwayland
            .client_geometry_guarded_for_window(&window, now)
    {
        return;
    }
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)
        .cloned()
    else {
        return;
    };
    let Some(output_geometry) = session.wayland.space.output_geometry(&output) else {
        return;
    };
    let Some(geometry) = session.wayland.space.element_geometry(&window) else {
        return;
    };
    let work_area = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
    let local = Rectangle::new(geometry.loc - output_geometry.loc, geometry.size);
    if session
        .clusters
        .update_member_floating_rect(&output_name, member, local, work_area)
    {
        let _ = session
            .clusters
            .prepare_surface_target(member, geometry, geometry);
        session.request_redraw();
    }
}

pub(crate) fn sync_cluster_camera<D: SessionDriver>(
    session: &mut Session<D>,
    output_name: &str,
    now: Duration,
) -> bool {
    let workspace_presented = session.clusters.active_on(output_name).is_some()
        || session
            .clusters
            .transition_cluster_on(output_name, now)
            .is_some();
    session
        .cameras
        .set_cluster_active(output_name, workspace_presented)
}
