//! External fullscreen transaction state and geometry.

use super::*;

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
