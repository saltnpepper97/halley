use std::collections::HashMap;
use std::time::{Duration, Instant};

mod frame;
mod sleep;
mod vt;

use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction};
use smithay::backend::drm::{DrmEvent, DrmEventMetadata, DrmEventTime};
use smithay::backend::input::KeyboardKeyEvent;
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::session::Event as SessionEvent;
use smithay::backend::session::Session;
use smithay::input::{Seat, SeatState};
use smithay::output::Output;
use smithay::reexports::drm::control::crtc;
use smithay::reexports::input::Libinput;
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::CompositorHandler;
use smithay::wayland::drm_syncobj::DrmSyncPointSource;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::Layer;

use crate::backend::tty::TtyBackend;
use crate::backend::{RenderOutcome, RenderStatus, Renderable};
use crate::cursor::CursorManager;
use crate::input::Keyboard;
use crate::input::keybinds::BackendKind;
use crate::input::pointer::Pointer;
use crate::render::{
    CLEAR_COLOR, CursorContext, DesktopContext, FrameContext, OverlayContext, RenderRequest,
    VisualContext,
};
use crate::wayland;

use self::frame::{EstimatedVblankTimer, OutputFrameState, VblankAction};
use super::RenderDriver as _;

struct TtyDriver {
    backend: TtyBackend,
    physical_input: crate::input::devices::PhysicalInputDevices,
    loop_handle: LoopHandle<'static, TtyApp>,
    loop_signal: LoopSignal,
    output_frames: HashMap<Output, OutputFrameState>,
    pause_reasons: PauseReasons,
    pending_output_config: Option<Vec<halley_config::OutputConfig>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PauseReasons {
    session: bool,
    system_sleep: bool,
}

impl PauseReasons {
    fn any(self) -> bool {
        self.session || self.system_sleep
    }
}

impl crate::ipc::OutputInfoSource for TtyDriver {
    fn output_info(&self) -> Vec<halley_ipc::OutputInfo> {
        crate::ipc::OutputInfoSource::output_info(&self.backend)
    }
}

impl super::RenderDriver for TtyDriver {
    fn dmabuf_capabilities(&mut self) -> crate::backend::dmabuf::DmabufCapabilities {
        self.backend.dmabuf_capabilities()
    }

    fn import_dmabuf(&mut self, dmabuf: &smithay::backend::allocator::dmabuf::Dmabuf) -> bool {
        self.backend.import_dmabuf(dmabuf)
    }

    fn dmabuf_feedback(
        &self,
        output: &Output,
    ) -> Option<&crate::backend::dmabuf::SurfaceDmabufFeedback> {
        self.backend.dmabuf_feedback(output)
    }

    fn request_redraw(&mut self, output: Option<&Output>) {
        if let Some(output) = output {
            if self.backend.output_dpms_enabled(output)
                && let Some(state) = self.output_frames.get_mut(output)
            {
                state.queue_redraw();
            }
            return;
        }
        for (output, state) in &mut self.output_frames {
            if self.backend.output_dpms_enabled(output) {
                state.queue_redraw();
            }
        }
    }

    fn with_renderer<T>(
        &mut self,
        f: impl FnOnce(&mut smithay::backend::renderer::gles::GlesRenderer) -> T,
    ) -> T {
        f(self.backend.renderer())
    }

    fn schedule_render_completion(
        &mut self,
        sync: smithay::backend::renderer::sync::SyncPoint,
        completion: Box<dyn FnOnce() + 'static>,
    ) -> Result<(), String> {
        let Some(fence) = sync.export() else {
            completion();
            return Ok(());
        };
        let mut completion = Some(completion);
        self.loop_handle
            .insert_source(
                Generic::new(fence, Interest::READ, Mode::OneShot),
                move |_, _, _| {
                    if let Some(completion) = completion.take() {
                        completion();
                    }
                    Ok(PostAction::Remove)
                },
            )
            .map(|_| ())
            .map_err(|err| format!("failed to watch render fence: {err}"))
    }

    fn register_drm_syncobj_source(&mut self, client: Client, source: DrmSyncPointSource) -> bool {
        self.loop_handle
            .insert_source(source, move |_, _, app| {
                let dh = app.wayland.display_handle.clone();
                app.client_compositor_state(&client)
                    .blocker_cleared(app, &dh);
                Ok(())
            })
            .map(|_| true)
            .unwrap_or_else(|err| {
                eventline::warn!("explicit sync: failed to register acquire-point source: {err}");
                false
            })
    }
}

impl super::OutputDriver for TtyDriver {
    fn primary_output(&self) -> &Output {
        self.backend.primary_output()
    }

    fn frame_callback_sequence(&self, output: &Output) -> u32 {
        self.output_frames
            .get(output)
            .map(OutputFrameState::frame_callback_sequence)
            .unwrap_or_default()
    }

    fn output_states(&self) -> Vec<super::output::OutputState> {
        self.backend.output_states()
    }

    fn test_output_configuration(
        &mut self,
        configuration: &[super::output::OutputConfiguration],
    ) -> Result<(), String> {
        self.backend.test_output_configuration(configuration)
    }

    fn apply_output_configuration(
        &mut self,
        configuration: &[super::output::OutputConfiguration],
    ) -> Result<Vec<super::output::OutputChange>, String> {
        let changes = self
            .backend
            .apply_runtime_output_configuration(configuration)?;
        for change in &changes {
            if change.before.mode != change.after.mode
                && let Some(state) = self.output_frames.get_mut(&change.after.output)
            {
                state.replace_clock(
                    self.backend
                        .refresh_interval_for_output(&change.after.output),
                );
            }
            if let Some(state) = self.output_frames.get_mut(&change.after.output) {
                state.set_vrr(self.backend.output_vrr_active(&change.after.output));
            }
        }
        Ok(changes)
    }

    fn gamma_size(&self, output: &Output) -> Result<u32, String> {
        self.backend.gamma_size(output)
    }

    fn set_gamma(&mut self, output: &Output, ramp: Option<Vec<u16>>) -> Result<(), String> {
        self.backend.set_gamma(output, ramp)
    }

    fn apply_dpms(
        &mut self,
        command: halley_ipc::DpmsCommand,
        output: Option<&str>,
    ) -> Result<(), String> {
        self.apply_dpms_command(command, output)
    }

    fn output_requires_lock_frame(&self, output: &Output) -> bool {
        self.backend.output_dpms_enabled(output)
    }
}

impl super::SessionDriver for TtyDriver {
    const BACKEND_KIND: BackendKind = BackendKind::Tty;

