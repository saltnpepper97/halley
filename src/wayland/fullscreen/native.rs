//! Native Wayland fullscreen requests and ownership changes.

use super::*;

impl FullscreenManager {
    pub fn request(
        &mut self,
        wayland: &mut WaylandState,
        toplevel: &ToplevelSurface,
        requested: Option<WlOutput>,
    ) {
        self.request_with_origin(
            wayland,
            toplevel,
            requested,
            FullscreenOrigin::Client,
            false,
        );
    }

    pub(crate) fn request_compositor(
        &mut self,
        wayland: &mut WaylandState,
        toplevel: &ToplevelSurface,
        retain_maximized: bool,
    ) {
        self.request_with_origin(
            wayland,
            toplevel,
            None,
            FullscreenOrigin::Compositor,
            retain_maximized,
        );
        if let Some(entry) = self.windows.get_mut(toplevel.wl_surface()) {
            // Mod+F already captured the outgoing texture at input time. Do
            // not overwrite it in the pre-commit hook after the new buffer
            // has been imported but before cached geometry is synchronized.
            entry.snapshot_serials.clear();
        }
    }

    pub(super) fn request_with_origin(
        &mut self,
        wayland: &mut WaylandState,
        toplevel: &ToplevelSurface,
        requested: Option<WlOutput>,
        origin: FullscreenOrigin,
        retain_maximized: bool,
    ) {
        let window = find_window(wayland, toplevel.wl_surface()).cloned();
        // A client can enter its own fullscreen mode while the compositor is
        // already presenting the same toplevel through Mod+F. Keep the
        // compositor's output target in that case; client protocol ownership
        // is tracked independently below.
        let retained_compositor_target = self
            .windows
            .get(toplevel.wl_surface())
            .filter(|entry| {
                origin == FullscreenOrigin::Client
                    && entry
                        .native
                        .is_some_and(|native| native.compositor_requested)
            })
            .map(|entry| entry.target_output.clone());
        let requested = requested.filter(|resource| {
            Output::from_resource(resource)
                .is_some_and(|output| wayland.space.outputs().any(|known| known == &output))
        });
        let requested_output = requested.as_ref().and_then(Output::from_resource);
        let target = retained_compositor_target
            .as_deref()
            .and_then(|name| output_by_name(wayland, name))
            .or(requested_output)
            .or_else(|| {
                window
                    .as_ref()
                    .and_then(crate::wayland::window_output_name)
                    .and_then(|name| output_by_name(wayland, &name))
            })
            .or_else(|| crate::wayland::focus::selected_output(wayland).cloned());

        let Some(target) = target else {
            send_required_configure(toplevel);
            return;
        };
        let Some(output_geometry) = wayland.space.output_geometry(&target) else {
            send_required_configure(toplevel);
            return;
        };

        let entry = self
            .windows
            .entry(toplevel.wl_surface().clone())
            .or_insert_with(|| FullscreenWindow {
                desired: false,
                active: false,
                presented: false,
                target_output: target.name(),
                restore: window.as_ref().and_then(|window| {
                    Some(WindowedPlacement {
                        location: wayland.space.element_location(window)?,
                        geometry: wayland.space.element_geometry(window)?,
                        output: crate::wayland::window_output_name(window),
                    })
                }),
                presentation_windowed: None,
                presentation_output: None,
                restore_presentation_output: None,
                fullscreen_size: output_geometry.size,
                transition: None,
                pending_motion: (0.0, 0.0),
                external_pending: None,
                snapshot_serials: Vec::new(),
                origin,
                native: Some(NativeFullscreenState::default()),
                native_pending: None,
                restore_kind: FullscreenRestoreKind::Windowed,
                preserve_stack: false,
                presentation_paused: false,
                navigation_released: false,
            });
        let now = crate::frame_clock::monotonic_now();
        let visual_before = entry.desired;
        let motion_before = visual_motion_state(entry, now);
        request_native_owner(entry, origin);
        let transition_requested = visual_before != entry.desired;
        if transition_requested {
            entry.pending_motion = motion_before;
            entry.transition = None;
            entry.native_pending = None;
        }
        entry.target_output = target.name();
        if retain_maximized {
            entry.restore_kind = FullscreenRestoreKind::WindowedFromMaximize;
        }
        // The destination is the size we are about to configure, decided once
        // here, exactly like field maximize decides its target rect at toggle
        // time. `handle_commit` only re-reads the client's committed size once
        // the transition has settled, to letterbox a client that stays smaller.
        entry.fullscreen_size = output_geometry.size;
        let protocol_origin = native_protocol_origin(entry);
        let protocol_desired = entry.native.is_none_or(|native| native.protocol_desired);
        let keep_maximized_protocol =
            retain_maximized && origin == FullscreenOrigin::Compositor && !protocol_desired;

        toplevel.with_pending_state(|state| {
            apply_protocol_presentation_state_for_request(
                state,
                protocol_origin,
                protocol_desired,
                keep_maximized_protocol,
            );
            state.size = Some(output_geometry.size);
            state.bounds = Some(output_geometry.size);
            state.fullscreen_output = (protocol_origin != FullscreenOrigin::Maximize
                && protocol_desired)
                .then_some(requested)
                .flatten();
        });
        let configure_serial = send_required_configure(toplevel);
        if transition_requested || entry.native_pending.is_some() {
            entry.native_pending = configure_serial.map(|serial| NativePending {
                serial,
                size_retry_at: None,
            });
        }
        if let Some(serial) = configure_serial
            && animations_enabled(&self.animations)
            && transition_requested
        {
            entry.snapshot_serials.push(serial);
        }
    }

