use std::cmp::Ordering;

use halley_core::field::{NodeId, Vec2};
use halley_core::viewport::Viewport;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Instant;

use eventline::debug;
use smithay::{
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::wayland_server::{Resource, backend::ObjectId, protocol::wl_surface::WlSurface},
    utils::{Logical, Physical, Raw, Size, Transform},
    wayland::{
        compositor::{get_parent, send_surface_state, with_states},
        fractional_scale::with_fractional_scale,
    },
};

use crate::compositor::root::Halley;
use crate::compositor::spawn::state::MonitorSpawnState;
use halley_config::ViewportOutputConfig;

#[derive(Clone, Debug)]
pub(crate) struct MonitorSpace {
    pub offset_x: i32,
    pub offset_y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub viewport: Viewport,
    pub usable_viewport: Viewport,
    pub zoom_ref_size: Vec2,
    pub zoom_log_vel: f32,
    pub pan_vel: Vec2,
    pub camera_target_center: Vec2,
    pub camera_target_view_size: Vec2,
}

const MIN_SCALE_STEP: i32 = 4;
const MAX_SCALE_STEP: i32 = 16;
const SCALE_STEP_DENOM: f64 = 4.0;
const MIN_LOGICAL_AREA: i32 = 800 * 480;
const MOBILE_TARGET_DPI: f64 = 135.0;
const LARGE_TARGET_DPI: f64 = 110.0;
const LARGE_MIN_SIZE_INCHES: f64 = 20.0;

pub(crate) struct MonitorState {
    pub(crate) outputs: HashMap<String, Output>,
    pub(crate) current_monitor: String,
    pub(crate) interaction_monitor: String,
    pub(crate) focused_monitor: String,
    pub(crate) monitors: HashMap<String, MonitorSpace>,
    pub(crate) node_monitor: HashMap<NodeId, String>,
    pub(crate) layer_surface_monitor: HashMap<ObjectId, String>,
    pub(crate) layer_surface_namespace: HashMap<ObjectId, String>,
    pub(crate) layer_surface_order: Vec<ObjectId>,
    pub(crate) aperture_layer_monitors: HashSet<String>,
    pub(crate) aperture_layer_heights: HashMap<String, i32>,
    /// Monitors whose aperture-driven `usable_viewport` change has been deferred
    /// because a cluster tile transition is animating there; flushed once the
    /// slide settles so the work area never moves mid-slide. See
    /// `refresh_monitor_usable_viewports`.
    pub(crate) pending_workarea_refresh: HashSet<String>,
    pub(crate) layer_surface_committed: HashSet<ObjectId>,
    pub(crate) layer_surface_last_configured_size: HashMap<ObjectId, Size<i32, Logical>>,
    pub(crate) layer_keyboard_focus: Option<ObjectId>,
}

fn preferred_monitor_name(monitors: &HashMap<String, MonitorSpace>) -> Option<String> {
    monitors
        .iter()
        .min_by(|a, b| {
            let (_, am) = a;
            let (_, bm) = b;
            am.offset_x
                .cmp(&bm.offset_x)
                .then(am.offset_y.cmp(&bm.offset_y))
                .then(a.0.cmp(b.0))
        })
        .map(|(name, _)| name.clone())
}

pub fn view_center_for_monitor(st: &Halley, monitor: &str) -> Vec2 {
    usable_viewport_for_monitor(st, monitor).center
}

pub fn usable_viewport_for_monitor(st: &Halley, monitor: &str) -> Viewport {
    let is_cluster = st.cluster_mode_active_for_monitor(monitor);

    if st.model.monitor_state.current_monitor == monitor {
        if !is_cluster {
            return st.model.viewport;
        }
        st.model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| space.usable_viewport)
            .unwrap_or(st.model.viewport)
    } else {
        st.model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| {
                if is_cluster {
                    space.usable_viewport
                } else {
                    space.viewport
                }
            })
            .unwrap_or(st.model.viewport)
    }
}

pub(crate) fn load_monitor_state(st: &mut Halley, name: &str) -> bool {
    let Some(space) = st.model.monitor_state.monitors.get(name).cloned() else {
        return false;
    };
    st.model.monitor_state.current_monitor = name.to_string();
    st.model.viewport = space.viewport;
    st.model.zoom_ref_size = space.zoom_ref_size;
    st.model.zoom_log_vel = space.zoom_log_vel;
    st.model.pan_vel = space.pan_vel;
    st.model.camera_target_center = space.camera_target_center;
    st.model.camera_target_view_size = space.camera_target_view_size;
    true
}

