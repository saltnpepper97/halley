use std::collections::{HashMap, HashSet, VecDeque};
use std::os::unix::io::AsFd;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use calloop::{LoopHandle, ping::Ping};
use halley_config::RuntimeTuning;
use halley_core::cluster_policy::{ClusterFormationState, ClusterPolicy, tick_cluster_formation};
use halley_core::decay::DecayLevel;
use halley_core::field::{Field, NodeId, Vec2};
use halley_core::viewport::Viewport;

use smithay::{
    delegate_dmabuf,
    desktop::PopupManager,
    input::{Seat, SeatState, pointer::CursorImageStatus},
    reexports::wayland_server::{DisplayHandle, backend::ObjectId},
    utils::{Logical, Rectangle},
    wayland::{
        compositor::CompositorState,
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState},
        drm_syncobj::DrmSyncobjState,
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

use crate::activity::CommitActivity;
use crate::animation::{AnimSpec, Animator};
use crate::backend::interface::DmabufImportBackend;
use crate::state::carry::CarryState;
pub(crate) use crate::state::clusters::ClusterState;
pub(crate) use crate::state::focus::FocusState;
pub(crate) use crate::state::fullscreen::FullscreenState;
pub(crate) use crate::state::interaction::InteractionState;
pub(crate) use crate::state::monitor::{MonitorSpace, MonitorState};
pub(crate) use crate::state::render::RenderState;
pub(crate) use crate::state::workspace::WorkspaceState;

mod carry;
mod client;
mod clusters;
mod focus;
mod fullscreen;
mod interaction;
mod monitor;
mod render;
mod runtime;
mod spawn;
mod workspace;

pub use client::ClientState;
pub(crate) use fullscreen::{FullscreenMotion, FullscreenScaleAnim, FullscreenSessionEntry};
pub(crate) use interaction::{
    ActiveDragState, BloomPullPreview, ClusterJoinCandidate, PendingCoreClick, PendingCorePress,
    ViewportPanAnim,
};
pub(crate) use render::{NodeAppIconCacheEntry, NodeAppIconTexture};
pub(crate) use spawn::{
    ActiveSpawnPan, MonitorSpawnState, PendingSpawnPan, SpawnAnchorMode, SpawnPatch, SpawnState,
};

// These protocol/global objects are intentionally retained on the root state so
// the corresponding Smithay globals remain alive for the compositor lifetime,
// even when some are only exercised indirectly through trait callbacks.
#[allow(dead_code)]
pub(crate) struct PlatformState {
    pub(crate) display_handle: DisplayHandle,
    pub(crate) compositor_state: CompositorState,
    pub(crate) viewporter_state: ViewporterState,
    pub(crate) xdg_shell_state: XdgShellState,
    pub(crate) xdg_decoration_state: XdgDecorationState,
    pub(crate) popup_manager: PopupManager,
    pub(crate) wlr_layer_shell_state: WlrLayerShellState,
    pub(crate) pointer_constraints_state: PointerConstraintsState,
    pub(crate) relative_pointer_manager_state: RelativePointerManagerState,
    pub(crate) idle_notifier_state: IdleNotifierState<Halley>,
    pub(crate) drm_syncobj_state: Option<DrmSyncobjState>,
    pub(crate) output_manager_state: OutputManagerState,
    pub(crate) shm_state: ShmState,
    pub(crate) dmabuf_state: DmabufState,
    pub(crate) dmabuf_global: Option<DmabufGlobal>,
    pub(crate) seat_state: SeatState<Halley>,
    pub(crate) data_device_state: DataDeviceState,
    pub(crate) primary_selection_state: PrimarySelectionState,
    pub(crate) data_control_state: DataControlState,
    pub(crate) seat: Seat<Halley>,
    pub(crate) cursor_image_status: CursorImageStatus,
    pub(crate) dmabuf_importer: Option<Rc<dyn DmabufImportBackend>>,
}

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

pub(crate) struct RuntimeState {
    pub(crate) tuning: RuntimeTuning,
    pub(crate) surface_activity: HashMap<ObjectId, CommitActivity>,
    pub(crate) exit_requested: bool,
    pub(crate) started_at: Instant,
    pub(crate) last_debug_dump_at: Instant,
    pub(crate) maintenance_dirty: bool,
    pub(crate) maintenance_ping: Option<Ping>,
    pub(crate) pending_drm_syncobj_surfaces: Arc<Mutex<Vec<ObjectId>>>,
    pub(crate) spawned_children: Vec<std::process::Child>,
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
        dh: &smithay::reexports::wayland_server::DisplayHandle,
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
                    zoom_ref_size: tuning.viewport_size,
                    camera_target_center: tuning.viewport_center,
                    camera_target_view_size: tuning.viewport_size,
                },
            );
        }
        // Choose the startup monitor from the actual layout, not config order.
        // This keeps the compositor's notion of the primary/current monitor
        // aligned with the leftmost/topmost active output that Xwayland clients
        // and games expect.
        let current_monitor =
            preferred_monitor_name(&monitors).unwrap_or_else(|| "default".to_string());
        // Bootstrap the viewport/camera from the startup monitor's LOCAL space.
        // tuning.viewport_center is in global layout coords (for Wayland output
        // advertising) — using it as a camera center would point the camera at
        // a world position far outside the local viewport on any monitor with
        // a non-zero offset.
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
                    cluster_mode_active: false,
                    cluster_mode_selected_nodes: HashSet::new(),
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
                    overlay_banner: None,
                    overlay_toast: None,
                    node_circle_texture: None,
                    node_circle_program: None,
                    node_squircle_program: None,
                    node_label_program: None,

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
                    pending_core_press: None,
                    pending_core_click: None,
                    grabbed_edge_pan_active: false,
                    grabbed_edge_pan_direction: Vec2 { x: 0.0, y: 0.0 },
                    grabbed_edge_pan_pressure: Vec2 { x: 0.0, y: 0.0 },
                    grabbed_edge_pan_monitor: None,
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
        out.ui.render_state.animator.set_spec(AnimSpec {
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
    pub(crate) fn new_for_test(
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        tuning: RuntimeTuning,
    ) -> Self {
        let event_loop = Box::leak(Box::new(
            calloop::EventLoop::<Self>::try_new().expect("test event loop"),
        ));
        Self::new(dh, event_loop.handle(), tuning)
    }

    pub(crate) fn configure_dmabuf_importer(
        &mut self,
        importer: Rc<dyn DmabufImportBackend>,
        main_device: Option<libc::dev_t>,
    ) {
        let formats = importer.dmabuf_formats();
        if formats.is_empty() {
            return;
        }

        let global = match main_device {
            Some(device) => {
                let feedback = DmabufFeedbackBuilder::new(device, formats.iter().copied())
                    .build()
                    .expect("renderer dmabuf feedback should be constructible");
                self.platform
                    .dmabuf_state
                    .create_global_with_default_feedback::<Halley>(
                        &self.platform.display_handle,
                        &feedback,
                    )
            }
            None => self
                .platform
                .dmabuf_state
                .create_global::<Halley>(&self.platform.display_handle, formats.iter().copied()),
        };

        self.platform.dmabuf_importer = Some(importer);
        self.platform.dmabuf_global = Some(global);
    }

    pub(crate) fn configure_dmabuf_importer_for_fd<Fd: AsFd>(
        &mut self,
        importer: Rc<dyn DmabufImportBackend>,
        device_fd: Fd,
    ) {
        let main_device = rustix::fs::fstat(device_fd).ok().map(|stat| stat.st_rdev);
        self.configure_dmabuf_importer(importer, main_device);
    }

    pub fn request_exit(&mut self) {
        self.runtime.exit_requested = true;
    }

    #[inline]
    pub fn note_input_activity(&mut self) {
        self.platform
            .idle_notifier_state
            .notify_activity(&self.platform.seat);
    }

    pub fn exit_requested(&self) -> bool {
        self.runtime.exit_requested
    }

    #[inline]
    pub fn set_maintenance_ping(&mut self, ping: Ping) {
        self.runtime.maintenance_ping = Some(ping);
        self.request_maintenance();
    }

    #[inline]
    pub fn request_maintenance(&mut self) {
        self.runtime.maintenance_dirty = true;
        if let Some(ping) = &self.runtime.maintenance_ping {
            ping.ping();
        }
    }

    pub fn next_maintenance_deadline(&self, now: Instant) -> Option<Instant> {
        if !self.model.focus_state.app_focused {
            return None;
        }

        let now_ms = self.now_ms(now);
        let mut next_ms: Option<u64> = None;
        let mut consider = |at_ms: u64| {
            next_ms = Some(next_ms.map_or(at_ms, |cur| cur.min(at_ms)));
        };

        if self.model.focus_state.primary_interaction_focus.is_some()
            && self.model.focus_state.interaction_focus_until_ms > now_ms
        {
            consider(self.model.focus_state.interaction_focus_until_ms);
        }
        if self.input.interaction_state.resize_static_node.is_some()
            && self.input.interaction_state.resize_static_until_ms > now_ms
        {
            consider(self.input.interaction_state.resize_static_until_ms);
        }
        if let Some(at_ms) = self
            .model
            .spawn_state
            .pending_spawn_activate_at_ms
            .values()
            .copied()
            .min()
            && at_ms > now_ms
        {
            consider(at_ms);
        }
        if let Some(at_ms) = self
            .model
            .workspace_state
            .active_transition_until_ms
            .values()
            .copied()
            .min()
            && at_ms > now_ms
        {
            consider(at_ms);
        }
        if let Some(at_ms) = self
            .model
            .workspace_state
            .primary_promote_cooldown_until_ms
            .values()
            .copied()
            .min()
            && at_ms > now_ms
        {
            consider(at_ms);
        }
        if let Some(deadline_ms) = self
            .input
            .interaction_state
            .pending_core_click
            .as_ref()
            .map(|pending| pending.deadline_ms)
            && deadline_ms > now_ms
        {
            consider(deadline_ms);
        }
        if self.runtime.tuning.debug_tick_dump {
            consider(
                now_ms.saturating_add(
                    self.runtime.tuning.debug_dump_every_ms.saturating_sub(
                        now.duration_since(self.runtime.last_debug_dump_at)
                            .as_millis() as u64,
                    ),
                ),
            );
        }

        next_ms.map(|at_ms| {
            now.checked_add(std::time::Duration::from_millis(
                at_ms.saturating_sub(now_ms),
            ))
            .unwrap_or(now)
        })
    }

    #[inline]
    pub fn run_maintenance_if_needed(&mut self, now: Instant) {
        let due = self
            .next_maintenance_deadline(now)
            .is_some_and(|deadline| deadline <= now);
        if self.runtime.maintenance_dirty || due {
            self.run_maintenance(now);
        }
    }

    #[inline]
    pub fn run_maintenance(&mut self, now: Instant) {
        self.runtime.maintenance_dirty = false;
        if !self.model.focus_state.app_focused {
            return;
        }
        self.reconcile_surface_bindings();
        let now_ms = now.duration_since(self.runtime.started_at).as_millis() as u64;
        let _ = self.recent_top_node_active(now);
        if let Some(pending) = self.input.interaction_state.pending_core_click.clone()
            && now_ms >= pending.deadline_ms
        {
            self.input.interaction_state.pending_core_click = None;
            if pending.reopen_bloom_on_timeout
                && let Some(cid) = self.model.field.cluster_id_for_core_public(pending.node_id)
            {
                let _ = self.open_cluster_bloom_for_monitor(pending.monitor.as_str(), cid);
            }
        }
        if self.has_any_active_cluster_workspace() {
            let active_monitors = self
                .model
                .cluster_state
                .active_cluster_workspaces
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            for monitor in active_monitors {
                self.layout_active_cluster_workspace_for_monitor(monitor.as_str(), now_ms);
            }
        }
        if let Some(fid) = self.model.focus_state.primary_interaction_focus
            && now_ms >= self.model.focus_state.interaction_focus_until_ms
        {
            let keep = self.model.field.node(fid).is_some_and(|n| {
                self.model.field.is_visible(fid) && n.kind == halley_core::field::NodeKind::Surface
            });
            if keep {
                self.model.focus_state.interaction_focus_until_ms = now_ms.saturating_add(30_000);
            } else {
                self.set_interaction_focus(None, 0, now);
            }
        }
        if self.model.focus_state.primary_interaction_focus.is_none()
            && self.model.monitor_state.layer_keyboard_focus.is_some()
        {
            self.reassert_layer_surface_keyboard_focus_if_drifted();
        }
        self.model
            .workspace_state
            .active_transition_until_ms
            .retain(|_, &mut until| until > now_ms);
        self.model
            .workspace_state
            .primary_promote_cooldown_until_ms
            .retain(|_, &mut until| until > now_ms);
        let alive_ids: HashSet<NodeId> = self.model.field.node_ids_all().into_iter().collect();
        self.model
            .carry_state
            .carry_zone_hint
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .carry_state
            .carry_zone_last_change_ms
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .carry_state
            .carry_zone_pending
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .carry_state
            .carry_zone_pending_since_ms
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .carry_state
            .carry_activation_anim_armed
            .retain(|id| alive_ids.contains(id));
        self.model
            .carry_state
            .carry_state_hold
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .focus_state
            .last_surface_focus_ms
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .workspace_state
            .manual_collapsed_nodes
            .retain(|id| alive_ids.contains(id));

        self.process_pending_spawn_activations(now, now_ms);
        let resize_settling = self
            .input
            .interaction_state
            .resize_static_node
            .is_some_and(|_| now_ms < self.input.interaction_state.resize_static_until_ms);
        if resize_settling
            && let (Some(id), Some(lock_pos)) = (
                self.input.interaction_state.resize_static_node,
                self.input.interaction_state.resize_static_lock_pos,
            )
            && let Some(n) = self.model.field.node(id)
            && ((n.pos.x - lock_pos.x).abs() > 0.05 || (n.pos.y - lock_pos.y).abs() > 0.05)
        {
            let _ = self.model.field.carry(id, lock_pos);
        }
        if self
            .input
            .interaction_state
            .resize_static_node
            .is_some_and(|_| now_ms >= self.input.interaction_state.resize_static_until_ms)
        {
            self.input.interaction_state.resize_static_node = None;
            self.input.interaction_state.resize_static_lock_pos = None;
            self.input.interaction_state.resize_static_until_ms = 0;
        }
        if !self.input.interaction_state.suspend_state_checks {
            self.enforce_pan_dominant_zone_states(now_ms);
            self.enforce_carry_zone_states();
        }
        if let Some(id) = self.input.interaction_state.resize_active {
            let _ = self.model.field.touch(id, now_ms);
            let _ = self.model.field.set_decay_level(id, DecayLevel::Hot);
        }
        if self.input.interaction_state.resize_active.is_none()
            && !(self.input.interaction_state.resize_static_node.is_some()
                && now_ms < self.input.interaction_state.resize_static_until_ms)
        {
            self.update_zoom_live_surface_sizes();
        }
        let _ = tick_cluster_formation(
            &mut self.model.field,
            now_ms,
            ClusterPolicy {
                enabled: false,
                distance_px: self.runtime.tuning.cluster_distance_px,
                dwell_ms: self.runtime.tuning.cluster_dwell_ms,
                ..Default::default()
            },
            &mut self.model.cluster_state.cluster_form_state,
        );
        self.enforce_single_primary_active_unit();
        if !self.input.interaction_state.suspend_state_checks
            && self.input.interaction_state.resize_active.is_none()
        {
            self.resolve_surface_overlap();
        }
        self.restore_pan_return_active_focus(now);
        self.ui
            .render_state
            .animator
            .observe_field(&self.model.field, now);

        if self.runtime.tuning.debug_tick_dump
            && now
                .duration_since(self.runtime.last_debug_dump_at)
                .as_millis() as u64
                >= self.runtime.tuning.debug_dump_every_ms
        {
            self.debug_dump();
            self.runtime.last_debug_dump_at = now;
        }
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
