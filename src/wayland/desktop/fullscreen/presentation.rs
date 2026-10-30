//! Fullscreen presentation queries and saved restore placement.

use super::*;

impl FullscreenManager {
    pub fn presentation(
        &self,
        surface: &WlSurface,
        output: &Output,
        now: Duration,
    ) -> Option<FullscreenPresentation> {
        let entry = self.windows.get(surface)?;
        if entry.target_output != output.name() {
            return None;
        }
        fullscreen_presentation(entry, now)
    }

    /// Returns the monitor-wide camera track for the fullscreen transaction.
    ///
    /// The endpoint is the original window center at native output zoom, as
    /// in old Halley. Unlike the surface presentation, this deliberately
    /// remains present at progress zero while a request/exit transaction owns
    /// the output, allowing input mutation to stay locked until cleanup.
    pub fn camera_frame(
        &self,
        output: &Output,
        output_geometry: Rectangle<i32, Logical>,
        now: Duration,
    ) -> Option<FullscreenCameraFrame> {
        self.camera_frame_matching(output, output_geometry, now, |_| true)
    }

    pub fn camera_frame_matching(
        &self,
        output: &Output,
        output_geometry: Rectangle<i32, Logical>,
        now: Duration,
        mut matches: impl FnMut(&WlSurface) -> bool,
    ) -> Option<FullscreenCameraFrame> {
        let (_, entry) = self.windows.iter().find(|(surface, entry)| {
            matches(surface)
                && entry_owns_output_presentation(entry)
                && entry.target_output == output.name()
                && (entry.active
                    || entry.desired
                    || entry.presented
                    || entry.transition.is_some()
                    || entry.external_pending.is_some())
        })?;
        let progress = visual_motion_state(entry, now).0.clamp(0.0, 1.0) as f32;
        let center = entry
            .restore
            .as_ref()
            .map(|restore| {
                let geometry = restore.geometry;
                Point::<f32, Physical>::from((
                    geometry.loc.x as f32 + geometry.size.w as f32 / 2.0
                        - output_geometry.loc.x as f32,
                    geometry.loc.y as f32 + geometry.size.h as f32 / 2.0
                        - output_geometry.loc.y as f32,
                ))
            })
            .unwrap_or_else(|| {
                Point::from((
                    output_geometry.size.w as f32 / 2.0,
                    output_geometry.size.h as f32 / 2.0,
                ))
            });
        Some(FullscreenCameraFrame {
            center,
            progress,
            desired: entry.desired,
            transition_active: entry.transition.is_some(),
        })
    }

    pub fn covers_top(&self, focused: Option<&WlSurface>, output: &Output, now: Duration) -> bool {
        self.covers_top_matching(focused, output, now, |_| true)
    }

    pub fn covers_top_matching(
        &self,
        _focused: Option<&WlSurface>,
        output: &Output,
        now: Duration,
        mut matches: impl FnMut(&WlSurface) -> bool,
    ) -> bool {
        self.windows.iter().any(|(surface, entry)| {
            matches(surface)
                && entry_owns_output_presentation(entry)
                && entry_covers_top(entry, &output.name(), now)
        })
    }

    pub fn covers_any_top(
        &self,
        wayland: &WaylandState,
        focused: Option<&WlSurface>,
        now: Duration,
    ) -> bool {
        wayland
            .space
            .outputs()
            .any(|output| self.covers_top(focused, output, now))
    }

    pub fn is_animating_on_output(&self, output: &Output, now: Duration) -> bool {
        self.is_animating_on_output_matching(output, now, |_| true)
    }

    pub fn is_animating_on_output_matching(
        &self,
        output: &Output,
        now: Duration,
        mut matches: impl FnMut(&WlSurface) -> bool,
    ) -> bool {
        self.windows.iter().any(|(surface, entry)| {
            matches(surface)
                && entry.target_output == output.name()
                && (entry
                    .native_pending
                    .is_some_and(|pending| pending.size_retry_at.is_some())
                    || entry
                        .transition
                        .is_some_and(|transition| !transition.is_finished_at(now)))
        })
    }

    /// Whether a real fullscreen request owns, or is in the process of
    /// taking/releasing, this output. Unlike `covers_top`, this includes the
    /// configure and transition handoffs so input-side desktop maintenance
    /// cannot leak into a fullscreen path between frames.
    pub(crate) fn has_fullscreen_activity_on_output_matching(
        &self,
        output: &Output,
        mut matches: impl FnMut(&WlSurface) -> bool,
    ) -> bool {
        self.windows.iter().any(|(surface, entry)| {
            matches(surface)
                && entry.origin != FullscreenOrigin::Maximize
                && entry_occupies_output(entry, &output.name())
        })
    }

