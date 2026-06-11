mod config;
pub(crate) mod core;

use std::path::{Path, PathBuf};
use std::time::Instant;

use eventline::{debug, warn};
use halley_api::{ApertureMode as IpcApertureMode, ApertureOutputStatus, ApertureStatusResponse};

use crate::compositor::root::Halley;

#[cfg(test)]
use halley_core::field::{NodeId, NodeState};
#[cfg(test)]
use halley_core::viewport::Viewport;

use self::core::{ApertureConfig, ApertureMode, ApertureRuntime, Rect, minimal_font_px};

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

    pub(crate) fn cached_mode(&self, monitor: &str, now: Instant) -> Option<ApertureMode> {
        self.runtime.cached_mode(monitor, now)
    }

    pub(crate) fn store_mode(&self, monitor: &str, mode: ApertureMode, now: Instant) {
        self.runtime.store_mode(monitor, mode, now);
    }

    pub(crate) fn invalidate_mode_cache(&self) {
        self.runtime.invalidate_mode_cache();
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

    // Reuse the current monitor's derived mode from the outputs list rather than
    // deriving it a second time.
    let mode = outputs
        .iter()
        .find(|output| output.output == monitor)
        .map(|output| output.mode)
        .unwrap_or_else(|| map_ipc_mode(derive_aperture_mode_for_monitor(st, monitor.as_str())));

    ApertureStatusResponse {
        output: Some(monitor),
        mode,
        outputs,
    }
}

fn derive_aperture_mode_for_monitor(st: &Halley, monitor: &str) -> ApertureMode {
    let now = Instant::now();
    if let Some(mode) = st.aperture.cached_mode(monitor, now) {
        return mode;
    }

    let perf_start = crate::perf::start();
    let output_size =
        crate::compositor::monitor::layer_shell::layer_output_size_for_monitor(st, monitor);
    let output_rect = Rect::new(
        0.0,
        0.0,
        output_size.w.max(1) as f32,
        output_size.h.max(1) as f32,
    );
    let mode = derive_aperture_mode(st, monitor, output_rect, output_rect, 1.0);
    st.aperture.store_mode(monitor, mode, now);
    if let Some(start) = perf_start {
        eventline::info!(
            "perf aperture_derive monitor={} mode={:?} took={:.2}ms",
            monitor,
            mode,
            crate::perf::elapsed_ms(start)
        );
    }
    mode
}

fn map_ipc_mode(mode: ApertureMode) -> IpcApertureMode {
    match mode {
        ApertureMode::Normal => IpcApertureMode::Normal,
        ApertureMode::Minimal => IpcApertureMode::Minimal,
        ApertureMode::Hidden => IpcApertureMode::Hidden,
    }
}

fn derive_aperture_mode(
    st: &Halley,
    monitor: &str,
    _output_rect: Rect,
    _work_area_rect: Rect,
    _scale: f64,
) -> ApertureMode {
    if st
        .model
        .fullscreen_state
        .fullscreen_active_node
        .get(monitor)
        .is_some_and(|node_id| crate::window::node_is_game_like(st, *node_id))
    {
        return ApertureMode::Hidden;
    }

    let minimal_intended =
        crate::compositor::workspace::state::maximize_session_active_on_monitor(st, monitor)
            || crate::compositor::clusters::system::active_cluster_workspace_for_monitor(
                st, monitor,
            )
            .is_some();
    if minimal_intended {
        return ApertureMode::Minimal;
    }

    ApertureMode::Normal
}

