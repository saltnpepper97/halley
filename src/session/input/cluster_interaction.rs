//! Coordinate cluster activation, creation, and bloom interaction.

use super::*;

pub(crate) fn cluster_owns_focus<D: SessionDriver>(
    session: &Session<D>,
    id: halley_core::cluster::ClusterId,
) -> bool {
    let logical = session.nodes.focused().is_some_and(|focused| {
        session.clusters.cluster_for_member(focused) == Some(id)
            || session.clusters.cluster_for_core(focused) == Some(id)
    });
    let keyboard = session
        .seat
        .get_keyboard()
        .and_then(|keyboard| keyboard.current_focus())
        .and_then(|focus| focus.wl_surface().map(|surface| surface.into_owned()))
        .and_then(|surface| session.nodes.id_for_surface(&surface))
        .is_some_and(|focused| session.clusters.cluster_for_member(focused) == Some(id));
    logical || keyboard
}

pub(crate) fn show_cluster_indicator<D: SessionDriver>(
    session: &mut Session<D>,
    id: halley_core::cluster::ClusterId,
    now: std::time::Duration,
) {
    let Some((output, name, layout)) = session.clusters.metadata(id).map(|metadata| {
        (
            metadata.output.clone(),
            metadata.name.clone(),
            metadata.layout,
        )
    }) else {
        return;
    };
    session
        .shell
        .overlays
        .show_cluster_indicator(&output, &name, layout, now);
}

pub(super) fn activation_shows_cluster_indicator(
    first_member: Option<halley_core::field::NodeId>,
) -> bool {
    first_member.is_none()
}

pub(crate) fn sync_cluster_activation_focus<D: SessionDriver>(
    session: &mut Session<D>,
    output: &Output,
    id: halley_core::cluster::ClusterId,
    collapsed_should_focus: bool,
    serial: smithay::utils::Serial,
) {
    let now = crate::frame_clock::monotonic_now();
    let output_name = output.name();
    crate::session::sync_cluster_camera(session, &output_name, now);
    if session.clusters.active_on(&output_name) == Some(id) {
        let first_member = session.clusters.first_member(id);
        if let Some(member) = first_member {
            if let Some(window) = session
                .nodes
                .record(member)
                .map(|record| record.window.clone())
            {
                crate::session::focus_window(session, &window, serial);
            }
        } else {
            // An empty workspace has no client surface to own keyboard focus,
            // but its persistent core is still the logical selection. Identify
            // the otherwise blank workspace; populated clusters reveal their
            // contents directly and do not need an activation card.
            crate::window::clear_focus(&mut session.wayland);
            session.nodes.focus(
                session.clusters.core_node(id),
                session.start_time.elapsed().as_millis() as u64,
            );
            crate::session::sync_keyboard_focus(session, serial);
        }
        if activation_shows_cluster_indicator(first_member) {
            show_cluster_indicator(session, id, now);
        }
    } else if collapsed_should_focus {
        crate::window::clear_focus(&mut session.wayland);
        session.nodes.focus(
            session.clusters.core_node(id),
            session.start_time.elapsed().as_millis() as u64,
        );
        crate::session::sync_keyboard_focus(session, serial);
    }
}

pub(super) fn close_blooms_for_keybind<D: SessionDriver>(
    session: &mut Session<D>,
    preferred_output: Option<&str>,
) -> bool {
    let open = session
        .wayland
        .space
        .outputs()
        .filter_map(|output| {
            let output_name = output.name();
            let cluster = session.clusters.bloom_open_on_output(&output_name)?;
            let core = session.clusters.core_node(cluster)?;
            Some((output_name, core))
        })
        .collect::<Vec<_>>();
    if open.is_empty() {
        return false;
    }
    let focus_core = preferred_output
        .and_then(|preferred| {
            open.iter()
                .find(|(output, _)| output == preferred)
                .map(|(_, core)| *core)
        })
        .or_else(|| {
            let focused = session.nodes.focused()?;
            open.iter()
                .find(|(_, core)| *core == focused)
                .map(|(_, core)| *core)
        })
        .or_else(|| open.first().map(|(_, core)| *core));
    let now = crate::frame_clock::monotonic_now();
    let changed = open.iter().fold(false, |changed, (output, _)| {
        session.clusters.close_bloom(output, now) || changed
    });
    if changed {
        crate::window::clear_focus(&mut session.wayland);
        session
            .nodes
            .focus(focus_core, session.start_time.elapsed().as_millis() as u64);
        crate::session::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
        session.request_redraw();
    }
    changed
}

