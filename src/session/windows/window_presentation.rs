//! Fullscreen, maximize, and window presentation handoffs.

use super::*;

pub(super) fn toggle_focused_fullscreen<D: SessionDriver>(
    session: &mut Session<D>,
    output: Option<&str>,
) {
    let Some(record) = focused_window_record(session, output) else {
        return;
    };
    let entering = !session.fullscreen.is_fullscreen_or_pending(&record.surface);
    set_record_fullscreen(session, record, entering);
}

/// Apply an opening-only rule after a window has acquired a node and output.
pub(crate) fn apply_initial_window_presentation<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    rule: &crate::window::rules::ResolvedWindowRule,
) {
    if rule.open_fullscreen {
        if !session.fullscreen.is_fullscreen_or_pending(surface)
            && let Some(record) = session
                .nodes
                .id_for_surface(surface)
                .and_then(|id| session.nodes.record(id))
                .filter(|record| !record.collapsed)
                .cloned()
        {
            set_record_fullscreen(session, record, true);
        }
    } else if rule.open_maximized {
        set_surface_field_maximized(session, surface, true);
    }
}

pub(super) fn set_record_fullscreen<D: SessionDriver>(
    session: &mut Session<D>,
    record: crate::nodes::NodeRecord,
    entering: bool,
) {
    set_record_fullscreen_with_release(session, record, entering, false);
}

/// Navigation exits both client and compositor fullscreen ownership. This is
/// distinct from Mod+F, which releases only compositor ownership.
pub(crate) fn leave_fullscreen_for_navigation<D: SessionDriver>(
    session: &mut Session<D>,
    record: crate::nodes::NodeRecord,
) {
    let surface = record.surface.clone();
    set_record_fullscreen_with_release(session, record, false, true);
    session.fullscreen.release_output_for_navigation(&surface);
}

pub(super) fn set_record_fullscreen_with_release<D: SessionDriver>(
    session: &mut Session<D>,
    record: crate::nodes::NodeRecord,
    entering: bool,
    release_all: bool,
) {
    let focused = record.surface;
    let window = record.window;
    let output_name =
        crate::wayland::window_output_name(&window).unwrap_or_else(|| record.output.clone());
    let now = crate::frame_clock::monotonic_now();
    cancel_grab_for_surface(session, &focused);
    let cluster_restore = cluster_presentation_restore(session, &focused, now, entering);
    let entering_output_rect = entering
        .then(|| {
            session
                .wayland
                .space
                .outputs()
                .find(|output| output.name() == output_name)
                .cloned()
                .and_then(|output| presented_window_rect(session, &window, &output, now))
        })
        .flatten();
    if entering {
        displace_fullscreen_on_output(session, &output_name, &focused);
    }
    let field_handoff = entering
        .then(|| {
            prepare_field_maximize_fullscreen_handoff(
                session,
                &window,
                &focused,
                &output_name,
                &output_name,
                now,
            )
        })
        .flatten();
    if !entering && let Some(restore) = cluster_restore.as_ref() {
        session.fullscreen.override_restore_from_cluster(
            &focused,
            restore.geometry,
            restore.output.clone(),
            restore.presentation_output,
        );
    }
    if let Some(toplevel) = window.toplevel() {
        if release_all
            || session
                .fullscreen
                .compositor_request_changes_visual(&focused, entering)
        {
            let textures = &mut session.render.fullscreen_textures;
            let capture = session.driver.with_renderer(|renderer| {
                textures.capture_previous(
                    renderer,
                    &window,
                    crate::render::fullscreen_texture::TextureTransitionOwner::Fullscreen,
                )
            });
            if let Err(err) = capture {
                eventline::warn!("fullscreen: failed to capture outgoing window texture: {err}");
            }
        }
        if entering {
            session.fullscreen.request_compositor(
                &mut session.wayland,
                toplevel,
                field_handoff.is_some(),
            );
        } else if release_all {
            session.fullscreen.unrequest(&session.wayland, toplevel);
            session.fullscreen.discard_snapshot_requests(&focused);
        } else {
            session
                .fullscreen
                .unrequest_compositor(&session.wayland, toplevel);
        }
    } else {
        crate::xwayland::set_window_fullscreen(session, &window, entering);
    }
    if let Some(presentation_output) = entering_output_rect {
        session
            .fullscreen
            .override_presentation_output(&focused, presentation_output);
    }
    if entering && let Some(restore) = cluster_restore {
        session.fullscreen.override_restore_from_cluster(
            &focused,
            restore.geometry,
            restore.output,
            restore.presentation_output,
        );
    }
    if let Some(handoff) = field_handoff {
        handoff.apply(&mut session.fullscreen, &focused);
    }
    pointer::reconcile_state(session);
    session.request_redraw();
}

