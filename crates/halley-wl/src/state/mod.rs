use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_core::cluster::{ActiveLayoutMode, ClusterId};
use halley_core::cluster_policy::{ClusterFormationState, ClusterPolicy, tick_cluster_formation};
use halley_core::decay::DecayLevel;
use halley_core::field::{Field, NodeId, Vec2, Visibility};
use halley_core::viewport::{FocusRings, RingZone, Viewport};
use halley_config::RuntimeTuning;

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_seat, delegate_shm, delegate_xdg_shell,
    input::{Seat, SeatHandler, SeatState, pointer::CursorImageStatus},
    reexports::wayland_server::{Client, Resource, backend::ObjectId, protocol::wl_seat},
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState},
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};

use crate::activity::CommitActivity;
use crate::anim::{AnimSpec, Animator};
mod carry;
mod client;
mod focus;
mod maintenance;
mod overlap;
mod overview;
mod render_state;
mod runtime_state;
mod surface_lifecycle;
mod wayland_handlers;
mod workspace;
mod zoom;
pub(crate) use carry::DockSide;
use carry::{DockLink, DockPending};
pub use client::ClientState;
use focus::ViewportPanAnim;
use overview::OverviewAnim;

pub struct HalleyWlState {
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub seat: Seat<Self>,

    pub field: Field,
    pub viewport: Viewport,
    pub tuning: RuntimeTuning,
    pub zoom_ref_size: Vec2,
    pub cursor_image_status: CursorImageStatus,

    /// wl_surface object id -> commit activity
    pub surface_activity: HashMap<ObjectId, CommitActivity>,
    /// wl_surface object id -> NodeId
    pub surface_to_node: HashMap<ObjectId, NodeId>,
    /// Node baseline size used for zoom-driven live toplevel resizing.
    zoom_nominal_size: HashMap<NodeId, Vec2>,
    /// Nodes whose client doesn't reliably apply configure-driven zoom resizing.
    zoom_resize_fallback: HashSet<NodeId>,
    /// Consecutive misses where node size stayed far from requested zoom target.
    zoom_resize_reject_streak: HashMap<NodeId, u8>,
    /// Last observed client-reported size per active surface node.
    zoom_last_observed_size: HashMap<NodeId, Vec2>,
    /// Consecutive ticks with effectively unchanged client size while zoom target differs.
    zoom_resize_static_streak: HashMap<NodeId, u8>,
    pub animator: Animator,
    pub interaction_focus: Option<NodeId>,
    interaction_focus_until_ms: u64,
    last_surface_focus_ms: HashMap<NodeId, u64>,
    pan_restore_active_focus: Option<NodeId>,
    app_focused: bool,
    cluster_form_state: ClusterFormationState,
    active_cluster_workspace: Option<ClusterId>,
    workspace_hidden_nodes: Vec<NodeId>,
    workspace_prev_viewport: Option<Viewport>,
    overview_mode: bool,
    overview_saved_viewport: Option<Viewport>,
    overview_saved_positions: HashMap<NodeId, Vec2>,
    overview_saved_states: HashMap<NodeId, halley_core::field::NodeState>,
    overview_anim: Option<OverviewAnim>,
    last_active_size: HashMap<NodeId, Vec2>,
    pending_spawn_activate_at_ms: HashMap<NodeId, u64>,
    active_transition_until_ms: HashMap<NodeId, u64>,
    primary_promote_cooldown_until_ms: HashMap<NodeId, u64>,
    carry_zone_hint: HashMap<NodeId, RingZone>,
    carry_zone_last_change_ms: HashMap<NodeId, u64>,
    carry_zone_pending: HashMap<NodeId, RingZone>,
    carry_zone_pending_since_ms: HashMap<NodeId, u64>,
    carry_activation_anim_armed: HashSet<NodeId>,
    docked_links: HashMap<NodeId, DockLink>,
    dock_pending: Option<DockPending>,
    resize_active: Option<NodeId>,
    resize_static_node: Option<NodeId>,
    resize_static_lock_pos: Option<Vec2>,
    resize_static_until_ms: u64,
    suspend_overlap_resolve: bool,
    suspend_state_checks: bool,
    smoothed_render_pos: HashMap<NodeId, Vec2>,
    node_hover_mix: HashMap<NodeId, f32>,
    node_preview_hover_node: Option<NodeId>,
    node_preview_hover_mix: f32,
    render_last_tick: Instant,
    viewport_pan_anim: Option<ViewportPanAnim>,
    pan_dominant_until_ms: u64,
    exit_requested: bool,

    spawn_cursor: u32,
    started_at: Instant,
    last_debug_dump_at: Instant,
}