    fn stop(&mut self) {
        self.loop_signal.stop();
    }
}

type TtyApp = super::Session<TtyDriver>;

impl TtyDriver {
    fn apply_dpms_command(
        &mut self,
        command: halley_ipc::DpmsCommand,
        output: Option<&str>,
    ) -> Result<(), String> {
        let applied = self.backend.apply_dpms(command, output)?;
        let now = crate::frame_clock::monotonic_now();
        for change in applied.changes {
            let Some(state) = self.output_frames.get_mut(&change.output) else {
                continue;
            };
            if change.enabled {
                state.resume(now);
            } else {
                for token in state.suspend(now) {
                    self.loop_handle.remove(token);
                }
            }
        }
        match applied.error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn wake_dpms_on_input(&mut self, target: Option<&Output>) {
        let output = if !self.backend.any_output_dpms_enabled() {
            None
        } else {
            let Some(target) = target else {
                return;
            };
            if self.backend.output_dpms_enabled(target) {
                return;
            }
            Some(target.name())
        };
        if let Err(err) = self.apply_dpms_command(halley_ipc::DpmsCommand::On, output.as_deref()) {
            eventline::warn!("tty dpms: failed to wake on input: {err}");
        }
    }
}

/// Runs the real-hardware (DRM/KMS) session - takes over the seat and a
/// free VT. Returns (rather than panicking) if `TtyBackend::new()` fails,
/// since that's expected when nested under a host compositor that already
/// holds exclusive session control.
pub fn run(explicit_config_path: Option<std::path::PathBuf>) {
    let initial = crate::config::load_initial(explicit_config_path);
    let config_path = initial.path;
    let runtime_config = initial.config;
    let (backend, session_notifier, drm_notifier) = match TtyBackend::new(&runtime_config.outputs) {
        Ok(parts) => parts,
        Err(err) => {
            eventline::error!("TtyBackend::new() failed: {err}");
            return;
        }
    };
    eventline::info!("TtyBackend constructed successfully");
    let initially_paused = !backend.session().is_active();

    let mut libinput_context =
        Libinput::new_with_udev::<LibinputSessionInterface<_>>(backend.session().into());
    libinput_context
        .udev_assign_seat(&backend.session().seat())
        .expect("failed to assign udev seat for libinput");
    if initially_paused {
        eventline::debug!("tty input: session starts inactive; suspending libinput");
        libinput_context.suspend();
    }
    let mut libinput_for_session = libinput_context.clone();
    let libinput_backend = LibinputInputBackend::new(libinput_context);

    let mut event_loop: EventLoop<TtyApp> =
        EventLoop::try_new().expect("failed to create event loop");
    let loop_signal = event_loop.get_signal();
    let loop_handle = event_loop.handle();

    let display: Display<TtyApp> = Display::new().expect("failed to create wayland display");
    let dh = display.handle();

    let mut seat_state = SeatState::new();
    let mut seat: Seat<TtyApp> = seat_state.new_wl_seat(&dh, "seat0");
    let applied_keyboard = crate::input::config::add_keyboard(&mut seat, &runtime_config.input)
        .expect("failed to advertise keyboard capability on the wl_seat");
    seat.add_pointer();

    let outputs: Vec<_> = backend.outputs().cloned().collect();
    let now = crate::frame_clock::monotonic_now();

    // Smithay's `Output` is its stable identity handle and is the key used
    // throughout its own per-output state maps despite containing an Arc.
    #[allow(clippy::mutable_key_type)]
    let output_frames = outputs
        .iter()
        .cloned()
        .map(|output| {
            let interval = backend.refresh_interval_for_output(&output);
            let vrr_active = backend.output_vrr_active(&output);
            let mut state = OutputFrameState::new(interval);
            state.set_vrr(vrr_active);
            if initially_paused {
                drop(state.suspend(now));
            }
            (output, state)
        })
        .collect();

    let mut driver = TtyDriver {
        backend,
        physical_input: crate::input::devices::PhysicalInputDevices::default(),
        loop_handle: loop_handle.clone(),
        loop_signal,
        output_frames,
        pause_reasons: PauseReasons {
            session: initially_paused,
            system_sleep: false,
        },
        pending_output_config: None,
    };
    let mut wayland = TtyApp::create_wayland_state(dh.clone(), &mut driver);
    for output in &outputs {
        wayland.ensure_output_global::<TtyApp>(output);
    }
    let idle_notifier_state =
        smithay::wayland::idle_notify::IdleNotifierState::new(&dh, loop_handle.clone());
    let presentation_state = smithay::wayland::presentation::PresentationState::new::<TtyApp>(
        &dh,
        smithay::utils::Clock::<smithay::utils::Monotonic>::new().id() as u32,
    );
    let session_lock = crate::wayland::session_lock::State::new::<TtyDriver>(&dh);
    let drm_syncobj_state = if smithay::wayland::drm_syncobj::supports_syncobj_eventfd(
        driver.backend.drm_device_fd(),
    ) {
        eventline::info!("explicit sync: linux-drm-syncobj-v1 available on the primary DRM device");
        Some(
            smithay::wayland::drm_syncobj::DrmSyncobjState::new::<TtyApp>(
                &dh,
                driver.backend.drm_device_fd().clone(),
            ),
        )
    } else {
        eventline::info!(
            "explicit sync: primary DRM device lacks syncobj eventfd support; protocol disabled"
        );
        None
    };
    let xwayland = crate::xwayland::State::<TtyDriver>::new(&dh, loop_handle.clone());
    let keyboard_monitor = match crate::accessibility::KeyboardMonitorService::start() {
        Ok(service) => Some(service),
        Err(err) => {
            eventline::warn!("accessibility: keyboard monitor unavailable: {err}");
            None
        }
    };
    let mut applied_input = runtime_config.input.clone();
    applied_input.keyboard = applied_keyboard;
    let startup_cluster_declarations = runtime_config.autostart.clusters.clone();
    let startup_cluster_default_layout = runtime_config.clusters.default_layout;
    let launch_environment = super::environment::LaunchEnvironment::new(&runtime_config.env);
    let launch_path = launch_environment.path();
    let system_color_scheme = crate::appearance::current_color_scheme();
    let mut app = TtyApp {
        driver,
        keyboard: Keyboard::from_config(
            &runtime_config.keybinds,
            BackendKind::Tty,
            launch_path.as_deref(),
        ),
        key_repeat: super::input::repeat::Policy::new(loop_handle.clone()),
        launch_environment,
        autostart: super::autostart::Autostart::enabled(),
        startup_clusters: super::startup_clusters::StartupClusters::default(),
        pointer: Pointer::new((100.0, 100.0)),
        cursor: CursorManager::new(&runtime_config.cursor),
        cursor_policy: super::cursor::Policy::new(&runtime_config.cursor, loop_handle.clone()),
        publish_session_environment: true,
        wayland,
        seat_state,
        seat,
        idle_notifier_state,
        presentation_state,
        drm_syncobj_state,
        session_lock,
        start_time: Instant::now(),
        wayland_display: None,
        config_path: config_path.clone(),
        config_watcher: None,
        startup_config_diagnostic: initial.diagnostic,
        shell: crate::shell::state::ShellState::new(&runtime_config),
        settings: super::RuntimeSettings::new_with_color_scheme(
            &runtime_config,
            applied_input,
            system_color_scheme,
        ),
        nodes: crate::nodes::NodesState::new_with_color_scheme(
            &runtime_config,
            system_color_scheme,
        ),
        trail: crate::trail::TrailState::new(runtime_config.trail),
        clusters: crate::clusters::ClusterSystem::new(
            runtime_config.clusters,
            runtime_config.animations.cluster,
        ),
        api_subscriptions: crate::ipc::ApiSubscriptions::default(),
        window_rules: crate::window::rules::WindowRulesState::new(
            runtime_config.window_rules.clone(),
        ),
        presentation_close_size_recovery:
            crate::window::recovery::PresentationCloseSizeRecovery::default(),
        cameras: crate::presentation::camera::OutputCameras::default(),
        capture: crate::capture::CaptureState::default(),
        pending_captures: std::collections::HashMap::new(),
        screenshot_encoder: None,
        screencast: crate::capture::screencast::ScreencastState::default(),
        interactions: super::InteractionState::default(),
        touch: super::touch::TouchState::default(),
        gestures: super::gesture::GestureState::default(),
        window_trace: super::trace::WindowTrace::from_env(),
        keyboard_monitor,
        opening_origins: super::opening::OpeningOrigins::default(),
        window_animations: crate::animation::WindowAnimations::new(
            runtime_config.animations.clone(),
        ),
        render: crate::render::resources::RenderState::new(
            runtime_config.animations.clone(),
            &runtime_config.font,
        ),
        fullscreen: crate::wayland::fullscreen::FullscreenManager::new(
            runtime_config.animations.clone(),
        ),
        maximize: crate::presentation::maximize::FieldMaximizeManager::new(
            runtime_config.field,
            runtime_config.animations.clone(),
        ),
        xwayland,
    };
    for output in outputs {
        app.wayland
            .space
            .map_output(&output, output.current_location());
        let geometry = app
            .wayland
            .space
            .output_geometry(&output)
            .expect("mapped tty output has geometry");
        app.cameras
            .insert(output.name(), geometry.size.to_physical(1));
    }
    app.initialize_startup_clusters(
        &startup_cluster_declarations,
        startup_cluster_default_layout,
        true,
    );
    app.initialize_config_notification();

    let socket_name = super::protocol::init_wayland_listener(display, &mut event_loop);
    app.wayland_display = Some(socket_name.clone());
    eventline::info!("wayland socket ready, WAYLAND_DISPLAY={socket_name:?}");
    app.arm_autostart_once(&socket_name, runtime_config.autostart.once.clone());
    super::environment::activate_session(&socket_name, runtime_config.cursor.size);
    if let Err(err) = crate::xwayland::start(&event_loop.handle(), &mut app, true) {
        eventline::warn!("xwayland: unavailable: {err}");
        app.run_autostart_once();
    }

    if let Err(err) =
        crate::ipc::init_ipc_listener(&event_loop.handle(), |app: &mut TtyApp, request| {
            crate::ipc::handle_request(app, request);
        })
    {
        eventline::error!("ipc: failed to start listener: {err}");
    }

    match crate::capture::encoder::ScreenshotEncoder::spawn(
        &event_loop.handle(),
        |app: &mut TtyApp, done| crate::capture::finish_encode(app, done),
    ) {
        Ok(encoder) => app.screenshot_encoder = Some(encoder),
        Err(err) => eventline::error!("screenshot: failed to start encoder: {err}"),
    }
    super::environment::notify_ready();
    if let Some(path) = config_path {
        match crate::config::watch(&event_loop.handle(), path, apply_runtime_config) {
            Ok(watcher) => app.config_watcher = Some(watcher),
            Err(err) => eventline::warn!("config: failed to start watcher: {err}"),
        }
    }
    if let Err(err) = crate::appearance::watch(&event_loop.handle(), |app: &mut TtyApp, scheme| {
        app.apply_system_color_scheme(scheme);
    }) {
        eventline::warn!("appearance: failed to start system colour watcher: {err}");
    }
    if let Err(err) = super::install_node_decay_timer(&event_loop.handle()) {
        eventline::warn!("nodes: failed to start decay timer: {err}");
    }
    if let Err(err) = sleep::install(&event_loop.handle()) {
        eventline::warn!("system sleep: failed to install monitor: {err}");
    }
    if let Err(err) = super::install_overlay_timer(&event_loop.handle()) {
        eventline::warn!("overlays: failed to start lifecycle timer: {err}");
    }
    if let Err(err) = super::install_frame_callback_fallback_timer(&event_loop.handle()) {
        eventline::warn!("frame-callback: failed to start fallback timer: {err}");
    }

    // Queue every output's first frame through the same state machine used
    // for all later redraws.
    queue_redraw(&mut app);
    redraw_queued_outputs(&mut app, &loop_handle);

    event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, app| {
            if let smithay::backend::input::InputEvent::Keyboard { event } = &event {
                let modifiers = app
                    .seat
                    .get_keyboard()
                    .expect("keyboard capability added at seat setup")
                    .modifier_state();
                if let Some(vt) = vt::target_from_keycode(
                    event.key_code().raw(),
                    event.state() == smithay::backend::input::KeyState::Pressed,
                    modifiers,
                ) {
                    // The switch may prevent every held key's release from
                    // reaching this VT. Do not retain compositor release-pair
                    // bookkeeping across that boundary.
                    app.interactions.suppressed_keys.clear();
                    app.key_repeat.cancel();
                    match app.driver.backend.change_vt(vt) {
                        Ok(()) => eventline::debug!("tty input: requested VT switch to {vt}"),
                        Err(err) => {
                            eventline::warn!("tty input: failed to switch to VT {vt}: {err}")
                        }
                    }
                    return;
                }
            }

            let wake_before = match &event {
                smithay::backend::input::InputEvent::Keyboard { .. } => {
                    crate::wayland::focus::selected_output(&app.wayland).cloned()
                }
                smithay::backend::input::InputEvent::PointerButton { .. }
                | smithay::backend::input::InputEvent::PointerAxis { .. } => output_at_pointer(app),
                _ => None,
            };
            if matches!(
                &event,
                smithay::backend::input::InputEvent::Keyboard { .. }
                    | smithay::backend::input::InputEvent::PointerButton { .. }
                    | smithay::backend::input::InputEvent::PointerAxis { .. }
            ) {
                app.driver.wake_dpms_on_input(wake_before.as_ref());
            }
            match &event {
                smithay::backend::input::InputEvent::DeviceAdded { device } => {
                    let first_touch = app
                        .driver
                        .physical_input
                        .added(device.clone(), &app.settings.input);
                    if first_touch && app.seat.get_touch().is_none() {
                        app.seat.add_touch();
                    }
                }
                smithay::backend::input::InputEvent::DeviceRemoved { device }
                    if app.driver.physical_input.removed(device) =>
                {
                    super::touch::cancel_all(app);
                    app.seat.remove_touch();
                }
                smithay::backend::input::InputEvent::DeviceRemoved { .. } => {}
                _ => {}
            }
            super::input::handle(app, &event, &socket_name);
            if matches!(
                &event,
                smithay::backend::input::InputEvent::PointerMotion { .. }
                    | smithay::backend::input::InputEvent::PointerMotionAbsolute { .. }
            ) {
                let target = output_at_pointer(app);
                app.driver.wake_dpms_on_input(target.as_ref());
            }
        })
        .expect("failed to insert libinput source");

