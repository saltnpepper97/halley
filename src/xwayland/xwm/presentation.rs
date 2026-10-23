use super::*;

pub(super) fn enter_fullscreen<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &X11Surface,
    origin: FullscreenRequestOrigin,
) {
    set_external_fullscreen(session, surface, true, origin);
}

pub(super) fn leave_fullscreen<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &X11Surface,
    origin: FullscreenRequestOrigin,
) {
    set_external_fullscreen(session, surface, false, origin);
}

pub(super) fn set_external_fullscreen<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &X11Surface,
    fullscreen: bool,
    origin: FullscreenRequestOrigin,
) {
    // Maximize uses the fullscreen presentation machinery internally, but it
    // must remain EWMH maximize to the X11 client. Advertising both
    // _NET_WM_STATE_MAXIMIZED_* and _NET_WM_STATE_FULLSCREEN confuses Steam's
    // client-side decoration state, so its second button press never asks to
    // unmaximize. Hyprland keeps this same client/internal-state distinction.
    let update_client_fullscreen = origin != FullscreenRequestOrigin::Maximize;
    if fullscreen && origin != FullscreenRequestOrigin::Maximize {
        let mut maximize = surface
            .user_data()
            .get_or_insert_threadsafe(MaximizeFullscreen::default)
            .0
            .lock()
            .expect("X11 maximize state lock poisoned");
        maximize.active = false;
        maximize.restore = None;
        if let Err(err) = surface.set_maximized(false) {
            eventline::warn!("xwayland: failed to clear maximized state for fullscreen: {err}");
        }
    }
    if session
        .xwayland
        .pending_windows
        .contains_key(&surface.window_id())
    {
        if update_client_fullscreen && let Err(err) = surface.set_fullscreen(fullscreen) {
            eventline::warn!("xwayland: failed to update pending fullscreen state: {err}");
        }
        return;
    }
    let Some(window) = window_for_surface(&session.wayland, &session.nodes, surface) else {
        return;
    };
    let rule = session
        .window_rules
        .resolve_identity(&crate::window::rules::identity(&window));
    let game =
        session
            .window_rules
            .games
            .is_game(&window, &session.wayland.display_handle, rule.game);
    // Exclusive games can answer a cross-monitor hover focus change with an
    // unset request. Keep that monitor's presentation while another monitor
    // is in use; a focused mode change and explicit compositor exit still work.
    let remote_focus_exit = game
        && !fullscreen
        && origin == FullscreenRequestOrigin::Client
        && session.fullscreen.external_desired_matches(&window, true)
        && window.wl_surface().is_some_and(|surface| {
            session.wayland.focused_window.as_ref() != Some(surface.as_ref())
        })
        && crate::wayland::focus::selected_output(&session.wayland).is_some_and(|output| {
            crate::wayland::window_output_name(&window).as_deref() != Some(output.name().as_str())
        });
    let fullscreen = fullscreen || remote_focus_exit;
    let now = crate::frame_clock::monotonic_now();
    if game
        && !remote_focus_exit
        && fullscreen
        && origin == FullscreenRequestOrigin::Client
        && let Some(surface) = window.wl_surface()
    {
        session
            .fullscreen
            .resume_external_presentation(surface.as_ref(), now);
    }
    let opening_animation = window.wl_surface().is_some_and(|wl_surface| {
        session
            .window_animations
            .is_animating(wl_surface.as_ref(), now)
    });
    // Source-engine clients briefly request fullscreen off/on while loading.
    // Once direct startup fullscreen has settled, preserve that geometry until
    // the independent window-open animation finishes. Explicit compositor
    // actions such as Mod+F still bypass this client-only guard.
    if origin == FullscreenRequestOrigin::Client
        && opening_animation
        && session.fullscreen.external_desired_matches(&window, true)
    {
        if !surface.is_fullscreen()
            && let Err(err) = surface.set_fullscreen(true)
        {
            eventline::warn!("xwayland: failed to retain startup fullscreen state: {err}");
        }
        eventline::debug!(
            "xwayland: ignored startup fullscreen chatter xid={} requested={fullscreen}",
            surface.window_id()
        );
        crate::session::reconcile_pointer_constraints(session);
        return;
    }
    let cluster_restore = window.wl_surface().and_then(|wl_surface| {
        crate::session::cluster_presentation_restore(session, wl_surface.as_ref(), now, fullscreen)
    });
    let field_output_rect = (fullscreen && cluster_restore.is_none())
        .then(|| {
            let output_name = crate::wayland::window_output_name(&window)?;
            session
                .wayland
                .space
                .outputs()
                .find(|output| output.name() == output_name)
                .cloned()
                .and_then(|output| {
                    crate::session::presented_window_rect(session, &window, &output, now)
                })
        })
        .flatten();
    let client_cluster_request = cluster_restore.is_some() && origin.client_owns_geometry();
    if client_cluster_request {
        let quiet_until = session
            .xwayland
            .arm_client_geometry_guard(surface.window_id(), now);
        if let Some(id) = window
            .wl_surface()
            .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
        {
            session.clusters.defer_surface_layout_until(id, quiet_until);
        }
    }
    let client_geometry_guarded = client_cluster_request
        && session
            .xwayland
            .client_geometry_guarded(surface.window_id(), now);
    let cluster_restore = if client_geometry_guarded {
        None
    } else {
        cluster_restore
    };
    if game
        && !fullscreen
        && origin != FullscreenRequestOrigin::Maximize
        && cluster_restore.is_none()
        && session.fullscreen.external_desired_matches(&window, true)
        && let Some(restore) =
            session
                .fullscreen
                .external_target_geometry(&session.wayland, &window, false)
    {
        // A game's first windowed video mode can arrive after its fullscreen
        // unset. Preserve the restored frame's center across that resize burst,
        // rather than anchoring the new size to the old fullscreen top-left.
        let outer = crate::titlebar::outer_rect_for_client(
            &window,
            restore,
            &session.settings.decorations,
            &session.settings.font,
        );
        session
            .xwayland
            .arm_client_restore_geometry_guard(surface.window_id(), now, outer);
    }
    if !fullscreen && let Some(restore) = cluster_restore.as_ref() {
        session.fullscreen.override_restore_from_cluster(
            window
                .wl_surface()
                .expect("managed X11 windows have a Wayland surface")
                .as_ref(),
            restore.geometry,
            restore.output.clone(),
            restore.presentation_output,
        );
    }
    if compositor_fullscreen_should_raise(fullscreen, origin) && cluster_restore.is_none() {
        crate::window::raise_managed(&mut session.wayland, &window);
        session.xwayland.raise_window(&window);
    }
    let state_changed = surface.is_fullscreen() != fullscreen;
    if update_client_fullscreen && let Err(err) = surface.set_fullscreen(fullscreen) {
        eventline::warn!("xwayland: failed to update fullscreen state: {err}");
    }
    if game {
        session.xwayland.sync_frame_extents(
            &window,
            &session.settings.decorations,
            &session.settings.font,
        );
    }
    if session
        .fullscreen
        .external_desired_matches(&window, fullscreen)
    {
        if game && origin == FullscreenRequestOrigin::Client {
            if !state_changed
                && let Some(control) = session.xwayland.control.as_ref()
                && let Err(err) = control.acknowledge_fullscreen(surface.window_id(), fullscreen)
            {
                eventline::warn!("xwayland: failed to acknowledge game fullscreen state: {err}");
            }
            // Reply at the accepted target without beginning another visual
            // transaction or restarting its smooth geometry timeline.
            let geometry = session
                .fullscreen
                .external_configure_geometry(&session.wayland, &window);
            if let Err(err) = surface.configure(geometry) {
                eventline::warn!("xwayland: failed to acknowledge game fullscreen geometry: {err}");
            }
        }
        eventline::debug!(
            "xwayland: coalesced fullscreen request xid={} fullscreen={fullscreen} \
             origin={origin:?}",
            surface.window_id()
        );
        crate::session::reconcile_pointer_constraints(session);
        return;
    }
    let field_handoff = (fullscreen && origin != FullscreenRequestOrigin::Maximize)
        .then(|| {
            let wl_surface = window.wl_surface()?;
            let output = crate::wayland::window_output_name(&window)?;
            crate::session::prepare_field_maximize_fullscreen_handoff(
                session,
                &window,
                wl_surface.as_ref(),
                &output,
                &output,
                now,
            )
        })
        .flatten();
    let opening = client_geometry_guarded || opening_animation;
    let policy = ExternalPresentationPolicy::select(
        origin,
        opening,
        crate::session::has_active_pointer_confinement(session),
        game,
    );
    if policy == ExternalPresentationPolicy::Opening {
        preserve_opening_center(session, surface, &window);
    }
    // Games can toggle fullscreen off/on while retaining their swapchain.
    // X11 has no xdg-style state acknowledgement, and an unchanged buffer
    // need never receive full damage again. Waiting for a resized repaint in
    // that case leaves the outgoing snapshot displayed indefinitely.
    let state_only = session
        .fullscreen
        .external_target_geometry(&session.wayland, &window, fullscreen)
        .is_some_and(|target| {
            window.wl_surface().is_some_and(|surface| {
                smithay::backend::renderer::utils::with_renderer_surface_state(
                    surface.as_ref(),
                    |state| state.surface_size() == Some(target.size),
                )
                .unwrap_or(false)
            })
        });
    let live_buffers = policy == ExternalPresentationPolicy::LiveAnimated;
    let animate = !state_only
        && (live_buffers
            || (policy == ExternalPresentationPolicy::Animated
                && capture_fullscreen_snapshot(session, &window, fullscreen)));
    if policy == ExternalPresentationPolicy::Opening {
        request_opening_fullscreen(session, surface, &window, fullscreen, origin);
    } else if animate {
        if live_buffers {
            remove_fullscreen_snapshot(session, &window);
        }
        let request = if fullscreen {
            session.fullscreen.request_external_animated(
                &mut session.wayland,
                &window,
                origin.presentation_origin(),
                live_buffers,
            )
        } else {
            session
                .fullscreen
                .unrequest_external_animated(&window, live_buffers)
        };
        match request {
            Some(ExternalTransactionRequest::Configure(geometry)) => {
                if let Err(err) = surface.configure(geometry) {
                    eventline::warn!("xwayland: failed to configure animated fullscreen: {err}");
                    remove_fullscreen_snapshot(session, &window);
                    settle_external_immediately(session, surface, &window, fullscreen, origin);
                } else {
                    eventline::debug!(
                        "xwayland: fullscreen policy xid={} fullscreen={fullscreen} \
                         origin={origin:?} policy={}",
                        surface.window_id(),
                        policy.name(),
                    );
                }
            }
            Some(ExternalTransactionRequest::NoChange) => {
                remove_fullscreen_snapshot(session, &window);
            }
            None => {
                remove_fullscreen_snapshot(session, &window);
                settle_external_immediately(session, surface, &window, fullscreen, origin);
            }
        }
    } else {
        let reason = if state_only {
            "state-only"
        } else {
            policy.name()
        };
        eventline::debug!(
            "xwayland: fullscreen policy xid={} fullscreen={fullscreen} \
             origin={origin:?} policy={reason}",
            surface.window_id(),
        );
        remove_fullscreen_snapshot(session, &window);
        settle_external_immediately(session, surface, &window, fullscreen, origin);
    }
    if fullscreen && let Some(wl_surface) = window.wl_surface() {
        if let Some(restore) = cluster_restore {
            session.fullscreen.override_restore_from_cluster(
                wl_surface.as_ref(),
                restore.geometry,
                restore.output,
                restore.presentation_output,
            );
        } else if let Some(rect) = field_output_rect {
            session
                .fullscreen
                .override_presentation_output(wl_surface.as_ref(), rect);
        }
    }
    if let Some(handoff) = field_handoff
        && let Some(wl_surface) = window.wl_surface()
    {
        handoff.apply(&mut session.fullscreen, wl_surface.as_ref());
    }
    crate::session::reconcile_pointer_constraints(session);
}