pub(super) fn close_blooms_for_typing_away<D: SessionDriver>(session: &mut Session<D>) -> bool {
    let window_drag_active = matches!(
        &session.interactions.grab,
        crate::input::grab::Grab::PendingWindowMove(_)
            | crate::input::grab::Grab::MoveWindow { .. }
    );
    let focused = session
        .seat
        .get_keyboard()
        .and_then(|keyboard| keyboard.current_focus())
        .and_then(|focus| focus.wl_surface().map(|surface| surface.into_owned()))
        .and_then(|surface| session.nodes.id_for_surface(&surface));
    let open = session
        .wayland
        .space
        .outputs()
        .filter_map(|output| {
            let output = output.name();
            let cluster = session.clusters.bloom_open_on_output(&output)?;
            Some((output, cluster))
        })
        .collect::<Vec<_>>();
    let now = crate::frame_clock::monotonic_now();
    let changed = open.into_iter().fold(false, |changed, (output, cluster)| {
        let focused_owns_bloom = focused.is_some_and(|node| {
            session.clusters.cluster_for_member(node) == Some(cluster)
                || session.clusters.cluster_for_core(node) == Some(cluster)
        });
        if typing_abandons_bloom(window_drag_active, focused_owns_bloom) {
            session.clusters.close_bloom(&output, now) || changed
        } else {
            changed
        }
    });
    if changed {
        session.request_redraw();
    }
    changed
}

pub(crate) fn begin_cluster_commit<D: SessionDriver>(session: &mut Session<D>) -> bool {
    if !session.shell.cluster_composer.accepts_input() {
        return false;
    }
    let focused = session.nodes.focused();
    let output = session
        .clusters
        .creation()
        .map(|creation| creation.output.clone());
    let fallback_core_position = output
        .as_deref()
        .and_then(|output_name| {
            let output = session
                .wayland
                .space
                .outputs()
                .find(|candidate| candidate.name() == output_name)?;
            let geometry = session.wayland.space.output_geometry(output)?;
            let view = session.cameras.view(output_name)?;
            Some(halley_core::field::Vec2 {
                x: geometry.loc.x as f32 + view.center.x,
                y: geometry.loc.y as f32 + view.center.y,
            })
        })
        .unwrap_or(halley_core::field::Vec2 { x: 0.0, y: 0.0 });
    let now = crate::frame_clock::monotonic_now();
    match session
        .clusters
        .prepare_creation(&session.nodes.field, focused, fallback_core_position)
    {
        Ok(prepared) => {
            if session
                .shell
                .cluster_composer
                .begin_commit(prepared, session.settings.apogee, now)
            {
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Modal, None);
                session.request_redraw();
                true
            } else {
                session.clusters.abort_prepared_creation();
                false
            }
        }
        Err(message) => {
            eventline::warn!("clusters: {message}");
            if let Some(output) = output {
                session
                    .shell
                    .overlays
                    .show_error(output, &message, 3_000, now);
            }
            session.request_redraw();
            false
        }
    }
}

pub(crate) fn finish_cluster_creation<D: SessionDriver>(session: &mut Session<D>) -> bool {
    if session.clusters.renaming_target().is_some() {
        let output = session
            .clusters
            .creation()
            .map(|creation| creation.output.clone());
        return match session.clusters.finish_rename() {
            Ok(_) => {
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Modal, None);
                session.request_redraw();
                true
            }
            Err(message) => {
                eventline::warn!("clusters: {message}");
                if let Some(output) = output {
                    session.shell.overlays.show_error(
                        output,
                        &message,
                        3_000,
                        crate::frame_clock::monotonic_now(),
                    );
                }
                session.request_redraw();
                false
            }
        };
    }
    let draft_id = session.clusters.creation_draft_id();
    if let Some(confirmation) = session
        .clusters
        .confirm_draft(crate::frame_clock::monotonic_now())
    {
        let Some(wayland_display) = session.wayland_display.clone() else {
            eventline::warn!("clusters: cannot launch draft apps before Wayland is ready");
            return false;
        };
        let x11_display = session.xwayland.display_name();
        for launch in confirmation.launches {
            if !launch.command.trim().is_empty() {
                crate::session::spawn::spawn_detached(
                    &launch.command,
                    &wayland_display,
                    x11_display.as_deref(),
                    session.cursor.size(),
                    session.launch.environment(),
                );
            }
        }
        session
            .cursor
            .set_override(crate::cursor::OverrideSource::Modal, None);
        crate::ipc::publish_cluster_draft(
            session,
            confirmation.id,
            halley_ipc::ClusterDraftState::Launching,
            None,
        );
        session.request_redraw();
        return true;
    }
    let prepared = session.clusters.prepared_creation().cloned();
    let focused_before = session.nodes.focused();
    match session.clusters.finish_creation(&mut session.nodes.field) {
        Ok(id) => {
            crate::nodes::resolve_new_cluster_core(session, id);
            let focus_core = prepared.map_or_else(
                || {
                    focused_before.is_some_and(|focused| {
                        session.clusters.cluster_for_member(focused) == Some(id)
                    })
                },
                |prepared| prepared.focus_core,
            );
            if focus_core {
                crate::window::clear_focus(&mut session.wayland);
                session.nodes.focus(
                    session.clusters.core_node(id),
                    session.start_time.elapsed().as_millis() as u64,
                );
            }
            crate::session::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
            session
                .cursor
                .set_override(crate::cursor::OverrideSource::Modal, None);
            if let Some(draft_id) = draft_id {
                crate::ipc::publish_cluster_draft(
                    session,
                    draft_id,
                    halley_ipc::ClusterDraftState::Completed,
                    None,
                );
            }
            true
        }
        Err(message) => {
            eventline::warn!("clusters: {message}");
            false
        }
    }
}