pub(crate) fn sync_current_monitor_state(st: &mut Halley) {
    if let Some(space) = st
        .model
        .monitor_state
        .monitors
        .get_mut(&st.model.monitor_state.current_monitor)
    {
        space.viewport = st.model.viewport;
        space.zoom_ref_size = st.model.zoom_ref_size;
        space.zoom_log_vel = st.model.zoom_log_vel;
        space.pan_vel = st.model.pan_vel;
        space.camera_target_center = st.model.camera_target_center;
        space.camera_target_view_size = st.model.camera_target_view_size;
    }
}

pub(crate) fn activate_monitor(st: &mut Halley, name: &str) -> bool {
    if st.model.monitor_state.current_monitor == name {
        return st.model.monitor_state.monitors.contains_key(name);
    }
    sync_current_monitor_state(st);
    load_monitor_state(st, name)
}

thread_local! {
    static LAST_XWAYLAND_PRIMARY: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Point the Xwayland (xwayland-satellite) RandR primary output at `monitor`.
///
/// XWayland clients such as SDL/Unity games read the X RandR *primary* at startup to
/// pick their resolution and "current" monitor. xwayland-satellite only updates the
/// primary on X-window focus and never resets a stale value, so a game launched on one
/// monitor can read the leftover primary of another (renders at the wrong monitor's
/// resolution and thinks it is on that monitor). The compositor therefore drives the
/// primary to follow the active monitor (the one the cursor — and hence a freshly
/// spawned window — is on), so it is already correct when the game reads it.
///
/// Runs `xrandr` against the satellite `DISPLAY` (X output names equal connector names).
/// Debounced (called on every pointer motion), and a no-op when Xwayland is disabled
/// (`DISPLAY` unset) or `monitor` is not a real output.
pub(crate) fn sync_xwayland_primary(st: &mut Halley, monitor: &str) {
    // Cheapest check first: this runs on every pointer-motion event.
    let already = LAST_XWAYLAND_PRIMARY.with(|last| last.borrow().as_deref() == Some(monitor));
    if already {
        return;
    }
    let Ok(display) = std::env::var("DISPLAY") else {
        return;
    };
    if display.trim().is_empty() || !st.model.monitor_state.outputs.contains_key(monitor) {
        return;
    }
    LAST_XWAYLAND_PRIMARY.with(|last| *last.borrow_mut() = Some(monitor.to_string()));

    let mut command = std::process::Command::new("xrandr");
    command
        .args(["--output", monitor, "--primary"])
        .env("DISPLAY", display)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match command.spawn() {
        Ok(child) => st.runtime.spawned_children.push(child),
        Err(err) => eventline::warn!("failed to set Xwayland primary to {}: {}", monitor, err),
    }
}

pub(crate) fn begin_temporary_render_monitor(st: &mut Halley, name: &str) -> Option<String> {
    let previous = st.model.monitor_state.current_monitor.clone();
    if previous != name && activate_monitor(st, name) {
        Some(previous)
    } else {
        None
    }
}

pub(crate) fn end_temporary_render_monitor(st: &mut Halley, previous: Option<String>) {
    if let Some(previous) = previous {
        let _ = activate_monitor(st, previous.as_str());
    }
}

pub(crate) fn interaction_monitor(st: &Halley) -> &str {
    if st
        .model
        .monitor_state
        .monitors
        .contains_key(&st.model.monitor_state.interaction_monitor)
    {
        st.model.monitor_state.interaction_monitor.as_str()
    } else {
        st.model.monitor_state.current_monitor.as_str()
    }
}

pub(crate) fn focused_monitor(st: &Halley) -> &str {
    if st
        .model
        .monitor_state
        .monitors
        .contains_key(&st.model.monitor_state.focused_monitor)
    {
        st.model.monitor_state.focused_monitor.as_str()
    } else {
        interaction_monitor(st)
    }
}

pub(crate) fn monitor_for_node_or_current(st: &Halley, node_id: NodeId) -> String {
    st.model
        .monitor_state
        .node_monitor
        .get(&node_id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone())
}

pub(crate) fn monitor_for_surface_or_current(st: &Halley, surface: &WlSurface) -> String {
    st.model
        .surface_to_node
        .get(&surface.id())
        .copied()
        .map(|node_id| monitor_for_node_or_current(st, node_id))
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone())
}

