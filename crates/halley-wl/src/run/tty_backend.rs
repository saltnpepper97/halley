use super::*;

use crate::backend_iface::{DmabufImportBackend, TtyDmabufImportBackend};
use crate::run::drm::{collect_outputs_for_ipc, queue_tty_drm_frame};
use crate::run::{build_tty_libinput_backend, probe_tty_drm_device_via_session};
use calloop::{Interest, Mode, PostAction, generic::Generic};

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent,
    PointerButtonEvent, PointerMotionEvent,
};

pub(super) fn run_tty_backend() -> Result<(), Box<dyn Error>> {
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
            if tuning.dev_enabled {
                info!("dev actions enabled: ring tuning + node move (via configured keybinds)");
            }

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
            let config_path_for_timer = config_path.clone();
            let last_maintenance_at = Rc::new(RefCell::new(Instant::now()));
            let last_maintenance_for_timer = last_maintenance_at.clone();
            let (mw, mh) = drm_probe.mode.size();
            let backend_handle = TtyBackendHandle {
                width: mw as i32,
                height: mh as i32,
            };
            state.zoom_ref_size = halley_core::field::Vec2 {
                x: backend_handle.width.max(1) as f32,
                y: backend_handle.height.max(1) as f32,
            };
            state
                .advertise_primary_output(drm_probe.connector_name.as_str(), drm_probe.mode.into());
            info!(
                "tty logical backend size={}x{}",
                backend_handle.width, backend_handle.height
            );
            {
                let mut ps = pointer_state.borrow_mut();
                ps.screen = (
                    (backend_handle.width as f32) * 0.5,
                    (backend_handle.height as f32) * 0.5,
                );
                ps.workspace_size = (backend_handle.width, backend_handle.height);
            }

            let initial_outputs = collect_outputs_for_ipc(
                &drm_probe.dev,
                drm_probe.connector_name.as_str(),
                drm_probe.mode,
            );
            publish_outputs(initial_outputs);

            let drm_crtc = drm_probe.crtc;
            let gbm_surface_for_vblank = drm_probe.gbm_surface.clone();
            let warned_vblank_mismatch = Rc::new(RefCell::new(false));
            let warned_vblank_mismatch_for_notifier = warned_vblank_mismatch.clone();
            ev.handle().insert_source(
                drm_probe.notifier,
                move |event, metadata, _st| match event {
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
                        if let Some(m) = metadata {
                            debug!("drm vblank seq={} crtc={:?}", m.sequence, crtc);
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
                initial_cursor,
                Some(&initial_cursor_image),
            ) {
                warn!("initial tty drm frame queue failed: {}", err);
            }

            ev.handle()
                .insert_source(libinput_backend, move |event, _, st| match event {
                    InputEvent::Keyboard { event } => {
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
                            BackendInputEventData::PointerMotionAbsolute { ws_w, ws_h, sx, sy },
                        );
                    }
                    InputEvent::PointerMotion { event } => {
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
                            BackendInputEventData::PointerMotionAbsolute { ws_w, ws_h, sx, sy },
                        );
                    }
                    InputEvent::PointerButton { event } => {
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

            let timer = Timer::from_duration(Duration::from_millis(16));
            let gbm_surface_for_timer = drm_probe.gbm_surface.clone();
            let renderer_for_timer = drm_probe.renderer.clone();

            ev.handle().insert_source(timer, move |_tick, _, st| {
                let now = Instant::now();

                drain_ipc_commands(|cmd| match cmd {
                    RuntimeIpcCommand::Quit => {
                        info!("ipc: quit requested");
                        st.request_exit();
                    }
                    RuntimeIpcCommand::Reload => {
                        let next = RuntimeTuning::load_from_path(config_path_for_timer.as_str());
                        st.apply_tuning(next);
                        info!("ipc: reloaded config from {}", config_path_for_timer.as_str());
                        info!("resolved keybinds: {}", st.tuning.keybinds_resolved_summary());
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
                    {
                        let mut ps = pointer_state_for_timer.borrow_mut();
                        let _ = advance_node_move_anim(st, &mut ps, now);
                    }
                    {
                        let mut last = last_maintenance_for_timer.borrow_mut();
                        if !resize_active
                            && now.duration_since(*last).as_millis() as u64 >= st.tuning.tick_ms
                        {
                            st.tick_maintenance(now);
                            *last = now;
                        }
                    }
                }

                let mut reloaded = false;
                let mut rx_ref = watch_rx_for_timer.borrow_mut();
                if let Some(rx) = rx_ref.as_mut() {
                    while rx.try_recv().is_ok() {
                        let next = RuntimeTuning::load_from_path(config_path_for_timer.as_str());
                        st.apply_tuning(next);
                        reloaded = true;
                    }
                }
                if reloaded {
                    info!("reloaded config from {}", config_path_for_timer.as_str());
                    info!("resolved keybinds: {}", st.tuning.keybinds_resolved_summary());
                }

                let ps = pointer_state_for_timer.borrow();
                let resize_preview = ps.resize;
                let cursor_screen = Some(ps.screen);
                drop(ps);

                let cursor_image = st.cursor_image_status.clone();
                if let Err(err) = queue_tty_drm_frame(
                    &gbm_surface_for_timer,
                    &renderer_for_timer,
                    st,
                    resize_preview,
                    cursor_screen,
                    Some(&cursor_image),
                ) {
                    warn!("tty drm frame queue skipped: {}", err);
                } else {
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

                TimeoutAction::ToDuration(Duration::from_millis(16))
            })?;

            info!("entering tty main loop");
            loop {
                ev.dispatch(None, &mut state)?;
                if state.exit_requested() {
                    info!("exit requested, shutting down tty main loop");
                    break Ok(());
                }
                display.dispatch_clients(&mut state)?;
                display.flush_clients()?;
            }
        }
    )
}
