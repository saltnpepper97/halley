mod dpms;
mod drm;

use super::*;

use crate::input::ctx::InputCtx;
use halley_config::{ViewportOutputConfig, ViewportVrrMode};
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
    TtyDrmDevice, TtyDrmOutput, TtyFrameQueueReport, TtyGpuManager, TtyOutputCaptureBackend,
    build_tty_dmabuf_output_feedbacks, current_tty_output_signature,
    probe_tty_drm_device_via_session, queue_tty_drm_frame, rebuild_tty_outputs,
    selected_tty_scanout_signature,
};
use crate::backend::vblank_throttle::VBlankThrottle;
use crate::compositor::exit_confirm::exit_confirm_controller;
use crate::compositor::interaction::ResizeCtx;
use calloop::{Interest, Mode, PostAction, generic::Generic, ping::make_ping};
use smithay::backend::drm::{DrmEventMetadata, DrmEventTime, DrmNode};
use smithay::backend::renderer::gles::GlesTexture;

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, Event, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent,
    PointerButtonEvent, PointerMotionEvent,
};

const CONFIG_RELOAD_SETTLE_MS: u64 = 100;
const OUTPUT_RESCAN_POLL_MS: u64 = 750;
const VBLANK_MISMATCH_LOG_AFTER_MS: u64 = 1_000;
const PENDING_FRAME_TIMEOUT_MS: u64 = 1_500;
const FRAME_STATS_LOG_INTERVAL_SECS: u64 = 10;

const HALLEY_X11_DISPLAY_NUM: u32 = 0;

#[derive(Clone, Debug, Default)]
struct VBlankMismatchState {
    first_seen_at: Option<Instant>,
    reported_active: bool,
}

#[derive(Debug)]
struct TtyFrameStats {
    last_report_at: Instant,
    queued_frames: u64,
    completed_vblanks: u64,
    page_flip_timeouts: u64,
    page_flip_recoveries: u64,
    vblank_mismatches: u64,
    direct_scanout_frames: u64,
    composed_frames: u64,
    sync_wait_count: u64,
    sync_wait_total_ns: u128,
    max_sync_wait: Duration,
}

impl TtyFrameStats {
    fn new(now: Instant) -> Self {
        Self {
            last_report_at: now,
            queued_frames: 0,
            completed_vblanks: 0,
            page_flip_timeouts: 0,
            page_flip_recoveries: 0,
            vblank_mismatches: 0,
            direct_scanout_frames: 0,
            composed_frames: 0,
            sync_wait_count: 0,
            sync_wait_total_ns: 0,
            max_sync_wait: Duration::ZERO,
        }
    }
}

fn tty_env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn monotonic_now_duration() -> Duration {
    smithay::utils::Clock::<smithay::utils::Monotonic>::new()
        .now()
        .into()
}

fn drm_vblank_timestamp(metadata: Option<&DrmEventMetadata>) -> Duration {
    if let Some(metadata) = metadata {
        if let DrmEventTime::Monotonic(timestamp) = metadata.time {
            return timestamp;
        }
    }

    monotonic_now_duration()
}

fn record_tty_frame_queue(
    frame_stats: Option<&Rc<RefCell<TtyFrameStats>>>,
    report: &TtyFrameQueueReport,
) {
    let Some(frame_stats) = frame_stats else {
        return;
    };
    if !report.queued {
        return;
    }

    let mut stats = frame_stats.borrow_mut();
    stats.queued_frames += 1;
    if report.direct_scanout_active {
        stats.direct_scanout_frames += 1;
    }
    if report.composed {
        stats.composed_frames += 1;
    }
    if let Some(wait) = report.sync_wait {
        stats.sync_wait_count += 1;
        stats.sync_wait_total_ns += wait.as_nanos();
        stats.max_sync_wait = stats.max_sync_wait.max(wait);
    }
}

