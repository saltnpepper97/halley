mod dpms;
mod drm;
mod frame;
mod output;
mod scheduler;
mod stats;

use super::*;

use crate::input::ctx::InputCtx;
use smithay::backend::renderer::ImportEgl;
use std::collections::HashMap;
use std::collections::HashSet;

use crate::backend::interface::{
    BackendView, DmabufImportBackend, TtyBackendHandle, TtyDmabufImportBackend,
};
use crate::backend::tty::dpms::{
    any_tty_output_dpms_enabled, apply_tty_dpms_command, publish_tty_outputs_snapshot_for_devices,
    sync_tty_dpms_state, tty_output_dpms_enabled, wake_tty_dpms_on_input,
};
use crate::backend::tty::drm::{
    TtyDrmDevice, TtyDrmOutput, TtyGpuManager, TtyOutputCaptureBackend,
    build_tty_dmabuf_output_feedbacks, current_tty_output_signature,
    probe_tty_drm_device_via_session, queue_tty_drm_frame, rebuild_tty_outputs,
};
use crate::backend::tty::frame::{
    TtyFrameClock, VBlankMismatchState, drm_vblank_timestamp, monotonic_now_duration,
    output_frame_interval, present_tty_frame_feedback, schedule_estimated_frame_callback,
    send_due_estimated_frame_callbacks, sync_tty_frame_clocks,
};
use crate::backend::tty::output::{
    active_mode_map, active_output_names, bootstrap_tty_viewports, canonical_tty_main_output_name,
    effective_tty_viewports_for_outputs, layout_size_for_outputs,
    log_effective_tty_viewport_fallback, output_advertise_order, outputs_match,
    primary_tty_monitor_dims,
};
use crate::backend::tty::scheduler::{
    tty_animation_output_ready_for_redraw, tty_animation_redraw_active,
    tty_animation_redraw_outputs, tty_due_outputs_for_timer, tty_output_animation_redraw_active,
    tty_outputs_include_animation_redraw, tty_ready_animation_redraw_outputs,
};
use crate::backend::tty::stats::{
    TtyFrameStats, maybe_log_tty_frame_stats, record_tty_frame_queue,
};
use crate::backend::vblank_throttle::VBlankThrottle;
use crate::compositor::exit_confirm;
use crate::compositor::interaction::ResizeCtx;
use calloop::ping::make_ping;
use smithay::backend::drm::DrmNode;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::udev::{UdevBackend, UdevEvent};

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, DeviceCapability, Event, GestureBeginEvent, GestureEndEvent,
    GesturePinchUpdateEvent, GestureSwipeUpdateEvent, InputEvent, KeyState, KeyboardKeyEvent,
    PointerAxisEvent, PointerButtonEvent, PointerMotionEvent, TouchEvent,
};

const CONFIG_RELOAD_SETTLE_MS: u64 = 100;
const VBLANK_MISMATCH_LOG_AFTER_MS: u64 = 1_000;
const PENDING_FRAME_TIMEOUT_MS: u64 = 1_500;

const HALLEY_X11_DISPLAY_NUM: u32 = 0;

fn tty_env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn take_ready_tty_redraw_outputs(
    backend_handle: &TtyBackendHandle,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    frame_clocks: &Rc<RefCell<HashMap<String, TtyFrameClock>>>,
    st: &mut Halley,
) -> HashSet<String> {
    if backend_handle.take_redraw_all_outputs() || std::mem::take(&mut st.runtime.tty_redraw_all) {
        for output in outputs.borrow().iter() {
            st.runtime
                .tty_redraw_outputs
                .insert(output.connector_name.clone());
        }
    }
    st.runtime
        .tty_redraw_outputs
        .extend(backend_handle.take_redraw_outputs());

    let output_names: HashSet<String> = outputs
        .borrow()
        .iter()
        .map(|output| output.connector_name.clone())
        .collect();
    st.runtime
        .tty_redraw_outputs
        .retain(|output_name| output_names.contains(output_name));

    let pending = output_frame_pending.borrow();
    {
        let mut clocks = frame_clocks.borrow_mut();
        for output_name in &st.runtime.tty_redraw_outputs {
            if pending.get(output_name.as_str()).copied().unwrap_or(false)
                && let Some(clock) = clocks.get_mut(output_name.as_str())
            {
                clock.queue_redraw();
            }
        }
    }
    let ready: HashSet<String> = st
        .runtime
        .tty_redraw_outputs
        .iter()
        .filter(|output_name| !pending.get(output_name.as_str()).copied().unwrap_or(false))
        .cloned()
        .collect();
    drop(pending);

    for output_name in &ready {
        st.runtime.tty_redraw_outputs.remove(output_name.as_str());
    }
    ready
}

fn queue_ready_tty_outputs(
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    dpms_enabled: &Rc<RefCell<HashMap<String, bool>>>,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    output_frame_pending_since: &Rc<RefCell<HashMap<String, Instant>>>,
    output_animation_redraw_active: &Rc<RefCell<HashMap<String, bool>>>,
    estimated_frame_callbacks: &Rc<RefCell<HashMap<String, Instant>>>,
    frame_clocks: &Rc<RefCell<HashMap<String, TtyFrameClock>>>,
    composed_frame_cache: &Rc<RefCell<HashMap<String, GlesTexture>>>,
    pointer_state: &Rc<RefCell<crate::compositor::interaction::PointerState>>,
    gpu_manager: &Rc<RefCell<TtyGpuManager>>,
    primary_render_node: DrmNode,
    first_frame_queued: &Rc<RefCell<HashSet<String>>>,
    frame_stats: Option<&Rc<RefCell<TtyFrameStats>>>,
    st: &mut Halley,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
    eligible_outputs: Option<&HashSet<String>>,
    _source: &str,
) {
    if !any_tty_output_dpms_enabled(&dpms_enabled.borrow()) {
        return;
    }

    let cursor_image = st.effective_cursor_image_status();
    let previous_monitor = st.model.monitor_state.current_monitor.clone();

    let outputs_ref = outputs.borrow();
    let mut render_order: Vec<_> = outputs_ref.iter().collect();
    render_order.sort_by_key(|output| {
        let animation_active = tty_output_animation_redraw_active(
            st,
            pointer_state,
            output.connector_name.as_str(),
            now,
        );
        (!animation_active, output.mode.vrefresh())
    });

    for output in render_order {
        let output_name = output.connector_name.as_str();
        if eligible_outputs.is_some_and(|eligible| !eligible.contains(output_name)) {
            continue;
        }
        if !tty_output_dpms_enabled(&dpms_enabled.borrow(), output_name) {
            output_frame_pending
                .borrow_mut()
                .insert(output.connector_name.clone(), false);
            output_frame_pending_since
                .borrow_mut()
                .remove(output.connector_name.as_str());
            continue;
        }

        let ps = pointer_state.borrow();
        let (hover_node, preview_hover_node) =
            resolve_hover_targets_for_monitor(st, &ps, now, output_name);
        let cursor_screen = Some(ps.screen);
        drop(ps);

        if output_frame_pending
            .borrow()
            .get(output_name)
            .copied()
            .unwrap_or(false)
        {
            continue;
        }

        match queue_tty_drm_frame(
            output_name,
            output.device_node,
            &output.compositor,
            gpu_manager,
            primary_render_node,
            output.render_node,
            composed_frame_cache,
            st,
            resize_preview,
            hover_node,
            preview_hover_node,
            cursor_screen,
            Some(&cursor_image),
        ) {
            Err(err) => {
                output_frame_pending_since
                    .borrow_mut()
                    .remove(output.connector_name.as_str());
                warn!("tty drm frame queue skipped for {}: {}", output_name, err)
            }
            Ok(report) => {
                record_tty_frame_queue(frame_stats, &report);
                let previous_active = output_animation_redraw_active
                    .borrow_mut()
                    .insert(
                        output.connector_name.clone(),
                        report.animation_redraw_active,
                    )
                    .unwrap_or(false);
                let _ = previous_active;
                if !report.queued {
                    output_frame_pending_since
                        .borrow_mut()
                        .remove(output.connector_name.as_str());
                    schedule_estimated_frame_callback(
                        estimated_frame_callbacks,
                        frame_clocks,
                        output,
                        now,
                    );
                    continue;
                }
                estimated_frame_callbacks
                    .borrow_mut()
                    .remove(output.connector_name.as_str());
                if first_frame_queued
                    .borrow_mut()
                    .insert(output.connector_name.clone())
                {
                    debug!("first tty drm frame queued for {}", output_name);
                }

                output_frame_pending
                    .borrow_mut()
                    .insert(output.connector_name.clone(), true);
                output_frame_pending_since
                    .borrow_mut()
                    .insert(output.connector_name.clone(), now);
                frame_clocks
                    .borrow_mut()
                    .entry(output.connector_name.clone())
                    .or_insert_with(|| TtyFrameClock::new(output_frame_interval(output)))
                    .mark_submitted();
                st.advance_tty_frame_callback_sequence(output_name);
                crate::frame_loop::send_frame_callbacks_for_output(st, output_name, now);
            }
        }
    }

    let _ = st.activate_monitor(previous_monitor.as_str());
}

