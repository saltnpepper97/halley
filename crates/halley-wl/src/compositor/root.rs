use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use calloop::LoopHandle;
use halley_config::RuntimeTuning;
use halley_core::cluster_policy::ClusterFormationState;
use halley_core::field::{Field, NodeId, Vec2};
use halley_core::viewport::Viewport;
use smithay::{
    delegate_dmabuf,
    desktop::PopupManager,
    input::{SeatState, pointer::CursorImageStatus},
    reexports::wayland_server::{DisplayHandle, backend::ObjectId},
    wayland::{
        compositor::CompositorState,
        cursor_shape::CursorShapeManagerState,
        dmabuf::DmabufState,
        idle_notify::IdleNotifierState,
        output::OutputManagerState,
        pointer_constraints::PointerConstraintsState,
        relative_pointer::RelativePointerManagerState,
        selection::{
            data_device::DataDeviceState, primary_selection::PrimarySelectionState,
            wlr_data_control::DataControlState,
        },
        shell::wlr_layer::WlrLayerShellState,
        shell::xdg::{XdgShellState, decoration::XdgDecorationState},
        shm::ShmState,
        viewporter::ViewporterState,
    },
};

use super::carry::state::CarryState;
use super::clusters::state::ClusterState;
use super::focus::state::FocusState;
use super::fullscreen::state::FullscreenState;
use super::interaction::state::InteractionState;
use super::monitor::state::{MonitorSpace, MonitorState};
use super::platform::PlatformState;
use super::runtime::RuntimeState;
use super::spawn::state::{MonitorSpawnState, SpawnState};
use super::workspace::state::WorkspaceState;
use crate::animation::Animator;
use crate::render::state::RenderState;

pub(crate) struct ModelState {
    pub(crate) carry_state: CarryState,
    pub(crate) monitor_state: MonitorState,
    pub(crate) focus_state: FocusState,
    pub(crate) cluster_state: ClusterState,
    pub(crate) workspace_state: WorkspaceState,
    pub(crate) fullscreen_state: FullscreenState,
    pub(crate) spawn_state: SpawnState,
    pub(crate) field: Field,
    pub(crate) viewport: Viewport,
    pub(crate) zoom_ref_size: Vec2,
    pub(crate) camera_target_center: Vec2,
    pub(crate) camera_target_view_size: Vec2,
    pub(crate) surface_to_node: HashMap<ObjectId, NodeId>,
    pub(crate) node_app_ids: HashMap<NodeId, String>,
}

pub(crate) struct UiState {
    pub(crate) render_state: RenderState,
}

pub(crate) struct InputState {
    pub(crate) interaction_state: InteractionState,
}

pub struct Halley {
    pub(crate) platform: PlatformState,
    pub(crate) model: ModelState,
    pub(crate) ui: UiState,
    pub(crate) input: InputState,
    pub(crate) portal: crate::protocol::wayland::portal::PortalState,
    pub(crate) runtime: RuntimeState,
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

impl Halley {
    pub(crate) const CLUSTER_OVERFLOW_REVEAL_EDGE_PX: f32 = 28.0;
    pub(crate) const VIEWPORT_PAN_PRELOAD_MS: u64 = 70;
    pub(crate) const VIEWPORT_PAN_DURATION_MS: u64 = 260;