pub(crate) fn monitor_for_constrained_surface_or_current(
    st: &Halley,
    surface: &WlSurface,
) -> String {
    monitor_for_surface(st, surface)
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone())
}

pub(crate) fn monitor_for_screen_or_current(st: &Halley, sx: f32, sy: f32) -> String {
    monitor_for_screen(st, sx, sy).unwrap_or_else(|| st.model.monitor_state.current_monitor.clone())
}

pub(crate) fn monitor_for_screen_or_interaction(st: &Halley, sx: f32, sy: f32) -> String {
    monitor_for_screen(st, sx, sy).unwrap_or_else(|| interaction_monitor(st).to_string())
}

pub(crate) fn set_interaction_monitor(st: &mut Halley, name: &str) {
    if st.model.monitor_state.monitors.contains_key(name) {
        st.model.monitor_state.interaction_monitor = name.to_string();
    }
}

pub(crate) fn set_focused_monitor(st: &mut Halley, name: &str) {
    if st.model.monitor_state.monitors.contains_key(name) {
        st.model.monitor_state.focused_monitor = name.to_string();
        sync_xwayland_primary(st, name);
    }
}

pub(crate) fn reconfigure_active_tty_monitors(
    st: &mut Halley,
    active_viewports: &[ViewportOutputConfig],
) {
    sync_current_monitor_state(st);

    let previous = st.model.monitor_state.monitors.clone();
    let mut monitors = HashMap::new();

    for viewport in active_viewports.iter().filter(|viewport| viewport.enabled) {
        let width = viewport.width.max(1) as i32;
        let height = viewport.height.max(1) as i32;
        let center = Vec2 {
            x: viewport.offset_x as f32 + width as f32 * 0.5,
            y: viewport.offset_y as f32 + height as f32 * 0.5,
        };
        let default_view = Viewport::new(
            center,
            Vec2 {
                x: width as f32,
                y: height as f32,
            },
        );

        let restored = previous.get(&viewport.connector);
        monitors.insert(
            viewport.connector.clone(),
            MonitorSpace {
                offset_x: viewport.offset_x,
                offset_y: viewport.offset_y,
                width,
                height,
                scale: restored.map(|m| m.scale).unwrap_or(1.0),
                viewport: restored.map(|m| m.viewport).unwrap_or(default_view),
                usable_viewport: restored.map(|m| m.usable_viewport).unwrap_or(default_view),
                zoom_ref_size: restored
                    .map(|m| m.zoom_ref_size)
                    .unwrap_or(default_view.size),
                zoom_log_vel: 0.0,
                pan_vel: Vec2 { x: 0.0, y: 0.0 },
                camera_target_center: restored
                    .map(|m| m.camera_target_center)
                    .unwrap_or(default_view.center),
                camera_target_view_size: restored
                    .map(|m| m.camera_target_view_size)
                    .unwrap_or(default_view.size),
            },
        );
    }

    if monitors.is_empty() {
        let view = st.runtime.tuning.viewport();
        monitors.insert(
            "default".to_string(),
            MonitorSpace {
                offset_x: 0,
                offset_y: 0,
                width: st.runtime.tuning.viewport_size.x.max(1.0).round() as i32,
                height: st.runtime.tuning.viewport_size.y.max(1.0).round() as i32,
                scale: 1.0,
                viewport: view,
                usable_viewport: view,
                zoom_ref_size: st.runtime.tuning.viewport_size,
                zoom_log_vel: 0.0,
                pan_vel: Vec2 { x: 0.0, y: 0.0 },
                camera_target_center: st.runtime.tuning.viewport_center,
                camera_target_view_size: st.runtime.tuning.viewport_size,
            },
        );
    }

    st.model.monitor_state.monitors = monitors;
    crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(st);
    st.model.spawn_state.per_monitor = st
        .model
        .monitor_state
        .monitors
        .iter()
        .map(|(name, monitor)| {
            let existing = st.model.spawn_state.per_monitor.get(name).cloned();
            (
                name.clone(),
                existing.unwrap_or_else(|| MonitorSpawnState::new(monitor.viewport.center)),
            )
        })
        .collect();

    if !st
        .model
        .monitor_state
        .monitors
        .contains_key(&st.model.monitor_state.current_monitor)
    {
        st.model.monitor_state.current_monitor =
            preferred_monitor_name(&st.model.monitor_state.monitors)
                .unwrap_or_else(|| "default".to_string());
    }

    if !st
        .model
        .monitor_state
        .monitors
        .contains_key(&st.model.monitor_state.interaction_monitor)
    {
        st.model.monitor_state.interaction_monitor = st.model.monitor_state.current_monitor.clone();
    }
    if !st
        .model
        .monitor_state
        .monitors
        .contains_key(&st.model.monitor_state.focused_monitor)
    {
        st.model.monitor_state.focused_monitor = st.model.monitor_state.interaction_monitor.clone();
    }

    let current = st.model.monitor_state.current_monitor.clone();
    let _ = load_monitor_state(st, current.as_str());
}