fn release_pending_tty_outputs(
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    output_frame_pending_since: &Rc<RefCell<HashMap<String, Instant>>>,
    output_names: &[String],
    reason: &str,
) -> usize {
    if output_names.is_empty() {
        return 0;
    }

    let wanted: HashSet<&str> = output_names.iter().map(String::as_str).collect();
    let mut released = Vec::new();

    for output in outputs.borrow().iter() {
        if !wanted.contains(output.connector_name.as_str()) {
            continue;
        }
        if let Err(err) = output.compositor.borrow_mut().frame_submitted() {
            warn!(
                "failed to release pending tty frame for {} during {}: {}",
                output.connector_name, reason, err
            );
        }
        released.push(output.connector_name.clone());
    }

    if released.is_empty() {
        return 0;
    }

    {
        let mut pending = output_frame_pending.borrow_mut();
        for output_name in &released {
            pending.insert(output_name.clone(), false);
        }
    }
    {
        let mut pending_since = output_frame_pending_since.borrow_mut();
        for output_name in &released {
            pending_since.remove(output_name.as_str());
        }
    }

    released.len()
}

fn advance_tty_redraw_frame(
    st: &mut Halley,
    pointer_state: &Rc<RefCell<PointerState>>,
    now: Instant,
    include_maintenance: bool,
) {
    crate::compositor::platform::drain_drm_syncobj_blockers(st);

    let resize_active = {
        let ps = pointer_state.borrow();
        ps.resize.is_some()
    };

    crate::frame_loop::tick_frame_effects(st, now);
    crate::frame_loop::tick_animator_frame(st, now);
    st.tick_fullscreen_motion(now);
    crate::frame_loop::begin_render_frame(st, now);
    {
        let mut ps = pointer_state.borrow_mut();
        let _ = advance_node_move_anim(st, &mut ps, now);
        let _ = advance_resize_anim(st, &mut ps, now);
    }
    crate::frame_loop::tick_live_overlap(st);
    if include_maintenance && !resize_active {
        st.run_maintenance_if_needed(now);
    }
}

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

    let lock_pid = match std::fs::read_to_string(&lock_path) {
        Ok(contents) => match contents.trim().parse::<u32>() {
            Ok(pid) => Some(pid),
            Err(err) => {
                warn!(
                    "refusing to reclaim X11 display {}: invalid lock pid in {}: {}",
                    display_num,
                    lock_path.display(),
                    err
                );
                return;
            }
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            warn!(
                "refusing to reclaim X11 display {}: could not read {}: {}",
                display_num,
                lock_path.display(),
                err
            );
            return;
        }
    };

    let should_remove = match lock_pid {
        Some(pid) => !process_exists(pid),
        None => false,
    };

    if !should_remove {
        if socket_path.exists() && !lock_path.exists() {
            warn!(
                "refusing to reclaim X11 display {}: socket exists without lock file",
                display_num
            );
        }
        return;
    }

    for path in [&socket_path, &lock_path] {
        if let Err(err) = std::fs::remove_file(path)
            && err.kind() != io::ErrorKind::NotFound
        {
            warn!(
                "failed to remove stale X11 path {}: {}",
                path.display(),
                err
            );
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

fn apply_tty_reload(
    drm_devices: &Rc<RefCell<Vec<TtyDrmDevice>>>,
    gpu_manager: &Rc<RefCell<TtyGpuManager>>,
    primary_render_node: DrmNode,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    backend_handle: &TtyBackendHandle,
    pointer_state: &Rc<RefCell<PointerState>>,
    st: &mut Halley,
    next: RuntimeTuning,
    config_path: &str,
    wayland_display: &str,
    reason: &str,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: &Rc<RefCell<HashMap<String, bool>>>,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    output_frame_pending_since: &Rc<RefCell<HashMap<String, Instant>>>,
    output_animation_redraw_active: &Rc<RefCell<HashMap<String, bool>>>,
    frame_clocks: &Rc<RefCell<HashMap<String, TtyFrameClock>>>,
    scanout_signature: &Rc<RefCell<Vec<String>>>,
) -> bool {
    let mut rebuilt = Vec::new();
    for device in drm_devices.borrow_mut().iter_mut() {
        let mut dev_ref = device.dev.borrow_mut();
        match rebuild_tty_outputs(
            &mut dev_ref,
            device.gbm.as_ref(),
            device.dev_fd.clone(),
            gpu_manager,
            device.render_node,
            &next,
            device.card_path.as_path(),
        ) {
            Ok(mut target) => rebuilt.append(&mut target),
            Err(err) => {
                warn!(
                    "{}: viewport reload skipped for {} on {}: {}",
                    reason,
                    config_path,
                    device.card_path.display(),
                    err
                );
            }
        }
    }
    if rebuilt.is_empty() {
        warn!(
            "{}: viewport reload rejected for {}: no usable tty outputs across DRM devices",
            reason, config_path
        );
        return false;
    }

    if reason == "rescan" && outputs_match(&outputs.borrow(), &rebuilt) {
        *scanout_signature.borrow_mut() = current_tty_output_signature(&rebuilt);
        let output_names = active_output_names(outputs.borrow().as_slice());
        let pending_outputs: Vec<String> = {
            let pending = output_frame_pending.borrow();
            output_names
                .iter()
                .filter(|output_name| pending.get(output_name.as_str()).copied().unwrap_or(false))
                .cloned()
                .collect()
        };
        if !pending_outputs.is_empty() {
            let _ = release_pending_tty_outputs(
                outputs,
                output_frame_pending,
                output_frame_pending_since,
                pending_outputs.as_slice(),
                "unchanged-output-rescan",
            );
        }
        {
            let mut pending_since = output_frame_pending_since.borrow_mut();
            for output_name in &output_names {
                pending_since.remove(output_name.as_str());
            }
        }
        st.runtime.tty_redraw_outputs.extend(output_names);
        return false;
    }

    let next_modes = active_mode_map(&rebuilt);
    let (layout_w, layout_h) = layout_size_for_outputs(&next, &rebuilt);
    backend_handle.set_size(layout_w, layout_h);
    log_effective_tty_viewport_fallback(&next, &rebuilt, reason);

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

    let live_camera = crate::bootstrap::capture_live_camera_state(st);
    st.apply_tuning(next);
    if reason != "rescan" {
        let effective_viewports = effective_tty_viewports_for_outputs(&st.runtime.tuning, &rebuilt);
        st.reconfigure_active_tty_monitors(&effective_viewports);
        let target_monitor = [
            st.focused_monitor().to_string(),
            st.interaction_monitor().to_string(),
            st.model.monitor_state.current_monitor.clone(),
        ]
        .into_iter()
        .find(|name| st.model.monitor_state.monitors.contains_key(name))
        .or_else(|| canonical_tty_main_output_name(&rebuilt, &st.runtime.tuning));
        if let Some(target_monitor) = target_monitor
            && st.model.monitor_state.current_monitor != target_monitor
        {
            let _ = st.activate_monitor(target_monitor.as_str());
        }
    }
    crate::bootstrap::restore_live_camera_state(st, live_camera);
    st.ui.render_state.clear_window_offscreen_caches();
    st.request_maintenance();

    *scanout_signature.borrow_mut() = current_tty_output_signature(&rebuilt);
    st.configure_dmabuf_output_feedbacks(build_tty_dmabuf_output_feedbacks(
        rebuilt.as_slice(),
        gpu_manager,
        primary_render_node,
    ));
    sync_tty_dpms_state(&rebuilt, dpms_enabled);
    sync_tty_frame_clocks(frame_clocks, rebuilt.as_slice());
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

    output_frame_pending_since.borrow_mut().clear();

    {
        let mut active = output_animation_redraw_active.borrow_mut();
        active.clear();
        for name in next_modes.keys() {
            active.insert(name.clone(), false);
        }
    }

    for name in output_advertise_order(outputs.borrow().as_slice(), &st.runtime.tuning) {
        if let Some(mode) = next_modes.get(name.as_str()) {
            let physical_size_mm = outputs
                .borrow()
                .iter()
                .find(|output| output.connector_name == name)
                .and_then(|output| output.physical_size_mm);
            st.advertise_output_with_physical_size(name.as_str(), (*mode).into(), physical_size_mm);
        }
    }
    publish_tty_outputs_snapshot_for_devices(
        &drm_devices.borrow(),
        &active_modes.borrow(),
        &dpms_enabled.borrow(),
        &st.runtime.tuning,
        st,
    );

    if crate::compositor::spawn::state::recompute_all_node_rule_opacities(st) {
        st.runtime.tty_redraw_all = true;
    }

    if reason != "rescan" {
        let reload_commands = st.runtime.tuning.autostart_on_reload.clone();
        run_autostart_commands(st, &reload_commands, wayland_display, "autostart");
    }

    debug!(
        "{}: reloaded config from {} with tty layout {}x{}",
        reason, config_path, layout_w, layout_h
    );

    true
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

            let resolved_config_path = RuntimeTuning::resolved_config_path();
            let (seat_name, drm_probe, libinput_backend, libinput_context, session_notifier) = {
                let config_path = resolved_config_path.path.to_string_lossy().to_string();
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

            let mut display: Display<Halley> = Display::new()?;
            let dh = display.handle();

            let resolved_config_path = crate::bootstrap::ensure_resolved_default_user_config(
                resolved_config_path,
                Some(&bootstrap_tty_viewports(drm_probe.outputs.as_slice())),
            );
            let config_source = resolved_config_path.source;
            let config_path = Rc::new(resolved_config_path.path.to_string_lossy().to_string());
            let aperture_config_path = Rc::new(crate::aperture::default_aperture_config_path());
            let (tuning, startup_config_error) =
                crate::bootstrap::load_startup_tuning(config_path.as_str());
            let aperture_config =
                crate::aperture::load_aperture_config_from_path(aperture_config_path.as_path());
            tuning.apply_process_env();
            if !Path::new(config_path.as_str()).exists() {
                warn!(
                    "config file not found at {}; using built-in defaults",
                    config_path.as_str()
                );
            }
            info!(
                "config: using {} {}",
                config_source.as_str(),
                config_path.as_str()
            );
            info!(
                "keyboard config: layout={} variant={} options={}",
                tuning.input.keyboard.layout,
                tuning.input.keyboard.variant,
                tuning.input.keyboard.options
            );
            if !aperture_config_path.as_path().exists() {
                warn!(
                    "aperture config file not found at {}; using built-in defaults",
                    aperture_config_path.display()
                );
            }
            crate::aperture::log_aperture_config_startup(aperture_config_path.as_ref());
            debug!("keybind modifier: {}", tuning.keybinds.modifier_name());
            debug!("resolved keybinds: {}", tuning.keybinds_resolved_summary());
            debug!("resolved zoom: {}", tuning.zoom_resolved_summary());

            let (watch_rx, _watcher): (Option<mpsc::Receiver<()>>, Option<RecommendedWatcher>) = {
                let (watch_tx, watch_rx) = mpsc::channel::<()>();
                let mut config_watch_targets = vec![
                    PathBuf::from(config_path.as_str()),
                    aperture_config_path.as_ref().clone(),
                ];
                config_watch_targets.extend(halley_config::gather_dependencies_for_file(
                    config_path.as_str(),
                ));
                // Also watch the aperture config's gather deps (e.g. pywal colours),
                // otherwise a rewrite of the gathered cache file won't trigger a reload.
                config_watch_targets.extend(halley_config::gather_dependencies_for_file(
                    aperture_config_path.to_string_lossy().as_ref(),
                ));
                let config_watch_targets_for_callback = config_watch_targets.clone();
                let mut watcher: RecommendedWatcher = notify::recommended_watcher(
                    move |result: Result<notify::Event, notify::Error>| {
                        if let Ok(event) = result {
                            let touches_config = if event.paths.is_empty() {
                                true
                            } else {
                                event.paths.iter().any(|path| {
                                    crate::aperture::config_matches_event_path(
                                        path,
                                        &config_watch_targets_for_callback,
                                    )
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
                for watch_root in crate::aperture::config_watch_roots(&config_watch_targets) {
                    if let Err(err) =
                        watcher.watch(watch_root.as_path(), RecursiveMode::NonRecursive)
                    {
                        warn!(
                            "config watch disabled for {}: {}",
                            watch_root.display(),
                            err
                        );
                    }
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
            sync_portal_activation_environment(sock_name.as_str());
            let xwayland_for_timer = xwayland.clone();
            let libinput_backend = libinput_backend;

            let mut ev: EventLoop<Halley> = EventLoop::try_new()?;
            let xwayland_event_loop = ev.handle();
            let xwayland_event_loop_for_timer = xwayland_event_loop.clone();
            let xwayland_watch_tokens = XwaylandSocketWatchTokens::default();
            let xwayland_watch_tokens_for_timer = xwayland_watch_tokens.clone();
            let _signal = ev.get_signal();
            let mut state = Halley::new(&dh, ev.handle(), tuning.clone());
            state.runtime.wayland_display = Some(sock_name.clone());
            state.apply_aperture_config(aperture_config);
            let capture_dmabuf_formats = {
                let mut gpu_manager = drm_probe.gpu_manager.borrow_mut();
                let renderer = gpu_manager
                    .single_renderer(&drm_probe.primary_render_node)
                    .map_err(|err| {
                        io::Error::other(format!(
                            "failed to query tty primary renderer formats: {:?}",
                            err
                        ))
                    })?;
                <GlesRenderer as smithay::backend::renderer::Bind<
                    smithay::backend::allocator::dmabuf::Dmabuf,
                >>::supported_formats(renderer.as_ref())
                .map(|formats| formats.iter().copied().collect())
                .unwrap_or_default()
            };
            let outputs = Rc::new(RefCell::new(drm_probe.outputs));
            log_effective_tty_viewport_fallback(&tuning, outputs.borrow().as_slice(), "startup");
            let effective_viewports =
                effective_tty_viewports_for_outputs(&tuning, outputs.borrow().as_slice());
            state.reconfigure_active_tty_monitors(&effective_viewports);
            if let Some(diagnostic) = startup_config_error.as_ref() {
                crate::bootstrap::show_config_startup_error(&mut state, diagnostic);
            }
            let dmabuf_importer: Rc<dyn DmabufImportBackend> =
                Rc::new(TtyDmabufImportBackend::new(
                    drm_probe.gpu_manager.clone(),
                    drm_probe.primary_render_node,
                ));
            state.configure_dmabuf_importer_for_fd(
                dmabuf_importer,
                drm_probe.primary_dev_fd.clone(),
            );
            state.configure_dmabuf_output_feedbacks(build_tty_dmabuf_output_feedbacks(
                outputs.borrow().as_slice(),
                &drm_probe.gpu_manager,
                drm_probe.primary_render_node,
            ));
            {
                let mut gpu_manager = drm_probe.gpu_manager.borrow_mut();
                match gpu_manager.single_renderer(&drm_probe.primary_render_node) {
                    Ok(mut renderer) => match renderer.as_mut().bind_wl_display(&dh) {
                        Ok(_) => eventline::info!(
                            "EGL hardware acceleration for Wayland clients enabled"
                        ),
                        Err(err) => eventline::warn!(
                            "failed to enable EGL hardware acceleration for Wayland clients: {err}"
                        ),
                    },
                    Err(err) => eventline::warn!(
                        "failed to borrow primary renderer for EGL Wayland binding: {:?}",
                        err
                    ),
                }
            }
            if smithay::wayland::drm_syncobj::supports_syncobj_eventfd(&drm_probe.primary_dev_fd) {
                state.platform.drm_syncobj_state = Some(
                    smithay::wayland::drm_syncobj::DrmSyncobjState::new::<Halley>(
                        &dh,
                        drm_probe.primary_dev_fd.clone(),
                    ),
                );
            }
            state.set_app_focused(true);
            state.platform.seat.add_pointer();
            super::initialize_seat_keyboard(&mut state);
            let autostart_once = state.runtime.tuning.autostart_once.clone();
            run_autostart_commands(&mut state, &autostart_once, sock_name.as_str(), "autostart");

            let mut dh_for_clients = dh.clone();
            ev.handle()
                .insert_source(listening, move |client_stream, _, _st| {
                    let _ =
                        dh_for_clients.insert_client(client_stream, Arc::new(ClientState::new()));
                })?;

            install_xwayland_socket_watchers(
                &xwayland_event_loop,
                &xwayland,
                &xwayland_watch_tokens,
            )?;

            let pending_output_rescan_at = Rc::new(RefCell::new(None::<Instant>));
            {
                let libinput_context_for_session = libinput_context.clone();
                let pending_output_rescan_at_for_session = pending_output_rescan_at.clone();
                ev.handle()
                    .insert_source(session_notifier, move |event, _, _st| match event {
                        SessionEvent::PauseSession => {
                            debug!("tty session paused");
                            libinput_context_for_session.borrow_mut().suspend();
                        }
                        SessionEvent::ActivateSession => {
                            debug!("tty session activated");
                            if libinput_context_for_session.borrow_mut().resume().is_err() {
                                warn!("failed to resume libinput context after session activation");
                            }
                            *pending_output_rescan_at_for_session.borrow_mut() =
                                Some(Instant::now() + Duration::from_millis(400));
                        }
                    })?;
            }

            match UdevBackend::new(seat_name.as_str()) {
                Ok(udev_backend) => {
                    let pending_output_rescan_at_for_udev = pending_output_rescan_at.clone();
                    ev.handle()
                        .insert_source(udev_backend, move |event, _, _st| {
                            match event {
                                UdevEvent::Added { device_id, path } => {
                                    debug!(
                                        "tty drm udev add: device_id={} path={}",
                                        device_id,
                                        path.display()
                                    );
                                }
                                UdevEvent::Changed { device_id } => {
                                    debug!("tty drm udev change: device_id={}", device_id);
                                }
                                UdevEvent::Removed { device_id } => {
                                    debug!("tty drm udev remove: device_id={}", device_id);
                                }
                            }
                            *pending_output_rescan_at_for_udev.borrow_mut() =
                                Some(Instant::now() + Duration::from_millis(400));
                        })?;
                }
                Err(err) => warn!("tty drm udev monitoring disabled: {}", err),
            }

            let mod_state = Rc::new(RefCell::new(ModState::default()));
            let mod_state_for_input = mod_state.clone();
            let pointer_state = Rc::new(RefCell::new(PointerState::default()));
            crate::protocol::wayland::portal::configure_output_capture_backend(
                &mut state,
                Rc::new(TtyOutputCaptureBackend {
                    gpu_manager: drm_probe.gpu_manager.clone(),
                    primary_render_node: drm_probe.primary_render_node,
                    outputs: outputs.clone(),
                    pointer_state: pointer_state.clone(),
                    dmabuf_formats: capture_dmabuf_formats,
                    capture_texture_cache: RefCell::new(HashMap::new()),
                }),
            );
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
            let aperture_config_path_for_timer = aperture_config_path.clone();
            let wayland_display_for_timer = sock_name.clone();
            let target_monitor = [
                state.focused_monitor().to_string(),
                state.interaction_monitor().to_string(),
                state.model.monitor_state.current_monitor.clone(),
            ]
            .into_iter()
            .find(|name| state.model.monitor_state.monitors.contains_key(name))
            .or_else(|| {
                canonical_tty_main_output_name(outputs.borrow().as_slice(), &state.runtime.tuning)
            });
            if let Some(target_monitor) = target_monitor
                && state.model.monitor_state.current_monitor != target_monitor
            {
                let _ = state.activate_monitor(target_monitor.as_str());
            }
            let (layout_w, layout_h) =
                layout_size_for_outputs(&state.runtime.tuning, &outputs.borrow());
            let backend_handle = TtyBackendHandle::new(layout_w, layout_h);
            for name in output_advertise_order(outputs.borrow().as_slice(), &state.runtime.tuning) {
                if let Some(output) = outputs
                    .borrow()
                    .iter()
                    .find(|output| output.connector_name == name)
                {
                    state.advertise_output_with_physical_size(
                        output.connector_name.as_str(),
                        output.mode.into(),
                        output.physical_size_mm,
                    );
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
                    .model
                    .monitor_state
                    .monitors
                    .get(&state.model.monitor_state.current_monitor)
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

            let gpu_manager = drm_probe.gpu_manager.clone();
            let primary_render_node = drm_probe.primary_render_node;
            let drm_devices = Rc::new(RefCell::new(drm_probe.devices));
            let active_modes = Rc::new(RefCell::new(active_mode_map(&outputs.borrow())));
            let dpms_enabled = Rc::new(RefCell::new(HashMap::new()));
            sync_tty_dpms_state(outputs.borrow().as_slice(), &dpms_enabled);
            publish_tty_outputs_snapshot_for_devices(
                &drm_devices.borrow(),
                &active_modes.borrow(),
                &dpms_enabled.borrow(),
                &tuning,
                &state,
            );
            let outputs_for_vblank = outputs.clone();
            let output_frame_pending = Rc::new(RefCell::new(HashMap::new()));
            let output_frame_pending_since =
                Rc::new(RefCell::new(HashMap::<String, Instant>::new()));
            let frame_stats = tty_env_flag("HALLEY_FRAME_STATS")
                .then(|| Rc::new(RefCell::new(TtyFrameStats::new(Instant::now()))));
            if frame_stats.is_some() {
                debug!(
                    "tty frame stats enabled; disable_direct_scanout={} force_composed={}",
                    tty_env_flag("HALLEY_DISABLE_DIRECT_SCANOUT"),
                    tty_env_flag("HALLEY_FORCE_COMPOSED")
                );
            }
            let output_animation_redraw_active = Rc::new(RefCell::new(HashMap::new()));
            let estimated_frame_callbacks =
                Rc::new(RefCell::new(HashMap::<String, Instant>::new()));
            let frame_clocks = Rc::new(RefCell::new(HashMap::<String, TtyFrameClock>::new()));
            sync_tty_frame_clocks(&frame_clocks, outputs.borrow().as_slice());
            let composed_frame_cache = Rc::new(RefCell::new(HashMap::<String, GlesTexture>::new()));
            {
                let mut pending = output_frame_pending.borrow_mut();
                let mut animation_active = output_animation_redraw_active.borrow_mut();
                for output in outputs.borrow().iter() {
                    pending.insert(output.connector_name.clone(), false);
                    animation_active.insert(output.connector_name.clone(), false);
                }
            }
            let scanout_signature = Rc::new(RefCell::new(current_tty_output_signature(
                &outputs.borrow(),
            )));
            let vblank_mismatch_state = Rc::new(RefCell::new(VBlankMismatchState::default()));
            let vblank_mismatch_state_for_notifier = vblank_mismatch_state.clone();
            let output_frame_pending_for_notifier = output_frame_pending.clone();
            let output_frame_pending_since_for_notifier = output_frame_pending_since.clone();
            let output_frame_pending_for_dpms_input = output_frame_pending.clone();
            let output_frame_pending_for_dpms_timer = output_frame_pending.clone();
            let output_frame_pending_since_for_timer = output_frame_pending_since.clone();
            let frame_clocks_for_notifier = frame_clocks.clone();
            let vblank_throttles = Rc::new(RefCell::new(HashMap::<String, VBlankThrottle>::new()));
            let vblank_throttles_for_notifier = vblank_throttles.clone();
            let first_vblank_logged =
                Rc::new(RefCell::new(std::collections::HashSet::<String>::new()));
            let first_vblank_logged_for_notifier = first_vblank_logged.clone();
            let active_modes_for_timer = active_modes.clone();
            let active_modes_for_input = active_modes.clone();
            let active_modes_for_notifier = active_modes.clone();
            let dpms_enabled_for_timer = dpms_enabled.clone();
            let dpms_enabled_for_input = dpms_enabled.clone();
            let dpms_just_woke_outputs = Rc::new(RefCell::new(HashSet::<String>::new()));
            let dpms_just_woke_outputs_for_timer = dpms_just_woke_outputs.clone();
            let dpms_just_woke_outputs_for_input = dpms_just_woke_outputs.clone();
            let backend_handle_for_timer = backend_handle.clone();
            let first_frame_queued =
                Rc::new(RefCell::new(std::collections::HashSet::<String>::new()));
            let first_frame_queued_for_timer = first_frame_queued.clone();
            let outputs_for_input = outputs.clone();
            let outputs_for_timer = outputs.clone();
            let drm_devices_for_input = drm_devices.clone();
            let outputs_for_redraw = outputs.clone();
            let composed_frame_cache_for_redraw = composed_frame_cache.clone();
            let composed_frame_cache_for_timer = composed_frame_cache.clone();
            let estimated_frame_callbacks_for_redraw = estimated_frame_callbacks.clone();
            let estimated_frame_callbacks_for_timer = estimated_frame_callbacks.clone();
            let frame_clocks_for_redraw = frame_clocks.clone();
            let frame_clocks_for_timer = frame_clocks.clone();
            let scanout_signature_for_timer = scanout_signature.clone();
            let output_timer_tick_at = Rc::new(RefCell::new(HashMap::<String, Instant>::new()));
            let output_timer_tick_at_for_timer = output_timer_tick_at.clone();
            let event_loop_handle_for_vblank = ev.handle();
            let (redraw_ping, redraw_source) = make_ping()?;
            let redraw_ping = Rc::new(redraw_ping);
            backend_handle.set_redraw_ping(redraw_ping.clone());
            let redraw_ping_for_vblank = redraw_ping.clone();
            let drm_notifiers = drm_devices
                .borrow_mut()
                .iter_mut()
                .filter_map(|device| {
                    device
                        .notifier
                        .take()
                        .map(|notifier| (device.node, notifier))
                })
                .collect::<Vec<_>>();
            for (notifier_device_node, notifier) in drm_notifiers {
                let outputs_for_vblank = outputs_for_vblank.clone();
                let active_modes_for_notifier = active_modes_for_notifier.clone();
                let output_frame_pending_for_notifier = output_frame_pending_for_notifier.clone();
                let output_frame_pending_since_for_notifier =
                    output_frame_pending_since_for_notifier.clone();
                let frame_clocks_for_notifier = frame_clocks_for_notifier.clone();
                let vblank_throttles_for_notifier = vblank_throttles_for_notifier.clone();
                let event_loop_handle_for_vblank = event_loop_handle_for_vblank.clone();
                let redraw_ping_for_vblank = redraw_ping_for_vblank.clone();
                let first_vblank_logged_for_notifier = first_vblank_logged_for_notifier.clone();
                let vblank_mismatch_state_for_notifier = vblank_mismatch_state_for_notifier.clone();
                let frame_stats_for_notifier = frame_stats.clone();
                ev.handle().insert_source(
                    notifier,
                    move |event, metadata, st| match event {
                        DrmEvent::VBlank(crtc) => {
                        let now = Instant::now();
                        let timestamp = drm_vblank_timestamp(metadata.as_ref());
                        let mut matched_outputs = Vec::new();
                        for output in outputs_for_vblank.borrow().iter() {
                            let initial_crtc = output.crtc;
                            let output_name = output.connector_name.clone();
                            let compositor = output.compositor.clone();
                            if output.device_node != notifier_device_node || crtc != initial_crtc {
                                continue;
                            }
                            let refresh_interval = active_modes_for_notifier
                                .borrow()
                                .get(output_name.as_str())
                                .map(|mode| {
                                    frame_interval_for_refresh_hz(Some(mode.vrefresh() as f64))
                                });
                            let sequence = metadata
                                .as_ref()
                                .map(|metadata| metadata.sequence as u64)
                                .unwrap_or(0);
                            let throttled_output_name = output_name.clone();
                            let redraw_ping_for_throttle = redraw_ping_for_vblank.clone();
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
                                    let output_frame_pending_since_for_notifier =
                                        output_frame_pending_since_for_notifier.clone();
                                    let frame_stats_for_notifier = frame_stats_for_notifier.clone();
                                    let frame_clocks_for_notifier = frame_clocks_for_notifier.clone();
                                    move |state| {
                                        let presentation_time = monotonic_now_duration();
                                        present_tty_frame_feedback(
                                            throttled_output_name.as_str(),
                                            compositor.borrow_mut().frame_submitted(),
                                            presentation_time,
                                            refresh_interval,
                                            sequence,
                                        );
                                        let redraw_needed = frame_clocks_for_notifier
                                            .borrow_mut()
                                            .get_mut(throttled_output_name.as_str())
                                            .is_some_and(|clock| {
                                                clock.presented(
                                                    presentation_time,
                                                    Instant::now(),
                                                    throttled_output_name.as_str(),
                                                )
                                            });
                                        if redraw_needed {
                                            state
                                                .runtime
                                                .tty_redraw_outputs
                                                .insert(throttled_output_name.clone());
                                        }
                                        if let Some(frame_stats) = &frame_stats_for_notifier {
                                            frame_stats.borrow_mut().completed_vblanks += 1;
                                        }
                                        output_frame_pending_for_notifier
                                            .borrow_mut()
                                            .insert(throttled_output_name.clone(), false);
                                        output_frame_pending_since_for_notifier
                                            .borrow_mut()
                                            .remove(throttled_output_name.as_str());
                                        redraw_ping_for_throttle.ping();
                                    }
                                });
                            if should_throttle {
                                continue;
                            }
                            present_tty_frame_feedback(
                                output_name.as_str(),
                                compositor.borrow_mut().frame_submitted(),
                                timestamp,
                                refresh_interval,
                                sequence,
                            );
                            let redraw_needed = frame_clocks_for_notifier
                                .borrow_mut()
                                .get_mut(output_name.as_str())
                                .is_some_and(|clock| {
                                    clock.presented(timestamp, now, output_name.as_str())
                                });
                            if redraw_needed {
                                st.runtime.tty_redraw_outputs.insert(output_name.clone());
                            }
                            if let Some(frame_stats) = &frame_stats_for_notifier {
                                frame_stats.borrow_mut().completed_vblanks += 1;
                            }
                            output_frame_pending_for_notifier
                                .borrow_mut()
                                .insert(output_name.clone(), false);
                            output_frame_pending_since_for_notifier
                                .borrow_mut()
                                .remove(output_name.as_str());
                            redraw_ping_for_vblank.ping();
                            crate::portal::capture_screencast_for_output(st, output_name.as_str());
                            matched_outputs.push(output_name.clone());
                            if first_vblank_logged_for_notifier
                                .borrow_mut()
                                .insert(output_name.clone())
                            {
                                debug!("first drm vblank/frame-done observed for {}", output_name);
                            }
                        }

                        if matched_outputs.is_empty() {
                            if let Some(frame_stats) = &frame_stats_for_notifier {
                                frame_stats.borrow_mut().vblank_mismatches += 1;
                            }
                            let pending_outputs: Vec<_> = outputs_for_vblank
                                .borrow()
                                .iter()
                                .filter(|output| output.device_node == notifier_device_node)
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

                            let mut mismatch_state =
                                vblank_mismatch_state_for_notifier.borrow_mut();
                            let active_for = mismatch_state
                                .first_seen_at
                                .get_or_insert(now)
                                .elapsed();
                            if active_for
                                >= Duration::from_millis(VBLANK_MISMATCH_LOG_AFTER_MS)
                                && !mismatch_state.reported_active
                            {
                                if !recoverable_outputs.is_empty() {
                                    debug!(
                                        "drm vblank crtc mismatch (node={} got={:?}); releasing pending outputs {:?} to keep scanout advancing",
                                        notifier_device_node, crtc, recoverable_outputs
                                    );
                                    let _ = release_pending_tty_outputs(
                                        &outputs_for_vblank,
                                        &output_frame_pending_for_notifier,
                                        &output_frame_pending_since_for_notifier,
                                        recoverable_outputs.as_slice(),
                                        "vblank-crtc-mismatch",
                                    );
                                } else if !pending_outputs.is_empty() {
                                    debug!(
                                        "drm vblank crtc mismatch (node={} got={:?}); keeping pending outputs {:?} blocked until they receive a real matched vblank",
                                        notifier_device_node, crtc, pending_outputs
                                    );
                                } else {
                                    debug!(
                                        "drm vblank crtc mismatch (node={} got={:?}); no configured output matched",
                                        notifier_device_node, crtc
                                    );
                                }
                                mismatch_state.reported_active = true;
                            }
                        } else {
                            let mut mismatch_state = vblank_mismatch_state_for_notifier.borrow_mut();
                            if mismatch_state.reported_active {
                                debug!(
                                    "drm vblank routing recovered on node={} crtc={:?} for {:?}",
                                    notifier_device_node, crtc, matched_outputs
                                );
                            }
                            mismatch_state.first_seen_at = None;
                            mismatch_state.reported_active = false;
                        }
                        }
                        DrmEvent::Error(err) => warn!("drm event error: {}", err),
                    },
                )?;
            }

            let dpms_enabled_for_redraw = dpms_enabled.clone();
            let output_frame_pending_for_redraw = output_frame_pending.clone();
            let output_animation_redraw_active_for_redraw = output_animation_redraw_active.clone();
            let pointer_state_for_redraw = pointer_state.clone();
            let backend_handle_for_redraw = backend_handle.clone();
            let gpu_manager_for_redraw = gpu_manager.clone();
            let primary_render_node_for_redraw = primary_render_node;
            let first_frame_queued_for_redraw = first_frame_queued.clone();
            let frame_stats_for_redraw = frame_stats.clone();
            ev.handle()
                .insert_source(redraw_source, move |_event, _metadata, st| {
                    let now = Instant::now();
                    let animation_redraw_active = tty_animation_redraw_active(
                        st,
                        &outputs_for_redraw,
                        &pointer_state_for_redraw,
                        now,
                    );
                    let animation_output_ready = tty_animation_output_ready_for_redraw(
                        st,
                        &outputs_for_redraw,
                        &dpms_enabled_for_redraw,
                        &output_frame_pending_for_redraw,
                        &pointer_state_for_redraw,
                        now,
                    );
                    let mut eligible_outputs = take_ready_tty_redraw_outputs(
                        &backend_handle_for_redraw,
                        &outputs_for_redraw,
                        &output_frame_pending_for_redraw,
                        &frame_clocks_for_redraw,
                        st,
                    );
                    if animation_redraw_active && animation_output_ready {
                        eligible_outputs.extend(tty_animation_redraw_outputs(
                            st,
                            &outputs_for_redraw,
                            &pointer_state_for_redraw,
                            now,
                        ));
                    }
                    let eligible_includes_animation = tty_outputs_include_animation_redraw(
                        st,
                        &pointer_state_for_redraw,
                        &eligible_outputs,
                        now,
                    );
                    if !eligible_outputs.is_empty()
                        && (!animation_redraw_active || eligible_includes_animation)
                    {
                        advance_tty_redraw_frame(st, &pointer_state_for_redraw, now, false);
                    }
                    if eligible_outputs.is_empty() {
                        return;
                    }
                    let ps = pointer_state_for_redraw.borrow();
                    let resize_preview = ps.resize;
                    drop(ps);
                    queue_ready_tty_outputs(
                        &outputs_for_redraw,
                        &dpms_enabled_for_redraw,
                        &output_frame_pending_for_redraw,
                        &output_frame_pending_since,
                        &output_animation_redraw_active_for_redraw,
                        &estimated_frame_callbacks_for_redraw,
                        &frame_clocks_for_redraw,
                        &composed_frame_cache_for_redraw,
                        &pointer_state_for_redraw,
                        &gpu_manager_for_redraw,
                        primary_render_node_for_redraw,
                        &first_frame_queued_for_redraw,
                        frame_stats_for_redraw.as_ref(),
                        st,
                        now,
                        resize_preview,
                        Some(&eligible_outputs),
                        "redraw",
                    );
                })?;

            let redraw_ping_for_maintenance = redraw_ping.clone();
            let (maintenance_ping, maintenance_source) = make_ping()?;
            state.runtime.maintenance_ping = Some(maintenance_ping);
            ev.handle()
                .insert_source(maintenance_source, move |_event, _metadata, st| {
                    st.run_maintenance_if_needed(Instant::now());
                    redraw_ping_for_maintenance.ping();
                })?;

            ev.handle()
                .insert_source(libinput_backend, move |event, _, st| match event {
                    InputEvent::Keyboard { event } => {
                        let tuning = st.runtime.tuning.clone();
                        let wake_output = st.focused_monitor().to_string();
                        wake_tty_dpms_on_input(
                            &drm_devices_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &outputs_for_input,
                            &tuning,
                            &output_frame_pending_for_dpms_input,
                            &dpms_just_woke_outputs_for_input,
                            Some(wake_output.as_str()),
                            st,
                        );
                        if !*keyboard_seen_for_input.borrow() {
                            debug!("tty input: first keyboard event received");
                            *keyboard_seen_for_input.borrow_mut() = true;
                        }
                        // Smithay's libinput backend already returns XKB
                        // keycodes here (evdev + 8). Do not add another +8
                        // or compositor bindings and client key delivery will
                        // both stop matching.
                        let code: u32 = event.key_code().into();
                        let pressed = event.state() == KeyState::Pressed;
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::Keyboard { code, pressed },
                        );
                    }
                    InputEvent::PointerMotionAbsolute { event } => {
                        let (ws_w, ws_h) = backend_handle.window_size_i32();
                        // Map the normalised [0,1] absolute position onto the
                        // monitor the pointer device physically covers, then
                        // offset into the combined layout.  Using the full
                        // layout dimensions here would stretch [0,1] across all
                        // monitors and lock the pointer to only the leftmost one.
                        let (mon_w, mon_h, mon_ox, mon_oy) = primary_tty_monitor_dims(
                            st.model.monitor_state.current_monitor.as_str(),
                            &st.runtime.tuning,
                            outputs_for_input.borrow().as_slice(),
                        );
                        let sx = mon_ox as f32 + event.x_transformed(mon_w) as f32;
                        let sy = mon_oy as f32 + event.y_transformed(mon_h) as f32;
                        let tuning = st.runtime.tuning.clone();
                        let wake_output = st.monitor_for_screen(sx, sy);
                        wake_tty_dpms_on_input(
                            &drm_devices_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &outputs_for_input,
                            &tuning,
                            &output_frame_pending_for_dpms_input,
                            &dpms_just_woke_outputs_for_input,
                            wake_output.as_deref(),
                            st,
                        );
                        if !*pointer_seen_for_input.borrow() {
                            debug!("tty input: first pointer event received");
                            *pointer_seen_for_input.borrow_mut() = true;
                        }
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
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
                        let tuning = st.runtime.tuning.clone();
                        let (ws_w, ws_h) = backend_handle.window_size_i32();
                        if let Some((hint_sx, hint_sy)) =
                            crate::compositor::interaction::state::take_pointer_screen_hint_request(
                                st,
                            )
                        {
                            let mut ps = pointer_state_for_input.borrow_mut();
                            ps.workspace_size = (ws_w, ws_h);
                            ps.screen = (hint_sx, hint_sy);
                            ps.world = crate::spatial::screen_to_world(
                                st,
                                ws_w.max(1),
                                ws_h.max(1),
                                hint_sx,
                                hint_sy,
                            );
                        }
                        let (last_sx, last_sy) = pointer_state_for_input.borrow().screen;
                        let sx = last_sx + event.delta_x() as f32;
                        let sy = last_sy + event.delta_y() as f32;
                        let wake_output = st.monitor_for_screen(sx, sy);
                        wake_tty_dpms_on_input(
                            &drm_devices_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &outputs_for_input,
                            &tuning,
                            &output_frame_pending_for_dpms_input,
                            &dpms_just_woke_outputs_for_input,
                            wake_output.as_deref(),
                            st,
                        );
                        if !*pointer_seen_for_input.borrow() {
                            debug!("tty input: first pointer event received");
                            *pointer_seen_for_input.borrow_mut() = true;
                        }
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        // Raw libinput delta straight off the device, before any
                        // routing/constraint handling. Compare against the
                        // `locked_relative` delta to see if motion is quantized or
                        // shrunk on the way to a locked game (TF2 mouselook debug).
                        if std::env::var_os("HALLEY_POINTER_TRACE")
                            .is_some_and(|value| value != "0")
                        {
                            eventline::info!(
                                "libinput_motion delta={:.3},{:.3} unaccel={:.3},{:.3}",
                                event.delta_x(),
                                event.delta_y(),
                                event.delta_x_unaccel(),
                                event.delta_y_unaccel(),
                            );
                        }
                        handle_backend_input_event(
                            st,
                            &input_ctx,
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
                        let tuning = st.runtime.tuning.clone();
                        let (sx, sy) = pointer_state_for_input.borrow().screen;
                        let wake_output = st.monitor_for_screen(sx, sy);
                        wake_tty_dpms_on_input(
                            &drm_devices_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &outputs_for_input,
                            &tuning,
                            &output_frame_pending_for_dpms_input,
                            &dpms_just_woke_outputs_for_input,
                            wake_output.as_deref(),
                            st,
                        );
                        if !*pointer_seen_for_input.borrow() {
                            debug!("tty input: first pointer event received");
                            *pointer_seen_for_input.borrow_mut() = true;
                        }
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
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
                    InputEvent::PointerAxis { event } => {
                        let tuning = st.runtime.tuning.clone();
                        let (sx, sy) = pointer_state_for_input.borrow().screen;
                        let wake_output = st.monitor_for_screen(sx, sy);
                        wake_tty_dpms_on_input(
                            &drm_devices_for_input,
                            &active_modes_for_input,
                            &dpms_enabled_for_input,
                            &outputs_for_input,
                            &tuning,
                            &output_frame_pending_for_dpms_input,
                            &dpms_just_woke_outputs_for_input,
                            wake_output.as_deref(),
                            st,
                        );
                        if !*pointer_seen_for_input.borrow() {
                            debug!("tty input: first pointer event received");
                            *pointer_seen_for_input.borrow_mut() = true;
                        }
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
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
                    InputEvent::GestureSwipeBegin { event } => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::GestureSwipeBegin {
                                fingers: event.fingers(),
                                time_msec: event.time_msec(),
                            },
                        );
                    }
                    InputEvent::GestureSwipeUpdate { event } => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::GestureSwipeUpdate {
                                delta_x: event.delta_x(),
                                delta_y: event.delta_y(),
                                time_msec: event.time_msec(),
                            },
                        );
                    }
                    InputEvent::GestureSwipeEnd { event } => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::GestureSwipeEnd {
                                cancelled: event.cancelled(),
                                time_msec: event.time_msec(),
                            },
                        );
                    }
                    InputEvent::GesturePinchBegin { event } => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::GesturePinchBegin {
                                fingers: event.fingers(),
                                time_msec: event.time_msec(),
                            },
                        );
                    }
                    InputEvent::GesturePinchUpdate { event } => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::GesturePinchUpdate {
                                delta_x: event.delta_x(),
                                delta_y: event.delta_y(),
                                scale: event.scale(),
                                rotation: event.rotation(),
                                time_msec: event.time_msec(),
                            },
                        );
                    }
                    InputEvent::GesturePinchEnd { event } => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::GesturePinchEnd {
                                cancelled: event.cancelled(),
                                time_msec: event.time_msec(),
                            },
                        );
                    }
                    InputEvent::GestureHoldBegin { event } => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::GestureHoldBegin {
                                fingers: event.fingers(),
                                time_msec: event.time_msec(),
                            },
                        );
                    }
                    InputEvent::GestureHoldEnd { event } => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::GestureHoldEnd {
                                cancelled: event.cancelled(),
                                time_msec: event.time_msec(),
                            },
                        );
                    }
                    InputEvent::TouchDown { event } => {
                        let (mon_w, mon_h, mon_ox, mon_oy) = primary_tty_monitor_dims(
                            st.model.monitor_state.current_monitor.as_str(),
                            &st.runtime.tuning,
                            outputs_for_input.borrow().as_slice(),
                        );
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::TouchDown {
                                ws_w: mon_w,
                                ws_h: mon_h,
                                slot: event.slot(),
                                sx: mon_ox as f32 + event.x_transformed(mon_w) as f32,
                                sy: mon_oy as f32 + event.y_transformed(mon_h) as f32,
                                time_msec: event.time_msec(),
                            },
                        );
                    }
                    InputEvent::TouchMotion { event } => {
                        let (mon_w, mon_h, mon_ox, mon_oy) = primary_tty_monitor_dims(
                            st.model.monitor_state.current_monitor.as_str(),
                            &st.runtime.tuning,
                            outputs_for_input.borrow().as_slice(),
                        );
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::TouchMotion {
                                ws_w: mon_w,
                                ws_h: mon_h,
                                slot: event.slot(),
                                sx: mon_ox as f32 + event.x_transformed(mon_w) as f32,
                                sy: mon_oy as f32 + event.y_transformed(mon_h) as f32,
                                time_msec: event.time_msec(),
                            },
                        );
                    }
                    InputEvent::TouchUp { event } => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::TouchUp {
                                slot: event.slot(),
                                time_msec: event.time_msec(),
                            },
                        );
                    }
                    InputEvent::TouchFrame { .. } => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::TouchFrame,
                        );
                    }
                    InputEvent::TouchCancel { .. } => {
                        let input_ctx = InputCtx {
                            mod_state: &mod_state_for_input,
                            pointer_state: &pointer_state_for_input,
                            backend: &backend_handle,
                            config_path: config_path.as_str(),
                            wayland_display: sock_name.as_str(),
                        };
                        handle_backend_input_event(
                            st,
                            &input_ctx,
                            BackendInputEventData::TouchCancel,
                        );
                    }
                    InputEvent::DeviceAdded { mut device } => {
                        if device.has_capability(DeviceCapability::Touch.into())
                            && st.platform.seat.get_touch().is_none()
                        {
                            st.platform.seat.add_touch();
                        }
                        crate::input::device_config::apply_device_config(
                            &mut device,
                            &st.runtime.tuning.input,
                        );
                        st.input.devices.push(device);
                    }
                    InputEvent::DeviceRemoved { device } => {
                        st.input.devices.retain(|d| d != &device);
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
            let portal_sync_timer = Timer::from_duration(Duration::from_millis(750));
            ev.handle()
                .insert_source(portal_sync_timer, move |_tick, _, _st| {
                    refresh_portal_services_nonblocking();
                    TimeoutAction::Drop
                })?;
            let timer = Timer::from_duration(initial_frame_interval);
            let gpu_manager_for_timer = gpu_manager.clone();
            let primary_render_node_for_timer = primary_render_node;
            let drm_devices_for_timer = drm_devices.clone();
            let frame_stats_for_timer = frame_stats.clone();

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

                drain_ipc_commands_with_fds(|request, fds| match request {
                    halley_api::Request::Compositor(halley_api::CompositorRequest::Quit) => {
                        info!("ipc: quit requested");
                        exit_confirm::show(&mut *st);
                        halley_api::Response::Ok
                    }
                    halley_api::Request::Compositor(halley_api::CompositorRequest::Reload) => {
                        let _ = crate::aperture::reload_aperture_config(
                            st,
                            aperture_config_path_for_timer.as_path(),
                            "ipc",
                        );
                        match RuntimeTuning::try_load_from_path_diagnostic(config_path_for_timer.as_str()) {
                            Ok(next) => {
                                if crate::bootstrap::viewport_section_changed(&st.runtime.tuning, &next) {
                                    apply_tty_reload(
                                        &drm_devices_for_timer,
                                        &gpu_manager_for_timer,
                                        primary_render_node_for_timer,
                                        &outputs_for_timer,
                                        &backend_handle_for_timer,
                                        &pointer_state_for_timer,
                                        st,
                                        next,
                                        config_path_for_timer.as_str(),
                                        wayland_display_for_timer.as_str(),
                                        "ipc",
                                        &active_modes_for_timer,
                                        &dpms_enabled_for_timer,
                                        &output_frame_pending_for_dpms_timer,
                                        &output_frame_pending_since_for_timer,
                                        &output_animation_redraw_active,
                                        &frame_clocks_for_timer,
                                        &scanout_signature_for_timer,
                                    );
                                } else {
                                    crate::bootstrap::apply_reloaded_tuning(
                                        st,
                                        next,
                                        config_path_for_timer.as_str(),
                                        wayland_display_for_timer.as_str(),
                                        "ipc",
                                    );
                                }
                            }
                            Err(err) => {
                                crate::bootstrap::show_config_reload_error(st, &err);
                                warn!(
                                    "ipc: reload skipped for {} because {}",
                                    config_path_for_timer.as_str(), err.message
                                );
                            }
                        }
                        debug!("resolved keybinds: {}", st.runtime.tuning.keybinds_resolved_summary());
                        debug!("resolved zoom: {}", st.runtime.tuning.zoom_resolved_summary());
                        halley_api::Response::Reloaded
                    }
                    halley_api::Request::Compositor(halley_api::CompositorRequest::Dpms {
                        command,
                        output,
                    }) => {
                        let tuning = st.runtime.tuning.clone();
                        let changed = apply_tty_dpms_command(
                            &drm_devices_for_timer,
                            &active_modes_for_timer,
                            &dpms_enabled_for_timer,
                            command,
                            output.as_deref(),
                            &outputs_for_timer,
                            &tuning,
                            &output_frame_pending_for_dpms_timer,
                            &dpms_just_woke_outputs_for_timer,
                            st,
                        );
                        if changed {
                            halley_api::Response::Ok
                        } else {
                            halley_api::Response::Error(halley_api::ApiError::NotFound(
                                "dpms request made no change".into(),
                            ))
                        }
                    }
                    request => crate::ipc::handle_request_with_fds(st, request, fds),
                });

                xwayland_for_timer.borrow_mut().tick();
                if let Err(err) = install_xwayland_socket_watchers(
                    &xwayland_event_loop_for_timer,
                    &xwayland_for_timer,
                    &xwayland_watch_tokens_for_timer,
                ) {
                    warn!("failed to register X11 socket watchers: {}", err);
                }
                st.run_maintenance_if_needed(now);

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
                    reloaded |= crate::aperture::reload_aperture_config(
                        st,
                        aperture_config_path_for_timer.as_path(),
                        "watch",
                    );
                    match RuntimeTuning::try_load_from_path_diagnostic(config_path_for_timer.as_str()) {
                        Ok(next) => {
                            if crate::bootstrap::viewport_section_changed(&st.runtime.tuning, &next) {
                                apply_tty_reload(
                                    &drm_devices_for_timer,
                                    &gpu_manager_for_timer,
                                    primary_render_node_for_timer,
                                    &outputs_for_timer,
                                    &backend_handle_for_timer,
                                    &pointer_state_for_timer,
                                    st,
                                    next,
                                    config_path_for_timer.as_str(),
                                    wayland_display_for_timer.as_str(),
                                    "watch",
                                    &active_modes_for_timer,
                                    &dpms_enabled_for_timer,
                                    &output_frame_pending_for_dpms_timer,
                                    &output_frame_pending_since_for_timer,
                                    &output_animation_redraw_active,
                                    &frame_clocks_for_timer,
                                    &scanout_signature_for_timer,
                                );
                            } else {
                                crate::bootstrap::apply_reloaded_tuning(
                                    st,
                                    next,
                                    config_path_for_timer.as_str(),
                                    wayland_display_for_timer.as_str(),
                                    "watch",
                                );
                            }
                            reloaded = true;
                        }
                        Err(err) => {
                            crate::bootstrap::show_config_reload_error(st, &err);
                            warn!(
                                "watch: reload skipped for {} because {}",
                                config_path_for_timer.as_str(), err.message
                            );
                        }
                    }
                }
                if pending_output_rescan_at_for_timer
                    .borrow()
                    .is_some_and(|deadline| now >= deadline)
                {
                    *pending_output_rescan_at_for_timer.borrow_mut() = None;
                if any_tty_output_dpms_enabled(&dpms_enabled_for_timer.borrow()) {
                    let next = st.runtime.tuning.clone();
                        apply_tty_reload(
                            &drm_devices_for_timer,
                            &gpu_manager_for_timer,
                            primary_render_node_for_timer,
                            &outputs_for_timer,
                        &backend_handle_for_timer,
                        &pointer_state_for_timer,
                        st,
                        next,
                        config_path_for_timer.as_str(),
                        wayland_display_for_timer.as_str(),
                            "rescan",
                            &active_modes_for_timer,
                            &dpms_enabled_for_timer,
                            &output_frame_pending_for_dpms_timer,
                            &output_frame_pending_since_for_timer,
                            &output_animation_redraw_active,
                            &frame_clocks_for_timer,
                            &scanout_signature_for_timer,
                        );
                } else {
                    // Reschedule for after wake — just leave pending cleared,
                    // wake will trigger its own rescan via the output signature check.
                    debug!("rescan deferred: dpms is off");
                }

                }
                if reloaded {
                    debug!("resolved keybinds: {}", st.runtime.tuning.keybinds_resolved_summary());
                    debug!("resolved zoom: {}", st.runtime.tuning.zoom_resolved_summary());
                }

                let ps = pointer_state_for_timer.borrow();
                let resize_preview = ps.resize;
                drop(ps);
                let stalled_outputs: Vec<String> = output_frame_pending_since_for_timer
                    .borrow()
                    .iter()
                    .filter_map(|(output_name, queued_at)| {
                        (now.saturating_duration_since(*queued_at)
                            >= Duration::from_millis(PENDING_FRAME_TIMEOUT_MS))
                        .then_some(output_name.clone())
                    })
                    .collect();
                if !stalled_outputs.is_empty() {
                    warn!(
                        "releasing stalled tty frames after {:?} for {:?}",
                        Duration::from_millis(PENDING_FRAME_TIMEOUT_MS),
                        stalled_outputs
                    );
                    let released = release_pending_tty_outputs(
                        &outputs_for_timer,
                        &output_frame_pending_for_dpms_timer,
                        &output_frame_pending_since_for_timer,
                        stalled_outputs.as_slice(),
                        "pending-frame-timeout",
                    );
                    if let Some(frame_stats) = &frame_stats_for_timer {
                        let mut stats = frame_stats.borrow_mut();
                        stats.page_flip_timeouts += stalled_outputs.len() as u64;
                        stats.page_flip_recoveries += released as u64;
                    }
                }
                let due_outputs = tty_due_outputs_for_timer(
                    &outputs_for_timer,
                    &active_modes_for_timer,
                    &dpms_enabled_for_timer,
                    &output_frame_pending_for_dpms_timer,
                    &output_timer_tick_at_for_timer,
                    now,
                );

                if any_tty_output_dpms_enabled(&dpms_enabled_for_timer.borrow()) {
                    send_due_estimated_frame_callbacks(
                        &estimated_frame_callbacks_for_timer,
                        &frame_clocks_for_timer,
                        &output_frame_pending_for_dpms_timer,
                        st,
                        now,
                    );

                    // On the first tick after DPMS wake, re-configure layer shell
                    // surfaces. Frame callbacks are sent only after a scanout frame queues.
                    if !dpms_just_woke_outputs_for_timer.borrow().is_empty() {
                        let woke_outputs: Vec<String> = dpms_just_woke_outputs_for_timer
                            .borrow()
                            .iter()
                            .cloned()
                            .collect();
                        st.input.interaction_state.dpms_just_woke = false;
                        dpms_just_woke_outputs_for_timer.borrow_mut().clear();
                        crate::compositor::monitor::layer_shell::configure_layer_shell_surfaces(
                            st,
                            (1, 1).into(),
                        );
                        st.runtime.tty_redraw_outputs.extend(woke_outputs);
                    }

                    let frame_callback_due_outputs: HashSet<String> = due_outputs
                        .iter()
                        .filter(|output_name| {
                            crate::frame_loop::output_has_pending_frame_callbacks(
                                st,
                                output_name.as_str(),
                            )
                        })
                        .cloned()
                        .collect();

                    if !due_outputs.is_empty() || !st.runtime.tty_redraw_outputs.is_empty() {
                        let animation_redraw_active = tty_animation_redraw_active(
                            st,
                            &outputs_for_timer,
                            &pointer_state_for_timer,
                            now,
                        );
                        let mut eligible_outputs = take_ready_tty_redraw_outputs(
                            &backend_handle_for_timer,
                            &outputs_for_timer,
                            &output_frame_pending_for_dpms_timer,
                            &frame_clocks_for_timer,
                            st,
                        );
                        eligible_outputs.extend(frame_callback_due_outputs);
                        if animation_redraw_active {
                            eligible_outputs.extend(tty_ready_animation_redraw_outputs(
                                st,
                                &outputs_for_timer,
                                &dpms_enabled_for_timer,
                                &output_frame_pending_for_dpms_timer,
                                &pointer_state_for_timer,
                                now,
                            ));
                        }
                        if !eligible_outputs.is_empty() {
                            let eligible_includes_animation = tty_outputs_include_animation_redraw(
                                st,
                                &pointer_state_for_timer,
                                &eligible_outputs,
                                now,
                            );
                            if !animation_redraw_active || eligible_includes_animation {
                                advance_tty_redraw_frame(st, &pointer_state_for_timer, now, false);
                            }
                            queue_ready_tty_outputs(
                                &outputs_for_timer,
                                &dpms_enabled_for_timer,
                                &output_frame_pending,
                                &output_frame_pending_since_for_timer,
                                &output_animation_redraw_active,
                                &estimated_frame_callbacks_for_timer,
                                &frame_clocks_for_timer,
                                &composed_frame_cache_for_timer,
                                &pointer_state_for_timer,
                                &gpu_manager_for_timer,
                                primary_render_node_for_timer,
                                &first_frame_queued_for_timer,
                                frame_stats_for_timer.as_ref(),
                                st,
                                now,
                                resize_preview,
                                Some(&eligible_outputs),
                                "timer",
                            );
                        }
                    }
                }

                maybe_log_tty_frame_stats(
                    frame_stats_for_timer.as_ref(),
                    &output_frame_pending_since_for_timer,
                    now,
                );

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
                    debug!(
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

#[allow(clippy::type_complexity)]
pub(crate) fn build_tty_libinput_backend(
    session: Rc<RefCell<LibSeatSession>>,
    seat: &str,
) -> Result<(LibinputInputBackend, Rc<RefCell<Libinput>>), Box<dyn Error>> {
    let mut context = Libinput::new_with_udev(LibinputSessionInterface::from(session));
    context
        .udev_assign_seat(seat)
        .map_err(|_| io::Error::other(format!("libinput seat assign failed for {}", seat)))?;
    let context_handle = Rc::new(RefCell::new(context.clone()));
    Ok((LibinputInputBackend::new(context), context_handle))
}