pub(super) fn compositor_fullscreen_should_raise(
    fullscreen: bool,
    origin: FullscreenRequestOrigin,
) -> bool {
    fullscreen && origin == FullscreenRequestOrigin::Compositor
}

pub(super) fn preserve_opening_center<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &X11Surface,
    window: &Window,
) {
    let Some(placement) = session
        .xwayland
        .opening_placements
        .get(&surface.window_id())
        .copied()
    else {
        return;
    };
    let centered = placement.centered(window.geometry().size);
    session.wayland.space.relocate_element(window, centered.loc);
}

pub(super) fn request_opening_fullscreen<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &X11Surface,
    window: &Window,
    fullscreen: bool,
    origin: FullscreenRequestOrigin,
) {
    let now = crate::frame_clock::monotonic_now();
    let Some((output, current_bounds)) = opening_presentation_bounds(session, window, now) else {
        settle_external_immediately(session, surface, window, fullscreen, origin);
        return;
    };
    let request = if fullscreen {
        let restore = session
            .xwayland
            .opening_placements
            .get(&surface.window_id())
            .and_then(|placement| placement.restore_geometry());
        session.fullscreen.request_external_opening(
            &mut session.wayland,
            window,
            restore,
            origin.presentation_origin(),
        )
    } else {
        session.fullscreen.unrequest_external_opening(window)
    };
    match request {
        Some(ExternalTransactionRequest::Configure(geometry)) => {
            let target_bounds = opening_target_bounds(session, &output, geometry, fullscreen)
                .unwrap_or_else(|| Rectangle::new((0, 0).into(), geometry.size.to_physical(1)));
            let Some(wl_surface) = window.wl_surface() else {
                settle_external_immediately(session, surface, window, fullscreen, origin);
                return;
            };
            session.window_animations.retarget(
                wl_surface.as_ref(),
                now,
                current_bounds,
                target_bounds,
            );
            if let Err(err) = surface.configure(geometry) {
                eventline::warn!("xwayland: failed to configure opening fullscreen: {err}");
                settle_external_immediately(session, surface, window, fullscreen, origin);
            } else {
                eventline::debug!(
                    "xwayland: fullscreen policy xid={} fullscreen={fullscreen} policy=opening",
                    surface.window_id()
                );
            }
        }
        Some(ExternalTransactionRequest::NoChange) => {}
        None => settle_external_immediately(session, surface, window, fullscreen, origin),
    }
}

