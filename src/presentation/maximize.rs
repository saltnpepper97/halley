use std::collections::HashMap;
use std::time::Duration;

use halley_config::{Animations, Field};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Rectangle, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use crate::animation::MotionTimeline;
use crate::presentation::{PresentationScope, PresentationWorkspace};

#[derive(Clone, Copy, Debug)]
pub struct FieldMaximizePresentation {
    pub progress: f64,
    pub transition_completion: f64,
    pub windowed_rect: Rectangle<i32, Logical>,
    pub windowed_output_rect: Option<Rectangle<i32, Physical>>,
    pub target_rect: Rectangle<i32, Physical>,
}

impl FieldMaximizePresentation {
    pub fn client_rect(self, windowed: Rectangle<i32, Physical>) -> Rectangle<i32, Physical> {
        interpolate_rect(windowed, self.target_rect, self.progress)
    }
}

#[derive(Clone, Debug)]
pub struct FieldRestore {
    pub surface: WlSurface,
    pub geometry: Rectangle<i32, Logical>,
    pub output: String,
}

#[derive(Clone, Debug)]
pub struct FieldMaximizeChange {
    pub geometry: Rectangle<i32, Logical>,
    pub output: String,
    pub displaced: Option<FieldRestore>,
}

#[derive(Debug)]
struct Entry {
    surface: WlSurface,
    restore_geometry: Rectangle<i32, Logical>,
    restore_output: String,
    target_rect: Rectangle<i32, Logical>,
    /// Rect the window animation eases from, when it differs from
    /// `restore_geometry`. Set by the fullscreen handoff so the grow starts at
    /// the on-screen fullscreen rect instead of the small windowed rect the
    /// window will eventually restore to. Mirrors
    /// `FullscreenManager::presentation_windowed`.
    presentation_windowed: Option<Rectangle<i32, Logical>>,
    /// Canonical output-local rectangle of the real live windowed endpoint.
    /// This stays stable for the whole maximize cycle so the restore animation
    /// and the first post-animation live frame use the same rectangle.
    presentation_output: Option<Rectangle<i32, Physical>>,
    /// Output-local rectangle occupied by a retiring fullscreen presentation.
    /// This is only the maximize-entry source; it must not replace the live
    /// windowed endpoint used by a later unmaximize.
    handoff_output: Option<Rectangle<i32, Physical>>,
    desired: bool,
    active: bool,
    window_timeline: Option<MotionTimeline>,
    camera_timeline: Option<MotionTimeline>,
    pending_window_motion: (f64, f64),
    pending_camera_motion: (f64, f64),
}

pub struct FieldMaximizeManager {
    field: Field,
    animations: Animations,
    entries: HashMap<PresentationScope, Entry>,
}

impl FieldMaximizeManager {
    pub fn new(field: Field, animations: Animations) -> Self {
        Self {
            field,
            animations,
            entries: HashMap::new(),
        }
    }

    pub fn reload(&mut self, field: Field, animations: Animations) -> bool {
        self.field = field;
        self.animations = animations;
        if !animations_enabled(&self.animations) {
            for entry in self.entries.values_mut() {
                entry.active = entry.desired;
                entry.window_timeline = None;
                entry.camera_timeline = None;
            }
            return true;
        }
        false
    }

    pub fn gap(&self) -> f32 {
        self.field.gap
    }

    pub fn animations_enabled(&self) -> bool {
        animations_enabled(&self.animations)
    }

