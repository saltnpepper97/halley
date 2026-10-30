//! Shared fullscreen state transitions, geometry, and protocol helpers.

use super::*;

pub(super) fn animations_enabled(animations: &Animations) -> bool {
    animations.enabled && animations.fullscreen.enabled
}

pub(super) fn pause_entry_on_output(entry: &mut FullscreenWindow, output: &str) -> bool {
    if entry.target_output != output
        || entry.origin == FullscreenOrigin::Maximize
        || !entry.desired
        || entry.presentation_paused
    {
        return false;
    }
    entry.presentation_paused = true;
    true
}

pub(super) fn resumes_on_output_return(
    entry: &FullscreenWindow,
    output: &str,
    previous_output: Option<&str>,
) -> bool {
    entry.target_output == output && previous_output.is_some_and(|previous| previous != output)
}

/// Whether the committed xdg states still count as the protocol fullscreen
/// edge `handle_commit` is waiting on.
pub(super) fn protocol_commit_is_active(
    protocol_desired: bool,
    has_target_state: bool,
    has_fullscreen: bool,
    has_maximized: bool,
    retain_maximized: bool,
) -> bool {
    if protocol_desired {
        has_target_state
    } else if retain_maximized {
        // Mod+F entered from maximize without taking the Fullscreen bit.
        // Maximized is allowed while that fullscreen presentation is desired;
        // exit disables this exception and waits for the normal state.
        has_fullscreen
    } else {
        has_fullscreen || has_maximized
    }
}

pub(super) fn apply_protocol_presentation_state_for_request(
    state: &mut ToplevelState,
    origin: FullscreenOrigin,
    active: bool,
    retain_maximized: bool,
) {
    if retain_maximized && origin == FullscreenOrigin::Compositor && !active {
        state.states.unset(State::Fullscreen);
        state.states.set(State::Maximized);
        crate::wayland::decoration::apply_tiled_hint(state);
        return;
    }
    apply_protocol_presentation_state(state, origin, active);
}

pub(super) fn protocol_presentation_state(origin: FullscreenOrigin) -> State {
    match origin {
        FullscreenOrigin::Maximize => State::Maximized,
        FullscreenOrigin::Client | FullscreenOrigin::Compositor => State::Fullscreen,
    }
}

pub(super) fn apply_protocol_presentation_state(
    state: &mut ToplevelState,
    origin: FullscreenOrigin,
    active: bool,
) {
    state.states.unset(State::Fullscreen);
    state.states.unset(State::Maximized);
    if active {
        state.states.set(protocol_presentation_state(origin));
    }
    // Mod+F keeps native clients protocol-windowed so nested client
    // fullscreen can trigger a fresh reflow. Keep every tiled edge in that
    // state: CSD clients otherwise repaint their own rounded corners even
    // though the compositor has already removed its fullscreen mask.
    crate::wayland::decoration::apply_tiled_hint(state);
}

pub(super) fn fullscreen_presentation(
    entry: &FullscreenWindow,
    now: Duration,
) -> Option<FullscreenPresentation> {
    // A parked client keeps its fullscreen buffer and protocol state, but
    // presents that buffer at its original Field geometry. Reporting settled
    // fullscreen progress would pin the texture to the output and make it
    // travel with the camera.
    let progress = if entry.presentation_paused {
        0.0
    } else {
        visual_motion_state(entry, now).0.clamp(0.0, 1.0)
    };
    let transition_completion = entry
        .transition
        .map(|transition| transition.completion_at(now))
        .unwrap_or_else(|| {
            if entry.desired != entry.active || entry.external_pending.is_some() {
                0.0
            } else {
                1.0
            }
        });
    fullscreen_presentation_is_visible(
        progress,
        entry.presentation_paused || entry_owns_presentation(entry),
    )
    .then_some(FullscreenPresentation {
        progress,
        transition_completion,
        windowed_geometry: entry
            .presentation_windowed
            .or_else(|| entry.restore.as_ref().map(|restore| restore.geometry)),
        // presentation_output is an already-projected output-local handoff
        // source. Once navigation releases output ownership, the Field camera
        // moves independently. Project the restore geometry through that live
        // camera throughout the exit, so its final frame meets the live window
        // instead of shrinking toward the old screen position then jumping.
        windowed_output_rect: if entry.presentation_paused || entry.navigation_released {
            None
        } else {
            entry.presentation_output
        },
        fullscreen_size: entry.fullscreen_size,
    })
}

