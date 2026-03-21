use super::*;
use std::collections::HashMap;

use crate::backend::interface::{
    BackendView, DmabufImportBackend, TtyBackendHandle, TtyDmabufImportBackend,
};
use crate::backend::tty_drm::{
    collect_outputs_for_ipc, find_tty_scanout_for_reload, probe_tty_drm_device_via_session,
    queue_tty_drm_frame,
};
use crate::backend::tty_input::build_tty_libinput_backend;
use calloop::{Interest, Mode, PostAction, generic::Generic};

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, Event, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent,
    PointerButtonEvent, PointerMotionEvent,
};

const CONFIG_RELOAD_SETTLE_MS: u64 = 100;

fn publish_tty_outputs_snapshot(
    dev: &DrmDevice,
    active_modes: &HashMap<String, drm_control::Mode>,
    dpms_enabled: bool,
    tuning: &RuntimeTuning,
) {
    let vrr_support: HashMap<String, String> = HashMap::new();
    let mut outputs = collect_outputs_for_ipc(dev, active_modes, tuning, &vrr_support);
    if !dpms_enabled {
        for output in &mut outputs {
            if active_modes.contains_key(&output.name) {
                output.enabled = false;
                output.current_mode = None;
                for mode in &mut output.modes {
                    mode.current = false;
                }
            }
        }
    }
    publish_outputs(outputs);
}

fn apply_tty_dpms_command(
    gbm_surfaces: &[Rc<RefCell<GbmBufferedSurface<GbmAllocator<DeviceFd>, ()>>>],
    dev: &Rc<RefCell<DrmDevice>>,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: &Rc<RefCell<bool>>,
    command: halley_ipc::DpmsCommand,
    renderer: &Rc<RefCell<GlesRenderer>>,
    tuning: &RuntimeTuning,
) {
    let target_enabled = match command {
        halley_ipc::DpmsCommand::On => true,
        halley_ipc::DpmsCommand::Off => false,
        halley_ipc::DpmsCommand::Toggle => !*dpms_enabled.borrow(),
    };

    if target_enabled == *dpms_enabled.borrow() {
        return;
    }

    if !target_enabled {
        for gbm_surface in gbm_surfaces {
            let result = gbm_surface.borrow().surface().clear();
            if let Err(err) = result {
                warn!("tty dpms off failed: {}", err);
            }
        }
        *dpms_enabled.borrow_mut() = false;
        info!("tty dpms: powered off active connectors");
    } else {
        *dpms_enabled.borrow_mut() = true;
        info!("tty dpms: powering on active connectors");
        // Eager modeset on wake: queue a blank frame to every output now so
        // the blocking DRM commit_pending() fires here rather than inside the
        // first timer tick. Without this, the slow monitor stalls the event
        // loop for its full modeset duration before showing anything.
        let mut rend = renderer.borrow_mut();
        for gbm_surface in gbm_surfaces {
            let mut gbm = gbm_surface.borrow_mut();
            match gbm.next_buffer() {
                Ok((mut dmabuf, _age)) => match rend.bind(&mut dmabuf) {
                    Ok(_target) => {
                        drop(_target);
                        match gbm.queue_buffer(None, None, ()) {
                            Ok(()) => info!("dpms wake: initial modeset queued"),
                            Err(err) => warn!("dpms wake: queue_buffer failed: {}", err),
                        }
                    }
                    Err(err) => warn!("dpms wake: bind failed: {}", err),
                },
                Err(err) => warn!("dpms wake: next_buffer failed: {}", err),
            }
        }
    }

    publish_tty_outputs_snapshot(
        &dev.borrow(),
        &active_modes.borrow(),
        *dpms_enabled.borrow(),
        tuning,
    );
}

fn wake_tty_dpms_on_input(
    gbm_surfaces: &[Rc<RefCell<GbmBufferedSurface<GbmAllocator<DeviceFd>, ()>>>],
    dev: &Rc<RefCell<DrmDevice>>,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: &Rc<RefCell<bool>>,
    renderer: &Rc<RefCell<GlesRenderer>>,
    tuning: &RuntimeTuning,
) {
    if *dpms_enabled.borrow() {
        return;
    }
    apply_tty_dpms_command(
        gbm_surfaces,
        dev,
        active_modes,
        dpms_enabled,
        halley_ipc::DpmsCommand::On,
        renderer,
        tuning,
    );
}