    /// Applies an xdg_toplevel `unset_fullscreen` request without allowing it
    /// to release a presentation which was explicitly selected by Mod+F.
    pub fn unrequest_client(&mut self, wayland: &WaylandState, toplevel: &ToplevelSurface) {
        if !self.windows.contains_key(toplevel.wl_surface()) {
            // Toolkits may unset fullscreen before a title-bar move even on
            // an ordinary window. Acknowledge without replacing its current
            // or pending resize target with an unconstrained (0, 0) size.
            send_required_configure(toplevel);
            return;
        }
        let nested = self
            .windows
            .get_mut(toplevel.wl_surface())
            .and_then(|entry| {
                entry.native?.compositor_requested.then(|| {
                    release_native_owner(entry, FullscreenOrigin::Client);
                    (
                        entry.target_output.clone(),
                        entry.fullscreen_size,
                        native_protocol_origin(entry),
                        entry
                            .native
                            .expect("native owner checked above")
                            .protocol_desired,
                    )
                })
            });
        if let Some((target_output, fullscreen_size, origin, protocol_desired)) = nested {
            let bounds = output_by_name(wayland, &target_output)
                .and_then(|output| wayland.space.output_geometry(&output))
                .map_or(fullscreen_size, |geometry| geometry.size);
            toplevel.with_pending_state(|state| {
                apply_protocol_presentation_state(state, origin, protocol_desired);
                // Mod+F still owns the visual output presentation. Keep the
                // configured size pinned while releasing the nested client
                // owner.
                state.size = Some(bounds);
                state.bounds = Some(bounds);
                state.fullscreen_output = None;
            });
            send_required_configure(toplevel);
            self.discard_snapshot_requests(toplevel.wl_surface());
            return;
        }
        self.unrequest(wayland, toplevel);
        self.discard_snapshot_requests(toplevel.wl_surface());
    }