    pub(crate) fn toggle(
        &mut self,
        output: &Output,
        workspace: PresentationWorkspace,
        restore: FieldRestore,
        target_rect: Rectangle<i32, Logical>,
        presentation_output: Option<Rectangle<i32, Physical>>,
        now: Duration,
    ) -> FieldMaximizeChange {
        let FieldRestore {
            surface,
            geometry: restore_geometry,
            output: restore_output,
        } = restore;
        let output_name = output.name();
        let scope = PresentationScope::new(output_name.clone(), workspace);
        let (restore_geometry, restore_output) = authoritative_restore(
            self.entries
                .get(&scope)
                .filter(|entry| entry.surface == surface)
                .map(|entry| (entry.restore_geometry, entry.restore_output.clone())),
            (restore_geometry, restore_output),
        );
        match self.entries.get_mut(&scope) {
            Some(entry) if entry.surface == surface && entry.desired => {
                let (window_progress, window_velocity) = pending_aware_motion_state(
                    entry.window_timeline,
                    entry.pending_window_motion,
                    entry.desired != entry.active,
                    entry.active,
                    now,
                );
                let (camera_progress, camera_velocity) = pending_aware_motion_state(
                    entry.camera_timeline,
                    entry.pending_camera_motion,
                    entry.desired != entry.active,
                    entry.active,
                    now,
                );
                entry.desired = false;
                entry.restore_geometry = restore_geometry;
                entry.restore_output = restore_output;
                entry.presentation_windowed = None;
                entry.handoff_output = None;
                // Field maximize parks the field camera while the client grows.
                // Keep the output-local endpoint captured on entry so the
                // window does not get projected through that moving camera on
                // the way back. Cluster exits provide a newer explicit tile
                // endpoint and replace it here.
                if presentation_output.is_some() {
                    entry.presentation_output = presentation_output;
                }
                entry.window_timeline = None;
                entry.camera_timeline = None;
                entry.pending_window_motion = (window_progress, window_velocity);
                entry.pending_camera_motion = (camera_progress, camera_velocity);
                FieldMaximizeChange {
                    geometry: entry.restore_geometry,
                    output: entry.restore_output.clone(),
                    displaced: None,
                }
            }
            Some(entry) if entry.surface == surface => {
                let (window_progress, window_velocity) = pending_aware_motion_state(
                    entry.window_timeline,
                    entry.pending_window_motion,
                    entry.desired != entry.active,
                    entry.active,
                    now,
                );
                let (camera_progress, camera_velocity) = pending_aware_motion_state(
                    entry.camera_timeline,
                    entry.pending_camera_motion,
                    entry.desired != entry.active,
                    entry.active,
                    now,
                );
                entry.desired = true;
                entry.restore_geometry = restore_geometry;
                entry.restore_output = restore_output;
                entry.target_rect = target_rect;
                entry.presentation_windowed = None;
                entry.handoff_output = None;
                // This is the output-local form of the original windowed
                // endpoint. A rapid re-entry may currently be displayed at an
                // intermediate rectangle; replacing the endpoint with that
                // rectangle makes the next restore animate there and then snap
                // to the real live window.
                entry.presentation_output =
                    canonical_presentation_output(entry.presentation_output, presentation_output);
                entry.window_timeline = None;
                entry.camera_timeline = None;
                entry.pending_window_motion = (window_progress, window_velocity);
                entry.pending_camera_motion = (camera_progress, camera_velocity);
                FieldMaximizeChange {
                    geometry: target_rect,
                    output: output_name,
                    displaced: None,
                }
            }
            Some(entry) => {
                let (camera_progress, camera_velocity) =
                    motion_state(entry.camera_timeline, entry.active, now);
                let displaced = FieldRestore {
                    surface: entry.surface.clone(),
                    geometry: entry.restore_geometry,
                    output: entry.restore_output.clone(),
                };
                *entry = Entry {
                    surface,
                    restore_geometry,
                    restore_output,
                    target_rect,
                    presentation_windowed: None,
                    presentation_output,
                    handoff_output: None,
                    desired: true,
                    active: false,
                    window_timeline: None,
                    camera_timeline: None,
                    pending_window_motion: (0.0, 0.0),
                    pending_camera_motion: (camera_progress, camera_velocity),
                };
                FieldMaximizeChange {
                    geometry: target_rect,
                    output: output_name,
                    displaced: Some(displaced),
                }
            }
            None => {
                self.entries.insert(
                    scope,
                    Entry {
                        surface,
                        restore_geometry,
                        restore_output,
                        target_rect,
                        presentation_windowed: None,
                        presentation_output,
                        handoff_output: None,
                        desired: true,
                        active: false,
                        window_timeline: None,
                        camera_timeline: None,
                        pending_window_motion: (0.0, 0.0),
                        pending_camera_motion: (0.0, 0.0),
                    },
                );
                FieldMaximizeChange {
                    geometry: target_rect,
                    output: output_name,
                    displaced: None,
                }
            }
        }
    }

