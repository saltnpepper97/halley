use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

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
pub(crate) struct DragCtx {
    pub(crate) node_id: halley_core::field::NodeId,
    pub(crate) current_offset: halley_core::field::Vec2,
    pub(crate) center_latched: bool,
    pub(crate) started_active: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ResizeHandle {
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
    pub(crate) handle: ResizeHandle,
    pub(crate) press_sx: f32,
    pub(crate) press_sy: f32,
    pub(crate) press_off_left_px: f32,
    pub(crate) press_off_right_px: f32,
    pub(crate) press_off_top_px: f32,
    pub(crate) press_off_bottom_px: f32,
    pub(crate) drag_started: bool,
    pub(crate) resize_mode_sent: bool,
    // Updated on every client commit so the render path always has a
    // single source of truth for what's actually been painted.
    pub(crate) live_geo_lx: f32,
    pub(crate) live_geo_ly: f32,
    pub(crate) live_geo_w: f32,
    pub(crate) live_geo_h: f32,
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
    pub(crate) drag: Option<DragCtx>,
    pub(crate) resize: Option<ResizeCtx>,
    pub(crate) move_anim: HashMap<halley_core::field::NodeId, NodeMoveAnim>,
    pub(crate) last_title_click: Option<TitleClickCtx>,
    pub(crate) panning: bool,
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
            drag: None,
            resize: None,
            move_anim: HashMap::new(),
            last_title_click: None,
            panning: false,
            pan_last_screen: (0.0, 0.0),
            hover_started_at: None,
            preview_block_until: None,
            resize_trace_node: None,
            resize_trace_until: None,
            resize_trace_last_at: None,
        }
    }
}
