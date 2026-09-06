use super::state::WindowState;
use super::*;
use x11rb::properties::WmHintsState;

fn sync_allowed_actions(
    control: Option<&crate::xwayland::control::X11Control>,
    surface: &X11Surface,
) {
    let Some(control) = control else {
        return;
    };
    let fixed_size = surface
        .min_size()
        .zip(surface.max_size())
        .is_some_and(|(minimum, maximum)| minimum == maximum);
    let actions = crate::xwayland::control::AllowedActions {
        resize: !fixed_size,
        maximize: !fixed_size,
        ..Default::default()
    };
    if let Err(err) = control.set_allowed_actions(surface.window_id(), actions) {
        eventline::warn!(
            "xwayland: failed to publish allowed actions xid={}: {err}",
            surface.window_id()
        );
    }
}

fn sync_urgency(surface: &X11Surface) {
    if !surface.hints().is_some_and(|hints| hints.urgent) || surface.demands_attention() {
        return;
    }
    if let Err(err) = surface.set_demands_attention(true) {
        eventline::warn!(
            "xwayland: failed to apply WM_HINTS urgency xid={}: {err}",
            surface.window_id()
        );
    }
}

/// Drops every compositor-side record of a managed X11 window.
///
/// Returns whether the window was actually being managed. Callers use that to
/// decide whether an ICCCM withdrawal is warranted: a reparenting window
/// manager also receives the `UnmapNotify` its own `ReparentWindow` caused, and
/// deleting `WM_STATE` off the back of one of those strands a live, mapped
/// window in the Withdrawn state.
pub(super) fn forget_window<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &X11Surface,
) -> bool {
    session
        .xwayland
        .forget_client_geometry_guard(surface.window_id());
    session.xwayland.forget_frame_extents(surface.window_id());
    let was_pending = session
        .xwayland
        .pending_windows
        .remove(&surface.window_id())
        .is_some();
    session
        .xwayland
        .opening_placements
        .remove(&surface.window_id());
    let Some(window) = window_for_surface(&session.wayland, &session.nodes, surface) else {
        return was_pending;
    };
    if session.interactions.steam_close_pressed.as_ref() == Some(&window) {
        session.interactions.steam_close_pressed = None;
    }
    if let Some(geometry) = session.wayland.space.element_geometry(&window) {
        remember_normal_size(session, surface, geometry.size);
    }
    let close_active = crate::session::closing::capture_and_activate_before_unmap(session, &window);
    crate::session::trace::x11_event(
        session,
        surface,
        "close-handoff",
        format_args!("active_before_unmap={close_active}"),
    );
    if let Some(wl_surface) = window.wl_surface().map(|surface| surface.into_owned()) {
        session.window_rules.forget(&wl_surface);
        let preparation = crate::session::prepare_window_unmap(session, &wl_surface);
        session.wayland.space.unmap_elem(&window);
        crate::session::trace::x11_event(
            session,
            surface,
            "close-live-removed",
            format_args!("active_before_unmap={close_active}"),
        );
        crate::session::finish_window_unmap(session, preparation);
        if let Some(id) = session.nodes.id_for_surface(&wl_surface) {
            crate::session::forget_destroyed_cluster_member(session, id);
        }
        if let Some(record) = session.nodes.remove_surface(&wl_surface) {
            session.forget_trail_node(record.id);
            session.render.overlay_previews.remove(record.id);
        }
    } else {
        session.wayland.space.unmap_elem(&window);
    }
    true
}