pub(super) fn opening_presentation_bounds<D: SessionDriver>(
    session: &Session<D>,
    window: &Window,
    now: std::time::Duration,
) -> Option<(Output, Rectangle<i32, smithay::utils::Physical>)> {
    let output = crate::wayland::window_output_name(window)
        .and_then(|name| {
            session
                .wayland
                .space
                .outputs()
                .find(|output| output.name() == name)
        })
        .or_else(|| crate::wayland::focus::selected_output(&session.wayland))
        .cloned()?;
    let output_geometry = session.wayland.space.output_geometry(&output)?;
    let presentation = crate::presentation::window::WindowPresentation::for_window(
        &session.wayland.space,
        &session.cameras,
        Some(&session.clusters),
        Some(&session.nodes),
        &session.window_animations,
        &session.fullscreen,
        &session.maximize,
        &session.settings.decorations,
        &session.settings.font,
        window,
        &output,
        now,
    )?;
    let local = presentation.visual_geometry().loc - output_geometry.loc;
    Some((
        output,
        Rectangle::new(
            local.to_physical(1),
            presentation.visual_geometry().size.to_physical(1),
        ),
    ))
}

pub(super) fn opening_target_bounds<D: SessionDriver>(
    session: &Session<D>,
    output: &Output,
    geometry: Rectangle<i32, Logical>,
    fullscreen: bool,
) -> Option<Rectangle<i32, smithay::utils::Physical>> {
    let output_geometry = session.wayland.space.output_geometry(output)?;
    let output_size = output_geometry.size.to_physical(1);
    if fullscreen {
        return Some(Rectangle::new((0, 0).into(), output_size));
    }
    let view = session.cameras.view(&output.name())?;
    Some(crate::render::camera_rect(
        geometry.to_physical(1),
        crate::presentation::camera::global_center(view.center, output_geometry),
        output_size,
        view.scale,
    ))
}

