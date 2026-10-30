use super::*;

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
        // Pointer hover is suspended during a drag. Keep keyboard actions on
        // the moved focus owner instead of the previous monitor's MRU window.
        let owns_client_focus = window.wl_surface().is_some_and(|surface| {
            session.wayland.focused_window.as_ref() == Some(surface.as_ref())
        });
        if owns_client_focus
            || (session.wayland.focused_window.is_none() && session.nodes.focused() == Some(id))
        {
            crate::wayland::focus::select_output(&mut session.wayland, output);
        }
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

fn close_candidate_in_workspace(
    active: Option<halley_core::cluster::ClusterId>,
    membership: Option<halley_core::cluster::ClusterId>,
) -> bool {
    active.is_none_or(|cluster| membership == Some(cluster))
}

pub fn close_focused_on_output<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    output: Option<&str>,
) {
    let belongs_to_output = |id: NodeId| {
        if let Some(record) = session.nodes.record(id) {
            let requested_output = output.unwrap_or(&record.output);
            return record.output == requested_output
                && close_candidate_in_workspace(
                    session.clusters.active_on(requested_output),
                    session.clusters.cluster_for_member(id),
                );
        }
        session
            .clusters
            .cluster_for_core(id)
            .and_then(|cluster| session.clusters.metadata(cluster))
            .is_some_and(|metadata| output.is_none_or(|output| metadata.output == output))
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
    let output_focused = output
        .and_then(|output| session.nodes.focused_on_output(output))
        .filter(|id| belongs_to_output(*id));
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

    let _ = crate::session::closing::capture_window_for_node(session, &record.window, decay);
    session.window_animations.remove(&record.surface);
    if let Some(restore) = session.maximize.take_restore(&record.surface) {
        session.render.fullscreen_textures.remove(&restore.surface);
        crate::session::configure_field_geometry(session, &restore);
        let _ = session.cameras.apply_field_maximize(&record.output, None);
        if let Some(record) = session.nodes.record_mut(id) {
            record.geometry = restore.geometry;
        }
    }
    crate::session::cancel_grab_for_surface(session, &record.surface);
    if crate::xwayland::is_x11(&record.window) {
        // Publish the accepted minimize before focus loss. An exclusive
        // fullscreen client can otherwise start a conflicting mode change
        // while its X11 frame still appears viewable and normal.
        crate::xwayland::set_hidden(&record.window, true);
        session.xwayland.set_window_iconic(&record.window);
    }
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
    }
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
        PlacementContext {
            decorations: &session.settings.decorations,
            font: &session.settings.font,
            fullscreen: &session.fullscreen,
            maximize: &session.maximize,
        },
    );
    if let Some(node) = session.nodes.field.node_mut(id) {
        node.pos = node_position;
        node.intrinsic_size = vec_size(geometry);
    }
    // Collapsing places a landmark for real; any remembered zoom home for this
    // node belonged to a different, now superseded position.
    session.nodes.commit_zoom_home(id);
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
    // Automatic decay and manual `Mod+N` share this seam; the collapse trigger
    // decides whether this one explains itself. See `session::decay_notice`.
    session.note_collapse(decay, &record.title, record.app_id.as_deref());
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
    // The window is now placed at the displayed center, so that position is
    // real rather than a reversible zoom offset.
    session.nodes.commit_zoom_home(id);
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
        let started = crate::session::opening::start(session, record.surface.clone(), &output, now);
        if started && crate::xwayland::is_x11(&record.window) {
            session
                .window_animations
                .defer_opening_until_commit(&record.surface, now);
        }
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
/// center at the output center. Used by explicit spatial retrieval (Apogee).
pub fn focus_and_center_node<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
) -> bool {
    focus_navigation_node(session, id, serial, true)
}

/// Alt+Tab raises an onscreen target in place. Only a target needing a reveal
/// may release presentation ownership on its output. See docs/apogee.md.
pub fn focus_cycle_node<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
) -> bool {
    focus_navigation_node(session, id, serial, false)
}

