use std::time::Instant;

use halley_core::field::NodeId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeHandle {
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
    pub(crate) node_id: NodeId,
    pub(crate) workspace_w: i32,
    pub(crate) workspace_h: i32,
    pub(crate) start_surface_w: i32,
    pub(crate) start_surface_h: i32,
    pub(crate) start_bbox_w: i32,
    pub(crate) start_bbox_h: i32,
    pub(crate) start_visual_w: i32,
    pub(crate) start_visual_h: i32,
    pub(crate) start_geo_lx: f32,
    pub(crate) start_geo_ly: f32,
    pub(crate) start_geo_inset_x: i32,
    pub(crate) start_geo_inset_y: i32,
    pub(crate) start_left_px: f32,
    pub(crate) start_right_px: f32,
    pub(crate) start_top_px: f32,
    pub(crate) start_bottom_px: f32,
    pub(crate) preview_left_px: f32,
    pub(crate) preview_right_px: f32,
    pub(crate) preview_top_px: f32,
    pub(crate) preview_bottom_px: f32,
    pub(crate) target_left_px: f32,
    pub(crate) target_right_px: f32,
    pub(crate) target_top_px: f32,
    pub(crate) target_bottom_px: f32,
    pub(crate) last_sent_w: i32,
    pub(crate) last_sent_h: i32,
    pub(crate) last_smooth_tick_at: Instant,
    pub(crate) handle: ResizeHandle,
    pub(crate) press_sx: f32,
    pub(crate) press_sy: f32,
    pub(crate) h_weight_left: f32,
    pub(crate) h_weight_right: f32,
    pub(crate) v_weight_top: f32,
    pub(crate) v_weight_bottom: f32,
    pub(crate) drag_started: bool,
    pub(crate) settling: bool,
    pub(crate) resize_mode_sent: bool,
}