fn apply_tty_reload(
    dev: &Rc<RefCell<DrmDevice>>,
    backend_handle: &TtyBackendHandle,
    pointer_state: &Rc<RefCell<PointerState>>,
    st: &mut HalleyWlState,
    next: RuntimeTuning,
    config_path: &str,
    wayland_display: &str,
    reason: &str,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: bool,
) {
    let desired = {
        let mut dev_ref = dev.borrow_mut();
        match find_tty_scanout_for_reload(&mut dev_ref, &next) {
            Ok(target) => target,
            Err(err) => {
                warn!(
                    "{}: viewport reload rejected for {}: {}; keeping last working tty mode",
                    reason, config_path, err
                );
                return;
            }
        }
    };
    let next_modes: HashMap<_, _> = desired
        .iter()
        .map(|(_, mode, _, name)| (name.clone(), *mode))
        .collect();
    let same_outputs = {
        let current = active_modes.borrow();
        current.len() == next_modes.len() && current.keys().all(|name| next_modes.contains_key(name))
    };
    if !same_outputs {
        warn!(
            "{}: live tty output-set reload is not supported yet; restart Halley to apply monitor changes from {}",
            reason, config_path
        );
        return;
    }
    let layout_w = next
        .tty_viewports
        .iter()
        .filter(|viewport| viewport.enabled)
        .map(|viewport| viewport.offset_x + viewport.width as i32)
        .max()
        .unwrap_or(next.viewport_size.x.max(1.0).round() as i32);
    let layout_h = next
        .tty_viewports
        .iter()
        .filter(|viewport| viewport.enabled)
        .map(|viewport| viewport.offset_y + viewport.height as i32)
        .max()
        .unwrap_or(next.viewport_size.y.max(1.0).round() as i32);
    backend_handle.set_size(layout_w, layout_h);
    {
        let mut ps = pointer_state.borrow_mut();
        let old = ps.workspace_size;
        ps.workspace_size = (layout_w, layout_h);
        if old.0 > 0 && old.1 > 0 {
            let sx = ps.screen.0 * (layout_w as f32) / (old.0 as f32);
            let sy = ps.screen.1 * (layout_h as f32) / (old.1 as f32);
            ps.screen = (
                sx.clamp(0.0, (layout_w.saturating_sub(1)) as f32),
                sy.clamp(0.0, (layout_h.saturating_sub(1)) as f32),
            );
        }
    }
    let live_camera = crate::run::capture_live_camera_state(st);
    st.apply_tuning(next);
    crate::run::restore_live_camera_state(st, live_camera);
    {
        let mut current = active_modes.borrow_mut();
        *current = next_modes.clone();
    }
    for (name, mode) in &next_modes {
        st.advertise_output(name.as_str(), (*mode).into());
    }
    publish_tty_outputs_snapshot(&dev.borrow(), &active_modes.borrow(), dpms_enabled, &st.tuning);
    let reload_commands = st.tuning.autostart_on_reload.clone();
    run_autostart_commands(st, &reload_commands, wayland_display, "autostart");
    info!(
        "{}: reloaded config from {} with tty layout {}x{}",
        reason,
        config_path,
        layout_w,
        layout_h
    );
}

/// Returns `(width, height, offset_x, offset_y)` for the first enabled
/// viewport in the tuning config.  This is the monitor that an absolute
/// pointer device (touchpad, tablet, most mice in some setups) physically
/// covers.  We use these dimensions — not the full combined-layout size — when
/// calling libinput's `x_transformed` / `y_transformed` so that the
/// normalised [0,1] range maps to one monitor rather than being stretched
/// across all of them.
fn primary_tty_monitor_dims(tuning: &RuntimeTuning) -> (i32, i32, i32, i32) {
    tuning
        .tty_viewports
        .iter()
        .find(|v| v.enabled)
        .map(|v| (v.width as i32, v.height as i32, v.offset_x, v.offset_y))
        .unwrap_or((1920, 1080, 0, 0))
}

