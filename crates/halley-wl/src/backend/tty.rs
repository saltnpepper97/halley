use super::*;
use std::collections::HashMap;

use crate::backend::interface::{
    BackendView, DmabufImportBackend, TtyBackendHandle, TtyDmabufImportBackend,
};
use crate::backend::tty_drm::{
    TtyDrmOutput, current_tty_output_signature,
    probe_tty_drm_device_via_session, queue_tty_drm_frame, rebuild_tty_outputs,
    selected_tty_scanout_signature,
};
use crate::backend::tty_dpms::{
    apply_tty_dpms_command, publish_tty_outputs_snapshot, wake_tty_dpms_on_input,
};
use crate::backend::tty_input::build_tty_libinput_backend;
use crate::backend::vblank_throttle::VBlankThrottle;
use calloop::{Interest, Mode, PostAction, generic::Generic};

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, Event, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent,
    PointerButtonEvent, PointerMotionEvent,
};

const CONFIG_RELOAD_SETTLE_MS: u64 = 100;
const OUTPUT_RESCAN_POLL_MS: u64 = 750;


const HALLEY_X11_DISPLAY_NUM: u32 = 0;

fn halley_x11_paths(display_num: u32) -> (PathBuf, PathBuf) {
    (
        PathBuf::from(format!("/tmp/.X11-unix/X{}", display_num)),
        PathBuf::from(format!("/tmp/.X{}-lock", display_num)),
    )
}

fn process_exists(pid: u32) -> bool {
    Path::new(&format!("/proc/{}", pid)).exists()
}

fn cleanup_stale_x11_display_files(display_num: u32) {
    let (socket_path, lock_path) = halley_x11_paths(display_num);

    let lock_pid = std::fs::read_to_string(&lock_path)
        .ok()
        .and_then(|contents| contents.trim().parse::<u32>().ok());

    let should_remove = match lock_pid {
        Some(pid) => !process_exists(pid),
        None => socket_path.exists() || lock_path.exists(),
    };

    if !should_remove {
        return;
    }

    for path in [&socket_path, &lock_path] {
        if let Err(err) = std::fs::remove_file(path) {
            if err.kind() != io::ErrorKind::NotFound {
                warn!("failed to remove stale X11 path {}: {}", path.display(), err);
            }
        }
    }
}

fn reset_inherited_display_env() {
    unsafe {
        env::remove_var("DISPLAY");
        env::remove_var("WAYLAND_DISPLAY");
        env::remove_var("WAYLAND_SOCKET");
    }
}

fn active_output_names(outputs: &[TtyDrmOutput]) -> Vec<String> {
    outputs
        .iter()
        .map(|output| output.connector_name.clone())
        .collect()
}

fn active_mode_map(outputs: &[TtyDrmOutput]) -> HashMap<String, drm_control::Mode> {
    outputs
        .iter()
        .map(|output| (output.connector_name.clone(), output.mode))
        .collect()
}

fn outputs_match(a: &[TtyDrmOutput], b: &[TtyDrmOutput]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    a.iter().all(|left| {
        b.iter().any(|right| {
            left.connector_name == right.connector_name
                && left.crtc == right.crtc
                && left.mode.size() == right.mode.size()
                && left.mode.vrefresh() == right.mode.vrefresh()
        })
    })
}


fn canonical_tty_main_output_name(
    outputs: &[TtyDrmOutput],
    tuning: &RuntimeTuning,
) -> Option<String> {
    outputs
        .iter()
        .min_by(|a, b| {
            let a_viewport = tuning
                .tty_viewports
                .iter()
                .find(|viewport| viewport.enabled && viewport.connector == a.connector_name);
            let b_viewport = tuning
                .tty_viewports
                .iter()
                .find(|viewport| viewport.enabled && viewport.connector == b.connector_name);

            let a_offset_x = a_viewport.map(|viewport| viewport.offset_x).unwrap_or(0);
            let b_offset_x = b_viewport.map(|viewport| viewport.offset_x).unwrap_or(0);
            let a_offset_y = a_viewport.map(|viewport| viewport.offset_y).unwrap_or(0);
            let b_offset_y = b_viewport.map(|viewport| viewport.offset_y).unwrap_or(0);

            a_offset_x
                .cmp(&b_offset_x)
                .then(a_offset_y.cmp(&b_offset_y))
                .then(a.connector_name.cmp(&b.connector_name))
        })
        .map(|output| output.connector_name.clone())
}

