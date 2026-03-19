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
    utils::{DeviceFd, Transform},
    wayland::socket::ListeningSocketSource,
};

use crate::activity::VisualState;
use crate::input::{BackendInputEventData, advance_node_move_anim, handle_backend_input_event};
use crate::interaction::types::{ModState, PointerState};
use crate::render::draw_debug_frame_to_target;
use crate::run::{
    RuntimeIpcCommand, drain_ipc_commands, ensure_dbus_session_bus_address,
    ensure_host_display, ensure_xdg_runtime_dir, ensure_xwayland_satellite, init_logging,
    publish_outputs, register_xwayland_request_channel, run_autostart_commands,
    shutdown_requested,
};
use crate::state::{ClientState, HalleyWlState};
use crate::surface::current_surface_size_for_node;

pub(crate) mod interface;
pub(crate) mod tty;
pub(crate) mod tty_drm;
pub(crate) mod tty_input;
pub(crate) mod winit;
