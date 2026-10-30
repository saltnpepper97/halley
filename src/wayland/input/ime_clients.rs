//! Screen-lock policy for IME clients, using only public Wayland APIs.
use std::any::Any;
use std::collections::HashMap;

use smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_manager_v2::Request;
use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use smithay::reexports::wayland_server::backend::{ClientId, protocol::ProtocolError};

const LOCK_ERROR: &str =
    "Input methods are disconnected while the session is locked; reconnect after unlocking.";

#[derive(Default)]
pub struct ImeClients(HashMap<ClientId, Client>);

impl ImeClients {
    pub fn allow_request<I: Resource>(
        &mut self,
        locked: bool,
        client: &Client,
        resource: &I,
        request: &I::Request,
        _display: &DisplayHandle,
    ) -> bool
    where
        I::Request: 'static,
    {
        if locked && I::interface().name.starts_with("zwp_input_") {
            // Let libwayland destroy the client after dispatch returns. Killing
            // it here invalidates resources still in use by the request callback.
            resource.post_error(0u32, LOCK_ERROR);
            return false;
        }
        if matches!(
            (request as &dyn Any).downcast_ref::<Request>(),
            Some(Request::GetInputMethod { .. })
        ) {
            self.0.insert(client.id(), client.clone());
        }
        true
    }

    pub fn disconnect_all(&mut self, display: &DisplayHandle) {
        for client in self.0.drain().map(|(_, client)| client) {
            disconnect(&client, display);
        }
    }
}

fn disconnect(client: &Client, display: &DisplayHandle) {
    client.kill(
        display,
        ProtocolError {
            code: 0,
            object_id: 1,
            object_interface: "wl_display".into(),
            message: LOCK_ERROR.into(),
        },
    );
}