    pub fn new(
        dh: &DisplayHandle,
        loop_handle: LoopHandle<'static, Self>,
        tuning: RuntimeTuning,
    ) -> Self {
        let now = Instant::now();
        let mut monitors = HashMap::new();
        for viewport in tuning
            .tty_viewports
            .iter()
            .filter(|viewport| viewport.enabled)
        {
            let width = viewport.width.max(1) as i32;
            let height = viewport.height.max(1) as i32;
            // MonitorSpace viewport uses GLOBAL world coordinates. The center
            // is at (offset_x + width/2, offset_y + height/2) so that every
            // monitor occupies a unique region of world space. Using local
            // (0,0)-origin coordinates caused monitors to share the same world
            // positions, breaking spawn placement, overlap resolution, focus
            // ring checks, and drag clamping across monitors.
            let global_center = Vec2 {
                x: viewport.offset_x as f32 + width as f32 * 0.5,
                y: viewport.offset_y as f32 + height as f32 * 0.5,
            };
            let view = Viewport::new(
                global_center,
                Vec2 {
                    x: width as f32,
                    y: height as f32,
                },
            );
            monitors.insert(
                viewport.connector.clone(),
                MonitorSpace {
                    offset_x: viewport.offset_x,
                    offset_y: viewport.offset_y,
                    width,
                    height,
                    viewport: view,
                    usable_viewport: view,
                    zoom_ref_size: view.size,
                    camera_target_center: view.center,
                    camera_target_view_size: view.size,
                },
            );
        }
        if monitors.is_empty() {
            let view = tuning.viewport();
            monitors.insert(
                "default".to_string(),
                MonitorSpace {
                    offset_x: 0,
                    offset_y: 0,
                    width: tuning.viewport_size.x.max(1.0).round() as i32,
                    height: tuning.viewport_size.y.max(1.0).round() as i32,
                    viewport: view,
                    usable_viewport: view,
                    zoom_ref_size: tuning.viewport_size,
                    camera_target_center: tuning.viewport_center,
                    camera_target_view_size: tuning.viewport_size,
                },
            );
        }
        let current_monitor =
            preferred_monitor_name(&monitors).unwrap_or_else(|| "default".to_string());
        let primary_viewport = monitors
            .get(&current_monitor)
            .map(|m| m.viewport)
            .unwrap_or_else(|| tuning.viewport());
        let primary_zoom_ref = monitors
            .get(&current_monitor)
            .map(|m| m.zoom_ref_size)
            .unwrap_or(tuning.viewport_size);
        let mut seat_state = SeatState::new();
        let seat = seat_state.new_wl_seat(dh, "halley");
        let primary_selection_state = PrimarySelectionState::new::<Halley>(dh);
        let data_control_state =
            DataControlState::new::<Halley, _>(dh, Some(&primary_selection_state), |_| true);

        let mut out = Self {
            platform: PlatformState {
                display_handle: dh.clone(),
                compositor_state: CompositorState::new::<Halley>(dh),
                viewporter_state: ViewporterState::new::<Halley>(dh),
                xdg_shell_state: XdgShellState::new::<Halley>(dh),
                xdg_activation_state: smithay::wayland::xdg_activation::XdgActivationState::new::<
                    Halley,
                >(dh),
                xdg_decoration_state: XdgDecorationState::new::<Halley>(dh),
                cursor_shape_manager_state: CursorShapeManagerState::new::<Halley>(dh),
                popup_manager: PopupManager::default(),
                wlr_layer_shell_state: WlrLayerShellState::new::<Halley>(dh),
                pointer_constraints_state: PointerConstraintsState::new::<Halley>(dh),
                relative_pointer_manager_state: RelativePointerManagerState::new::<Halley>(dh),
                idle_notifier_state: IdleNotifierState::new(dh, loop_handle),
                drm_syncobj_state: None,
                output_manager_state: OutputManagerState::new_with_xdg_output::<Halley>(dh),
                shm_state: ShmState::new::<Halley>(dh, vec![]),
                dmabuf_state: DmabufState::new(),
                dmabuf_global: None,
                seat_state,
                data_device_state: DataDeviceState::new::<Halley>(dh),
                primary_selection_state,
                data_control_state,
                seat,
                cursor_image_status: CursorImageStatus::default_named(),
                dmabuf_importer: None,
            },
            model: ModelState {
                carry_state: CarryState {
                    carry_zone_hint: HashMap::new(),
                    carry_zone_last_change_ms: HashMap::new(),
                    carry_zone_pending: HashMap::new(),
                    carry_zone_pending_since_ms: HashMap::new(),
                    carry_activation_anim_armed: HashSet::new(),
                    carry_direct_nodes: HashSet::new(),
                    carry_state_hold: HashMap::new(),
                },
                monitor_state: MonitorState {
                    outputs: HashMap::new(),
                    current_monitor: current_monitor.clone(),
                    interaction_monitor: current_monitor.clone(),
                    focused_monitor: current_monitor.clone(),
                    monitors,
                    node_monitor: HashMap::new(),
                    layer_surface_monitor: HashMap::new(),
                    layer_surface_last_configured_size: HashMap::new(),
                    layer_keyboard_focus: None,
                },
                focus_state: FocusState {
                    interaction_focus_until_ms: 0,
                    last_surface_focus_ms: HashMap::new(),
                    focus_trail: HashMap::new(),
                    blocked_monitor_focus_restore: HashSet::new(),
                    suppress_trail_record_once: false,
                    pan_restore_active_focus: None,
                    app_focused: true,
                    monitor_focus: HashMap::new(),
                    primary_interaction_focus: None,
                    focus_ring_preview_until_ms: HashMap::new(),
                    recent_top_node: None,
                    recent_top_until: None,
                },
                cluster_state: ClusterState {
                    cluster_form_state: ClusterFormationState::default(),
                    active_cluster_workspaces: HashMap::new(),
                    cluster_bloom_open: HashMap::new(),
                    cluster_mode_selected_nodes: HashMap::new(),
                    workspace_hidden_nodes: HashMap::new(),
                    workspace_prev_viewports: HashMap::new(),
                    workspace_core_positions: HashMap::new(),
                    cluster_overflow_members: HashMap::new(),
                    cluster_overflow_rects: HashMap::new(),
                    cluster_overflow_scroll_offsets: HashMap::new(),
                    cluster_overflow_reveal_started_at_ms: HashMap::new(),
                    cluster_overflow_visible_until_ms: HashMap::new(),
                    cluster_overflow_promotion_anim: HashMap::new(),
                },
                workspace_state: WorkspaceState {
                    last_active_size: HashMap::new(),
                    manual_collapsed_nodes: HashSet::new(),
                    active_transition_until_ms: HashMap::new(),
                    primary_promote_cooldown_until_ms: HashMap::new(),
                },
                fullscreen_state: FullscreenState {
                    fullscreen_active_node: HashMap::new(),
                    fullscreen_suspended_node: HashMap::new(),
                    fullscreen_restore: HashMap::new(),
                    fullscreen_motion: HashMap::new(),
                    fullscreen_scale_anim: HashMap::new(),
                    direct_scanout: HashMap::new(),
                },
                spawn_state: SpawnState {
                    pending_spawn_activate_at_ms: HashMap::new(),
                    pending_tiled_insert_reveal_at_ms: HashMap::new(),
                    pending_tiled_insert_preserve_focus: HashSet::new(),
                    pending_spawn_monitor: None,
                    per_monitor: HashMap::new(),
                    pending_spawn_pan_queue: VecDeque::new(),
                    active_spawn_pan: None,
                    applied_window_rules: HashMap::new(),
                    pending_rule_rechecks: HashSet::new(),
                    pending_initial_reveal: HashSet::new(),
                },
                field: Field::new(),
                viewport: primary_viewport,
                zoom_ref_size: primary_zoom_ref,
                camera_target_center: primary_viewport.center,
                camera_target_view_size: primary_zoom_ref,
                surface_to_node: HashMap::new(),
                node_app_ids: HashMap::new(),
            },
            ui: UiState {
                render_state: RenderState {
                    animator: Animator::new(now),
                    node_app_icon_cache: HashMap::new(),
                    node_hover_mix: HashMap::new(),
                    node_preview_hover: HashMap::new(),
                    bearings_visible: false,
                    bearings_mix: HashMap::new(),
                    cluster_tile_tracks: HashMap::new(),
                    cluster_tile_entry_pending: HashSet::new(),
                    cluster_tile_frozen_geometry: HashMap::new(),
                    cluster_bloom_mix: HashMap::new(),
                    overlay_banner: HashMap::new(),
                    overlay_toast: HashMap::new(),
                    overlay_exit_confirm: HashMap::new(),
                    stack_cycle_transition: HashMap::new(),
                    ui_text: std::cell::RefCell::new(crate::render::text::UiTextRenderer::default()),
                    node_circle_texture: None,
                    node_circle_program: None,
                    node_square_program: None,
                    node_squircle_program: None,
                    ui_rect_rounded_program: None,
                    ui_rect_rounded_program_failed: false,
                    ui_rect_square_program: None,
                    ui_rect_square_program_failed: false,
                    window_texture_program: None,
                    window_texture_program_failed: false,
                    surface_clip_program: None,
                    surface_clip_program_failed: false,
                    ui_text_program: None,
                    ui_text_program_failed: false,
                    zoom_nominal_size: HashMap::new(),
                    zoom_resize_fallback: HashSet::new(),
                    zoom_resize_reject_streak: HashMap::new(),
                    zoom_last_observed_size: HashMap::new(),
                    zoom_resize_static_streak: HashMap::new(),
                    render_last_tick: now,
                    bbox_loc: HashMap::new(),
                    window_geometry: HashMap::new(),
                    window_offscreen_cache: HashMap::new(),
                },
            },
            input: InputState {
                interaction_state: InteractionState {
                    reset_input_state_requested: false,
                    pending_pointer_screen_hint: None,
                    last_pointer_screen_global: None,
                    suppress_layer_shell_configure: false,
                    dpms_just_woke: false,
                    resize_active: None,
                    resize_static_node: None,
                    resize_static_lock_pos: None,
                    resize_static_until_ms: 0,
                    drag_authority_node: None,
                    drag_authority_velocity: Vec2 { x: 0.0, y: 0.0 },
                    suspend_overlap_resolve: false,
                    suspend_state_checks: false,
                    physics_velocity: HashMap::new(),
                    physics_last_tick: now,
                    smoothed_render_pos: HashMap::new(),
                    viewport_pan_anim: None,
                    pan_dominant_until_ms: 0,
                    active_drag: None,
                    cluster_join_candidate: None,
                    bloom_pull_preview: None,
                    cluster_overflow_drag_preview: None,
                    overlay_hover_target: None,
                    pending_core_press: None,
                    pending_core_click: None,
                    grabbed_edge_pan_active: false,
                    grabbed_edge_pan_direction: Vec2 { x: 0.0, y: 0.0 },
                    grabbed_edge_pan_pressure: Vec2 { x: 0.0, y: 0.0 },
                    grabbed_edge_pan_monitor: None,
                    cursor_override_icon: None,
                    cursor_hidden_by_typing: false,
                    last_cursor_activity_at_ms: 0,
                },
            },
            portal: crate::protocol::wayland::portal::PortalState::default(),
            runtime: RuntimeState {
                tuning,
                surface_activity: HashMap::new(),
                exit_requested: false,
                started_at: now,
                maintenance_dirty: true,
                maintenance_ping: None,
                pending_drm_syncobj_surfaces: Arc::new(Mutex::new(Vec::new())),
                activation: Default::default(),
                spawned_children: Vec::new(),
            },
        };
        out.ui
            .render_state
            .animator
            .set_spec(crate::animation::AnimSpec {
                state_change_ms: 360,
                bounce: 1.45,
            });
        out.model.spawn_state.per_monitor = out
            .model
            .monitor_state
            .monitors
            .iter()
            .map(|(name, monitor)| {
                (
                    name.clone(),
                    MonitorSpawnState::new(monitor.viewport.center),
                )
            })
            .collect();
        let current_monitor = out.model.monitor_state.current_monitor.clone();
        let _ = out.load_monitor_state(current_monitor.as_str());
        let _ = out.platform.display_handle.create_global::<
            Halley,
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
            _,
        >(3, ());
        out
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(dh: &DisplayHandle, tuning: RuntimeTuning) -> Self {
        let event_loop = Box::leak(Box::new(
            calloop::EventLoop::<Self>::try_new().expect("test event loop"),
        ));
        Self::new(dh, event_loop.handle(), tuning)
    }

    pub(crate) fn focus_ctx(&mut self) -> super::ctx::FocusCtx<'_> {
        super::ctx::focus_ctx(self)
    }