impl<D: SessionDriver> XwmHandler for Session<D> {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwayland
            .xwm
            .as_mut()
            .expect("XWM event delivered without an active XWM")
    }

    fn new_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let generation = self.xwayland.managed_states.register(window.window_id());
        crate::session::trace::x11_event(
            self,
            &window,
            "new-window",
            format_args!("generation={generation} geometry={:?}", window.geometry()),
        );
        eventline::debug!(
            "xwayland: registered managed window xid={} generation={generation}",
            window.window_id()
        );
    }

    fn new_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let xid = window.window_id();
        cancel_pending_override_redirect(self, xid);
        self.xwayland.known_override_redirects.remove(&xid);
        self.xwayland.override_redirect_placements.remove(&xid);
    }

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        crate::session::trace::x11_event(
            self,
            &surface,
            "map-request",
            format_args!(
                "geometry={:?} fullscreen={} maximized={} mapped={}",
                surface.geometry(),
                surface.is_fullscreen(),
                surface.is_maximized(),
                surface.is_mapped(),
            ),
        );
        if window_for_surface(&self.wayland, &self.nodes, &surface).is_some()
            || self
                .xwayland
                .pending_windows
                .contains_key(&surface.window_id())
        {
            eventline::debug!(
                "xwayland: ignored duplicate map request xid={}",
                surface.window_id()
            );
            // The window stays managed, so `WM_STATE` must read Normal even if
            // an earlier withdrawal cleared it. Re-asserting is idempotent and
            // costs one property write on a path that is already rare.
            if self.xwayland.managed_states.state(surface.window_id()) != WindowState::Normal {
                self.xwayland
                    .transition_surface(&surface, WindowState::Normal);
            }
            return;
        }
        let initial_size = surface.geometry().size;
        let initial_iconic = matches!(
            surface.hints().and_then(|hints| hints.initial_state),
            Some(WmHintsState::Iconic)
        );
        if let Err(err) = surface.set_mapped(true) {
            eventline::warn!("xwayland: failed to map window: {err}");
            return;
        }
        self.xwayland
            .transition_surface(&surface, WindowState::Normal);
        sync_allowed_actions(self.xwayland.control.as_ref(), &surface);
        sync_urgency(&surface);
        if initial_iconic {
            self.xwayland
                .managed_states
                .transition(surface.window_id(), WindowState::Iconic);
        }
        let window = Window::new_x11_window(surface.clone());
        self.xwayland.pending_windows.insert(
            surface.window_id(),
            PendingWindow {
                surface: surface.clone(),
                window,
                initial_size,
            },
        );
        eventline::debug!(
            "xwayland: map requested xid={} fullscreen={} maximized={} initial_iconic={} initial={}x{}",
            surface.window_id(),
            surface.is_fullscreen(),
            surface.is_maximized(),
            initial_iconic,
            initial_size.w,
            initial_size.h
        );
        consider_map(self, surface.window_id(), None);
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let xid = surface.window_id();
        if window_for_surface(&self.wayland, &self.nodes, &surface).is_some()
            || self.xwayland.pending_override_redirects.contains_key(&xid)
        {
            eventline::debug!("xwayland: ignored duplicate override-redirect map xid={xid}");
            return;
        }
        let first_map = self.xwayland.known_override_redirects.insert(xid);
        let configured = self.xwayland.override_redirect_placements.remove(&xid);
        match override_redirect_map_admission(first_map, configured.is_some()) {
            OverrideRedirectMapAdmission::ImmediateFresh => {
                map_override_redirect(self, surface.clone(), surface.geometry(), None, false);
            }
            OverrideRedirectMapAdmission::ImmediateConfigured => {
                let geometry = configured
                    .expect("configured admission requires placement")
                    .geometry;
                map_override_redirect(self, surface, geometry, None, false);
            }
            OverrideRedirectMapAdmission::AwaitConfigure => {
                let geometry = surface.geometry();
                defer_override_redirect_remap(self, surface, geometry);
            }
        }
    }

    fn unmapped_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        crate::session::trace::x11_event(
            self,
            &surface,
            "unmapped",
            format_args!("geometry={:?}", surface.geometry()),
        );
        cancel_pending_override_redirect(self, surface.window_id());
        self.xwayland
            .override_redirect_placements
            .remove(&surface.window_id());
        let managed = forget_window(self, &surface);
        if !surface.is_override_redirect()
            && let Err(err) = surface.set_mapped(false)
        {
            eventline::warn!("xwayland: failed to acknowledge unmap: {err}");
        }
        // `set_mapped(false)` uses IconicState internally; delete WM_STATE
        // afterwards because this path is an ICCCM withdrawal, not minimize.
        // Only when this really was a managed window, though: an unmap for a
        // window we were not tracking must not strip `WM_STATE`, which toolkits
        // (Chromium/CEF, so Steam) read to decide the window is managed and
        // viewable at all.
        if managed {
            self.xwayland
                .transition_surface(&surface, WindowState::Withdrawn);
        }
        refresh_override_redirect_owners(self);
        crate::session::sync_keyboard_focus(self, SERIAL_COUNTER.next_serial());
        crate::session::pointer::refresh_desktop_client_focus(
            self,
            self.start_time.elapsed().as_millis() as u32,
        );
        self.request_redraw();
    }

    fn destroyed_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        crate::session::trace::x11_event(
            self,
            &surface,
            "destroyed",
            format_args!("geometry={:?}", surface.geometry()),
        );
        cancel_pending_override_redirect(self, surface.window_id());
        self.xwayland
            .known_override_redirects
            .remove(&surface.window_id());
        self.xwayland
            .override_redirect_placements
            .remove(&surface.window_id());
        forget_window(self, &surface);
        self.xwayland.managed_states.destroy(surface.window_id());
        crate::session::trace::forget_x11(self, &surface);
        refresh_override_redirect_owners(self);
        crate::session::sync_keyboard_focus(self, SERIAL_COUNTER.next_serial());
        crate::session::pointer::refresh_desktop_client_focus(
            self,
            self.start_time.elapsed().as_millis() as u32,
        );
        self.request_redraw();
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // ICCCM override-redirect clients configure themselves directly.
        // Smithay reports their resulting ConfigureNotify separately.
        if surface.is_override_redirect() {
            return;
        }
        // Halley's field owns normal-window placement and stacking. Transient
        // coordinates are honored by `requested_geometry`; denied coordinates
        // and reorder requests still receive Smithay's synthetic
        // ConfigureNotify for the final compositor-owned geometry.
        let geometry = super::configure::requested_geometry(&surface, x, y, width, height);
        crate::session::trace::x11_event(
            self,
            &surface,
            "configure-request",
            format_args!(
                "requested={{x:{x:?},y:{y:?},width:{width:?},height:{height:?}}} resolved={geometry:?} current={:?}",
                surface.geometry(),
            ),
        );
        if surface.is_transient_for().is_some()
            && (x.is_some() || y.is_some())
            && let Some(window) = window_for_surface(&self.wayland, &self.nodes, &surface)
            && let Some(location) = super::presentation::source_element_location_from_root_screen(
                self,
                &window,
                geometry.loc,
                crate::frame_clock::monotonic_now(),
            )
        {
            self.wayland.space.relocate_element(&window, location);
        }
        if let Err(err) = surface.configure(geometry) {
            eventline::warn!("xwayland: configure request failed: {err}");
        }
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        geometry: Rectangle<i32, Logical>,
        above: Option<u32>,
    ) {
        crate::session::trace::x11_sampled_event(
            self,
            &surface,
            "configure-notify",
            format_args!("geometry={geometry:?} above={above:?}"),
        );
        if self
            .xwayland
            .pending_windows
            .contains_key(&surface.window_id())
        {
            consider_map(self, surface.window_id(), surface.wl_surface().as_ref());
            return;
        }
        if surface.is_override_redirect() {
            let xid = surface.window_id();
            if cancel_pending_override_redirect(self, xid).is_some() {
                map_override_redirect(self, surface, geometry, above, true);
                return;
            }
            let Some(window) = window_for_surface(&self.wayland, &self.nodes, &surface) else {
                self.xwayland
                    .override_redirect_placements
                    .insert(xid, OverrideRedirectPlacement { geometry });
                return;
            };
            let previous_output = crate::wayland::window_output_name(&window);
            let resolution = override_redirect_resolution(self, &surface, geometry);
            apply_override_redirect_resolution(&window, &resolution);
            let source_location =
                override_redirect_source_location(self, &window, &resolution, geometry.loc);
            let current_output = crate::wayland::window_output_name(&window);
            if previous_output != current_output {
                eventline::debug!(
                    "xwayland: override-redirect xid={} moved output {:?} -> {:?}",
                    surface.window_id(),
                    previous_output,
                    current_output
                );
            }
            self.wayland
                .space
                .relocate_element(&window, source_location);
            let action = restack_override_redirect(self, &window, above);
            let output = override_redirect_output_name(&window);
            describe_override_redirect_configure(
                &surface,
                geometry,
                output.as_deref(),
                &resolution,
                above,
                action,
            );
            crate::session::pointer::refresh_desktop_client_focus(
                self,
                self.start_time.elapsed().as_millis() as u32,
            );
            self.request_redraw();
            return;
        }
        let Some(window) = window_for_surface(&self.wayland, &self.nodes, &surface) else {
            return;
        };
        let now = crate::frame_clock::monotonic_now();
        match self
            .fullscreen
            .settle_external_configure(&mut self.wayland, &window, geometry, now)
        {
            ExternalConfigureResult::NotPending => {
                let opening = window.wl_surface().is_some_and(|wl_surface| {
                    self.window_animations
                        .is_animating(wl_surface.as_ref(), now)
                });
                let client_geometry_guarded = self
                    .xwayland
                    .client_geometry_guarded(surface.window_id(), now);
                let grabbed = window.wl_surface().is_some_and(|wl_surface| {
                    crate::input::grab::belongs_to_surface(
                        &self.interactions.grab,
                        wl_surface.as_ref(),
                    )
                });
                let fullscreen = window.wl_surface().is_some_and(|wl_surface| {
                    self.fullscreen
                        .is_fullscreen_or_pending(wl_surface.as_ref())
                });
                let placement = if opening_placement_is_authoritative(
                    opening,
                    client_geometry_guarded,
                    grabbed,
                    fullscreen,
                ) {
                    self.xwayland
                        .opening_placements
                        .get(&surface.window_id())
                        .copied()
                } else {
                    self.xwayland
                        .opening_placements
                        .remove(&surface.window_id());
                    None
                };
                if let Some(placement) = placement {
                    let centered = placement.centered(geometry.size);
                    self.wayland.space.relocate_element(&window, centered.loc);
                }
                // Managed X geometry is in root-desktop coordinates; `Space`
                // is in Halley's Field coordinates.  The request path applies
                // accepted source placement explicitly, so ConfigureNotify
                // must never feed the reported X root position back into it.
                self.fullscreen
                    .update_external_windowed_placement(&self.wayland, &window);
                remember_normal_size(self, &surface, geometry.size);
            }
            ExternalConfigureResult::Waiting => {}
            ExternalConfigureResult::Settled {
                fullscreen,
                animated,
            } => {
                eventline::debug!(
                    "xwayland: fullscreen configure settled xid={} fullscreen={fullscreen} animated={animated}",
                    surface.window_id()
                );
                if !fullscreen {
                    remember_normal_size(self, &surface, geometry.size);
                }
                if animated {
                    // The Wayland buffer may have committed before X11's
                    // matching ConfigureNotify. In that ordering the
                    // configure is the second half of the transaction, so it
                    // must authorize and freeze the endpoint here rather than
                    // relying on render-time sampling of an unaccepted buffer.
                    let textures = &mut self.render.fullscreen_textures;
                    let capture = self.driver.with_renderer(|renderer| {
                        textures.capture_target(
                            renderer,
                            &window,
                            crate::render::fullscreen_texture::TextureTransitionOwner::Fullscreen,
                        )
                    });
                    if let Err(err) = capture {
                        eventline::warn!(
                            "window transition: failed to freeze X11 configure target texture: {err}"
                        );
                    }
                } else {
                    remove_fullscreen_snapshot(self, &window);
                }
            }
        }
        crate::session::reconcile_pointer_constraints(self);
        crate::session::pointer::refresh_desktop_client_focus(
            self,
            self.start_time.elapsed().as_millis() as u32,
        );
        self.request_redraw();
    }

    fn property_notify(&mut self, _xwm: XwmId, surface: X11Surface, property: WmWindowProperty) {
        if matches!(
            property,
            WmWindowProperty::Hints
                | WmWindowProperty::TransientFor
                | WmWindowProperty::Class
                | WmWindowProperty::Pid
        ) {
            refresh_override_redirect_owners(self);
        }
        if property == WmWindowProperty::Hints {
            sync_urgency(&surface);
        }
        if matches!(
            property,
            WmWindowProperty::Class
                | WmWindowProperty::Title
                | WmWindowProperty::MotifHints
                | WmWindowProperty::NormalHints
        ) && let Some(window) = window_for_surface(&self.wayland, &self.nodes, &surface)
        {
            if matches!(property, WmWindowProperty::Class | WmWindowProperty::Title) {
                self.window_rules.track_window(&window);
            }
            if property == WmWindowProperty::MotifHints {
                // Toggling client-side decorations changes the top inset.
                self.xwayland.sync_frame_extents(
                    &window,
                    &self.settings.decorations,
                    &self.settings.font,
                );
            }
            self.request_redraw();
        }
        if property == WmWindowProperty::NormalHints {
            sync_allowed_actions(self.xwayland.control.as_ref(), &surface);
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        eventline::debug!(
            "xwayland: maximize request xid={} client_maximized={} client_fullscreen={} \
             maximize_origin={}",
            surface.window_id(),
            surface.is_maximized(),
            surface.is_fullscreen(),
            maximize_origin_active(&surface),
        );
        let maximized = window_for_surface(&self.wayland, &self.nodes, &surface)
            .and_then(|window| window.wl_surface().map(|surface| surface.into_owned()))
            .is_some_and(|wl_surface| {
                let _ = crate::session::set_surface_field_maximized(self, &wl_surface, true);
                self.maximize.contains(&wl_surface)
            });
        if let Err(err) = surface.set_maximized(maximized) {
            eventline::warn!("xwayland: failed to apply maximized state: {err}");
        }
        self.request_redraw();
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        eventline::debug!(
            "xwayland: unmaximize request xid={} client_maximized={} client_fullscreen={} \
             maximize_origin={}",
            surface.window_id(),
            surface.is_maximized(),
            surface.is_fullscreen(),
            maximize_origin_active(&surface),
        );
        if let Some(wl_surface) = window_for_surface(&self.wayland, &self.nodes, &surface)
            .and_then(|window| window.wl_surface().map(|surface| surface.into_owned()))
        {
            let _ = crate::session::set_surface_field_maximized(self, &wl_surface, false);
        }
        // `set_surface_field_maximized` synchronizes EWMH state as part of the
        // geometry transaction. Avoid echoing the same state write: some X11
        // clients answer it with another normal-geometry configure, which can
        // replace the first frames of the reverse maximize presentation.
        if surface.is_maximized()
            && let Err(err) = surface.set_maximized(false)
        {
            eventline::warn!("xwayland: failed to clear maximized state: {err}");
        }
        self.request_redraw();
    }

    fn minimize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let changed = window_for_surface(&self.wayland, &self.nodes, &surface)
            .and_then(|window| {
                window
                    .wl_surface()
                    .map(|wl_surface| wl_surface.into_owned())
            })
            .and_then(|wl_surface| self.nodes.id_for_surface(&wl_surface))
            .is_some_and(|id| crate::nodes::collapse(self, id, SERIAL_COUNTER.next_serial()));
        if !changed {
            // Keep both EWMH and ICCCM state truthful when a minimize cannot
            // be honored (for example, while fullscreen owns the window).
            // Some clients stop presenting as soon as they send
            // WM_CHANGE_STATE(IconicState) and wait for WM_STATE to confirm or
            // reject the request. Re-publish NormalState even when it was
            // already our tracked state so the client receives that rejection.
            if let Err(err) = surface.set_hidden(false) {
                eventline::warn!("xwayland: failed to reject minimize request: {err}");
            }
            self.xwayland
                .transition_surface(&surface, WindowState::Normal);
        }
        self.request_redraw();
    }

    fn unminimize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(id) = window_for_surface(&self.wayland, &self.nodes, &surface)
            .and_then(|window| {
                window
                    .wl_surface()
                    .map(|wl_surface| wl_surface.into_owned())
            })
            .and_then(|wl_surface| self.nodes.id_for_surface(&wl_surface))
        {
            let _ = crate::nodes::restore(self, id, SERIAL_COUNTER.next_serial());
        }
        self.request_redraw();
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        crate::session::trace::x11_event(
            self,
            &surface,
            "fullscreen-request",
            format_args!("geometry={:?}", surface.geometry()),
        );
        enter_fullscreen(self, &surface, FullscreenRequestOrigin::Client);
        self.request_redraw();
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        crate::session::trace::x11_event(
            self,
            &surface,
            "unfullscreen-request",
            format_args!("geometry={:?}", surface.geometry()),
        );
        leave_fullscreen(self, &surface, FullscreenRequestOrigin::Client);
        self.request_redraw();
    }

    fn demands_attention_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Err(err) = surface.set_demands_attention(true) {
            eventline::warn!(
                "xwayland: failed to set demands-attention xid={}: {err}",
                surface.window_id()
            );
        }
        self.request_redraw();
    }

    fn undemands_attention_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Err(err) = surface.set_demands_attention(false) {
            eventline::warn!(
                "xwayland: failed to clear demands-attention xid={}: {err}",
                surface.window_id()
            );
        }
        self.request_redraw();
    }

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        button: u32,
        resize_edge: ResizeEdge,
    ) {
        let Some(button) = evdev_button(button) else {
            return;
        };
        let Some(window) = window_for_surface(&self.wayland, &self.nodes, &surface) else {
            return;
        };
        crate::session::begin_pointer_resize(self, &window, resize_handle(resize_edge), button);
    }

    fn move_request(&mut self, _xwm: XwmId, surface: X11Surface, button: u32) {
        let Some(button) = evdev_button(button) else {
            return;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let Some(serial) = pointer.with_grab(|serial, _| serial) else {
            return;
        };
        let Some(start_data) = pointer.grab_start_data() else {
            return;
        };
        if start_data.button != button {
            return;
        }
        let Some(surface_root) = surface.wl_surface() else {
            return;
        };
        let Some((focused, _)) = start_data.focus else {
            return;
        };
        if crate::wayland::compositor::root_surface(&focused) != surface_root {
            return;
        }
        let Some(window) = window_for_surface(&self.wayland, &self.nodes, &surface) else {
            return;
        };
        if crate::session::begin_client_pointer_move(self, &window, serial, button) {
            self.request_redraw();
        }
    }

    fn active_window_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        timestamp: u32,
        _currently_active_window: Option<X11Surface>,
    ) {
        if surface.is_override_redirect() {
            eventline::debug!(
                "xwayland: ignored activation request for override-redirect xid={}",
                surface.window_id()
            );
            return;
        }
        let focused_xid = self.wayland.focused_window.as_ref().and_then(|focused| {
            self.wayland.space.elements().find_map(|window| {
                (window.wl_surface().as_deref() == Some(focused))
                    .then(|| window.x11_surface().map(|surface| surface.window_id()))
                    .flatten()
            })
        });
        if !self.xwayland.managed_states.accept_activation(
            timestamp,
            surface.window_id(),
            focused_xid,
        ) {
            if let Err(err) = surface.set_demands_attention(true) {
                eventline::warn!(
                    "xwayland: failed to mark rejected activation xid={}: {err}",
                    surface.window_id()
                );
            }
            eventline::debug!(
                "xwayland: rejected stale activation xid={} timestamp={} active={focused_xid:?}",
                surface.window_id(),
                timestamp
            );
            return;
        }
        if let Some(window) = window_for_surface(&self.wayland, &self.nodes, &surface) {
            if let Err(err) = surface.set_demands_attention(false) {
                eventline::warn!(
                    "xwayland: failed to clear activation attention xid={}: {err}",
                    surface.window_id()
                );
            }
            crate::session::focus_window(self, &window, SERIAL_COUNTER.next_serial());
            self.request_redraw();
        }
    }

    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        crate::xwayland::selection::allow_access(&self.wayland, xwm)
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        crate::xwayland::selection::send(&self.seat, selection, mime_type, fd);
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        crate::xwayland::selection::set(
            &self.wayland.display_handle,
            &self.seat,
            selection,
            mime_types,
        );
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        crate::xwayland::selection::clear(&self.wayland.display_handle, &self.seat, selection);
    }

    fn randr_primary_output_change(&mut self, _xwm: XwmId, _output_name: Option<String>) {}

    fn disconnected(&mut self, _xwm: XwmId) {
        eventline::warn!("xwayland: window manager disconnected");
        self.xwayland.clear();
    }
}
