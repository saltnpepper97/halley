use super::*;

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
    active_connector_name: &str,
    active_mode: drm_control::Mode,
    dpms_enabled: bool,
) {
    let mut outputs = collect_outputs_for_ipc(dev, active_connector_name, active_mode);
    if !dpms_enabled {
        for output in &mut outputs {
            if output.name == active_connector_name {
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
    gbm_surface: &Rc<RefCell<GbmBufferedSurface<GbmAllocator<DeviceFd>, ()>>>,
    dev: &Rc<RefCell<DrmDevice>>,
    current_connector_name: &Rc<RefCell<String>>,
    current_mode: &Rc<RefCell<drm_control::Mode>>,
    dpms_enabled: &Rc<RefCell<bool>>,
    command: halley_ipc::DpmsCommand,
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
        let result = gbm_surface.borrow().surface().clear();
        match result {
            Ok(()) => {
                *dpms_enabled.borrow_mut() = false;
                info!(
                    "tty dpms: powered off connector {}",
                    current_connector_name.borrow().as_str()
                );
            }
            Err(err) => {
                warn!("tty dpms off failed: {}", err);
                return;
            }
        }
    } else {
        *dpms_enabled.borrow_mut() = true;
        info!(
            "tty dpms: powering on connector {}",
            current_connector_name.borrow().as_str()
        );
    }

    publish_tty_outputs_snapshot(
        &dev.borrow(),
        current_connector_name.borrow().as_str(),
        *current_mode.borrow(),
        *dpms_enabled.borrow(),
    );
}

fn wake_tty_dpms_on_input(
    gbm_surface: &Rc<RefCell<GbmBufferedSurface<GbmAllocator<DeviceFd>, ()>>>,
    dev: &Rc<RefCell<DrmDevice>>,
    current_connector_name: &Rc<RefCell<String>>,
    current_mode: &Rc<RefCell<drm_control::Mode>>,
    dpms_enabled: &Rc<RefCell<bool>>,
) {
    if *dpms_enabled.borrow() {
        return;
    }
    apply_tty_dpms_command(
        gbm_surface,
        dev,
        current_connector_name,
        current_mode,
        dpms_enabled,
        halley_ipc::DpmsCommand::On,
    );
}

fn apply_tty_reload(
    dev: &Rc<RefCell<DrmDevice>>,
    gbm_surface: &Rc<RefCell<GbmBufferedSurface<GbmAllocator<DeviceFd>, ()>>>,
    backend_handle: &TtyBackendHandle,
    pointer_state: &Rc<RefCell<PointerState>>,
    st: &mut HalleyWlState,
    next: RuntimeTuning,
    config_path: &str,
    wayland_display: &str,
    reason: &str,
    current_connector_name: &Rc<RefCell<String>>,
    current_mode: &Rc<RefCell<drm_control::Mode>>,
    current_crtc: drm_control::crtc::Handle,
    dpms_enabled: bool,
) {
    let (target_crtc, target_mode, _target_connector, target_connector_name) = {
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

    if target_crtc != current_crtc || target_connector_name != *current_connector_name.borrow() {
        warn!(
            "{}: live tty viewport reload only supports the current connector/crtc (wanted connector={}, current connector={}); keeping last working mode",
            reason,
            target_connector_name,
            current_connector_name.borrow().as_str()
        );
        return;
    }

    let previous_mode = *current_mode.borrow();
    {
        let mut surface = gbm_surface.borrow_mut();
        if let Err(err) = surface.use_mode(target_mode) {
            warn!(
                "{}: tty mode apply failed for {}: {}; keeping last working mode",
                reason, config_path, err
            );
            let _ = surface.use_mode(previous_mode);
            return;
        }
        surface.reset_buffers();
    }

    let (mw, mh) = target_mode.size();
    backend_handle.set_size(mw as i32, mh as i32);
    {
        let mut ps = pointer_state.borrow_mut();
        let old = ps.workspace_size;
        ps.workspace_size = (mw as i32, mh as i32);
        if old.0 > 0 && old.1 > 0 {
            let sx = ps.screen.0 * (mw as f32) / (old.0 as f32);
            let sy = ps.screen.1 * (mh as f32) / (old.1 as f32);
            ps.screen = (
                sx.clamp(0.0, (mw.saturating_sub(1)) as f32),
                sy.clamp(0.0, (mh.saturating_sub(1)) as f32),
            );
        }
    }

    *current_mode.borrow_mut() = target_mode;
    let mut next = next;
    next.viewport_size = halley_core::field::Vec2 {
        x: mw as f32,
        y: mh as f32,
    };
    let live_camera = crate::run::capture_live_camera_state(st);
    st.apply_tuning(next);
    crate::run::restore_live_camera_state(st, live_camera);
    st.advertise_primary_output(current_connector_name.borrow().as_str(), target_mode.into());
    publish_tty_outputs_snapshot(
        &dev.borrow(),
        current_connector_name.borrow().as_str(),
        target_mode,
        dpms_enabled,
    );
    let reload_commands = st.tuning.autostart_on_reload.clone();
    run_autostart_commands(st, &reload_commands, wayland_display, "autostart");
    info!(
        "{}: reloaded config from {} with tty mode {}x{} on {}",
        reason,
        config_path,
        mw,
        mh,
        current_connector_name.borrow().as_str()
    );
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

            let mut state = HalleyWlState::new(&dh, tuning.clone());
            let dmabuf_importer: Rc<dyn DmabufImportBackend> =
                Rc::new(TtyDmabufImportBackend::new(drm_probe.renderer.clone()));
            state.configure_dmabuf_importer_for_fd(dmabuf_importer, drm_probe.dev.device_fd());
            state.set_app_focused(true);
            state.seat.add_pointer();
            if state
                .seat
                .add_keyboard(Default::default(), 200, 30)
                .is_err()
            {
                warn!("failed to initialize wl_seat keyboard");
            }

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
            let autostart_once = state.tuning.autostart_once.clone();
            run_autostart_commands(&mut state, &autostart_once, sock_name.as_str(), "autostart");

            let libinput_backend = libinput_backend;
            let debug_input = crate::input::pointer_map_debug_enabled();

            let mut ev: EventLoop<HalleyWlState> = EventLoop::try_new()?;
            let _signal = ev.get_signal();

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
            let (mw, mh) = drm_probe.mode.size();
            let backend_handle = TtyBackendHandle::new(mw as i32, mh as i32);
            state.zoom_ref_size = halley_core::field::Vec2 {
                x: mw.max(1) as f32,
                y: mh.max(1) as f32,
            };
            state.snap_camera_targets_to_live();
            state
                .advertise_primary_output(drm_probe.connector_name.as_str(), drm_probe.mode.into());
            info!("tty logical backend size={}x{}", mw, mh);
            {
                let mut ps = pointer_state.borrow_mut();
                ps.screen = ((mw as f32) * 0.5, (mh as f32) * 0.5);
                ps.workspace_size = (mw as i32, mh as i32);
            }

            let dev = Rc::new(RefCell::new(drm_probe.dev));
            let current_connector_name = Rc::new(RefCell::new(drm_probe.connector_name.clone()));
            let current_mode = Rc::new(RefCell::new(drm_probe.mode));
            let dpms_enabled = Rc::new(RefCell::new(true));
            publish_tty_outputs_snapshot(
                &dev.borrow(),
                current_connector_name.borrow().as_str(),
                *current_mode.borrow(),
                true,
            );

            let drm_crtc = drm_probe.crtc;
            let gbm_surface_for_vblank = drm_probe.gbm_surface.clone();
            let warned_vblank_mismatch = Rc::new(RefCell::new(false));
            let warned_vblank_mismatch_for_notifier = warned_vblank_mismatch.clone();
            let dev_for_timer = dev.clone();
            let dev_for_input = dev.clone();
            let current_connector_name_for_timer = current_connector_name.clone();
            let current_connector_name_for_input = current_connector_name.clone();
            let current_mode_for_timer = current_mode.clone();
            let current_mode_for_input = current_mode.clone();
            let dpms_enabled_for_timer = dpms_enabled.clone();
            let dpms_enabled_for_input = dpms_enabled.clone();
            let backend_handle_for_timer = backend_handle.clone();
            let gbm_surface_for_input = drm_probe.gbm_surface.clone();
            ev.handle().insert_source(
                drm_probe.notifier,
                move |event, _metadata, _st| match event {
                    DrmEvent::VBlank(crtc) => {
                        let expected_crtc = { gbm_surface_for_vblank.borrow().crtc() };
                        if crtc != expected_crtc {
                            if !*warned_vblank_mismatch_for_notifier.borrow() {
                                warn!(
                                    "drm vblank crtc mismatch (expected={:?}, got={:?}; initial={:?}); accepting event to keep scanout advancing",
                                    expected_crtc, crtc, drm_crtc
                                );
                                *warned_vblank_mismatch_for_notifier.borrow_mut() = true;
                            }
                        } else if *warned_vblank_mismatch_for_notifier.borrow() {
                            info!("drm vblank crtc routing recovered on {:?}", crtc);
                            *warned_vblank_mismatch_for_notifier.borrow_mut() = false;
                        }
                        if let Err(err) = gbm_surface_for_vblank.borrow_mut().frame_submitted() {
                            warn!("failed to mark drm frame submitted: {}", err);
                        }
                    }
                    DrmEvent::Error(err) => warn!("drm event error: {}", err),
                },
            )?;

            let initial_cursor = Some(pointer_state.borrow().screen);
            let initial_cursor_image = state.cursor_image_status.clone();
            let initial_resize_preview = pointer_state.borrow().resize;
            if let Err(err) = queue_tty_drm_frame(
                &drm_probe.gbm_surface,
                &drm_probe.renderer,
                &mut state,
                initial_resize_preview,
                None,
                None,
                initial_cursor,
                Some(&initial_cursor_image),
            ) {
                warn!("initial tty drm frame queue failed: {}", err);
            }

            ev.handle()
                .insert_source(libinput_backend, move |event, _, st| match event {
                    InputEvent::Keyboard { event } => {
                        wake_tty_dpms_on_input(
                            &gbm_surface_for_input,
                            &dev_for_input,
                            &current_connector_name_for_input,
                            &current_mode_for_input,
                            &dpms_enabled_for_input,
                        );
                        if !*keyboard_seen_for_input.borrow() {
                            info!("tty input: first keyboard event received");
                            *keyboard_seen_for_input.borrow_mut() = true;
                        }
                        let code: u32 = event.key_code().into();
                        let pressed = event.state() == KeyState::Pressed;
                        if debug_input {
                            info!("tty input keyboard code={} pressed={}", code, pressed);
                        }
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
                            &gbm_surface_for_input,
                            &dev_for_input,
                            &current_connector_name_for_input,
                            &current_mode_for_input,
                            &dpms_enabled_for_input,
                        );
                        if !*pointer_seen_for_input.borrow() {
                            info!("tty input: first pointer event received");
                            *pointer_seen_for_input.borrow_mut() = true;
                        }
                        let (ws_w, ws_h) = backend_handle.window_size_i32();
                        let sx = event.x_transformed(ws_w) as f32;
                        let sy = event.y_transformed(ws_h) as f32;
                        if debug_input {
                            info!(
                                "ptr-map abs raw=({:.4},{:.4}) ws={}x{} -> screen=({:.2},{:.2})",
                                event.x(),
                                event.y(),
                                ws_w,
                                ws_h,
                                sx,
                                sy
                            );
                        }
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
                            &gbm_surface_for_input,
                            &dev_for_input,
                            &current_connector_name_for_input,
                            &current_mode_for_input,
                            &dpms_enabled_for_input,
                        );
                        if !*pointer_seen_for_input.borrow() {
                            info!("tty input: first pointer event received");
                            *pointer_seen_for_input.borrow_mut() = true;
                        }
                        let (ws_w, ws_h) = backend_handle.window_size_i32();
                        let (last_sx, last_sy) = pointer_state_for_input.borrow().screen;
                        let sx = last_sx + event.delta_x() as f32;
                        let sy = last_sy + event.delta_y() as f32;
                        if debug_input {
                            info!(
                                "ptr-map rel delta=({:.3},{:.3}) last=({:.2},{:.2}) ws={}x{} -> screen=({:.2},{:.2})",
                                event.delta_x(),
                                event.delta_y(),
                                last_sx,
                                last_sy,
                                ws_w,
                                ws_h,
                                sx,
                                sy
                            );
                        }
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
                            &gbm_surface_for_input,
                            &dev_for_input,
                            &current_connector_name_for_input,
                            &current_mode_for_input,
                            &dpms_enabled_for_input,
                        );
                        if !*pointer_seen_for_input.borrow() {
                            info!("tty input: first pointer event received");
                            *pointer_seen_for_input.borrow_mut() = true;
                        }
                        if debug_input {
                            info!(
                                "tty input pointer-button code={} state={:?}",
                                event.button_code(),
                                event.state(),
                            );
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
                            &gbm_surface_for_input,
                            &dev_for_input,
                            &current_connector_name_for_input,
                            &current_mode_for_input,
                            &dpms_enabled_for_input,
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
                                amount_v120_vertical: event.amount_v120(Axis::Vertical),
                                amount_vertical: event.amount(Axis::Vertical),
                            },
                        );
                    }
                    _ => {}
                })?;
            info!("libinput event source enabled for tty backend");

            let initial_frame_interval =
                frame_interval_for_refresh_hz(Some(current_mode.borrow().vrefresh() as f64));
            let timer = Timer::from_duration(initial_frame_interval);
            let gbm_surface_for_timer = drm_probe.gbm_surface.clone();
            let renderer_for_timer = drm_probe.renderer.clone();

            ev.handle().insert_source(timer, move |_tick, _, st| {
                let now = Instant::now();

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
                                    &gbm_surface_for_timer,
                                    &backend_handle_for_timer,
                                    &pointer_state_for_timer,
                                    st,
                                    next,
                                    config_path_for_timer.as_str(),
                                    wayland_display_for_timer.as_str(),
                                    "ipc",
                                    &current_connector_name_for_timer,
                                    &current_mode_for_timer,
                                    drm_crtc,
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
                            &gbm_surface_for_timer,
                            &dev_for_timer,
                            &current_connector_name_for_timer,
                            &current_mode_for_timer,
                            &dpms_enabled_for_timer,
                            command,
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
                                &gbm_surface_for_timer,
                                &backend_handle_for_timer,
                                &pointer_state_for_timer,
                                st,
                                next,
                                config_path_for_timer.as_str(),
                                wayland_display_for_timer.as_str(),
                                    "watch",
                                    &current_connector_name_for_timer,
                                    &current_mode_for_timer,
                                    drm_crtc,
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
                    if let Err(err) = queue_tty_drm_frame(
                        &gbm_surface_for_timer,
                        &renderer_for_timer,
                        st,
                        resize_preview,
                        hover_node,
                        preview_hover_node,
                        cursor_screen,
                        Some(&cursor_image),
                    ) {
                        warn!("tty drm frame queue skipped: {}", err);
                    } else {
                        st.send_frame_callbacks(now);
                    }
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

                let frame_interval =
                    frame_interval_for_refresh_hz(Some(current_mode_for_timer.borrow().vrefresh() as f64));
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
