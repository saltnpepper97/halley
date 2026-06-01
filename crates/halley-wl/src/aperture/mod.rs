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

use self::core::{
    ApertureConfig, ApertureMode, ApertureRuntime, ClockSnapshot, Rect, Size, minimal_font_px,
};

const MINIMAL_TAB_PADDING_Y_PX: f32 = 4.0;
const MINIMAL_TAB_HEIGHT_TOLERANCE_PX: f32 = 12.0;
const APERTURE_AFTER_GAP_PX: f32 = 4.0;

pub(crate) use config::{
    config_matches_event_path, config_watch_roots, default_aperture_config_path,
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
    let output_size =
        crate::compositor::monitor::layer_shell::layer_output_size_for_monitor(st, monitor);
    let output_rect = Rect::new(
        0.0,
        0.0,
        output_size.w.max(1) as f32,
        output_size.h.max(1) as f32,
    );
    derive_aperture_mode(st, monitor, output_rect, output_rect, 1.0)
}

fn map_ipc_mode(mode: ApertureMode) -> IpcApertureMode {
    match mode {
        ApertureMode::Normal => IpcApertureMode::Normal,
        ApertureMode::Collapsed => IpcApertureMode::Collapsed,
        ApertureMode::Minimal => IpcApertureMode::Minimal,
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
    if st
        .model
        .fullscreen_state
        .fullscreen_active_node
        .contains_key(monitor)
    {
        return ApertureMode::Hidden;
    }

    if crate::compositor::workspace::state::maximize_session_active_on_monitor(st, monitor)
        || (crate::compositor::clusters::system::active_cluster_workspace_for_monitor(st, monitor)
            .is_some()
            && matches!(
                st.runtime.tuning.cluster_layout_kind(),
                halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
            ))
    {
        return ApertureMode::Minimal;
    }

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
        return ApertureMode::Collapsed;
    }

    ApertureMode::Minimal
}

pub(crate) fn small_reservation_px_for_monitor(st: &Halley, monitor: &str) -> i32 {
    if st
        .model
        .fullscreen_state
        .fullscreen_active_node
        .contains_key(monitor)
    {
        return 0;
    }

    if !crate::compositor::monitor::layer_shell::aperture_layer_present_for_monitor(st, monitor) {
        return 0;
    }

    let Some(existing_gap_px) = existing_minimal_layout_top_gap_px(st, monitor) else {
        return 0;
    };
    let required_clearance_px = required_minimal_aperture_clearance_px(st, monitor);
    aperture_reserve_px_for_clearance(required_clearance_px, existing_gap_px)
}

fn existing_minimal_layout_top_gap_px(st: &Halley, monitor: &str) -> Option<f32> {
    if crate::compositor::workspace::state::maximize_session_active_on_monitor(st, monitor) {
        return Some(st.runtime.tuning.non_overlap_gap_px.max(0.0));
    }

    if crate::compositor::clusters::system::active_cluster_workspace_for_monitor(st, monitor)
        .is_some()
        && matches!(
            st.runtime.tuning.cluster_layout_kind(),
            halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
        )
    {
        return Some(st.runtime.tuning.tile_gaps_outer_px.max(0.0));
    }

    None
}

fn required_minimal_aperture_clearance_px(st: &Halley, monitor: &str) -> f32 {
    let fallback_tab_height = fallback_minimal_aperture_tab_height_px(st);
    let tab_height =
        crate::compositor::monitor::layer_shell::aperture_minimal_tab_height_for_monitor(
            st, monitor,
        )
        .unwrap_or(fallback_tab_height);

    tab_height.max(1.0) + APERTURE_AFTER_GAP_PX
}

pub(crate) fn accepted_minimal_aperture_tab_height_px(st: &Halley, height_px: i32) -> Option<i32> {
    let height = height_px as f32;
    let max_plausible =
        fallback_minimal_aperture_tab_height_px(st) + MINIMAL_TAB_HEIGHT_TOLERANCE_PX;
    (height > 1.0 && height <= max_plausible.max(1.0)).then_some(height_px)
}

fn fallback_minimal_aperture_tab_height_px(st: &Halley) -> f32 {
    minimal_font_px(st.aperture_config().peek.clock.font_px) as f32 + MINIMAL_TAB_PADDING_Y_PX * 2.0
}