fn maybe_log_tty_frame_stats(
    frame_stats: Option<&Rc<RefCell<TtyFrameStats>>>,
    output_frame_pending_since: &Rc<RefCell<HashMap<String, Instant>>>,
    now: Instant,
) {
    let Some(frame_stats) = frame_stats else {
        return;
    };

    let mut stats = frame_stats.borrow_mut();
    if now.saturating_duration_since(stats.last_report_at)
        < Duration::from_secs(FRAME_STATS_LOG_INTERVAL_SECS)
    {
        return;
    }

    let pending_since = output_frame_pending_since.borrow();
    let pending_frames = pending_since.len();
    let max_pending_age = pending_since
        .values()
        .map(|queued_at| now.saturating_duration_since(*queued_at))
        .max()
        .unwrap_or(Duration::ZERO);
    let avg_sync_wait = if stats.sync_wait_count == 0 {
        Duration::ZERO
    } else {
        Duration::from_nanos((stats.sync_wait_total_ns / stats.sync_wait_count as u128) as u64)
    };

    debug!(
        "tty frame stats: queued={} completed_vblanks={} page_flip_timeouts={} page_flip_recoveries={} vblank_mismatches={} direct_scanout={} composed={} sync_waits={} avg_sync_wait={:?} max_sync_wait={:?} pending_frames={} max_pending_age={:?}",
        stats.queued_frames,
        stats.completed_vblanks,
        stats.page_flip_timeouts,
        stats.page_flip_recoveries,
        stats.vblank_mismatches,
        stats.direct_scanout_frames,
        stats.composed_frames,
        stats.sync_wait_count,
        avg_sync_wait,
        stats.max_sync_wait,
        pending_frames,
        max_pending_age
    );
    stats.last_report_at = now;
}

fn queue_ready_tty_outputs(
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    dpms_enabled: &Rc<RefCell<HashMap<String, bool>>>,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    output_frame_pending_since: &Rc<RefCell<HashMap<String, Instant>>>,
    output_animation_redraw_active: &Rc<RefCell<HashMap<String, bool>>>,
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
    source: &str,
) {
    if !any_tty_output_dpms_enabled(&dpms_enabled.borrow()) {
        return;
    }

    let cursor_image = st.effective_cursor_image_status();
    let previous_monitor = st.model.monitor_state.current_monitor.clone();

    let outputs_ref = outputs.borrow();
    let mut render_order: Vec<_> = outputs_ref.iter().collect();
    render_order.sort_by_key(|output| output.mode.vrefresh());

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
                    if source == "timer" {
                        crate::frame_loop::send_frame_callbacks_for_output(st, output_name, now);
                    }
                    continue;
                }
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

fn tty_animation_redraw_active(
    st: &Halley,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    pointer_state: &Rc<RefCell<PointerState>>,
    now: Instant,
) -> bool {
    outputs.borrow().iter().any(|output| {
        tty_output_animation_redraw_active(st, pointer_state, output.connector_name.as_str(), now)
    })
}

fn tty_animation_redraw_outputs(
    st: &Halley,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    pointer_state: &Rc<RefCell<PointerState>>,
    now: Instant,
) -> HashSet<String> {
    outputs
        .borrow()
        .iter()
        .filter_map(|output| {
            tty_output_animation_redraw_active(
                st,
                pointer_state,
                output.connector_name.as_str(),
                now,
            )
            .then_some(output.connector_name.clone())
        })
        .collect()
}

fn tty_output_animation_redraw_active(
    st: &Halley,
    pointer_state: &Rc<RefCell<PointerState>>,
    output_name: &str,
    now: Instant,
) -> bool {
    if !pointer_state.borrow().move_anim.is_empty() {
        return true;
    }

    crate::frame_loop::tty_output_animation_redraw_state(st, output_name, now).active
}

fn tty_animation_output_ready_for_redraw(
    st: &Halley,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    dpms_enabled: &Rc<RefCell<HashMap<String, bool>>>,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    pointer_state: &Rc<RefCell<PointerState>>,
    now: Instant,
) -> bool {
    let outputs_ref = outputs.borrow();
    let dpms_ref = dpms_enabled.borrow();
    let pending_ref = output_frame_pending.borrow();

    outputs_ref.iter().any(|output| {
        dpms_ref
            .get(output.connector_name.as_str())
            .copied()
            .unwrap_or(true)
            && !pending_ref
                .get(output.connector_name.as_str())
                .copied()
                .unwrap_or(false)
            && tty_output_animation_redraw_active(
                st,
                pointer_state,
                output.connector_name.as_str(),
                now,
            )
    })
}