pub(crate) fn run_tty_backend() -> Result<(), Box<dyn Error>> {
    eprintln!("halley-wl tty: starting");
    scope!(
        "halley-wl-tty",
        success = "compositor exited",
        failure = "compositor failed",
        aborted = "compositor aborted",
        {
            ensure_xdg_runtime_dir()?;
            ensure_dbus_session_bus_address();
            if let Err(err) = init_logging() {
                eprintln!("halley-wl tty: logging init failed: {err}");
                return Err(err);
            }
            eprintln!("halley-wl tty: logging initialized");

            let (seat_name, drm_probe, libinput_backend, libinput_context, session_notifier) = {
                let config_path = RuntimeTuning::config_path();
                let tuning = RuntimeTuning::load_from_path(config_path.as_str());
                let (tty_session, session_notifier) = LibSeatSession::new().map_err(|err| {
                    io::Error::other(format!("failed to initialize libseat session: {:?}", err))
                })?;
                info!("tty libseat backend=auto");
                let tty_session = Rc::new(RefCell::new(tty_session));
                let seat_name = tty_session.borrow().seat();
                let drm_probe = probe_tty_drm_device_via_session(
                    seat_name.as_str(),
                    tty_session.clone(),
                    &tuning,
                )?;
                let (libinput_backend, libinput_context) =
                    build_tty_libinput_backend(tty_session.clone(), seat_name.as_str())?;
                (
                    seat_name,
                    drm_probe,
                    libinput_backend,
                    libinput_context,
                    session_notifier,
                )
            };

            info!(
                "tty backend starting on seat={} with direct DRM scanout",
                seat_name
            );

            let mut display: Display<HalleyWlState> = Display::new()?;
            let dh = display.handle();

            let config_path = Rc::new(RuntimeTuning::config_path());
            let tuning = RuntimeTuning::load_from_path(config_path.as_str());
            tuning.apply_process_env();
            if !Path::new(config_path.as_str()).exists() {
                warn!(
                    "config file not found at {}; using built-in defaults",
                    config_path.as_str()
                );
            }
            info!("runtime tuning: {:?}", tuning);
            info!("config path: {}", config_path.as_str());
            info!("keybind modifier: {}", tuning.keybinds.modifier_name());
            info!("resolved keybinds: {}", tuning.keybinds_resolved_summary());
            info!("physics enabled: {}", tuning.physics_enabled);

            let (watch_rx, _watcher): (Option<mpsc::Receiver<()>>, Option<RecommendedWatcher>) = {
                let (watch_tx, watch_rx) = mpsc::channel::<()>();
                let config_watch_target = PathBuf::from(config_path.as_str());
                let config_watch_name = config_watch_target
                    .file_name()
                    .map(std::ffi::OsStr::to_os_string);
                let mut watcher: RecommendedWatcher = notify::recommended_watcher(
                    move |result: Result<notify::Event, notify::Error>| {
                        if let Ok(event) = result {
                            let touches_config = if event.paths.is_empty() {
                                true
                            } else {
                                event.paths.iter().any(|path| {
                                    path == &config_watch_target
                                        || config_watch_name
                                            .as_ref()
                                            .is_some_and(|name| path.file_name() == Some(name))
                                })
                            };
                            if touches_config {
                                match event.kind {
                                    EventKind::Modify(_)
                                    | EventKind::Create(_)
                                    | EventKind::Remove(_) => {
                                        let _ = watch_tx.send(());
                                    }
                                    _ => {}
                                }
                            }
                        }
                    },
                )?;
                let watch_root = Path::new(config_path.as_str())
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from(config_path.as_str()));
                if let Err(err) = watcher.watch(watch_root.as_path(), RecursiveMode::NonRecursive) {
                    warn!(
                        "config watch disabled for {} (watch root {}): {}",
                        config_path.as_str(),
                        watch_root.display(),
                        err
                    );
                }
                (Some(watch_rx), Some(watcher))
            };

            let listening = ListeningSocketSource::new_auto().map_err(|err| {
                let xdg_runtime_dir =
                    env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "<unset>".to_string());
                io::Error::other(format!(
                    "failed to create WAYLAND listening socket (XDG_RUNTIME_DIR={}): {}",
                    xdg_runtime_dir, err
                ))
            })?;
            let sock_name = listening.socket_name().to_string_lossy().to_string();
            info!("WAYLAND_DISPLAY={}", sock_name);
            let xwayland = Rc::new(RefCell::new(ensure_xwayland_satellite(sock_name.as_str())?));
            let (xwayland_request_tx, xwayland_request_rx) = mpsc::channel::<()>();
            register_xwayland_request_channel(xwayland_request_tx);
            let xwayland_request_rx = Rc::new(RefCell::new(xwayland_request_rx));
            let xwayland_for_timer = xwayland.clone();
            let xwayland_request_for_timer = xwayland_request_rx.clone();
            let libinput_backend = libinput_backend;

            let mut ev: EventLoop<HalleyWlState> = EventLoop::try_new()?;
            let _signal = ev.get_signal();
            let mut state = HalleyWlState::new(&dh, ev.handle(), tuning.clone());
            let dmabuf_importer: Rc<dyn DmabufImportBackend> =
                Rc::new(TtyDmabufImportBackend::new(drm_probe.renderer.clone()));
            state.configure_dmabuf_importer_for_fd(dmabuf_importer, drm_probe.dev.device_fd());
            if smithay::wayland::drm_syncobj::supports_syncobj_eventfd(drm_probe.dev.device_fd()) {
                state.drm_syncobj_state =
                    Some(smithay::wayland::drm_syncobj::DrmSyncobjState::new::<
                        HalleyWlState,
                    >(&dh, drm_probe.dev.device_fd().clone()));
            }
            state.set_app_focused(true);
            state.seat.add_pointer();
            if state
                .seat
                .add_keyboard(Default::default(), 200, 30)
                .is_err()
            {
                warn!("failed to initialize wl_seat keyboard");
            }
            let autostart_once = state.tuning.autostart_once.clone();
            run_autostart_commands(&mut state, &autostart_once, sock_name.as_str(), "autostart");

            let mut dh_for_clients = dh.clone();
            ev.handle()
                .insert_source(listening, move |client_stream, _, _st| {
                    let _ =
                        dh_for_clients.insert_client(client_stream, Arc::new(ClientState::new()));
                })?;

            if let Some(listener) = xwayland.borrow().filesystem_listener_source()? {
                let xwayland_for_x11 = xwayland.clone();
                ev.handle().insert_source(
                    Generic::new(listener, Interest::READ, Mode::Level),
                    move |_readiness, _listener, _st| {
                        xwayland_for_x11.borrow_mut().request_start();
                        Ok(PostAction::Continue)
                    },
                )?;
            }
            if let Some(listener) = xwayland.borrow().abstract_listener_source()? {
                let xwayland_for_x11 = xwayland.clone();
                ev.handle().insert_source(
                    Generic::new(listener, Interest::READ, Mode::Level),
                    move |_readiness, _listener, _st| {
                        xwayland_for_x11.borrow_mut().request_start();
                        Ok(PostAction::Continue)
                    },
                )?;
            }

            {
                let libinput_context_for_session = libinput_context.clone();
                ev.handle()
                    .insert_source(session_notifier, move |event, _, _st| match event {
                        SessionEvent::PauseSession => {
                            info!("tty session paused");
                            libinput_context_for_session.borrow_mut().suspend();
                        }
                        SessionEvent::ActivateSession => {
                            info!("tty session activated");
                            if libinput_context_for_session.borrow_mut().resume().is_err() {
                                warn!("failed to resume libinput context after session activation");
                            }
                        }
                    })?;
            }

            let mod_state = Rc::new(RefCell::new(ModState::default()));
            let mod_state_for_input = mod_state.clone();
            let pointer_state = Rc::new(RefCell::new(PointerState::default()));
            let mod_state_for_timer = mod_state.clone();
            let pointer_state_for_input = pointer_state.clone();
            let pointer_state_for_timer = pointer_state.clone();
            let keyboard_seen = Rc::new(RefCell::new(false));
            let keyboard_seen_for_input = keyboard_seen.clone();
            let keyboard_seen_for_timer = keyboard_seen.clone();
            let pointer_seen = Rc::new(RefCell::new(false));
            let pointer_seen_for_input = pointer_seen.clone();
            let pointer_seen_for_timer = pointer_seen.clone();
            let input_started_at = Instant::now();
            let warned_keyboard_missing = Rc::new(RefCell::new(false));
            let warned_keyboard_missing_for_timer = warned_keyboard_missing.clone();
            let warned_pointer_missing = Rc::new(RefCell::new(false));
            let warned_pointer_missing_for_timer = warned_pointer_missing.clone();
            let watch_rx = Rc::new(RefCell::new(watch_rx));
            let watch_rx_for_timer = watch_rx.clone();
            let pending_watch_reload_at = Rc::new(RefCell::new(None::<Instant>));
            let pending_watch_reload_at_for_timer = pending_watch_reload_at.clone();
            let config_path_for_timer = config_path.clone();
            let wayland_display_for_timer = sock_name.clone();
            let layout_w = state
                .monitors
                .values()
                .map(|monitor| monitor.offset_x + monitor.width)
                .max()
                .unwrap_or(state.tuning.viewport_size.x.max(1.0).round() as i32);
            let layout_h = state
                .monitors
                .values()
                .map(|monitor| monitor.offset_y + monitor.height)
                .max()
                .unwrap_or(state.tuning.viewport_size.y.max(1.0).round() as i32);
            let backend_handle = TtyBackendHandle::new(layout_w, layout_h);
            for output in &drm_probe.outputs {
                state.advertise_output(output.connector_name.as_str(), output.mode.into());
            }
            info!("tty logical backend size={}x{}", layout_w, layout_h);
            {
                let mut ps = pointer_state.borrow_mut();
                // Start the cursor at the centre of the primary monitor
                // (the first configured and active output), not the centre of
                // the combined layout bounding box.  With two side-by-side
                // monitors the combined-layout centre falls on the boundary
                // between them, which makes the cursor appear stuck at the
                // edge of the main display on startup.
                let (start_sx, start_sy) = state
                    .monitors
                    .get(&state.current_monitor)
                    .map(|m| {
                        (
                            m.offset_x as f32 + m.width as f32 * 0.5,
                            m.offset_y as f32 + m.height as f32 * 0.5,
                        )
                    })
                    .unwrap_or(((layout_w as f32) * 0.5, (layout_h as f32) * 0.5));
                ps.screen = (start_sx, start_sy);
                ps.workspace_size = (layout_w, layout_h);
            }

            // Capture renderer + output surfaces before drm_probe.dev is moved out.
            let init_renderer = drm_probe.renderer.clone();
            let init_outputs: Vec<_> = drm_probe
                .outputs
                .iter()
                .map(|o| (o.connector_name.clone(), o.gbm_surface.clone()))
                .collect();

            let dev = Rc::new(RefCell::new(drm_probe.dev));
            let active_modes = Rc::new(RefCell::new(
                drm_probe
                    .outputs
                    .iter()
                    .map(|output| (output.connector_name.clone(), output.mode))
                    .collect::<HashMap<_, _>>(),
            ));
            let dpms_enabled = Rc::new(RefCell::new(true));
            publish_tty_outputs_snapshot(&dev.borrow(), &active_modes.borrow(), true, &tuning);

            // Eager initial modeset: bind + queue a blank buffer to every output
            // now so the blocking DRM commit_pending() fires here rather than
            // inside the first timer tick. Without this, dual-monitor startup
            // stalls the event loop for the full modeset duration of the slowest
            // output before anything appears on screen.
            {
                let mut renderer = init_renderer.borrow_mut();
                for (connector_name, gbm_surface) in &init_outputs {
                    let mut gbm = gbm_surface.borrow_mut();
                    match gbm.next_buffer() {
                        Ok((mut dmabuf, _age)) => match renderer.bind(&mut dmabuf) {
                            Ok(_target) => {
                                drop(_target);
                                match gbm.queue_buffer(None, None, ()) {
                                    Ok(()) => info!(
                                        "initial modeset queued for {}",
                                        connector_name
                                    ),
                                    Err(err) => warn!(
                                        "initial modeset queue_buffer failed for {}: {}",
                                        connector_name, err
                                    ),
                                }
                            }
                            Err(err) => warn!(
                                "initial modeset bind failed for {}: {}",
                                connector_name, err
                            ),
                        },
                        Err(err) => warn!(
                            "initial modeset next_buffer failed for {}: {}",
                            connector_name, err
                        ),
                    }
                }
            }

            let gbm_surfaces_for_vblank: Vec<_> = drm_probe
                .outputs
                .iter()
                .map(|output| {
                    (
                        output.crtc,
                        output.connector_name.clone(),
                        output.gbm_surface.clone(),
                    )
                })
                .collect();
            let output_frame_pending = Rc::new(RefCell::new(
                drm_probe
                    .outputs
                    .iter()
                    .map(|output| (output.connector_name.clone(), false))
                    .collect::<HashMap<_, _>>(),
            ));
            let warned_vblank_mismatch = Rc::new(RefCell::new(false));
            let warned_vblank_mismatch_for_notifier = warned_vblank_mismatch.clone();
            let output_frame_pending_for_notifier = output_frame_pending.clone();
            let first_vblank_logged = Rc::new(RefCell::new(std::collections::HashSet::<String>::new()));
            let first_vblank_logged_for_notifier = first_vblank_logged.clone();
            let dev_for_timer = dev.clone();
            let dev_for_input = dev.clone();
            let active_modes_for_timer = active_modes.clone();
            let active_modes_for_input = active_modes.clone();
            let dpms_enabled_for_timer = dpms_enabled.clone();
            let dpms_enabled_for_input = dpms_enabled.clone();
            let backend_handle_for_timer = backend_handle.clone();
            let first_frame_queued = Rc::new(RefCell::new(std::collections::HashSet::<String>::new()));
            let first_frame_queued_for_timer = first_frame_queued.clone();
            let gbm_surfaces_for_input: Vec<_> = drm_probe
                .outputs
                .iter()
                .map(|output| output.gbm_surface.clone())
                .collect();
            let gbm_surfaces_for_timer = gbm_surfaces_for_input.clone();
            ev.handle().insert_source(
                drm_probe.notifier,
                move |event, _metadata, _st| match event {
                    DrmEvent::VBlank(crtc) => {
                        let mut matched_outputs = Vec::new();
                        for (initial_crtc, output_name, gbm_surface) in &gbm_surfaces_for_vblank {
                            let live_crtc = { gbm_surface.borrow().crtc() };
                            if live_crtc != *initial_crtc {
                                debug!(
                                    "drm surface live crtc differs from initial for {}: initial={:?} live={:?}",
                                    output_name, initial_crtc, live_crtc
                                );
                            }
                            if crtc != *initial_crtc {
                                continue;
                            }
                            if let Err(err) = gbm_surface.borrow_mut().frame_submitted() {
                                warn!(
                                    "failed to mark drm frame submitted for {}: {}",
                                    output_name, err
                                );
                            }
                            output_frame_pending_for_notifier
                                .borrow_mut()
                                .insert(output_name.clone(), false);
                            matched_outputs.push(output_name.clone());
                            if first_vblank_logged_for_notifier
                                .borrow_mut()
                                .insert(output_name.clone())
                            {
                                info!("first drm vblank/frame-done observed for {}", output_name);
                            }
                        }

                        if matched_outputs.is_empty() {
                            let pending_outputs: Vec<_> = gbm_surfaces_for_vblank
                                .iter()
                                .filter_map(|(_, output_name, _)| {
                                    output_frame_pending_for_notifier
                                        .borrow()
                                        .get(output_name.as_str())
                                        .copied()
                                        .unwrap_or(false)
                                        .then_some(output_name.clone())
                                })
                                .collect();

                            let recoverable_outputs: Vec<_> = pending_outputs
                                .iter()
                                .filter(|output_name| {
                                    first_vblank_logged_for_notifier
                                        .borrow()
                                        .contains(output_name.as_str())
                                })
                                .cloned()
                                .collect();

                            if !recoverable_outputs.is_empty() {
                                if !*warned_vblank_mismatch_for_notifier.borrow() {
                                    warn!(
                                        "drm vblank crtc mismatch (got={:?}); releasing pending outputs {:?} to keep scanout advancing",
                                        crtc, recoverable_outputs
                                    );
                                    *warned_vblank_mismatch_for_notifier.borrow_mut() = true;
                                }
                                for (_, output_name, gbm_surface) in &gbm_surfaces_for_vblank {
                                    if !recoverable_outputs.iter().any(|name| name == output_name)
                                    {
                                        continue;
                                    }
                                    if let Err(err) = gbm_surface.borrow_mut().frame_submitted() {
                                        warn!(
                                            "failed to mark drm frame submitted for {} during mismatch recovery: {}",
                                            output_name, err
                                        );
                                    }
                                    output_frame_pending_for_notifier
                                        .borrow_mut()
                                        .insert(output_name.clone(), false);
                                }
                            } else if !pending_outputs.is_empty() {
                                if !*warned_vblank_mismatch_for_notifier.borrow() {
                                    warn!(
                                        "drm vblank crtc mismatch (got={:?}); keeping pending outputs {:?} blocked until they receive a real matched vblank",
                                        crtc, pending_outputs
                                    );
                                    *warned_vblank_mismatch_for_notifier.borrow_mut() = true;
                                }
                            } else if !*warned_vblank_mismatch_for_notifier.borrow() {
                                warn!(
                                    "drm vblank crtc mismatch (got={:?}); no configured output matched",
                                    crtc
                                );
                                *warned_vblank_mismatch_for_notifier.borrow_mut() = true;
                            }
                        } else if *warned_vblank_mismatch_for_notifier.borrow() {
                            info!(
                                "drm vblank crtc routing recovered on {:?} for {:?}",
                                crtc, matched_outputs
                            );
                            *warned_vblank_mismatch_for_notifier.borrow_mut() = false;
                        }
                    }
                    DrmEvent::Error(err) => warn!("drm event error: {}", err),
                },
            )?;

            let renderer_for_input = drm_probe.renderer.clone();
            ev.handle()
                .insert_source(libinput_backend, move |event, _, st| match event {
                    InputEvent::Keyboard { event } => {
                        wake_tty_dpms_on_input(
                            &gbm_surfaces_for_input,
                            &dev_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &renderer_for_input,
                            &st.tuning,
                        );
                        if !*keyboard_seen_for_input.borrow() {
                            info!("tty input: first keyboard event received");
                            *keyboard_seen_for_input.borrow_mut() = true;
                        }
                        // Smithay's libinput backend already returns XKB
                        // keycodes here (evdev + 8). Do not add another +8
                        // or compositor bindings and client key delivery will
                        // both stop matching.
                        let code: u32 = event.key_code().into();
                        let pressed = event.state() == KeyState::Pressed;
                        handle_backend_input_event(
                            st,
                            &mod_state_for_input,
                            &pointer_state_for_input,
                            &backend_handle,
                            config_path.as_str(),
                            sock_name.as_str(),
                            BackendInputEventData::Keyboard { code, pressed },
                        );
                    }
                    InputEvent::PointerMotionAbsolute { event } => {
                        wake_tty_dpms_on_input(
                            &gbm_surfaces_for_input,
                            &dev_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &renderer_for_input,
                            &st.tuning,
                        );
                        if !*pointer_seen_for_input.borrow() {
                            info!("tty input: first pointer event received");
                            *pointer_seen_for_input.borrow_mut() = true;
                        }
                        let (ws_w, ws_h) = backend_handle.window_size_i32();
                        // Map the normalised [0,1] absolute position onto the
                        // monitor the pointer device physically covers, then
                        // offset into the combined layout.  Using the full
                        // layout dimensions here would stretch [0,1] across all
                        // monitors and lock the pointer to only the leftmost one.
                        let (mon_w, mon_h, mon_ox, mon_oy) =
                            primary_tty_monitor_dims(&st.tuning);
                        let sx = mon_ox as f32 + event.x_transformed(mon_w) as f32;
                        let sy = mon_oy as f32 + event.y_transformed(mon_h) as f32;
                        handle_backend_input_event(
                            st,
                            &mod_state_for_input,
                            &pointer_state_for_input,
                            &backend_handle,
                            config_path.as_str(),
                            sock_name.as_str(),
                            BackendInputEventData::PointerMotionAbsolute {
                                ws_w,
                                ws_h,
                                sx,
                                sy,
                                delta_x: 0.0,
                                delta_y: 0.0,
                                delta_x_unaccel: 0.0,
                                delta_y_unaccel: 0.0,
                                time_usec: event.time(),
                            },
                        );
                    }
                    InputEvent::PointerMotion { event } => {
                        wake_tty_dpms_on_input(
                            &gbm_surfaces_for_input,
                            &dev_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &renderer_for_input,
                            &st.tuning,
                        );
                        if !*pointer_seen_for_input.borrow() {
                            info!("tty input: first pointer event received");
                            *pointer_seen_for_input.borrow_mut() = true;
                        }
                        let (ws_w, ws_h) = backend_handle.window_size_i32();
                        let (last_sx, last_sy) = pointer_state_for_input.borrow().screen;
                        let sx = last_sx + event.delta_x() as f32;
                        let sy = last_sy + event.delta_y() as f32;
                        handle_backend_input_event(
                            st,
                            &mod_state_for_input,
                            &pointer_state_for_input,
                            &backend_handle,
                            config_path.as_str(),
                            sock_name.as_str(),
                            BackendInputEventData::PointerMotionAbsolute {
                                ws_w,
                                ws_h,
                                sx,
                                sy,
                                delta_x: event.delta_x(),
                                delta_y: event.delta_y(),
                                delta_x_unaccel: event.delta_x_unaccel(),
                                delta_y_unaccel: event.delta_y_unaccel(),
                                time_usec: event.time(),
                            },
                        );
                    }
                    InputEvent::PointerButton { event } => {
                        wake_tty_dpms_on_input(
                            &gbm_surfaces_for_input,
                            &dev_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &renderer_for_input,
                            &st.tuning,
                        );
                        if !*pointer_seen_for_input.borrow() {
                            info!("tty input: first pointer event received");
                            *pointer_seen_for_input.borrow_mut() = true;
                        }
                        handle_backend_input_event(
                            st,
                            &mod_state_for_input,
                            &pointer_state_for_input,
                            &backend_handle,
                            config_path.as_str(),
                            sock_name.as_str(),
                            BackendInputEventData::PointerButton {
                                button_code: event.button_code(),
                                state: event.state(),
                            },
                        );
                    }
                    InputEvent::PointerAxis { event } => {
                        wake_tty_dpms_on_input(
                            &gbm_surfaces_for_input,
                            &dev_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &renderer_for_input,
                            &st.tuning,
                        );
                        if !*pointer_seen_for_input.borrow() {
                            info!("tty input: first pointer event received");
                            *pointer_seen_for_input.borrow_mut() = true;
                        }
                        handle_backend_input_event(
                            st,
                            &mod_state_for_input,
                            &pointer_state_for_input,
                            &backend_handle,
                            config_path.as_str(),
                            sock_name.as_str(),
                            BackendInputEventData::PointerAxis {
                                source: event.source(),
                                amount_v120_horizontal: event.amount_v120(Axis::Horizontal),
                                amount_v120_vertical: event.amount_v120(Axis::Vertical),
                                amount_horizontal: event.amount(Axis::Horizontal),
                                amount_vertical: event.amount(Axis::Vertical),
                                relative_direction_horizontal: event
                                    .relative_direction(Axis::Horizontal),
                                relative_direction_vertical: event
                                    .relative_direction(Axis::Vertical),
                            },
                        );
                    }
                    _ => {}
                })?;
            info!("libinput event source enabled for tty backend");

            let initial_frame_interval = frame_interval_for_refresh_hz(
                active_modes
                    .borrow()
                    .values()
                    .map(|mode| mode.vrefresh() as f64)
                    .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)),
            );
            let timer = Timer::from_duration(initial_frame_interval);
            let renderer_for_timer = drm_probe.renderer.clone();
            let outputs_for_timer: Vec<_> = drm_probe
                .outputs
                .iter()
                .map(|output| (output.connector_name.clone(), output.gbm_surface.clone()))
                .collect();

            ev.handle().insert_source(timer, move |_tick, _, st| {
                if st.take_input_state_reset_request() {
                    *mod_state_for_timer.borrow_mut() = ModState::default();
                    let mut ps = pointer_state_for_timer.borrow_mut();
                    ps.intercepted_buttons.clear();
                    ps.intercepted_binding_buttons.clear();
                    ps.intercepted_buttons.clear();
                    st.set_drag_authority_node(None);
                    ps.drag = None;
                    ps.move_anim.clear();
                    ps.panning = false;
                }
                if let Some((sx, sy)) = st.take_pointer_screen_hint_request() {
                    let mut ps = pointer_state_for_timer.borrow_mut();
                    let (ws_w, ws_h) = ps.workspace_size;
                    ps.screen = (sx, sy);
                    ps.world = crate::spatial::screen_to_world(st, ws_w.max(1), ws_h.max(1), sx, sy);
                }
                let now = Instant::now();
                st.drain_drm_syncobj_blockers();

                st.spawned_children.retain_mut(|child| {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            debug!("reaped child pid={} status={}", child.id(), status);
                            false
                        }
                        Ok(None) => true,
                        Err(err) => {
                            warn!("try_wait failed for child pid={}: {}", child.id(), err);
                            false
                        }
                    }
                });

                drain_ipc_commands(|cmd| match cmd {
                    RuntimeIpcCommand::Quit => {
                        info!("ipc: quit requested");
                        st.request_exit();
                    }
                    RuntimeIpcCommand::Reload => {
                        if let Some(next) =
                            RuntimeTuning::try_load_from_path(config_path_for_timer.as_str())
                        {
                            if crate::run::viewport_section_changed(&st.tuning, &next) {
                                apply_tty_reload(
                                    &dev_for_timer,
                                    &backend_handle_for_timer,
                                    &pointer_state_for_timer,
                                    st,
                                    next,
                                    config_path_for_timer.as_str(),
                                    wayland_display_for_timer.as_str(),
                                    "ipc",
                                    &active_modes_for_timer,
                                    *dpms_enabled_for_timer.borrow(),
                                );
                            } else {
                                let next = crate::run::preserve_viewport_section(&st.tuning, next);
                                crate::run::apply_reloaded_tuning(
                                    st,
                                    next,
                                    config_path_for_timer.as_str(),
                                    wayland_display_for_timer.as_str(),
                                    "ipc",
                                );
                            }
                        } else {
                            warn!(
                                "ipc: reload skipped for {} because config parse/load failed",
                                config_path_for_timer.as_str()
                            );
                        }
                        info!("resolved keybinds: {}", st.tuning.keybinds_resolved_summary());
                    }
                    RuntimeIpcCommand::NodeMove(direction) => {
                        let _ =
                            crate::interaction::actions::move_latest_node_direction(st, direction);
                    }
                    RuntimeIpcCommand::Trail(direction) => {
                        let _ = crate::interaction::actions::step_window_trail(st, direction);
                    }
                    RuntimeIpcCommand::Dpms(command) => {
                        apply_tty_dpms_command(
                            &gbm_surfaces_for_timer,
                            &dev_for_timer,
                            &active_modes_for_timer,
                            &dpms_enabled_for_timer,
                            command,
                            &renderer_for_timer,
                            &st.tuning,
                        );
                    }
                });

                {
                    let rx = xwayland_request_for_timer.borrow_mut();
                    while rx.try_recv().is_ok() {
                        xwayland_for_timer.borrow_mut().request_start();
                    }
                }
                xwayland_for_timer.borrow_mut().tick();

                {
                    let ps = pointer_state_for_timer.borrow();
                    let resize_active = ps.resize.is_some();
                    drop(ps);

                    st.tick_frame_effects(now);
                    st.tick_animator_frame(now);
                    st.tick_fullscreen_motion(now);
                    st.begin_render_frame(now);
                    {
                        let mut ps = pointer_state_for_timer.borrow_mut();
                        let _ = advance_node_move_anim(st, &mut ps, now);
                    }
                    st.tick_live_overlap();
                    if !resize_active {
                        st.run_maintenance_if_needed(now);
                    }
                }

                let mut reloaded = false;
                let mut rx_ref = watch_rx_for_timer.borrow_mut();
                if let Some(rx) = rx_ref.as_mut() {
                    while rx.try_recv().is_ok() {
                        *pending_watch_reload_at_for_timer.borrow_mut() =
                            Some(now + Duration::from_millis(CONFIG_RELOAD_SETTLE_MS));
                    }
                }
                if pending_watch_reload_at_for_timer
                    .borrow()
                    .is_some_and(|deadline| now >= deadline)
                {
                    *pending_watch_reload_at_for_timer.borrow_mut() = None;
                    if let Some(next) =
                        RuntimeTuning::try_load_from_path(config_path_for_timer.as_str())
                    {
                        if crate::run::viewport_section_changed(&st.tuning, &next) {
                            apply_tty_reload(
                                &dev_for_timer,
                                &backend_handle_for_timer,
                                &pointer_state_for_timer,
                                st,
                                next,
                                config_path_for_timer.as_str(),
                                wayland_display_for_timer.as_str(),
                                    "watch",
                                    &active_modes_for_timer,
                                    *dpms_enabled_for_timer.borrow(),
                                );
                            } else {
                                let next = crate::run::preserve_viewport_section(&st.tuning, next);
                                crate::run::apply_reloaded_tuning(
                                    st,
                                next,
                                config_path_for_timer.as_str(),
                                wayland_display_for_timer.as_str(),
                                "watch",
                            );
                        }
                        reloaded = true;
                    } else {
                        warn!(
                            "watch: reload skipped for {} because config parse/load failed",
                            config_path_for_timer.as_str()
                        );
                    }
                }
                if reloaded {
                    info!("resolved keybinds: {}", st.tuning.keybinds_resolved_summary());
                }

                let ps = pointer_state_for_timer.borrow();
                let resize_preview = ps.resize;
                let (hover_node, preview_hover_node) = resolve_hover_targets(st, &ps, now);
                let cursor_screen = Some(ps.screen);
                drop(ps);

                if *dpms_enabled_for_timer.borrow() {
                    let cursor_image = st.cursor_image_status.clone();
                    for (output_name, gbm_surface) in &outputs_for_timer {
                        let frame_already_pending = output_frame_pending
                            .borrow()
                            .get(output_name.as_str())
                            .copied()
                            .unwrap_or(false);
                        if frame_already_pending {
                            continue;
                        }
                        if let Err(err) = queue_tty_drm_frame(
                            output_name.as_str(),
                            gbm_surface,
                            &renderer_for_timer,
                            st,
                            resize_preview,
                            hover_node,
                            preview_hover_node,
                            cursor_screen,
                            Some(&cursor_image),
                        ) {
                            warn!("tty drm frame queue skipped for {}: {}", output_name, err);
                        } else {
                            if first_frame_queued_for_timer
                                .borrow_mut()
                                .insert(output_name.clone())
                            {
                                info!("first tty drm frame queued for {}", output_name);
                            }
                            output_frame_pending
                                .borrow_mut()
                                .insert(output_name.clone(), true);
                        }
                    }
                    st.send_frame_callbacks(now);
                }

                let secs = now.duration_since(input_started_at).as_secs();
                if secs >= 5
                    && !*keyboard_seen_for_timer.borrow()
                    && !*warned_keyboard_missing_for_timer.borrow()
                {
                    warn!(
                        "no keyboard events detected {}s after startup; keybinds will not work until keyboard input reaches libinput (seat permissions or seat mismatch)",
                        secs
                    );
                    *warned_keyboard_missing_for_timer.borrow_mut() = true;
                }
                if secs >= 5
                    && !*pointer_seen_for_timer.borrow()
                    && !*warned_pointer_missing_for_timer.borrow()
                {
                    warn!(
                        "no pointer events detected {}s after startup; pointer may be unavailable on current seat",
                        secs
                    );
                    *warned_pointer_missing_for_timer.borrow_mut() = true;
                }

                let frame_interval = frame_interval_for_refresh_hz(
                    active_modes_for_timer
                        .borrow()
                        .values()
                        .map(|mode| mode.vrefresh() as f64)
                        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)),
                );
                TimeoutAction::ToDuration(frame_interval)
            })?;

            info!("entering tty main loop");
            loop {
                ev.dispatch(None, &mut state)?;
                if state.exit_requested() || shutdown_requested() {
                    info!("exit requested, shutting down tty main loop");
                    break Ok(());
                }
                display.dispatch_clients(&mut state)?;
                display.flush_clients()?;
            }
        }
    )
}
