use std::collections::{HashMap, HashSet, VecDeque};
use std::os::unix::io::AsFd;
use std::rc::Rc;
use std::time::Instant;

use calloop::ping::Ping;
use halley_config::RuntimeTuning;
use halley_core::cluster::ClusterId;
use halley_core::cluster_policy::{ClusterFormationState, ClusterPolicy, tick_cluster_formation};
use halley_core::decay::DecayLevel;
use halley_core::field::{Field, NodeId, Vec2};
use halley_core::viewport::{FocusZone, Viewport};

use smithay::backend::renderer::gles::{GlesTexProgram, GlesTexture};
use smithay::{
    delegate_dmabuf,
    desktop::PopupManager,
    input::{Seat, SeatState, pointer::CursorImageStatus},
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::wayland_server::{DisplayHandle, backend::ObjectId},
    utils::{Logical, Rectangle, Transform},
    wayland::{
        compositor::CompositorState,
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState},
        output::OutputManagerState,
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
use crate::wm::ViewportPanAnim;

mod client;
mod render_state;
mod runtime_state;
pub use client::ClientState;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpawnFrontierPoint {
    pub pos: Vec2,
    pub score: f32,
    pub dir: Vec2,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct SpawnPatch {
    pub anchor: Vec2,
    pub focus_node: Option<NodeId>,
    pub focus_pos: Vec2,
    pub growth_dir: Vec2,
    pub placements_in_patch: u32,
    pub frontier: Vec<SpawnFrontierPoint>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingSpawnPan {
    pub node_id: NodeId,
    pub target_center: Vec2,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ActiveSpawnPan {
    pub node_id: NodeId,
    pub pan_start_at_ms: u64,
    pub reveal_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SpawnAnchorMode {
    Focus,
    View,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct WindowOffscreenKey {
    pub width: i32,
    pub height: i32,
}

#[derive(Default)]
pub(crate) struct WindowOffscreenCache {
    /// Native 1.0x surface-tree bbox size used to build the offscreen image.
    pub key: WindowOffscreenKey,

    /// Set when the cached offscreen image should be rebuilt before use.
    pub dirty: bool,

    /// Last frame this cache entry was touched.
    pub last_used_at: Option<Instant>,

    /// Cached 1.0x surface-tree render target for zoomed compositing.
    pub texture: Option<GlesTexture>,

    /// Logical bbox paired with the cached texture.
    pub bbox: Option<Rectangle<i32, Logical>>,
}

impl WindowOffscreenCache {
    #[inline]
    pub fn matches_size(&self, width: i32, height: i32) -> bool {
        self.key.width == width && self.key.height == height
    }

    #[inline]
    pub fn set_size(&mut self, width: i32, height: i32) {
        self.key = WindowOffscreenKey { width, height };
        self.texture = None;
        self.bbox = None;
    }

    #[inline]
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    #[inline]
    pub fn mark_clean(&mut self, now: Instant) {
        self.dirty = false;
        self.last_used_at = Some(now);
    }

    #[inline]
    pub fn touch(&mut self, now: Instant) {
        self.last_used_at = Some(now);
    }
}

#[derive(Clone)]
pub(crate) struct NodeAppIconTexture {
    pub texture: GlesTexture,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone)]
pub(crate) enum NodeAppIconCacheEntry {
    Ready(NodeAppIconTexture),
    Missing,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FullscreenSessionEntry {
    pub pos: Vec2,
    pub size: Vec2,
    pub pinned: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FullscreenMotion {
    pub from: Vec2,
    pub to: Vec2,
    pub start_ms: u64,
    pub duration_ms: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FullscreenScaleAnim {
    pub start_ms: u64,
    pub duration_ms: u64,
}

pub struct HalleyWlState {
    pub display_handle: DisplayHandle,
    pub compositor_state: CompositorState,
    pub viewporter_state: ViewporterState,
    pub xdg_shell_state: XdgShellState,
    pub popup_manager: PopupManager,
    pub wlr_layer_shell_state: WlrLayerShellState,
    pub output_manager_state: OutputManagerState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub data_control_state: DataControlState,
    pub seat: Seat<Self>,
    pub primary_output: Option<Output>,
    pub layer_keyboard_focus: Option<ObjectId>,

    pub field: Field,
    pub viewport: Viewport,
    pub tuning: RuntimeTuning,
    pub zoom_ref_size: Vec2,
    pub cursor_image_status: CursorImageStatus,
    pub(crate) dmabuf_importer: Option<Rc<dyn DmabufImportBackend>>,

    pub surface_activity: HashMap<ObjectId, CommitActivity>,
    pub surface_to_node: HashMap<ObjectId, NodeId>,
    pub(crate) node_app_ids: HashMap<NodeId, String>,
    pub(crate) node_app_icon_cache: HashMap<String, NodeAppIconCacheEntry>,
    pub(crate) zoom_nominal_size: HashMap<NodeId, Vec2>,
    pub(crate) zoom_resize_fallback: HashSet<NodeId>,
    pub(crate) zoom_resize_reject_streak: HashMap<NodeId, u8>,
    pub(crate) zoom_last_observed_size: HashMap<NodeId, Vec2>,
    pub(crate) zoom_resize_static_streak: HashMap<NodeId, u8>,
    pub animator: Animator,
    pub interaction_focus: Option<NodeId>,
    pub(crate) interaction_focus_until_ms: u64,
    pub(crate) last_surface_focus_ms: HashMap<NodeId, u64>,
    pub pan_restore_active_focus: Option<NodeId>,
    pub(crate) app_focused: bool,
    pub(crate) cluster_form_state: ClusterFormationState,
    pub(crate) active_cluster_workspace: Option<ClusterId>,
    pub(crate) workspace_hidden_nodes: Vec<NodeId>,
    pub(crate) workspace_prev_viewport: Option<Viewport>,
    pub(crate) last_active_size: HashMap<NodeId, Vec2>,
    pub pending_spawn_activate_at_ms: HashMap<NodeId, u64>,
    pub(crate) active_transition_until_ms: HashMap<NodeId, u64>,
    pub(crate) primary_promote_cooldown_until_ms: HashMap<NodeId, u64>,

    pub(crate) dock_decay_offscreen_since_ms: HashMap<NodeId, u64>,
    pub(crate) carry_zone_hint: HashMap<NodeId, FocusZone>,
    pub(crate) carry_zone_last_change_ms: HashMap<NodeId, u64>,
    pub(crate) carry_zone_pending: HashMap<NodeId, FocusZone>,
    pub(crate) carry_zone_pending_since_ms: HashMap<NodeId, u64>,
    pub(crate) carry_activation_anim_armed: HashSet<NodeId>,
    pub(crate) carry_direct_nodes: HashSet<NodeId>,

    // Nodes explicitly collapsed by the user via keybind/toggle.
    // Maintenance must not auto-resurrect these.
    pub(crate) manual_collapsed_nodes: HashSet<NodeId>,
    pub(crate) docking_hold_count: u32,

    pub(crate) resize_active: Option<NodeId>,
    pub(crate) resize_static_node: Option<NodeId>,
    pub(crate) resize_static_lock_pos: Option<Vec2>,
    pub(crate) resize_static_until_ms: u64,
    pub(crate) suspend_overlap_resolve: bool,
    pub(crate) suspend_state_checks: bool,
    pub(crate) smoothed_render_pos: HashMap<NodeId, Vec2>,
    pub(crate) node_hover_mix: HashMap<NodeId, f32>,
    pub(crate) node_preview_hover_node: Option<NodeId>,
    pub(crate) node_preview_hover_mix: f32,
    pub(crate) render_last_tick: Instant,
    pub(crate) viewport_pan_anim: Option<ViewportPanAnim>,
    pub(crate) pan_dominant_until_ms: u64,
    pub(crate) exit_requested: bool,

    pub(crate) bbox_loc: HashMap<NodeId, (f32, f32)>,
    pub(crate) window_geometry: HashMap<NodeId, (f32, f32, f32, f32)>,
    pub(crate) recent_top_node: Option<NodeId>,
    pub(crate) recent_top_until: Option<Instant>,
    pub(crate) window_offscreen_cache: HashMap<NodeId, WindowOffscreenCache>,
    pub(crate) node_circle_texture: Option<GlesTexture>,
    pub(crate) node_squircle_program: Option<GlesTexProgram>,
    pub(crate) node_label_program: Option<GlesTexProgram>,
    pub(crate) fullscreen_active_node: Option<NodeId>,
    pub(crate) fullscreen_restore: HashMap<NodeId, FullscreenSessionEntry>,
    pub(crate) fullscreen_motion: HashMap<NodeId, FullscreenMotion>,
    pub(crate) fullscreen_scale_anim: HashMap<NodeId, FullscreenScaleAnim>,

    pub(crate) spawn_cursor: u32,
    pub(crate) spawn_patch: Option<SpawnPatch>,
    pub(crate) spawn_anchor_mode: SpawnAnchorMode,
    pub(crate) spawn_view_anchor: Vec2,
    pub(crate) spawn_pan_start_center: Option<Vec2>,
    pub(crate) spawn_last_pan_ms: u64,
    pub(crate) pending_spawn_pan_queue: VecDeque<PendingSpawnPan>,
    pub(crate) active_spawn_pan: Option<ActiveSpawnPan>,
    pub(crate) started_at: Instant,
    pub(crate) last_debug_dump_at: Instant,
    pub(crate) maintenance_dirty: bool,
    pub(crate) maintenance_ping: Option<Ping>,

    pub(crate) spawned_children: Vec<std::process::Child>,
}

impl HalleyWlState {
    pub(crate) fn preserve_collapsed_surface(&self, id: NodeId) -> bool {
        self.manual_collapsed_nodes.contains(&id)
            || self.field.node(id).is_some_and(|n| {
                n.kind == halley_core::field::NodeKind::Surface
                    && n.state == halley_core::field::NodeState::Node
            })
    }

    pub fn new(
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        tuning: RuntimeTuning,
    ) -> Self {
        let now = Instant::now();
        let initial_view_anchor = tuning.viewport_center;
        let mut seat_state = SeatState::new();
        let seat = seat_state.new_wl_seat(dh, "halley");
        let primary_selection_state = PrimarySelectionState::new::<HalleyWlState>(dh);
        let data_control_state =
            DataControlState::new::<HalleyWlState, _>(dh, Some(&primary_selection_state), |_| true);
        let mut out = Self {
            display_handle: dh.clone(),
            compositor_state: CompositorState::new::<HalleyWlState>(dh),
            viewporter_state: ViewporterState::new::<HalleyWlState>(dh),
            xdg_shell_state: XdgShellState::new::<HalleyWlState>(dh),
            popup_manager: PopupManager::default(),
            wlr_layer_shell_state: WlrLayerShellState::new::<HalleyWlState>(dh),
            output_manager_state: OutputManagerState::new_with_xdg_output::<HalleyWlState>(dh),
            shm_state: ShmState::new::<HalleyWlState>(dh, vec![]),
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
            seat_state,
            data_device_state: DataDeviceState::new::<HalleyWlState>(dh),
            primary_selection_state,
            data_control_state,
            seat,
            primary_output: None,
            layer_keyboard_focus: None,

            field: Field::new(),
            viewport: tuning.viewport(),
            zoom_ref_size: tuning.viewport_size,
            cursor_image_status: CursorImageStatus::default_named(),
            dmabuf_importer: None,
            tuning,

            surface_activity: HashMap::new(),
            surface_to_node: HashMap::new(),
            node_app_ids: HashMap::new(),
            node_app_icon_cache: HashMap::new(),
            zoom_nominal_size: HashMap::new(),
            zoom_resize_fallback: HashSet::new(),
            zoom_resize_reject_streak: HashMap::new(),
            zoom_last_observed_size: HashMap::new(),
            zoom_resize_static_streak: HashMap::new(),
            animator: Animator::new(now),
            interaction_focus: None,
            interaction_focus_until_ms: 0,
            last_surface_focus_ms: HashMap::new(),
            pan_restore_active_focus: None,
            app_focused: true,
            cluster_form_state: ClusterFormationState::default(),
            active_cluster_workspace: None,
            workspace_hidden_nodes: Vec::new(),
            workspace_prev_viewport: None,
            last_active_size: HashMap::new(),
            pending_spawn_activate_at_ms: HashMap::new(),
            active_transition_until_ms: HashMap::new(),
            primary_promote_cooldown_until_ms: HashMap::new(),
            dock_decay_offscreen_since_ms: HashMap::new(),
            carry_zone_hint: HashMap::new(),
            carry_zone_last_change_ms: HashMap::new(),
            carry_zone_pending: HashMap::new(),
            carry_zone_pending_since_ms: HashMap::new(),
            carry_activation_anim_armed: HashSet::new(),
            carry_direct_nodes: HashSet::new(),
            manual_collapsed_nodes: HashSet::new(),
            docking_hold_count: 0,
            resize_active: None,
            resize_static_node: None,
            resize_static_lock_pos: None,
            resize_static_until_ms: 0,
            suspend_overlap_resolve: false,
            suspend_state_checks: false,
            smoothed_render_pos: HashMap::new(),
            node_hover_mix: HashMap::new(),
            node_preview_hover_node: None,
            node_preview_hover_mix: 0.0,
            render_last_tick: now,
            viewport_pan_anim: None,
            pan_dominant_until_ms: 0,
            exit_requested: false,

            bbox_loc: HashMap::new(),
            window_geometry: HashMap::new(),
            recent_top_node: None,
            recent_top_until: None,
            window_offscreen_cache: HashMap::new(),
            node_circle_texture: None,
            node_squircle_program: None,
            node_label_program: None,
            fullscreen_active_node: None,
            fullscreen_restore: HashMap::new(),
            fullscreen_motion: HashMap::new(),
            fullscreen_scale_anim: HashMap::new(),

            spawn_cursor: 0,
            spawn_patch: None,
            spawn_anchor_mode: SpawnAnchorMode::Focus,
            spawn_view_anchor: initial_view_anchor,
            spawn_pan_start_center: None,
            spawn_last_pan_ms: 0,
            pending_spawn_pan_queue: VecDeque::new(),
            active_spawn_pan: None,
            started_at: now,
            last_debug_dump_at: now,
            maintenance_dirty: true,
            maintenance_ping: None,

            spawned_children: Vec::new(),
        };
        out.animator.set_spec(AnimSpec {
            state_change_ms: out.tuning.dev_anim_state_change_ms,
            bounce: out.tuning.dev_anim_bounce,
        });
        out
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
                    .create_global_with_default_feedback::<HalleyWlState>(
                        &self.display_handle,
                        &feedback,
                    )
            }
            None => self
                .dmabuf_state
                .create_global::<HalleyWlState>(&self.display_handle, formats.iter().copied()),
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

    pub(crate) fn advertise_primary_output(&mut self, name: &str, mode: OutputMode) {
        let output = self.primary_output.get_or_insert_with(|| {
            let output = Output::new(
                name.to_string(),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "halley".to_string(),
                    model: name.to_string(),
                },
            );
            let _ = output.create_global::<HalleyWlState>(&self.display_handle);
            output
        });

        output.add_mode(mode);
        output.set_preferred(mode);
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
    }

    pub fn set_recent_top_node(&mut self, node_id: NodeId, until: Instant) {
        self.recent_top_node = Some(node_id);
        self.recent_top_until = Some(until);
    }

    pub fn recent_top_node_active(&mut self, now: Instant) -> Option<NodeId> {
        if self.recent_top_until.is_some_and(|until| now >= until) {
            self.recent_top_node = None;
            self.recent_top_until = None;
            return None;
        }
        self.recent_top_node
    }

    pub(crate) fn ensure_window_offscreen_cache(
        &mut self,
        node_id: NodeId,
        width: i32,
        height: i32,
        now: Instant,
    ) -> &mut WindowOffscreenCache {
        let cache = self.window_offscreen_cache.entry(node_id).or_default();

        let width = width.max(1);
        let height = height.max(1);

        if !cache.matches_size(width, height) {
            cache.set_size(width, height);
            cache.mark_dirty();
        }

        cache.touch(now);
        cache
    }

    pub(crate) fn mark_window_offscreen_dirty(&mut self, node_id: NodeId) {
        if let Some(cache) = self.window_offscreen_cache.get_mut(&node_id) {
            cache.mark_dirty();
        }
    }

    pub(crate) fn clear_window_offscreen_cache_for(&mut self, node_id: NodeId) {
        self.window_offscreen_cache.remove(&node_id);
    }

    pub(crate) fn prune_window_offscreen_cache(&mut self, now: Instant) {
        let alive: HashSet<NodeId> = self.field.nodes().keys().copied().collect();
        self.window_offscreen_cache.retain(|id, cache| {
            alive.contains(id)
                && cache
                    .last_used_at
                    .is_none_or(|t| now.saturating_duration_since(t).as_secs() < 5)
        });
    }

    pub fn request_exit(&mut self) {
        self.exit_requested = true;
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
        if !self.app_focused {
            return None;
        }

        let now_ms = self.now_ms(now);
        let mut next_ms: Option<u64> = None;
        let mut consider = |at_ms: u64| {
            next_ms = Some(next_ms.map_or(at_ms, |cur| cur.min(at_ms)));
        };

        if self.interaction_focus.is_some() && self.interaction_focus_until_ms > now_ms {
            consider(self.interaction_focus_until_ms);
        }
        if self.resize_static_node.is_some() && self.resize_static_until_ms > now_ms {
            consider(self.resize_static_until_ms);
        }
        if let Some(at_ms) = self.pending_spawn_activate_at_ms.values().copied().min()
            && at_ms > now_ms
        {
            consider(at_ms);
        }
        if let Some(at_ms) = self.active_transition_until_ms.values().copied().min()
            && at_ms > now_ms
        {
            consider(at_ms);
        }
        if let Some(at_ms) = self
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
        if !self.app_focused {
            return;
        }
        self.reconcile_surface_bindings();
        let now_ms = now.duration_since(self.started_at).as_millis() as u64;
        let _ = self.recent_top_node_active(now);
        if self.active_cluster_workspace.is_some() {
            self.layout_active_cluster_workspace(now_ms);
            self.animator.observe_field(&self.field, now);
            return;
        }
        if let Some(fid) = self.interaction_focus
            && now_ms >= self.interaction_focus_until_ms
        {
            let keep = self.field.node(fid).is_some_and(|n| {
                self.field.is_visible(fid) && n.kind == halley_core::field::NodeKind::Surface
            });
            if keep {
                self.interaction_focus_until_ms = now_ms.saturating_add(30_000);
            } else {
                self.set_interaction_focus(None, 0, now);
            }
        }
        if self.interaction_focus.is_none() && self.layer_keyboard_focus.is_some() {
            self.reassert_layer_surface_keyboard_focus_if_drifted();
        }
        self.active_transition_until_ms
            .retain(|_, &mut until| until > now_ms);
        self.primary_promote_cooldown_until_ms
            .retain(|_, &mut until| until > now_ms);
        let alive_ids: HashSet<NodeId> = self.field.nodes().keys().copied().collect();
        self.carry_zone_hint.retain(|id, _| alive_ids.contains(id));
        self.carry_zone_last_change_ms
            .retain(|id, _| alive_ids.contains(id));
        self.carry_zone_pending
            .retain(|id, _| alive_ids.contains(id));
        self.carry_zone_pending_since_ms
            .retain(|id, _| alive_ids.contains(id));
        self.carry_activation_anim_armed
            .retain(|id| alive_ids.contains(id));
        self.last_surface_focus_ms
            .retain(|id, _| alive_ids.contains(id));
        self.manual_collapsed_nodes
            .retain(|id| alive_ids.contains(id));

        self.process_pending_spawn_activations(now, now_ms);
        let resize_settling = self
            .resize_static_node
            .is_some_and(|_| now_ms < self.resize_static_until_ms);
        if resize_settling
            && let (Some(id), Some(lock_pos)) =
                (self.resize_static_node, self.resize_static_lock_pos)
            && let Some(n) = self.field.node(id)
            && ((n.pos.x - lock_pos.x).abs() > 0.05 || (n.pos.y - lock_pos.y).abs() > 0.05)
        {
            let _ = self.field.carry(id, lock_pos);
        }
        if self
            .resize_static_node
            .is_some_and(|_| now_ms >= self.resize_static_until_ms)
        {
            self.resize_static_node = None;
            self.resize_static_lock_pos = None;
            self.resize_static_until_ms = 0;
        }
        let focus_ring = self.active_focus_ring();
        if !self.suspend_state_checks {
            self.enforce_pan_dominant_zone_states(focus_ring, now_ms);
            self.enforce_carry_zone_states();
        }
        if let Some(id) = self.resize_active {
            let _ = self.field.touch(id, now_ms);
            let _ = self.field.set_decay_level(id, DecayLevel::Hot);
        }
        if self.resize_active.is_none()
            && !(self.resize_static_node.is_some() && now_ms < self.resize_static_until_ms)
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
            &mut self.cluster_form_state,
        );
        self.enforce_single_primary_active_unit(focus_ring);
        if !self.suspend_state_checks && self.resize_active.is_none() {
            self.resolve_surface_overlap();
        }
        self.restore_pan_return_active_focus(now);
        self.animator.observe_field(&self.field, now);

        if self.tuning.debug_tick_dump
            && now.duration_since(self.last_debug_dump_at).as_millis() as u64
                >= self.tuning.debug_dump_every_ms
        {
            self.debug_dump();
            self.last_debug_dump_at = now;
        }
    }
}

impl Drop for HalleyWlState {
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

delegate_dmabuf!(HalleyWlState);