fn focus_navigation_node<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    serial: smithay::utils::Serial,
    center: bool,
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
    if !record.attached {
        return false;
    }
    let presented = session.fullscreen.is_fullscreen_or_pending(&record.surface)
        || session.maximize.contains(&record.surface);
    let target_geometry = if record.collapsed {
        centered_rect(node.pos, record.geometry.size)
    } else {
        session
            .wayland
            .space
            .element_geometry(&record.window)
            .unwrap_or(record.geometry)
    };
    let target_geometry = crate::titlebar::outer_rect_for_client(
        &record.window,
        target_geometry,
        &session.settings.decorations,
        &session.settings.font,
    );
    let needs_pan = !presented
        && selected_output.as_ref().is_some_and(|output| {
            let Some(geometry) = session.wayland.space.output_geometry(output) else {
                return false;
            };
            let Some(view) = session.cameras.view(&record.output) else {
                return false;
            };
            let viewport = crate::presentation::camera::world_viewport(view, geometry);
            let delta = if viewport.contains_rect(target_geometry) {
                Vec2 { x: 0.0, y: 0.0 }
            } else {
                minimal_reveal_delta(viewport, target_geometry, 24)
            };
            delta.x != 0.0 || delta.y != 0.0
        });
    if !center {
        session.cameras.cancel_queued_field_center(&record.output);
    }
    let candidates = if center || needs_pan {
        session
            .fullscreen
            .navigation_candidates(&record.output, &record.surface)
    } else {
        Vec::new()
    };
    for surface in candidates {
        let Some(outgoing) = session
            .nodes
            .id_for_surface(&surface)
            .and_then(|id| session.nodes.record(id))
            .cloned()
        else {
            continue;
        };
        let rule = session
            .window_rules
            .resolve_identity(&crate::window::rules::identity(&outgoing.window));
        if session.window_rules.games.is_game(
            &outgoing.window,
            &session.wayland.display_handle,
            rule.game,
        ) {
            session
                .fullscreen
                .pause_presentation_on_output(&surface, &record.output);
        } else {
            crate::session::leave_fullscreen_for_navigation(session, outgoing.clone());
        }
        let output = session
            .wayland
            .space
            .outputs()
            .find(|output| output.name() == outgoing.output)
            .cloned();
        if let Some(output) = output {
            session.sync_fullscreen_camera(&output, crate::frame_clock::monotonic_now());
        }
    }
    if !center && needs_pan {
        let workspace = crate::presentation::workspace_for_surface(
            &session.clusters,
            &session.nodes,
            &record.surface,
        );
        let maximized = session
            .nodes
            .records()
            .filter(|outgoing| {
                outgoing.output == record.output
                    && outgoing.surface != record.surface
                    && session.maximize.contains(&outgoing.surface)
                    && crate::presentation::workspace_for_surface(
                        &session.clusters,
                        &session.nodes,
                        &outgoing.surface,
                    ) == workspace
            })
            .map(|outgoing| outgoing.surface.clone())
            .collect::<Vec<_>>();
        for surface in maximized {
            crate::session::set_surface_field_maximized(session, &surface, false);
        }
    }
    // Keyboard navigation selects the destination monitor before the old
    // client sees focus loss. A cursor warp alone does not update that state.
    if let Some(output) = selected_output.as_ref() {
        crate::wayland::focus::select_output(&mut session.wayland, output);
    }
    let activated = if record.collapsed {
        restore_with_centering(
            session,
            id,
            serial,
            Some(if center {
                halley_config::RestoreCentering::Always
            } else {
                halley_config::RestoreCentering::Never
            }),
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
        if center {
            let _ = session.cameras.center_field_on(
                &record.output,
                Vec2 {
                    x: node.pos.x - output_geometry.loc.x as f32,
                    y: node.pos.y - output_geometry.loc.y as f32,
                },
            );
        } else if needs_pan && let Some(view) = session.cameras.field_restore_view(&record.output) {
            let viewport = crate::presentation::camera::world_viewport(view, output_geometry);
            let delta = if viewport.contains_rect(target_geometry) {
                Vec2 { x: 0.0, y: 0.0 }
            } else {
                minimal_reveal_delta(viewport, target_geometry, 24)
            };
            session.cameras.queue_field_center(
                &record.output,
                Vec2 {
                    x: view.center.x + delta.x,
                    y: view.center.y + delta.y,
                },
            );
        }
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

#[cfg(test)]
mod workspace_close_tests {
    #[test]
    fn active_cluster_close_never_targets_a_hidden_field_or_other_cluster_window() {
        use halley_core::cluster::ClusterId;
        let active = ClusterId::new(1);
        assert!(!super::close_candidate_in_workspace(Some(active), None));
        assert!(!super::close_candidate_in_workspace(
            Some(active),
            Some(ClusterId::new(2))
        ));
        assert!(super::close_candidate_in_workspace(
            Some(active),
            Some(active)
        ));
        assert!(super::close_candidate_in_workspace(None, None));
    }
}
