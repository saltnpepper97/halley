//! Harness-free nested test process; all session and protocol code is production code.
#![allow(dead_code, unused_imports, unused_variables)]
#[path = "../../src/accessibility/mod.rs"]
mod accessibility;
#[path = "../../src/animation/mod.rs"]
mod animation;
#[path = "../../src/appearance.rs"]
mod appearance;
#[path = "../../src/autostart_log.rs"]
mod autostart_log;
#[path = "../../src/backend/mod.rs"]
mod backend;
#[path = "../../src/capture/mod.rs"]
mod capture;
#[path = "../../src/clusters/mod.rs"]
mod clusters;
#[path = "../../src/config.rs"]
mod config;
#[path = "../../src/cursor/mod.rs"]
mod cursor;
#[path = "../../src/frame_clock.rs"]
mod frame_clock;
#[path = "../../src/idle_service.rs"]
mod idle_service;
#[path = "../../src/input/mod.rs"]
mod input;
#[path = "../../src/ipc.rs"]
mod ipc;
#[path = "../../src/logging.rs"]
mod logging;
#[path = "../../src/nodes/mod.rs"]
mod nodes;
#[path = "../../src/presentation/mod.rs"]
mod presentation;
#[path = "../../src/render/mod.rs"]
mod render;
#[path = "../../src/session/mod.rs"]
mod session;
#[path = "../../src/shell/mod.rs"]
mod shell;
#[path = "../../src/titlebar.rs"]
mod titlebar;
#[path = "../../src/trail.rs"]
mod trail;
#[path = "../../src/wayland/dispatch.rs"]
mod upstream_protocols;
#[path = "../../src/wayland/mod.rs"]
mod wayland;
#[path = "../../src/window/mod.rs"]
mod window;
#[cfg(feature = "xwayland")]
#[path = "../../src/xwayland/mod.rs"]
mod xwayland;
#[cfg(not(feature = "xwayland"))]
#[path = "../../src/xwayland/disabled.rs"]
mod xwayland;

fn main() {
    // Plain workspace tests must never launch a desktop session.
    let Some(path) = std::env::var_os("HALLEY_TEST_WINIT_CONFIG") else {
        return;
    };
    #[cfg(feature = "winit")]
    {
        logging::init();
        session::winit::run(Some(path.into()));
    }
}