    event_loop
        .handle()
        .insert_source(session_notifier, {
            let loop_handle = loop_handle.clone();
            move |event, _, app| match event {
                SessionEvent::PauseSession => {
                    eventline::info!("session event: pause");
                    let was_paused = app.driver.pause_reasons.any();
                    app.driver.pause_reasons.session = true;
                    libinput_for_session.suspend();
                    if !was_paused {
                        suspend_redraw_state(app, &loop_handle);
                    }
                    app.driver.backend.pause();
                }
                SessionEvent::ActivateSession => {
                    eventline::info!("session event: activate");
                    if libinput_for_session.resume().is_err() {
                        eventline::warn!(
                            "tty input: failed to resume libinput after VT activation"
                        );
                    }
                    match app.driver.backend.resume() {
                        // The whole DRM pipeline (and any frame that was in
                        // flight before the switch away) is gone - reset
                        // clean rather than trusting whatever redraw states
                        // said before the switch.
                        Ok(()) => {
                            app.driver.pause_reasons.session = false;
                            super::pointer::recover_after_session_resume(app);
                            if let Some(outputs) = app.driver.pending_output_config.take() {
                                apply_tty_output_config(app, &outputs);
                            }
                            if !app.driver.pause_reasons.any() {
                                resume_redraw_state(app);
                            }
                        }
                        Err(err) => eventline::error!("resume failed: {err}"),
                    }
                }
            }
        })
        .expect("failed to insert session notifier");

