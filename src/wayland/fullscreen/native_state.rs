//! Native ownership, commits, and output reassignment.

use super::*;

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
