use super::*;

use crate::input::ctx::InputCtx;
use crate::protocol::wayland::portal;

use crate::backend::interface::{
    BackendView, DmabufImportBackend, RenderBackend, WinitBackendHandle,
};
use crate::compositor::interaction::PointerState;
use calloop::{Interest, Mode, PostAction, generic::Generic};
use halley_ipc::{LogicalOutputInfo, ModeInfo, OutputInfo, OutputStatus};

const CONFIG_RELOAD_SETTLE_MS: u64 = 100;

struct WinitOutputCaptureBackend {
    backend: Rc<RefCell<smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>>>,
    pointer_state: Rc<RefCell<PointerState>>,
    dmabuf_formats: Vec<smithay::backend::allocator::Format>,
}

impl WinitOutputCaptureBackend {
    fn new(
        backend: Rc<RefCell<smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>>>,
        pointer_state: Rc<RefCell<PointerState>>,
        dmabuf_formats: Vec<smithay::backend::allocator::Format>,
    ) -> Self {
        Self {
            backend,
            pointer_state,
            dmabuf_formats,
        }
    }
}

impl portal::OutputCaptureBackend for WinitOutputCaptureBackend {
    fn capture_dmabuf_formats(&self) -> Vec<smithay::backend::allocator::Format> {
        self.dmabuf_formats.clone()
    }

    fn capture_output_shm(
        &self,
        st: &mut Halley,
        output_name: &str,
        overlay_cursor: bool,
        logical_region: Option<smithay::utils::Rectangle<i32, smithay::utils::Logical>>,
    ) -> Result<portal::ShmCaptureFrame, Box<dyn Error>> {
        let mut backend = self
            .backend
            .try_borrow_mut()
            .map_err(|_| io::Error::other("winit renderer already borrowed during screencopy"))?;
        let size = backend.window_size();
        let physical_size: smithay::utils::Size<i32, smithay::utils::Physical> =
            (size.w.max(1), size.h.max(1)).into();
        let ps = self
            .pointer_state
            .try_borrow()
            .map_err(|_| io::Error::other("pointer state already borrowed during screencopy"))?;
        let now = Instant::now();
        let resize_preview = ps.resize;
        let (hover_node, preview_hover_node) = resolve_hover_targets(st, &ps, now);
        let cursor_screen = overlay_cursor.then_some(ps.screen);
        drop(ps);

        portal::capture_output_via_renderer(
            backend.renderer(),
            st,
            output_name,
            physical_size,
            st.output_transform_for(output_name),
            resize_preview,
            hover_node,
            preview_hover_node,
            cursor_screen,
            overlay_cursor,
            logical_region,
        )
    }

    fn capture_output_dmabuf(
        &self,
        st: &mut Halley,
        output_name: &str,
        overlay_cursor: bool,
        logical_region: Option<smithay::utils::Rectangle<i32, smithay::utils::Logical>>,
        dmabuf: &mut smithay::backend::allocator::dmabuf::Dmabuf,
    ) -> Result<crate::backend::interface::CaptureDmabufResult, Box<dyn Error>> {
        let mut backend = self.backend.try_borrow_mut().map_err(|_| {
            io::Error::other("winit renderer already borrowed during dma-buf screencopy")
        })?;
        let size = backend.window_size();
        let physical_size: smithay::utils::Size<i32, smithay::utils::Physical> =
            (size.w.max(1), size.h.max(1)).into();
        let ps = self.pointer_state.try_borrow().map_err(|_| {
            io::Error::other("pointer state already borrowed during dma-buf screencopy")
        })?;
        let now = Instant::now();
        let resize_preview = ps.resize;
        let (hover_node, preview_hover_node) = resolve_hover_targets(st, &ps, now);
        let cursor_screen = overlay_cursor.then_some(ps.screen);
        drop(ps);

        portal::capture_output_into_dmabuf_via_renderer(
            backend.renderer(),
            st,
            output_name,
            physical_size,
            st.output_transform_for(output_name),
            resize_preview,
            hover_node,
            preview_hover_node,
            cursor_screen,
            overlay_cursor,
            logical_region,
            dmabuf,
        )
    }
}

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

