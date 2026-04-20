pub(crate) mod core;
mod config;

use std::path::{Path, PathBuf};
use std::time::Instant;

use eventline::{debug, warn};
use halley_ipc::{ApertureMode as IpcApertureMode, ApertureStatusResponse};

use crate::compositor::root::Halley;
use crate::text::ui_text_size_px_in;

use halley_core::field::NodeId;

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
    let usable = crate::compositor::monitor::layer_shell::layer_shell_usable_rect_for_monitor(
        st,
        monitor.as_str(),
    );
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
    let mode = derive_aperture_mode(st, output_rect, work_area_rect, 1.0);
    ApertureStatusResponse {
        output: Some(monitor),
        mode: match mode {
            ApertureMode::Normal => IpcApertureMode::Normal,
            ApertureMode::Collapsed => IpcApertureMode::Collapsed,
            ApertureMode::Hidden => IpcApertureMode::Hidden,
        },
    }
}

fn derive_aperture_mode(
    st: &Halley,
    output_rect: Rect,
    work_area_rect: Rect,
    scale: f64,
) -> ApertureMode {
    let render_state = &st.ui.render_state;
    let windows = active_window_rects_for_current_monitor(st, Instant::now());
    let family = st.aperture_config().clock.font_family.clone();
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

fn active_window_rects_for_current_monitor(st: &Halley, now: Instant) -> Vec<Rect> {
    let monitor = st.model.monitor_state.current_monitor.as_str();
    let width = st
        .model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| space.width)
        .unwrap_or(st.model.viewport.size.x.round().max(1.0) as i32);
    let height = st
        .model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| space.height)
        .unwrap_or(st.model.viewport.size.y.round().max(1.0) as i32);

    st.model
        .field
        .nodes()
        .iter()
        .filter_map(|(&node_id, node)| {
            (node.state == halley_core::field::NodeState::Active
                && st.model.field.is_visible(node_id)
                && node_belongs_to_monitor(st, node_id, monitor))
            .then_some(node_id)
        })
        .filter_map(|node_id| {
            crate::input::active_node_screen_rect(st, width, height, node_id, now, None).map(
                |(left, top, right, bottom)| {
                    Rect::new(
                        left.min(right),
                        top.min(bottom),
                        (right - left).abs(),
                        (bottom - top).abs(),
                    )
                },
            )
        })
        .collect()
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
