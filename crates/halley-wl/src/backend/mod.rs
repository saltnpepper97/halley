use std::cell::RefCell;
use std::env;
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use calloop::EventLoop;
use calloop::timer::{TimeoutAction, Timer};

use eventline::{debug, info, scope, warn};
use halley_config::RuntimeTuning;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use smithay::reexports::drm::control::{self as drm_control, Device as DrmControlDevice};
use smithay::{
    backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
    backend::allocator::{Format, Fourcc},
    backend::drm::DrmEvent,
    backend::drm::GbmBufferedSurface,
    backend::drm::{DrmDevice, DrmDeviceFd},
    backend::egl::{EGLContext, EGLDisplay},
    backend::input::{
        AbsolutePositionEvent, Axis, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent,
        PointerButtonEvent,
    },
    backend::libinput::LibinputInputBackend,
    backend::libinput::LibinputSessionInterface,
    backend::renderer::gles::GlesRenderer,
    backend::renderer::{Bind, ImportDma},
    backend::session::libseat::LibSeatSession,
    backend::session::{Event as SessionEvent, Session},
    backend::udev::{all_gpus, primary_gpu},
    backend::winit::{self as smithay_winit, WinitEvent},
    reexports::input::Libinput,
    reexports::wayland_server::Display,
    utils::DeviceFd,
    wayland::socket::ListeningSocketSource,
};

use crate::activity::VisualState;
use crate::animation::advance_node_move_anim;
use crate::input::{BackendInputEventData, handle_backend_input_event};
use crate::interaction::types::{ModState, PointerState};
use crate::render::draw_debug_frame_to_target;
use crate::run::{
    RuntimeIpcCommand, drain_ipc_commands, ensure_dbus_session_bus_address, ensure_host_display,
    ensure_xdg_runtime_dir, ensure_xwayland_satellite, init_logging, publish_outputs,
    register_xwayland_request_channel, run_autostart_commands, shutdown_requested,
};
use crate::spatial::node_in_active_area;
use crate::state::{ClientState, HalleyWlState};
use crate::surface::current_surface_size_for_node;

pub(crate) mod interface;
pub(crate) mod tty;
pub(crate) mod tty_drm;
pub(crate) mod tty_input;
pub(crate) mod winit;

pub(crate) const HOVER_PREVIEW_DWELL_MS: u64 = 1_500;

pub(crate) fn frame_interval_for_refresh_hz(refresh_hz: Option<f64>) -> Duration {
    let hz = refresh_hz.unwrap_or(60.0).clamp(30.0, 360.0);
    Duration::from_secs_f64(1.0 / hz)
}

pub(crate) fn resolve_hover_targets(
    st: &HalleyWlState,
    ps: &PointerState,
    now: Instant,
) -> (
    Option<halley_core::field::NodeId>,
    Option<halley_core::field::NodeId>,
) {
    let hover_blocked = ps.preview_block_until.is_some_and(|t| now < t);
    let hovered = if hover_blocked { None } else { ps.hover_node };
    let preview_ready = hovered.is_some_and(|id| {
        node_in_active_area(st, id)
            && ps.hover_started_at.is_some_and(|at| {
                now.duration_since(at).as_millis() as u64 >= HOVER_PREVIEW_DWELL_MS
            })
    });
    if preview_ready {
        (None, hovered)
    } else {
        (hovered, None)
    }
}