fn publish_winit_output_snapshot(
    width: i32,
    height: i32,
    focused: bool,
    offset_x: i32,
    offset_y: i32,
) {
    let width = width.max(0) as u32;
    let height = height.max(0) as u32;

    publish_outputs(vec![OutputInfo {
        name: "winit-0".to_string(),
        status: OutputStatus::Connected,
        enabled: true,
        current_mode: Some(ModeInfo {
            width,
            height,
            refresh_hz: None,
            preferred: true,
            current: true,
        }),
        modes: vec![ModeInfo {
            width,
            height,
            refresh_hz: None,
            preferred: true,
            current: true,
        }],
        vrr_mode: None,
        vrr_support: None,
        direct_scanout_candidate_node: None,
        direct_scanout_active_node: None,
        direct_scanout_reason: None,
        logical: Some(LogicalOutputInfo {
            scale: 1.0,
            focused,
            offset_x,
            offset_y,
        }),
    }]);
}

fn apply_winit_reload(
    backend: &Rc<RefCell<smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>>>,
    st: &mut Halley,
    mut next: RuntimeTuning,
    config_path: &str,
    wayland_display: &str,
    reason: &str,
) {
    let ws = backend.borrow().window_size();
    next.viewport_size = halley_core::field::Vec2 {
        x: ws.w.max(1) as f32,
        y: ws.h.max(1) as f32,
    };
    let live_camera = crate::bootstrap::capture_live_camera_state(st);
    st.apply_tuning(next);
    crate::bootstrap::restore_live_camera_state(st, live_camera);
    st.advertise_output(
        "winit-0",
        smithay::output::Mode {
            size: (ws.w.max(1), ws.h.max(1)).into(),
            refresh: 0,
        },
    );
    let reload_commands = st.runtime.tuning.autostart_on_reload.clone();
    run_autostart_commands(st, &reload_commands, wayland_display, "autostart");
    info!(
        "{}: reloaded config from {} with viewport {}x{}",
        reason,
        config_path,
        ws.w.max(1),
        ws.h.max(1)
    );
}

