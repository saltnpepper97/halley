mod config;
pub(crate) mod core;

use std::path::{Path, PathBuf};
use std::time::Instant;

use eventline::{debug, warn};
use halley_ipc::{ApertureMode as IpcApertureMode, ApertureOutputStatus, ApertureStatusResponse};

use crate::compositor::root::Halley;
use crate::text::ui_text_size_px_in;

use halley_core::field::{NodeId, NodeKind, NodeState};
use halley_core::viewport::Viewport;

use self::core::{ApertureConfig, ApertureMode, ApertureRuntime, ClockSnapshot, Rect, Size};

pub(crate) use config::{
    aperture_config_matches_event_path, config_watch_roots, default_aperture_config_path,
};

pub(crate) struct ApertureState {
    runtime: ApertureRuntime,
}

impl ApertureState {
    pub(crate) fn new(config: ApertureConfig, now: Instant) -> Self {
        let _ = now;
        Self {
            runtime: ApertureRuntime::new(config),
        }
    }

    pub(crate) fn apply_config(&mut self, config: ApertureConfig) {
        self.runtime.apply_config(config);
    }

    pub(crate) fn config(&self) -> &ApertureConfig {
        self.runtime.config()
    }

    pub(crate) fn snapshot_for_mode<F>(
        &self,
        mode: ApertureMode,
        output_rect: Rect,
        work_area_rect: Rect,
        scale: f64,
        measure_text: F,
    ) -> Option<ClockSnapshot>
    where
        F: FnMut(u32, &str) -> Size,
    {
        self.runtime
            .snapshot_for_mode(mode, output_rect, work_area_rect, scale, measure_text)
    }
}

pub(crate) fn try_load_aperture_config_from_path(path: &Path) -> Result<ApertureConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => ApertureConfig::parse_str(raw.as_str()).map_err(|err| err.to_string()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(ApertureConfig::default()),
        Err(err) => Err(format!("failed to read {}: {err}", path.display())),
    }
}

pub(crate) fn load_aperture_config_from_path(path: &Path) -> ApertureConfig {
    try_load_aperture_config_from_path(path).unwrap_or_default()
}

pub(crate) fn apply_reloaded_aperture_config(st: &mut Halley, config: ApertureConfig) {
    st.apply_aperture_config(config);
}

pub(crate) fn reload_aperture_config(st: &mut Halley, path: &Path, reason: &str) -> bool {
    match try_load_aperture_config_from_path(path) {
        Ok(config) => {
            apply_reloaded_aperture_config(st, config);
            debug!("{reason}: reloaded aperture config from {}", path.display());
            true
        }
        Err(err) => {
            warn!(
                "{reason}: aperture reload skipped for {} because {}",
                path.display(),
                err
            );
            false
        }
    }
}

pub(crate) fn aperture_status(st: &Halley) -> ApertureStatusResponse {
    let monitor = st.model.monitor_state.current_monitor.clone();
    let mode = derive_aperture_mode_for_monitor(st, monitor.as_str());
    let mut outputs: Vec<_> = st
        .model
        .monitor_state
        .monitors
        .keys()
        .map(|monitor| ApertureOutputStatus {
            output: monitor.clone(),
            mode: map_ipc_mode(derive_aperture_mode_for_monitor(st, monitor.as_str())),
        })
        .collect();
    outputs.sort_by(|a, b| a.output.cmp(&b.output));

    ApertureStatusResponse {
        output: Some(monitor),
        mode: map_ipc_mode(mode),
        outputs,
    }
}

fn derive_aperture_mode_for_monitor(st: &Halley, monitor: &str) -> ApertureMode {
    let usable =
        crate::compositor::monitor::layer_shell::layer_shell_usable_rect_for_monitor(st, monitor);
    let output_rect = Rect::new(
        0.0,
        0.0,
        usable.size.w.max(1) as f32,
        usable.size.h.max(1) as f32,
    );
    let work_area_rect = Rect::new(
        usable.loc.x as f32,
        usable.loc.y as f32,
        usable.size.w as f32,
        usable.size.h as f32,
    );
    derive_aperture_mode(st, monitor, output_rect, work_area_rect, 1.0)
}

fn map_ipc_mode(mode: ApertureMode) -> IpcApertureMode {
    match mode {
        ApertureMode::Normal => IpcApertureMode::Normal,
        ApertureMode::Collapsed => IpcApertureMode::Collapsed,
        ApertureMode::Hidden => IpcApertureMode::Hidden,
    }
}