    pub fn presentation(
        &self,
        surface: &WlSurface,
        _output: &Output,
        output_geometry: Rectangle<i32, Logical>,
        now: Duration,
    ) -> Option<FieldMaximizePresentation> {
        let entry = self
            .entries
            .values()
            .find(|entry| &entry.surface == surface)?;
        let pending = entry.desired != entry.active;
        let progress = if pending {
            entry.pending_window_motion.0
        } else {
            progress(entry.window_timeline, entry.active, now)
        }
        .clamp(0.0, 1.0);
        let transition_completion = if pending {
            0.0
        } else {
            entry
                .window_timeline
                .map(|timeline| timeline.completion_at(now))
                .unwrap_or(1.0)
        };
        presentation_is_visible(progress, pending || entry.window_timeline.is_some()).then(|| {
            FieldMaximizePresentation {
                progress,
                transition_completion,
                windowed_rect: entry
                    .presentation_windowed
                    .unwrap_or(entry.restore_geometry),
                windowed_output_rect: maximize_windowed_output(
                    entry.handoff_output,
                    entry.presentation_output,
                ),
                target_rect: Rectangle::new(
                    (entry.target_rect.loc - output_geometry.loc).to_physical(1),
                    entry.target_rect.size.to_physical(1),
                ),
            }
        })
    }

    /// Eases the window animation from the rect a retiring fullscreen
    /// presentation last occupied, rather than from the windowed rect the
    /// window restores to. The mirror of
    /// `FullscreenManager::override_restore_from_field`, which is what already
    /// makes the maximize -> fullscreen direction start from the maximized
    /// rect.
    pub(crate) fn override_windowed_from_fullscreen(
        &mut self,
        surface: &WlSurface,
        fullscreen_geometry: Rectangle<i32, Logical>,
        fullscreen_output_rect: Option<Rectangle<i32, Physical>>,
    ) {
        if let Some(entry) = self
            .entries
            .values_mut()
            .find(|entry| &entry.surface == surface && entry.desired)
        {
            entry.presentation_windowed = Some(fullscreen_geometry);
            entry.handoff_output = fullscreen_output_rect;
        }
    }

    pub(crate) fn camera_progress(
        &self,
        output: &Output,
        workspace: PresentationWorkspace,
        now: Duration,
    ) -> Option<f32> {
        let entry = self
            .entries
            .get(&PresentationScope::new(output.name(), workspace))?;
        let progress = if entry.desired != entry.active {
            entry.pending_camera_motion.0
        } else {
            progress(entry.camera_timeline, entry.active, now)
        };
        Some(progress.clamp(0.0, 1.0) as f32)
    }

    pub fn owns_output(&self, output: &str) -> bool {
        self.entries.keys().any(|scope| scope.output == output)
    }

    pub fn contains(&self, surface: &WlSurface) -> bool {
        self.entries
            .values()
            .any(|entry| &entry.surface == surface && entry.desired)
    }

    /// Whether maximize still owns this surface's Space geometry.
    ///
    /// `contains` follows the logical maximized edge, which drops as soon as
    /// unmaximize is requested. Native clients often ack that configure before
    /// attaching the restored buffer, and Space still reports the maximized
    /// rectangle in that window. Cluster floating sync and workspace layout
    /// must keep treating the presentation as owned until `handle_commit`
    /// settles, matching `FullscreenManager::is_fullscreen_or_pending`.
    pub fn is_maximized_or_pending(&self, surface: &WlSurface) -> bool {
        self.entries.values().any(|entry| {
            &entry.surface == surface && owns_client_geometry(entry.desired, entry.active)
        })
    }