pub(crate) fn monitor_for_screen_clamped(
    st: &Halley,
    sx: f32,
    sy: f32,
) -> Option<(String, f32, f32)> {
    let mut best: Option<(&String, f64, f32, f32, i32, i32)> = None;
    for (name, monitor) in &st.model.monitor_state.monitors {
        let min_x = monitor.offset_x as f32;
        let max_x = (monitor.offset_x + monitor.width - 1) as f32;
        let min_y = monitor.offset_y as f32;
        let max_y = (monitor.offset_y + monitor.height - 1) as f32;
        let clamped_sx = sx.clamp(min_x, max_x);
        let clamped_sy = sy.clamp(min_y, max_y);
        let dx = (sx - clamped_sx) as f64;
        let dy = (sy - clamped_sy) as f64;
        let distance = dx * dx + dy * dy;
        let better = best.as_ref().is_none_or(
            |(best_name, best_distance, _, _, best_offset_x, best_offset_y)| match distance
                .total_cmp(best_distance)
            {
                Ordering::Less => true,
                Ordering::Greater => false,
                Ordering::Equal => {
                    (monitor.offset_x, monitor.offset_y, name.as_str())
                        < (*best_offset_x, *best_offset_y, best_name.as_str())
                }
            },
        );
        if better {
            best = Some((
                name,
                distance,
                clamped_sx,
                clamped_sy,
                monitor.offset_x,
                monitor.offset_y,
            ));
        }
    }
    best.map(|(name, _, clamped_sx, clamped_sy, _, _)| (name.clone(), clamped_sx, clamped_sy))
}

pub(crate) fn monitor_for_screen(st: &Halley, sx: f32, sy: f32) -> Option<String> {
    monitor_for_screen_clamped(st, sx, sy).map(|(name, _, _)| name)
}

pub(crate) fn local_screen_in_monitor(
    st: &Halley,
    name: &str,
    sx: f32,
    sy: f32,
) -> (i32, i32, f32, f32) {
    if let Some(monitor) = st.model.monitor_state.monitors.get(name) {
        (
            monitor.width,
            monitor.height,
            sx - monitor.offset_x as f32,
            sy - monitor.offset_y as f32,
        )
    } else {
        let w = st.runtime.tuning.viewport_size.x.max(1.0).round() as i32;
        let h = st.runtime.tuning.viewport_size.y.max(1.0).round() as i32;
        (w, h, sx, sy)
    }
}

pub(crate) fn node_visible_on_current_monitor(st: &Halley, id: NodeId) -> bool {
    st.model
        .monitor_state
        .node_monitor
        .get(&id)
        .is_none_or(|monitor| monitor == &st.model.monitor_state.current_monitor)
}

pub(crate) fn node_assigned_to_current_monitor(st: &Halley, id: NodeId) -> bool {
    st.model
        .monitor_state
        .node_monitor
        .get(&id)
        .is_some_and(|monitor| monitor == &st.model.monitor_state.current_monitor)
}

#[allow(dead_code)]
pub(crate) fn assign_node_to_current_monitor(st: &mut Halley, id: NodeId) {
    let monitor = st.model.monitor_state.current_monitor.clone();
    assign_node_to_monitor(st, id, monitor.as_str());
}