pub(crate) struct FieldMaximizeFullscreenHandoff {
    restore: crate::presentation::maximize::FieldRestore,
    restore_output_rect: Option<Rectangle<i32, smithay::utils::Physical>>,
    geometry: Rectangle<i32, Logical>,
    field_output_rect: Option<Rectangle<i32, smithay::utils::Physical>>,
}

pub(crate) fn presentation_workspace_for_surface<D: SessionDriver>(
    session: &Session<D>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> crate::presentation::PresentationWorkspace {
    crate::presentation::workspace_for_surface(&session.clusters, &session.nodes, surface)
}

impl FieldMaximizeFullscreenHandoff {
    pub(crate) fn apply(
        self,
        fullscreen: &mut crate::wayland::fullscreen::FullscreenManager,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        fullscreen.override_restore_from_field(
            surface,
            self.restore.geometry,
            self.restore.output,
            self.restore_output_rect,
            self.geometry,
            self.field_output_rect,
        );
    }
}

/// Ends field maximize before fullscreen takes ownership of the same output.
///
/// Client-originated fullscreen requests must use this path too. Leaving the
/// maximize manager alive underneath fullscreen makes camera ownership snap
/// back and forth, which sends the window diagonally away from its anchored
/// presentation on both entry and exit.
pub(crate) fn prepare_field_maximize_fullscreen_handoff<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    maximize_output: &str,
    fullscreen_output: &str,
    now: std::time::Duration,
) -> Option<FieldMaximizeFullscreenHandoff> {
    let workspace = presentation_workspace_for_surface(session, surface);
    let same_surface = session.maximize.contains(surface);
    let field_output_rect = (same_surface && maximize_output == fullscreen_output)
        .then(|| {
            session
                .wayland
                .space
                .outputs()
                .find(|output| output.name() == maximize_output)
                .cloned()
                .and_then(|output| presented_window_rect(session, window, &output, now))
        })
        .flatten();
    let restore_output_rect = session.maximize.restore_presentation_output(surface);
    let restore = session
        .maximize
        .take_scope_restore(maximize_output, workspace)?;
    let geometry = (restore.surface == *surface)
        .then(|| session.wayland.space.element_geometry(window))
        .flatten();

    session.render.fullscreen_textures.remove(&restore.surface);
    let camera_handoff = restore.surface == *surface
        && maximize_output == fullscreen_output
        && session
            .cameras
            .handoff_field_maximize_to_fullscreen(maximize_output);
    if !camera_handoff {
        let _ = session.cameras.apply_field_maximize(maximize_output, None);
    }
    if restore.surface != *surface {
        configure_field_geometry(session, &restore);
    }

    geometry.map(|geometry| FieldMaximizeFullscreenHandoff {
        restore,
        restore_output_rect,
        geometry,
        field_output_rect,
    })
}