fn output_advertise_order(outputs: &[TtyDrmOutput], tuning: &RuntimeTuning) -> Vec<String> {
    let main_output = canonical_tty_main_output_name(outputs, tuning);
    let mut ordered: Vec<(String, i32, i32, bool)> = outputs
        .iter()
        .map(|output| {
            let (offset_x, offset_y) = tuning
                .tty_viewports
                .iter()
                .find(|viewport| viewport.enabled && viewport.connector == output.connector_name)
                .map(|viewport| (viewport.offset_x, viewport.offset_y))
                .unwrap_or((0, 0));
            let is_main = main_output
                .as_deref()
                .is_some_and(|name| name == output.connector_name.as_str());
            (output.connector_name.clone(), offset_x, offset_y, is_main)
        })
        .collect();

    // Xwayland/XRandR output listing follows wl_output global creation order.
    // Keep the compositor's canonical main output last, and advertise the rest
    // from right-to-left so the wl_output/XRandR view stays stable even when
    // connectors probe in a different order at boot.
    ordered.sort_by(|a, b| {
        a.3.cmp(&b.3)
            .then(b.1.cmp(&a.1))
            .then(a.2.cmp(&b.2))
            .then(a.0.cmp(&b.0))
    });

    ordered.into_iter().map(|(name, _, _, _)| name).collect()
}

fn layout_size_for_outputs(tuning: &RuntimeTuning, outputs: &[TtyDrmOutput]) -> (i32, i32) {
    let active_names = active_output_names(outputs);
    let layout_w = tuning
        .tty_viewports
        .iter()
        .filter(|viewport| viewport.enabled)
        .filter(|viewport| active_names.iter().any(|name| name == &viewport.connector))
        .map(|viewport| viewport.offset_x + viewport.width as i32)
        .max()
        .unwrap_or(tuning.viewport_size.x.max(1.0).round() as i32);
    let layout_h = tuning
        .tty_viewports
        .iter()
        .filter(|viewport| viewport.enabled)
        .filter(|viewport| active_names.iter().any(|name| name == &viewport.connector))
        .map(|viewport| viewport.offset_y + viewport.height as i32)
        .max()
        .unwrap_or(tuning.viewport_size.y.max(1.0).round() as i32);
    (layout_w, layout_h)
}

fn apply_tty_reload(
    dev: &Rc<RefCell<DrmDevice>>,
    gbm: &Rc<GbmDevice<DrmDeviceFd>>,
    dev_fd: &DrmDeviceFd,
    renderer: &Rc<RefCell<GlesRenderer>>,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    backend_handle: &TtyBackendHandle,
    pointer_state: &Rc<RefCell<PointerState>>,
    st: &mut HalleyWlState,
    next: RuntimeTuning,
    config_path: &str,
    wayland_display: &str,
    reason: &str,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: bool,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    scanout_signature: &Rc<RefCell<Vec<String>>>,
    card_path: &Path,
) -> bool {
    let rebuilt = {
        let mut dev_ref = dev.borrow_mut();
        match rebuild_tty_outputs(
            &mut dev_ref,
            gbm.as_ref(),
            dev_fd.clone(),
            renderer,
            &next,
            card_path,
        ) {
            Ok(target) => target,
            Err(err) => {
                warn!(
                    "{}: viewport reload rejected for {}: {}; keeping last working tty mode",
                    reason, config_path, err
                );
                return false;
            }
        }
    };

    if reason == "rescan" && outputs_match(&outputs.borrow(), &rebuilt) {
        *scanout_signature.borrow_mut() = current_tty_output_signature(&rebuilt);
        return false;
    }

    let next_modes = active_mode_map(&rebuilt);
    let (layout_w, layout_h) = layout_size_for_outputs(&next, &rebuilt);
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
    if reason != "rescan" {
        st.reconfigure_active_tty_monitors(&active_output_names(&rebuilt));
        if let Some(main_output) = canonical_tty_main_output_name(&rebuilt, &st.tuning) {
            if st.current_monitor != main_output {
                let _ = st.activate_monitor(main_output.as_str());
            }
        }
    }
    crate::run::restore_live_camera_state(st, live_camera);

    *scanout_signature.borrow_mut() = current_tty_output_signature(&rebuilt);
    *outputs.borrow_mut() = rebuilt;

    {
        let mut current = active_modes.borrow_mut();
        *current = next_modes.clone();
    }

    {
        let mut pending = output_frame_pending.borrow_mut();
        pending.clear();
        for name in next_modes.keys() {
            pending.insert(name.clone(), false);
        }
    }

    for name in output_advertise_order(outputs.borrow().as_slice(), &st.tuning) {
        if let Some(mode) = next_modes.get(name.as_str()) {
            st.advertise_output(name.as_str(), (*mode).into());
        }
    }

    publish_tty_outputs_snapshot(&dev.borrow(), &active_modes.borrow(), dpms_enabled, &st.tuning);

    if reason != "rescan" {
        let reload_commands = st.tuning.autostart_on_reload.clone();
        run_autostart_commands(st, &reload_commands, wayland_display, "autostart");
    }

    info!(
        "{}: reloaded config from {} with tty layout {}x{}",
        reason, config_path, layout_w, layout_h
    );

    true
}

