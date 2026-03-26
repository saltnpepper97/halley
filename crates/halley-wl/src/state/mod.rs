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
use halley_core::trail::Trail;
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
        shell::xdg::XdgShellState,
        shm::ShmState,
        viewporter::ViewporterState,
    },
};

use crate::activity::CommitActivity;
use crate::animation::{AnimSpec, Animator};
use crate::backend::interface::DmabufImportBackend;
use crate::state::carry::CarryState;
use crate::state::focus::FocusState;
use crate::state::fullscreen::FullscreenState;
use crate::state::interaction::InteractionState;
use crate::state::monitor::{MonitorSpace, MonitorState};
use crate::state::render::RenderState;
use crate::state::spawn::SpawnState;
use crate::state::workspace::WorkspaceState;

mod carry;
mod client;
mod focus;
mod fullscreen;
mod interaction;
mod monitor;
mod render;
mod runtime;
mod spawn;
mod workspace;

pub use client::ClientState;
pub(crate) use interaction::ViewportPanAnim;
pub(crate) use render::{NodeAppIconCacheEntry, NodeAppIconTexture};
pub(crate) use fullscreen::{FullscreenMotion, FullscreenSessionEntry, FullscreenScaleAnim};
pub(crate) use spawn::{
    ActiveSpawnPan, MonitorSpawnState, PendingSpawnPan, SpawnAnchorMode, SpawnPatch,
};

pub struct Halley {
    pub display_handle: DisplayHandle,
    pub compositor_state: CompositorState,
    pub viewporter_state: ViewporterState,
    pub xdg_shell_state: XdgShellState,
    pub popup_manager: PopupManager,
    pub wlr_layer_shell_state: WlrLayerShellState,
    pub pointer_constraints_state: PointerConstraintsState,
    pub relative_pointer_manager_state: RelativePointerManagerState,
    pub idle_notifier_state: IdleNotifierState<Self>,
    pub drm_syncobj_state: Option<DrmSyncobjState>,
    pub output_manager_state: OutputManagerState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub data_control_state: DataControlState,
    pub seat: Seat<Self>,

    pub(crate) carry_state: CarryState,
    pub(crate) monitor_state: MonitorState,
    pub(crate) focus_state: FocusState,
    pub(crate) workspace_state: WorkspaceState,
    pub(crate) interaction_state: InteractionState,
    pub(crate) render_state: RenderState,
    pub(crate) fullscreen_state: FullscreenState,
    pub(crate) spawn_state: SpawnState,

    pub field: Field,
    pub viewport: Viewport,
    pub tuning: RuntimeTuning,
    pub zoom_ref_size: Vec2,
    pub(crate) camera_target_center: Vec2,
    pub(crate) camera_target_view_size: Vec2,
    pub cursor_image_status: CursorImageStatus,
    pub(crate) dmabuf_importer: Option<Rc<dyn DmabufImportBackend>>,

    pub surface_activity: HashMap<ObjectId, CommitActivity>,
    pub surface_to_node: HashMap<ObjectId, NodeId>,
    pub(crate) node_app_ids: HashMap<NodeId, String>,

    pub(crate) exit_requested: bool,

    pub(crate) started_at: Instant,
    pub(crate) last_debug_dump_at: Instant,
    pub(crate) maintenance_dirty: bool,
    pub(crate) maintenance_ping: Option<Ping>,
    pub(crate) pending_drm_syncobj_surfaces: Arc<Mutex<Vec<ObjectId>>>,