pub(super) fn fullscreen_presentation_is_visible(progress: f64, transition_active: bool) -> bool {
    // A transition owns the handoff even at its exact zero endpoint. Dropping
    // presentation there exposes the client's newly configured live buffer for
    // one frame before the captured texture blend takes over (and once again
    // on exit immediately before cleanup), which reads as a fullscreen flash.
    transition_active || progress > 0.0
}

pub(super) fn entry_owns_presentation(entry: &FullscreenWindow) -> bool {
    entry.transition.is_some() || entry.desired != entry.active || entry.external_pending.is_some()
}

pub(super) fn fullscreen_origin_allows_global_blur(origin: FullscreenOrigin) -> bool {
    origin != FullscreenOrigin::Client
}

pub(super) fn fullscreen_entry_suppresses_chrome(entry: &FullscreenWindow) -> bool {
    entry.desired && entry.origin != FullscreenOrigin::Maximize
}

pub(super) fn fullscreen_entry_chrome_alpha(
    entry: &FullscreenWindow,
    animated: bool,
    now: Duration,
) -> f32 {
    if fullscreen_entry_suppresses_chrome(entry) {
        return 0.0;
    }
    if !animated || entry.origin == FullscreenOrigin::Maximize {
        return 1.0;
    }
    // Wait for the accepted windowed commit before beginning the return fade.
    if entry.active != entry.desired || entry.external_pending.is_some() {
        return 0.0;
    }
    let Some(transition) = entry.transition else {
        return 1.0;
    };
    if transition.duration().is_zero() {
        return 1.0;
    }
    // Use elapsed time, not spring displacement: overshoot must not make the
    // titlebar flash or fade back out. Begin while the window is settling and
    // finish before the spring's almost stationary tail, rather than making
    // chrome arrive as a separate, late event. Scale with the motion duration.
    let progress = ((transition.linear_progress_at(now) - 0.65) / 0.25).clamp(0.0, 1.0);
    (progress * progress * (3.0 - 2.0 * progress)) as f32
}

/// Protocol state used for a native presentation.
///
/// Client ownership advertises fullscreen. Compositor-only Mod+F deliberately
/// remains protocol-windowed while still configuring the output-sized buffer,
/// so a later client fullscreen request produces the state edge applications
/// use to reflow nested content. Field maximize advertises maximized.
pub(super) fn native_protocol_origin(entry: &FullscreenWindow) -> FullscreenOrigin {
    match entry.native {
        Some(native) if native.client_requested => FullscreenOrigin::Client,
        Some(native) if native.compositor_requested => FullscreenOrigin::Compositor,
        _ => entry.origin,
    }
}

pub(super) fn request_native_owner(entry: &mut FullscreenWindow, origin: FullscreenOrigin) {
    entry.navigation_released = false;
    let native = entry
        .native
        .get_or_insert_with(NativeFullscreenState::default);
    native.request(origin);
    entry.desired = native.visual_desired();
    entry.origin = native.presentation_origin().unwrap_or(origin);
}

pub(super) fn release_native_owner(entry: &mut FullscreenWindow, origin: FullscreenOrigin) {
    let Some(native) = entry.native.as_mut() else {
        return;
    };
    native.release(origin);
    entry.desired = native.visual_desired();
    entry.origin = native.presentation_origin().unwrap_or(entry.origin);
}

pub(super) fn release_all_native_owners(entry: &mut FullscreenWindow) {
    if let Some(native) = entry.native.as_mut() {
        native.release_all();
    }
    entry.desired = false;
}

pub(super) fn native_owner_change_is_visual(
    native: &mut NativeFullscreenState,
    origin: FullscreenOrigin,
    requested: bool,
) -> bool {
    let before = native.visual_desired();
    if requested {
        native.request(origin);
    } else {
        native.release(origin);
    }
    before != native.visual_desired()
}

pub(super) fn fullscreen_commit_action(
    entry: &FullscreenWindow,
    protocol_committed: bool,
) -> FullscreenCommitAction {
    let protocol_desired = entry
        .native
        .map_or(entry.desired, |native| native.protocol_desired);
    if protocol_committed != protocol_desired {
        FullscreenCommitAction::Ignore
    } else if entry.desired == entry.active {
        FullscreenCommitAction::ProtocolOnly
    } else {
        FullscreenCommitAction::Visual(entry.desired)
    }
}