pub(super) fn displace_fullscreen_on_output<D: SessionDriver>(
    session: &mut Session<D>,
    output: &str,
    except: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) {
    let workspace = presentation_workspace_for_surface(session, except);
    let occupants = session
        .fullscreen
        .occupants_on_output(output, except)
        .into_iter()
        .filter(|(surface, _)| presentation_workspace_for_surface(session, surface) == workspace)
        .collect::<Vec<_>>();
    for (surface, origin) in occupants {
        let restore = session.fullscreen.restore_location(&surface);
        let window = session
            .nodes
            .id_for_surface(&surface)
            .and_then(|id| session.nodes.record(id))
            .map(|record| record.window.clone())
            .or_else(|| {
                session
                    .wayland
                    .space
                    .elements()
                    .find(|window| {
                        window
                            .wl_surface()
                            .is_some_and(|candidate| candidate.as_ref() == &surface)
                    })
                    .cloned()
            });
        if let Some(window) = window {
            if let Some(toplevel) = window.toplevel() {
                session.fullscreen.unrequest(&session.wayland, toplevel);
            } else if origin == crate::wayland::fullscreen::FullscreenOrigin::Maximize {
                crate::xwayland::restore_maximized_window(session, &window);
            } else {
                crate::xwayland::set_window_fullscreen(session, &window, false);
            }
            if let Some((location, restore_output)) = restore {
                if let Some(restore_output) = restore_output.as_deref().and_then(|name| {
                    session
                        .wayland
                        .space
                        .outputs()
                        .find(|candidate| candidate.name() == name)
                        .cloned()
                }) {
                    crate::wayland::set_window_output(&window, &restore_output);
                }
                session.wayland.space.relocate_element(&window, location);
            }
        }
        session.fullscreen.remove(&surface);
        session.render.fullscreen_textures.remove(&surface);
    }
}

pub(crate) fn forget_destroyed_cluster_member<D: SessionDriver>(
    session: &mut Session<D>,
    id: halley_core::field::NodeId,
) {
    let work_area = session
        .nodes
        .record(id)
        .and_then(|record| {
            session
                .wayland
                .space
                .outputs()
                .find(|output| output.name() == record.output)
        })
        .map(|output| smithay::desktop::layer_map_for_output(output).non_exclusive_zone());
    if let Some(work_area) = work_area {
        session.clusters.forget_destroyed_member_animated(
            &mut session.nodes.field,
            id,
            work_area,
            crate::frame_clock::monotonic_now(),
        );
    } else {
        session
            .clusters
            .forget_destroyed_member(&mut session.nodes.field, id);
    }
}

pub(super) fn toggle_focused_field_maximize<D: SessionDriver>(
    session: &mut Session<D>,
    output: Option<&str>,
) {
    let Some(record) = focused_window_record(session, output) else {
        return;
    };
    let _ = toggle_field_maximize(session, record);
}

/// Window actions follow the compositor's live client focus first. The node
/// focus is persistent by design and can lag briefly during hover transitions;
/// using it first made maximize/fullscreen target an older cluster member.
pub(crate) fn set_surface_field_maximized<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    maximized: bool,
) -> bool {
    // Client-side title bars can issue an interactive move on press before a
    // double-click state request. Retire that pending move first so maximize
    // and unmaximize use the same transaction as the title-bar button.
    cancel_grab_for_surface(session, surface);
    if session.maximize.contains(surface) == maximized {
        return false;
    }
    let Some(record) = session
        .nodes
        .id_for_surface(surface)
        .and_then(|id| session.nodes.record(id))
        .filter(|record| !record.collapsed)
        .cloned()
    else {
        return false;
    };
    toggle_field_maximize(session, record)
}

