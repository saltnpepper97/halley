//! External window fullscreen requests and configure transactions.

use super::*;

impl FullscreenManager {
    pub(crate) fn request_external(
        &mut self,
        wayland: &mut WaylandState,
        window: &Window,
        origin: FullscreenOrigin,
    ) -> Option<Rectangle<i32, Logical>> {
        let wl_surface = window.wl_surface().map(|surface| surface.into_owned())?;
        let window = find_window(wayland, &wl_surface).cloned()?;
        let target = crate::wayland::window_output_name(&window)
            .and_then(|name| output_by_name(wayland, &name))
            .or_else(|| crate::wayland::focus::selected_output(wayland).cloned());
        let target = target?;
        let output_geometry = wayland.space.output_geometry(&target)?;
        let target_name = target.name();
        self.windows
            .entry(wl_surface)
            .and_modify(|entry| {
                entry.origin = origin;
                settle_external_fullscreen(entry, &target_name, output_geometry.size);
            })
            .or_insert_with(|| FullscreenWindow {
                desired: true,
                active: true,
                presented: true,
                target_output: target_name,
                restore: Some(WindowedPlacement {
                    location: wayland
                        .space
                        .element_location(&window)
                        .unwrap_or(output_geometry.loc),
                    geometry: wayland
                        .space
                        .element_geometry(&window)
                        .unwrap_or_else(|| window.geometry()),
                    output: crate::wayland::window_output_name(&window),
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
                native: None,
                native_pending: None,
                restore_kind: FullscreenRestoreKind::Windowed,
                preserve_stack: false,
                presentation_paused: false,
                navigation_released: false,
            });
        crate::wayland::set_window_output(&window, &target);
        // X11 fullscreen changes presentation geometry, not stacking. Using
        // `map_element` here would silently move an already-mapped window to
        // the top and cover windows the user had deliberately kept above it.
        wayland.space.relocate_element(&window, output_geometry.loc);
        Some(output_geometry)
    }

    pub fn unrequest_external(
        &mut self,
        wayland: &mut WaylandState,
        window: &Window,
    ) -> Option<Rectangle<i32, Logical>> {
        let wl_surface = window.wl_surface().map(|surface| surface.into_owned())?;
        let restore = self
            .windows
            .remove(&wl_surface)
            .and_then(|entry| entry.restore)?;
        if let Some(output) = restore
            .output
            .as_deref()
            .and_then(|name| output_by_name(wayland, name))
        {
            crate::wayland::set_window_output(window, &output);
        }
        // Leaving X11 fullscreen preserves the same stack slot as entry.
        wayland.space.relocate_element(window, restore.location);
        Some(restore.geometry)
    }

    pub(crate) fn request_external_animated(
        &mut self,
        wayland: &mut WaylandState,
        window: &Window,
        origin: FullscreenOrigin,
        live_buffers: bool,
    ) -> Option<ExternalTransactionRequest> {
        self.request_external_transaction(
            wayland,
            window,
            if live_buffers {
                ExternalPresentationKind::LiveAnimated
            } else {
                ExternalPresentationKind::Animated
            },
            None,
            origin,
        )
    }

    pub(crate) fn request_external_opening(
        &mut self,
        wayland: &mut WaylandState,
        window: &Window,
        restore_geometry: Option<Rectangle<i32, Logical>>,
        origin: FullscreenOrigin,
    ) -> Option<ExternalTransactionRequest> {
        self.request_external_transaction(
            wayland,
            window,
            ExternalPresentationKind::Opening,
            restore_geometry,
            origin,
        )
    }

    pub(super) fn request_external_transaction(
        &mut self,
        wayland: &mut WaylandState,
        window: &Window,
        presentation: ExternalPresentationKind,
        restore_geometry: Option<Rectangle<i32, Logical>>,
        origin: FullscreenOrigin,
    ) -> Option<ExternalTransactionRequest> {
        let wl_surface = window.wl_surface().map(|surface| surface.into_owned())?;
        let window = find_window(wayland, &wl_surface).cloned()?;
        let target = crate::wayland::window_output_name(&window)
            .and_then(|name| output_by_name(wayland, &name))
            .or_else(|| crate::wayland::focus::selected_output(wayland).cloned())?;
        let output_geometry = wayland.space.output_geometry(&target)?;
        let target_name = target.name();
        let current_restore =
            wayland
                .space
                .element_location(&window)
                .map(|location| WindowedPlacement {
                    location,
                    geometry: wayland
                        .space
                        .element_geometry(&window)
                        .unwrap_or_else(|| Rectangle::new(location, window.geometry().size)),
                    output: crate::wayland::window_output_name(&window),
                });
        let restore = prefer_seeded_restore(
            restore_geometry.map(|geometry| WindowedPlacement {
                location: geometry.loc,
                geometry,
                output: crate::wayland::window_output_name(&window),
            }),
            current_restore,
        );

        let entry = self
            .windows
            .entry(wl_surface)
            .or_insert_with(|| FullscreenWindow {
                desired: false,
                active: false,
                presented: false,
                target_output: target_name.clone(),
                restore: restore.clone(),
                presentation_windowed: None,
                presentation_output: None,
                restore_presentation_output: None,
                fullscreen_size: output_geometry.size,
                transition: None,
                pending_motion: (0.0, 0.0),
                external_pending: None,
                snapshot_serials: Vec::new(),
                origin,
                native: None,
                native_pending: None,
                restore_kind: FullscreenRestoreKind::Windowed,
                preserve_stack: false,
                presentation_paused: false,
                navigation_released: false,
            });
        entry.origin = origin;
        entry.target_output = target_name;
        entry.fullscreen_size = output_geometry.size;
        if entry.restore.is_none() {
            entry.restore = restore;
        }
        crate::wayland::set_window_output(&window, &target);
        Some(begin_external_transaction(
            entry,
            true,
            output_geometry,
            presentation,
            crate::frame_clock::monotonic_now(),
        ))
    }

    pub(crate) fn unrequest_external_animated(
        &mut self,
        window: &Window,
        live_buffers: bool,
    ) -> Option<ExternalTransactionRequest> {
        self.unrequest_external_transaction(
            window,
            if live_buffers {
                ExternalPresentationKind::LiveAnimated
            } else {
                ExternalPresentationKind::Animated
            },
        )
    }

    pub(crate) fn unrequest_external_opening(
        &mut self,
        window: &Window,
    ) -> Option<ExternalTransactionRequest> {
        self.unrequest_external_transaction(window, ExternalPresentationKind::Opening)
    }

    pub(super) fn unrequest_external_transaction(
        &mut self,
        window: &Window,
        presentation: ExternalPresentationKind,
    ) -> Option<ExternalTransactionRequest> {
        let wl_surface = window.wl_surface().map(|surface| surface.into_owned())?;
        let entry = self.windows.get_mut(&wl_surface)?;
        begin_external_restore(entry, presentation, crate::frame_clock::monotonic_now())
    }

    pub(crate) fn settle_external_configure(
        &mut self,
        wayland: &mut WaylandState,
        window: &Window,
        observed: Rectangle<i32, Logical>,
        now: Duration,
    ) -> ExternalConfigureResult {
        let Some(wl_surface) = window.wl_surface().map(|surface| surface.into_owned()) else {
            return ExternalConfigureResult::NotPending;
        };
        let Some(entry) = self.windows.get_mut(&wl_surface) else {
            return ExternalConfigureResult::NotPending;
        };
        let result = acknowledge_external_geometry(entry, observed, &self.animations, now);
        let ExternalConfigureResult::Settled {
            fullscreen,
            animated,
        } = result
        else {
            return result;
        };
        if !relocate_external_window(wayland, window, entry) {
            entry.transition = None;
            entry.presented = fullscreen;
            return ExternalConfigureResult::Settled {
                fullscreen,
                animated: false,
            };
        }
        ExternalConfigureResult::Settled {
            fullscreen,
            animated,
        }
    }

    /// Completes an animated X11 resize only after the matching Wayland buffer
    /// commit. X ConfigureNotify reports geometry before applications such as
    /// Qt have necessarily repainted their Xwayland surface; Niri's xdg resize
    /// transaction naturally waits for this client commit boundary.
    pub(crate) fn settle_external_surface_commit(
        &mut self,
        wayland: &mut WaylandState,
        surface: &WlSurface,
        surface_size: Option<Size<i32, Logical>>,
        target_repaint_ready: bool,
        now: Duration,
    ) -> bool {
        let Some(window) = find_window(wayland, surface).cloned() else {
            return false;
        };
        let Some(entry) = self.windows.get_mut(surface) else {
            return false;
        };
        if !target_repaint_ready {
            return false;
        }
        let ExternalConfigureResult::Settled {
            fullscreen,
            animated: _,
        } = acknowledge_external_surface(entry, surface_size, &self.animations, now)
        else {
            return false;
        };
        if !relocate_external_window(wayland, &window, entry) {
            entry.transition = None;
            entry.presented = fullscreen;
        }
        true
    }

    /// Returns whether a pending presentation was completed, even if its
    /// output disappeared before the window could be relocated. The caller
    /// must retire the completed presentation's snapshots in either case.
    pub(crate) fn finish_external_presentation(
        &mut self,
        wayland: &mut WaylandState,
        window: &Window,
    ) -> bool {
        let Some(wl_surface) = window.wl_surface().map(|surface| surface.into_owned()) else {
            return false;
        };
        let Some(entry) = self.windows.get_mut(&wl_surface) else {
            return false;
        };
        if entry.external_pending.is_none() && entry.transition.is_none() {
            return false;
        }
        finish_external_transition(entry);
        let _ = relocate_external_window(wayland, window, entry);
        true
    }

    pub(crate) fn external_desired_matches(&self, window: &Window, fullscreen: bool) -> bool {
        window
            .wl_surface()
            .is_some_and(|surface| desired_matches(self.windows.get(surface.as_ref()), fullscreen))
    }

    /// A game can send its windowed size before X11 reports the restore
    /// configure. Accept that size as the exit endpoint, keeping one transaction.
    pub(crate) fn negotiate_external_windowed_geometry(
        &mut self,
        window: &Window,
        geometry: Rectangle<i32, Logical>,
    ) {
        let Some(surface) = window.wl_surface() else {
            return;
        };
        let Some(entry) = self.windows.get_mut(surface.as_ref()) else {
            return;
        };
        if entry.desired {
            return;
        }
        let Some(pending) = entry.external_pending.as_mut() else {
            return;
        };
        if pending.presentation != ExternalPresentationKind::LiveAnimated {
            return;
        }
        if pending.geometry == geometry {
            return;
        }
        pending.geometry = geometry;
        pending.geometry_acknowledged = false;
        if let Some(restore) = entry.restore.as_mut() {
            restore.location += geometry.loc - restore.geometry.loc;
            restore.geometry = geometry;
        }
        entry.presentation_windowed = Some(geometry);
        // The retained projection describes the old windowed size. Project
        // the negotiated endpoint through the live camera during this exit.
        entry.presentation_output = None;
        entry.restore_presentation_output = None;
    }

    /// X11 ConfigureRequest must not replace an in-flight presentation target
    /// or resize a settled fullscreen client behind the presentation manager.
    pub(crate) fn external_configure_geometry(
        &self,
        wayland: &WaylandState,
        window: &Window,
    ) -> Option<Rectangle<i32, Logical>> {
        let surface = window.wl_surface()?;
        let entry = self.windows.get(surface.as_ref())?;
        if let Some(pending) = entry.external_pending {
            return Some(pending.geometry);
        }
        entry
            .desired
            .then(|| self.external_target_geometry(wayland, window, true))
            .flatten()
    }

    /// The geometry a new X11 fullscreen request will configure. A state-only
    /// change with a matching live buffer does not need a resize snapshot.
    pub(crate) fn external_target_geometry(
        &self,
        wayland: &WaylandState,
        window: &Window,
        fullscreen: bool,
    ) -> Option<Rectangle<i32, Logical>> {
        if fullscreen {
            let output = crate::wayland::window_output_name(window)
                .and_then(|name| output_by_name(wayland, &name))
                .or_else(|| crate::wayland::focus::selected_output(wayland).cloned())?;
            wayland.space.output_geometry(&output)
        } else {
            let surface = window.wl_surface()?;
            self.windows
                .get(surface.as_ref())?
                .restore
                .as_ref()
                .map(|restore| restore.geometry)
        }
    }

    pub(crate) fn update_external_windowed_placement(
        &mut self,
        wayland: &WaylandState,
        window: &Window,
    ) {
        let Some(surface) = window.wl_surface() else {
            return;
        };
        let Some(entry) = self.windows.get_mut(surface.as_ref()) else {
            return;
        };
        if !can_update_external_restore(entry) {
            return;
        }
        let (Some(location), Some(geometry)) = (
            wayland.space.element_location(window),
            wayland.space.element_geometry(window),
        ) else {
            return;
        };
        entry.restore = Some(WindowedPlacement {
            location,
            geometry,
            output: crate::wayland::window_output_name(window),
        });
    }
}