/// Whether a commit may retarget the fullscreen rect to the client's own size.
///
/// Adopting the committed size is how a client that stays smaller than the
/// output gets letterboxed, but the fullscreen rect is also the transition's
/// destination. Clients routinely ack the fullscreen configure a commit or two
/// before their buffer catches up, so sampling mid-flight aims the grow at the
/// old windowed size and then snaps outward when the real buffer lands. Field
/// maximize never had this because its target rect is fixed at toggle time.
pub(super) fn may_adopt_client_size(entry: &FullscreenWindow, now: Duration) -> bool {
    entry
        .transition
        .is_none_or(|transition| transition.is_finished_at(now))
}

pub(super) fn native_visual_commit_ready(
    entry: &mut FullscreenWindow,
    serial: Option<Serial>,
    buffer_size: Option<Size<i32, Logical>>,
    target_repaint_ready: bool,
    now: Duration,
) -> bool {
    if !target_repaint_ready || buffer_size.is_none() {
        return false;
    }
    if let Some(pending) = entry.native_pending
        && serial.is_none_or(|serial| !serial.is_no_older_than(&pending.serial))
    {
        return false;
    }
    if entry.origin != FullscreenOrigin::Compositor
        || native_visual_buffer_matches(entry, buffer_size, entry.desired)
    {
        return true;
    }
    let Some(pending) = entry.native_pending.as_mut() else {
        return false;
    };
    let deadline = *pending.size_retry_at.get_or_insert(now + NATIVE_SIZE_WAIT);
    if now < deadline {
        return false;
    }
    if entry.desired {
        // This acknowledged endpoint stayed smaller than requested. Center it
        // at its chosen size instead of stretching it to the configured size.
        entry.fullscreen_size = buffer_size.expect("attached endpoint checked above");
    }
    true
}

pub(super) fn reassign_fullscreen_entry(
    entry: &mut FullscreenWindow,
    output: &str,
    fullscreen_size: Size<i32, Logical>,
    restore_location: Point<i32, Logical>,
) {
    entry.target_output = output.to_owned();
    entry.fullscreen_size = fullscreen_size;
    if let Some(restore) = entry.restore.as_mut() {
        let geometry_offset = restore.geometry.loc - restore.location;
        restore.location = restore_location;
        restore.geometry.loc = restore_location + geometry_offset;
        restore.output = Some(output.to_owned());
    }
    entry.presentation_windowed = entry.restore.as_ref().map(|restore| restore.geometry);
    entry.presentation_output = None;
    entry.restore_presentation_output = None;
    entry.transition = None;
    entry.external_pending = None;
    entry.native_pending = None;
    entry.snapshot_serials.clear();
    entry.presented = entry.active;
    entry.pending_motion = (if entry.active { 1.0 } else { 0.0 }, 0.0);
}

pub(super) fn configure_reassigned_native(
    entry: &FullscreenWindow,
    state: &mut ToplevelState,
    size: Size<i32, Logical>,
    bounds: Size<i32, Logical>,
) {
    let protocol_desired = entry
        .native
        .map_or(entry.desired, |native| native.protocol_desired);
    apply_protocol_presentation_state_for_request(
        state,
        native_protocol_origin(entry),
        protocol_desired,
        retains_maximized_protocol(entry),
    );
    state.size = Some(size);
    state.bounds = Some(bounds);
    // The disabled wl_output must not survive in the next configure. Output
    // assignment already selects the fallback; no client preference is needed.
    state.fullscreen_output = None;
}

pub(super) fn native_visual_buffer_matches(
    entry: &FullscreenWindow,
    buffer_size: Option<Size<i32, Logical>>,
    desired: bool,
) -> bool {
    let Some(observed) = buffer_size else {
        return false;
    };
    if desired {
        let outgoing = entry.restore.as_ref().map(|restore| restore.geometry.size);
        observed == entry.fullscreen_size || outgoing.is_none_or(|outgoing| observed != outgoing)
    } else {
        let target = entry.restore.as_ref().map(|restore| restore.geometry.size);
        target.is_none_or(|target| observed == target) || observed != entry.fullscreen_size
    }
}

pub(super) fn committed_xdg_window_size(surface: &WlSurface) -> Option<Size<i32, Logical>> {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
            .map(|geometry| geometry.size)
    })
}

pub(super) fn entry_owns_output_presentation(entry: &FullscreenWindow) -> bool {
    !entry.presentation_paused && !entry.navigation_released
}