/// Returns `(width, height, offset_x, offset_y)` for the compositor's current
/// tty monitor when available, otherwise for the canonical live main output.
/// We use one real monitor's dimensions — not the full combined-layout size —
/// when calling libinput's `x_transformed` / `y_transformed` so that the
/// normalised [0,1] range maps to one monitor rather than being stretched
/// across all of them.
fn primary_tty_monitor_dims(
    current_monitor: &str,
    tuning: &RuntimeTuning,
    outputs: &[TtyDrmOutput],
) -> (i32, i32, i32, i32) {
    let canonical_name = canonical_tty_main_output_name(outputs, tuning);
    let preferred_name = if outputs
        .iter()
        .any(|output| output.connector_name == current_monitor)
    {
        Some(current_monitor)
    } else {
        canonical_name.as_deref()
    };

    preferred_name
        .and_then(|name| {
            tuning
                .tty_viewports
                .iter()
                .find(|viewport| viewport.enabled && viewport.connector == name)
                .map(|viewport| {
                    (
                        viewport.width as i32,
                        viewport.height as i32,
                        viewport.offset_x,
                        viewport.offset_y,
                    )
                })
        })
        .or_else(|| {
            outputs.iter().find_map(|output| {
                (output.connector_name == current_monitor).then(|| {
                    let (w, h) = output.mode.size();
                    (w as i32, h as i32, 0, 0)
                })
            })
        })
        .or_else(|| {
            canonical_tty_main_output_name(outputs, tuning).and_then(|name| {
                outputs.iter().find_map(|output| {
                    (output.connector_name == name).then(|| {
                        let (w, h) = output.mode.size();
                        let (offset_x, offset_y) = tuning
                            .tty_viewports
                            .iter()
                            .find(|viewport| {
                                viewport.enabled && viewport.connector == output.connector_name
                            })
                            .map(|viewport| (viewport.offset_x, viewport.offset_y))
                            .unwrap_or((0, 0));
                        (w as i32, h as i32, offset_x, offset_y)
                    })
                })
            })
        })
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
            reset_inherited_display_env();
            cleanup_stale_x11_display_files(HALLEY_X11_DISPLAY_NUM);
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
            info!("config path: {}", config_path.as_str());
            info!("keybind modifier: {}", tuning.keybinds.modifier_name());
            info!("resolved keybinds: {}", tuning.keybinds_resolved_summary());

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
            let outputs = Rc::new(RefCell::new(drm_probe.outputs));
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

            let pending_output_rescan_at = Rc::new(RefCell::new(None::<Instant>));
            {
                let libinput_context_for_session = libinput_context.clone();
                let pending_output_rescan_at_for_session = pending_output_rescan_at.clone();
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
                            *pending_output_rescan_at_for_session.borrow_mut() =
                                Some(Instant::now() + Duration::from_millis(400));
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
            let pending_output_rescan_at_for_timer = pending_output_rescan_at.clone();
            let config_path_for_timer = config_path.clone();
            let wayland_display_for_timer = sock_name.clone();
            state.reconfigure_active_tty_monitors(&active_output_names(&outputs.borrow()));
            if let Some(main_output) =
                canonical_tty_main_output_name(outputs.borrow().as_slice(), &state.tuning)
            {
                if state.current_monitor != main_output {
                    let _ = state.activate_monitor(main_output.as_str());
                }
            }
            let (layout_w, layout_h) = layout_size_for_outputs(&state.tuning, &outputs.borrow());
            let backend_handle = TtyBackendHandle::new(layout_w, layout_h);
            for name in output_advertise_order(outputs.borrow().as_slice(), &state.tuning) {
                if let Some(output) = outputs
                    .borrow()
                    .iter()
                    .find(|output| output.connector_name == name)
                {
                    state.advertise_output(output.connector_name.as_str(), output.mode.into());
                }
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

            let dev = Rc::new(RefCell::new(drm_probe.dev));
            let gbm = Rc::new(drm_probe.gbm);
            let drm_probe_dev_fd = drm_probe.dev_fd; // kept alive so GBM refs stay valid
            let card_path = drm_probe.card_path.clone();
            let active_modes = Rc::new(RefCell::new(active_mode_map(&outputs.borrow())));
            let dpms_enabled = Rc::new(RefCell::new(true));
            publish_tty_outputs_snapshot(&dev.borrow(), &active_modes.borrow(), true, &tuning);
            let outputs_for_vblank = outputs.clone();
            let output_frame_pending = Rc::new(RefCell::new(HashMap::new()));
            let scanout_signature = Rc::new(RefCell::new(current_tty_output_signature(&outputs.borrow())));
            let warned_vblank_mismatch = Rc::new(RefCell::new(false));
            let warned_vblank_mismatch_for_notifier = warned_vblank_mismatch.clone();
            let output_frame_pending_for_notifier = output_frame_pending.clone();
            let output_frame_pending_for_dpms_input = output_frame_pending.clone();
            let output_frame_pending_for_dpms_timer = output_frame_pending.clone();
            let vblank_throttles = Rc::new(RefCell::new(HashMap::<String, VBlankThrottle>::new()));
            let vblank_throttles_for_notifier = vblank_throttles.clone();
            let first_vblank_logged = Rc::new(RefCell::new(std::collections::HashSet::<String>::new()));
            let first_vblank_logged_for_notifier = first_vblank_logged.clone();
            let dev_for_timer = dev.clone();
            let dev_for_input = dev.clone();
            let gbm_for_timer = gbm.clone();
            let dev_fd = drm_probe_dev_fd;
            let dev_fd_for_timer = dev_fd.clone();
            let active_modes_for_timer = active_modes.clone();
            let active_modes_for_input = active_modes.clone();
            let active_modes_for_notifier = active_modes.clone();
            let dpms_enabled_for_timer = dpms_enabled.clone();
            let dpms_enabled_for_input = dpms_enabled.clone();
            let backend_handle_for_timer = backend_handle.clone();
            let first_frame_queued = Rc::new(RefCell::new(std::collections::HashSet::<String>::new()));
            let first_frame_queued_for_timer = first_frame_queued.clone();
            let outputs_for_input = outputs.clone();
            let outputs_for_timer = outputs.clone();
            let scanout_signature_for_timer = scanout_signature.clone();
            let pending_scanout_probe_at = Rc::new(RefCell::new(Some(
                Instant::now() + Duration::from_millis(OUTPUT_RESCAN_POLL_MS),
            )));
            let pending_scanout_probe_at_for_timer = pending_scanout_probe_at.clone();
            let event_loop_handle_for_vblank = ev.handle();
            ev.handle().insert_source(
                drm_probe.notifier,
                move |event, _metadata, _st| match event {
                    DrmEvent::VBlank(crtc) => {
                        let timestamp = Instant::now();
                        let mut matched_outputs = Vec::new();
                        for output in outputs_for_vblank.borrow().iter() {
                            let initial_crtc = output.crtc;
                            let output_name = output.connector_name.clone();
                            let compositor = output.compositor.clone();
                            if crtc != initial_crtc {
                                continue;
                            }
                            let refresh_interval = active_modes_for_notifier
                                .borrow()
                                .get(output_name.as_str())
                                .map(|mode| frame_interval_for_refresh_hz(Some(mode.vrefresh() as f64)));
                            let throttled_output_name = output_name.clone();
                            let should_throttle = vblank_throttles_for_notifier
                                .borrow_mut()
                                .entry(output_name.clone())
                                .or_insert_with(|| {
                                    VBlankThrottle::new(
                                        event_loop_handle_for_vblank.clone(),
                                        output_name.clone(),
                                    )
                                })
                                .throttle(refresh_interval, timestamp, {
                                    let compositor = compositor.clone();
                                    let output_frame_pending_for_notifier =
                                        output_frame_pending_for_notifier.clone();
                                    move |_state| {
                                        if let Err(err) = compositor.borrow_mut().frame_submitted() {
                                            warn!("failed to mark drm frame submitted after throttle: {}", err);
                                        }
                                        output_frame_pending_for_notifier
                                            .borrow_mut()
                                            .insert(throttled_output_name.clone(), false);
                                    }
                                });
                            if should_throttle {
                                continue;
                            }
                            if let Err(err) = compositor.borrow_mut().frame_submitted() {
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
                            let pending_outputs: Vec<_> = outputs_for_vblank
                                .borrow()
                                .iter()
                                .filter_map(|output| {
                                    output_frame_pending_for_notifier
                                        .borrow()
                                        .get(output.connector_name.as_str())
                                        .copied()
                                        .unwrap_or(false)
                                        .then_some(output.connector_name.clone())
                                })
                                .collect();

                            let recoverable_outputs: Vec<String> = pending_outputs
                                .iter()
                                .filter(|output_name: &&String| {
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

            let _renderer_for_input = drm_probe.renderer.clone();
            ev.handle()
                .insert_source(libinput_backend, move |event, _, st| match event {
                    InputEvent::Keyboard { event } => {
                        let tuning = st.tuning.clone();
                        wake_tty_dpms_on_input(
                            &dev_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &outputs_for_input,
                            &tuning,
                            &output_frame_pending_for_dpms_input,
                            st,
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
                        let tuning = st.tuning.clone();
                        wake_tty_dpms_on_input(
                            &dev_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &outputs_for_input,
                            &tuning,
                            &output_frame_pending_for_dpms_input,
                            st,
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
                        let (mon_w, mon_h, mon_ox, mon_oy) = primary_tty_monitor_dims(
                            st.current_monitor.as_str(),
                            &st.tuning,
                            outputs_for_input.borrow().as_slice(),
                        );
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
                        let tuning = st.tuning.clone();
                        wake_tty_dpms_on_input(
                            &dev_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &outputs_for_input,
                            &tuning,
                            &output_frame_pending_for_dpms_input,
                            st,
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
                        let tuning = st.tuning.clone();
                        wake_tty_dpms_on_input(
                            &dev_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &outputs_for_input,
                            &tuning,
                            &output_frame_pending_for_dpms_input,
                            st,
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
                        let tuning = st.tuning.clone();
                        wake_tty_dpms_on_input(
                            &dev_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &outputs_for_input,
                            &tuning,
                            &output_frame_pending_for_dpms_input,
                            st,
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
                                    &gbm_for_timer,
                                    &dev_fd_for_timer,
                                    &renderer_for_timer,
                                    &outputs_for_timer,
                                    &backend_handle_for_timer,
                                    &pointer_state_for_timer,
                                    st,
                                    next,
                                    config_path_for_timer.as_str(),
                                    wayland_display_for_timer.as_str(),
                                    "ipc",
                                    &active_modes_for_timer,
                                    *dpms_enabled_for_timer.borrow(),
                                    &output_frame_pending_for_dpms_timer,
                                    &scanout_signature_for_timer,
                                    card_path.as_path(),
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
                        let tuning = st.tuning.clone();
                        apply_tty_dpms_command(
                            &dev_for_timer,
                            &active_modes_for_timer,
                            &dpms_enabled_for_timer,
                            command,
                            &outputs_for_timer,
                            &tuning,
                            &output_frame_pending_for_dpms_timer,
                            st,
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
                                &gbm_for_timer,
                                &dev_fd_for_timer,
                                &renderer_for_timer,
                                &outputs_for_timer,
                                &backend_handle_for_timer,
                                &pointer_state_for_timer,
                                st,
                                next,
                                config_path_for_timer.as_str(),
                                wayland_display_for_timer.as_str(),
                                    "watch",
                                    &active_modes_for_timer,
                                    *dpms_enabled_for_timer.borrow(),
                                    &output_frame_pending_for_dpms_timer,
                                    &scanout_signature_for_timer,
                                    card_path.as_path(),
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
                if pending_scanout_probe_at_for_timer
                    .borrow()
                    .is_some_and(|deadline| now >= deadline)
                {
                    *pending_scanout_probe_at_for_timer.borrow_mut() =
                        Some(now + Duration::from_millis(OUTPUT_RESCAN_POLL_MS));
                    let maybe_signature = {
                        let mut dev = dev_for_timer.borrow_mut();
                        selected_tty_scanout_signature(&mut dev, &st.tuning)
                    };
                    match maybe_signature {
                        Ok(next_signature) => {
                            let current_signature = scanout_signature_for_timer.borrow().clone();
                            if next_signature != current_signature {
                                *pending_output_rescan_at_for_timer.borrow_mut() = Some(now);
                            }
                        }
                        Err(err) => {
                            debug!("tty drm topology probe skipped: {}", err);
                        }
                    }
                }

                if pending_output_rescan_at_for_timer
                    .borrow()
                    .is_some_and(|deadline| now >= deadline)
                {
                    *pending_output_rescan_at_for_timer.borrow_mut() = None;
                if *dpms_enabled_for_timer.borrow() {
                    let next = st.tuning.clone();
                    apply_tty_reload(
                        &dev_for_timer,
                        &gbm_for_timer,
                        &dev_fd_for_timer,
                        &renderer_for_timer,
                        &outputs_for_timer,
                        &backend_handle_for_timer,
                        &pointer_state_for_timer,
                        st,
                        next,
                        config_path_for_timer.as_str(),
                        wayland_display_for_timer.as_str(),
                        "rescan",
                        &active_modes_for_timer,
                        *dpms_enabled_for_timer.borrow(),
                        &output_frame_pending_for_dpms_timer,
                        &scanout_signature_for_timer,
                        card_path.as_path(),
                    );
                } else {
                    // Reschedule for after wake — just leave pending cleared,
                    // wake will trigger its own rescan via the output signature check.
                    debug!("rescan deferred: dpms is off");
                }

                }
                if reloaded {
                    info!("resolved keybinds: {}", st.tuning.keybinds_resolved_summary());
                }

                let ps = pointer_state_for_timer.borrow();
                let resize_preview = ps.resize;
                drop(ps);

                if *dpms_enabled_for_timer.borrow() {
                    // On the first tick after DPMS wake, re-configure layer shell
                    // surfaces and flush frame callbacks so wallpaper clients
                    // re-present before we queue the first scanout frame.
                    if st.dpms_just_woke {
                        st.dpms_just_woke = false;
                        let wake_size = {
                            let outputs_ref = outputs_for_timer.borrow();
                            outputs_ref.first().map(|o| {
                                let (w, h) = o.mode.size();
                                smithay::utils::Size::<i32, smithay::utils::Logical>::from(
                                    (w as i32, h as i32),
                                )
                            })
                        };
                        if let Some(size) = wake_size {
                            st.configure_layer_shell_surfaces(size);
                        }
                        st.send_frame_callbacks(now);
                    }

                    st.send_frame_callbacks(now);

                    let cursor_image = st.cursor_image_status.clone();
                    let previous_monitor = st.current_monitor.clone();

                    let outputs_ref = outputs_for_timer.borrow();
                    let mut render_order: Vec<_> = outputs_ref.iter().collect();

                    // Queue lower-refresh / "slower" outputs first so the slow secondary
                    // gets its first frame submitted before the faster primary.
                    render_order.sort_by_key(|output| output.mode.vrefresh());

                    for output in render_order {
                        let output_name = output.connector_name.as_str();
                        let _ = st.activate_monitor(output_name);

                        let ps = pointer_state_for_timer.borrow();
                        let (hover_node, preview_hover_node) = resolve_hover_targets(st, &ps, now);
                        let cursor_screen = Some(ps.screen);
                        drop(ps);

                        let compositor = &output.compositor;
                        let frame_already_pending = output_frame_pending
                            .borrow()
                            .get(output_name)
                            .copied()
                            .unwrap_or(false);

                        if frame_already_pending {
                            continue;
                        }

                        if let Err(err) = queue_tty_drm_frame(
                            output_name,
                            compositor,
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
                                .insert(output.connector_name.clone())
                            {
                                info!("first tty drm frame queued for {}", output_name);
                            }

                            output_frame_pending
                                .borrow_mut()
                                .insert(output.connector_name.clone(), true);
                        }
                    }

                    let _ = st.activate_monitor(previous_monitor.as_str());
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
            let result = loop {
                ev.dispatch(None, &mut state)?;
                if state.exit_requested() || shutdown_requested() {
                    info!("exit requested, shutting down tty main loop");
                    break Ok(());
                }
                display.dispatch_clients(&mut state)?;
                display.flush_clients()?;
            };
            cleanup_stale_x11_display_files(HALLEY_X11_DISPLAY_NUM);
            result
        }
    )
}



