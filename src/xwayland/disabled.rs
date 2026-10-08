//! Feature-neutral XWayland facade used by builds without native X11 support.

#[path = "focus.rs"]
mod focus;

use std::ffi::OsString;
use std::marker::PhantomData;

use calloop::LoopHandle;
use smithay::desktop::{Space, Window};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DisplayHandle};
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::compositor::CompositorClientState;

use crate::session::{Session, SessionDriver};

pub use focus::KeyboardFocusTarget;

pub struct State<D: SessionDriver> {
    _driver: PhantomData<D>,
}

impl<D: SessionDriver> State<D> {
    pub fn new(_display: &DisplayHandle, _loop_handle: LoopHandle<'static, Session<D>>) -> Self {
        Self {
            _driver: PhantomData,
        }
    }

    pub(crate) fn arm_speculative_close_timeout(
        &self,
        _surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
    }

    pub fn display_name(&self) -> Option<OsString> {
        None
    }

    pub fn raise_window(&mut self, _window: &Window) {}

    pub fn sync_active_window(&self, _window: Option<u32>, _has_keyboard_focus: bool) {}

    pub fn focus_globally_active_window(&self, _window: Option<u32>) {}

    pub fn sync_frame_extents(
        &self,
        _window: &Window,
        _decorations: &halley_config::Decorations,
        _font: &halley_config::Font,
    ) {
    }

    pub fn sync_all_frame_extents(
        &self,
        _space: &Space<Window>,
        _decorations: &halley_config::Decorations,
        _font: &halley_config::Font,
    ) {
    }

    pub fn sync_desktop_geometry(&self, _space: &Space<Window>) {}

    pub fn sync_stacking_order(&mut self, _space: &Space<Window>) {}

    pub fn configure_key_repeat(&self, _delay: i32, _rate: i32) {}

    pub fn set_window_iconic(&mut self, _window: &Window) {}

    pub fn set_window_normal(&mut self, _window: &Window) {}

    pub(crate) fn client_geometry_guarded_for_window(
        &self,
        _window: &Window,
        _now: std::time::Duration,
    ) -> bool {
        false
    }
}

pub fn start<D>(
    _loop_handle: &LoopHandle<'static, Session<D>>,
    session: &mut Session<D>,
    _publish_environment: bool,
) -> Result<(), Box<dyn std::error::Error>>
where
    D: SessionDriver,
{
    session.run_autostart_once();
    Ok(())
}

pub fn reconfigure_fullscreen(_windows: Vec<(Window, Rectangle<i32, Logical>)>) {}

pub fn close_window(_window: &Window) {}

pub fn configure_window<D: SessionDriver>(
    _session: &mut Session<D>,
    _window: &Window,
    _geometry: Rectangle<i32, Logical>,
) {
}

pub fn sync_position<D: SessionDriver>(_session: &Session<D>, _window: &Window) -> bool {
    false
}

pub fn sync_positions<D: SessionDriver>(_session: &Session<D>) -> bool {
    false
}

pub fn sync_stacking_order<D: SessionDriver>(_session: &mut Session<D>) {}

pub fn constrain_window_size(
    _window: &Window,
    requested: Size<i32, Logical>,
) -> Size<i32, Logical> {
    requested
}

pub fn set_window_fullscreen<D: SessionDriver>(
    _session: &mut Session<D>,
    _window: &Window,
    _fullscreen: bool,
) {
}

pub fn restore_maximized_window<D: SessionDriver>(_session: &mut Session<D>, _window: &Window) {}

pub fn handle_commit<D: SessionDriver>(
    _session: &mut Session<D>,
    _surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) {
}

pub fn is_override_redirect(_window: &Window) -> bool {
    false
}

pub fn server_geometry(_window: &Window) -> Option<(u32, Rectangle<i32, Logical>)> {
    None
}

pub fn is_x11(_window: &Window) -> bool {
    false
}

pub(crate) fn is_steam_client_close_hit(_window: &Window, _local: Point<f64, Logical>) -> bool {
    false
}

pub fn is_fullscreen(_window: &Window) -> bool {
    false
}

pub fn uses_server_decorations(_window: &Window) -> bool {
    false
}

pub fn can_maximize(_window: &Window) -> bool {
    true
}

pub fn window_for_xid(_space: &Space<Window>, _xid: u32) -> Option<Window> {
    None
}

pub fn pointer_constraint_proxy_authority(
    _space: &Space<Window>,
    _surface: &WlSurface,
) -> Option<WlSurface> {
    None
}

pub(crate) fn parent_window_from<'a>(
    _windows: impl Iterator<Item = &'a Window>,
    _window: &Window,
) -> Option<Window> {
    None
}

pub fn metadata(_window: &Window) -> Option<(String, String)> {
    None
}

pub fn set_hidden(_window: &Window, _hidden: bool) {}

pub fn set_maximized(_window: &Window, _maximized: bool) {}

pub fn compositor_client_state(_client: &Client) -> Option<&CompositorClientState> {
    None
}