fn tty_due_outputs_for_timer(
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: &Rc<RefCell<HashMap<String, bool>>>,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    output_timer_tick_at: &Rc<RefCell<HashMap<String, Instant>>>,
    now: Instant,
) -> HashSet<String> {
    let outputs_ref = outputs.borrow();
    let modes_ref = active_modes.borrow();
    let dpms_ref = dpms_enabled.borrow();
    let pending_ref = output_frame_pending.borrow();
    let mut last_tick_ref = output_timer_tick_at.borrow_mut();

    last_tick_ref.retain(|name, _| {
        outputs_ref
            .iter()
            .any(|output| output.connector_name == *name)
    });

    outputs_ref
        .iter()
        .filter_map(|output| {
            let output_name = output.connector_name.as_str();
            if !dpms_ref.get(output_name).copied().unwrap_or(true)
                || pending_ref.get(output_name).copied().unwrap_or(false)
            {
                return None;
            }

            let refresh_hz = modes_ref
                .get(output_name)
                .map(|mode| mode.vrefresh() as f64)
                .or(Some(output.mode.vrefresh() as f64));
            let interval = frame_interval_for_refresh_hz(refresh_hz);
            let due = last_tick_ref
                .get(output_name)
                .is_none_or(|last| now.saturating_duration_since(*last) >= interval);
            if !due {
                return None;
            }

            last_tick_ref.insert(output.connector_name.clone(), now);
            Some(output.connector_name.clone())
        })
        .collect()
}

fn advance_tty_redraw_frame(
    st: &mut Halley,
    pointer_state: &Rc<RefCell<PointerState>>,
    now: Instant,
    include_maintenance: bool,
) {
    st.drain_drm_syncobj_blockers();

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
        if let Err(err) = std::fs::remove_file(path) {
            if err.kind() != io::ErrorKind::NotFound {
                warn!(
                    "failed to remove stale X11 path {}: {}",
                    path.display(),
                    err
                );
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

fn bootstrap_tty_viewports(outputs: &[TtyDrmOutput]) -> Vec<ViewportOutputConfig> {
    let mut ordered: Vec<_> = outputs
        .iter()
        .map(|output| {
            let (width, height) = output.mode.size();
            (
                output.connector_name.clone(),
                width as u32,
                height as u32,
                output.mode.vrefresh() as f64,
            )
        })
        .collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0));

    let mut offset_x = 0;
    ordered
        .into_iter()
        .map(|(connector, width, height, refresh_rate)| {
            let viewport = ViewportOutputConfig {
                connector,
                enabled: true,
                offset_x,
                offset_y: 0,
                width,
                height,
                refresh_rate: Some(refresh_rate),
                transform_degrees: 0,
                vrr: ViewportVrrMode::Off,
                focus_ring: None,
            };
            offset_x += width as i32;
            viewport
        })
        .collect()
}

fn effective_tty_viewports_for_outputs(
    tuning: &RuntimeTuning,
    outputs: &[TtyDrmOutput],
) -> Vec<ViewportOutputConfig> {
    let active_names = active_output_names(outputs);
    let configured: Vec<_> = tuning
        .tty_viewports
        .iter()
        .filter(|viewport| viewport.enabled)
        .filter(|viewport| active_names.iter().any(|name| name == &viewport.connector))
        .cloned()
        .collect();
    if !configured.is_empty() {
        return configured;
    }

    bootstrap_tty_viewports(outputs)
}

fn effective_tty_viewport_fallback_reason(
    tuning: &RuntimeTuning,
    outputs: &[TtyDrmOutput],
) -> Option<&'static str> {
    let active_names = active_output_names(outputs);
    let enabled_configured = tuning
        .tty_viewports
        .iter()
        .filter(|viewport| viewport.enabled);
    let matched = enabled_configured
        .clone()
        .any(|viewport| active_names.iter().any(|name| name == &viewport.connector));
    if matched {
        return None;
    }

    if tuning.tty_viewports.is_empty() {
        Some("no viewport outputs configured")
    } else if tuning
        .tty_viewports
        .iter()
        .all(|viewport| !viewport.enabled)
    {
        Some("viewport outputs configured but none are enabled")
    } else {
        Some("no enabled viewport outputs matched detected outputs")
    }
}