fn aperture_reserve_px_for_clearance(required_clearance_px: f32, existing_gap_px: f32) -> i32 {
    (required_clearance_px.max(0.0) - existing_gap_px.max(0.0))
        .max(0.0)
        .ceil() as i32
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use halley_core::field::{NodeId, Vec2};
    use smithay::reexports::wayland_server::Display;

    use super::*;

    const MONITOR: &str = "monitor_a";

    fn single_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        tuning.decorations.border.size_px = 0;
        tuning.decorations.secondary_border.enabled = false;
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: MONITOR.to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        }];
        tuning
    }

    fn usable_top_px(st: &Halley) -> i32 {
        let viewport = st
            .model
            .monitor_state
            .monitors
            .get(MONITOR)
            .expect("monitor")
            .usable_viewport;
        (viewport.center.y - viewport.size.y * 0.5).round() as i32
    }

    fn use_large_aperture_config(st: &mut Halley) {
        let mut config = ApertureConfig::default();
        config.peek.clock.font_px = 80;
        st.apply_aperture_config(config);
    }

    fn mark_aperture_layer_present(st: &mut Halley) {
        st.model
            .monitor_state
            .aperture_layer_monitors
            .insert(MONITOR.to_string());
    }

    fn set_aperture_tab_height(st: &mut Halley, height_px: i32) {
        mark_aperture_layer_present(st);
        st.model
            .monitor_state
            .aperture_layer_heights
            .insert(MONITOR.to_string(), height_px);
    }

    fn final_maximize_top_px(st: &Halley) -> i32 {
        usable_top_px(st) + st.runtime.tuning.non_overlap_gap_px.round() as i32
    }

    fn node_top_px(st: &Halley, id: NodeId) -> i32 {
        let node = st.model.field.node(id).expect("node");
        (node.pos.y - node.intrinsic_size.y * 0.5).round() as i32
    }

    #[test]
    fn field_gap_contributes_to_aperture_reserve() {
        assert_eq!(aperture_reserve_px_for_clearance(30.0, 20.0), 10);
    }

    #[test]
    fn tile_outer_gap_contributes_to_aperture_reserve() {
        assert_eq!(aperture_reserve_px_for_clearance(30.0, 10.0), 20);
    }

    #[test]
    fn large_user_gap_needs_no_aperture_reserve() {
        assert_eq!(aperture_reserve_px_for_clearance(30.0, 40.0), 0);
    }

    #[test]
    fn required_clearance_uses_actual_minimal_tab_height() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        use_large_aperture_config(&mut st);
        set_aperture_tab_height(&mut st, 22);

        assert_eq!(required_minimal_aperture_clearance_px(&st, MONITOR), 26.0);
    }

    #[test]
    fn expanded_aperture_height_is_rejected_for_minimal_reserve() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        use_large_aperture_config(&mut st);

        assert_eq!(accepted_minimal_aperture_tab_height_px(&st, 160), None);
    }

    #[test]
    fn expanded_aperture_height_does_not_overwrite_cached_minimal_tab_height() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        use_large_aperture_config(&mut st);
        set_aperture_tab_height(&mut st, 22);

        if let Some(height) = accepted_minimal_aperture_tab_height_px(&st, 160) {
            st.model
                .monitor_state
                .aperture_layer_heights
                .insert(MONITOR.to_string(), height);
        }

        assert_eq!(required_minimal_aperture_clearance_px(&st, MONITOR), 26.0);
    }

    #[test]
    fn switching_between_maximized_and_tiled_layouts_recomputes_usable_viewport() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        use_large_aperture_config(&mut st);
        set_aperture_tab_height(&mut st, 22);

        let required = required_minimal_aperture_clearance_px(&st, MONITOR).ceil();
        st.runtime.tuning.non_overlap_gap_px = required - 10.0;
        st.runtime.tuning.tile_gaps_outer_px = required - 20.0;
        let original_field_gap = st.runtime.tuning.non_overlap_gap_px;
        let original_tile_gap = st.runtime.tuning.tile_gaps_outer_px;

        let maximized = st.model.field.spawn_surface(
            "maximized",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(maximized, MONITOR);
        let now = Instant::now();
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut st, maximized, now, MONITOR,
            )
        );
        assert_eq!(small_reservation_px_for_monitor(&st, MONITOR), 10);
        assert_eq!(usable_top_px(&st), 10);
        assert_eq!(final_maximize_top_px(&st), required as i32);

        assert!(
            crate::compositor::workspace::state::abort_maximize_session_for_monitor(
                &mut st, MONITOR,
            )
        );

        let master = st.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = st.model.field.spawn_surface(
            "stack",
            Vec2 { x: 500.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(master, MONITOR);
        st.assign_node_to_monitor(stack, MONITOR);
        let cid = st.create_cluster(vec![master, stack]).expect("cluster");
        let core = st.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, MONITOR);
        assert!(st.enter_cluster_workspace_by_core(core, MONITOR, now));

        assert_eq!(small_reservation_px_for_monitor(&st, MONITOR), 20);
        assert_eq!(usable_top_px(&st), 20);
        assert_eq!(node_top_px(&st, master), required as i32);
        assert_eq!(st.runtime.tuning.non_overlap_gap_px, original_field_gap);
        assert_eq!(st.runtime.tuning.tile_gaps_outer_px, original_tile_gap);
    }

    #[test]
    fn existing_gap_that_fits_tab_adds_no_reserve() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        use_large_aperture_config(&mut st);
        set_aperture_tab_height(&mut st, 22);
        st.runtime.tuning.non_overlap_gap_px = 40.0;

        let id = st.model.field.spawn_surface(
            "maximized",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(id, MONITOR);
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut st,
                id,
                Instant::now(),
                MONITOR,
            )
        );

        assert_eq!(small_reservation_px_for_monitor(&st, MONITOR), 0);
        assert_eq!(usable_top_px(&st), 0);
        assert_eq!(final_maximize_top_px(&st), 40);
    }

    #[test]
    fn missing_aperture_layer_produces_no_phantom_reserve() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        use_large_aperture_config(&mut st);
        st.runtime.tuning.non_overlap_gap_px = 0.0;

        let id = st.model.field.spawn_surface(
            "maximized",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(id, MONITOR);
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut st,
                id,
                Instant::now(),
                MONITOR,
            )
        );

        assert_eq!(small_reservation_px_for_monitor(&st, MONITOR), 0);
        assert_eq!(usable_top_px(&st), 0);
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