    pub fn output_for_surface(&self, surface: &WlSurface) -> Option<&str> {
        self.entries.iter().find_map(|(scope, entry)| {
            (&entry.surface == surface && entry.desired).then_some(scope.output.as_str())
        })
    }

    pub fn remove(&mut self, surface: &WlSurface) -> bool {
        self.take_restore(surface).is_some()
    }

    pub fn take_restore(&mut self, surface: &WlSurface) -> Option<FieldRestore> {
        let scope = self
            .entries
            .iter()
            .find_map(|(scope, entry)| (&entry.surface == surface).then(|| scope.clone()))?;
        self.take_scope_restore(&scope.output, scope.workspace)
    }

    pub(crate) fn take_scope_restore(
        &mut self,
        output: &str,
        workspace: PresentationWorkspace,
    ) -> Option<FieldRestore> {
        let entry = self
            .entries
            .remove(&PresentationScope::new(output, workspace))?;
        Some(FieldRestore {
            surface: entry.surface,
            geometry: entry.restore_geometry,
            output: entry.restore_output,
        })
    }

    pub fn restore(&self, surface: &WlSurface) -> Option<FieldRestore> {
        self.entries.values().find_map(|entry| {
            (&entry.surface == surface).then(|| FieldRestore {
                surface: entry.surface.clone(),
                geometry: entry.restore_geometry,
                output: entry.restore_output.clone(),
            })
        })
    }

    pub(crate) fn restore_presentation_output(
        &self,
        surface: &WlSurface,
    ) -> Option<Rectangle<i32, Physical>> {
        self.entries
            .values()
            .find(|entry| &entry.surface == surface)
            .and_then(|entry| entry.presentation_output)
    }