fn log_effective_tty_viewport_fallback(
    tuning: &RuntimeTuning,
    outputs: &[TtyDrmOutput],
    source: &str,
) {
    let Some(reason) = effective_tty_viewport_fallback_reason(tuning, outputs) else {
        return;
    };
    let layout = effective_tty_viewports_for_outputs(tuning, outputs)
        .into_iter()
        .map(|viewport| {
            let refresh = viewport
                .refresh_rate
                .map(|hz| format!("@{hz:.3}Hz"))
                .unwrap_or_default();
            format!(
                "{}={}x{}{}+{}+{}",
                viewport.connector,
                viewport.width,
                viewport.height,
                refresh,
                viewport.offset_x,
                viewport.offset_y,
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    warn!(
        "{}: tty monitor fallback active: {}; derived layout [{}]",
        source, reason, layout
    );
}

fn effective_tty_viewport_for_output<'a>(
    tuning: &RuntimeTuning,
    outputs: &'a [TtyDrmOutput],
    output_name: &str,
) -> Option<ViewportOutputConfig> {
    effective_tty_viewports_for_outputs(tuning, outputs)
        .into_iter()
        .find(|viewport| viewport.connector == output_name)
}

fn canonical_tty_main_output_name(
    outputs: &[TtyDrmOutput],
    tuning: &RuntimeTuning,
) -> Option<String> {
    let effective_viewports = effective_tty_viewports_for_outputs(tuning, outputs);
    outputs
        .iter()
        .min_by(|a, b| {
            let a_viewport = effective_viewports
                .iter()
                .find(|viewport| viewport.connector == a.connector_name);
            let b_viewport = effective_viewports
                .iter()
                .find(|viewport| viewport.connector == b.connector_name);

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
    let effective_viewports = effective_tty_viewports_for_outputs(tuning, outputs);
    let mut ordered: Vec<(String, i32, i32, bool)> = outputs
        .iter()
        .map(|output| {
            let (offset_x, offset_y) = effective_viewports
                .iter()
                .find(|viewport| viewport.connector == output.connector_name)
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
    let active_viewports = effective_tty_viewports_for_outputs(tuning, outputs);

    if active_viewports.is_empty() {
        return (
            tuning.viewport_size.x.max(1.0).round() as i32,
            tuning.viewport_size.y.max(1.0).round() as i32,
        );
    }

    let min_x = active_viewports.iter().map(|v| v.offset_x).min().unwrap();
    let max_x = active_viewports
        .iter()
        .map(|v| v.offset_x + v.width as i32)
        .max()
        .unwrap();
    let min_y = active_viewports.iter().map(|v| v.offset_y).min().unwrap();
    let max_y = active_viewports
        .iter()
        .map(|v| v.offset_y + v.height as i32)
        .max()
        .unwrap();

    (max_x - min_x, max_y - min_y)
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

    *scanout_signature.borrow_mut() = current_tty_output_signature(&rebuilt);
    st.configure_dmabuf_output_feedbacks(build_tty_dmabuf_output_feedbacks(
        rebuilt.as_slice(),
        gpu_manager,
        primary_render_node,
    ));
    sync_tty_dpms_state(&rebuilt, dpms_enabled);
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
            st.advertise_output(name.as_str(), (*mode).into());
        }
    }

    publish_tty_outputs_snapshot_for_devices(
        &drm_devices.borrow(),
        &active_modes.borrow(),
        &dpms_enabled.borrow(),
        &st.runtime.tuning,
        st,
    );

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
        .and_then(|name| effective_tty_viewport_for_output(tuning, outputs, name))
        .map(|viewport| {
            (
                viewport.width as i32,
                viewport.height as i32,
                viewport.offset_x,
                viewport.offset_y,
            )
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
                effective_tty_viewport_for_output(tuning, outputs, name.as_str()).map(|viewport| {
                    (
                        viewport.width as i32,
                        viewport.height as i32,
                        viewport.offset_x,
                        viewport.offset_y,
                    )
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

            let mut display: Display<Halley> = Display::new()?;
            let dh = display.handle();

            crate::bootstrap::ensure_default_user_config(Some(&bootstrap_tty_viewports(
                drm_probe.outputs.as_slice(),
            )));
            let config_path = Rc::new(RuntimeTuning::config_path());
            let aperture_config_path = Rc::new(crate::aperture::default_aperture_config_path());
            let tuning = RuntimeTuning::load_from_path(config_path.as_str());
            let aperture_config =
                crate::aperture::load_aperture_config_from_path(aperture_config_path.as_path());
            tuning.apply_process_env();
            if !Path::new(config_path.as_str()).exists() {
                warn!(
                    "config file not found at {}; using built-in defaults",
                    config_path.as_str()
                );
            }
            info!("config path: {}", config_path.as_str());
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
                let config_watch_target = PathBuf::from(config_path.as_str());
                let aperture_watch_target = aperture_config_path.as_ref().clone();
                let mut watcher: RecommendedWatcher = notify::recommended_watcher(
                    move |result: Result<notify::Event, notify::Error>| {
                        if let Ok(event) = result {
                            let touches_config = if event.paths.is_empty() {
                                true
                            } else {
                                event.paths.iter().any(|path| {
                                    crate::aperture::aperture_config_matches_event_path(
                                        path,
                                        config_watch_target.as_path(),
                                        aperture_watch_target.as_path(),
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
                for watch_root in crate::aperture::config_watch_roots(
                    Path::new(config_path.as_str()),
                    aperture_config_path.as_path(),
                ) {
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
            sync_portal_activation_environment(sock_name.as_str());
            let xwayland = Rc::new(RefCell::new(ensure_xwayland_satellite(sock_name.as_str())?));
            let (xwayland_request_tx, xwayland_request_rx) = mpsc::channel::<()>();
            register_xwayland_request_channel(xwayland_request_tx);
            let xwayland_request_rx = Rc::new(RefCell::new(xwayland_request_rx));
            let xwayland_for_timer = xwayland.clone();
            let xwayland_request_for_timer = xwayland_request_rx.clone();
            let libinput_backend = libinput_backend;

            let mut ev: EventLoop<Halley> = EventLoop::try_new()?;
            let _signal = ev.get_signal();
            let mut state = Halley::new(&dh, ev.handle(), tuning.clone());
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
            let scanout_signature_for_timer = scanout_signature.clone();
            let pending_scanout_probe_at = Rc::new(RefCell::new(Some(
                Instant::now() + Duration::from_millis(OUTPUT_RESCAN_POLL_MS),
            )));
            let pending_scanout_probe_at_for_timer = pending_scanout_probe_at.clone();
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
                let vblank_throttles_for_notifier = vblank_throttles_for_notifier.clone();
                let event_loop_handle_for_vblank = event_loop_handle_for_vblank.clone();
                let redraw_ping_for_vblank = redraw_ping_for_vblank.clone();
                let first_vblank_logged_for_notifier = first_vblank_logged_for_notifier.clone();
                let vblank_mismatch_state_for_notifier = vblank_mismatch_state_for_notifier.clone();
                let frame_stats_for_notifier = frame_stats.clone();
                ev.handle().insert_source(
                    notifier,
                    move |event, metadata, _st| match event {
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
                                    move |_state| {
                                        if let Err(err) = compositor.borrow_mut().frame_submitted() {
                                            warn!("failed to mark drm frame submitted after throttle: {}", err);
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
                            if let Err(err) = compositor.borrow_mut().frame_submitted() {
                                warn!(
                                    "failed to mark drm frame submitted for {}: {}",
                                    output_name, err
                                );
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
                    if !(animation_redraw_active && !animation_output_ready) {
                        advance_tty_redraw_frame(st, &pointer_state_for_redraw, now, false);
                    }
                    let eligible_outputs = animation_redraw_active.then(|| {
                        tty_animation_redraw_outputs(
                            st,
                            &outputs_for_redraw,
                            &pointer_state_for_redraw,
                            now,
                        )
                    });
                    let ps = pointer_state_for_redraw.borrow();
                    let resize_preview = ps.resize;
                    drop(ps);
                    queue_ready_tty_outputs(
                        &outputs_for_redraw,
                        &dpms_enabled_for_redraw,
                        &output_frame_pending_for_redraw,
                        &output_frame_pending_since,
                        &output_animation_redraw_active_for_redraw,
                        &composed_frame_cache_for_redraw,
                        &pointer_state_for_redraw,
                        &gpu_manager_for_redraw,
                        primary_render_node_for_redraw,
                        &first_frame_queued_for_redraw,
                        frame_stats_for_redraw.as_ref(),
                        st,
                        now,
                        resize_preview,
                        eligible_outputs.as_ref(),
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
                        let (ws_w, ws_h) = backend_handle.window_size_i32();
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

                drain_ipc_commands(|request| match request {
                    halley_ipc::Request::Compositor(halley_ipc::CompositorRequest::Quit) => {
                        info!("ipc: quit requested");
                        exit_confirm_controller(&mut *st).show();
                        halley_ipc::Response::Ok
                    }
                    halley_ipc::Request::Compositor(halley_ipc::CompositorRequest::Reload) => {
                        let _ = crate::aperture::reload_aperture_config(
                            st,
                            aperture_config_path_for_timer.as_path(),
                            "ipc",
                        );
                        if let Some(next) =
                            RuntimeTuning::try_load_from_path(config_path_for_timer.as_str())
                        {
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
                                    &scanout_signature_for_timer,
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
                        debug!("resolved keybinds: {}", st.runtime.tuning.keybinds_resolved_summary());
                        debug!("resolved zoom: {}", st.runtime.tuning.zoom_resolved_summary());
                        halley_ipc::Response::Reloaded
                    }
                    halley_ipc::Request::Compositor(halley_ipc::CompositorRequest::Dpms {
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
                            halley_ipc::Response::Ok
                        } else {
                            halley_ipc::Response::Error(halley_ipc::IpcError::NotFound(
                                "dpms request made no change".into(),
                            ))
                        }
                    }
                    request => crate::ipc::handle_request(st, request),
                });

                {
                    let rx = xwayland_request_for_timer.borrow_mut();
                    while rx.try_recv().is_ok() {
                        xwayland_for_timer.borrow_mut().request_start();
                    }
                }
                xwayland_for_timer.borrow_mut().tick();
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
                    if let Some(next) =
                        RuntimeTuning::try_load_from_path(config_path_for_timer.as_str())
                    {
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
                                    &scanout_signature_for_timer,
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
                if pending_scanout_probe_at_for_timer
                    .borrow()
                    .is_some_and(|deadline| now >= deadline)
                {
                    *pending_scanout_probe_at_for_timer.borrow_mut() =
                        Some(now + Duration::from_millis(OUTPUT_RESCAN_POLL_MS));
                    let maybe_signature: Result<Vec<String>, Box<dyn Error>> = {
                        let mut signature = Vec::new();
                        for device in drm_devices_for_timer.borrow_mut().iter_mut() {
                            let mut dev = device.dev.borrow_mut();
                            match selected_tty_scanout_signature(&mut dev, &st.runtime.tuning) {
                                Ok(mut device_signature) => signature.append(&mut device_signature),
                                Err(err) => debug!(
                                    "tty drm topology probe skipped for {}: {}",
                                    device.card_path.display(),
                                    err
                                ),
                            }
                        }
                        if signature.is_empty() {
                            Err(io::Error::other("no usable tty scanouts across DRM devices").into())
                        } else {
                            signature.sort();
                            Ok(signature)
                        }
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
                    // On the first tick after DPMS wake, re-configure layer shell
                    // surfaces. Frame callbacks are sent only after a scanout frame queues.
                    if !dpms_just_woke_outputs_for_timer.borrow().is_empty() {
                        st.input.interaction_state.dpms_just_woke = false;
                        dpms_just_woke_outputs_for_timer.borrow_mut().clear();
                        crate::compositor::monitor::layer_shell::configure_layer_shell_surfaces(
                            st,
                            (1, 1).into(),
                        );
                    }

                    if !due_outputs.is_empty() {
                        // Keep the redraw-ping path biased toward animation-active outputs, but
                        // let the timer continue servicing every due output. Otherwise local
                        // zoom/pan on one monitor can starve unrelated outputs that still need
                        // regular scanout, such as fullscreen video playback.
                        advance_tty_redraw_frame(st, &pointer_state_for_timer, now, false);
                        queue_ready_tty_outputs(
                            &outputs_for_timer,
                            &dpms_enabled_for_timer,
                            &output_frame_pending,
                            &output_frame_pending_since_for_timer,
                            &output_animation_redraw_active,
                            &composed_frame_cache_for_timer,
                            &pointer_state_for_timer,
                            &gpu_manager_for_timer,
                            primary_render_node_for_timer,
                            &first_frame_queued_for_timer,
                            frame_stats_for_timer.as_ref(),
                            st,
                            now,
                            resize_preview,
                            Some(&due_outputs),
                            "timer",
                        );
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