    /// Whether immersive fullscreen is presenting on this output right now.
    ///
    /// Includes enter/exit transitions so overlay chrome such as Bearings
    /// disappears with the client. Parked (Alt-Tabbed) fullscreen and
    /// field-maximize are excluded so the desktop overlay can return.
    pub(crate) fn presents_immersive_on_output_matching(
        &self,
        output: &Output,
        mut matches: impl FnMut(&WlSurface) -> bool,
    ) -> bool {
        self.windows.iter().any(|(surface, entry)| {
            matches(surface) && entry_presents_immersive(entry, &output.name())
        })
    }

    pub fn has_stable_fullscreen_on_output(&self, output: &Output, now: Duration) -> bool {
        self.stable_fullscreen_surface_on_output(output, now)
            .is_some()
    }

    pub fn stable_fullscreen_surface_on_output(
        &self,
        output: &Output,
        now: Duration,
    ) -> Option<&WlSurface> {
        self.stable_fullscreen_surface_on_output_matching(output, now, |_| true)
    }

    pub fn stable_fullscreen_surface_on_output_matching(
        &self,
        output: &Output,
        now: Duration,
        mut matches: impl FnMut(&WlSurface) -> bool,
    ) -> Option<&WlSurface> {
        self.windows.iter().find_map(|(surface, entry)| {
            (matches(surface)
                && entry.target_output == output.name()
                && entry.origin != FullscreenOrigin::Maximize
                && entry.desired
                && entry.active
                && (entry.presented
                    || entry
                        .transition
                        .is_some_and(|transition| transition.is_finished_at(now)))
                && entry.external_pending.is_none())
            .then_some(surface)
        })
    }

    /// Whether compositor policy may add blur behind this surface.
    ///
    /// A client-owned fullscreen is an immersive presentation request and
    /// keeps the composition-saving fast path. Compositor-owned fullscreen
    /// (`Mod+F`) and maximize presentations remain ordinary managed-window
    /// views, so configured window effects continue through their transition.
    pub(crate) fn allows_global_blur(&self, surface: &WlSurface) -> bool {
        self.windows
            .get(surface)
            .is_none_or(|entry| fullscreen_origin_allows_global_blur(entry.origin))
    }

    /// Whether an external (X11) configure handoff is still outstanding.
    ///
    /// The transaction owns the window's geometry until the client acks, so
    /// compositor-side position resync must not race it.
    pub(crate) fn awaits_external_configure(&self, surface: &WlSurface) -> bool {
        self.windows
            .get(surface)
            .is_some_and(|entry| entry.external_pending.is_some())
    }

    pub fn is_fullscreen_or_pending(&self, surface: &WlSurface) -> bool {
        self.windows
            .get(surface)
            .is_some_and(|entry| entry.active || entry.desired)
    }

    /// Presentation ownership outlives the fullscreen state while an
    /// acknowledged exit is still animating. Other presentation modes must
    /// retire that owner before starting their own transition.
    pub(crate) fn has_presentation(&self, surface: &WlSurface, now: Duration) -> bool {
        self.windows
            .get(surface)
            .is_some_and(|entry| fullscreen_presentation(entry, now).is_some())
    }

    /// Whether compositor-owned window chrome should be omitted.
    ///
    /// Entering fullscreen removes chrome before the first animated frame.
    /// Exiting keeps chrome attached to the returning client rectangle, while
    /// `chrome_alpha` fades its pixels in near the end of the return motion.
    /// Maximize presentations remain decorated.
    pub(crate) fn suppresses_chrome(&self, surface: &WlSurface) -> bool {
        self.windows
            .get(surface)
            .is_some_and(fullscreen_entry_suppresses_chrome)
    }

    /// Protocol fullscreen remains a backstop during X11 mode negotiation.
    pub(crate) fn suppresses_window_chrome(&self, window: &Window) -> bool {
        window
            .wl_surface()
            .is_some_and(|surface| self.suppresses_chrome(surface.as_ref()))
            || crate::xwayland::is_fullscreen(window)
    }

