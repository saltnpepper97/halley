use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use halley_core::cluster::ClusterId;
use halley_config::CompositorBindingAction;
use halley_config::PointerBindingAction;

#[derive(Default, Clone)]
pub(crate) struct ModState {
    pub(crate) super_down: bool,
    pub(crate) left_super_down: bool,
    pub(crate) right_super_down: bool,
    pub(crate) alt_down: bool,
    pub(crate) left_alt_down: bool,
    pub(crate) right_alt_down: bool,
    pub(crate) ctrl_down: bool,
    pub(crate) left_ctrl_down: bool,
    pub(crate) right_ctrl_down: bool,
    pub(crate) shift_down: bool,
    pub(crate) left_shift_down: bool,
    pub(crate) right_shift_down: bool,
    /// Keys whose press was intercepted by the compositor (not forwarded to
    /// clients). The matching release must also be intercepted so clients never
    /// receive an unpaired release event and end up with stuck keys.
    pub(crate) intercepted_keys: HashSet<u32>,
    pub(crate) intercepted_compositor_actions: HashMap<u32, CompositorBindingAction>,
}

#[derive(Clone, Copy)]
pub(crate) enum DragAxisMode {
    Free,
    EdgePanNeg,
    EdgePanPos,
}

impl DragAxisMode {
    pub(crate) fn sign(self) -> f32 {
        match self {
            Self::Free => 0.0,
            Self::EdgePanNeg => -1.0,
            Self::EdgePanPos => 1.0,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DragCtx {
    pub(crate) node_id: halley_core::field::NodeId,
    pub(crate) allow_monitor_transfer: bool,
    pub(crate) edge_pan_eligible: bool,
    pub(crate) current_offset: halley_core::field::Vec2,
    pub(crate) center_latched: bool,
    pub(crate) started_active: bool,
    pub(crate) edge_pan_x: DragAxisMode,
    pub(crate) edge_pan_y: DragAxisMode,
    pub(crate) edge_pan_pressure: halley_core::field::Vec2,
    pub(crate) last_pointer_world: halley_core::field::Vec2,
    pub(crate) last_update_at: Instant,
    pub(crate) release_velocity: halley_core::field::Vec2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeHandle {
    /// Handle not yet committed. Waiting for the pointer to exceed the dead
    /// zone so the drag direction can be used to lock an octant.
    Pending,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy)]
pub(crate) struct ResizeCtx {
    pub(crate) node_id: halley_core::field::NodeId,
    pub(crate) start_surface_w: i32,
    pub(crate) start_surface_h: i32,
    pub(crate) start_bbox_w: i32,
    pub(crate) start_bbox_h: i32,
    pub(crate) start_visual_w: i32,
    pub(crate) start_visual_h: i32,
    pub(crate) start_geo_lx: f32,
    pub(crate) start_geo_ly: f32,
    pub(crate) start_left_px: f32,
    pub(crate) start_right_px: f32,
    pub(crate) start_top_px: f32,
    pub(crate) start_bottom_px: f32,
    pub(crate) preview_left_px: f32,
    pub(crate) preview_right_px: f32,
    pub(crate) preview_top_px: f32,
    pub(crate) preview_bottom_px: f32,
    pub(crate) last_sent_w: i32,
    pub(crate) last_sent_h: i32,
    pub(crate) last_configure_at: Instant,
    /// Which edge/corner is active. Starts as `Pending` for interior/binding
    /// grabs and is locked once the pointer exceeds the dead zone. Edge grabs
    /// commit immediately at press time.
    pub(crate) handle: ResizeHandle,
    pub(crate) press_sx: f32,
    pub(crate) press_sy: f32,
    /// Signed per-edge weights set at commit time. The preview rect is updated
    /// each frame as:
    ///
    ///   new_left   = start_left   + h_weight_left  * dx
    ///   new_right  = start_right  + h_weight_right * dx
    ///   new_top    = start_top    + v_weight_top   * dy
    ///   new_bottom = start_bottom + v_weight_bottom * dy
    ///
    /// Anchored edges have weight 0.0. Moving edges have weight +1.0 (tracks
    /// pointer directly) or -1.0 (moves opposite — used for left/top edges so
    /// that dragging right/down on those edges correctly grows the window from
    /// the opposite side). Zero-weight edges on both sides of an axis means
    /// that axis is not resized (e.g. a pure Left grab doesn't touch height).
    pub(crate) h_weight_left: f32,
    pub(crate) h_weight_right: f32,
    pub(crate) v_weight_top: f32,
    pub(crate) v_weight_bottom: f32,
    /// True once the handle has been committed and motion is live.
    pub(crate) drag_started: bool,
    pub(crate) resize_mode_sent: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct NodeMoveAnim {
    pub(crate) node_id: halley_core::field::NodeId,
    pub(crate) from: halley_core::field::Vec2,
    pub(crate) to: halley_core::field::Vec2,
    pub(crate) started_at: Instant,
    pub(crate) duration: Duration,
}

#[derive(Clone, Copy)]
pub(crate) struct TitleClickCtx {
    pub(crate) node_id: halley_core::field::NodeId,
    pub(crate) at: Instant,
}

#[derive(Clone)]
pub(crate) struct BloomDragCtx {
    pub(crate) cluster_id: ClusterId,
    pub(crate) member_id: halley_core::field::NodeId,
    pub(crate) monitor: String,
    pub(crate) core_screen: (f32, f32),
}

#[derive(Clone, Copy)]
pub(crate) struct HitNode {
    pub(crate) node_id: halley_core::field::NodeId,
    pub(crate) on_titlebar: bool,
    pub(crate) is_core: bool,
}

pub(crate) const NODE_DOUBLE_CLICK_MS: u64 = 350;

#[derive(Clone)]
pub(crate) struct PointerState {
    pub(crate) world: halley_core::field::Vec2,
    pub(crate) screen: (f32, f32),
    pub(crate) workspace_size: (i32, i32),
    pub(crate) hover_node: Option<halley_core::field::NodeId>,
    /// Pointer buttons whose press was intercepted by the compositor. The
    /// matching release must also be intercepted so clients do not receive a
    /// stray release after a compositor-owned drag/resize gesture.
    pub(crate) intercepted_buttons: HashMap<u32, PointerBindingAction>,
    /// Non-pointer-binding buttons intercepted by compositor/launch bindings.
    /// Their releases must also be intercepted so clients never see an
    /// unpaired button release.
    pub(crate) intercepted_binding_buttons: HashSet<u32>,
    pub(crate) drag: Option<DragCtx>,
    pub(crate) resize: Option<ResizeCtx>,
    pub(crate) move_anim: HashMap<halley_core::field::NodeId, NodeMoveAnim>,
    pub(crate) bloom_drag: Option<BloomDragCtx>,
    pub(crate) last_title_click: Option<TitleClickCtx>,
    pub(crate) panning: bool,
    pub(crate) pan_monitor: Option<String>,
    pub(crate) pan_last_screen: (f32, f32),
    pub(crate) hover_started_at: Option<Instant>,
    pub(crate) preview_block_until: Option<Instant>,
    pub(crate) resize_trace_node: Option<halley_core::field::NodeId>,
    pub(crate) resize_trace_until: Option<Instant>,
    pub(crate) resize_trace_last_at: Option<Instant>,
}

impl Default for PointerState {
    fn default() -> Self {
        Self {
            world: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            screen: (0.0, 0.0),
            workspace_size: (1, 1),
            hover_node: None,
            intercepted_buttons: HashMap::new(),
            intercepted_binding_buttons: HashSet::new(),
            drag: None,
            resize: None,
            move_anim: HashMap::new(),
            bloom_drag: None,
            last_title_click: None,
            panning: false,
            pan_monitor: None,
            pan_last_screen: (0.0, 0.0),
            hover_started_at: None,
            preview_block_until: None,
            resize_trace_node: None,
            resize_trace_until: None,
            resize_trace_last_at: None,
        }
    }
}
