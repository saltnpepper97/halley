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
use halley_config::{InputConfig, RuntimeTuning};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use smithay::reexports::drm::control::{self as drm_control, Device as DrmControlDevice};
use smithay::{
    backend::drm::DrmEvent,
    backend::input::{
        AbsolutePositionEvent, Axis, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent,
        PointerButtonEvent,
    },
    backend::libinput::LibinputInputBackend,
    backend::libinput::LibinputSessionInterface,
    backend::renderer::ImportDma,
    backend::renderer::gles::GlesRenderer,
    backend::session::libseat::LibSeatSession,
    backend::session::{Event as SessionEvent, Session},
    backend::udev::{all_gpus, primary_gpu},
    backend::winit::{self as smithay_winit, WinitEvent},
    reexports::input::Libinput,
    reexports::wayland_server::Display,
    utils::DeviceFd,
    wayland::socket::ListeningSocketSource,
};

use crate::animation::advance_node_move_anim;
use crate::bootstrap::{
    drain_ipc_commands, ensure_dbus_session_bus_address, ensure_host_display,
    ensure_xdg_runtime_dir, ensure_xwayland_satellite, init_logging, publish_outputs,
    refresh_portal_services_nonblocking, register_xwayland_request_channel, run_autostart_commands,
    shutdown_requested, sync_portal_activation_environment,
};
use crate::compositor::interaction::{ModState, PointerState};
use crate::compositor::root::Halley;
use crate::compositor::surface::current_surface_size_for_node;
use crate::input::pointer::resize::advance_resize_anim;
use crate::input::{BackendInputEventData, handle_backend_input_event};
use crate::protocol::wayland::ClientState;
use crate::spatial::node_in_active_area;

pub(crate) mod interface;
pub(crate) mod tty;
pub(crate) mod vblank_throttle;
pub(crate) mod winit;

pub(crate) const HOVER_PREVIEW_DWELL_MS: u64 = 1_500;

pub(crate) struct ResolvedXkbConfig {
    layout: String,
    variant: String,
    options: Option<String>,
}

impl ResolvedXkbConfig {
    pub(crate) fn from_input(input: &InputConfig) -> Self {
        Self {
            layout: input.keyboard.layout.clone(),
            variant: input.keyboard.variant.clone(),
            options: (!input.keyboard.options.is_empty()).then(|| input.keyboard.options.clone()),
        }
    }

    pub(crate) fn as_smithay(&self) -> smithay::input::keyboard::XkbConfig<'_> {
        smithay::input::keyboard::XkbConfig {
            rules: "",
            model: "",
            layout: self.layout.as_str(),
            variant: self.variant.as_str(),
            options: self.options.clone(),
        }
    }
}

pub(crate) fn initialize_seat_keyboard(st: &mut Halley) {
    let input = &st.runtime.tuning.input;
    let repeat_delay = input.repeat_delay;
    let repeat_rate = input.repeat_rate;
    let configured = ResolvedXkbConfig::from_input(input);

    if let Err(err) =
        st.platform
            .seat
            .add_keyboard(configured.as_smithay(), repeat_delay, repeat_rate)
    {
        warn!(
            "failed to initialize wl_seat keyboard with layout='{}' variant='{}' options='{}': {}; falling back to Smithay defaults",
            input.keyboard.layout, input.keyboard.variant, input.keyboard.options, err
        );
        if st
            .platform
            .seat
            .add_keyboard(Default::default(), repeat_delay, repeat_rate)
            .is_err()
        {
            warn!("failed to initialize wl_seat keyboard");
        }
    }
}

pub(crate) fn frame_interval_for_refresh_hz(refresh_hz: Option<f64>) -> Duration {
    let hz = refresh_hz.unwrap_or(60.0).clamp(30.0, 360.0);
    Duration::from_secs_f64(1.0 / hz)
}

pub(crate) fn resolve_hover_targets(
    st: &Halley,
    ps: &PointerState,
    now: Instant,
) -> (
    Option<halley_core::field::NodeId>,
    Option<halley_core::field::NodeId>,
) {
    resolve_hover_targets_with_scope(st, ps, now, None)
}

pub(crate) fn resolve_hover_targets_for_monitor(
    st: &Halley,
    ps: &PointerState,
    now: Instant,
    monitor: &str,
) -> (
    Option<halley_core::field::NodeId>,
    Option<halley_core::field::NodeId>,
) {
    resolve_hover_targets_with_scope(st, ps, now, Some(monitor))
}

fn resolve_hover_targets_with_scope(
    st: &Halley,
    ps: &PointerState,
    now: Instant,
    monitor: Option<&str>,
) -> (
    Option<halley_core::field::NodeId>,
    Option<halley_core::field::NodeId>,
) {
    if bloom_pull_preview_active_for_scope(st, ps, monitor) {
        return (None, None);
    }

    let hover_blocked = ps.preview_block_until.is_some_and(|t| now < t);
    let overlay_hover = overlay_hover_target_for_scope(st, monitor);
    let hovered = if hover_blocked {
        None
    } else {
        overlay_hover.or_else(|| fallback_hover_node_for_scope(st, ps, monitor))
    };
    let preview_ready = hovered.is_some_and(|id| {
        ps.hover_started_at
            .is_some_and(|at| now.duration_since(at).as_millis() as u64 >= HOVER_PREVIEW_DWELL_MS)
            && (overlay_hover == Some(id) || hover_preview_node_is_eligible(st, id, monitor))
    });
    if preview_ready {
        (None, hovered)
    } else {
        (hovered, None)
    }
}

fn bloom_pull_preview_active_for_scope(
    st: &Halley,
    ps: &PointerState,
    monitor: Option<&str>,
) -> bool {
    ps.bloom_drag.is_some()
        || match monitor {
            Some(monitor) => st
                .input
                .interaction_state
                .bloom_pull_preview
                .as_ref()
                .is_some_and(|preview| preview.monitor == monitor),
            None => st.input.interaction_state.bloom_pull_preview.is_some(),
        }
}

fn overlay_hover_target_for_scope(
    st: &Halley,
    monitor: Option<&str>,
) -> Option<halley_core::field::NodeId> {
    st.input
        .interaction_state
        .overlay_hover_target
        .as_ref()
        .filter(|target| monitor.is_none_or(|monitor| target.monitor == monitor))
        .map(|target| target.node_id)
}

fn fallback_hover_node_for_scope(
    st: &Halley,
    ps: &PointerState,
    monitor: Option<&str>,
) -> Option<halley_core::field::NodeId> {
    match monitor {
        Some(monitor) => ps
            .hover_node
            .filter(|id| hover_node_matches_monitor(st, *id, monitor)),
        None => ps.hover_node,
    }
}

fn hover_preview_node_is_eligible(
    st: &Halley,
    id: halley_core::field::NodeId,
    monitor: Option<&str>,
) -> bool {
    match monitor {
        Some(monitor) => hover_node_matches_monitor(st, id, monitor),
        None => node_in_active_area(st, id),
    }
}

fn hover_node_matches_monitor(st: &Halley, id: halley_core::field::NodeId, monitor: &str) -> bool {
    st.model
        .monitor_state
        .node_monitor
        .get(&id)
        .is_none_or(|node_monitor| node_monitor == monitor)
}
