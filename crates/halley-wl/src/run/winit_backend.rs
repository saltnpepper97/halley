use super::*;

fn apply_host_cursor(
    backend: &Rc<RefCell<smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>>>,
    image: &smithay::input::pointer::CursorImageStatus,
) {
    let backend = backend.borrow();
    let window = backend.window();
    match image {
        smithay::input::pointer::CursorImageStatus::Hidden => {
            window.set_cursor_visible(false);
        }
        smithay::input::pointer::CursorImageStatus::Named(icon) => {
            window.set_cursor_visible(true);
            window.set_cursor(*icon);
        }
        _ => {
            window.set_cursor_visible(true);
            window.set_cursor(smithay::input::pointer::CursorIcon::Default);
        }
    }
}

pub(super) fn run_winit_backend() -> Result<(), Box<dyn Error>> {
    scope!(
        "halley-wl",
        success = "compositor exited",
        failure = "compositor failed",
        aborted = "compositor aborted",
        {
            ensure_xdg_runtime_dir()?;
            ensure_dbus_session_bus_address();
            init_logging()?;
            let _host_backend_guard = ensure_host_display()?;

            // Wayland display (clients + globals live here)
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

            // Global compositor state
            let mut state = HalleyWlState::new(&dh, tuning.clone());
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

            // Wayland listening socket
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

            // Winit backend (compositor runs in a window)
            let (backend, winit_source) = winit::init::<GlesRenderer>().map_err(|err| {
                let wayland_display =
                    env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
                let x11_display = env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
                io::Error::other(format!(
                    "failed to initialize winit backend (WAYLAND_DISPLAY={}, DISPLAY={}): {}",
                    wayland_display, x11_display, err
                ))
            })?;
            let backend = Rc::new(RefCell::new(backend));
            let backend_handle = WinitBackendHandle::new(backend.clone());
            let xwayland = Rc::new(RefCell::new(ensure_xwayland_satellite(sock_name.as_str())?));
            let (xwayland_request_tx, xwayland_request_rx) = mpsc::channel::<()>();
            register_xwayland_request_channel(xwayland_request_tx);
            let xwayland_request_rx = Rc::new(RefCell::new(xwayland_request_rx));
            let xwayland_for_timer = xwayland.clone();
            let xwayland_request_for_timer = xwayland_request_rx.clone();
            {
                // Apply runtime config exactly the same way as manual reload keybind.
                // This keeps startup baseline consistent with reload behavior.
                let fresh = RuntimeTuning::load_from_path(config_path.as_str());
                state.apply_tuning(fresh);
                let ws = backend.borrow().window_size();
                state.zoom_ref_size = halley_core::field::Vec2 {
                    x: ws.w.max(1) as f32,
                    y: ws.h.max(1) as f32,
                };
            }
            apply_host_cursor(&backend, &state.cursor_image_status);
            let backend_for_winit = backend.clone();
            let backend_for_cursor_timer = backend.clone();
            let backend_handle_for_winit = backend_handle.clone();
            let backend_handle_for_timer = backend_handle.clone();
            let config_path_for_winit = config_path.clone();
            let wayland_display_for_winit = sock_name.clone();
            let mod_state = Rc::new(RefCell::new(ModState::default()));
            let mod_state_for_winit = mod_state.clone();
            let pointer_state = Rc::new(RefCell::new(PointerState::default()));
            {
                let ws = backend.borrow().window_size();
                let mut ps = pointer_state.borrow_mut();
                ps.workspace_size = (ws.w.max(1), ws.h.max(1));
                ps.screen = ((ws.w as f32) * 0.5, (ws.h as f32) * 0.5);
            }
            let pointer_state_for_winit = pointer_state.clone();
            let pointer_state_for_timer = pointer_state.clone();
            let watch_rx = Rc::new(RefCell::new(watch_rx));
            let watch_rx_for_timer = watch_rx.clone();
            let config_path_for_timer = config_path.clone();
            let last_maintenance_at = Rc::new(RefCell::new(Instant::now()));
            let last_maintenance_for_timer = last_maintenance_at.clone();

            // calloop loop
            let mut ev: EventLoop<HalleyWlState> = EventLoop::try_new()?;
            let _signal = ev.get_signal();

            let mut dh_for_clients = dh.clone();
            ev.handle()
                .insert_source(listening, move |client_stream, _, _st| {
                    // attach per-client data (needed by CompositorHandler::client_compositor_state)
                    let _ =
                        dh_for_clients.insert_client(client_stream, Arc::new(ClientState::new()));
                })?;

            ev.handle()
                .insert_source(winit_source, move |event, _, st| {
                    match event {
                        WinitEvent::Redraw => {
                            let ps = pointer_state_for_winit.borrow();
                            let now = Instant::now();
                            const HOVER_PREVIEW_DWELL_MS: u64 = 1_500;
                            let resize_preview = ps.resize;
                            let hover_blocked = ps.preview_block_until.is_some_and(|t| now < t);
                            let hovered = if hover_blocked { None } else { ps.hover_node };
                            let preview_ready = hovered.is_some()
                                && ps.hover_started_at.is_some_and(|at| {
                                    now.duration_since(at).as_millis() as u64
                                        >= HOVER_PREVIEW_DWELL_MS
                                });
                            // Staged hover UX:
                            // 1) label-only hold, 2) preview appears while label relaxes.
                            let hover_node = if preview_ready { None } else { hovered };
                            let preview_hover_node = if preview_ready { hovered } else { None };
                            if let Err(err) = backend_handle_for_winit.draw_frame(
                                st,
                                resize_preview,
                                hover_node,
                                preview_hover_node,
                            ) {
                                debug!("draw failed: {}", err);
                            } else {
                                st.send_frame_callbacks(now);
                            }
                        }
                        WinitEvent::Resized { size, .. } => {
                            debug!("winit event: {:?}", event);
                            st.zoom_ref_size = halley_core::field::Vec2 {
                                x: size.w.max(1) as f32,
                                y: size.h.max(1) as f32,
                            };
                            {
                                let mut ps = pointer_state_for_winit.borrow_mut();
                                let (old_w, old_h) = ps.workspace_size;
                                let new_w = size.w.max(1);
                                let new_h = size.h.max(1);
                                if old_w > 0 && old_h > 0 {
                                    let sx = ps.screen.0 * (new_w as f32) / (old_w as f32);
                                    let sy = ps.screen.1 * (new_h as f32) / (old_h as f32);
                                    let max_x = (new_w - 1) as f32;
                                    let max_y = (new_h - 1) as f32;
                                    ps.screen = (sx.clamp(0.0, max_x), sy.clamp(0.0, max_y));
                                }
                                ps.workspace_size = (new_w, new_h);
                            }
                            // Keep world camera size stable from config/runtime state.
                            // Window pixel resize should not mutate world units.
                            let ps = pointer_state_for_winit.borrow();
                            let now = Instant::now();
                            const HOVER_PREVIEW_DWELL_MS: u64 = 1_500;
                            let resize_preview = ps.resize;
                            let hover_blocked = ps.preview_block_until.is_some_and(|t| now < t);
                            let hovered = if hover_blocked { None } else { ps.hover_node };
                            let preview_ready = hovered.is_some()
                                && ps.hover_started_at.is_some_and(|at| {
                                    now.duration_since(at).as_millis() as u64
                                        >= HOVER_PREVIEW_DWELL_MS
                                });
                            let hover_node = if preview_ready { None } else { hovered };
                            let preview_hover_node = if preview_ready { hovered } else { None };
                            if let Err(err) = backend_handle_for_winit.draw_frame(
                                st,
                                resize_preview,
                                hover_node,
                                preview_hover_node,
                            ) {
                                debug!("draw failed: {}", err);
                            } else {
                                st.send_frame_callbacks(now);
                            }
                        }
                        WinitEvent::Focus(false) => {
                            debug!("winit event: {:?}", event);
                            *mod_state_for_winit.borrow_mut() = ModState::default();
                            let mut ps = pointer_state_for_winit.borrow_mut();
                            // In nested development, transient focus flicker can happen while
                            // right-drag resizing. Do not forcibly cancel in-flight interactions
                            // here; let pointer release/motion handlers finalize cleanly.
                            if ps.resize.is_none() {
                                ps.drag = None;
                                ps.move_anim.clear();
                                ps.panning = false;
                            }
                            st.set_app_focused(false);
                        }
                        WinitEvent::Focus(true) => {
                            debug!("winit event: {:?}", event);
                            st.set_app_focused(true);
                            let now = Instant::now();
                            if let Some(id) = st.last_input_surface_node() {
                                st.set_interaction_focus(Some(id), 30_000, now);
                            }
                        }
                        WinitEvent::CloseRequested => {
                            debug!("winit event: {:?}", event);
                            st.request_exit();
                        }
                        WinitEvent::Input(InputEvent::Keyboard { event }) => {
                            let code = event.key_code().into();
                            let pressed = event.state() == KeyState::Pressed;
                            handle_backend_input_event(
                                st,
                                &mod_state_for_winit,
                                &pointer_state_for_winit,
                                &backend_handle_for_winit,
                                config_path_for_winit.as_str(),
                                wayland_display_for_winit.as_str(),
                                BackendInputEventData::Keyboard { code, pressed },
                            );
                        }
                        WinitEvent::Input(InputEvent::PointerMotionAbsolute { event }) => {
                            let ws = backend_for_winit.borrow().window_size();
                            let sx = event.x_transformed(ws.w) as f32;
                            let sy = event.y_transformed(ws.h) as f32;
                            handle_backend_input_event(
                                st,
                                &mod_state_for_winit,
                                &pointer_state_for_winit,
                                &backend_handle_for_winit,
                                config_path_for_winit.as_str(),
                                wayland_display_for_winit.as_str(),
                                BackendInputEventData::PointerMotionAbsolute {
                                    ws_w: ws.w,
                                    ws_h: ws.h,
                                    sx,
                                    sy,
                                },
                            );
                        }
                        WinitEvent::Input(InputEvent::PointerMotion { event }) => {
                            let ws = backend_for_winit.borrow().window_size();
                            let (last_sx, last_sy) = pointer_state_for_winit.borrow().screen;
                            let sx = last_sx
                                + smithay::backend::input::PointerMotionEvent::<
                                    smithay::backend::winit::WinitInput,
                                >::delta_x(&event) as f32;
                            let sy = last_sy
                                + smithay::backend::input::PointerMotionEvent::<
                                    smithay::backend::winit::WinitInput,
                                >::delta_y(&event) as f32;
                            handle_backend_input_event(
                                st,
                                &mod_state_for_winit,
                                &pointer_state_for_winit,
                                &backend_handle_for_winit,
                                config_path_for_winit.as_str(),
                                wayland_display_for_winit.as_str(),
                                BackendInputEventData::PointerMotionAbsolute {
                                    ws_w: ws.w,
                                    ws_h: ws.h,
                                    sx,
                                    sy,
                                },
                            );
                        }
                        WinitEvent::Input(InputEvent::PointerButton { event }) => {
                            handle_backend_input_event(
                                st,
                                &mod_state_for_winit,
                                &pointer_state_for_winit,
                                &backend_handle_for_winit,
                                config_path_for_winit.as_str(),
                                wayland_display_for_winit.as_str(),
                                BackendInputEventData::PointerButton {
                                    button_code: event.button_code(),
                                    state: event.state(),
                                },
                            );
                        }
                        WinitEvent::Input(InputEvent::PointerAxis { event }) => {
                            handle_backend_input_event(
                                st,
                                &mod_state_for_winit,
                                &pointer_state_for_winit,
                                &backend_handle_for_winit,
                                config_path_for_winit.as_str(),
                                wayland_display_for_winit.as_str(),
                                BackendInputEventData::PointerAxis {
                                    amount_v120_vertical: event.amount_v120(Axis::Vertical),
                                    amount_vertical: event.amount(Axis::Vertical),
                                },
                            );
                        }
                        _ => {}
                    }
                })?;

            // Removed optional extra direct-libinput hookup in winit backend.

            // Periodic tick: evaluate commit activity state transitions (no spam)
            let timer = Timer::from_duration(Duration::from_millis(16));
            ev.handle().insert_source(timer, move |_tick, _, st| {
                let now = Instant::now();
                {
                    let rx = xwayland_request_for_timer.borrow_mut();
                    while rx.try_recv().is_ok() {
                        xwayland_for_timer.borrow_mut().request_start();
                    }
                }
                xwayland_for_timer.borrow_mut().tick();
                let resize_active = {
                    let ps = pointer_state_for_timer.borrow();
                    ps.resize.is_some()
                };
                st.tick_overview_frame(now);
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

                if st.tuning.debug_tick_dump {
                    for (sid, act) in st.surface_activity.iter_mut() {
                        if let Some((new_state, cps)) = act.tick(now, true) {
                            match new_state {
                                VisualState::Active => {
                                    info!("visual active surface={} cps={:.1}", sid, cps)
                                }
                                VisualState::Fading => {
                                    debug!("visual fading surface={} cps={:.1}", sid, cps)
                                }
                                VisualState::Inactive => info!("visual inactive surface={}", sid),
                            }
                        }
                    }
                }
                {
                    let mut ps = pointer_state_for_timer.borrow_mut();
                    if let (Some(id), Some(until)) = (ps.resize_trace_node, ps.resize_trace_until) {
                        if now >= until {
                            ps.resize_trace_node = None;
                            ps.resize_trace_until = None;
                            ps.resize_trace_last_at = None;
                        } else {
                            let due = ps
                                .resize_trace_last_at
                                .is_none_or(|at| now.duration_since(at).as_millis() as u64 >= 120);
                            if due {
                                if let Some(n) = st.field.node(id) {
                                    let surf = current_surface_size_for_node(st, id);
                                    info!(
                                        "resize-trace id={} pos=({:.1},{:.1}) intrinsic=({:.1},{:.1}) surface={:?} state={:?}",
                                        id.as_u64(),
                                        n.pos.x,
                                        n.pos.y,
                                        n.intrinsic_size.x,
                                        n.intrinsic_size.y,
                                        surf.map(|v| (v.x, v.y)),
                                        n.state,
                                    );
                                } else {
                                    info!("resize-trace id={} missing-node", id.as_u64());
                                }
                                ps.resize_trace_last_at = Some(now);
                            }
                        }
                    }
                }

                apply_host_cursor(&backend_for_cursor_timer, &st.cursor_image_status);
                backend_handle_for_timer.request_redraw();
                TimeoutAction::ToDuration(Duration::from_millis(16))
            })?;

            info!("entering main loop");

            loop {
                // 1) run calloop sources (socket, winit, timer, etc.)
                ev.dispatch(None, &mut state)?;

                if state.exit_requested() {
                    info!("exit requested, shutting down main loop");
                    break Ok(());
                }

                // 2) dispatch wayland clients
                display.dispatch_clients(&mut state)?;

                // 3) flush outgoing events
                display.flush_clients()?;
            }
        }
    )
}
