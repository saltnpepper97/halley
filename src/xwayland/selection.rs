use std::os::fd::OwnedFd;

use smithay::input::Seat;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::selection::data_device::{
    clear_data_device_selection, current_data_device_selection_userdata,
    request_data_device_client_selection, set_data_device_selection,
};
use smithay::wayland::selection::primary_selection::{
    clear_primary_selection, current_primary_selection_userdata, request_primary_client_selection,
    set_primary_selection,
};
use smithay::xwayland::X11Surface;
use smithay::xwayland::xwm::XwmId;

use crate::session::{Session, SessionDriver};

pub fn allow_access(wayland: &crate::wayland::WaylandState, xwm: XwmId) -> bool {
    let Some(focused) = wayland.focused_window.as_ref() else {
        return false;
    };
    wayland.space.elements().any(|window| {
        window
            .wl_surface()
            .is_some_and(|surface| surface.as_ref() == focused)
            && window
                .x11_surface()
                .and_then(X11Surface::xwm_id)
                .is_some_and(|id| id == xwm)
    })
}

pub fn send<D: SessionDriver>(
    seat: &Seat<Session<D>>,
    selection: SelectionTarget,
    mime_type: String,
    fd: OwnedFd,
) {
    match selection {
        SelectionTarget::Clipboard => {
            if let Err(err) = request_data_device_client_selection(seat, mime_type, fd) {
                eventline::warn!("xwayland: clipboard transfer failed: {err}");
            }
        }
        SelectionTarget::Primary => {
            if let Err(err) = request_primary_client_selection(seat, mime_type, fd) {
                eventline::warn!("xwayland: primary selection transfer failed: {err}");
            }
        }
    }
}

pub fn set<D: SessionDriver>(
    display: &DisplayHandle,
    seat: &Seat<Session<D>>,
    selection: SelectionTarget,
    mime_types: Vec<String>,
) {
    match selection {
        SelectionTarget::Clipboard => set_data_device_selection(display, seat, mime_types, ()),
        SelectionTarget::Primary => set_primary_selection(display, seat, mime_types, ()),
    }
}

pub fn clear<D: SessionDriver>(
    display: &DisplayHandle,
    seat: &Seat<Session<D>>,
    selection: SelectionTarget,
) {
    match selection {
        SelectionTarget::Clipboard if current_data_device_selection_userdata(seat).is_some() => {
            clear_data_device_selection(display, seat)
        }
        SelectionTarget::Primary if current_primary_selection_userdata(seat).is_some() => {
            clear_primary_selection(display, seat)
        }
        _ => {}
    }
}

impl<D: SessionDriver> super::State<D> {
    /// Publish native client offers (including cleared selections) to X11.
    pub(crate) fn update_selection(
        &mut self,
        target: SelectionTarget,
        mime_types: Option<Vec<String>>,
    ) {
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.new_selection(target, mime_types) {
                eventline::warn!("xwayland: failed to update {target:?} selection: {err}");
            }
        }
    }

    /// Fulfil a Wayland request for a selection owned by an X11 client.
    pub(crate) fn request_selection(
        &mut self,
        target: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.send_selection(target, mime_type, fd) {
                eventline::warn!("xwayland: failed to request {target:?} selection: {err}");
            }
        }
    }
}