    /// Share the existing motion timeline so chrome has no separate delay,
    /// redraw timer, or geometry animation and becomes fully visible at rest.
    pub(crate) fn chrome_alpha(&self, surface: &WlSurface, now: Duration) -> f32 {
        self.windows.get(surface).map_or(1.0, |entry| {
            fullscreen_entry_chrome_alpha(entry, animations_enabled(&self.animations), now)
        })
    }

    pub(crate) fn occupants_on_output(
        &self,
        output: &str,
        except: &WlSurface,
    ) -> Vec<(WlSurface, FullscreenOrigin)> {
        self.windows
            .iter()
            .filter(|(surface, entry)| *surface != except && entry_occupies_output(entry, output))
            .map(|(surface, entry)| (surface.clone(), entry.origin))
            .collect()
    }

    /// Client maximize requests received while Mod+F owns the window are
    /// echoes of the previous maximized state, not a new user action.
    pub(crate) fn suppresses_client_maximize(&self, surface: &WlSurface) -> bool {
        self.windows.get(surface).is_some_and(|entry| {
            entry.desired
                && entry
                    .native
                    .is_some_and(|native| native.compositor_requested)
        })
    }

    pub(crate) fn restore_placement(
        &self,
        surface: &WlSurface,
    ) -> Option<(Rectangle<i32, Logical>, Option<String>)> {
        let restore = self.windows.get(surface)?.restore.as_ref()?;
        Some((restore.geometry, restore.output.clone()))
    }

    pub(crate) fn restore_location(
        &self,
        surface: &WlSurface,
    ) -> Option<(Point<i32, Logical>, Option<String>)> {
        let restore = self.windows.get(surface)?.restore.as_ref()?;
        Some((restore.location, restore.output.clone()))
    }

    pub(crate) fn restore_presentation_output(
        &self,
        surface: &WlSurface,
    ) -> Option<Rectangle<i32, Physical>> {
        self.windows
            .get(surface)
            .and_then(|entry| entry.restore_presentation_output)
    }

    pub(crate) fn override_restore_from_field(
        &mut self,
        surface: &WlSurface,
        restore_geometry: Rectangle<i32, Logical>,
        restore_output: String,
        restore_output_rect: Option<Rectangle<i32, Physical>>,
        field_geometry: Rectangle<i32, Logical>,
        field_output_rect: Option<Rectangle<i32, Physical>>,
    ) {
        if let Some(entry) = self.windows.get_mut(surface) {
            record_maximize_handoff(
                entry,
                WindowedPlacement {
                    location: restore_geometry.loc,
                    geometry: restore_geometry,
                    output: Some(restore_output),
                },
                restore_output_rect,
                field_geometry,
                field_output_rect,
            );
        }
    }

    /// Freezes the field window's on-screen source rectangle while fullscreen
    /// parks and replaces the field camera. Cluster and maximize handoffs may
    /// subsequently replace this with their more specific endpoint.
    pub(crate) fn override_presentation_output(
        &mut self,
        surface: &WlSurface,
        presentation_output: Rectangle<i32, Physical>,
    ) {
        if let Some(entry) = self.windows.get_mut(surface)
            && entry.restore_presentation_output.is_none()
        {
            // The first output-local windowed rectangle is the canonical live
            // restore endpoint. A reversal may be visibly between endpoints;
            // never replace the canonical endpoint with that transient frame.
            retain_restore_presentation_output(entry, presentation_output);
        }
    }

    pub(crate) fn override_restore_from_cluster(
        &mut self,
        surface: &WlSurface,
        restore_geometry: Rectangle<i32, Logical>,
        restore_output: String,
        presentation_output: Option<Rectangle<i32, Physical>>,
    ) {
        if let Some(entry) = self.windows.get_mut(surface) {
            record_cluster_restore(
                entry,
                WindowedPlacement {
                    location: restore_geometry.loc,
                    geometry: restore_geometry,
                    output: Some(restore_output),
                },
                presentation_output,
            );
        }
    }

    /// Match a client commit to the fullscreen configure which requested it.
    /// Clients may skip intermediate configures, so all serials no newer than
    /// the acknowledged commit are consumed, as in Niri's resize path.
    pub fn should_capture_snapshot(&mut self, surface: &WlSurface, commit_serial: Serial) -> bool {
        let Some(entry) = self.windows.get_mut(surface) else {
            return false;
        };
        let mut capture = false;
        entry.snapshot_serials.retain(|serial| {
            if commit_serial.is_no_older_than(serial) {
                capture = true;
                false
            } else {
                true
            }
        });
        capture
    }
}