pub(super) fn entry_presents_immersive(entry: &FullscreenWindow, output: &str) -> bool {
    entry.origin != FullscreenOrigin::Maximize
        && entry_owns_output_presentation(entry)
        && entry_occupies_output(entry, output)
}

pub(super) fn entry_covers_top(entry: &FullscreenWindow, output: &str, now: Duration) -> bool {
    entry.target_output == output
        && entry.active
        && entry
            .transition
            .is_none_or(|transition| transition.is_finished_at(now))
}

pub(super) fn desired_matches(entry: Option<&FullscreenWindow>, desired: bool) -> bool {
    entry.is_some_and(|entry| entry.desired == desired)
}

pub(super) fn retain_restore_presentation_output(
    entry: &mut FullscreenWindow,
    presentation_output: Rectangle<i32, Physical>,
) {
    if entry.restore_presentation_output.is_none() {
        entry.presentation_output = Some(presentation_output);
        entry.restore_presentation_output = Some(presentation_output);
    }
}

pub(super) fn record_maximize_handoff(
    entry: &mut FullscreenWindow,
    restore: WindowedPlacement,
    restore_output_rect: Option<Rectangle<i32, Physical>>,
    maximized_geometry: Rectangle<i32, Logical>,
    maximized_output_rect: Option<Rectangle<i32, Physical>>,
) {
    entry.restore = Some(restore);
    entry.presentation_windowed = Some(maximized_geometry);
    entry.presentation_output = maximized_output_rect;
    entry.restore_presentation_output = restore_output_rect;
    entry.restore_kind = FullscreenRestoreKind::WindowedFromMaximize;
}

pub(super) fn record_cluster_restore(
    entry: &mut FullscreenWindow,
    restore: WindowedPlacement,
    presentation_output: Option<Rectangle<i32, Physical>>,
) {
    // A maximize handoff already owns the original normal placement and its
    // pre-maximize camera projection. Cluster updates cannot replace either.
    if entry.restore_kind == FullscreenRestoreKind::WindowedFromMaximize {
        return;
    }
    entry.presentation_windowed = Some(restore.geometry);
    entry.restore = Some(restore);
    entry.presentation_output = presentation_output;
    entry.restore_presentation_output = presentation_output;
    entry.preserve_stack = true;
}

pub(super) fn retains_maximized_protocol(entry: &FullscreenWindow) -> bool {
    entry.desired
        && entry.restore_kind == FullscreenRestoreKind::WindowedFromMaximize
        && entry.origin == FullscreenOrigin::Compositor
        && entry.native.is_some_and(|native| !native.protocol_desired)
}

pub(super) fn select_restore_presentation_endpoint(entry: &mut FullscreenWindow) {
    entry.presentation_windowed = entry.restore.as_ref().map(|restore| restore.geometry);
    entry.presentation_output = entry.restore_presentation_output;
}

pub(super) fn begin_external_restore(
    entry: &mut FullscreenWindow,
    presentation: ExternalPresentationKind,
    now: Duration,
) -> Option<ExternalTransactionRequest> {
    let geometry = entry.restore.as_ref()?.geometry;
    select_restore_presentation_endpoint(entry);
    Some(begin_external_transaction(
        entry,
        false,
        geometry,
        presentation,
        now,
    ))
}

pub(super) fn prefer_seeded_restore(
    seeded: Option<WindowedPlacement>,
    current: Option<WindowedPlacement>,
) -> Option<WindowedPlacement> {
    seeded.or(current)
}

pub(super) fn record_native_exit_placement(
    entry: &mut FullscreenWindow,
    seeded: Option<WindowedPlacement>,
    observed: WindowedPlacement,
) {
    // Native clients may acknowledge the windowed configure one commit before
    // attaching the resized buffer. At that point Space still reports the
    // fullscreen geometry. The pre-fullscreen placement is the configure's
    // authoritative endpoint; replacing it with the stale observed geometry
    // turns the exit motion into a same-size move followed by a cleanup snap.
    let placement = match seeded {
        Some(mut seeded) if observed.geometry.size != entry.fullscreen_size => {
            // Toolkits may enforce a minimum size instead of accepting the
            // exact restore configure (Firefox is a common example). Animate
            // to the buffer the client actually committed while preserving
            // the pre-fullscreen placement.
            seeded.geometry.size = observed.geometry.size;
            seeded
        }
        Some(seeded) => seeded,
        None => observed,
    };
    entry.presentation_windowed = Some(placement.geometry);
    entry.restore = Some(placement);
}