#[cfg(test)]
fn unobstructed_aperture_mode_for_bounds(
    mode: ApertureMode,
    bounds: Rect,
    windows: &[Rect],
) -> Option<ApertureMode> {
    (!clock_obstructed(bounds, windows)).then_some(mode)
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

/// True when a minimal aperture tab is the intended mode for `monitor` (an active
/// tiling cluster workspace or a maximize session). Used to gate learning of the
/// aperture's minimal-tab height: only commits made while minimal is intended are
/// recorded, so the Minimal→Normal close ramp (which climbs back through the
/// accepted height band) cannot pollute the stored value with a too-large height.
pub(crate) fn monitor_minimal_aperture_intended(st: &Halley, monitor: &str) -> bool {
    existing_minimal_layout_top_gap_px(st, monitor).is_some()
}

fn existing_minimal_layout_top_gap_px(st: &Halley, monitor: &str) -> Option<f32> {
    if crate::compositor::workspace::state::maximize_session_active_on_monitor(st, monitor) {
        return Some(st.runtime.tuning.non_overlap_gap_px.max(0.0));
    }

    if crate::compositor::clusters::system::active_cluster_workspace_for_monitor(st, monitor)
        .is_some()
    {
        return Some(st.runtime.tuning.tile_gaps_outer_px.max(0.0));
    }

    None
}

fn required_minimal_aperture_clearance_px(st: &Halley, _monitor: &str) -> f32 {
    // The Minimal state is the reserved bar; its height is an explicit config value
    // (`clock-small.height-px`) that the client renders to. Reserving it directly
    // makes the top clearance exact from the very first frame — no glyph
    // measurement, no learned/cached guess, and it can't drift when sizes change.
    let small_height_px = st.aperture_config().peek.clock.small_height_px.max(1) as f32;
    small_height_px + APERTURE_AFTER_GAP_PX
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

    fn two_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = single_monitor_tuning();
        tuning
            .tty_viewports
            .push(halley_config::ViewportOutputConfig {
                connector: "monitor_b".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            });
        tuning
    }

    fn test_output_rect() -> Rect {
        Rect::new(0.0, 0.0, 800.0, 600.0)
    }

    fn derive_test_mode(st: &Halley, monitor: &str) -> ApertureMode {
        let rect = test_output_rect();
        derive_aperture_mode(st, monitor, rect, rect, 1.0)
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
    fn required_clearance_uses_config_small_bar_height() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let mut config = ApertureConfig::default();
        config.peek.clock.small_height_px = 22;
        st.apply_aperture_config(config);

        // Reservation = configured bar height + APERTURE_AFTER_GAP_PX (4).
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
    fn committed_aperture_height_does_not_affect_reservation() {
        // The reservation is driven purely by config (`clock-small.height-px`), so
        // whatever height the client actually commits — even an expanded one — does
        // not move the reserved bar.
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let mut config = ApertureConfig::default();
        config.peek.clock.small_height_px = 22;
        st.apply_aperture_config(config);
        set_aperture_tab_height(&mut st, 160);

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
    fn field_maximize_freezes_aperture_workarea_after_baseline() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        set_aperture_tab_height(&mut st, 22);
        let mut config = ApertureConfig::default();
        config.peek.clock.small_height_px = 22;
        st.apply_aperture_config(config.clone());

        let required = required_minimal_aperture_clearance_px(&st, MONITOR).ceil();
        st.runtime.tuning.non_overlap_gap_px = required - 10.0;
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
        assert_eq!(usable_top_px(&st), 10);

        config.peek.clock.small_height_px = 42;
        st.apply_aperture_config(config);

        assert_eq!(
            usable_top_px(&st),
            10,
            "aperture commits must not rebase active field maximize work area"
        );
        assert!(
            st.model
                .monitor_state
                .pending_workarea_refresh
                .contains(MONITOR)
        );
    }

    #[test]
    fn leaving_field_maximize_applies_deferred_aperture_workarea() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        set_aperture_tab_height(&mut st, 22);
        let mut config = ApertureConfig::default();
        config.peek.clock.small_height_px = 22;
        st.apply_aperture_config(config.clone());

        let required = required_minimal_aperture_clearance_px(&st, MONITOR).ceil();
        st.runtime.tuning.non_overlap_gap_px = required - 10.0;
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
        config.peek.clock.small_height_px = 42;
        st.apply_aperture_config(config);
        assert_eq!(usable_top_px(&st), 10);

        assert!(
            crate::compositor::workspace::state::abort_maximize_session_for_monitor(
                &mut st, MONITOR,
            )
        );

        assert_eq!(usable_top_px(&st), 0);
        assert!(
            !st.model
                .monitor_state
                .pending_workarea_refresh
                .contains(MONITOR)
        );
    }

    #[test]
    fn restoring_field_maximize_keeps_aperture_workarea_frozen() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        // Keep the session alive in `Restoring` state after the un-maximize toggle.
        st.runtime.tuning.animations.enabled = true;
        st.runtime.tuning.animations.maximize.enabled = true;
        set_aperture_tab_height(&mut st, 22);
        let mut config = ApertureConfig::default();
        config.peek.clock.small_height_px = 22;
        st.apply_aperture_config(config.clone());

        let required = required_minimal_aperture_clearance_px(&st, MONITOR).ceil();
        st.runtime.tuning.non_overlap_gap_px = required - 10.0;
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
        assert_eq!(usable_top_px(&st), 10);

        // A taller aperture commit must defer while the session is active.
        config.peek.clock.small_height_px = 42;
        st.apply_aperture_config(config);
        assert_eq!(usable_top_px(&st), 10);

        // Toggling off enters the restore animation; the work area must stay frozen
        // until the session ends, not pop back as the window slides shut.
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut st,
                id,
                Instant::now(),
                MONITOR,
            )
        );
        assert_eq!(
            usable_top_px(&st),
            10,
            "restore animation must keep the aperture work area frozen"
        );
        assert!(
            st.model
                .monitor_state
                .pending_workarea_refresh
                .contains(MONITOR)
        );
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

    #[test]
    fn minimal_mode_hides_when_even_small_tab_is_obstructed() {
        let clock = Rect::new(760.0, 0.0, 30.0, 16.0);
        let blocker = Rect::new(0.0, 0.0, 800.0, 600.0);

        assert_eq!(
            unobstructed_aperture_mode_for_bounds(ApertureMode::Minimal, clock, &[blocker]),
            None
        );
    }

    #[test]
    fn minimal_mode_survives_when_small_tab_is_unobstructed() {
        let clock = Rect::new(760.0, 0.0, 30.0, 16.0);
        let blocker = Rect::new(0.0, 64.0, 800.0, 536.0);

        assert_eq!(
            unobstructed_aperture_mode_for_bounds(ApertureMode::Minimal, clock, &[blocker]),
            Some(ApertureMode::Minimal)
        );
    }

    #[test]
    fn game_fullscreen_hides_only_on_that_monitor() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, two_monitor_tuning());

        let game = st.model.field.spawn_surface(
            "game",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 800.0, y: 600.0 },
        );
        st.assign_node_to_monitor(game, MONITOR);
        st.model.node_app_ids.insert(game, "steam_app_42".into());
        st.model
            .fullscreen_state
            .fullscreen_active_node
            .insert(MONITOR.to_string(), game);

        assert_eq!(derive_test_mode(&st, MONITOR), ApertureMode::Hidden);
        assert_ne!(derive_test_mode(&st, "monitor_b"), ApertureMode::Hidden);
    }

    #[test]
    fn non_game_fullscreen_keeps_field_underlay_mode() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let video = st.model.field.spawn_surface(
            "video",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 800.0, y: 600.0 },
        );
        st.assign_node_to_monitor(video, MONITOR);
        st.model.node_app_ids.insert(video, "firefox".into());
        st.model
            .fullscreen_state
            .fullscreen_active_node
            .insert(MONITOR.to_string(), video);

        assert_eq!(derive_test_mode(&st, MONITOR), ApertureMode::Normal);
    }

    #[test]
    fn maximized_mode_keeps_minimal_when_small_tab_is_unobstructed() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        set_aperture_tab_height(&mut st, 22);

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

        assert_eq!(derive_test_mode(&st, MONITOR), ApertureMode::Minimal);
    }

    #[test]
    fn tiled_cluster_mode_keeps_minimal_when_small_tab_is_unobstructed() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        set_aperture_tab_height(&mut st, 22);
        let now = Instant::now();

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

        assert_eq!(derive_test_mode(&st, MONITOR), ApertureMode::Minimal);
    }

    #[test]
    fn collapsed_surface_obstruction_uses_marker_extents() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let id = st.model.field.spawn_surface(
            "collapsed-firefox",
            Vec2 { x: 160.0, y: 0.0 },
            Vec2 {
                x: 1200.0,
                y: 900.0,
            },
        );
        st.assign_node_to_monitor(id, MONITOR);
        let _ = st.model.field.set_state(id, NodeState::Node);

        let (viewport, width, height) = {
            let space = st
                .model
                .monitor_state
                .monitors
                .get(MONITOR)
                .expect("monitor");
            (space.viewport, space.width.max(1), space.height.max(1))
        };
        let now = Instant::now();
        let rect = node_screen_rect_for_monitor(&st, id, viewport, width, height, now)
            .expect("collapsed obstruction rect");
        let node = st.model.field.node(id).expect("node");
        let anim = crate::frame_loop::anim_style_for(&st, id, node.state.clone(), now);
        let expected = crate::presentation::node_render_diameter_px(
            &st,
            node.intrinsic_size,
            node.label.len(),
            anim.scale,
        )
        .round()
        .max(1.0);

        assert_eq!(rect.w, expected);
        assert_eq!(rect.h, expected);

        let center_x = rect.x + rect.w * 0.5;
        let center_y = rect.y + rect.h * 0.5;
        let open_window_rect = Rect::new(center_x - 600.0, center_y - 450.0, 1200.0, 900.0);
        let clock_bounds = Rect::new(rect.right() + 20.0, rect.y, 80.0, rect.h);

        assert!(
            clock_obstructed(clock_bounds, &[open_window_rect]),
            "test setup should intersect the old open-window obstruction rect"
        );
        assert!(
            !clock_obstructed(clock_bounds, &[rect]),
            "collapsed nodes should only obstruct aperture with their marker extents"
        );
    }
}