    pub fn remove_output(&mut self, output: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|scope, _| scope.output != output);
        self.entries.len() != before
    }

    pub fn cleanup(&mut self, now: Duration) -> FieldMaximizeCleanup {
        let mut changed = false;
        let mut finished_surfaces = Vec::new();
        self.entries.retain(|_, entry| {
            if entry
                .window_timeline
                .is_some_and(|timeline| timeline.is_finished_at(now))
            {
                entry.window_timeline = None;
                finished_surfaces.push(entry.surface.clone());
                changed = true;
            }
            if entry
                .camera_timeline
                .is_some_and(|timeline| timeline.is_finished_at(now))
            {
                entry.camera_timeline = None;
                changed = true;
            }
            entry.active
                || entry.desired
                || entry.window_timeline.is_some()
                || entry.camera_timeline.is_some()
        });
        FieldMaximizeCleanup {
            visual_finished: changed,
            finished_surfaces,
        }
    }

    pub fn is_animating(&self, now: Duration) -> bool {
        self.entries.values().any(|entry| {
            entry
                .window_timeline
                .is_some_and(|timeline| !timeline.is_finished_at(now))
                || entry
                    .camera_timeline
                    .is_some_and(|timeline| !timeline.is_finished_at(now))
        })
    }

    pub(crate) fn is_animating_on_output(
        &self,
        output: &Output,
        workspace: PresentationWorkspace,
        now: Duration,
    ) -> bool {
        self.entries
            .get(&PresentationScope::new(output.name(), workspace))
            .is_some_and(|entry| {
                entry
                    .window_timeline
                    .is_some_and(|timeline| !timeline.is_finished_at(now))
                    || entry
                        .camera_timeline
                        .is_some_and(|timeline| !timeline.is_finished_at(now))
            })
    }

    pub fn handle_commit(
        &mut self,
        wayland: &mut crate::wayland::WaylandState,
        surface: &WlSurface,
        buffer_size: Option<Size<i32, Logical>>,
        target_repaint_ready: bool,
        now: Duration,
    ) -> bool {
        let Some(scope) = self
            .entries
            .iter()
            .find_map(|(scope, entry)| (&entry.surface == surface).then(|| scope.clone()))
        else {
            return false;
        };
        let output_name = scope.output.clone();
        let Some(window) = wayland
            .space
            .elements()
            .find(|window| {
                window
                    .wl_surface()
                    .is_some_and(|candidate| candidate.as_ref() == surface)
            })
            .cloned()
        else {
            return false;
        };
        let entry = self
            .entries
            .get_mut(&scope)
            .expect("maximize output found above");
        if entry.desired == entry.active {
            return false;
        }
        let buffer_size = if let Some(toplevel) = window.toplevel() {
            let maximized_committed = toplevel.with_committed_state(|state| {
                state.is_some_and(|state| state.states.contains(State::Maximized))
            });
            if maximized_committed != entry.desired {
                return false;
            }
            committed_xdg_window_size(surface).or(buffer_size)
        } else {
            buffer_size
        };
        if !target_repaint_ready {
            return false;
        }
        let (geometry, outgoing_size) = maximize_resize_endpoints(
            entry.desired,
            entry.restore_geometry,
            entry.target_rect,
            entry.presentation_windowed,
        );
        let location = geometry.loc;
        let Some(observed_size) =
            accepted_resize_buffer_size(buffer_size, geometry.size, outgoing_size)
        else {
            return false;
        };
        if observed_size != geometry.size {
            if entry.desired {
                entry.target_rect.size = observed_size;
            } else {
                entry.restore_geometry.size = observed_size;
            }
        }
        let output = if entry.desired {
            wayland
                .space
                .outputs()
                .find(|output| output.name() == output_name)
        } else {
            wayland
                .space
                .outputs()
                .find(|output| output.name() == entry.restore_output)
        }
        .cloned();
        if let Some(output) = output {
            crate::wayland::set_window_output(&window, &output);
        }
        let target = if entry.desired { 1.0 } else { 0.0 };
        entry.window_timeline = timeline(
            &self.animations,
            now,
            entry.pending_window_motion.0,
            target,
            entry.pending_window_motion.1,
        );
        entry.camera_timeline = timeline(
            &self.animations,
            now,
            entry.pending_camera_motion.0,
            target,
            entry.pending_camera_motion.1,
        );
        entry.active = entry.desired;
        if wayland.space.element_location(&window) != Some(location) {
            wayland.space.relocate_element(&window, location);
        }
        true
    }
}

pub struct FieldMaximizeCleanup {
    pub visual_finished: bool,
    pub finished_surfaces: Vec<WlSurface>,
}

fn animations_enabled(animations: &Animations) -> bool {
    animations.enabled && animations.maximize.enabled
}

fn presentation_is_visible(progress: f64, timeline_active: bool) -> bool {
    // A running timeline owns the handoff even at its exact zero endpoint.
    // Dropping presentation there exposes the plain camera path for one frame -
    // the window has already been relocated to the maximized rect while still
    // carrying its old buffer - which reads as a flash. Same guard as
    // `fullscreen_presentation_is_visible`.
    timeline_active || progress > 0.0
}

fn timeline(
    animations: &Animations,
    now: Duration,
    from: f64,
    to: f64,
    velocity: f64,
) -> Option<MotionTimeline> {
    animations_enabled(animations)
        .then(|| MotionTimeline::between(animations.maximize.motion, now, from, to, velocity))
}

fn progress(timeline: Option<MotionTimeline>, active: bool, now: Duration) -> f64 {
    timeline
        .map(|timeline| timeline.value_at(now))
        .unwrap_or_else(|| if active { 1.0 } else { 0.0 })
}

fn motion_state(timeline: Option<MotionTimeline>, active: bool, now: Duration) -> (f64, f64) {
    timeline
        .map(|timeline| (timeline.value_at(now), timeline.velocity_at(now)))
        .unwrap_or_else(|| (if active { 1.0 } else { 0.0 }, 0.0))
}