pub(super) fn can_update_external_restore(entry: &FullscreenWindow) -> bool {
    !entry.desired && entry.external_pending.is_none()
}

pub(super) fn entry_occupies_output(entry: &FullscreenWindow, output: &str) -> bool {
    entry.target_output == output
        && (entry.active
            || entry.desired
            || entry.presented
            || entry.transition.is_some()
            || entry.external_pending.is_some())
}

pub(super) fn settle_external_fullscreen(
    entry: &mut FullscreenWindow,
    target_output: &str,
    fullscreen_size: Size<i32, Logical>,
) {
    entry.navigation_released = false;
    entry.desired = true;
    entry.active = true;
    entry.presented = true;
    entry.target_output = target_output.to_string();
    entry.fullscreen_size = fullscreen_size;
    entry.transition = None;
    entry.pending_motion = (1.0, 0.0);
    entry.external_pending = None;
}

pub(super) fn begin_external_transaction(
    entry: &mut FullscreenWindow,
    desired: bool,
    geometry: Rectangle<i32, Logical>,
    presentation: ExternalPresentationKind,
    now: Duration,
) -> ExternalTransactionRequest {
    if desired {
        entry.navigation_released = false;
    }
    if entry.desired == desired {
        return ExternalTransactionRequest::NoChange;
    }
    freeze_visual_for_configure(entry, now);
    entry.presentation_paused = false;
    entry.desired = desired;
    entry.external_pending = Some(ExternalPending {
        geometry,
        presentation,
        geometry_acknowledged: false,
        surface_committed: presentation != ExternalPresentationKind::Animated,
    });
    ExternalTransactionRequest::Configure(geometry)
}

pub(super) fn acknowledge_external_geometry(
    entry: &mut FullscreenWindow,
    observed: Rectangle<i32, Logical>,
    animations: &Animations,
    now: Duration,
) -> ExternalConfigureResult {
    let Some(pending) = entry.external_pending.as_mut() else {
        return ExternalConfigureResult::NotPending;
    };
    if observed != pending.geometry {
        return ExternalConfigureResult::Waiting;
    }
    pending.geometry_acknowledged = true;
    settle_external_transaction(entry, animations, now)
}

pub(super) fn acknowledge_external_surface(
    entry: &mut FullscreenWindow,
    surface_size: Option<Size<i32, Logical>>,
    animations: &Animations,
    now: Duration,
) -> ExternalConfigureResult {
    let Some(pending) = entry.external_pending.as_mut() else {
        return ExternalConfigureResult::NotPending;
    };
    // A surface that happened to have the target size before Xwayland
    // acknowledged the configure does not prove that it contains the repaint
    // for that configure. Only a subsequent commit may release the visual
    // transaction.
    if !pending.geometry_acknowledged {
        return ExternalConfigureResult::Waiting;
    }
    if surface_size != Some(pending.geometry.size) {
        return ExternalConfigureResult::Waiting;
    }
    pending.surface_committed = true;
    settle_external_transaction(entry, animations, now)
}

pub(super) fn settle_external_transaction(
    entry: &mut FullscreenWindow,
    animations: &Animations,
    now: Duration,
) -> ExternalConfigureResult {
    let Some(pending) = entry.external_pending else {
        return ExternalConfigureResult::NotPending;
    };
    if !pending.geometry_acknowledged || !pending.surface_committed {
        return ExternalConfigureResult::Waiting;
    }
    entry.external_pending = None;
    let fullscreen = entry.desired;
    match pending.presentation {
        ExternalPresentationKind::Opening => {
            entry.active = fullscreen;
            entry.presented = fullscreen;
            entry.transition = None;
        }
        ExternalPresentationKind::Animated | ExternalPresentationKind::LiveAnimated => {
            settle_visual_commit(entry, animations, now, fullscreen);
        }
    }
    ExternalConfigureResult::Settled {
        fullscreen,
        animated: entry.transition.is_some(),
    }
}

pub(super) fn finish_external_transition(entry: &mut FullscreenWindow) {
    entry.external_pending = None;
    entry.active = entry.desired;
    entry.presented = entry.desired;
    entry.transition = None;
    entry.pending_motion = (if entry.presented { 1.0 } else { 0.0 }, 0.0);
}