    pub(crate) spawned_children: Vec<std::process::Child>,
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
            display_handle: dh.clone(),
            compositor_state: CompositorState::new::<Halley>(dh),
            viewporter_state: ViewporterState::new::<Halley>(dh),
            xdg_shell_state: XdgShellState::new::<Halley>(dh),
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
                monitors,
                node_monitor: HashMap::new(),
                layer_surface_monitor: HashMap::new(),
                layer_keyboard_focus: None,
            },

            focus_state: FocusState {
                interaction_focus_until_ms: 0,
                last_surface_focus_ms: HashMap::new(),
                focus_trail: Trail::new(),
                suppress_trail_record_once: false,
                pan_restore_active_focus: None,
                app_focused: true,
                monitor_focus: HashMap::new(),
                primary_interaction_focus: None,
                focus_ring_preview_until_ms: HashMap::new(),
                recent_top_node: None,
                recent_top_until: None,
            },

            workspace_state: WorkspaceState {
                cluster_form_state: ClusterFormationState::default(),
                active_cluster_workspace: None,
                workspace_hidden_nodes: Vec::new(),
                workspace_prev_viewport: None,
                last_active_size: HashMap::new(),
                manual_collapsed_nodes: HashSet::new(),
                active_transition_until_ms: HashMap::new(),
                primary_promote_cooldown_until_ms: HashMap::new(),
            },

            render_state: RenderState {
                animator: Animator::new(now),

                node_app_icon_cache: HashMap::new(),
                node_hover_mix: HashMap::new(),
                node_preview_hover_node: None,
                node_preview_hover_mix: 0.0,
                node_circle_texture: None,
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

                suspend_overlap_resolve: false,
                suspend_state_checks: false,

                physics_velocity: HashMap::new(),
                physics_last_tick: now,

                smoothed_render_pos: HashMap::new(),
                viewport_pan_anim: None,
                pan_dominant_until_ms: 0,
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
                per_monitor: HashMap::new(),
                pending_spawn_pan_queue: VecDeque::new(),
                active_spawn_pan: None,
            },

            field: Field::new(),
            viewport: primary_viewport,
            zoom_ref_size: primary_zoom_ref,
            camera_target_center: primary_viewport.center,
            camera_target_view_size: primary_zoom_ref,
            cursor_image_status: CursorImageStatus::default_named(),
            dmabuf_importer: None,

            tuning,

            surface_activity: HashMap::new(),
            surface_to_node: HashMap::new(),
            node_app_ids: HashMap::new(),

            exit_requested: false,

            started_at: now,
            last_debug_dump_at: now,
            maintenance_dirty: true,
            maintenance_ping: None,
            pending_drm_syncobj_surfaces: Arc::new(Mutex::new(Vec::new())),

            spawned_children: Vec::new(),
        };
        out.render_state.animator.set_spec(AnimSpec {
            state_change_ms: out.tuning.dev_anim_state_change_ms,
            bounce: out.tuning.dev_anim_bounce,
        });
        out.spawn_state.per_monitor = out
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
        let current_monitor = out.monitor_state.current_monitor.clone();
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
                self.dmabuf_state
                    .create_global_with_default_feedback::<Halley>(
                        &self.display_handle,
                        &feedback,
                    )
            }
            None => self
                .dmabuf_state
                .create_global::<Halley>(&self.display_handle, formats.iter().copied()),
        };

        self.dmabuf_importer = Some(importer);
        self.dmabuf_global = Some(global);
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
        self.exit_requested = true;
    }

    #[inline]
    pub fn note_input_activity(&mut self) {
        self.idle_notifier_state.notify_activity(&self.seat);
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_requested
    }

    #[inline]
    pub fn set_maintenance_ping(&mut self, ping: Ping) {
        self.maintenance_ping = Some(ping);
        self.request_maintenance();
    }

    #[inline]
    pub fn request_maintenance(&mut self) {
        self.maintenance_dirty = true;
        if let Some(ping) = &self.maintenance_ping {
            ping.ping();
        }
    }

    pub fn next_maintenance_deadline(&self, now: Instant) -> Option<Instant> {
        if !self.focus_state.app_focused {
            return None;
        }

        let now_ms = self.now_ms(now);
        let mut next_ms: Option<u64> = None;
        let mut consider = |at_ms: u64| {
            next_ms = Some(next_ms.map_or(at_ms, |cur| cur.min(at_ms)));
        };

        if self.focus_state.primary_interaction_focus.is_some()
            && self.focus_state.interaction_focus_until_ms > now_ms
        {
            consider(self.focus_state.interaction_focus_until_ms);
        }
        if self.interaction_state.resize_static_node.is_some()
            && self.interaction_state.resize_static_until_ms > now_ms
        {
            consider(self.interaction_state.resize_static_until_ms);
        }
        if let Some(at_ms) = self.spawn_state.pending_spawn_activate_at_ms.values().copied().min()
            && at_ms > now_ms
        {
            consider(at_ms);
        }
        if let Some(at_ms) = self
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
            .workspace_state
            .primary_promote_cooldown_until_ms
            .values()
            .copied()
            .min()
            && at_ms > now_ms
        {
            consider(at_ms);
        }
        if self.tuning.debug_tick_dump {
            consider(
                now_ms.saturating_add(
                    self.tuning.debug_dump_every_ms.saturating_sub(
                        now.duration_since(self.last_debug_dump_at).as_millis() as u64,
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
        if self.maintenance_dirty || due {
            self.run_maintenance(now);
        }
    }

    #[inline]
    pub fn run_maintenance(&mut self, now: Instant) {
        self.maintenance_dirty = false;
        if !self.focus_state.app_focused {
            return;
        }
        self.reconcile_surface_bindings();
        let now_ms = now.duration_since(self.started_at).as_millis() as u64;
        let _ = self.recent_top_node_active(now);
        if self.workspace_state.active_cluster_workspace.is_some() {
            self.layout_active_cluster_workspace(now_ms);
            self.render_state.animator.observe_field(&self.field, now);
            return;
        }
        if let Some(fid) = self.focus_state.primary_interaction_focus
            && now_ms >= self.focus_state.interaction_focus_until_ms
        {
            let keep = self.field.node(fid).is_some_and(|n| {
                self.field.is_visible(fid) && n.kind == halley_core::field::NodeKind::Surface
            });
            if keep {
                self.focus_state.interaction_focus_until_ms = now_ms.saturating_add(30_000);
            } else {
                self.set_interaction_focus(None, 0, now);
            }
        }
        if self.focus_state.primary_interaction_focus.is_none()
            && self.monitor_state.layer_keyboard_focus.is_some()
        {
            self.reassert_layer_surface_keyboard_focus_if_drifted();
        }
        self.workspace_state
            .active_transition_until_ms
            .retain(|_, &mut until| until > now_ms);
        self.workspace_state
            .primary_promote_cooldown_until_ms
            .retain(|_, &mut until| until > now_ms);
        let alive_ids: HashSet<NodeId> = self.field.nodes().keys().copied().collect();
        self.carry_state.carry_zone_hint.retain(|id, _| alive_ids.contains(id));
        self.carry_state.carry_zone_last_change_ms
            .retain(|id, _| alive_ids.contains(id));
        self.carry_state.carry_zone_pending
            .retain(|id, _| alive_ids.contains(id));
        self.carry_state.carry_zone_pending_since_ms
            .retain(|id, _| alive_ids.contains(id));
        self.carry_state.carry_activation_anim_armed
            .retain(|id| alive_ids.contains(id));
        self.carry_state.carry_state_hold.retain(|id, _| alive_ids.contains(id));
        self.focus_state
            .last_surface_focus_ms
            .retain(|id, _| alive_ids.contains(id));
        self.workspace_state
            .manual_collapsed_nodes
            .retain(|id| alive_ids.contains(id));

        self.process_pending_spawn_activations(now, now_ms);
        let resize_settling = self
            .interaction_state
            .resize_static_node
            .is_some_and(|_| now_ms < self.interaction_state.resize_static_until_ms);
        if resize_settling
            && let (Some(id), Some(lock_pos)) = (
                self.interaction_state.resize_static_node,
                self.interaction_state.resize_static_lock_pos,
            )
            && let Some(n) = self.field.node(id)
            && ((n.pos.x - lock_pos.x).abs() > 0.05 || (n.pos.y - lock_pos.y).abs() > 0.05)
        {
            let _ = self.field.carry(id, lock_pos);
        }
        if self
            .interaction_state
            .resize_static_node
            .is_some_and(|_| now_ms >= self.interaction_state.resize_static_until_ms)
        {
            self.interaction_state.resize_static_node = None;
            self.interaction_state.resize_static_lock_pos = None;
            self.interaction_state.resize_static_until_ms = 0;
        }
        if !self.interaction_state.suspend_state_checks {
            self.enforce_pan_dominant_zone_states(now_ms);
            self.enforce_carry_zone_states();
        }
        if let Some(id) = self.interaction_state.resize_active {
            let _ = self.field.touch(id, now_ms);
            let _ = self.field.set_decay_level(id, DecayLevel::Hot);
        }
        if self.interaction_state.resize_active.is_none()
            && !(self.interaction_state.resize_static_node.is_some()
                && now_ms < self.interaction_state.resize_static_until_ms)
        {
            self.update_zoom_live_surface_sizes();
        }
        let _ = tick_cluster_formation(
            &mut self.field,
            now_ms,
            ClusterPolicy {
                enabled: false,
                distance_px: self.tuning.cluster_distance_px,
                dwell_ms: self.tuning.cluster_dwell_ms,
                ..Default::default()
            },
            &mut self.workspace_state.cluster_form_state,
        );
        self.enforce_single_primary_active_unit();
        if !self.interaction_state.suspend_state_checks
            && self.interaction_state.resize_active.is_none()
        {
            self.resolve_surface_overlap();
        }
        self.restore_pan_return_active_focus(now);
        self.render_state.animator.observe_field(&self.field, now);

        if self.tuning.debug_tick_dump
            && now.duration_since(self.last_debug_dump_at).as_millis() as u64
                >= self.tuning.debug_dump_every_ms
        {
            self.debug_dump();
            self.last_debug_dump_at = now;
        }
    }
}

impl Drop for Halley {
    fn drop(&mut self) {
        for child in &mut self.spawned_children {
            let pgid = child.id() as i32;
            unsafe {
                libc::kill(-pgid, libc::SIGTERM);
            }
            let _ = child.wait();
        }
    }
}

delegate_dmabuf!(Halley);