pub(super) fn settle_external_immediately<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &X11Surface,
    window: &Window,
    fullscreen: bool,
    origin: FullscreenRequestOrigin,
) {
    let geometry = if fullscreen {
        session.fullscreen.request_external(
            &mut session.wayland,
            window,
            origin.presentation_origin(),
        )
    } else {
        session
            .fullscreen
            .unrequest_external(&mut session.wayland, window)
    };
    if let Some(geometry) = geometry
        && let Err(err) = surface.configure(geometry)
    {
        eventline::warn!("xwayland: failed to configure fullscreen window: {err}");
    }
}

pub(super) fn capture_fullscreen_snapshot<D: SessionDriver>(
    session: &mut Session<D>,
    window: &Window,
    fullscreen: bool,
) -> bool {
    let Some(surface) = window.wl_surface().map(|surface| surface.into_owned()) else {
        return false;
    };
    if !session
        .fullscreen
        .should_capture_external_snapshot(&surface, fullscreen)
    {
        return false;
    }
    let textures = &mut session.render.fullscreen_textures;
    match session.driver.with_renderer(|renderer| {
        textures.capture_previous(
            renderer,
            window,
            crate::render::fullscreen_texture::TextureTransitionOwner::Fullscreen,
        )
    }) {
        Ok(()) => true,
        Err(err) => {
            eventline::warn!("fullscreen: failed to capture X11 window texture: {err}");
            false
        }
    }
}

