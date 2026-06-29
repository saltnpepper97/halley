use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use calloop::LoopHandle;
use halley_config::RuntimeTuning;
use halley_core::cluster::ClusterId;
use halley_core::cluster_policy::ClusterFormationState;
use halley_core::field::{Field, NodeId, Vec2};
use halley_core::viewport::Viewport;
use smithay::utils::{Logical, Point};
use smithay::{
    delegate_dmabuf,
    desktop::PopupManager,
    input::SeatState,
    reexports::wayland_server::{DisplayHandle, backend::ObjectId},
    wayland::{
        background_effect::BackgroundEffectState,
        compositor::CompositorState,
        cursor_shape::CursorShapeManagerState,
        dmabuf::DmabufState,
        fractional_scale::FractionalScaleManagerState,
        idle_notify::IdleNotifierState,
        output::OutputManagerState,
        pointer_constraints::PointerConstraintsState,
        pointer_gestures::PointerGesturesState,
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
    /// Inertial zoom velocity in log(view-size) units per second. Repeated zoom
    /// input accumulates this (accelerating ramp); friction decays it to a stop.
    pub(crate) zoom_log_vel: f32,
    /// Inertial pan velocity in world units per second. A flick at the end of a
    /// pan gesture seeds this; friction decays it to a stop (see `camera.rs`).
    pub(crate) pan_vel: Vec2,
    pub(crate) camera_target_center: Vec2,
    pub(crate) camera_target_view_size: Vec2,
    pub(crate) surface_to_node: HashMap<ObjectId, NodeId>,
    pub(crate) node_app_ids: HashMap<NodeId, String>,
    /// For window-parented popups that should render pinned to the screen (e.g.
    /// Steam's install-complete notification), the frozen configure-time anchor
    /// `target.loc` (= `-(parent_tl - viewport_tl)` against the fixed monitor
    /// frame), keyed by the popup surface. Lets the render path reproject the
    /// popup onto the monitor output immune to camera zoom/pan. See
    /// `configure_popup_position`.
    pub(crate) pinned_popup_anchor: HashMap<ObjectId, Point<i32, Logical>>,
}

pub(crate) struct UiState {
    pub(crate) render_state: RenderState,
}

pub(crate) struct InputState {
    pub(crate) interaction_state: InteractionState,
    /// Live libinput devices, tracked so configured settings can be re-applied on reload.
    /// Empty under the winit/nested backend, which has no libinput devices.
    pub(crate) devices: Vec<smithay::reexports::input::Device>,
}

pub struct Halley {
    pub(crate) platform: PlatformState,
    pub(crate) model: ModelState,
    pub(crate) ui: UiState,
    pub(crate) aperture: crate::aperture::ApertureState,
    pub(crate) input: InputState,
    pub(crate) portal: crate::protocol::wayland::portal::PortalState,
    pub(crate) screencast: crate::portal::ScreencastState,
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
        #[cfg(feature = "aperture")]
        let initial_aperture_config = crate::aperture::core::ApertureConfig::default();
        #[cfg(not(feature = "aperture"))]
        let initial_aperture_config = crate::aperture::core::ApertureConfig;
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
                    scale: 1.0,
                    viewport: view,
                    usable_viewport: view,
                    zoom_ref_size: view.size,
                    zoom_log_vel: 0.0,
                    pan_vel: Vec2 { x: 0.0, y: 0.0 },
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
                    scale: 1.0,
                    viewport: view,
                    usable_viewport: view,
                    zoom_ref_size: tuning.viewport_size,
                    zoom_log_vel: 0.0,
                    pan_vel: Vec2 { x: 0.0, y: 0.0 },
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
                background_effect_state: BackgroundEffectState::new::<Halley>(dh),
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
                pointer_gestures_state: PointerGesturesState::new::<Halley>(dh),
                presentation_state: smithay::wayland::presentation::PresentationState::new::<Halley>(
                    dh,
                    <smithay::utils::Monotonic as smithay::utils::ClockSource>::ID as u32,
                ),
                relative_pointer_manager_state: RelativePointerManagerState::new::<Halley>(dh),
                fractional_scale_manager_state: FractionalScaleManagerState::new::<Halley>(dh),
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
                session_lock: crate::protocol::wayland::session_lock::HalleySessionLockState::new::<
                    Halley,
                    _,
                >(dh, |_| true),
                seat,
                cursor_manager: crate::render::CursorManager::default(),
                dmabuf_importer: None,
                dmabuf_output_feedbacks: HashMap::new(),
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
                    layer_surface_namespace: HashMap::new(),
                    layer_surface_order: Vec::new(),
                    aperture_layer_monitors: HashSet::new(),
                    aperture_layer_heights: HashMap::new(),
                    pending_workarea_refresh: HashSet::new(),
                    layer_surface_committed: HashSet::new(),
                    layer_surface_last_configured_size: HashMap::new(),
                    layer_keyboard_focus: None,
                },
                focus_state: FocusState {
                    interaction_focus_until_ms: 0,
                    last_surface_focus_ms: HashMap::new(),
                    outside_focus_ring_since_ms: HashMap::new(),
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
                    overlap_raise_order: HashMap::new(),
                    next_overlap_raise_order: 0,
                },
                cluster_state: ClusterState {
                    cluster_form_state: ClusterFormationState::default(),
                    cluster_names: HashMap::new(),
                    cluster_name_prompt: HashMap::new(),
                    cluster_finalize_drafts: HashMap::new(),
                    pending_lift_cluster_builds: HashMap::new(),
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
                    cluster_slot_order: HashMap::new(),
                    pending_cluster_slot_transition: HashMap::new(),
                },
                workspace_state: WorkspaceState {
                    last_active_size: HashMap::new(),
                    manual_collapsed_nodes: HashSet::new(),
                    pending_collapses: HashMap::new(),
                    pending_silent_close_until_ms: HashMap::new(),
                    user_pinned_nodes: HashSet::new(),
                    active_transitions: HashMap::new(),
                    primary_promote_cooldown_until_ms: HashMap::new(),
                    maximize_sessions: HashMap::new(),
                    maximize_animation: HashMap::new(),
                    maximize_resume: HashMap::new(),
                },
                fullscreen_state: FullscreenState {
                    fullscreen_active_node: HashMap::new(),
                    fullscreen_origin: HashMap::new(),
                    fullscreen_suspended_node: HashMap::new(),
                    fullscreen_soft_suspended_node: HashMap::new(),
                    fullscreen_restore: HashMap::new(),
                    fullscreen_motion: HashMap::new(),
                    fullscreen_scale_anim: HashMap::new(),
                    fullscreen_camera_restore: HashMap::new(),
                    direct_scanout: HashMap::new(),
                    fullscreen_hidden_cluster_siblings: HashMap::new(),
                    client_fullscreen_blocked_nodes: HashSet::new(),
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
                    live_window_opacity: HashMap::new(),
                    pending_rule_rechecks: HashSet::new(),
                    pending_initial_reveal: HashSet::new(),
                    pending_initial_spawn_placement: None,
                    initial_spawn_placements: HashMap::new(),
                    pending_pan_activate: None,
                },
                field: Field::new(),
                viewport: primary_viewport,
                zoom_ref_size: primary_zoom_ref,
                zoom_log_vel: 0.0,
                pan_vel: Vec2 { x: 0.0, y: 0.0 },
                camera_target_center: primary_viewport.center,
                camera_target_view_size: primary_zoom_ref,
                surface_to_node: HashMap::new(),
                node_app_ids: HashMap::new(),
                pinned_popup_anchor: HashMap::new(),
            },
            ui: UiState {
                render_state: RenderState {
                    animator: Animator::new(now),
                    cache: Default::default(),
                    view: crate::render::state::RenderViewState {
                        node_hover_mix: HashMap::new(),
                        node_preview_hover: HashMap::new(),
                        bearings_visible: false,
                        bearings_mix: HashMap::new(),
                        cluster_bloom_mix: HashMap::new(),
                        apogee_core_hover_mix: HashMap::new(),
                    },
                    overlays: crate::render::state::RenderOverlayState {
                        overlay_banner: HashMap::new(),
                        overlay_toast: HashMap::new(),
                        overlay_exit_confirm: HashMap::new(),
                    },
                    window_animations: crate::render::state::RenderWindowAnimationState {
                        cluster_tile_tracks: HashMap::new(),
                        cluster_tile_entry_pending: HashSet::new(),
                        cluster_tile_frozen_geometry: HashMap::new(),
                        closing_window_animations: HashMap::new(),
                        animation_prewarm_requests: HashMap::new(),
                        stack_cycle_transition: HashMap::new(),
                        raise_animations: HashMap::new(),
                        landmark_slide_animations: HashMap::new(),
                    },
                    gpu: Default::default(),
                    telemetry: crate::render::state::RenderTelemetryState {
                        fps_samplers: HashMap::new(),
                        render_last_tick: now,
                    },
                },
            },
            aperture: crate::aperture::ApertureState::new(initial_aperture_config, now),
            input: InputState {
                interaction_state: InteractionState {
                    reset_input_state_requested: false,
                    pending_pointer_screen_hint: None,
                    last_pointer_screen_global: None,
                    pointer_contents: Default::default(),
                    pointer_surface_origin: None,
                    pointer_focus: None,
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
                    suppress_fullscreen_resume_on_focus: false,
                    physics_velocity: HashMap::new(),
                    physics_last_tick: now,
                    smoothed_render_pos: HashMap::new(),
                    viewport_pan_anim: None,
                    pan_dominant_until_ms: 0,
                    pending_maximize: None,
                    active_drag: None,
                    cluster_join_candidate: None,
                    bloom_pull_preview: None,
                    cluster_overflow_drag_preview: None,
                    grabbed_layer_surface: None,
                    cluster_name_prompt_drag_monitor: None,
                    cluster_name_prompt_repeat: None,
                    screenshot_session: None,
                    pending_screenshot_capture: None,
                    inflight_screenshot_capture: None,
                    screenshot_next_serial: 1,
                    last_screenshot_result: None,
                    portal_chooser: None,
                    modal_release_keys: HashSet::new(),
                    forwarded_pressed_keys: HashSet::new(),
                    keys_physically_down: HashSet::new(),
                    pending_modal_focus_restore: None,
                    focus_cycle_session: None,
                    active_gesture_route: None,
                    apogee_session: None,
                    apogee_live_preview_node: None,
                    apogee_live_preview_last_at: None,
                    apogee_hover_node: None,
                    overlay_hover_target: None,
                    cursor_override_until_ms: None,
                    pending_core_hover: None,
                    pending_core_press: None,
                    pending_collapsed_node_press: None,
                    pending_move_press: None,
                    pending_core_click: None,
                    pending_collapsed_node_click: None,
                    grabbed_edge_pan_active: false,
                    grabbed_edge_pan_direction: Vec2 { x: 0.0, y: 0.0 },
                    grabbed_edge_pan_pressure: Vec2 { x: 0.0, y: 0.0 },
                    grabbed_edge_pan_monitor: None,
                    cursor_override_icon: None,
                    cursor_hidden_by_typing: false,
                    cursor_hidden_by_keyboard_nav: false,
                    last_cursor_activity_at_ms: 0,
                },
                devices: Vec::new(),
            },
            portal: crate::protocol::wayland::portal::PortalState::default(),
            screencast: crate::portal::ScreencastState::default(),
            runtime: RuntimeState {
                tuning,
                surface_activity: HashMap::new(),
                exit_requested: false,
                started_at: now,
                maintenance_dirty: true,
                skip_next_cluster_relayout: false,
                screenshot_full_repaint_until_ms: 0,
                maintenance_ping: None,
                tty_redraw_all: true,
                tty_redraw_outputs: HashSet::new(),
                tty_frame_callback_sequence: HashMap::new(),
                pending_drm_syncobj_surfaces: Arc::new(Mutex::new(Vec::new())),
                activation: Default::default(),
                spawned_children: Vec::new(),
                wayland_display: None,
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

    pub(crate) fn apply_aperture_config(&mut self, config: crate::aperture::core::ApertureConfig) {
        self.aperture.apply_config(config);
        crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(self);
        self.request_maintenance();
    }

    pub(crate) fn request_window_animation_prewarm(&mut self, node_id: NodeId, now: Instant) {
        self.ui
            .render_state
            .request_window_animation_prewarm(node_id, now);
        self.request_maintenance();
    }

    pub(crate) fn node_user_pinned(&self, id: NodeId) -> bool {
        self.model.workspace_state.user_pinned_nodes.contains(&id)
    }

    pub(crate) fn set_node_user_pinned(&mut self, id: NodeId, pinned: bool) -> bool {
        if self.model.field.node(id).is_none() {
            return false;
        }
        if pinned {
            self.model.workspace_state.user_pinned_nodes.insert(id);
            let _ = self.model.field.set_pinned(id, true);
        } else {
            self.model.workspace_state.user_pinned_nodes.remove(&id);
            let _ = self
                .model
                .field
                .set_pinned(id, self.node_session_pinned(id));
        }
        true
    }

    pub fn create_cluster(
        &mut self,
        members: Vec<NodeId>,
    ) -> Result<ClusterId, halley_core::field::ClusterCreateError> {
        let members_clone = members.clone();
        let cid = self.model.field.create_cluster(members)?;

        for member in members_clone {
            self.model.workspace_state.user_pinned_nodes.remove(&member);
        }

        if self
            .model
            .field
            .cluster(cid)
            .is_some_and(|cluster| cluster.pinned)
            && let Some(core_id) = self.model.field.cluster(cid).and_then(|c| c.core)
        {
            self.model.workspace_state.user_pinned_nodes.insert(core_id);
        }

        Ok(cid)
    }

    pub fn collapse_cluster(&mut self, id: ClusterId) -> Option<NodeId> {
        let core_id = self.model.field.collapse_cluster(id)?;

        if self
            .model
            .field
            .cluster(id)
            .is_some_and(|cluster| cluster.pinned)
        {
            self.model.workspace_state.user_pinned_nodes.insert(core_id);
        }

        Some(core_id)
    }

    pub(crate) fn node_session_pinned(&self, id: NodeId) -> bool {
        if self
            .model
            .workspace_state
            .maximize_sessions
            .values()
            .any(|session| {
                session.state == crate::compositor::workspace::state::MaximizeSessionState::Active
                    && session.node_snapshots.contains_key(&id)
            })
        {
            return true;
        }

        let Some(entry) = self.model.fullscreen_state.fullscreen_restore.get(&id) else {
            return false;
        };
        let node_monitor = self
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone());
        self.model
            .fullscreen_state
            .fullscreen_active_node
            .contains_key(&node_monitor)
            || entry.pinned
    }

    #[allow(dead_code)]
    pub(crate) fn aperture_config(&self) -> &crate::aperture::core::ApertureConfig {
        self.aperture.config()
    }

    pub(crate) fn surface_lifecycle_ctx(&mut self) -> super::ctx::SurfaceLifecycleCtx<'_> {
        super::ctx::surface_lifecycle_ctx(self)
    }

    pub(crate) fn layer_shell_ctx(&mut self) -> super::ctx::LayerShellCtx<'_> {
        super::ctx::layer_shell_ctx(self)
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

    pub(crate) fn request_input_state_reset(&mut self) {
        self.input.interaction_state.reset_input_state_requested = true;
    }

    pub(crate) fn begin_modal_keyboard_capture(&mut self) {
        self.clear_keyboard_focus();
        self.request_input_state_reset();
    }

    pub(crate) fn schedule_modal_focus_restore(&mut self, target: Option<NodeId>, now: Instant) {
        self.schedule_modal_focus_restore_after(target, now, 80);
    }

    pub(crate) fn schedule_modal_focus_restore_after(
        &mut self,
        target: Option<NodeId>,
        now: Instant,
        delay_ms: u64,
    ) {
        self.input.interaction_state.pending_modal_focus_restore = Some(
            crate::compositor::interaction::state::PendingModalFocusRestore {
                target,
                restore_at_ms: self.now_ms(now).saturating_add(delay_ms.max(1)),
            },
        );
        self.request_input_state_reset();
        self.request_maintenance();
    }

    pub(crate) fn reconfigure_active_tty_monitors(
        &mut self,
        active_viewports: &[halley_config::ViewportOutputConfig],
    ) {
        super::monitor::state::reconfigure_active_tty_monitors(self, active_viewports)
    }

    pub(crate) fn monitor_for_screen(&self, sx: f32, sy: f32) -> Option<String> {
        super::monitor::state::monitor_for_screen(self, sx, sy)
    }

    pub(crate) fn monitor_for_node_or_current(&self, node_id: NodeId) -> String {
        super::monitor::state::monitor_for_node_or_current(self, node_id)
    }

    pub(crate) fn monitor_for_constrained_surface_or_current(
        &self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) -> String {
        super::monitor::state::monitor_for_constrained_surface_or_current(self, surface)
    }

    pub(crate) fn monitor_for_screen_or_current(&self, sx: f32, sy: f32) -> String {
        super::monitor::state::monitor_for_screen_or_current(self, sx, sy)
    }

    pub(crate) fn monitor_for_screen_or_interaction(&self, sx: f32, sy: f32) -> String {
        super::monitor::state::monitor_for_screen_or_interaction(self, sx, sy)
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

    pub(crate) fn node_assigned_to_current_monitor(&self, id: NodeId) -> bool {
        super::monitor::state::node_assigned_to_current_monitor(self, id)
    }

    #[allow(dead_code)]
    pub(crate) fn assign_node_to_current_monitor(&mut self, id: NodeId) {
        let monitor = self.model.monitor_state.current_monitor.clone();
        self.assign_node_to_monitor(id, monitor.as_str())
    }

    pub(crate) fn assign_node_to_monitor(&mut self, id: NodeId, monitor: &str) {
        let previous_monitor = self.model.monitor_state.node_monitor.get(&id).cloned();
        super::monitor::state::assign_node_to_monitor(self, id, monitor);
        crate::compositor::clusters::system::sync_cluster_name_for_node_monitor(
            &mut *self, id, monitor,
        );
        if previous_monitor.as_deref() != Some(monitor)
            && super::workspace::state::abort_maximize_session_for_external_active_node_on_monitor(
                self, monitor, id,
            )
        {
            self.request_maintenance();
        }
    }

    pub(crate) fn output_transform_for(&self, name: &str) -> smithay::utils::Transform {
        super::monitor::state::output_transform_for(self, name)
    }

    pub(crate) fn advertise_output(&mut self, name: &str, mode: smithay::output::Mode) {
        super::monitor::state::advertise_output(self, name, mode)
    }

    pub(crate) fn advertise_output_with_physical_size(
        &mut self,
        name: &str,
        mode: smithay::output::Mode,
        physical_size_mm: Option<(u32, u32)>,
    ) {
        super::monitor::state::advertise_output_with_physical_size(
            self,
            name,
            mode,
            physical_size_mm,
        )
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

    pub(crate) fn effective_cursor_image_status(
        &self,
    ) -> smithay::input::pointer::CursorImageStatus {
        super::platform::effective_cursor_image_status(self)
    }

    pub(crate) fn configure_dmabuf_importer(
        &mut self,
        importer: std::rc::Rc<dyn crate::backend::interface::DmabufImportBackend>,
        main_device: Option<rustix::fs::Dev>,
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

    pub(crate) fn configure_dmabuf_output_feedbacks(
        &mut self,
        output_feedbacks: std::collections::HashMap<
            String,
            smithay::wayland::dmabuf::DmabufFeedback,
        >,
    ) {
        super::platform::configure_dmabuf_output_feedbacks(self, output_feedbacks)
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

    pub(crate) fn resolve_surface_overlap(&mut self) {
        super::overlap::system::resolve_surface_overlap(self)
    }

    pub(crate) fn resolve_landmarks_overlapped_by_active_window(&mut self, window_id: NodeId) {
        super::overlap::system::resolve_landmarks_overlapped_by_active_window(self, window_id)
    }

    pub(crate) fn request_toplevel_resize(&mut self, node_id: NodeId, width: i32, height: i32) {
        super::overlap::system::request_toplevel_resize(self, node_id, width, height)
    }

    pub(crate) fn node_draws_above_fullscreen_on_monitor(&self, id: NodeId, monitor: &str) -> bool {
        super::spawn::state::node_draws_above_fullscreen_on_monitor(self, id, monitor)
    }

    pub(crate) fn node_draws_above_fullscreen_on_current_monitor(&self, id: NodeId) -> bool {
        self.node_draws_above_fullscreen_on_monitor(
            id,
            self.model.monitor_state.current_monitor.as_str(),
        )
    }

    pub fn now_ms(&self, now: Instant) -> u64 {
        super::runtime::now_ms(self, now)
    }

    pub fn apply_tuning(&mut self, tuning: RuntimeTuning) {
        super::runtime::apply_tuning(self, tuning)
    }

    pub fn exit_requested(&self) -> bool {
        super::runtime::exit_requested(self)
    }

    pub fn request_maintenance(&mut self) {
        super::runtime::request_maintenance(self)
    }

    pub fn request_tty_redraw_for_monitor(&mut self, monitor: &str) {
        self.runtime.tty_redraw_outputs.insert(monitor.to_string());
        if let Some(ping) = &self.runtime.maintenance_ping {
            ping.ping();
        }
    }

    pub fn advance_tty_frame_callback_sequence(&mut self, output_name: &str) -> u32 {
        let sequence = self
            .runtime
            .tty_frame_callback_sequence
            .entry(output_name.to_string())
            .or_insert(0);
        *sequence = sequence.wrapping_add(1);
        *sequence
    }

    pub fn tty_frame_callback_sequence(&self, output_name: &str) -> u32 {
        self.runtime
            .tty_frame_callback_sequence
            .get(output_name)
            .copied()
            .unwrap_or(0)
    }

    pub fn run_maintenance_if_needed(&mut self, now: Instant) {
        super::runtime::run_maintenance_if_needed(self, now)
    }

    pub(crate) fn record_focus_trail_visit(&mut self, id: NodeId) {
        super::focus::trail::record_focus_trail_visit(self, id)
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
        direction: halley_api::TrailDirection,
        now: Instant,
    ) -> bool {
        super::focus::trail::navigate_window_trail(self, direction, now)
    }

    pub(crate) fn previous_window_from_trail_on_close(
        &mut self,
        monitor: &str,
        closing_id: NodeId,
    ) -> Option<NodeId> {
        super::focus::trail::previous_window_from_trail_on_close(self, monitor, closing_id)
    }

    pub(crate) fn restore_focus_to_node_after_close(
        &mut self,
        monitor: &str,
        id: NodeId,
        now: Instant,
        suppress_pan: bool,
    ) -> bool {
        super::focus::trail::restore_focus_to_node_after_close(self, monitor, id, now, suppress_pan)
    }

    pub(crate) fn enforce_single_primary_active_unit(&mut self) {
        super::focus::decay::enforce_single_primary_active_unit(self)
    }

    #[cfg(test)]
    pub(crate) fn surface_is_definitively_outside_focus_ring(&self, id: NodeId) -> bool {
        super::focus::decay::surface_is_definitively_outside_focus_ring(self, id)
    }

    pub fn apply_single_surface_decay_policy(
        &mut self,
        id: NodeId,
        now_ms: u64,
        active_delay_ms: u64,
        inactive_delay_ms: u64,
    ) {
        super::focus::decay::apply_single_surface_decay_policy(
            self,
            id,
            now_ms,
            active_delay_ms,
            inactive_delay_ms,
        )
    }

    pub fn active_focus_ring(&self) -> halley_core::viewport::FocusRing {
        super::focus::state::active_focus_ring(self)
    }

    pub fn focus_ring_for_monitor(&self, monitor: &str) -> halley_core::viewport::FocusRing {
        super::focus::state::focus_ring_for_monitor(self, monitor)
    }

    pub fn should_draw_focus_ring_preview(&self, now: Instant) -> bool {
        super::focus::state::should_draw_focus_ring_preview(self, now)
    }

    pub(crate) fn focus_monitor_view(&mut self, monitor: &str, now: Instant) {
        super::focus::state::focus_monitor_view(self, monitor, now)
    }

    pub fn set_interaction_focus(&mut self, id: Option<NodeId>, hold_ms: u64, now: Instant) {
        super::focus::state::set_interaction_focus(self, id, hold_ms, now)
    }

    #[allow(dead_code)]
    pub(crate) fn focused_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        super::focus::state::focused_node_for_monitor(self, monitor)
    }

    pub fn set_recent_top_node(&mut self, node_id: NodeId, until: Instant) {
        super::focus::state::set_recent_top_node(self, node_id, until)
    }

    pub fn raise_overlap_policy_node(&mut self, node_id: NodeId) -> bool {
        super::focus::state::raise_overlap_policy_node(self, node_id)
    }

    pub fn overlap_policy_stack_rank(&self, node_id: NodeId) -> (u64, u64) {
        super::focus::state::overlap_policy_stack_rank(self, node_id)
    }

    pub(crate) fn focus_pointer_target(
        &mut self,
        node_id: NodeId,
        hold_ms: u64,
        now: Instant,
    ) -> NodeId {
        super::focus::system::focus_pointer_target(self, node_id, hold_ms, now)
    }

    pub(crate) fn focus_cycle_session_active(&self) -> bool {
        super::focus::cycle::focus_cycle_session_active(self)
    }

    #[cfg(test)]
    pub(crate) fn focus_cycle_preview_node(&self) -> Option<NodeId> {
        super::focus::cycle::focus_cycle_preview_node(self)
    }

    pub(crate) fn start_or_step_focus_cycle(
        &mut self,
        direction: halley_config::FocusCycleBindingAction,
        now: Instant,
    ) -> bool {
        super::focus::cycle::start_or_step_focus_cycle(self, direction, now)
    }

    pub(crate) fn cancel_focus_cycle(&mut self) -> bool {
        super::focus::cycle::cancel_focus_cycle(self)
    }

    pub(crate) fn commit_focus_cycle(&mut self, now: Instant) -> bool {
        super::focus::cycle::commit_focus_cycle(self, now)
    }

    pub fn set_app_focused(&mut self, focused: bool) {
        super::focus::system::set_app_focused(self, focused)
    }

    pub(crate) fn clear_keyboard_focus(&mut self) {
        super::focus::system::clear_keyboard_focus(self)
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
        super::focus::system::apply_wayland_focus_state(self, id)
    }

    pub fn update_focus_tracking_for_surface(&mut self, fid: NodeId, now_ms: u64) {
        super::focus::system::update_focus_tracking_for_surface(self, fid, now_ms)
    }

    pub fn note_pan_activity(&mut self, now: Instant) {
        super::focus::system::note_pan_activity(self, now)
    }

    pub(crate) fn note_pan_viewport_change(&mut self, now: Instant) {
        super::focus::system::note_pan_viewport_change(self, now)
    }

    pub fn set_pan_restore_focus_target(&mut self, id: NodeId) {
        super::focus::system::set_pan_restore_focus_target(self, id)
    }

    pub fn animate_viewport_center_to(&mut self, target_center: Vec2, now: Instant) -> bool {
        super::focus::system::animate_viewport_center_to(self, target_center, now)
    }

    pub fn animate_viewport_center_to_on_monitor(
        &mut self,
        monitor: &str,
        target_center: Vec2,
        now: Instant,
    ) -> bool {
        super::focus::system::animate_viewport_center_to_on_monitor(
            self,
            monitor,
            target_center,
            now,
        )
    }

    pub(crate) fn tick_viewport_pan_animation(&mut self, now_ms: u64) {
        super::focus::system::tick_viewport_pan_animation(self, now_ms)
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

    pub fn resolve_overlap_now(&mut self) {
        super::focus::system::resolve_overlap_now(self)
    }

    pub fn set_last_active_size_now(&mut self, id: NodeId, size: Vec2) {
        super::focus::system::set_last_active_size_now(self, id, size)
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

    pub(crate) fn fullscreen_monitor_for_node(&self, node_id: NodeId) -> Option<&str> {
        super::fullscreen::system::fullscreen_monitor_for_node(self, node_id)
    }

    pub(crate) fn is_fullscreen_active(&self, node_id: NodeId) -> bool {
        super::fullscreen::system::is_fullscreen_active(self, node_id)
    }

    pub(crate) fn is_fullscreen_session_node(&self, node_id: NodeId) -> bool {
        super::fullscreen::system::is_fullscreen_session_node(self, node_id)
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

    pub(crate) fn soft_suspend_xdg_fullscreen(&mut self, node_id: NodeId, now: Instant) {
        super::fullscreen::system::soft_suspend_xdg_fullscreen(self, node_id, now)
    }

    pub(crate) fn enter_xdg_fullscreen(
        &mut self,
        node_id: NodeId,
        output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
        now: Instant,
    ) {
        super::fullscreen::system::enter_xdg_fullscreen(self, node_id, output, now)
    }

    pub(crate) fn enter_user_fullscreen(
        &mut self,
        node_id: NodeId,
        output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
        now: Instant,
    ) {
        super::fullscreen::system::enter_user_fullscreen(self, node_id, output, now)
    }

    pub(crate) fn exit_xdg_fullscreen(&mut self, node_id: NodeId, now: Instant) {
        super::fullscreen::system::exit_xdg_fullscreen(self, node_id, now)
    }

    pub(crate) fn exit_xdg_fullscreen_no_anim(&mut self, node_id: NodeId, now: Instant) {
        super::fullscreen::system::exit_xdg_fullscreen_no_anim(self, node_id, now)
    }

    pub(crate) fn tick_fullscreen_motion(&mut self, now: Instant) {
        super::fullscreen::system::tick_fullscreen_motion(self, now)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn viewport_center_for_monitor(&self, monitor: &str) -> Vec2 {
        super::spawn::reveal::placement::viewport_center_for_monitor(self, monitor)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn resolve_spawn_target_monitor(&self) -> String {
        super::spawn::reveal::placement::resolve_spawn_target_monitor(self)
    }

    #[cfg(test)]
    pub(crate) fn current_spawn_focus(&self, monitor: &str) -> (Option<NodeId>, Vec2) {
        super::spawn::reveal::placement::current_spawn_focus(self, monitor)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn viewport_fully_contains_surface_on_monitor(
        &self,
        monitor: &str,
        id: NodeId,
    ) -> bool {
        super::spawn::reveal::placement::viewport_fully_contains_surface_on_monitor(
            self, monitor, id,
        )
    }

    #[cfg(test)]
    pub(crate) fn right_spawn_candidate_for_focus(&self, id: NodeId, size: Vec2) -> Option<Vec2> {
        super::spawn::reveal::placement::right_spawn_candidate_for_focus(self, id, size)
    }

    #[cfg(test)]
    pub(crate) fn star_candidate_offsets(&self, size: Vec2) -> Vec<Vec2> {
        super::spawn::reveal::placement::star_candidate_offsets(self, size)
    }

    #[cfg(test)]
    pub(crate) fn spawn_star_step_x(&self, size: Vec2) -> f32 {
        super::spawn::reveal::placement::spawn_star_step_x(self, size)
    }

    #[cfg(test)]
    pub(crate) fn spawn_star_step_y(&self, size: Vec2) -> f32 {
        super::spawn::reveal::placement::spawn_star_step_y(self, size)
    }

    #[cfg(test)]
    pub(crate) fn spawn_candidate_for_focus_dir(
        &self,
        id: NodeId,
        size: Vec2,
        dir: Vec2,
    ) -> Option<Vec2> {
        super::spawn::reveal::placement::spawn_candidate_for_focus_dir(self, id, size, dir)
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
        super::spawn::reveal::placement::update_spawn_patch(
            self, monitor, anchor, focus_node, focus_pos, growth_dir,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn pick_spawn_position(&mut self, size: Vec2) -> (String, Vec2, bool) {
        super::spawn::reveal::placement::pick_spawn_position(self, size)
    }

    pub(crate) fn spawn_target_monitor_for_intent(
        &self,
        intent: &super::spawn::rules::InitialWindowIntent,
    ) -> String {
        super::spawn::reveal::placement::spawn_target_monitor_for_intent(self, intent)
    }

    pub(crate) fn pick_spawn_position_with_intent(
        &mut self,
        size: Vec2,
        intent: &super::spawn::rules::InitialWindowIntent,
    ) -> (String, Vec2, bool) {
        super::spawn::reveal::placement::pick_spawn_position_with_intent(self, size, intent)
    }

    pub(crate) fn finalize_initial_spawn_position(&mut self, id: NodeId, size: Vec2) -> bool {
        super::spawn::reveal::placement::finalize_initial_spawn_position(self, id, size)
    }

    pub(crate) fn reveal_new_toplevel_node(
        &mut self,
        id: NodeId,
        is_transient: bool,
        now: Instant,
    ) {
        super::spawn::reveal::reveal_new_toplevel_node(self, id, is_transient, now)
    }

    pub(crate) fn remove_node_from_field(&mut self, id: NodeId, now_ms: u64) -> bool {
        crate::compositor::clusters::system::remove_node_from_field(self, id, now_ms)
    }

    pub fn cluster_bloom_for_monitor(
        &mut self,
        monitor: &str,
    ) -> Option<halley_core::cluster::ClusterId> {
        crate::compositor::clusters::system::cluster_bloom_for_monitor(self, monitor)
    }

    #[cfg(test)]
    pub(crate) fn sync_cluster_monitor(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        preferred: Option<&str>,
    ) -> bool {
        crate::compositor::clusters::system::sync_cluster_monitor(self, cid, preferred)
    }

    #[cfg(test)]
    pub(crate) fn enter_cluster_workspace_by_core(
        &mut self,
        core_id: NodeId,
        monitor: &str,
        now: Instant,
    ) -> bool {
        crate::compositor::clusters::system::enter_cluster_workspace_by_core(
            self, core_id, monitor, now,
        )
    }

    pub fn close_cluster_bloom_for_monitor(&mut self, monitor: &str) -> bool {
        crate::compositor::clusters::system::close_cluster_bloom_for_monitor(self, monitor)
    }

    pub fn detach_member_from_cluster(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        member_id: NodeId,
        world_pos: Vec2,
        now: Instant,
    ) -> bool {
        crate::compositor::clusters::system::detach_member_from_cluster(
            self, cid, member_id, world_pos, now,
        )
    }

    #[allow(dead_code)]
    pub fn absorb_node_into_cluster(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        node_id: NodeId,
        now: Instant,
    ) -> bool {
        crate::compositor::clusters::system::absorb_node_into_cluster(self, cid, node_id, now)
    }

    pub fn active_cluster_workspace_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<halley_core::cluster::ClusterId> {
        super::clusters::system::active_cluster_workspace_for_monitor(self, monitor)
    }

    pub(crate) fn reveal_cluster_overflow_for_monitor(&mut self, monitor: &str, now_ms: u64) {
        crate::compositor::clusters::system::reveal_cluster_overflow_for_monitor(
            self, monitor, now_ms,
        )
    }

    pub(crate) fn cluster_overflow_rect_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<halley_core::tiling::Rect> {
        crate::compositor::clusters::system::cluster_overflow_rect_for_monitor(self, monitor)
    }

    pub(crate) fn cluster_overflow_slot_rect_for_monitor(
        &self,
        monitor: &str,
        overflow_len: usize,
        slot_index: usize,
    ) -> Option<halley_core::tiling::Rect> {
        crate::compositor::clusters::system::cluster_overflow_slot_rect_for_monitor(
            self,
            monitor,
            overflow_len,
            slot_index,
        )
    }

    pub(crate) fn active_cluster_tile_rect_for_member(
        &self,
        monitor: &str,
        member_id: NodeId,
    ) -> Option<halley_core::tiling::Rect> {
        crate::compositor::clusters::system::active_cluster_tile_rect_for_member(
            self, monitor, member_id,
        )
    }

    pub(crate) fn adjust_cluster_overflow_scroll_for_monitor(
        &mut self,
        monitor: &str,
        delta: i32,
    ) -> bool {
        crate::compositor::clusters::system::adjust_cluster_overflow_scroll_for_monitor(
            self, monitor, delta,
        )
    }

    pub(crate) fn cluster_spawn_rect_for_new_member(
        &self,
        monitor: &str,
        cid: halley_core::cluster::ClusterId,
    ) -> Option<halley_core::tiling::Rect> {
        crate::compositor::clusters::system::cluster_spawn_rect_for_new_member(self, monitor, cid)
    }

    pub(crate) fn swap_cluster_overflow_member_with_visible(
        &mut self,
        monitor: &str,
        cid: halley_core::cluster::ClusterId,
        overflow_member: NodeId,
        visible_member: NodeId,
        now_ms: u64,
    ) -> bool {
        crate::compositor::clusters::system::swap_cluster_overflow_member_with_visible(
            self,
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
        crate::compositor::clusters::system::reorder_cluster_overflow_member(
            self,
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
        crate::compositor::clusters::system::move_active_cluster_member_to_drop_tile(
            self, monitor, member, world_pos, now_ms,
        )
    }

    pub(crate) fn cycle_active_stack_for_monitor(
        &mut self,
        monitor: &str,
        direction: halley_core::cluster_layout::ClusterCycleDirection,
        now: Instant,
    ) -> bool {
        crate::compositor::clusters::system::cycle_active_stack_for_monitor(
            self, monitor, direction, now,
        )
    }

    pub fn collapse_active_cluster_workspace(&mut self, now: Instant) -> bool {
        crate::compositor::clusters::system::collapse_active_cluster_workspace(self, now)
    }

    pub fn cluster_mode_active(&self) -> bool {
        crate::compositor::clusters::system::cluster_mode_active(self)
    }

    pub fn cluster_mode_active_for_monitor(&self, monitor: &str) -> bool {
        crate::compositor::clusters::system::cluster_mode_active_for_monitor(self, monitor)
    }

    pub fn enter_cluster_mode(&mut self) -> bool {
        crate::compositor::clusters::system::enter_cluster_mode(self)
    }

    pub fn exit_cluster_mode(&mut self) -> bool {
        crate::compositor::clusters::system::exit_cluster_mode(self)
    }

    pub fn toggle_cluster_mode_selection(&mut self, node_id: NodeId) -> bool {
        crate::compositor::clusters::system::toggle_cluster_mode_selection(self, node_id)
    }

    pub fn toggle_cluster_workspace_by_core(&mut self, core_id: NodeId, now: Instant) -> bool {
        crate::compositor::clusters::system::toggle_cluster_workspace_by_core(self, core_id, now)
    }

    pub(crate) fn activate_cluster_slot_on_current_monitor(
        &mut self,
        slot: u8,
        now: Instant,
    ) -> bool {
        crate::compositor::clusters::system::activate_cluster_slot_on_current_monitor(
            self, slot, now,
        )
    }

    pub(crate) fn process_pending_cluster_slot_transition_for_current_monitor(
        &mut self,
        now: Instant,
    ) -> bool {
        crate::compositor::clusters::system::process_pending_cluster_slot_transition_for_current_monitor(self, now)
    }

    pub fn has_active_cluster_workspace(&self) -> bool {
        crate::compositor::clusters::system::has_active_cluster_workspace(self)
    }

    pub(crate) fn layout_active_cluster_workspace_for_monitor(
        &mut self,
        monitor: &str,
        now_ms: u64,
    ) {
        crate::compositor::clusters::system::layout_active_cluster_workspace_for_monitor(
            self, monitor, now_ms,
        )
    }

    pub(crate) fn focus_active_tiled_cluster_member_for_monitor(
        &mut self,
        monitor: &str,
        preferred_index: Option<usize>,
        now: Instant,
    ) -> bool {
        crate::compositor::clusters::system::focus_active_tiled_cluster_member_for_monitor(
            self,
            monitor,
            preferred_index,
            now,
        )
    }

    pub(crate) fn tile_focus_active_cluster_member_for_monitor(
        &mut self,
        monitor: &str,
        direction: halley_config::DirectionalAction,
        now: Instant,
    ) -> bool {
        crate::compositor::clusters::system::tile_focus_active_cluster_member_for_monitor(
            self, monitor, direction, now,
        )
    }

    pub(crate) fn tile_swap_active_cluster_member_for_monitor(
        &mut self,
        monitor: &str,
        direction: halley_config::DirectionalAction,
        now: Instant,
    ) -> bool {
        crate::compositor::clusters::system::tile_swap_active_cluster_member_for_monitor(
            self, monitor, direction, now,
        )
    }

    pub(crate) fn cycle_active_cluster_layout_for_monitor(
        &mut self,
        monitor: &str,
        now: Instant,
    ) -> bool {
        crate::compositor::clusters::system::cycle_active_cluster_layout_for_monitor(
            self, monitor, now,
        )
    }
}

impl Drop for Halley {
    fn drop(&mut self) {
        for child in &mut self.runtime.spawned_children {
            let pgid = child.id() as i32;
            if let Some(pid) = rustix::process::Pid::from_raw(pgid) {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::TERM);
            }
            let _ = child.wait();
        }
    }
}

delegate_dmabuf!(Halley);