fn pending_aware_motion_state(
    timeline: Option<MotionTimeline>,
    pending_motion: (f64, f64),
    configure_pending: bool,
    active: bool,
    now: Duration,
) -> (f64, f64) {
    if configure_pending {
        pending_motion
    } else {
        motion_state(timeline, active, now)
    }
}

fn maximize_windowed_output(
    handoff: Option<Rectangle<i32, Physical>>,
    restore: Option<Rectangle<i32, Physical>>,
) -> Option<Rectangle<i32, Physical>> {
    handoff.or(restore)
}

fn canonical_presentation_output(
    existing: Option<Rectangle<i32, Physical>>,
    requested: Option<Rectangle<i32, Physical>>,
) -> Option<Rectangle<i32, Physical>> {
    existing.or(requested)
}

fn authoritative_restore(
    existing: Option<(Rectangle<i32, Logical>, String)>,
    requested: (Rectangle<i32, Logical>, String),
) -> (Rectangle<i32, Logical>, String) {
    existing.unwrap_or(requested)
}

fn owns_client_geometry(desired: bool, active: bool) -> bool {
    active || desired
}

fn maximize_resize_endpoints(
    desired: bool,
    restore_geometry: Rectangle<i32, Logical>,
    target_rect: Rectangle<i32, Logical>,
    presentation_windowed: Option<Rectangle<i32, Logical>>,
) -> (Rectangle<i32, Logical>, Size<i32, Logical>) {
    if desired {
        // Fullscreen hands maximize a different source from the canonical
        // windowed restore. Reject that actual outgoing size until Firefox
        // commits its restored frame, rather than adopting fullscreen size
        // as a client constraint at the inset maximized location.
        (
            target_rect,
            presentation_windowed.unwrap_or(restore_geometry).size,
        )
    } else {
        (restore_geometry, target_rect.size)
    }
}

fn accepted_resize_buffer_size(
    observed: Option<Size<i32, Logical>>,
    target: Size<i32, Logical>,
    outgoing: Size<i32, Logical>,
) -> Option<Size<i32, Logical>> {
    let observed = observed?;
    (observed == target || observed != outgoing).then_some(observed)
}

fn committed_xdg_window_size(surface: &WlSurface) -> Option<Size<i32, Logical>> {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
            .map(|geometry| geometry.size)
    })
}