pub(super) fn remove_fullscreen_snapshot<D: SessionDriver>(
    session: &mut Session<D>,
    window: &Window,
) {
    if let Some(surface) = window.wl_surface() {
        session.render.fullscreen_textures.remove(surface.as_ref());
    }
}

pub(crate) fn set_window_fullscreen<D: SessionDriver>(
    session: &mut Session<D>,
    window: &Window,
    fullscreen: bool,
) {
    let Some(surface) = window.x11_surface().cloned() else {
        return;
    };
    if fullscreen {
        enter_fullscreen(session, &surface, FullscreenRequestOrigin::Compositor);
    } else {
        leave_fullscreen(session, &surface, FullscreenRequestOrigin::Compositor);
    }
}

pub(crate) fn restore_maximized_window<D: SessionDriver>(
    session: &mut Session<D>,
    window: &Window,
) {
    if let Some(surface) = window.x11_surface().cloned() {
        restore_window(session, &surface);
    }
}

pub(super) fn restore_window<D: SessionDriver>(session: &mut Session<D>, surface: &X11Surface) {
    if session
        .xwayland
        .pending_windows
        .contains_key(&surface.window_id())
    {
        if let Some(state) = surface.user_data().get::<MaximizeFullscreen>() {
            let mut state = state.0.lock().expect("X11 maximize state lock poisoned");
            state.active = false;
            state.restore = None;
        }
        if let Err(err) = surface.set_maximized(false) {
            eventline::warn!("xwayland: failed to clear pending maximized state: {err}");
        }
        if let Err(err) = surface.set_fullscreen(false) {
            eventline::warn!("xwayland: failed to clear pending fullscreen state: {err}");
        }
        return;
    }
    if let Err(err) = surface.set_maximized(false) {
        eventline::warn!("xwayland: failed to clear maximized state: {err}");
    }
    let restore = surface
        .user_data()
        .get::<MaximizeFullscreen>()
        .and_then(|state| {
            let mut state = state.0.lock().expect("X11 maximize state lock poisoned");
            let restore = state.active.then(|| state.restore.clone()).flatten();
            state.active = false;
            state.restore = None;
            restore
        });
    if let Some(restore) = restore {
        let window = window_for_surface(&session.wayland, &session.nodes, surface);
        let fullscreen_active = window
            .as_ref()
            .is_some_and(|window| session.fullscreen.external_desired_matches(window, true));
        leave_fullscreen(session, surface, FullscreenRequestOrigin::Maximize);
        if !fullscreen_active && let Some(window) = window {
            if let Some(output) = restore
                .output
                .as_deref()
                .and_then(|name| output_named(&session.wayland, name))
            {
                crate::wayland::set_window_output(&window, &output);
            }
            session
                .wayland
                .space
                .relocate_element(&window, restore.geometry.loc);
            if let Err(err) = surface.configure(restore.geometry) {
                eventline::warn!("xwayland: failed to restore maximized geometry: {err}");
            }
        }
    }
}

pub(super) fn resize_handle(edge: ResizeEdge) -> crate::input::grab::ResizeHandle {
    use crate::input::grab::ResizeHandle;

    match edge {
        ResizeEdge::Top => ResizeHandle::Top,
        ResizeEdge::Bottom => ResizeHandle::Bottom,
        ResizeEdge::Left => ResizeHandle::Left,
        ResizeEdge::TopLeft => ResizeHandle::TopLeft,
        ResizeEdge::BottomLeft => ResizeHandle::BottomLeft,
        ResizeEdge::Right => ResizeHandle::Right,
        ResizeEdge::TopRight => ResizeHandle::TopRight,
        ResizeEdge::BottomRight => ResizeHandle::BottomRight,
    }
}

