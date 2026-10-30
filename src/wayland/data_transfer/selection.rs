use smithay::input::dnd::{DnDGrab, DndFocus, DndGrabHandler, GrabType, Source};
use smithay::input::pointer::Focus;
use smithay::input::{Seat, SeatHandler};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};
use smithay::utils::Serial;
use smithay::wayland::selection::data_device::{DataDeviceHandler, set_data_device_focus};
use smithay::wayland::selection::primary_selection::{PrimarySelectionHandler, set_primary_focus};

/// Points both selections at the focused surface's client, so that client -
/// and only it - may read the clipboard and primary selection.
///
/// Has to be called explicitly on every focus change: Smithay exposes
/// `set_data_device_focus`/`set_primary_focus` as freestanding functions and
/// never calls them off the back of `KeyboardHandle::set_focus` itself. Skip
/// this and both globals still advertise, clients still bind them, and
/// copy/paste silently does nothing - the failure mode is a working-looking
/// clipboard that never transfers, not an error.
pub fn sync_selection_focus<D>(
    display_handle: &DisplayHandle,
    seat: &Seat<D>,
    focused: Option<&WlSurface>,
) where
    D: SeatHandler + DataDeviceHandler + PrimarySelectionHandler + 'static,
{
    let client = focused.and_then(|surface| surface.client());
    set_data_device_focus(display_handle, seat, client.clone());
    set_primary_focus(display_handle, seat, client);
}

/// Promotes the implicit pointer/touch grab a client already holds into a
/// real server-side drag-and-drop grab, in response to
/// `wl_data_device.start_drag`.
///
/// This has to be implemented explicitly: `WaylandDndGrabHandler`'s default
/// `dnd_requested` just cancels the source, which silently kills *every*
/// client-initiated drag (dragging a Firefox tab out, dropping a file onto a
/// terminal, and so on) rather than failing loudly. Ported from old halley's
/// own `dnd_requested` (`halley-wl/src/protocol/wayland/handlers.rs`), which
/// carries the same warning.
///
/// Shared by both session drivers rather than duplicated into each one: it's
/// pure seat/grab plumbing with nothing backend-specific in it, so it takes
/// the app type as a parameter the same way `input::match_bind` and
/// `input::grab`'s conversions do.
pub fn start_dnd_grab<D, S>(
    state: &mut D,
    display_handle: &DisplayHandle,
    source: S,
    seat: Seat<D>,
    serial: Serial,
    type_: GrabType,
) -> bool
where
    D: SeatHandler + DndGrabHandler + 'static,
    D::PointerFocus: DndFocus<D>,
    D::TouchFocus: DndFocus<D>,
    S: Source,
{
    match type_ {
        GrabType::Pointer => {
            let start = seat.get_pointer().and_then(|pointer| {
                pointer
                    .grab_start_data()
                    .map(|start_data| (pointer, start_data))
            });
            match start {
                Some((pointer, start_data)) => {
                    let grab = DnDGrab::new_pointer(display_handle, start_data, source, seat);
                    // `Focus::Keep` - the drag has to stay bound to the
                    // surface the button went down on; re-resolving focus
                    // mid-drag would hand the grab to whatever the cursor
                    // happens to be over.
                    pointer.set_grab(state, grab, serial, Focus::Keep);
                    true
                }
                // No implicit grab to promote (the button was already
                // released, or the serial didn't match a real press) -
                // cancelling tells the client the drag didn't start, which is
                // what it's waiting to hear.
                None => {
                    source.cancel();
                    false
                }
            }
        }
        GrabType::Touch => {
            let start = seat.get_touch().and_then(|touch| {
                touch
                    .grab_start_data()
                    .map(|start_data| (touch, start_data))
            });
            match start {
                Some((touch, start_data)) => {
                    let grab = DnDGrab::new_touch(display_handle, start_data, source, seat);
                    touch.set_grab(state, grab, serial);
                    true
                }
                None => {
                    source.cancel();
                    false
                }
            }
        }
    }
}