pub(crate) fn run_winit_backend() -> Result<(), Box<dyn Error>> {
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

            let mut display: Display<Halley> = Display::new()?;
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
            info!("config path: {}", config_path.as_str());
            info!("keybind modifier: {}", tuning.keybinds.modifier_name());
            info!("resolved keybinds: {}", tuning.keybinds_resolved_summary());
            info!("resolved zoom: {}", tuning.zoom_resolved_summary());

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

            let (backend, winit_source) = smithay_winit::init::<GlesRenderer>().map_err(|err| {
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
            let mut ev: EventLoop<Halley> = EventLoop::try_new()?;
            let _signal = ev.get_signal();
            let mut state = Halley::new(&dh, ev.handle(), tuning.clone());
            state.platform.seat.add_pointer();
            if state
                .platform
                .seat
                .add_keyboard(Default::default(), 200, 30)
                .is_err()
            {
                warn!("failed to initialize wl_seat keyboard");
            }
            let dmabuf_importer: Rc<dyn DmabufImportBackend> = Rc::new(backend_handle.clone());
            state.configure_dmabuf_importer(dmabuf_importer, None);
            let xwayland = Rc::new(RefCell::new(ensure_xwayland_satellite(sock_name.as_str())?));
            let (xwayland_request_tx, xwayland_request_rx) = mpsc::channel::<()>();
            register_xwayland_request_channel(xwayland_request_tx);
            let xwayland_request_rx = Rc::new(RefCell::new(xwayland_request_rx));
            let xwayland_for_timer = xwayland.clone();
            let xwayland_request_for_timer = xwayland_request_rx.clone();
            {
                let mut fresh = RuntimeTuning::load_from_path(config_path.as_str());
                let ws = backend.borrow().window_size();
                fresh.viewport_size = halley_core::field::Vec2 {
                    x: ws.w.max(1) as f32,
                    y: ws.h.max(1) as f32,
                };
                state.apply_tuning(fresh);
                state.model.zoom_ref_size = halley_core::field::Vec2 {
                    x: ws.w.max(1) as f32,
                    y: ws.h.max(1) as f32,
                };
                state.snap_camera_targets_to_live();
                state.advertise_output(
                    "winit-0",
                    smithay::output::Mode {
                        size: (ws.w.max(1), ws.h.max(1)).into(),
                        refresh: 0,
                    },
                );
            }
            let autostart_once = state.runtime.tuning.autostart_once.clone();
            run_autostart_commands(&mut state, &autostart_once, sock_name.as_str(), "autostart");
            apply_host_cursor(&backend, &state.effective_cursor_image_status());
            let backend_for_winit = backend.clone();
            let backend_for_timer = backend.clone();
            let backend_for_cursor_timer = backend.clone();
            let backend_for_output_timer = backend.clone();
            let backend_handle_for_winit = backend_handle.clone();
            let backend_handle_for_timer = backend_handle.clone();
            let config_path_for_winit = config_path.clone();
            let wayland_display_for_winit = sock_name.clone();
            let wayland_display_for_timer = sock_name.clone();
            let mod_state = Rc::new(RefCell::new(ModState::default()));
            let mod_state_for_winit = mod_state.clone();
            let pointer_state = Rc::new(RefCell::new(PointerState::default()));
            let capture_dmabuf_formats = {
                let mut backend_ref = backend.borrow_mut();
                <GlesRenderer as smithay::backend::renderer::Bind<
                    smithay::backend::allocator::dmabuf::Dmabuf,
                >>::supported_formats(backend_ref.renderer())
                .map(|formats| formats.iter().copied().collect())
                .unwrap_or_default()
            };
            portal::configure_output_capture_backend(
                &mut state,
                Rc::new(WinitOutputCaptureBackend::new(
                    backend.clone(),
                    pointer_state.clone(),
                    capture_dmabuf_formats,
                )),
            );
            let mod_state_for_timer = mod_state.clone();
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
            let pending_watch_reload_at = Rc::new(RefCell::new(None::<Instant>));
            let pending_watch_reload_at_for_timer = pending_watch_reload_at.clone();
            let config_path_for_timer = config_path.clone();
            {
                let ws = backend.borrow().window_size();
                publish_winit_output_snapshot(ws.w, ws.h, true, 0, 0);
            }

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

            ev.handle()
                .insert_source(winit_source, move |event, _, st| match event {
                    WinitEvent::Redraw => {
                        let ps = pointer_state_for_winit.borrow();
                        let now = Instant::now();
                        let resize_preview = ps.resize;
                        let (hover_node, preview_hover_node) = resolve_hover_targets(st, &ps, now);
                        if let Err(err) = backend_handle_for_winit.draw_frame(
                            st,
                            resize_preview,
                            hover_node,
                            preview_hover_node,
                        ) {
                            debug!("draw failed: {}", err);
                        } else {
                            crate::render::send_frame_callbacks(st, now);
                        }
                    }
                    WinitEvent::Resized { size, .. } => {
                        debug!("winit event: {:?}", event);
                        st.model.zoom_ref_size = halley_core::field::Vec2 {
                            x: size.w.max(1) as f32,
                            y: size.h.max(1) as f32,
                        };
                        st.snap_camera_targets_to_live();
                        st.advertise_output(
                            "winit-0",
                            smithay::output::Mode {
                                size: (size.w.max(1), size.h.max(1)).into(),
                                refresh: 0,
                            },
                        );
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
                        let ps = pointer_state_for_winit.borrow();
                        let now = Instant::now();
                        let resize_preview = ps.resize;
                        let (hover_node, preview_hover_node) = resolve_hover_targets(st, &ps, now);
                        if let Err(err) = backend_handle_for_winit.draw_frame(
                            st,
                            resize_preview,
                            hover_node,
                            preview_hover_node,
                        ) {
                            debug!("draw failed: {}", err);
                        } else {
                            crate::render::send_frame_callbacks(st, now);
                        }
                    }
                    WinitEvent::Focus(false) => {
                        debug!("winit event: {:?}", event);
                        *mod_state_for_winit.borrow_mut() = ModState::default();
                        let mut ps = pointer_state_for_winit.borrow_mut();
                        if ps.resize.is_none() {
                            crate::compositor::carry::system::set_drag_authority_node(st, None);
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
                        let code: u32 = event.key_code().into();
                        let pressed = event.state() == KeyState::Pressed;
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_winit,
                            pointer_state: &pointer_state_for_winit,
                            backend: &backend_handle_for_winit,
                            config_path: config_path_for_winit.as_str(),
                            wayland_display: wayland_display_for_winit.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::Keyboard { code, pressed },
                        );
                    }
                    WinitEvent::Input(InputEvent::PointerMotionAbsolute { event }) => {
                        let ws = backend_for_winit.borrow().window_size();
                        let sx = event.x_transformed(ws.w) as f32;
                        let sy = event.y_transformed(ws.h) as f32;
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_winit,
                            pointer_state: &pointer_state_for_winit,
                            backend: &backend_handle_for_winit,
                            config_path: config_path_for_winit.as_str(),
                            wayland_display: wayland_display_for_winit.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::PointerMotionAbsolute {
                                ws_w: ws.w,
                                ws_h: ws.h,
                                sx,
                                sy,
                                delta_x: 0.0,
                                delta_y: 0.0,
                                delta_x_unaccel: 0.0,
                                delta_y_unaccel: 0.0,
                                time_usec: smithay::backend::input::Event::<
                                    smithay::backend::winit::WinitInput,
                                >::time(&event),
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
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_winit,
                            pointer_state: &pointer_state_for_winit,
                            backend: &backend_handle_for_winit,
                            config_path: config_path_for_winit.as_str(),
                            wayland_display: wayland_display_for_winit.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::PointerMotionAbsolute {
                                ws_w: ws.w,
                                ws_h: ws.h,
                                sx,
                                sy,
                                delta_x: smithay::backend::input::PointerMotionEvent::<
                                    smithay::backend::winit::WinitInput,
                                >::delta_x(&event),
                                delta_y: smithay::backend::input::PointerMotionEvent::<
                                    smithay::backend::winit::WinitInput,
                                >::delta_y(&event),
                                delta_x_unaccel: smithay::backend::input::PointerMotionEvent::<
                                    smithay::backend::winit::WinitInput,
                                >::delta_x_unaccel(
                                    &event
                                ),
                                delta_y_unaccel: smithay::backend::input::PointerMotionEvent::<
                                    smithay::backend::winit::WinitInput,
                                >::delta_y_unaccel(
                                    &event
                                ),
                                time_usec: smithay::backend::input::Event::<
                                    smithay::backend::winit::WinitInput,
                                >::time(&event),
                            },
                        );
                    }
                    WinitEvent::Input(InputEvent::PointerButton { event }) => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_winit,
                            pointer_state: &pointer_state_for_winit,
                            backend: &backend_handle_for_winit,
                            config_path: config_path_for_winit.as_str(),
                            wayland_display: wayland_display_for_winit.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::PointerButton {
                                button_code: event.button_code(),
                                state: event.state(),
                            },
                        );
                    }
                    WinitEvent::Input(InputEvent::PointerAxis { event }) => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_winit,
                            pointer_state: &pointer_state_for_winit,
                            backend: &backend_handle_for_winit,
                            config_path: config_path_for_winit.as_str(),
                            wayland_display: wayland_display_for_winit.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
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

            let initial_frame_interval = frame_interval_for_refresh_hz(
                state
                    .runtime
                    .tuning
                    .tty_viewports
                    .first()
                    .and_then(|vp| vp.refresh_rate),
            );
            let timer = Timer::from_duration(initial_frame_interval);
            ev.handle().insert_source(timer, move |_tick, _, st| {
                if crate::compositor::interaction::state::take_input_state_reset_request(st) {
                    mod_state_for_timer.borrow_mut().clear_intercepts();
                    let mut ps = pointer_state_for_timer.borrow_mut();
                    ps.intercepted_buttons.clear();
                    ps.intercepted_binding_buttons.clear();
                    ps.intercepted_buttons.clear();
                    crate::compositor::carry::system::set_drag_authority_node(st, None);
                    ps.drag = None;
                    ps.move_anim.clear();
                    ps.panning = false;
                }
                if let Some((sx, sy)) = crate::compositor::interaction::state::take_pointer_screen_hint_request(st) {
                    let mut ps = pointer_state_for_timer.borrow_mut();
                    let (ws_w, ws_h) = ps.workspace_size;
                    ps.screen = (sx, sy);
                    ps.world = crate::spatial::screen_to_world(st, ws_w.max(1), ws_h.max(1), sx, sy);
                }
                let now = Instant::now();
                st.drain_drm_syncobj_blockers();

                st.runtime.spawned_children.retain_mut(|child| {
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

                drain_ipc_commands(|request| match request {
                    halley_ipc::Request::Compositor(halley_ipc::CompositorRequest::Quit) => {
                        info!("ipc: quit requested");
                        st.show_exit_confirm_overlay();
                        halley_ipc::Response::Ok
                    }
                    halley_ipc::Request::Compositor(halley_ipc::CompositorRequest::Reload) => {
                        if let Some(next) =
                            RuntimeTuning::try_load_from_path(config_path_for_timer.as_str())
                        {
                            if crate::bootstrap::viewport_section_changed(&st.runtime.tuning, &next) {
                                apply_winit_reload(
                                    &backend_for_timer,
                                    st,
                                    next,
                                    config_path_for_timer.as_str(),
                                    wayland_display_for_timer.as_str(),
                                    "ipc",
                                );
                            } else {
                                let next = crate::bootstrap::preserve_viewport_section(&st.runtime.tuning, next);
                                crate::bootstrap::apply_reloaded_tuning(
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
                        info!("resolved keybinds: {}", st.runtime.tuning.keybinds_resolved_summary());
                        info!("resolved zoom: {}", st.runtime.tuning.zoom_resolved_summary());
                        halley_ipc::Response::Reloaded
                    }
                    halley_ipc::Request::Compositor(halley_ipc::CompositorRequest::Dpms {
                        command,
                        output,
                    }) => {
                        let target = output.unwrap_or_else(|| "all outputs".to_string());
                        warn!(
                            "ipc: ignoring tty-only dpms command on winit backend: {:?} ({})",
                            command, target
                        );
                        halley_ipc::Response::Error(halley_ipc::IpcError::Unsupported(
                            "dpms is only supported on the tty backend".into(),
                        ))
                    }
                    request => crate::ipc::handle_request(st, request),
                });

                {
                    let ws = backend_for_output_timer.borrow().window_size();
                    publish_winit_output_snapshot(ws.w, ws.h, true, 0, 0);
                }

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
                crate::render::tick_frame_effects(st, now);
                crate::render::tick_animator_frame(st, now);
                st.tick_fullscreen_motion(now);
                crate::render::begin_render_frame(st, now);
                {
                    let mut ps = pointer_state_for_timer.borrow_mut();
                    let _ = advance_node_move_anim(st, &mut ps, now);
                }
                crate::render::tick_live_overlap(st);
                if !resize_active {
                    st.run_maintenance_if_needed(now);
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
                        if crate::bootstrap::viewport_section_changed(&st.runtime.tuning, &next) {
                            apply_winit_reload(
                                &backend_for_timer,
                                st,
                                next,
                                config_path_for_timer.as_str(),
                                wayland_display_for_timer.as_str(),
                                "watch",
                            );
                        } else {
                            let next = crate::bootstrap::preserve_viewport_section(&st.runtime.tuning, next);
                            crate::bootstrap::apply_reloaded_tuning(
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
                    info!("resolved keybinds: {}", st.runtime.tuning.keybinds_resolved_summary());
                    info!("resolved zoom: {}", st.runtime.tuning.zoom_resolved_summary());
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
                                if let Some(n) = st.model.field.node(id) {
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

                apply_host_cursor(&backend_for_cursor_timer, &st.effective_cursor_image_status());
                backend_handle_for_timer.request_redraw();
                TimeoutAction::ToDuration(frame_interval_for_refresh_hz(
                    st.runtime.tuning
                        .tty_viewports
                        .first()
                        .and_then(|vp| vp.refresh_rate),
                ))
            })?;

            info!("entering main loop");

            loop {
                ev.dispatch(None, &mut state)?;

                if state.exit_requested() || crate::bootstrap::shutdown_requested() {
                    info!("exit requested, shutting down main loop");
                    break Ok(());
                }

                display.dispatch_clients(&mut state)?;
                display.flush_clients()?;
            }
        }
    )
}