pub(super) fn evdev_button(x11_button: u32) -> Option<u32> {
    match x11_button {
        1 => Some(0x110),
        2 => Some(0x112),
        3 => Some(0x111),
        _ => None,
    }
}

pub(crate) fn reconfigure_fullscreen(windows: Vec<(Window, Rectangle<i32, Logical>)>) {
    for (window, geometry) in windows {
        let Some(surface) = window.x11_surface() else {
            continue;
        };
        if let Err(err) = surface.configure(geometry) {
            eventline::warn!("xwayland: failed to reconfigure fullscreen output: {err}");
        }
    }
}

pub(super) struct PositionResync {
    /// Override-redirect geometry is client-owned; Smithay rejects the configure.
    pub(super) override_redirect: bool,
    /// Map admission, the client-geometry quiet period, and fullscreen
    /// transactions each own the window's geometry and send their own
    /// configures. Resyncing underneath them would race the handoff.
    pub(super) owns_own_geometry: bool,
    pub(super) x_location: Point<i32, Logical>,
    pub(super) root_screen_location: Point<i32, Logical>,
}

/// Compare-before-configure: a settled desktop must send nothing, or the
/// per-dispatch sweep would spam every X11 client once a frame.
pub(super) fn position_resync_needed(resync: PositionResync) -> bool {
    !resync.override_redirect
        && !resync.owns_own_geometry
        && resync.x_location != resync.root_screen_location
}

fn presentation_for_window<D: SessionDriver>(
    session: &Session<D>,
    window: &Window,
    now: std::time::Duration,
) -> Option<(Output, crate::presentation::window::WindowPresentation)> {
    let output = crate::wayland::window_output_name(window)
        .and_then(|name| {
            session
                .wayland
                .space
                .outputs()
                .find(|output| output.name() == name)
        })
        .or_else(|| crate::wayland::focus::selected_output(&session.wayland))
        .cloned()?;
    let presentation = crate::presentation::window::WindowPresentation::for_window(
        &session.wayland.space,
        &session.cameras,
        Some(&session.clusters),
        Some(&session.nodes),
        &session.window_animations,
        &session.fullscreen,
        &session.maximize,
        &session.settings.decorations,
        &session.settings.font,
        window,
        &output,
        now,
    )?;
    Some((output, presentation))
}

pub(super) fn source_element_location_from_root_screen<D: SessionDriver>(
    session: &Session<D>,
    window: &Window,
    root_screen: Point<i32, Logical>,
    now: std::time::Duration,
) -> Option<Point<i32, Logical>> {
    let (_, presentation) = presentation_for_window(session, window, now)?;
    let source_root = presentation.root_source_from_screen(root_screen);
    Some(source_root + window.geometry().loc)
}

pub(super) fn owner_relative_source_point(
    owner_source_root: Point<i32, Logical>,
    owner_x_root: Point<i32, Logical>,
    child_x_root: Point<i32, Logical>,
) -> Point<i32, Logical> {
    owner_source_root + (child_x_root - owner_x_root)
}

pub(super) fn source_point_from_owner_x_root<D: SessionDriver>(
    session: &Session<D>,
    owner: &Window,
    child_x_root: Point<i32, Logical>,
    now: std::time::Duration,
) -> Option<Point<i32, Logical>> {
    let owner_x_root = owner.x11_surface()?.last_configure().loc;
    let (_, presentation) = presentation_for_window(session, owner, now)?;
    Some(owner_relative_source_point(
        presentation.root_source_origin(),
        owner_x_root,
        child_x_root,
    ))
}

fn presentation_geometry_is_moving<D: SessionDriver>(
    session: &Session<D>,
    window: &Window,
    output: &Output,
    now: std::time::Duration,
) -> bool {
    let camera_moving = session.cameras.get(&output.name()).is_some_and(|camera| {
        camera.center != camera.target_center
            || camera.view_size != camera.target_view_size
            || camera.pan_vel.x != 0.0
            || camera.pan_vel.y != 0.0
            || camera.zoom_log_vel != 0.0
    });
    let opening = window.wl_surface().is_some_and(|surface| {
        session
            .window_animations
            .is_animating(surface.as_ref(), now)
    });
    let grabbed = window.wl_surface().is_some_and(|surface| {
        crate::input::grab::belongs_to_surface(&session.interactions.grab, surface.as_ref())
    });
    camera_moving
        || opening
        || grabbed
        || session.nodes.has_physics_on_output(&output.name(), now)
        || session.clusters.is_animating_on_output(&output.name(), now)
        || session.fullscreen.is_animating_on_output(output, now)
        || session.maximize.is_animating(now)
}