    event_loop
        .handle()
        .insert_source(drm_notifier, |event, metadata, app| match event {
            DrmEvent::VBlank(crtc) => on_vblank(app, crtc, metadata.as_ref()),
            DrmEvent::Error(err) => eventline::error!("drm event: error {err:?}"),
        })
        .expect("failed to insert drm notifier");

    eventline::info!("session ready: outputs active; use the configured Quit chord to exit");
    event_loop
        .run(None, &mut app, |app| {
            if !app.driver.pause_reasons.any() {
                redraw_queued_outputs(app, &loop_handle);
            }
            crate::ipc::publish_api_events(app);
            let _ = app.wayland.display_handle.flush_clients();
        })
        .expect("event loop run failed");
    eventline::info!("quit requested, exiting cleanly");
}

fn presentation_time(metadata: Option<&DrmEventMetadata>) -> Option<Duration> {
    match metadata?.time {
        DrmEventTime::Monotonic(time) if !time.is_zero() => Some(time),
        DrmEventTime::Monotonic(_) | DrmEventTime::Realtime(_) => None,
    }
}

fn on_vblank(app: &mut TtyApp, crtc: crtc::Handle, metadata: Option<&DrmEventMetadata>) {
    let Some(output) = app.driver.backend.output_for_crtc(crtc).cloned() else {
        eventline::warn!("vblank received for unknown CRTC {crtc:?}");
        return;
    };
    let presented = presentation_time(metadata);
    let sequence = metadata
        .map(|metadata| u64::from(metadata.sequence))
        .unwrap_or(0);
    let refresh_interval = app.driver.backend.refresh_interval_for_output(&output);
    let Some(state) = app.driver.output_frames.get_mut(&output) else {
        return;
    };
    let throttle = state.throttle_vblank(presented, refresh_interval);
    if let Some(token) = throttle.cancel_timer {
        app.driver.loop_handle.remove(token);
    }
    if let Some(delay) = throttle.delay {
        if throttle.warn {
            eventline::warn!(
                "output {:?}: kernel reported a vblank less than half a refresh after the previous event; delaying completion by {delay:?}",
                output.name()
            );
        }
        let delayed_output = output.clone();
        let timer = Timer::from_duration(delay);
        match app
            .driver
            .loop_handle
            .insert_source(timer, move |_, _, app| {
                if let Some(state) = app.driver.output_frames.get_mut(&delayed_output) {
                    state.vblank_throttle_timer_fired();
                }
                complete_vblank(app, crtc, &delayed_output, presented, sequence);
                TimeoutAction::Drop
            }) {
            Ok(token) => {
                if let Some(state) = app.driver.output_frames.get_mut(&output) {
                    state.vblank_throttle_timer_armed(token);
                }
                return;
            }
            Err(err) => eventline::warn!(
                "output {:?}: failed to arm vblank throttle timer: {err}",
                output.name()
            ),
        }
    }

    complete_vblank(app, crtc, &output, presented, sequence);
}

fn complete_vblank(
    app: &mut TtyApp,
    crtc: crtc::Handle,
    output: &Output,
    presented: Option<Duration>,
    sequence: u64,
) {
    let (submission, acknowledge_failed) = match app.driver.backend.frame_submitted(crtc) {
        Ok(submission) => (submission, false),
        Err(err) => {
            eventline::warn!(
                "failed to acknowledge vblank for {:?}: {err}",
                output.name()
            );
            (None, true)
        }
    };
    let Some(state) = app.driver.output_frames.get_mut(output) else {
        return;
    };
    let (mut action, unexpected) = state.on_vblank(presented);
    if acknowledge_failed {
        // Smithay may have failed while submitting an already-queued follow-up
        // frame. Always redraw after an acknowledgement error so the cleared
        // backend gate is exercised and the output cannot settle while stale.
        state.queue_redraw();
        action = VblankAction::Redraw;
    }
    if let Some(unexpected) = unexpected {
        eventline::warn!(
            "unexpected redraw state on vblank for {:?}: {unexpected}",
            output.name()
        );
    }
    if !app.driver.backend.output_dpms_enabled(output) {
        return;
    }

    if let Some(mut submission) = submission {
        if let Some(generation) = submission.session_lock_generation {
            app.session_lock.presented(output, generation);
        }
        // Keep the prediction attached to its submitted frame for
        // diagnostics, while reporting the kernel page-flip timestamp to
        // clients whenever it is available.
        let _target_presentation_time = submission.target_presentation_time;
        let fixed_refresh = output
            .current_mode()
            .map(|mode| Duration::from_secs_f64(1_000.0 / mode.refresh as f64))
            .filter(|refresh| !refresh.is_zero());
        let refresh = match (submission.variable_refresh, fixed_refresh) {
            (true, Some(refresh)) => smithay::wayland::presentation::Refresh::variable(refresh),
            (false, Some(refresh)) => smithay::wayland::presentation::Refresh::fixed(refresh),
            (_, None) => smithay::wayland::presentation::Refresh::Unknown,
        };
        let (time, flags) = match presented {
            Some(time) => (
                smithay::utils::Time::<smithay::utils::Monotonic>::from(time),
                smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::Vsync
                    | smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::HwClock
                    | smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::HwCompletion,
            ),
            None => (
                smithay::utils::Clock::<smithay::utils::Monotonic>::new().now(),
                smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::Vsync,
            ),
        };
        submission
            .presentation_feedback
            .presented(time, refresh, sequence, flags);
    }

    if action == VblankAction::SendCallbacks {
        send_output_frame_callbacks(app, output);
    }
}

fn send_output_frame_callbacks(app: &mut TtyApp, output: &Output) {
    let elapsed = app.start_time.elapsed();
    let callback_now = crate::frame_clock::monotonic_now();
    let primary = app.driver.backend.primary_output();
    let frame_callback_sequence = app
        .driver
        .output_frames
        .get(output)
        .map(OutputFrameState::frame_callback_sequence)
        .unwrap_or_default();
    if app.session_lock.active() {
        crate::wayland::session_lock::send_frames(
            &app.session_lock,
            output,
            elapsed,
            frame_callback_sequence,
        );
    } else if app
        .shell
        .cluster_composer
        .target_output()
        .is_some_and(|name| name == output.name())
    {
        crate::shell::cluster_composer::send_preview_frames(
            &app.shell.cluster_composer,
            &app.nodes,
            output,
            elapsed,
            frame_callback_sequence,
        );
    } else if app.shell.apogee.is_active() {
        // Direct surface previews are damage tracked by Smithay, so let the
        // output's own vblank cadence drive every visible client. Static
        // clients stop committing and therefore stop scheduling work.
        crate::shell::apogee::send_preview_frames(
            &app.shell.apogee,
            &app.nodes,
            output,
            elapsed,
            frame_callback_sequence,
        );
    } else if app.shell.focus_cycle.is_active() {
        crate::shell::focus_cycle::send_preview_frames(
            &app.shell.focus_cycle,
            &app.nodes,
            output,
            elapsed,
            frame_callback_sequence,
        );
    } else {
        let cluster_exclusive_member =
            crate::wayland::frame_callbacks::cluster_exclusive_callback_member(
                &app.wayland.space,
                &app.clusters,
                &app.nodes,
                &app.fullscreen,
                &app.maximize,
                output,
                callback_now,
            );
        app.wayland
            .space
            .elements()
            .filter(|window| wayland::window_is_on_output(window, output, primary))
            .for_each(|window| {
                let window_member = window
                    .wl_surface()
                    .and_then(|surface| app.nodes.id_for_surface(surface.as_ref()));
                let compositor_snapshot = window.wl_surface().is_some_and(|surface| {
                    app.render
                        .fullscreen_textures
                        .awaiting_target(surface.as_ref())
                        || app
                            .render
                            .arrange_textures
                            .awaiting_target(surface.as_ref())
                });
                let require_visible = crate::wayland::frame_callbacks::requires_render_visibility(
                    window_member,
                    cluster_exclusive_member,
                    compositor_snapshot,
                );
                window.send_frame(
                    output,
                    elapsed,
                    crate::wayland::frame_callbacks::FALLBACK_THROTTLE,
                    |surface, states| {
                        crate::wayland::frame_callbacks::callback_output(
                            surface,
                            states,
                            output,
                            frame_callback_sequence,
                            require_visible,
                        )
                    },
                );
            });
        crate::nodes::send_hover_preview_frame(
            &app.nodes,
            output,
            elapsed,
            callback_now,
            frame_callback_sequence,
        );
    }
    wayland::layer_shell::send_frames(output, elapsed, frame_callback_sequence);
    crate::cursor::surface::send_frame(
        &app.cursor,
        &app.wayland.space,
        output,
        app.pointer.position(),
        elapsed,
        frame_callback_sequence,
    );
    crate::wayland::dnd::send_frame(
        app.wayland.dnd_icon.as_ref(),
        output,
        elapsed,
        frame_callback_sequence,
    );
    app.wayland.space.refresh();
    if crate::xwayland::sync_positions(app) {
        // The client's root coordinates just moved under a possibly stationary
        // pointer, so its hover state is stale until it is re-routed.
        super::pointer::refresh_desktop_client_focus(
            app,
            app.start_time.elapsed().as_millis() as u32,
        );
    }
    crate::xwayland::sync_stacking_order(app);
    wayland::layer_shell::cleanup(&mut app.wayland);
}

/// Pure state transition - safe to call from any input/event handler with
/// no event-loop access. The actual rendering only ever happens in
/// `redraw_output()`, called from the `run()` tail once a redraw is actually
/// `Queued`.
fn queue_redraw(app: &mut TtyApp) {
    app.request_redraw();
}

fn output_at_pointer(app: &TtyApp) -> Option<Output> {
    app.wayland
        .space
        .output_under(app.pointer.position())
        .next()
        .cloned()
}

fn queue_output_redraw(app: &mut TtyApp, output: &Output) {
    app.request_output_redraw(output);
}

fn apply_runtime_config(app: &mut TtyApp, reload: crate::config::ConfigReload) {
    let accepted = matches!(reload, crate::config::ConfigReload::Loaded(_));
    match reload {
        crate::config::ConfigReload::Loaded(config) => {
            let config = *config;
            let reload_commands = config.autostart.on_reload.clone();
            app.apply_common_config(&config);
            app.driver.physical_input.reload(&app.settings.input);

            if app.driver.pause_reasons.any() {
                app.driver.pending_output_config = Some(config.outputs);
            } else {
                apply_tty_output_config(app, &config.outputs);
            }
            app.run_autostart_reload(&reload_commands);
            app.clear_config_reload_error();
        }
        crate::config::ConfigReload::Rejected(diagnostic) => {
            eventline::debug!("config: rejected reload for {:?}", diagnostic.path);
            app.show_config_reload_error();
        }
    }
    crate::ipc::publish_config_reload(app, accepted);
}

fn apply_tty_output_config(app: &mut TtyApp, outputs_config: &[halley_config::OutputConfig]) {
    let changes = app.driver.backend.apply_output_config(outputs_config);
    let mut layout_changed = false;
    let mut output_changed = false;

    for change in changes {
        output_changed = true;
        if change.mode_changed {
            let interval = app
                .driver
                .backend
                .refresh_interval_for_output(&change.output);
            if let Some(state) = app.driver.output_frames.get_mut(&change.output) {
                state.replace_clock(interval);
            }
        }
        let vrr_active = app.driver.backend.output_vrr_active(&change.output);
        if let Some(state) = app.driver.output_frames.get_mut(&change.output) {
            state.set_vrr(vrr_active);
        }

        if change.layout_changed {
            app.wayland
                .space
                .map_output(&change.output, change.output.current_location());
            smithay::desktop::layer_map_for_output(&change.output).arrange();
            layout_changed = true;
        }

        if change.size_changed
            && let Some(geometry) = app.wayland.space.output_geometry(&change.output)
        {
            app.cameras
                .reset(change.output.name(), geometry.size.to_physical(1));
            let external = app
                .fullscreen
                .reconfigure_output(&app.wayland, &change.output);
            crate::xwayland::reconfigure_fullscreen(external);
        }

        queue_output_redraw(app, &change.output);
    }

    if layout_changed {
        app.wayland.space.refresh();
        // A pure output move rebases every window's global coordinates; the X
        // server only learns about it through this resync.
        crate::xwayland::sync_positions(app);
        app.capture.update_layout(&app.wayland.space);
    }
    if layout_changed || output_changed {
        app.xwayland.sync_desktop_geometry(&app.wayland.space);
    }
    if output_changed {
        crate::wayland::session_lock::configure_surfaces(app);
        super::pointer::update_client_state(app, app.start_time.elapsed().as_millis() as u32);
        app.notify_output_management();
    }
}

fn redraw_queued_outputs(app: &mut TtyApp, loop_handle: &LoopHandle<'_, TtyApp>) {
    let now = crate::frame_clock::monotonic_now();
    let _ = super::input::tick_grabbed_window_edge_pan(app, now);
    let _ = crate::nodes::tick_physics(app, now);
    // Match startup's connector/CRTC activation order. Iterating the
    // `HashMap` made a multi-output DPMS wake nondeterministic, so the
    // primary could intermittently queue its modeset before the secondary
    // that normally commits first during compositor startup.
    let outputs: Vec<_> = app
        .driver
        .backend
        .outputs()
        .filter(|output| {
            app.driver
                .output_frames
                .get(*output)
                .is_some_and(OutputFrameState::is_redraw_queued)
        })
        .cloned()
        .collect();

    for output in outputs {
        redraw_output(app, &output, loop_handle);
    }
}

fn redraw_output(app: &mut TtyApp, output: &Output, loop_handle: &LoopHandle<'_, TtyApp>) {
    let now = crate::frame_clock::monotonic_now();
    super::reconcile_cluster_surfaces(app, &output.name());
    let (target_presentation_time, dt) = {
        let state = app
            .driver
            .output_frames
            .get_mut(output)
            .expect("redraw output has frame state");
        state.next_frame_sample(now)
    };

    let pointer_is_on_output = app
        .wayland
        .space
        .output_under(app.pointer.position())
        .next()
        .is_some_and(|under| under == output);
    let view_before = pointer_is_on_output
        .then(|| app.cameras.view(&output.name()))
        .flatten();
    let cluster_camera_changed =
        super::sync_cluster_camera(app, &output.name(), target_presentation_time);
    let fullscreen_camera_changed = app.sync_fullscreen_camera(output, target_presentation_time);
    let zoom_tick = app.cameras.get_mut(&output.name()).map(|camera| {
        let before = crate::input::zoom::scale(camera);
        let (after, animating) = crate::input::zoom::tick(
            camera,
            &app.settings.zoom,
            app.settings.input.gestures.pan_decay_rate,
            dt.as_secs_f32(),
        );
        (animating, (before != after).then_some((before, after)))
    });
    let camera_animating = zoom_tick.is_some_and(|(animating, _)| animating);
    let edge_pan_animating = super::input::grabbed_window_edge_pan_active_on(app, &output.name());
    if let Some((before, after)) = zoom_tick.and_then(|(_, scales)| scales) {
        if after < before && app.clusters.active_on(&output.name()).is_none() {
            crate::nodes::reconcile_landmarks_for_zoom(app, &output.name(), after);
        }
        app.shell.overlays.show_zoom_indicator(
            &output.name(),
            after,
            &app.settings.overlays.zoom_indicator,
            target_presentation_time,
        );
    }
    let primary = app.driver.backend.primary_output();
    let window_animating = app.wayland.space.elements().any(|window| {
        wayland::window_is_on_output(window, output, primary)
            && window.wl_surface().is_some_and(|surface| {
                app.window_animations
                    .is_animating(surface.as_ref(), target_presentation_time)
            })
    });
    let arrange_animating = app.wayland.space.elements().any(|window| {
        wayland::window_is_on_output(window, output, primary)
            && window.wl_surface().is_some_and(|surface| {
                app.window_animations
                    .is_arranging(surface.as_ref(), target_presentation_time)
            })
    });
    let presentation_workspace = crate::presentation::active_workspace_on_output(
        &app.clusters,
        &output.name(),
        target_presentation_time,
    );
    let fullscreen_animating = app.fullscreen.is_animating_on_output_matching(
        output,
        target_presentation_time,
        |surface| {
            crate::presentation::workspace_for_surface(&app.clusters, &app.nodes, surface)
                == presentation_workspace
        },
    );
    let maximize_animating = app.maximize.is_animating_on_output(
        output,
        presentation_workspace,
        target_presentation_time,
    );
    let closing_animating = app
        .render
        .window_close_animations
        .is_animating_on_output(output, target_presentation_time);
    let node_animating = app
        .nodes
        .is_animating_on_output(&output.name(), target_presentation_time);
    let bearings_animating = app.shell.bearings.tick(
        &output.name(),
        target_presentation_time,
        !app.fullscreen
            .presents_immersive_on_output_matching(output, |surface| {
                crate::presentation::surface_workspace_is_active(
                    &app.clusters,
                    &app.nodes,
                    surface,
                    &output.name(),
                    target_presentation_time,
                )
            }),
    );
    let focus_cycle_animating = app.shell.focus_cycle.tick(target_presentation_time);
    let composer_animating = app
        .shell
        .cluster_composer
        .target_output()
        .is_some_and(|name| name == output.name())
        && crate::shell::cluster_composer::tick_session(app, target_presentation_time);
    let apogee_animating = crate::shell::apogee::tick(app, target_presentation_time);
    let background_animating = app.background_animates_on_output(output, target_presentation_time);
    let overlay_animating = app.shell.overlays.animating(target_presentation_time);
    let cluster_animating = app
        .clusters
        .is_animating_on_output(&output.name(), target_presentation_time)
        || app
            .clusters
            .bloom_is_animating_on_output(&output.name(), target_presentation_time)
        || app
            .clusters
            .labels_animating_on_output(&output.name(), app.nodes.config.show_labels);
    let show_cursor = super::pointer::cursor_visible(app);
    let cursor_override = super::pointer::cursor_override(app);
    crate::cursor::surface::refresh_outputs(
        &app.cursor,
        &app.wayland.space,
        app.pointer.position(),
    );
    crate::wayland::dnd::refresh_outputs(
        app.wayland.dnd_icon.as_ref(),
        &app.wayland.space,
        app.pointer.position(),
    );
    if pointer_is_on_output {
        let next_cursor_frame = show_cursor
            .then(|| {
                app.cursor.current_next_frame_in_with_override(
                    output.current_scale().integer_scale(),
                    target_presentation_time,
                    cursor_override,
                )
            })
            .flatten();
        app.cursor_policy
            .schedule_animation(output, next_cursor_frame);
    }
    let mut animating = camera_animating
        || edge_pan_animating
        || fullscreen_camera_changed
        || window_animating
        || closing_animating
        || node_animating
        || bearings_animating
        || focus_cycle_animating
        || composer_animating
        || apogee_animating
        || background_animating
        || overlay_animating
        || cluster_animating
        || fullscreen_animating
        || maximize_animating
        || app.settings.debug.overlay_fps && !app.session_lock.active();
    if pointer_is_on_output {
        let time = app.start_time.elapsed().as_millis() as u32;
        if cluster_camera_changed || fullscreen_animating || maximize_animating || arrange_animating
        {
            super::pointer::update_client_state(app, time);
        } else if cluster_animating {
            super::pointer::refresh_client_focus(app, time);
        }
    }
    let view_after = pointer_is_on_output
        .then(|| app.cameras.view(&output.name()))
        .flatten();
    if view_before != view_after {
        super::pointer::update_client_state(app, app.start_time.elapsed().as_millis() as u32);
    }
    super::trace::snapshot(app);
    let vrr_auto_eligible = auto_vrr_eligible(app, output, target_presentation_time);

    let outcome = match app.driver.backend.render(
        output,
        RenderRequest {
            frame: FrameContext {
                target_presentation_time,
                vrr_auto_eligible,
                force_full_repaint: animating,
                clear: CLEAR_COLOR,
            },
            desktop: DesktopContext {
                session_lock: &app.session_lock,
                space: &app.wayland.space,
                focused: app.wayland.focused_window.as_ref(),
                cameras: &app.cameras,
                window_animations: &app.window_animations,
                fullscreen: &app.fullscreen,
                maximize: &app.maximize,
                nodes: &app.nodes,
                clusters: &app.clusters,
                window_rules: &app.window_rules,
                layer_rules: &app.settings.layer_rules,
                node_grab_active: app.interactions.grab.landmark_active(),
                titlebar_hovered: app.interactions.titlebar_hovered.as_ref(),
                titlebar_pressed: app.interactions.titlebar_pressed.as_ref(),
            },
            cursor: CursorContext {
                cursor: &app.cursor,
                dnd_icon: app.wayland.dnd_icon.as_ref(),
                cursor_position: app.pointer.position(),
                show_cursor,
                cursor_override,
            },
            overlays: OverlayContext {
                capture_overlay: app.capture.overlay(),
                bearings: &app.shell.bearings,
                focus_cycle: &app.shell.focus_cycle,
                apogee: &app.shell.apogee,
                cluster_composer: &app.shell.cluster_composer,
                apogee_config: app.settings.apogee,
                overlays: &app.shell.overlays,
                overlay_config: &app.settings.overlays,
            },
            visuals: VisualContext {
                decorations: &app.settings.decorations,
                pins: &app.settings.field.pins,
                font: &app.settings.font,
                debug: app.settings.debug,
                blur: app.settings.effects.blur,
                shadows: app.settings.effects.shadows,
                background: &app.settings.background,
                background_base: app.config_path.as_deref().and_then(std::path::Path::parent),
            },
            resources: crate::render::resources::RenderResources::from(&mut app.render),
        },
    ) {
        Ok(outcome) => outcome,
        Err(err) => {
            eventline::error!("render failed for {:?}: {err}", output.name());
            RenderOutcome::new(RenderStatus::Skipped, None)
        }
    };
    animating |= app.render.node_renderer.has_pending_icons();
    if app.window_animations.cleanup(target_presentation_time) {
        // The frame just composed can still scale a lagging pre-configure
        // client buffer into the arrangement endpoint. Owe one live-geometry
        // frame after retiring that endpoint so it cannot stay latched.
        animating = true;
        super::pointer::update_client_state(app, app.start_time.elapsed().as_millis() as u32);
    }
    app.render
        .arrange_textures
        .retain_surfaces(|surface| app.window_animations.has_arrange_timeline(surface));
    app.render
        .window_close_animations
        .cleanup(target_presentation_time);
    if app.cleanup_fullscreen(target_presentation_time) {
        // Cleanup retires the transition and drops its crossfade textures, so
        // the scene this frame rendered is the last one drawn from those
        // textures. One more frame is owed to swap back to the live surfaces;
        // without it the swap waits on unrelated damage and lands as a pop.
        animating = true;
        super::sync_keyboard_focus(app, smithay::utils::SERIAL_COUNTER.next_serial());
        super::pointer::update_client_state(app, app.start_time.elapsed().as_millis() as u32);
    }
    let feedback = app.driver.dmabuf_feedback(output).cloned();
    if let (Some(feedback), Some(element_states)) = (feedback.as_ref(), outcome.element_states()) {
        let primary_output = app.driver.backend.primary_output().clone();
        wayland::dmabuf::send_output_feedback(
            &app.wayland,
            output,
            &primary_output,
            &app.session_lock,
            feedback,
            element_states,
        );
    }
    if let Some(element_states) = outcome.element_states().cloned() {
        app.update_idle_inhibit_visibility(output, &element_states);
    }
    let vrr_active = app.driver.backend.output_vrr_active(output);
    if let Some(state) = app.driver.output_frames.get_mut(output) {
        state.set_vrr(vrr_active);
    }

    if outcome.status() == RenderStatus::Submitted {
        let state = app
            .driver
            .output_frames
            .get_mut(output)
            .expect("rendered output has frame state");
        state.advance_frame_callback_sequence();
        if let Some(token) = state.frame_submitted(animating) {
            loop_handle.remove(token);
        }
        // The compositor has latched every client buffer used by this frame.
        // Release the next callbacks now; waiting for the page-flip event can
        // starve an otherwise idle output while input is active elsewhere.
        // A commit produced by these callbacks remains gated on that vblank.
        send_output_frame_callbacks(app, output);
        app.service_screencopy(output);
        return;
    }

    queue_estimated_vblank_timer(app, output, animating, loop_handle);
}

fn auto_vrr_eligible(app: &TtyApp, output: &Output, now: Duration) -> bool {
    if app.session_lock.active()
        || app.capture.is_active()
        || app.settings.debug.overlay_fps
        || app
            .shell
            .cluster_composer
            .target_output()
            .is_some_and(|name| name == output.name())
        || app.shell.apogee.is_active()
        || app.shell.focus_cycle.session().is_some()
        || app.shell.bearings.mix(&output.name()) > 0.002
        || app
            .nodes
            .hover_preview_visible_on_output(&output.name(), now)
    {
        return false;
    }
    let overlays = app.shell.overlays.snapshot(&output.name(), now);
    if overlays.exit_mix.is_some()
        || overlays.confirmation.is_some()
        || overlays.notification.is_some()
        || overlays.zoom_indicator.is_some()
        || overlays.cluster_indicator.is_some()
    {
        return false;
    }
    if smithay::desktop::layer_map_for_output(output)
        .layers_on(Layer::Overlay)
        .next()
        .is_some()
    {
        return false;
    }
    app.fullscreen
        .stable_fullscreen_surface_on_output_matching(output, now, |surface| {
            crate::presentation::surface_workspace_is_active(
                &app.clusters,
                &app.nodes,
                surface,
                &output.name(),
                now,
            )
        })
        .is_some_and(|surface| app.window_rules.opacity(surface) >= 0.999)
}

fn queue_estimated_vblank_timer(
    app: &mut TtyApp,
    output: &Output,
    animating: bool,
    loop_handle: &LoopHandle<'_, TtyApp>,
) {
    let state = app
        .driver
        .output_frames
        .get_mut(output)
        .expect("estimated-vblank output has frame state");
    let now = crate::frame_clock::monotonic_now();
    let EstimatedVblankTimer::ArmAfter(delay) = state.frame_skipped(animating, now) else {
        return;
    };
    let output = output.clone();
    let token = loop_handle
        .insert_source(Timer::from_duration(delay), move |_, _, app| {
            on_estimated_vblank_timer(app, &output);
            TimeoutAction::Drop
        })
        .expect("failed to arm estimated-vblank timer");
    state.timer_armed(token);
}

fn on_estimated_vblank_timer(app: &mut TtyApp, output: &Output) {
    let Some(state) = app.driver.output_frames.get_mut(output) else {
        return;
    };
    match state.estimated_vblank_fired() {
        Ok(true) => {
            state.advance_frame_callback_sequence();
            send_output_frame_callbacks(app, output)
        }
        Ok(false) => {}
        Err(unexpected) => {
            eventline::warn!(
                "unexpected redraw state on estimated-vblank timer for {:?}: {unexpected}",
                output.name()
            );
        }
    }
}

fn suspend_redraw_state(app: &mut TtyApp, loop_handle: &LoopHandle<'_, TtyApp>) {
    let now = crate::frame_clock::monotonic_now();
    for state in app.driver.output_frames.values_mut() {
        for token in state.suspend(now) {
            loop_handle.remove(token);
        }
    }
}

fn handle_system_sleep(app: &mut TtyApp, preparing: bool) {
    if preparing {
        if app.driver.pause_reasons.system_sleep {
            return;
        }
        eventline::info!("system sleep: preparing");
        let was_paused = app.driver.pause_reasons.any();
        app.driver.pause_reasons.system_sleep = true;
        if !was_paused {
            let loop_handle = app.driver.loop_handle.clone();
            suspend_redraw_state(app, &loop_handle);
        }
        return;
    }

    eventline::info!("system sleep: resumed; invalidating pre-suspend output buffers");
    let was_system_sleep = app.driver.pause_reasons.system_sleep;
    app.driver.backend.recover_after_system_sleep();
    app.driver.pause_reasons.system_sleep = false;
    super::pointer::recover_after_session_resume(app);

    if app.driver.pause_reasons.any() {
        return;
    }
    if let Some(outputs) = app.driver.pending_output_config.take() {
        apply_tty_output_config(app, &outputs);
    }
    if was_system_sleep {
        resume_redraw_state(app);
    } else {
        // A delayed subscription can miss PrepareForSleep(true). Buffer
        // invalidation still makes the next queued frame a complete redraw.
        app.request_redraw();
    }
}

fn resume_redraw_state(app: &mut TtyApp) {
    let now = crate::frame_clock::monotonic_now();
    for state in app.driver.output_frames.values_mut() {
        state.resume(now);
    }
}

#[cfg(test)]
mod pause_tests {
    use super::PauseReasons;

    #[test]
    fn overlapping_pause_reasons_require_both_resumes() {
        let mut reasons = PauseReasons {
            session: true,
            system_sleep: true,
        };
        assert!(reasons.any());

        reasons.system_sleep = false;
        assert!(reasons.any());

        reasons.session = false;
        assert!(!reasons.any());
    }
}
