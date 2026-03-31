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
                xdg_decoration_state: XdgDecorationState::new::<Halley>(dh),
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
                    cluster_overflow_visible_until_ms: HashMap::new(),
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
                    pending_spawn_monitor: None,
                    per_monitor: HashMap::new(),
                    pending_spawn_pan_queue: VecDeque::new(),
                    active_spawn_pan: None,
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
                    cluster_bloom_mix: HashMap::new(),
                    overlay_banner: HashMap::new(),
                    overlay_toast: HashMap::new(),
                    node_circle_texture: None,
                    node_circle_program: None,
                    node_squircle_program: None,
                    node_label_program: None,
                    node_label_program_failed: false,
                    window_texture_program: None,
                    window_texture_program_failed: false,
                    surface_clip_program: None,
                    surface_clip_program_failed: false,
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
                },
            },
            runtime: RuntimeState {
                tuning,
                surface_activity: HashMap::new(),
                exit_requested: false,
                started_at: now,
                last_debug_dump_at: now,
                maintenance_dirty: true,
                maintenance_ping: None,
                pending_drm_syncobj_surfaces: Arc::new(Mutex::new(Vec::new())),
                spawned_children: Vec::new(),
            },
        };
        out.ui
            .render_state
            .animator
            .set_spec(crate::animation::AnimSpec {
                state_change_ms: out.runtime.tuning.dev_anim_state_change_ms,
                bounce: out.runtime.tuning.dev_anim_bounce,
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