pub(crate) fn assign_node_to_monitor(st: &mut Halley, id: NodeId, monitor: &str) {
    let _ = st.spawn_monitor_state_mut(monitor);
    st.model
        .monitor_state
        .node_monitor
        .insert(id, monitor.to_string());

    // Update surface output assignments immediately so Xwayland and Wayland clients
    // know which output the window is on before the next commit.
    if let Some(surface) = crate::compositor::focus::system::wl_surface_for_node(st, id) {
        assign_surface_to_monitor(st, &surface, monitor);
    }
}

pub(crate) fn assign_surface_to_monitor(st: &Halley, surface: &WlSurface, monitor: &str) {
    for (name, output) in &st.model.monitor_state.outputs {
        if name == monitor {
            output.enter(surface);
        } else {
            output.leave(surface);
        }
    }
    set_surface_preferred_scale_for_monitor(st, surface, monitor);
}

pub(crate) fn assign_layer_surface_to_monitor(
    st: &mut Halley,
    surface: &WlSurface,
    monitor: String,
) {
    st.model
        .monitor_state
        .layer_surface_monitor
        .insert(surface.id(), monitor.clone());
    set_surface_preferred_scale_for_monitor(st, surface, monitor.as_str());
}

pub(crate) fn output_transform_for(st: &Halley, name: &str) -> Transform {
    let degrees = st
        .runtime
        .tuning
        .tty_viewports
        .iter()
        .find(|viewport| viewport.connector == name)
        .map(|viewport| viewport.transform_degrees)
        .unwrap_or(0);
    match degrees {
        90 => Transform::_90,
        180 => Transform::_180,
        270 => Transform::_270,
        _ => Transform::Normal,
    }
}

pub(crate) fn advertise_output(st: &mut Halley, name: &str, mode: OutputMode) {
    advertise_output_with_physical_size(st, name, mode, None)
}

pub(crate) fn advertise_output_with_physical_size(
    st: &mut Halley,
    name: &str,
    mode: OutputMode,
    physical_size_mm: Option<(u32, u32)>,
) {
    let transform = output_transform_for(st, name);
    let location = st
        .model
        .monitor_state
        .monitors
        .get(name)
        .map(|monitor| (monitor.offset_x, monitor.offset_y).into())
        .unwrap_or_else(|| (0, 0).into());
    let scale = guess_output_scale(physical_size_mm, mode.size);
    if let Some(monitor) = st.model.monitor_state.monitors.get_mut(name) {
        monitor.scale = scale;
    }
    let physical_size: Size<i32, Raw> = physical_size_mm
        .map(|(w, h)| Size::from((w as i32, h as i32)))
        .unwrap_or_else(|| Size::from((0, 0)));
    let output = if let Some(output) = st.model.monitor_state.outputs.get(name) {
        output.clone()
    } else {
        let output = Output::new(
            name.to_string(),
            PhysicalProperties {
                size: physical_size,
                subpixel: Subpixel::Unknown,
                make: "halley".to_string(),
                model: name.to_string(),
                serial_number: "unknown".to_string(),
            },
        );
        output.add_mode(mode);
        output.set_preferred(mode);
        output.change_current_state(
            Some(mode),
            Some(transform),
            Some(Scale::Fractional(scale)),
            Some(location),
        );
        let _ = output.create_global::<Halley>(&st.platform.display_handle);
        st.model
            .monitor_state
            .outputs
            .insert(name.to_string(), output.clone());
        output
    };
    output.add_mode(mode);
    output.set_preferred(mode);
    output.change_current_state(
        Some(mode),
        Some(transform),
        Some(Scale::Fractional(scale)),
        Some(location),
    );
    let logical_w = (mode.size.w as f64 / scale).round().max(1.0) as i32;
    let logical_h = (mode.size.h as f64 / scale).round().max(1.0) as i32;
    let logical_size: Size<i32, Logical> =
        transform.transform_size(Size::from((logical_w, logical_h)));
    debug!(
        "advertised output {} mode={}x{} scale={:.3} logical={}x{} transform={:?} location=({}, {}) physical_mm={:?}",
        name,
        mode.size.w,
        mode.size.h,
        scale,
        logical_size.w,
        logical_size.h,
        transform,
        location.x,
        location.y,
        physical_size_mm,
    );
}

