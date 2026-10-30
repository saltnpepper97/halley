//! No-op window tracing for builds without XWayland.

use std::fmt;

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use super::{Session, SessionDriver};

pub(super) struct WindowTrace;

impl WindowTrace {
    pub(super) fn from_env() -> Self {
        Self
    }
}

pub(crate) fn surface_event<D: SessionDriver>(
    _session: &mut Session<D>,
    _surface: &WlSurface,
    _event: &'static str,
    _details: fmt::Arguments<'_>,
) {
}

pub(crate) fn surface_sampled_event<D: SessionDriver>(
    _session: &mut Session<D>,
    _surface: &WlSurface,
    _event: &'static str,
    _details: fmt::Arguments<'_>,
) {
}

pub(crate) fn snapshot<D: SessionDriver>(_session: &mut Session<D>) {}
