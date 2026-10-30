use smithay::reexports::wayland_server::backend::protocol::ProtocolError;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::wayland::compositor::{Barrier, add_blocker};

// wl_display is handled internally by wayland-server, which does not expose
// its generated Error enum. This is the core protocol's server-internal error.
const WL_DISPLAY_ERROR_IMPLEMENTATION: u32 = 3;

/// An unmonitorable buffer must not reach rendering or leave a live client
/// waiting forever. Keep its transaction blocked until the fatal server error
/// tears down the connection and its pending surface state.
pub fn reject_commit(
    surface: &WlSurface,
    message: String,
    defer_error: impl FnOnce(Client, ProtocolError),
) {
    add_blocker(surface, Barrier::new(false));
    if let Some(client) = surface.client() {
        // libwayland must finish dispatching this commit before its client and
        // resources are destroyed. The backend schedules this error for idle.
        defer_error(
            client,
            ProtocolError {
                code: WL_DISPLAY_ERROR_IMPLEMENTATION,
                object_id: 1,
                object_interface: "wl_display".into(),
                message,
            },
        );
    }
}
