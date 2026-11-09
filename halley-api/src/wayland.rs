//! Optional surface-bound Wayland interfaces, separate from command-socket IPC.
//!
//! The host owns its connection, dispatch, and surface lifecycle. Bind the
//! optional dismissal manager through the host's registry and watch an owned
//! layer-shell surface. Absence of the global is normal on older compositors.

/// Client bindings for `halley-surface-dismissal-v1`.
#[cfg(feature = "wayland-client")]
pub mod client {
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/halley-surface-dismissal-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocols/halley-surface-dismissal-v1.xml");
}

/// Server bindings for `halley-surface-dismissal-v1`.
#[cfg(feature = "wayland-server")]
pub mod server {
    use wayland_server;
    use wayland_server::protocol::*;
    pub mod __interfaces {
        use wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/halley-surface-dismissal-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_server_code!("protocols/halley-surface-dismissal-v1.xml");
}
