use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};
use smithay::utils::Serial;
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::set_primary_focus;

use crate::compositor::root::Halley;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SeatFocusOwner {
    Interaction,
    LayerShell,
    SessionLock,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SeatFocusState {
    owner: Option<SeatFocusOwner>,
    target: Option<ObjectId>,
}

fn target_id(focus: Option<&WlSurface>) -> Option<ObjectId> {
    focus.map(Resource::id)
}

pub(crate) fn sync_selection_focus(st: &Halley, surface: Option<&WlSurface>) {
    let client = surface.and_then(|wl| wl.client());
    set_data_device_focus(
        &st.platform.display_handle,
        &st.platform.seat,
        client.clone(),
    );
    set_primary_focus(&st.platform.display_handle, &st.platform.seat, client);
}

fn request_changes_owner_or_target(
    state: &SeatFocusState,
    owner: SeatFocusOwner,
    target: Option<&WlSurface>,
) -> bool {
    state.owner != Some(owner) || state.target != target_id(target)
}

fn apply(
    st: &mut Halley,
    owner: SeatFocusOwner,
    focus: Option<WlSurface>,
    serial: Serial,
    allow_repair: bool,
) -> bool {
    if !allow_repair
        && !request_changes_owner_or_target(&st.model.focus_state.seat, owner, focus.as_ref())
    {
        return false;
    }

    let Some(keyboard) = st.platform.seat.get_keyboard() else {
        return false;
    };
    let actual = keyboard.current_focus();
    let changed = target_id(actual.as_ref()) != target_id(focus.as_ref());
    if changed {
        crate::input::keyboard::flush_stuck_forwarded_keys(st);
        keyboard.set_focus(st, focus.clone(), serial);
    }

    sync_selection_focus(st, focus.as_ref());
    st.model.focus_state.seat = SeatFocusState {
        owner: Some(owner),
        target: target_id(focus.as_ref()),
    };
    changed
}

pub(crate) fn set_interaction_focus(
    st: &mut Halley,
    focus: Option<WlSurface>,
    serial: Serial,
) -> bool {
    apply(st, SeatFocusOwner::Interaction, focus, serial, false)
}

pub(crate) fn set_layer_shell_focus(
    st: &mut Halley,
    focus: Option<WlSurface>,
    serial: Serial,
) -> bool {
    apply(st, SeatFocusOwner::LayerShell, focus, serial, true)
}

pub(crate) fn set_session_lock_focus(
    st: &mut Halley,
    focus: Option<WlSurface>,
    serial: Serial,
) -> bool {
    apply(st, SeatFocusOwner::SessionLock, focus, serial, true)
}

#[cfg(test)]
mod tests {
    use super::{SeatFocusOwner, SeatFocusState, request_changes_owner_or_target};

    #[test]
    fn repeated_interaction_request_does_not_reapply_focus() {
        let state = SeatFocusState {
            owner: Some(SeatFocusOwner::Interaction),
            target: None,
        };

        assert!(!request_changes_owner_or_target(
            &state,
            SeatFocusOwner::Interaction,
            None,
        ));
    }

    #[test]
    fn ownership_transition_applies_even_when_target_is_none() {
        let state = SeatFocusState {
            owner: Some(SeatFocusOwner::LayerShell),
            target: None,
        };

        assert!(request_changes_owner_or_target(
            &state,
            SeatFocusOwner::Interaction,
            None,
        ));
    }
}