    pub(crate) fn spawn_ctx(&mut self) -> super::ctx::SpawnCtx<'_> {
        super::ctx::spawn_ctx(self)
    }

    pub(crate) fn surface_lifecycle_ctx(&mut self) -> super::ctx::SurfaceLifecycleCtx<'_> {
        super::ctx::surface_lifecycle_ctx(self)
    }

    pub(crate) fn layer_shell_ctx(&mut self) -> super::ctx::LayerShellCtx<'_> {
        super::ctx::layer_shell_ctx(self)
    }

    pub(crate) fn pointer_ctx(&mut self) -> super::ctx::PointerCtx<'_> {
        super::ctx::pointer_ctx(self)
    }

    pub(crate) fn fullscreen_ctx(&mut self) -> super::ctx::FullscreenCtx<'_> {
        super::ctx::fullscreen_ctx(self)
    }

    #[allow(dead_code)]
    pub(crate) fn monitor_ctx(&mut self) -> super::ctx::MonitorCtx<'_> {
        super::ctx::monitor_ctx(self)
    }

    #[allow(dead_code)]
    pub(crate) fn cluster_ctx(&mut self) -> super::ctx::ClusterCtx<'_> {
        super::ctx::cluster_ctx(self)
    }

    #[allow(dead_code)]
    pub(crate) fn carry_ctx(&mut self) -> super::ctx::CarryCtx<'_> {
        super::ctx::carry_ctx(self)
    }

    #[allow(dead_code)]
    pub(crate) fn interaction_ctx(&mut self) -> super::ctx::InteractionCtx<'_> {
        super::ctx::interaction_ctx(self)
    }

    #[allow(dead_code)]
    pub(crate) fn workspace_ctx(&mut self) -> super::ctx::WorkspaceCtx<'_> {
        super::ctx::workspace_ctx(self)
    }

    pub fn mark_active_transition(&mut self, id: NodeId, now: Instant, duration_ms: u64) {
        super::workspace::state::mark_active_transition(self, id, now, duration_ms)
    }

    pub fn active_transition_alpha(&self, id: NodeId, now: Instant) -> f32 {
        super::workspace::state::active_transition_alpha(self, id, now)
    }

    pub(crate) fn preserve_collapsed_surface(&self, id: NodeId) -> bool {
        super::workspace::state::preserve_collapsed_surface(self, id)
    }

    #[allow(dead_code)]
    pub(crate) fn default_spawn_view_anchor_for_monitor(&self, monitor: &str) -> Vec2 {
        super::spawn::state::default_spawn_view_anchor_for_monitor(self, monitor)
    }

    pub(crate) fn spawn_monitor_state(
        &self,
        monitor: &str,
    ) -> super::spawn::state::MonitorSpawnState {
        super::spawn::state::spawn_monitor_state(self, monitor)
    }

    pub(crate) fn spawn_monitor_state_mut(
        &mut self,
        monitor: &str,
    ) -> &mut super::spawn::state::MonitorSpawnState {
        super::spawn::state::spawn_monitor_state_mut(self, monitor)
    }

    pub(crate) fn process_pending_spawn_activations(&mut self, now: Instant, now_ms: u64) {
        super::spawn::state::process_pending_spawn_activations(self, now, now_ms)
    }

    pub(crate) fn camera_view_size(&self) -> Vec2 {
        super::monitor::camera::camera_view_size(self)
    }

    pub(crate) fn pan_camera_target(&mut self, delta: Vec2) {
        super::monitor::camera::pan_camera_target(self, delta)
    }

    #[allow(dead_code)]
    pub(crate) fn set_camera_target_view_size(&mut self, size: Vec2) {
        super::monitor::camera::set_camera_target_view_size(self, size)
    }

    pub(crate) fn snap_camera_targets_to_live(&mut self) {
        super::monitor::camera::snap_camera_targets_to_live(self)
    }

    #[allow(dead_code)]
    pub(crate) fn clamp_camera_view_size(&self, size: Vec2) -> Vec2 {
        super::monitor::camera::clamp_camera_view_size(self, size)
    }

    pub(crate) fn zoom_blocked_by_interaction(&self) -> bool {
        super::monitor::camera::zoom_blocked_by_interaction(self)
    }

    pub(crate) fn update_zoom_live_surface_sizes(&mut self) {
        super::monitor::camera::update_zoom_live_surface_sizes(self)
    }

    pub(crate) fn zoom_by_steps(&mut self, steps: f32) {
        super::monitor::camera::zoom_by_steps(self, steps)
    }

    pub(crate) fn reset_zoom(&mut self) {
        super::monitor::camera::reset_zoom(self)
    }

    pub(crate) fn tick_camera_smoothing(&mut self, now: Instant) {
        super::monitor::camera::tick_camera_smoothing(self, now)
    }

    pub fn active_zoom_lock_scale(&self) -> f32 {
        super::monitor::camera::active_zoom_lock_scale(self)
    }

    pub fn camera_render_scale(&self) -> f32 {
        super::monitor::camera::camera_render_scale(self)
    }

    pub fn view_center_for_monitor(&self, monitor: &str) -> Vec2 {
        super::monitor::state::view_center_for_monitor(self, monitor)
    }

    pub fn usable_viewport_for_monitor(&self, monitor: &str) -> Viewport {
        super::monitor::state::usable_viewport_for_monitor(self, monitor)
    }

    pub(crate) fn load_monitor_state(&mut self, name: &str) -> bool {
        super::monitor::state::load_monitor_state(self, name)
    }

    pub(crate) fn sync_current_monitor_state(&mut self) {
        super::monitor::state::sync_current_monitor_state(self)
    }

    pub(crate) fn activate_monitor(&mut self, name: &str) -> bool {
        super::monitor::state::activate_monitor(self, name)
    }

    pub(crate) fn begin_temporary_render_monitor(&mut self, name: &str) -> Option<String> {
        super::monitor::state::begin_temporary_render_monitor(self, name)
    }

    pub(crate) fn end_temporary_render_monitor(&mut self, previous: Option<String>) {
        super::monitor::state::end_temporary_render_monitor(self, previous)
    }

    pub(crate) fn interaction_monitor(&self) -> &str {
        super::monitor::state::interaction_monitor(self)
    }

    pub(crate) fn focused_monitor(&self) -> &str {
        super::monitor::state::focused_monitor(self)
    }

    pub(crate) fn set_interaction_monitor(&mut self, name: &str) {
        super::monitor::state::set_interaction_monitor(self, name)
    }

    pub(crate) fn set_focused_monitor(&mut self, name: &str) {
        super::monitor::state::set_focused_monitor(self, name)
    }

    pub(crate) fn show_exit_confirm_overlay(&mut self) {
        let mut monitors: Vec<String> = self.model.monitor_state.monitors.keys().cloned().collect();
        if monitors.is_empty() {
            monitors.push(self.model.monitor_state.current_monitor.clone());
        }
        for monitor in monitors {
            self.ui.render_state.show_exit_confirm(monitor.as_str());
        }
    }

    pub(crate) fn clear_exit_confirm_overlay(&mut self) {
        let mut monitors: Vec<String> = self
            .ui
            .render_state
            .overlay_exit_confirm
            .keys()
            .cloned()
            .collect();
        if monitors.is_empty() {
            monitors.push(self.model.monitor_state.current_monitor.clone());
        }
        for monitor in monitors {
            self.ui.render_state.clear_exit_confirm(monitor.as_str());
        }
    }

    pub(crate) fn exit_confirm_active(&self) -> bool {
        self.ui.render_state.exit_confirm_visible()
    }

    pub(crate) fn reconfigure_active_tty_monitors(&mut self, active_outputs: &[String]) {
        super::monitor::state::reconfigure_active_tty_monitors(self, active_outputs)
    }

    pub(crate) fn monitor_for_screen(&self, sx: f32, sy: f32) -> Option<String> {
        super::monitor::state::monitor_for_screen(self, sx, sy)
    }

    pub(crate) fn local_screen_in_monitor(
        &self,
        name: &str,
        sx: f32,
        sy: f32,
    ) -> (i32, i32, f32, f32) {
        super::monitor::state::local_screen_in_monitor(self, name, sx, sy)
    }

    pub(crate) fn node_visible_on_current_monitor(&self, id: NodeId) -> bool {
        super::monitor::state::node_visible_on_current_monitor(self, id)
    }

    pub(crate) fn assign_node_to_current_monitor(&mut self, id: NodeId) {
        super::monitor::state::assign_node_to_current_monitor(self, id)
    }

    pub(crate) fn assign_node_to_monitor(&mut self, id: NodeId, monitor: &str) {
        super::monitor::state::assign_node_to_monitor(self, id, monitor)
    }

    pub(crate) fn assign_layer_surface_to_monitor(
        &mut self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        monitor: String,
    ) {
        super::monitor::state::assign_layer_surface_to_monitor(self, surface, monitor)
    }

    pub(crate) fn output_transform_for(&self, name: &str) -> smithay::utils::Transform {
        super::monitor::state::output_transform_for(self, name)
    }

    pub(crate) fn advertise_output(&mut self, name: &str, mode: smithay::output::Mode) {
        super::monitor::state::advertise_output(self, name, mode)
    }

    pub(crate) fn preferred_xdg_decoration_mode(
        &self,
    ) -> smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode{
        super::platform::preferred_xdg_decoration_mode(self)
    }

    pub(crate) fn apply_toplevel_tiled_hint(
        &self,
        state: &mut smithay::wayland::shell::xdg::ToplevelState,
    ) {
        super::platform::apply_toplevel_tiled_hint(self, state)
    }

    pub(crate) fn refresh_xdg_decoration_mode(&mut self) {
        super::platform::refresh_xdg_decoration_mode(self)
    }

    pub(crate) fn effective_cursor_image_status(
        &self,
    ) -> smithay::input::pointer::CursorImageStatus {
        super::platform::effective_cursor_image_status(self)
    }

    pub(crate) fn install_drm_syncobj_blocker(
        &mut self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        super::platform::install_drm_syncobj_blocker(self, surface)
    }

    pub(crate) fn drain_drm_syncobj_blockers(&mut self) {
        super::platform::drain_drm_syncobj_blockers(self)
    }

    pub(crate) fn configure_dmabuf_importer(
        &mut self,
        importer: std::rc::Rc<dyn crate::backend::interface::DmabufImportBackend>,
        main_device: Option<libc::dev_t>,
    ) {
        super::platform::configure_dmabuf_importer(self, importer, main_device)
    }

    pub(crate) fn configure_dmabuf_importer_for_fd<Fd: std::os::unix::io::AsFd>(
        &mut self,
        importer: std::rc::Rc<dyn crate::backend::interface::DmabufImportBackend>,
        device_fd: Fd,
    ) {
        super::platform::configure_dmabuf_importer_for_fd(self, importer, device_fd)
    }

    pub fn note_input_activity(&mut self) {
        super::platform::note_input_activity(self)
    }

    pub(crate) fn non_overlap_gap_world(&self) -> f32 {
        super::overlap::system::non_overlap_gap_world(self)
    }

    pub(crate) fn required_sep_x(
        &self,
        a_pos_x: f32,
        a_ext: super::overlap::system::CollisionExtents,
        b_pos_x: f32,
        b_ext: super::overlap::system::CollisionExtents,
        gap: f32,
    ) -> f32 {
        super::overlap::system::required_sep_x(self, a_pos_x, a_ext, b_pos_x, b_ext, gap)
    }

    pub(crate) fn required_sep_y(
        &self,
        a_pos_y: f32,
        a_ext: super::overlap::system::CollisionExtents,
        b_pos_y: f32,
        b_ext: super::overlap::system::CollisionExtents,
        gap: f32,
    ) -> f32 {
        super::overlap::system::required_sep_y(self, a_pos_y, a_ext, b_pos_y, b_ext, gap)
    }

    pub(crate) fn carry_surface_non_overlap(
        &mut self,
        id: NodeId,
        to: Vec2,
        clamp_only: bool,
    ) -> bool {
        super::overlap::system::carry_surface_non_overlap(self, id, to, clamp_only)
    }

    pub(crate) fn surface_window_collision_extents(
        &self,
        n: &halley_core::field::Node,
    ) -> super::overlap::system::CollisionExtents {
        super::overlap::system::surface_window_collision_extents(self, n)
    }

    pub(crate) fn spawn_obstacle_extents_for_node(
        &self,
        n: &halley_core::field::Node,
    ) -> super::overlap::system::CollisionExtents {
        super::overlap::system::spawn_obstacle_extents_for_node(self, n)
    }

    pub(crate) fn collision_extents_for_node(
        &self,
        n: &halley_core::field::Node,
    ) -> super::overlap::system::CollisionExtents {
        super::overlap::system::collision_extents_for_node(self, n)
    }

    pub(crate) fn collision_size_for_node(&self, n: &halley_core::field::Node) -> Vec2 {
        super::overlap::system::collision_size_for_node(self, n)
    }

    pub(crate) fn resolve_surface_overlap(&mut self) {
        super::overlap::system::resolve_surface_overlap(self)
    }

    pub(crate) fn request_toplevel_resize(&mut self, node_id: NodeId, width: i32, height: i32) {
        super::overlap::system::request_toplevel_resize(self, node_id, width, height)
    }

    pub(crate) fn node_has_overlap_policy(&self, id: NodeId) -> bool {
        if matches!(
            self.runtime.tuning.cluster_layout_kind(),
            halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Stacking
        ) {
            return false;
        }
        self.model
            .spawn_state
            .applied_window_rules
            .get(&id)
            .is_some_and(|rule| {
                rule.overlap_policy != halley_config::InitialWindowOverlapPolicy::None
            })
    }

    pub fn now_ms(&self, now: Instant) -> u64 {
        super::runtime::runtime_controller(self).now_ms(now)
    }

    #[allow(dead_code)]
    pub(crate) fn debug_dump(&self) {
        super::runtime::runtime_controller(self).debug_dump()
    }

    pub fn apply_tuning(&mut self, tuning: RuntimeTuning) {
        super::runtime::runtime_controller(self).apply_tuning(tuning)
    }

    pub fn request_exit(&mut self) {
        super::runtime::runtime_controller(self).request_exit()
    }

    pub fn exit_requested(&self) -> bool {
        super::runtime::runtime_controller(self).exit_requested()
    }

    pub fn request_maintenance(&mut self) {
        super::runtime::runtime_controller(self).request_maintenance()
    }

    #[allow(dead_code)]
    pub fn next_maintenance_deadline(&self, now: Instant) -> Option<Instant> {
        super::runtime::runtime_controller(self).next_maintenance_deadline(now)
    }

    pub fn run_maintenance_if_needed(&mut self, now: Instant) {
        super::runtime::runtime_controller(self).run_maintenance_if_needed(now)
    }

    #[allow(dead_code)]
    pub fn run_maintenance(&mut self, now: Instant) {
        super::runtime::runtime_controller(self).run_maintenance(now)
    }

    pub(crate) fn record_focus_trail_visit(&mut self, id: NodeId) {
        super::focus::trail::focus_trail_controller(self).record_focus_trail_visit(id)
    }

    #[cfg(test)]
    pub(crate) fn trail_for_monitor_mut(
        &mut self,
        monitor: &str,
    ) -> &mut halley_core::trail::Trail {
        super::focus::trail::trail_for_monitor_mut(self, monitor)
    }

    pub(crate) fn navigate_window_trail(
        &mut self,
        direction: halley_ipc::TrailDirection,
        now: Instant,
    ) -> bool {
        super::focus::trail::focus_trail_controller(self).navigate_window_trail(direction, now)
    }

    pub(crate) fn previous_window_from_trail_on_close(
        &mut self,
        monitor: &str,
        closing_id: NodeId,
    ) -> Option<NodeId> {
        super::focus::trail::focus_trail_controller(self)
            .previous_window_from_trail_on_close(monitor, closing_id)
    }

    pub(crate) fn restore_focus_to_node_after_close(
        &mut self,
        monitor: &str,
        id: NodeId,
        now: Instant,
        suppress_pan: bool,
    ) -> bool {
        super::focus::trail::focus_trail_controller(self).restore_focus_to_node_after_close(
            monitor,
            id,
            now,
            suppress_pan,
        )
    }

    pub(crate) fn enforce_single_primary_active_unit(&mut self) {
        super::focus::decay::focus_decay_controller(self).enforce_single_primary_active_unit()
    }

    #[cfg(test)]
    pub(crate) fn surface_is_definitively_outside_focus_ring(&self, id: NodeId) -> bool {
        super::focus::decay::focus_decay_controller(self)
            .surface_is_definitively_outside_focus_ring(id)
    }

    pub fn apply_single_surface_decay_policy(
        &mut self,
        id: NodeId,
        now_ms: u64,
        active_delay_ms: u64,
        inactive_delay_ms: u64,
    ) {
        super::focus::decay::focus_decay_controller(self).apply_single_surface_decay_policy(
            id,
            now_ms,
            active_delay_ms,
            inactive_delay_ms,
        )
    }

    pub(crate) fn companion_surface_node(&self, now_ms: u64) -> Option<NodeId> {
        super::focus::state::focus_state_controller(self).companion_surface_node(now_ms)
    }

    pub fn active_focus_ring(&self) -> halley_core::viewport::FocusRing {
        super::focus::state::focus_state_controller(self).active_focus_ring()
    }

    pub fn focus_ring_for_monitor(&self, monitor: &str) -> halley_core::viewport::FocusRing {
        super::focus::state::focus_state_controller(self).focus_ring_for_monitor(monitor)
    }

    pub fn should_draw_focus_ring_preview(&self, now: Instant) -> bool {
        super::focus::state::focus_state_controller(self).should_draw_focus_ring_preview(now)
    }

    pub(crate) fn focus_monitor_view(&mut self, monitor: &str, now: Instant) {
        super::focus::state::focus_state_controller(self).focus_monitor_view(monitor, now)
    }

    pub fn set_interaction_focus(&mut self, id: Option<NodeId>, hold_ms: u64, now: Instant) {
        super::focus::state::focus_state_controller(self).set_interaction_focus(id, hold_ms, now)
    }

    pub(crate) fn restore_pan_return_active_focus(&mut self, now: Instant) {
        super::focus::state::focus_state_controller(self).restore_pan_return_active_focus(now)
    }

    #[allow(dead_code)]
    pub fn reassert_wayland_keyboard_focus_if_drifted(&mut self, id: Option<NodeId>) {
        super::focus::state::focus_state_controller(self)
            .reassert_wayland_keyboard_focus_if_drifted(id)
    }

    #[allow(dead_code)]
    pub(crate) fn focused_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        super::focus::state::focus_state_controller(self).focused_node_for_monitor(monitor)
    }

    #[allow(dead_code)]
    pub(crate) fn focused_monitor_for_node(&self, id: NodeId) -> Option<String> {
        super::focus::state::focus_state_controller(self).focused_monitor_for_node(id)
    }

    #[allow(dead_code)]
    pub(crate) fn set_monitor_focus(&mut self, monitor: &str, id: NodeId) {
        super::focus::state::focus_state_controller(self).set_monitor_focus(monitor, id)
    }

    pub fn set_recent_top_node(&mut self, node_id: NodeId, until: Instant) {
        super::focus::state::focus_state_controller(self).set_recent_top_node(node_id, until)
    }

    pub fn recent_top_node_active(&mut self, now: Instant) -> Option<NodeId> {
        super::focus::state::focus_state_controller(self).recent_top_node_active(now)
    }

    pub fn set_app_focused(&mut self, focused: bool) {
        super::focus::system::focus_system_controller(self).set_app_focused(focused)
    }

    pub(crate) fn clear_keyboard_focus(&mut self) {
        super::focus::system::focus_system_controller(self).clear_keyboard_focus()
    }

    pub fn wl_surface_for_node(
        &self,
        id: NodeId,
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
        super::focus::system::wl_surface_for_node(self, id)
    }

    #[cfg(test)]
    pub(crate) fn fullscreen_focus_override(&self, requested: Option<NodeId>) -> Option<NodeId> {
        super::focus::system::fullscreen_focus_override(self, requested)
    }

    pub(crate) fn update_selection_focus_from_surface(
        &self,
        surface: Option<&smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>,
    ) {
        super::focus::system::update_selection_focus_from_surface(self, surface)
    }

    pub fn apply_wayland_focus_state(&mut self, id: Option<NodeId>) {
        super::focus::system::focus_system_controller(self).apply_wayland_focus_state(id)
    }

    pub fn update_focus_tracking_for_surface(&mut self, fid: NodeId, now_ms: u64) {
        super::focus::system::focus_system_controller(self)
            .update_focus_tracking_for_surface(fid, now_ms)
    }

    pub fn note_pan_activity(&mut self, now: Instant) {
        super::focus::system::focus_system_controller(self).note_pan_activity(now)
    }

    pub(crate) fn note_pan_viewport_change(&mut self, now: Instant) {
        super::focus::system::focus_system_controller(self).note_pan_viewport_change(now)
    }

    pub fn set_pan_restore_focus_target(&mut self, id: NodeId) {
        super::focus::system::focus_system_controller(self).set_pan_restore_focus_target(id)
    }

    pub fn animate_viewport_center_to(&mut self, target_center: Vec2, now: Instant) -> bool {
        super::focus::system::focus_system_controller(self)
            .animate_viewport_center_to(target_center, now)
    }

    pub fn animate_viewport_center_to_delayed(
        &mut self,
        target_center: Vec2,
        now: Instant,
        delay_ms: u64,
    ) -> bool {
        super::focus::system::focus_system_controller(self).animate_viewport_center_to_delayed(
            target_center,
            now,
            delay_ms,
        )
    }

    pub(crate) fn tick_viewport_pan_animation(&mut self, now_ms: u64) {
        super::focus::system::focus_system_controller(self).tick_viewport_pan_animation(now_ms)
    }

    pub(crate) fn surface_is_fully_visible_on_monitor(&self, monitor: &str, id: NodeId) -> bool {
        super::focus::system::surface_is_fully_visible_on_monitor(self, monitor, id)
    }

    pub(crate) fn minimal_reveal_center_for_surface_on_monitor(
        &self,
        monitor: &str,
        id: NodeId,
    ) -> Option<Vec2> {
        super::focus::system::minimal_reveal_center_for_surface_on_monitor(self, monitor, id)
    }

    pub(crate) fn maybe_pan_to_restored_focus_on_close(
        &mut self,
        monitor: &str,
        id: NodeId,
        now: Instant,
    ) -> bool {
        super::focus::system::focus_system_controller(self)
            .maybe_pan_to_restored_focus_on_close(monitor, id, now)
    }

    pub fn begin_resize_interaction(&mut self, id: NodeId, now: Instant) {
        super::focus::system::focus_system_controller(self).begin_resize_interaction(id, now)
    }

    pub fn end_resize_interaction(&mut self, now: Instant) {
        super::focus::system::focus_system_controller(self).end_resize_interaction(now)
    }

    pub fn resolve_overlap_now(&mut self) {
        super::focus::system::focus_system_controller(self).resolve_overlap_now()
    }

    pub fn set_last_active_size_now(&mut self, id: NodeId, size: Vec2) {
        super::focus::system::focus_system_controller(self).set_last_active_size_now(id, size)
    }

    pub fn last_focused_surface_node(&self) -> Option<NodeId> {
        super::focus::system::last_focused_surface_node(self)
    }

    pub fn last_focused_surface_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        super::focus::system::last_focused_surface_node_for_monitor(self, monitor)
    }

    pub fn last_input_surface_node(&self) -> Option<NodeId> {
        super::focus::system::last_input_surface_node(self)
    }

    pub fn last_input_surface_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        super::focus::system::last_input_surface_node_for_monitor(self, monitor)
    }

    pub(crate) fn fullscreen_entry_scale(&self, node_id: NodeId, now_ms: u64) -> f32 {
        super::fullscreen::system::fullscreen_entry_scale(self, node_id, now_ms)
    }

    pub(crate) fn fullscreen_monitor_for_node(&self, node_id: NodeId) -> Option<&str> {
        super::fullscreen::system::fullscreen_monitor_for_node(self, node_id)
    }

    pub(crate) fn is_fullscreen_active(&self, node_id: NodeId) -> bool {
        super::fullscreen::system::is_fullscreen_active(self, node_id)
    }

    pub(crate) fn fullscreen_target_size_for(&self, monitor_name: &str) -> (i32, i32) {
        self.model
            .monitor_state
            .outputs
            .get(monitor_name)
            .and_then(|output| output.current_mode())
            .map(|mode| (mode.size.w, mode.size.h))
            .unwrap_or_else(|| {
                let space = self.model.monitor_state.monitors.get(monitor_name);
                let size = space
                    .map(|m| m.viewport.size)
                    .unwrap_or(self.model.viewport.size);
                (
                    size.x.round().max(96.0) as i32,
                    size.y.round().max(72.0) as i32,
                )
            })
    }

    pub(crate) fn suspend_xdg_fullscreen(&mut self, node_id: NodeId, now: Instant) {
        super::fullscreen::system::fullscreen_controller(self).suspend_xdg_fullscreen(node_id, now)
    }

    pub(crate) fn enter_xdg_fullscreen(
        &mut self,
        node_id: NodeId,
        output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
        now: Instant,
    ) {
        super::fullscreen::system::fullscreen_controller(self)
            .enter_xdg_fullscreen(node_id, output, now)
    }

    pub(crate) fn exit_xdg_fullscreen(&mut self, node_id: NodeId, now: Instant) {
        super::fullscreen::system::fullscreen_controller(self).exit_xdg_fullscreen(node_id, now)
    }

    pub(crate) fn drop_fullscreen_surface(&mut self, id: NodeId, now: Instant) {
        super::fullscreen::system::fullscreen_controller(self).drop_fullscreen_surface(id, now)
    }

    pub(crate) fn tick_fullscreen_motion(&mut self, now: Instant) {
        super::fullscreen::system::fullscreen_controller(self).tick_fullscreen_motion(now)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn viewport_center_for_monitor(&self, monitor: &str) -> Vec2 {
        super::spawn::reveal::spawn_reveal_controller(self).viewport_center_for_monitor(monitor)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn resolve_spawn_target_monitor(&self) -> String {
        super::spawn::reveal::spawn_reveal_controller(self).resolve_spawn_target_monitor()
    }

    #[cfg(test)]
    pub(crate) fn current_spawn_focus(&self, monitor: &str) -> (Option<NodeId>, Vec2) {
        super::spawn::reveal::spawn_reveal_controller(self).current_spawn_focus(monitor)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn viewport_fully_contains_surface_on_monitor(
        &self,
        monitor: &str,
        id: NodeId,
    ) -> bool {
        super::spawn::reveal::spawn_reveal_controller(self)
            .viewport_fully_contains_surface_on_monitor(monitor, id)
    }

    #[cfg(test)]
    pub(crate) fn right_spawn_candidate_for_focus(&self, id: NodeId, size: Vec2) -> Option<Vec2> {
        super::spawn::reveal::spawn_reveal_controller(self)
            .right_spawn_candidate_for_focus(id, size)
    }

    #[cfg(test)]
    pub(crate) fn spawn_star_step(&self, size: Vec2) -> f32 {
        super::spawn::reveal::spawn_reveal_controller(self).spawn_star_step(size)
    }

    #[cfg(test)]
    pub(crate) fn star_candidate_offsets(&self, size: Vec2) -> Vec<Vec2> {
        super::spawn::reveal::spawn_reveal_controller(self).star_candidate_offsets(size)
    }

    #[cfg(test)]
    pub(crate) fn spawn_star_step_x(&self, size: Vec2) -> f32 {
        super::spawn::reveal::spawn_reveal_controller(self).spawn_star_step_x(size)
    }

    #[cfg(test)]
    pub(crate) fn spawn_star_step_y(&self, size: Vec2) -> f32 {
        super::spawn::reveal::spawn_reveal_controller(self).spawn_star_step_y(size)
    }

    #[cfg(test)]
    pub(crate) fn spawn_candidate_for_focus_dir(
        &self,
        id: NodeId,
        size: Vec2,
        dir: Vec2,
    ) -> Option<Vec2> {
        super::spawn::reveal::spawn_reveal_controller(self)
            .spawn_candidate_for_focus_dir(id, size, dir)
    }

    #[cfg(test)]
    pub(crate) fn update_spawn_patch(
        &mut self,
        monitor: &str,
        anchor: Vec2,
        focus_node: Option<NodeId>,
        focus_pos: Vec2,
        growth_dir: Vec2,
    ) {
        super::spawn::reveal::spawn_reveal_controller(self)
            .update_spawn_patch(monitor, anchor, focus_node, focus_pos, growth_dir)
    }

    #[allow(dead_code)]
    pub(crate) fn pick_spawn_position(&mut self, size: Vec2) -> (String, Vec2, bool) {
        super::spawn::reveal::spawn_reveal_controller(self).pick_spawn_position(size)
    }

    pub(crate) fn spawn_target_monitor_for_intent(
        &self,
        intent: &super::spawn::rules::InitialWindowIntent,
    ) -> String {
        super::spawn::reveal::spawn_reveal_controller(self).spawn_target_monitor_for_intent(intent)
    }

    pub(crate) fn pick_spawn_position_with_intent(
        &mut self,
        size: Vec2,
        intent: &super::spawn::rules::InitialWindowIntent,
    ) -> (String, Vec2, bool) {
        super::spawn::reveal::spawn_reveal_controller(self)
            .pick_spawn_position_with_intent(size, intent)
    }

    #[allow(dead_code)]
    pub(crate) fn maybe_start_pending_spawn_pan(&mut self, now: Instant) {
        super::spawn::reveal::spawn_reveal_controller(self).maybe_start_pending_spawn_pan(now)
    }

    pub(crate) fn tick_pending_spawn_pan(&mut self, now: Instant, now_ms: u64) {
        super::spawn::reveal::spawn_reveal_controller(self).tick_pending_spawn_pan(now, now_ms)
    }

    pub(crate) fn reveal_new_toplevel_node(
        &mut self,
        id: NodeId,
        is_transient: bool,
        now: Instant,
    ) {
        super::spawn::reveal::spawn_reveal_controller(self).reveal_new_toplevel_node(
            id,
            is_transient,
            now,
        )
    }

    pub(crate) fn remove_node_from_field(&mut self, id: NodeId, now_ms: u64) -> bool {
        super::clusters::system::cluster_system_controller(self).remove_node_from_field(id, now_ms)
    }

    pub fn cluster_bloom_for_monitor(
        &mut self,
        monitor: &str,
    ) -> Option<halley_core::cluster::ClusterId> {
        super::clusters::system::cluster_system_controller(self).cluster_bloom_for_monitor(monitor)
    }

    #[cfg(test)]
    pub(crate) fn sync_cluster_monitor(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        preferred: Option<&str>,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .sync_cluster_monitor(cid, preferred)
    }

    #[cfg(test)]
    pub(crate) fn enter_cluster_workspace_by_core(
        &mut self,
        core_id: NodeId,
        monitor: &str,
        now: Instant,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .enter_cluster_workspace_by_core(core_id, monitor, now)
    }

    #[cfg(test)]
    pub(crate) fn exit_cluster_workspace_for_monitor(
        &mut self,
        monitor: &str,
        now: Instant,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .exit_cluster_workspace_for_monitor(monitor, now)
    }

    pub fn open_cluster_bloom_for_monitor(
        &mut self,
        monitor: &str,
        cid: halley_core::cluster::ClusterId,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .open_cluster_bloom_for_monitor(monitor, cid)
    }

    pub fn close_cluster_bloom_for_monitor(&mut self, monitor: &str) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .close_cluster_bloom_for_monitor(monitor)
    }

    pub fn detach_member_from_cluster(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        member_id: NodeId,
        world_pos: Vec2,
        now: Instant,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .detach_member_from_cluster(cid, member_id, world_pos, now)
    }

    #[allow(dead_code)]
    pub fn absorb_node_into_cluster(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        node_id: NodeId,
        now: Instant,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .absorb_node_into_cluster(cid, node_id, now)
    }

    pub(crate) fn commit_ready_cluster_join_for_node(
        &mut self,
        node_id: NodeId,
        now: Instant,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .commit_ready_cluster_join_for_node(node_id, now)
    }

    pub fn active_cluster_workspace_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<halley_core::cluster::ClusterId> {
        super::clusters::system::active_cluster_workspace_for_monitor(self, monitor)
    }

    pub(crate) fn stack_layout_rects_for_members(
        &self,
        monitor: &str,
        members: &[NodeId],
    ) -> Option<std::collections::HashMap<NodeId, halley_core::tiling::Rect>> {
        super::clusters::system::stack_layout_rects_for_members(self, monitor, members)
    }

    pub(crate) fn reveal_cluster_overflow_for_monitor(&mut self, monitor: &str, now_ms: u64) {
        super::clusters::system::cluster_system_controller(self)
            .reveal_cluster_overflow_for_monitor(monitor, now_ms)
    }

    pub(crate) fn hide_cluster_overflow_for_monitor(&mut self, monitor: &str) {
        super::clusters::system::cluster_system_controller(self)
            .hide_cluster_overflow_for_monitor(monitor)
    }

    pub(crate) fn cluster_overflow_rect_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<halley_core::tiling::Rect> {
        super::clusters::system::cluster_system_controller(self)
            .cluster_overflow_rect_for_monitor(monitor)
    }

    pub(crate) fn cluster_overflow_slot_rect_for_monitor(
        &self,
        monitor: &str,
        overflow_len: usize,
        slot_index: usize,
    ) -> Option<halley_core::tiling::Rect> {
        super::clusters::system::cluster_system_controller(self)
            .cluster_overflow_slot_rect_for_monitor(monitor, overflow_len, slot_index)
    }

    pub(crate) fn active_cluster_tile_rect_for_member(
        &self,
        monitor: &str,
        member_id: NodeId,
    ) -> Option<halley_core::tiling::Rect> {
        super::clusters::system::cluster_system_controller(self)
            .active_cluster_tile_rect_for_member(monitor, member_id)
    }

    pub(crate) fn adjust_cluster_overflow_scroll_for_monitor(
        &mut self,
        monitor: &str,
        delta: i32,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .adjust_cluster_overflow_scroll_for_monitor(monitor, delta)
    }

    pub(crate) fn cluster_spawn_rect_for_new_member(
        &self,
        monitor: &str,
        cid: halley_core::cluster::ClusterId,
    ) -> Option<halley_core::tiling::Rect> {
        super::clusters::system::cluster_system_controller(self)
            .cluster_spawn_rect_for_new_member(monitor, cid)
    }

    pub fn has_any_active_cluster_workspace(&self) -> bool {
        super::clusters::system::cluster_system_controller(self).has_any_active_cluster_workspace()
    }

    pub(crate) fn swap_cluster_overflow_member_with_visible(
        &mut self,
        monitor: &str,
        cid: halley_core::cluster::ClusterId,
        overflow_member: NodeId,
        visible_member: NodeId,
        now_ms: u64,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .swap_cluster_overflow_member_with_visible(
                monitor,
                cid,
                overflow_member,
                visible_member,
                now_ms,
            )
    }

    pub(crate) fn reorder_cluster_overflow_member(
        &mut self,
        monitor: &str,
        cid: halley_core::cluster::ClusterId,
        member: NodeId,
        target_overflow_index: usize,
        now_ms: u64,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self).reorder_cluster_overflow_member(
            monitor,
            cid,
            member,
            target_overflow_index,
            now_ms,
        )
    }

    pub(crate) fn move_active_cluster_member_to_drop_tile(
        &mut self,
        monitor: &str,
        member: NodeId,
        world_pos: Vec2,
        now_ms: u64,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .move_active_cluster_member_to_drop_tile(monitor, member, world_pos, now_ms)
    }

    pub(crate) fn cycle_active_stack_for_monitor(
        &mut self,
        monitor: &str,
        direction: halley_core::cluster_layout::ClusterCycleDirection,
        now: Instant,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .cycle_active_stack_for_monitor(monitor, direction, now)
    }

    pub fn collapse_active_cluster_workspace(&mut self, now: Instant) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .collapse_active_cluster_workspace(now)
    }

    pub fn cluster_mode_active(&self) -> bool {
        super::clusters::system::cluster_system_controller(self).cluster_mode_active()
    }

    pub fn cluster_mode_active_for_monitor(&self, monitor: &str) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .cluster_mode_active_for_monitor(monitor)
    }

    pub fn enter_cluster_mode(&mut self) -> bool {
        super::clusters::system::cluster_system_controller(self).enter_cluster_mode()
    }

    pub fn exit_cluster_mode(&mut self) -> bool {
        super::clusters::system::cluster_system_controller(self).exit_cluster_mode()
    }

    pub fn toggle_cluster_mode_selection(&mut self, node_id: NodeId) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .toggle_cluster_mode_selection(node_id)
    }

    pub fn confirm_cluster_mode(&mut self, now: Instant) -> bool {
        super::clusters::system::cluster_system_controller(self).confirm_cluster_mode(now)
    }

    pub fn toggle_cluster_workspace_by_core(&mut self, core_id: NodeId, now: Instant) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .toggle_cluster_workspace_by_core(core_id, now)
    }

    pub fn has_active_cluster_workspace(&self) -> bool {
        super::clusters::system::cluster_system_controller(self).has_active_cluster_workspace()
    }

    pub(crate) fn layout_active_cluster_workspace_for_monitor(
        &mut self,
        monitor: &str,
        now_ms: u64,
    ) {
        super::clusters::system::cluster_system_controller(self)
            .layout_active_cluster_workspace_for_monitor(monitor, now_ms)
    }

    pub(crate) fn focus_active_tiled_cluster_member_for_monitor(
        &mut self,
        monitor: &str,
        preferred_index: Option<usize>,
        now: Instant,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .focus_active_tiled_cluster_member_for_monitor(monitor, preferred_index, now)
    }

    pub(crate) fn tile_focus_active_cluster_member_for_monitor(
        &mut self,
        monitor: &str,
        direction: halley_config::DirectionalAction,
        now: Instant,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .tile_focus_active_cluster_member_for_monitor(monitor, direction, now)
    }

    pub(crate) fn tile_swap_active_cluster_member_for_monitor(
        &mut self,
        monitor: &str,
        direction: halley_config::DirectionalAction,
        now: Instant,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .tile_swap_active_cluster_member_for_monitor(monitor, direction, now)
    }

    pub(crate) fn cycle_active_cluster_layout_for_monitor(
        &mut self,
        monitor: &str,
        now: Instant,
    ) -> bool {
        super::clusters::system::cluster_system_controller(self)
            .cycle_active_cluster_layout_for_monitor(monitor, now)
    }
}

impl Drop for Halley {
    fn drop(&mut self) {
        for child in &mut self.runtime.spawned_children {
            let pgid = child.id() as i32;
            unsafe {
                libc::kill(-pgid, libc::SIGTERM);
            }
            let _ = child.wait();
        }
    }
}

delegate_dmabuf!(Halley);
