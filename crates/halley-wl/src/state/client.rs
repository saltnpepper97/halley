use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::wayland::compositor::CompositorClientState;

#[derive(Debug)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientState {
    pub fn new() -> Self {
        Self {
            compositor_state: CompositorClientState::default(),
        }
    }
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