pub(super) fn toggle_field_maximize<D: SessionDriver>(
    session: &mut Session<D>,
    record: crate::nodes::NodeRecord,
) -> bool {
    let output_name =
        crate::wayland::window_output_name(&record.window).unwrap_or_else(|| record.output.clone());
    let Some(target_output) = session
        .wayland
        .space
        .outputs()
        .find(|candidate| candidate.name() == output_name)
        .cloned()
    else {
        return false;
    };
    let Some(output_geometry) = session.wayland.space.output_geometry(&target_output) else {
        return false;
    };
    let usable = smithay::desktop::layer_map_for_output(&target_output).non_exclusive_zone();
    let gap = session.settings.field.gap.ceil() as i32;
    let outer_target = Rectangle::new(
        output_geometry.loc
            + usable.loc
            + smithay::utils::Point::<i32, smithay::utils::Logical>::from((gap, gap)),
        (
            usable.size.w.saturating_sub(gap.saturating_mul(2)).max(1),
            usable.size.h.saturating_sub(gap.saturating_mul(2)).max(1),
        )
            .into(),
    );
    let target = crate::titlebar::client_rect_for_outer(
        &record.window,
        outer_target,
        &session.settings.decorations,
        &session.settings.font,
    );
    let now = crate::frame_clock::monotonic_now();
    let entering = !session.maximize.contains(&record.surface);
    if entering {
        session.cameras.cancel_queued_field_center(&output_name);
    }
    let workspace = presentation_workspace_for_surface(session, &record.surface);
    // A maximize entry owns the original windowed placement for its entire
    // lifetime, including an in-flight unmaximize/re-maximize reversal. The
    // live Space geometry is already the maximized configure by the time a
    // client requests unmaximize and must never replace this snapshot.
    let tracked_restore = session.maximize.restore(&record.surface);
    let cluster_restore = cluster_presentation_restore(session, &record.surface, now, entering);
    let inherited_restore = session.fullscreen.restore_placement(&record.surface);
    let inherited_restore_output_rect = session
        .fullscreen
        .restore_presentation_output(&record.surface);
    let Some(restore_geometry) = tracked_restore
        .as_ref()
        .map(|restore| restore.geometry)
        .or_else(|| inherited_restore.as_ref().map(|(geometry, _)| *geometry))
        .or_else(|| cluster_restore.as_ref().map(|restore| restore.geometry))
        .or_else(|| session.wayland.space.element_geometry(&record.window))
    else {
        return false;
    };
    let restore_output = tracked_restore
        .as_ref()
        .map(|restore| restore.output.clone())
        .or_else(|| inherited_restore.and_then(|(_, output)| output))
        .or_else(|| {
            cluster_restore
                .as_ref()
                .map(|restore| restore.output.clone())
        })
        .unwrap_or_else(|| output_name.clone());

    let handoff_from_fullscreen = session.fullscreen.has_presentation(&record.surface, now);
    let handoff_output_rect = handoff_from_fullscreen
        .then(|| presented_window_rect(session, &record.window, &target_output, now))
        .flatten();
    let presentation_output = cluster_restore
        .as_ref()
        .and_then(|restore| restore.presentation_output)
        .or(inherited_restore_output_rect)
        .or_else(|| {
            entering
                .then(|| presented_window_rect(session, &record.window, &target_output, now))
                .flatten()
        });
    if session.maximize.animations_enabled() {
        let textures = &mut session.render.fullscreen_textures;
        let capture = session.driver.with_renderer(|renderer| {
            textures.capture_previous(
                renderer,
                &record.window,
                crate::render::fullscreen_texture::TextureTransitionOwner::Maximize,
            )
        });
        if let Err(err) = capture {
            eventline::warn!("maximize: failed to capture previous window texture: {err}");
        }
    }
    cancel_grab_for_surface(session, &record.surface);
    if entering {
        displace_fullscreen_on_output(session, &output_name, &record.surface);
    }
    // Maximizing out of fullscreen, including an unfinished exit, hands the
    // whole travel to maximize: it eases from the current window rect down to
    // the maximized rect. Letting fullscreen arm its own exit
    // transition instead would run two timelines at once, and fullscreen wins
    // in `window_visual_state`, so the shrink toward the small windowed rect is
    // what you would see until it retired and the maximize track took over
    // mid-flight.
    let handoff_geometry = handoff_from_fullscreen
        .then(|| {
            let geometry = session.wayland.space.element_geometry(&record.window);
            session
                .cameras
                .handoff_fullscreen_to_field_maximize(&output_name);
            if let Some(toplevel) = record.window.toplevel() {
                session.fullscreen.retire_for_handoff(toplevel);
            } else {
                crate::xwayland::set_window_fullscreen(session, &record.window, false);
                session.fullscreen.remove(&record.surface);
            }
            geometry
        })
        .flatten();
    if handoff_geometry.is_none() {
        let camera_progress = session
            .maximize
            .camera_progress(&target_output, workspace, now);
        session
            .cameras
            .clear_field_maximize_handoff(&output_name, camera_progress);
    }
    let change = session.maximize.toggle(
        &target_output,
        workspace,
        crate::presentation::maximize::FieldRestore {
            surface: record.surface.clone(),
            geometry: restore_geometry,
            output: restore_output,
        },
        target,
        presentation_output,
        now,
    );
    if let Some(handoff_geometry) = handoff_geometry {
        session.maximize.override_windowed_from_fullscreen(
            &record.surface,
            handoff_geometry,
            handoff_output_rect,
        );
    }
    if let Some(displaced) = change.displaced.as_ref() {
        session
            .render
            .fullscreen_textures
            .remove(&displaced.surface);
        configure_field_geometry(session, displaced);
    }
    if entering {
        // Maximize takes the top stack slot once, matching fullscreen entry.
        // This keeps windows that were already in front from riding over the
        // maximize transition, without creating an always-on-top layer: any
        // later explicit raise can still move another window above it.
        crate::window::raise_managed(&mut session.wayland, &record.window);
        session.xwayland.raise_window(&record.window);
    }
    configure_field_geometry(
        session,
        &crate::presentation::maximize::FieldRestore {
            surface: record.surface,
            geometry: change.geometry,
            output: change.output,
        },
    );
    pointer::reconcile_state(session);
    session.request_redraw();
    true
}

