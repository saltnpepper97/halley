mod external;
mod external_state;
mod native;
mod native_state;
mod presentation;
mod state;

use external_state::*;
use native_state::*;
use state::*;

use std::collections::HashMap;
use std::time::Duration;

use halley_config::Animations;
use smithay::desktop::Window;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Serial, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::{SurfaceCachedState, ToplevelState, ToplevelSurface};

use crate::animation::MotionTimeline;
use crate::presentation::camera::{FullscreenCameraFrame, OutputCameras};

use super::WaylandState;

// Give clients which acknowledge before resizing a chance to attach their
// target buffer, without pinning a valid fixed-size client indefinitely.
const NATIVE_SIZE_WAIT: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug)]
struct NativePending {
    serial: Serial,
    size_retry_at: Option<Duration>,
}

#[derive(Clone, Debug)]
struct WindowedPlacement {
    location: Point<i32, Logical>,
    geometry: Rectangle<i32, Logical>,
    output: Option<String>,
}

#[derive(Debug)]
struct FullscreenWindow {
    desired: bool,
    active: bool,
    presented: bool,
    target_output: String,
    restore: Option<WindowedPlacement>,
    presentation_windowed: Option<Rectangle<i32, Logical>>,
    /// Exact output-local source retained across a mode-to-mode handoff. This
    /// rectangle already includes the outgoing camera and must not be projected
    /// through the incoming camera again.
    presentation_output: Option<Rectangle<i32, Physical>>,
    /// Output-local rectangle of the real post-fullscreen live endpoint.
    /// This can differ from `presentation_output` during a maximize-to-
    /// fullscreen handoff, where entry starts at the maximized rectangle but
    /// exit restores the earlier windowed rectangle.
    restore_presentation_output: Option<Rectangle<i32, Physical>>,
    fullscreen_size: Size<i32, Logical>,
    transition: Option<MotionTimeline>,
    /// Motion sample held while the matching client configure/repaint is
    /// pending. Rapid reversals reuse this sample instead of snapping back to
    /// the last committed endpoint before the new timeline can begin.
    pending_motion: (f64, f64),
    external_pending: Option<ExternalPending>,
    snapshot_serials: Vec<Serial>,
    origin: FullscreenOrigin,
    native: Option<NativeFullscreenState>,
    native_pending: Option<NativePending>,
    restore_kind: FullscreenRestoreKind,
    preserve_stack: bool,
    /// Temporarily releases output-camera/top-layer ownership while retaining
    /// the client's fullscreen protocol and geometry. Navigation pauses resume
    /// through direct activation.
    presentation_paused: bool,
    /// An ordinary app is exiting after explicit navigation. Keep its resize
    /// transition alive without retaining ownership of the output camera.
    navigation_released: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum FullscreenRestoreKind {
    #[default]
    Windowed,
    /// Entry began at maximize, but exit owns the pre-maximize normal window.
    WindowedFromMaximize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FullscreenOrigin {
    Client,
    Compositor,
    Maximize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NativeFullscreenState {
    client_requested: bool,
    compositor_requested: bool,
    protocol_desired: bool,
    protocol_active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FullscreenCommitAction {
    Ignore,
    ProtocolOnly,
    Visual(bool),
}

impl NativeFullscreenState {
    fn request(&mut self, origin: FullscreenOrigin) {
        match origin {
            FullscreenOrigin::Client => {
                self.client_requested = true;
                self.protocol_desired = true;
            }
            FullscreenOrigin::Compositor => {
                self.compositor_requested = true;
                // Mod+F owns the output presentation, not the client's
                // fullscreen state. Keeping this protocol-windowed gives
                // Firefox a real state edge when HTML video later enters
                // fullscreen, which is required for correct 16:10 reflow.
                self.protocol_desired = self.client_requested;
            }
            FullscreenOrigin::Maximize => {}
        }
    }

    fn release(&mut self, origin: FullscreenOrigin) {
        match origin {
            FullscreenOrigin::Client => {
                self.client_requested = false;
                // Honor the client's logical fullscreen edge even while Mod+F
                // retains the output presentation. Firefox uses this edge to
                // reflow HTML fullscreen content when the toplevel was already
                // fullscreen before the video entered fullscreen.
                self.protocol_desired = false;
            }
            FullscreenOrigin::Compositor => {
                self.compositor_requested = false;
                self.protocol_desired = self.client_requested;
            }
            FullscreenOrigin::Maximize => {}
        }
    }

    fn release_all(&mut self) {
        self.client_requested = false;
        self.compositor_requested = false;
        self.protocol_desired = false;
    }

    fn visual_desired(self) -> bool {
        self.client_requested || self.compositor_requested
    }

    fn presentation_origin(self) -> Option<FullscreenOrigin> {
        if self.compositor_requested {
            Some(FullscreenOrigin::Compositor)
        } else if self.client_requested {
            Some(FullscreenOrigin::Client)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExternalPresentationKind {
    Opening,
    Animated,
    /// Keep client buffers live while the compositor animates geometry. Game
    /// swapchains must not wait on a captured endpoint or full-buffer damage.
    LiveAnimated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ExternalPending {
    geometry: Rectangle<i32, Logical>,
    presentation: ExternalPresentationKind,
    geometry_acknowledged: bool,
    surface_committed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExternalTransactionRequest {
    NoChange,
    Configure(Rectangle<i32, Logical>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExternalConfigureResult {
    NotPending,
    Waiting,
    Settled { fullscreen: bool, animated: bool },
}

#[derive(Clone, Copy, Debug)]
pub struct FullscreenPresentation {
    pub progress: f64,
    pub transition_completion: f64,
    pub windowed_geometry: Option<Rectangle<i32, Logical>>,
    pub windowed_output_rect: Option<Rectangle<i32, Physical>>,
    pub fullscreen_size: Size<i32, Logical>,
}

impl FullscreenPresentation {
    pub fn fullscreen_rect(self, output_size: Size<i32, Physical>) -> Rectangle<i32, Physical> {
        let fullscreen_size = self.fullscreen_size.to_physical(1);
        Rectangle::new(
            (
                (output_size.w - fullscreen_size.w) / 2,
                (output_size.h - fullscreen_size.h) / 2,
            )
                .into(),
            fullscreen_size,
        )
    }

    pub fn client_rect(
        self,
        windowed: Rectangle<i32, Physical>,
        output_size: Size<i32, Physical>,
    ) -> Rectangle<i32, Physical> {
        let fullscreen = self.fullscreen_rect(output_size);
        interpolate_rect(windowed, fullscreen, self.progress)
    }
}

pub struct FullscreenManager {
    animations: Animations,
    windows: HashMap<WlSurface, FullscreenWindow>,
}

#[derive(Default)]
pub(crate) struct OutputReassignment {
    pub retired_snapshots: Vec<WlSurface>,
    pub external_configures: Vec<(Window, Rectangle<i32, Logical>)>,
}

impl FullscreenManager {
    pub fn new(animations: Animations) -> Self {
        Self {
            animations,
            windows: HashMap::new(),
        }
    }

    // Navigation changes fullscreen only on the destination monitor. Filter
    // here, before app/game policy, so leaving another monitor cannot exit or
    // park its fullscreen presentation.
    pub(crate) fn navigation_candidates(
        &self,
        output: &str,
        selected: &WlSurface,
    ) -> Vec<WlSurface> {
        self.windows
            .iter()
            .filter(|(surface, entry)| {
                *surface != selected
                    && entry.target_output == output
                    && entry.origin != FullscreenOrigin::Maximize
                    && entry.desired
                    && !entry.presentation_paused
            })
            .map(|(surface, _)| surface.clone())
            .collect()
    }

    /// A game releases its output only when navigation selects another window
    /// on that same output. Other monitors can be used without moving the game
    /// back into the Field or changing its fullscreen protocol/client size.
    pub(crate) fn pause_presentation_on_output(
        &mut self,
        surface: &WlSurface,
        output: &str,
    ) -> bool {
        self.windows
            .get_mut(surface)
            .is_some_and(|entry| pause_entry_on_output(entry, output))
    }

    pub(crate) fn release_output_for_navigation(&mut self, surface: &WlSurface) {
        if let Some(entry) = self.windows.get_mut(surface)
            && !entry.desired
        {
            entry.navigation_released = true;
        }
    }

    /// Resumes any parked fullscreen presentation after direct pointer
    /// activation.
    pub(crate) fn resume_presentation(&mut self, surface: &WlSurface) -> Option<String> {
        self.resume_presentation_if(surface, |_| true)
    }

    /// Physical hover returning from a different monitor can resume a parked
    /// game. Same-output hover keeps the deliberate click-to-resume policy.
    pub(crate) fn resume_presentation_on_output_return(
        &mut self,
        surface: &WlSurface,
        output: &str,
        previous_output: Option<&str>,
    ) -> Option<String> {
        self.resume_presentation_if(surface, |entry| {
            resumes_on_output_return(entry, output, previous_output)
        })
    }

    pub(crate) fn is_presentation_paused(&self, surface: &WlSurface) -> bool {
        self.windows
            .get(surface)
            .is_some_and(|entry| entry.presentation_paused)
    }

    fn resume_presentation_if(
        &mut self,
        surface: &WlSurface,
        allows: impl FnOnce(&FullscreenWindow) -> bool,
    ) -> Option<String> {
        let entry = self.windows.get_mut(surface)?;
        if !entry.presentation_paused
            || entry.origin == FullscreenOrigin::Maximize
            || !allows(entry)
        {
            return None;
        }
        entry.presentation_paused = false;
        Some(entry.target_output.clone())
    }

    pub fn reload(&mut self, animations: Animations) -> bool {
        self.animations = animations;
        if animations_enabled(&self.animations) {
            return false;
        }
        self.windows.retain(|_, entry| {
            entry.transition = None;
            entry.snapshot_serials.clear();
            entry.presented = entry.desired;
            entry.pending_motion = (if entry.presented { 1.0 } else { 0.0 }, 0.0);
            entry.active || entry.desired || entry.presented
        });
        true
    }

    /// Whether changing one native fullscreen owner changes what is actually
    /// presented. Client and compositor ownership can overlap; adding or
    /// removing one owner while the other remains must not replace the
    /// outgoing texture or start a second visual transition.
    pub(crate) fn client_request_changes_visual(
        &self,
        surface: &WlSurface,
        requested: bool,
    ) -> bool {
        self.owner_request_changes_visual(surface, FullscreenOrigin::Client, requested)
    }

    pub(crate) fn compositor_request_changes_visual(
        &self,
        surface: &WlSurface,
        requested: bool,
    ) -> bool {
        self.owner_request_changes_visual(surface, FullscreenOrigin::Compositor, requested)
    }

    fn owner_request_changes_visual(
        &self,
        surface: &WlSurface,
        origin: FullscreenOrigin,
        requested: bool,
    ) -> bool {
        if !animations_enabled(&self.animations) {
            return false;
        }
        let mut native = self
            .windows
            .get(surface)
            .and_then(|entry| entry.native)
            .unwrap_or_default();
        native_owner_change_is_visual(&mut native, origin, requested)
    }

    /// Request-boundary snapshots supersede the configure-ack fallback. Once
    /// the caller captured the frame currently on screen, a later pre-commit
    /// hook must never replace it with a client tree that has already begun
    /// resizing.
    pub(crate) fn discard_snapshot_requests(&mut self, surface: &WlSurface) {
        if let Some(entry) = self.windows.get_mut(surface) {
            entry.snapshot_serials.clear();
        }
    }

    pub fn handle_commit(
        &mut self,
        wayland: &mut WaylandState,
        cameras: &OutputCameras,
        surface: &WlSurface,
        buffer_size: Option<Size<i32, Logical>>,
        target_repaint_ready: bool,
        now: Duration,
    ) -> bool {
        let Some(window) = find_window(wayland, surface).cloned() else {
            return false;
        };
        let Some(toplevel) = window.toplevel() else {
            return false;
        };
        // Compare the configured client window, not the raw root-surface
        // allocation. Firefox keeps transparent CSD/shadow margins around a
        // windowed surface, so its old buffer can be larger than the outgoing
        // xdg geometry and would otherwise masquerade as a resize.
        let buffer_size = committed_xdg_window_size(surface).or(buffer_size);
        let Some(entry) = self.windows.get(surface) else {
            return false;
        };
        let protocol_origin = native_protocol_origin(entry);
        let protocol_desired = entry
            .native
            .map_or(entry.desired, |native| native.protocol_desired);
        let visual_desired = entry.desired;
        let target_output = entry.target_output.clone();
        let restore = entry.restore.clone();
        let preserve_stack = entry.preserve_stack;
        let retain_maximized = retains_maximized_protocol(entry);
        let committed = toplevel.with_committed_state(|state| {
            state.is_some_and(|state| {
                protocol_commit_is_active(
                    protocol_desired,
                    state
                        .states
                        .contains(protocol_presentation_state(protocol_origin)),
                    state.states.contains(State::Fullscreen),
                    state.states.contains(State::Maximized),
                    retain_maximized,
                )
            })
        });
        let commit_action = fullscreen_commit_action(entry, committed);
        if commit_action == FullscreenCommitAction::Ignore {
            return false;
        }
        if matches!(commit_action, FullscreenCommitAction::Visual(_)) {
            let serial = toplevel.with_cached_state(|state| {
                state.last_acked.as_ref().map(|configure| configure.serial)
            });
            let entry = self.windows.get_mut(surface).expect("entry checked above");
            if !native_visual_commit_ready(entry, serial, buffer_size, target_repaint_ready, now) {
                return false;
            }
        }
        if let Some(native) = self
            .windows
            .get_mut(surface)
            .and_then(|entry| entry.native.as_mut())
        {
            native.protocol_active = committed;
        }
        if commit_action == FullscreenCommitAction::ProtocolOnly {
            if !visual_desired {
                return false;
            }
            let Some(output) = output_by_name(wayland, &target_output) else {
                return false;
            };
            let Some(output_geometry) = wayland.space.output_geometry(&output) else {
                return false;
            };
            let size = window.geometry().size;
            let location = center_in_rect(size, output_geometry.loc, output_geometry.size);
            if wayland.space.element_location(&window) != Some(location) {
                wayland.space.relocate_element(&window, location);
            }
            let entry = self.windows.get_mut(surface).expect("entry checked above");
            if may_adopt_client_size(entry, now) {
                entry.fullscreen_size = size;
            }
            return false;
        }

        let Some(output) = output_by_name(wayland, &target_output) else {
            return false;
        };
        let Some(output_geometry) = wayland.space.output_geometry(&output) else {
            return false;
        };

        if visual_desired {
            super::set_window_output(&window, &output);
            let location = center_in_rect(
                window.geometry().size,
                output_geometry.loc,
                output_geometry.size,
            );
            if preserve_stack {
                wayland.space.relocate_element(&window, location);
            } else {
                wayland.space.map_element(window.clone(), location, true);
                crate::window::enforce_dialog_stacking(wayland);
            }
        } else {
            let location = match restore.as_ref() {
                Some(restore) if restore.output.as_deref() == Some(target_output.as_str()) => {
                    restore.location
                }
                _ => crate::window::routing::centered_location_for_size(
                    wayland,
                    cameras,
                    &output,
                    window.geometry().size,
                ),
            };
            super::set_window_output(&window, &output);
            if preserve_stack {
                wayland.space.relocate_element(&window, location);
            } else {
                wayland.space.map_element(window.clone(), location, true);
                crate::window::enforce_dialog_stacking(wayland);
            }
        }

        let entry = self.windows.get_mut(surface).expect("entry checked above");
        if !visual_desired
            && let (Some(location), Some(geometry)) = (
                wayland.space.element_location(&window),
                wayland.space.element_geometry(&window),
            )
        {
            record_native_exit_placement(
                entry,
                restore,
                WindowedPlacement {
                    location,
                    geometry,
                    output: Some(output.name()),
                },
            );
        }
        settle_visual_commit(entry, &self.animations, now, visual_desired);
        true
    }

    pub(crate) fn should_capture_external_snapshot(
        &self,
        surface: &WlSurface,
        fullscreen: bool,
    ) -> bool {
        animations_enabled(&self.animations)
            && self
                .windows
                .get(surface)
                .is_none_or(|entry| entry.desired != fullscreen)
    }

    /// The old output and its output-local animation endpoints are gone.
    /// Keep protocol ownership, but move the transaction and its restore
    /// placement together before accepting any more client commits.
    pub(crate) fn reassign_disabled_output(
        &mut self,
        wayland: &mut WaylandState,
        cameras: &OutputCameras,
        disabled: &str,
        fallback: &Output,
    ) -> OutputReassignment {
        let mut result = OutputReassignment::default();
        let Some(target) = wayland.space.output_geometry(fallback) else {
            return result;
        };
        let fallback_name = fallback.name();
        let mut occupied = self.windows.values().any(|entry| {
            entry_owns_output_presentation(entry) && entry_occupies_output(entry, &fallback_name)
        });
        for (surface, entry) in &mut self.windows {
            if entry.target_output != disabled {
                continue;
            }
            let Some(window) = find_window(wayland, surface).cloned() else {
                continue;
            };
            let windowed_size = entry
                .restore
                .as_ref()
                .map_or(window.geometry().size, |restore| restore.geometry.size);
            let restore_location = crate::window::routing::centered_location_for_size(
                wayland,
                cameras,
                fallback,
                windowed_size,
            );
            reassign_fullscreen_entry(entry, &fallback_name, target.size, restore_location);
            entry.presentation_paused |= occupied;
            occupied |= entry_owns_output_presentation(entry)
                && entry_occupies_output(entry, &fallback_name);
            super::set_window_output(&window, fallback);
            let geometry = if entry.desired {
                target
            } else {
                entry
                    .restore
                    .as_ref()
                    .map_or(Rectangle::new(restore_location, windowed_size), |restore| {
                        restore.geometry
                    })
            };
            let location = if entry.desired {
                center_in_rect(window.geometry().size, target.loc, target.size)
            } else {
                restore_location
            };
            wayland.space.relocate_element(&window, location);
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|state| {
                    configure_reassigned_native(entry, state, geometry.size, target.size);
                });
                if let Some(serial) = send_required_configure(toplevel)
                    && entry.desired != entry.active
                {
                    entry.native_pending = Some(NativePending {
                        serial,
                        size_retry_at: None,
                    });
                }
            } else {
                // A cross-output move has no usable old animation endpoint.
                // Supersede the old X11 geometry/buffer acknowledgement pair.
                finish_external_transition(entry);
                result.external_configures.push((window, geometry));
            }
            result.retired_snapshots.push(surface.clone());
        }
        result
    }

    pub fn reconfigure_output(
        &mut self,
        wayland: &WaylandState,
        output: &Output,
    ) -> Vec<(Window, Rectangle<i32, Logical>)> {
        let Some(geometry) = wayland.space.output_geometry(output) else {
            return Vec::new();
        };
        let mut external = Vec::new();
        for (surface, entry) in &mut self.windows {
            if entry.target_output != output.name() || !(entry.active || entry.desired) {
                continue;
            }
            let Some(window) = find_window(wayland, surface) else {
                continue;
            };
            if let Some(toplevel) = window.toplevel() {
                let protocol_origin = native_protocol_origin(entry);
                let protocol_desired = entry
                    .native
                    .map_or(entry.desired, |native| native.protocol_desired);
                entry.fullscreen_size = geometry.size;
                toplevel.with_pending_state(|state| {
                    apply_protocol_presentation_state(state, protocol_origin, protocol_desired);
                    state.size = Some(geometry.size);
                    state.bounds = Some(geometry.size);
                });
                let serial = toplevel.send_configure();
                if entry.native_pending.is_some() {
                    entry.native_pending = Some(NativePending {
                        serial,
                        size_retry_at: None,
                    });
                }
            } else {
                entry.fullscreen_size = geometry.size;
                external.push((window.clone(), geometry));
            }
        }
        external
    }

    /// Retry an already acknowledged, valid endpoint without requiring the
    /// client to produce another commit just to release the size heuristic.
    pub(crate) fn native_size_retries_due(&self, now: Duration) -> Vec<WlSurface> {
        self.windows
            .iter()
            .filter_map(|(surface, entry)| {
                entry
                    .native_pending
                    .and_then(|pending| pending.size_retry_at)
                    .filter(|deadline| now >= *deadline)
                    .map(|_| surface.clone())
            })
            .collect()
    }

    pub(crate) fn defer_native_size_retry_until_commit(&mut self, surface: &WlSurface) {
        if let Some(pending) = self
            .windows
            .get_mut(surface)
            .and_then(|entry| entry.native_pending.as_mut())
        {
            pending.size_retry_at = None;
        }
    }

    pub fn cleanup(&mut self, now: Duration) -> FullscreenCleanup {
        let mut finished = false;
        let mut finished_surfaces = Vec::new();
        self.windows.retain(|surface, entry| {
            if entry
                .transition
                .is_some_and(|transition| transition.is_finished_at(now))
                && entry.active == entry.desired
                && entry.external_pending.is_none()
            {
                entry.transition = None;
                entry.presented = entry.desired;
                finished = true;
                finished_surfaces.push(surface.clone());
            }
            entry.active
                || entry.desired
                || entry.presented
                || entry.transition.is_some()
                || entry.external_pending.is_some()
        });
        FullscreenCleanup {
            visual_finished: finished,
            finished_surfaces,
        }
    }

    pub(crate) fn reassociate_external_surface(
        &mut self,
        previous: &WlSurface,
        surface: &WlSurface,
    ) {
        if self
            .windows
            .get(previous)
            .is_some_and(|entry| entry.native.is_none())
            && let Some(entry) = self.windows.remove(previous)
        {
            self.windows.insert(surface.clone(), entry);
        }
    }

    pub fn remove(&mut self, surface: &WlSurface) {
        self.windows.remove(surface);
    }
}

pub struct FullscreenCleanup {
    pub visual_finished: bool,
    pub finished_surfaces: Vec<WlSurface>,
}

#[cfg(test)]
mod tests;