impl HalleyWlState {
    pub fn new(
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        tuning: RuntimeTuning,
    ) -> Self {
        let now = Instant::now();
        let mut seat_state = SeatState::new();
        let seat = seat_state.new_wl_seat(dh, "halley");
        let mut out = Self {
            compositor_state: CompositorState::new::<HalleyWlState>(dh),
            xdg_shell_state: XdgShellState::new::<HalleyWlState>(dh),
            shm_state: ShmState::new::<HalleyWlState>(dh, vec![]),
            seat_state,
            data_device_state: DataDeviceState::new::<HalleyWlState>(dh),
            seat,

            field: Field::new(),
            viewport: tuning.viewport(),
            zoom_ref_size: tuning.viewport_size,
            cursor_image_status: CursorImageStatus::default_named(),
            tuning,

            surface_activity: HashMap::new(),
            surface_to_node: HashMap::new(),
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
            overview_mode: false,
            overview_saved_viewport: None,
            overview_saved_positions: HashMap::new(),
            overview_saved_states: HashMap::new(),
            overview_anim: None,
            last_active_size: HashMap::new(),
            pending_spawn_activate_at_ms: HashMap::new(),
            active_transition_until_ms: HashMap::new(),
            primary_promote_cooldown_until_ms: HashMap::new(),
            carry_zone_hint: HashMap::new(),
            carry_zone_last_change_ms: HashMap::new(),
            carry_zone_pending: HashMap::new(),
            carry_zone_pending_since_ms: HashMap::new(),
            carry_activation_anim_armed: HashSet::new(),
            docked_links: HashMap::new(),
            dock_pending: None,
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

            spawn_cursor: 0,
            started_at: now,
            last_debug_dump_at: now,
        };
        out.animator.set_spec(AnimSpec {
            state_change_ms: out.tuning.dev_anim_state_change_ms,
            bounce: out.tuning.dev_anim_bounce,
        });
        out
    }

    pub fn request_exit(&mut self) {
        self.exit_requested = true;
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_requested
    }

    #[inline]
    pub fn tick_maintenance(&mut self, now: Instant) {
        if !self.app_focused {
            return;
        }
        self.reconcile_surface_bindings();
        let now_ms = now.duration_since(self.started_at).as_millis() as u64;
        self.tick_overview_animation(now_ms);
        self.tick_viewport_pan_animation(now_ms);
        if self.overview_mode {
            // Overview mode is authoritative: don't let decay/rings/resize logic
            // pull nodes back into Active while in the overview workspace.
            self.animator.observe_field(&self.field, now);
            return;
        }
        if self.active_cluster_workspace.is_some() {
            self.layout_active_cluster_workspace(now_ms);
            self.animator.observe_field(&self.field, now);
            return;
        }
        if let Some(fid) = self.interaction_focus {
            if now_ms >= self.interaction_focus_until_ms {
                // Keep keyboard focus sticky while the focused surface still exists/visible.
                // This mirrors the expected compositor behavior: focus only changes due to
                // explicit focus operations, not passive timer expiry.
                let keep = self.field.node(fid).is_some_and(|n| {
                    self.field.is_visible(fid) && n.kind == halley_core::field::NodeKind::Surface
                });
                if keep {
                    self.interaction_focus_until_ms = now_ms.saturating_add(30_000);
                } else {
                    self.set_interaction_focus(None, 0, now);
                }
            }
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
        self.process_pending_spawn_activations(now, now_ms);
        let resize_settling = self
            .resize_static_node
            .is_some_and(|_| now_ms < self.resize_static_until_ms);
        if resize_settling {
            if let (Some(id), Some(lock_pos)) =
                (self.resize_static_node, self.resize_static_lock_pos)
            {
                if let Some(n) = self.field.node(id) {
                    if (n.pos.x - lock_pos.x).abs() > 0.05 || (n.pos.y - lock_pos.y).abs() > 0.05 {
                        let _ = self.field.carry(id, lock_pos);
                    }
                }
            }
        }
        if self
            .resize_static_node
            .is_some_and(|_| now_ms >= self.resize_static_until_ms)
        {
            self.resize_static_node = None;
            self.resize_static_lock_pos = None;
            self.resize_static_until_ms = 0;
        }
        if resize_settling {
            self.animator.observe_field(&self.field, now);
            return;
        }
        let rings = self.active_rings();
        let pan_dominant = now_ms < self.pan_dominant_until_ms;
        if !self.suspend_state_checks {
            // Single source of truth for runtime state transitions.
            // Avoid mixing ring-decay + promote-center policies, which can
            // issue contradictory Hot/Cold writes near boundaries.
            self.enforce_pan_dominant_zone_states(rings, now_ms);
            self.enforce_carry_zone_states();
        }
        if let Some(id) = self.resize_active {
            let _ = self.field.touch(id, now_ms);
            let _ = self.field.set_decay_level(id, DecayLevel::Hot);
        }
        // State updates are handled by a single ring policy path above.
        // Never run zoom-driven client resize while interactive resize is active
        // (or in its short static cooldown), otherwise two configure sources
        // fight and cause visible drift/jitter at some size thresholds.
        if self.resize_active.is_none()
            && !(self.resize_static_node.is_some() && now_ms < self.resize_static_until_ms)
        {
            self.update_zoom_live_surface_sizes();
        }
        // TEMP safety rollback: keep runtime cluster auto-formation disabled while we
        // stabilize app-mapping/rendering behavior.
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
        if !self.suspend_state_checks
            && self.resize_active.is_none()
            && !(self.resize_static_node.is_some() && now_ms < self.resize_static_until_ms)
        {
            self.enforce_docked_pairs();
        }
        self.enforce_single_primary_active_unit(rings);
        if !self.suspend_state_checks
            && self.resize_active.is_none()
            && !(self.resize_static_node.is_some() && now_ms < self.resize_static_until_ms)
        {
            self.resolve_surface_overlap();
        }
        if !self.suspend_state_checks && !pan_dominant {
            self.decay_tiny_nodes_on_zoom_out();
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