pub(crate) fn refresh_surface_preferred_scale(st: &Halley, surface: &WlSurface) {
    let monitor = monitor_for_surface(st, surface)
        .unwrap_or_else(|| st.model.monitor_state.focused_monitor.clone());
    set_surface_preferred_scale_for_monitor(st, surface, monitor.as_str());
}

pub(crate) fn set_surface_preferred_scale_for_monitor(
    st: &Halley,
    surface: &WlSurface,
    monitor: &str,
) {
    let scale = st
        .model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|monitor| monitor.scale)
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .unwrap_or(1.0);
    let transform = output_transform_for(st, monitor);
    with_states(surface, |states| {
        let output_scale = Scale::Fractional(scale);
        send_surface_state(surface, states, output_scale.integer_scale(), transform);
        with_fractional_scale(states, |fractional| {
            fractional.set_preferred_scale(scale);
        });
    });
}

fn monitor_for_surface(st: &Halley, surface: &WlSurface) -> Option<String> {
    let mut current = surface.clone();
    loop {
        let key = current.id();
        if let Some(node_id) = st.model.surface_to_node.get(&key)
            && let Some(monitor) = st.model.monitor_state.node_monitor.get(node_id)
        {
            return Some(monitor.clone());
        }
        if let Some(monitor) = st.model.monitor_state.layer_surface_monitor.get(&key) {
            return Some(monitor.clone());
        }
        if let Some(parent) = get_parent(&current) {
            current = parent;
        } else {
            return None;
        }
    }
}

fn guess_output_scale(
    physical_size_mm: Option<(u32, u32)>,
    resolution: Size<i32, Physical>,
) -> f64 {
    let Some((width_mm, height_mm)) = physical_size_mm else {
        return 1.0;
    };
    if width_mm == 0 || height_mm == 0 || resolution.w <= 0 || resolution.h <= 0 {
        return 1.0;
    }

    let width_mm = width_mm as f64;
    let height_mm = height_mm as f64;
    let diagonal_inches = (width_mm * width_mm + height_mm * height_mm).sqrt() / 25.4;
    if diagonal_inches <= 0.0 {
        return 1.0;
    }
    let target_dpi = if diagonal_inches < LARGE_MIN_SIZE_INCHES {
        MOBILE_TARGET_DPI
    } else {
        LARGE_TARGET_DPI
    };
    let physical_dpi = ((resolution.w * resolution.w + resolution.h * resolution.h) as f64).sqrt()
        / diagonal_inches;
    let ideal = physical_dpi / target_dpi;

    (MIN_SCALE_STEP..=MAX_SCALE_STEP)
        .map(|step| step as f64 / SCALE_STEP_DENOM)
        .filter(|scale| scale_is_valid_for_resolution(resolution, *scale))
        .min_by(|a, b| {
            (a - ideal)
                .abs()
                .partial_cmp(&(b - ideal).abs())
                .unwrap_or(Ordering::Equal)
        })
        .map(closest_fractional_scale)
        .unwrap_or(1.0)
}

fn scale_is_valid_for_resolution(resolution: Size<i32, Physical>, scale: f64) -> bool {
    let logical_w = (resolution.w as f64 / scale).round() as i32;
    let logical_h = (resolution.h as f64 / scale).round() as i32;
    logical_w * logical_h >= MIN_LOGICAL_AREA
}