#[derive(Clone, Debug)]
pub(crate) struct ClusterPresentationRestore {
    pub(crate) geometry: Rectangle<i32, Logical>,
    pub(crate) output: String,
    pub(crate) presentation_output: Option<Rectangle<i32, smithay::utils::Physical>>,
}

pub(crate) fn cluster_presentation_restore<D: SessionDriver>(
    session: &Session<D>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    now: std::time::Duration,
    entering: bool,
) -> Option<ClusterPresentationRestore> {
    let id = session.nodes.id_for_surface(surface)?;
    let cluster = session.clusters.cluster_for_member(id)?;
    let metadata = session.clusters.metadata(cluster)?;
    (session.clusters.active_on(&metadata.output) == Some(cluster)).then_some(())?;
    let output = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == metadata.output)?;
    let output_geometry = session.wayland.space.output_geometry(output)?;
    let work_area = smithay::desktop::layer_map_for_output(output).non_exclusive_zone();
    let target = session.clusters.workspace_surface_target_for(
        id,
        &metadata.output,
        work_area,
        output_geometry,
    )?;
    let window = session.nodes.record(id)?.window.clone();
    let geometry = if session.clusters.member_layout(id)
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
    let tiled = Rectangle::new(
        (geometry.loc - output_geometry.loc).to_physical(1),
        geometry.size.to_physical(1),
    );
    // Entry must start at the member's live, possibly animated tile. Exit
    // must finish at the latest layout target rather than reusing the current
    // fullscreen/maximized rectangle as its windowed endpoint.
    let presented = Some(
        entering
            .then(|| presented_window_rect(session, &window, output, now))
            .flatten()
            .unwrap_or(tiled),
    );
    Some(ClusterPresentationRestore {
        geometry,
        output: metadata.output.clone(),
        presentation_output: presented,
    })
}

pub(crate) fn presented_window_rect<D: SessionDriver>(
    session: &Session<D>,
    window: &smithay::desktop::Window,
    output: &smithay::output::Output,
    now: std::time::Duration,
) -> Option<Rectangle<i32, smithay::utils::Physical>> {
    crate::presentation::window::window_visual_state(
        &session.wayland.space,
        &session.cameras,
        Some(&session.clusters),
        Some(&session.nodes),
        window,
        output,
        &session.window_animations,
        &session.fullscreen,
        &session.maximize,
        &session.settings.decorations,
        &session.settings.font,
        now,
    )
    .map(|visual| visual.animated_rect)
}

pub(crate) fn configure_field_geometry<D: SessionDriver>(
    session: &mut Session<D>,
    request: &crate::presentation::maximize::FieldRestore,
) {
    let Some(window) = session
        .nodes
        .id_for_surface(&request.surface)
        .and_then(|id| session.nodes.record(id))
        .map(|record| record.window.clone())
    else {
        return;
    };
    if let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == request.output)
        .cloned()
    {
        crate::wayland::set_window_output(&window, &output);
    }
    session
        .wayland
        .space
        .relocate_element(&window, request.geometry.loc);
    if let Some(toplevel) = window.toplevel() {
        let maximized = session.maximize.contains(&request.surface);
        toplevel.with_pending_state(|pending| {
            pending.size = Some(request.geometry.size);
            pending.bounds = Some(request.geometry.size);
            if maximized {
                pending.states.set(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
                );
            } else {
                pending.states.unset(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
                );
            }
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_configure();
        }
    } else {
        crate::xwayland::set_maximized(&window, session.maximize.contains(&request.surface));
        crate::xwayland::configure_window(session, &window, request.geometry);
    }
}