#[cfg(test)]
fn clock_obstructed(clock_bounds: Rect, windows: &[Rect]) -> bool {
    windows
        .iter()
        .copied()
        .any(|window| rects_intersect(clock_bounds, window))
}

#[cfg(test)]
fn node_screen_rect_for_monitor(
    st: &Halley,
    node_id: NodeId,
    viewport: Viewport,
    width: i32,
    height: i32,
    now: Instant,
) -> Option<Rect> {
    let node = st.model.field.node(node_id)?;
    if st.model.monitor_state.current_monitor
        == st
            .model
            .monitor_state
            .node_monitor
            .get(&node_id)
            .map(String::as_str)
            .unwrap_or(st.model.monitor_state.current_monitor.as_str())
        && node.state == NodeState::Active
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

    let view_w = viewport.size.x.max(1.0);
    let view_h = viewport.size.y.max(1.0);
    let nx = ((node.pos.x - viewport.center.x) / view_w) + 0.5;
    let ny = ((node.pos.y - viewport.center.y) / view_h) + 0.5;
    let cx = nx * width as f32;
    let cy = ny * height as f32;

    let (local_w, local_h) = if node.state == NodeState::Node {
        let anim = crate::frame_loop::anim_style_for(st, node_id, node.state.clone(), now);
        let diameter = crate::presentation::node_render_diameter_px(
            st,
            node.intrinsic_size,
            node.label.len(),
            anim.scale,
        )
        .round()
        .max(1.0);
        (diameter, diameter)
    } else {
        st.ui
            .render_state
            .cache
            .window_geometry
            .get(&node_id)
            .copied()
            .map(|(_, _, w, h)| (w.max(1.0), h.max(1.0)))
            .unwrap_or((
                node.intrinsic_size.x.max(1.0),
                node.intrinsic_size.y.max(1.0),
            ))
    };
    let left = cx - (local_w * 0.5);
    let top = cy - (local_h * 0.5);
    Some(Rect::new(left, top, local_w, local_h))
}

#[cfg(test)]
fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

pub(crate) fn log_aperture_config_startup(path: &PathBuf) {
    debug!("aperture config path: {}", path.display());
}