/// Republishes a managed X11 window's position to the X server when the
/// compositor has moved it without configuring the client.
///
/// The compositor's `Space` and `X11Surface::last_configure()` are deliberately
/// separate stores. `Space` is Halley's Field/source coordinate system, while
/// X11 geometry is expressed in the fixed root-desktop coordinate system.
/// Publishing `Space::element_location` directly works only while an output's
/// camera transform is the identity and otherwise offsets every client-side
/// root-coordinate operation, including pointer hit testing and popup layout.
///
/// Only the position is sent; the size stays whatever the client last agreed
/// to. A position-only configure cannot make a client resize or map a second
/// time, which is what the SDL2 cursor-lock constraint depends on. Hyprland
/// resyncs the same way, and its `sendWindowSize` dedup treats a pure position
/// change as configure-worthy only for X11 windows.
///
/// Returns whether a configure was sent.
pub(crate) fn sync_position<D: SessionDriver>(session: &Session<D>, window: &Window) -> bool {
    let Some(surface) = window.x11_surface() else {
        return false;
    };
    let xid = surface.window_id();
    let now = crate::frame_clock::monotonic_now();
    let Some((output, presentation)) = presentation_for_window(session, window, now) else {
        return false;
    };
    let location = presentation.root_screen_origin();
    let current = surface.last_configure();
    let owns_own_geometry = session.xwayland.pending_windows.contains_key(&xid)
        || session.xwayland.client_geometry_guarded(xid, now)
        || presentation_geometry_is_moving(session, window, &output, now)
        || window.wl_surface().is_some_and(|wl_surface| {
            session.fullscreen.is_fullscreen_or_pending(&wl_surface)
                || session.fullscreen.awaits_external_configure(&wl_surface)
        });
    if !position_resync_needed(PositionResync {
        override_redirect: surface.is_override_redirect(),
        owns_own_geometry,
        x_location: current.loc,
        root_screen_location: location,
    }) {
        return false;
    }
    if let Err(err) = surface.configure(Rectangle::new(location, current.size)) {
        eventline::warn!("xwayland: failed to resync window position xid={xid}: {err}");
        return false;
    }
    eventline::debug!(
        "xwayland: published root position xid={xid} source={:?} from {:?} to {location:?}",
        session.wayland.space.element_location(window),
        current.loc,
    );
    true
}

/// Sweeps every managed X11 window for compositor/X position drift.
///
/// Runs once per dispatch next to `Space::refresh`. The compare-before-configure
/// check in [`sync_position`] is what keeps this cheap: a settled desktop sends
/// nothing.
pub(crate) fn sync_positions<D: SessionDriver>(session: &Session<D>) -> bool {
    let windows = session
        .wayland
        .space
        .elements()
        .filter(|window| window.x11_surface().is_some())
        .cloned()
        .collect::<Vec<_>>();
    // Deliberately not `any`: every drifted window must be resynced, and `any`
    // would stop at the first one.
    let mut synced = false;
    for window in &windows {
        synced |= sync_position(session, window);
    }
    synced
}

pub(crate) fn configure_window<D: SessionDriver>(
    session: &mut Session<D>,
    window: &Window,
    geometry: Rectangle<i32, Logical>,
) {
    let Some(surface) = window.x11_surface() else {
        return;
    };
    // Placement belongs to Halley's Field.  X11 only needs a configure while
    // the native client size changes; the settled presentation sweep publishes
    // the corresponding root-desktop position after any motion has finished.
    session.wayland.space.relocate_element(window, geometry.loc);
    let size = super::configure::constrain_surface_size(surface, geometry.size);
    let current = surface.last_configure();
    if current.size == size {
        return;
    }
    if let Err(err) = surface.configure(Rectangle::new(current.loc, size)) {
        eventline::warn!("xwayland: failed to configure window geometry: {err}");
    }
}