fn interpolate_rect(
    from: Rectangle<i32, Physical>,
    to: Rectangle<i32, Physical>,
    progress: f64,
) -> Rectangle<i32, Physical> {
    let lerp =
        |from: i32, to: i32| (f64::from(from) + f64::from(to - from) * progress).round() as i32;
    Rectangle::new(
        (lerp(from.loc.x, to.loc.x), lerp(from.loc.y, to.loc.y)).into(),
        (
            lerp(from.size.w, to.size.w).max(1),
            lerp(from.size.h, to.size.h).max(1),
        )
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_config::{AnimationCurve, AnimationMotion, EasingMotion};

    #[test]
    fn maximize_motion_retargets_without_discontinuity() {
        let mut animations = Animations::default();
        animations.maximize.motion = AnimationMotion::Easing(EasingMotion {
            duration_ms: 400,
            curve: AnimationCurve::Linear,
        });
        let started = Duration::from_secs(1);
        let forward = timeline(&animations, started, 0.0, 1.0, 0.0).unwrap();
        let reversed_at = started + Duration::from_millis(100);
        let value = forward.value_at(reversed_at);
        let velocity = forward.velocity_at(reversed_at);
        let reverse = timeline(&animations, reversed_at, value, 0.0, velocity).unwrap();

        assert!((reverse.value_at(reversed_at) - value).abs() < f64::EPSILON);
        assert_eq!(
            reverse.value_at(reversed_at + Duration::from_millis(400)),
            0.0
        );
    }

    #[test]
    fn rapid_reversal_before_commit_keeps_the_frozen_motion_sample() {
        let frozen = (0.62, -1.4);
        assert_eq!(
            pending_aware_motion_state(None, frozen, true, true, Duration::from_secs(2)),
            frozen,
            "a re-toggle before the restore commit must not jump to the active endpoint"
        );
        assert_eq!(
            pending_aware_motion_state(None, frozen, false, true, Duration::from_secs(2)),
            (1.0, 0.0),
        );
    }

    #[test]
    fn rapid_reentry_keeps_the_original_live_restore_rectangle() {
        let original = Rectangle::new((120, 90).into(), (800, 600).into());
        let intermediate = Rectangle::new((70, 55).into(), (1200, 800).into());

        assert_eq!(
            canonical_presentation_output(Some(original), Some(intermediate)),
            Some(original),
            "an intermediate reversal frame is not the post-animation live endpoint"
        );
    }

    #[test]
    fn fullscreen_handoff_source_does_not_replace_unmaximize_restore_endpoint() {
        let floating = Rectangle::new((120, 90).into(), (800, 600).into());
        let fullscreen = Rectangle::new((0, 0).into(), (1920, 1080).into());

        assert_eq!(
            maximize_windowed_output(Some(fullscreen), Some(floating)),
            Some(fullscreen),
            "maximize entry starts where fullscreen left the window"
        );
        assert_eq!(
            maximize_windowed_output(None, Some(floating)),
            Some(floating),
            "unmaximize restores to the real live windowed rectangle"
        );
    }

    #[test]
    fn maximized_rect_lands_exactly_on_target() {
        let from = Rectangle::new((100, 80).into(), (800, 600).into());
        let to = Rectangle::new((20, 20).into(), (1880, 1040).into());
        assert_eq!(interpolate_rect(from, to, 1.0), to);
    }

    #[test]
    fn unmaximize_handshake_still_owns_client_geometry() {
        assert!(owns_client_geometry(true, false));
        assert!(owns_client_geometry(true, true));
        assert!(owns_client_geometry(false, true));
        assert!(!owns_client_geometry(false, false));
    }

    #[test]
    fn existing_maximize_cycle_keeps_its_original_windowed_restore() {
        let original = Rectangle::new((120, 90).into(), (900, 650).into());
        let maximized = Rectangle::new((20, 20).into(), (1880, 1040).into());

        assert_eq!(
            authoritative_restore(
                Some((original, "DP-1".to_string())),
                (maximized, "DP-2".to_string()),
            ),
            (original, "DP-1".to_string())
        );
        assert_eq!(
            authoritative_restore(None, (maximized, "DP-2".to_string())),
            (maximized, "DP-2".to_string())
        );
    }

    #[test]
    fn maximize_waits_for_a_real_repaint_and_accepts_client_constraints() {
        let outgoing = Size::from((800, 600));
        let target = Size::from((1880, 1040));
        let constrained = Size::from((1800, 1000));

        assert_eq!(accepted_resize_buffer_size(None, target, outgoing), None);
        assert_eq!(
            accepted_resize_buffer_size(Some(outgoing), target, outgoing),
            None,
        );
        assert_eq!(
            accepted_resize_buffer_size(Some(target), target, outgoing),
            Some(target),
        );
        assert_eq!(
            accepted_resize_buffer_size(Some(constrained), target, outgoing),
            Some(constrained),
        );
    }

    #[test]
    fn fullscreen_exit_does_not_adopt_fullscreen_size_at_maximized_location() {
        let maximized = Rectangle::new((20, 70).into(), (2520, 1350).into());
        let fullscreen = Rectangle::new((0, 0).into(), (2560, 1440).into());
        // A lone tile may equal maximize, but smaller tiles in multi-window
        // clusters have the same fault. Neither is the actual outgoing frame.
        for tile in [
            maximized,
            Rectangle::new((1040, 70).into(), (1500, 1350).into()),
            Rectangle::new((780, 420).into(), (1000, 665).into()),
        ] {
            let (target, outgoing) =
                maximize_resize_endpoints(true, tile, maximized, Some(fullscreen));
            assert_eq!(
                accepted_resize_buffer_size(Some(fullscreen.size), target.size, outgoing),
                None,
                "an early maximized-state commit with the old fullscreen frame must not settle",
            );
            let restored_size =
                accepted_resize_buffer_size(Some(maximized.size), target.size, outgoing).unwrap();
            let restored = Rectangle::new(target.loc, restored_size);
            assert_eq!(restored, maximized);
            let presentation = FieldMaximizePresentation {
                progress: 1.0,
                transition_completion: 1.0,
                windowed_rect: fullscreen,
                windowed_output_rect: Some(fullscreen.to_physical(1)),
                target_rect: restored.to_physical(1),
            };
            assert_eq!(
                presentation.client_rect(fullscreen.to_physical(1)),
                maximized.to_physical(1),
                "exit ends at the inset maximized rectangle without shifting a fullscreen-sized client",
            );
        }
    }

    #[test]
    fn ordinary_maximize_still_accepts_same_size_and_client_constraints() {
        let tile = Rectangle::new((20, 70).into(), (2520, 1350).into());
        let (target, outgoing) = maximize_resize_endpoints(true, tile, tile, None);
        assert_eq!(
            accepted_resize_buffer_size(Some(tile.size), target.size, outgoing),
            Some(tile.size),
        );

        let floating = Rectangle::new((100, 90).into(), (800, 600).into());
        let (target, outgoing) = maximize_resize_endpoints(true, floating, tile, None);
        assert_eq!(
            accepted_resize_buffer_size(Some(floating.size), target.size, outgoing),
            None,
        );
        let constrained = (2400, 1300).into();
        assert_eq!(
            accepted_resize_buffer_size(Some(constrained), target.size, outgoing),
            Some(constrained),
        );
    }

    #[test]
    fn unmaximize_after_fullscreen_uses_the_maximized_source_and_original_restore() {
        let floating = Rectangle::new((100, 90).into(), (800, 600).into());
        let maximized = Rectangle::new((20, 70).into(), (2520, 1350).into());
        let fullscreen = Rectangle::new((0, 0).into(), (2560, 1440).into());
        let (target, outgoing) =
            maximize_resize_endpoints(false, floating, maximized, Some(fullscreen));
        assert_eq!(
            accepted_resize_buffer_size(Some(maximized.size), target.size, outgoing),
            None,
        );
        let restored = Rectangle::new(
            target.loc,
            accepted_resize_buffer_size(Some(floating.size), target.size, outgoing).unwrap(),
        );
        assert_eq!(restored, floating);
    }

    #[test]
    fn presentation_survives_its_own_zero_endpoint() {
        assert!(
            presentation_is_visible(0.0, true),
            "the handoff frame must not fall back to the plain camera path"
        );
        assert!(presentation_is_visible(0.5, false));
        assert!(!presentation_is_visible(0.0, false));
    }

    #[test]
    fn fullscreen_handoff_starts_the_grow_at_the_fullscreen_rect() {
        let fullscreen = Rectangle::new((0, 0).into(), (1920, 1080).into());
        let target = Rectangle::new((20, 20).into(), (1880, 1040).into());
        let presentation = |source, progress| FieldMaximizePresentation {
            progress,
            transition_completion: progress,
            windowed_rect: Rectangle::new((100, 80).into(), (800, 600).into()),
            windowed_output_rect: Some(source),
            target_rect: target,
        };

        // The handoff source replaces the windowed rect the window restores to,
        // so the first frame is exactly where fullscreen left off rather than a
        // jump down to the small rect.
        for source in [
            fullscreen,
            Rectangle::new((50, 40).into(), (1360, 840).into()),
        ] {
            // A quick fullscreen exit -> maximize also inherits the current
            // intermediate rectangle, without waiting for the windowed size.
            for (progress, expected) in [(0.0, source), (1.0, target)] {
                let presentation = presentation(source, progress);
                let windowed = presentation.windowed_output_rect.unwrap();
                assert_eq!(presentation.client_rect(windowed), expected);
            }
        }
    }
}