    /// Releases only Mod+F ownership. A client which is independently
    /// fullscreen remains fullscreen and becomes the presentation owner.
    pub(crate) fn unrequest_compositor(
        &mut self,
        wayland: &WaylandState,
        toplevel: &ToplevelSurface,
    ) {
        let retained_client = self
            .windows
            .get_mut(toplevel.wl_surface())
            .and_then(|entry| {
                let native = entry.native?;
                (native.compositor_requested && native.client_requested).then(|| {
                    release_native_owner(entry, FullscreenOrigin::Compositor);
                    (
                        entry.target_output.clone(),
                        entry.fullscreen_size,
                        native_protocol_origin(entry),
                    )
                })
            });
        if let Some((target_output, fullscreen_size, origin)) = retained_client {
            let bounds = output_by_name(wayland, &target_output)
                .and_then(|output| wayland.space.output_geometry(&output))
                .map_or(fullscreen_size, |geometry| geometry.size);
            toplevel.with_pending_state(|state| {
                apply_protocol_presentation_state(state, origin, true);
                state.size = Some(bounds);
                state.bounds = Some(bounds);
            });
            send_required_configure(toplevel);
            self.discard_snapshot_requests(toplevel.wl_surface());
            return;
        }
        self.unrequest(wayland, toplevel);
        self.discard_snapshot_requests(toplevel.wl_surface());
    }

    pub fn unrequest(&mut self, wayland: &WaylandState, toplevel: &ToplevelSurface) {
        let (restore_size, transition_requested, origin) = self
            .windows
            .get_mut(toplevel.wl_surface())
            .map(|entry| {
                let now = crate::frame_clock::monotonic_now();
                let visual_before = entry.desired;
                let motion_before = visual_motion_state(entry, now);
                release_all_native_owners(entry);
                let transition_requested = visual_before != entry.desired;
                if transition_requested {
                    entry.pending_motion = motion_before;
                    entry.transition = None;
                    entry.native_pending = None;
                }
                if let Some(window) = find_window(wayland, toplevel.wl_surface()) {
                    entry.fullscreen_size = window.geometry().size;
                }
                select_restore_presentation_endpoint(entry);
                (
                    entry.restore.as_ref().map(|restore| restore.geometry.size),
                    transition_requested,
                    entry.origin,
                )
            })
            .unwrap_or((None, false, FullscreenOrigin::Client));
        let bounds = self
            .windows
            .get(toplevel.wl_surface())
            .and_then(|entry| output_by_name(wayland, &entry.target_output))
            .and_then(|output| wayland.space.output_geometry(&output))
            .map(|geometry| geometry.size);

        toplevel.with_pending_state(|state| {
            apply_protocol_presentation_state(state, origin, false);
            state.size = restore_size;
            state.bounds = bounds;
            state.fullscreen_output = None;
            crate::wayland::decoration::apply_tiled_hint(state);
        });
        if let Some(serial) = send_required_configure(toplevel)
            && transition_requested
            && let Some(entry) = self.windows.get_mut(toplevel.wl_surface())
        {
            entry.native_pending = Some(NativePending {
                serial,
                size_retry_at: None,
            });
            if animations_enabled(&self.animations) {
                entry.snapshot_serials.push(serial);
            }
        }
    }

    /// Drops fullscreen state without arming the exit transition.
    ///
    /// Used only when another presentation immediately takes ownership of the
    /// same window - today the field-maximize handoff. The caller's animation
    /// covers the whole travel from the on-screen fullscreen rect, so a second
    /// shrink transition here would fight it (fullscreen wins in
    /// `window_visual_state`, so the maximize grow would only become visible
    /// once the shrink retired, mid-flight). Sending no configure and pushing
    /// no snapshot serial leaves the single configure and the captured
    /// crossfade texture to the caller.
    pub(crate) fn retire_for_handoff(&mut self, toplevel: &ToplevelSurface) {
        let Some(entry) = self.windows.remove(toplevel.wl_surface()) else {
            return;
        };
        toplevel.with_pending_state(|state| {
            apply_protocol_presentation_state(state, entry.origin, false);
            state.fullscreen_output = None;
            crate::wayland::decoration::apply_tiled_hint(state);
        });
    }
}