pub(super) fn bloom_drag_handoff(
    window_location: Point<i32, Logical>,
    window_size: smithay::utils::Size<i32, Logical>,
    pointer_world: halley_core::field::Vec2,
) -> (halley_core::field::Vec2, halley_core::field::Vec2) {
    let source_offset = halley_core::field::Vec2 {
        x: window_location.x as f32 - pointer_world.x,
        y: window_location.y as f32 - pointer_world.y,
    };
    let center = halley_core::field::Vec2 {
        x: window_location.x as f32 + window_size.w as f32 * 0.5,
        y: window_location.y as f32 + window_size.h as f32 * 0.5,
    };
    (source_offset, center)
}

pub(crate) fn wakeup_cluster_interactions<D: SessionDriver>(
    session: &mut Session<D>,
    now: std::time::Duration,
) -> bool {
    let mut changed = session.clusters.repeat_name_input_if_due(now);
    changed |= crate::session::expire_cluster_draft(session, now);
    changed |= session.clusters.overflow_wakeup(now);
    changed |= session.clusters.tick_join_candidate_ready(now);

    let Some((cluster_id, member_id, output_name, tether_started)) = session.clusters.bloom_pull()
    else {
        return changed;
    };
    let Some(tether_started) = tether_started else {
        return changed;
    };
    if now.saturating_sub(tether_started) < crate::clusters::DETACH_HOLD_DURATION {
        return changed;
    }
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)
        .cloned()
    else {
        session.clusters.clear_bloom_pull();
        return true;
    };
    let Some(output_geometry) = session.wayland.space.output_geometry(&output) else {
        session.clusters.clear_bloom_pull();
        return true;
    };
    let Some(camera) = session.cameras.get(&output_name) else {
        session.clusters.clear_bloom_pull();
        return true;
    };
    let position = session.pointer.position();
    let world = crate::input::grab::screen_to_world_on_output(position, camera, output_geometry);
    if !session
        .clusters
        .detach_member(&mut session.nodes.field, cluster_id, member_id, world, now)
    {
        session.clusters.clear_bloom_pull();
        return true;
    }
    let _ = session.clusters.begin_field_drag(member_id);
    session.clusters.force_close_bloom(&output_name);
    session.clusters.set_overlay_hovered(None);
    crate::nodes::set_collapsed_output(session, member_id, &output);
    session.nodes.clear_direct_motion(member_id);
    let _ = crate::nodes::move_grabbed_body_rigid(session, member_id, world);
    if let Some(record) = session.nodes.record(member_id).cloned() {
        wayland::set_window_output(&record.window, &output);
        let window_location = session
            .wayland
            .space
            .element_location(&record.window)
            .unwrap_or(record.geometry.loc);
        let window_size = session
            .wayland
            .space
            .element_geometry(&record.window)
            .map(|geometry| geometry.size)
            .unwrap_or(record.geometry.size);
        let (source_offset, center) = bloom_drag_handoff(window_location, window_size, world);
        session.interactions.grab = crate::input::grab::Grab::MoveWindow {
            id: Some(member_id),
            window: record.window.clone(),
            cluster_drag: None,
            drag_size: None,
            button: BTN_LEFT,
            client_owned: false,
            anchor: crate::input::grab::WindowGrabAnchor::Source(source_offset),
            edge_pan: None,
            last_world: center,
            last_update: now,
            velocity: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
        };
        // The bloom press was compositor-only. Retire its suppression at the
        // handoff so the physical release reaches the MoveWindow cleanup
        // below; that cleanup still intercepts it, so no orphan release is
        // forwarded to the newly focused client.
        session
            .interactions
            .suppressed_buttons
            .release_is_suppressed(BTN_LEFT);
        crate::session::focus_window(session, &record.window, SERIAL_COUNTER.next_serial());
    }
    session.cursor.set_override(
        crate::cursor::OverrideSource::Grab,
        Some(smithay::input::pointer::CursorIcon::Grabbing),
    );
    true
}
