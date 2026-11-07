//! Synthetic keyboard input is unavailable while the session is locked.
use crate::input::virtual_keyboard::{self, Host};
use crate::session::{Session, SessionDriver};
use smithay::backend::input::InputEvent;
use smithay::input::Seat;
use smithay::reexports::wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_v1::{Request, ZwpVirtualKeyboardV1};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, backend::ClientId};
use smithay::wayland::virtual_keyboard::{VirtualKeyboardBackend, VirtualKeyboardHandler, VirtualKeyboardUserData};

impl<D: SessionDriver> Host for Session<D> {
    fn virtual_keyboard_seat(&self) -> Seat<Self> {
        self.seat.clone()
    }
    fn virtual_keyboard_locked(&self) -> bool {
        self.session_lock.active()
    }
}

impl<D: SessionDriver> VirtualKeyboardHandler for Session<D> {
    fn process_virtual_keyboard_event(&mut self, event: InputEvent<VirtualKeyboardBackend>) {
        virtual_keyboard::handle_event(self, event);
    }
}

impl<D: SessionDriver> Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData> for Session<D> {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ZwpVirtualKeyboardV1,
        request: Request,
        data: &VirtualKeyboardUserData,
        display: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        virtual_keyboard::request(data, state, client, resource, request, display, init);
    }
    fn destroyed(
        state: &mut Self,
        client: ClientId,
        resource: &ZwpVirtualKeyboardV1,
        data: &VirtualKeyboardUserData,
    ) {
        smithay::wayland::Dispatch2::destroyed(data, state, client, resource);
    }
}