fn derive_aperture_mode(
    st: &Halley,
    monitor: &str,
    output_rect: Rect,
    work_area_rect: Rect,
    scale: f64,
) -> ApertureMode {
    let render_state = &st.ui.render_state;
    let windows = active_window_rects_for_monitor(st, monitor, Instant::now());
    let family = st.aperture_config().peek.clock.font_family.clone();
    let normal = st.aperture_snapshot_for_mode(
        ApertureMode::Normal,
        output_rect,
        work_area_rect,
        scale,
        |font_px, text| {
            let (w, h) = ui_text_size_px_in(render_state, family.as_str(), font_px, text);
            Size {
                w: w as f32,
                h: h as f32,
            }
        },
    );
    if normal
        .as_ref()
        .is_some_and(|snapshot| !clock_obstructed(snapshot.bounds, &windows))
    {
        return ApertureMode::Normal;
    }

    let collapsed = st.aperture_snapshot_for_mode(
        ApertureMode::Collapsed,
        output_rect,
        work_area_rect,
        scale,
        |font_px, text| {
            let (w, h) = ui_text_size_px_in(render_state, family.as_str(), font_px, text);
            Size {
                w: w as f32,
                h: h as f32,
            }
        },
    );
    if collapsed
        .as_ref()
        .is_some_and(|snapshot| !clock_obstructed(snapshot.bounds, &windows))
    {
        ApertureMode::Collapsed
    } else {
        ApertureMode::Hidden
    }
}

fn clock_obstructed(clock_bounds: Rect, windows: &[Rect]) -> bool {
    windows
        .iter()
        .copied()
        .any(|window| rects_intersect(clock_bounds, window))
}

fn active_window_rects_for_monitor(st: &Halley, monitor: &str, now: Instant) -> Vec<Rect> {
    let Some(space) = st.model.monitor_state.monitors.get(monitor) else {
        return Vec::new();
    };
    let width = space.width.max(1);
    let height = space.height.max(1);

    st.model
        .field
        .nodes()
        .iter()
        .filter_map(|(&node_id, node)| {
            aperture_obstruction_candidate(st, node_id, node, monitor).then_some(node_id)
        })
        .filter_map(|node_id| {
            node_screen_rect_for_monitor(st, node_id, space.viewport, width, height, now)
        })
        .collect()
}

fn aperture_obstruction_candidate(
    st: &Halley,
    node_id: NodeId,
    node: &halley_core::field::Node,
    monitor: &str,
) -> bool {
    node.kind == NodeKind::Surface
        && matches!(
            node.state,
            NodeState::Active | NodeState::Drifting | NodeState::Node
        )
        && st.model.field.is_visible(node_id)
        && node_belongs_to_monitor(st, node_id, monitor)
}

fn node_screen_rect_for_monitor(
    st: &Halley,
    node_id: NodeId,
    viewport: Viewport,
    width: i32,
    height: i32,
    now: Instant,
) -> Option<Rect> {
    if st.model.monitor_state.current_monitor
        == st
            .model
            .monitor_state
            .node_monitor
            .get(&node_id)
            .map(String::as_str)
            .unwrap_or(st.model.monitor_state.current_monitor.as_str())
    {
        if let Some((left, top, right, bottom)) =
            crate::input::active_node_screen_rect(st, width, height, node_id, now, None)
        {
            return Some(Rect::new(
                left.min(right),
                top.min(bottom),
                (right - left).abs(),
                (bottom - top).abs(),
            ));
        }
    }

    let node = st.model.field.node(node_id)?;
    let view_w = viewport.size.x.max(1.0);
    let view_h = viewport.size.y.max(1.0);
    let nx = ((node.pos.x - viewport.center.x) / view_w) + 0.5;
    let ny = ((node.pos.y - viewport.center.y) / view_h) + 0.5;
    let cx = nx * width as f32;
    let cy = ny * height as f32;

    let (_, _, local_w, local_h) = st
        .ui
        .render_state
        .cache
        .window_geometry
        .get(&node_id)
        .copied()
        .map(|(x, y, w, h)| (x, y, w.max(1.0), h.max(1.0)))
        .unwrap_or((
            0.0,
            0.0,
            node.intrinsic_size.x.max(1.0),
            node.intrinsic_size.y.max(1.0),
        ));
    let left = cx - (local_w * 0.5);
    let top = cy - (local_h * 0.5);
    Some(Rect::new(left, top, local_w, local_h))
}

fn node_belongs_to_monitor(st: &Halley, node_id: NodeId, monitor: &str) -> bool {
    st.model
        .monitor_state
        .node_monitor
        .get(&node_id)
        .map(|owner| owner.as_str())
        .unwrap_or(st.model.monitor_state.current_monitor.as_str())
        == monitor
}

fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

pub(crate) fn log_aperture_config_startup(path: &PathBuf) {
    debug!("aperture config path: {}", path.display());
}