fn closest_fractional_scale(scale: f64) -> f64 {
    const FRACTIONAL_SCALE_DENOM: f64 = 120.0;
    (scale * FRACTIONAL_SCALE_DENOM).round() / FRACTIONAL_SCALE_DENOM
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        tuning
    }

    #[test]
    fn reconfigure_active_monitors_preserves_focused_monitor_when_still_present() {
        let tuning = two_monitor_tuning();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        state.set_interaction_monitor("left");
        state.set_focused_monitor("right");
        let active = state.runtime.tuning.tty_viewports.clone();
        state.reconfigure_active_tty_monitors(&active);

        assert_eq!(state.focused_monitor(), "right");
        assert_eq!(state.interaction_monitor(), "left");
    }

    #[test]
    fn render_assignment_requires_explicit_current_monitor_owner() {
        let tuning = two_monitor_tuning();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let node_id = state.model.field.spawn_surface(
            "straddling",
            Vec2 { x: 790.0, y: 300.0 },
            Vec2 { x: 240.0, y: 160.0 },
        );

        let _ = state.activate_monitor("left");
        assert!(!node_assigned_to_current_monitor(&state, node_id));
        state.assign_node_to_monitor(node_id, "left");
        assert!(node_assigned_to_current_monitor(&state, node_id));

        let _ = state.activate_monitor("right");
        assert!(!node_assigned_to_current_monitor(&state, node_id));
    }

    #[test]
    fn reconfigure_active_monitors_falls_back_when_focused_monitor_disappears() {
        let tuning = two_monitor_tuning();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        state.set_interaction_monitor("left");
        state.set_focused_monitor("right");
        let active: Vec<_> = state
            .runtime
            .tuning
            .tty_viewports
            .iter()
            .filter(|viewport| viewport.connector == "left")
            .cloned()
            .collect();
        state.reconfigure_active_tty_monitors(&active);

        assert_eq!(state.focused_monitor(), "left");
        assert_eq!(state.interaction_monitor(), "left");
    }

    #[test]
    fn current_monitor_cluster_usable_viewport_returns_stored_usable_rect() {
        let tuning = two_monitor_tuning();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        state.model.monitor_state.current_monitor = "left".to_string();
        state.model.viewport =
            Viewport::new(Vec2 { x: 400.0, y: 300.0 }, Vec2 { x: 800.0, y: 600.0 });
        state
            .model
            .cluster_state
            .cluster_mode_selected_nodes
            .insert("left".to_string(), std::collections::HashSet::new());
        state
            .model
            .monitor_state
            .monitors
            .get_mut("left")
            .expect("left")
            .usable_viewport =
            Viewport::new(Vec2 { x: 400.0, y: 320.0 }, Vec2 { x: 800.0, y: 560.0 });

        assert_eq!(
            state.usable_viewport_for_monitor("left"),
            Viewport::new(Vec2 { x: 400.0, y: 320.0 }, Vec2 { x: 800.0, y: 560.0 })
        );
    }

    #[test]
    fn reconfigure_active_monitors_uses_supplied_fallback_viewports() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());

        let fallback = vec![
            halley_config::ViewportOutputConfig {
                connector: "HDMI-A-1".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 1920,
                height: 1080,
                refresh_rate: Some(60.0),
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "DP-1".to_string(),
                enabled: true,
                offset_x: 1920,
                offset_y: 0,
                width: 2560,
                height: 1440,
                refresh_rate: Some(144.0),
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];

        state.reconfigure_active_tty_monitors(&fallback);

        assert_eq!(state.model.monitor_state.monitors.len(), 2);
        assert!(state.model.monitor_state.monitors.contains_key("HDMI-A-1"));
        assert!(state.model.monitor_state.monitors.contains_key("DP-1"));
        assert_eq!(state.model.monitor_state.current_monitor, "HDMI-A-1");
        assert_eq!(state.model.monitor_state.monitors["DP-1"].offset_x, 1920);
    }

    #[test]
    fn monitor_for_screen_clamped_snaps_gap_points_to_nearest_monitor_edge() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 2560,
                height: 1440,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 2560,
                offset_y: 0,
                width: 1920,
                height: 1200,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let state = Halley::new_for_test(&dh, tuning);

        let (monitor, sx, sy) =
            monitor_for_screen_clamped(&state, 3000.0, 1300.0).expect("clamped monitor");

        assert_eq!(monitor, "right");
        assert_eq!(sx, 3000.0);
        assert_eq!(sy, 1199.0);
    }

    #[test]
    fn monitor_for_screen_clamped_preserves_points_inside_a_monitor() {
        let tuning = two_monitor_tuning();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let state = Halley::new_for_test(&dh, tuning);

        let (monitor, sx, sy) =
            monitor_for_screen_clamped(&state, 1200.0, 200.0).expect("clamped monitor");

        assert_eq!(monitor, "right");
        assert_eq!(sx, 1200.0);
        assert_eq!(sy, 200.0);
    }

    #[test]
    fn guess_output_scale_uses_dpi_when_physical_size_is_known() {
        assert_eq!(
            guess_output_scale(Some((598, 336)), Size::from((3840, 2160))),
            1.5
        );
    }

    #[test]
    fn guess_output_scale_falls_back_without_physical_size() {
        assert_eq!(guess_output_scale(None, Size::from((3840, 2160))), 1.0);
    }
}