pub(super) fn relocate_external_window(
    wayland: &mut WaylandState,
    window: &Window,
    entry: &FullscreenWindow,
) -> bool {
    let target = if entry.desired {
        let Some(output) = output_by_name(wayland, &entry.target_output) else {
            return false;
        };
        let Some(geometry) = wayland.space.output_geometry(&output) else {
            return false;
        };
        (output, geometry.loc)
    } else {
        let Some(restore) = entry.restore.as_ref() else {
            return false;
        };
        let Some(output) = restore
            .output
            .as_deref()
            .and_then(|name| output_by_name(wayland, name))
            .or_else(|| output_by_name(wayland, &entry.target_output))
        else {
            return false;
        };
        (output, restore.location)
    };
    crate::wayland::set_window_output(window, &target.0);
    wayland.space.relocate_element(window, target.1);
    true
}

pub(super) fn visual_motion_state(entry: &FullscreenWindow, now: Duration) -> (f64, f64) {
    if entry.desired != entry.active || entry.external_pending.is_some() {
        entry.pending_motion
    } else {
        entry
            .transition
            .map(|transition| (transition.value_at(now), transition.velocity_at(now)))
            .unwrap_or_else(|| (if entry.presented { 1.0 } else { 0.0 }, 0.0))
    }
}

pub(super) fn freeze_visual_for_configure(entry: &mut FullscreenWindow, now: Duration) {
    entry.pending_motion = visual_motion_state(entry, now);
    entry.transition = None;
}

pub(super) fn retarget_visual(
    entry: &mut FullscreenWindow,
    animations: &Animations,
    now: Duration,
    presented: bool,
) {
    let (current, velocity) = visual_motion_state(entry, now);
    if animations_enabled(animations) {
        entry.transition = Some(MotionTimeline::between(
            animations.fullscreen.motion,
            now,
            current,
            if presented { 1.0 } else { 0.0 },
            velocity,
        ));
    } else {
        entry.presented = presented;
        entry.transition = None;
    }
}

/// Start the visual timeline only after the configured client state commits.
/// This keeps geometry motion and the old-to-new texture crossfade on the same
/// clock, matching Niri's resize transaction boundary.
pub(super) fn settle_visual_commit(
    entry: &mut FullscreenWindow,
    animations: &Animations,
    now: Duration,
    desired: bool,
) {
    let target = if desired { 1.0 } else { 0.0 };
    if entry
        .transition
        .is_none_or(|transition| (transition.target() - target).abs() > f64::EPSILON)
    {
        retarget_visual(entry, animations, now, desired);
    }
    entry.active = desired;
    entry.native_pending = None;
}

pub(super) fn send_required_configure(toplevel: &ToplevelSurface) -> Option<Serial> {
    if toplevel.is_initial_configure_sent() {
        return Some(toplevel.send_configure());
    }
    None
}

pub(super) fn find_window<'a>(
    wayland: &'a WaylandState,
    surface: &WlSurface,
) -> Option<&'a Window> {
    wayland
        .space
        .elements()
        .find(|window| {
            window
                .wl_surface()
                .is_some_and(|candidate| candidate.as_ref() == surface)
        })
        .or_else(|| wayland.unmapped.get(surface))
}

pub(super) fn output_by_name(wayland: &WaylandState, name: &str) -> Option<Output> {
    wayland
        .space
        .outputs()
        .find(|output| output.name() == name)
        .cloned()
}

pub(super) fn center_in_rect(
    size: Size<i32, Logical>,
    location: Point<i32, Logical>,
    bounds: Size<i32, Logical>,
) -> Point<i32, Logical> {
    (
        location.x + (bounds.w - size.w) / 2,
        location.y + (bounds.h - size.h) / 2,
    )
        .into()
}

pub(super) fn interpolate_rect(
    from: Rectangle<i32, Physical>,
    to: Rectangle<i32, Physical>,
    progress: f64,
) -> Rectangle<i32, Physical> {
    let interpolate =
        |from: i32, to: i32| (f64::from(from) + f64::from(to - from) * progress).round() as i32;
    Rectangle::new(
        (
            interpolate(from.loc.x, to.loc.x),
            interpolate(from.loc.y, to.loc.y),
        )
            .into(),
        (
            interpolate(from.size.w, to.size.w).max(0),
            interpolate(from.size.h, to.size.h).max(0),
        )
            .into(),
    )
}
