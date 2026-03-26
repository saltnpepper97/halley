use std::collections::HashMap;

use halley_core::field::{NodeId, Vec2};

#[derive(Clone, Copy, Debug)]
pub(crate) struct FullscreenSessionEntry {
    pub pos: Vec2,
    pub size: Vec2,
    pub viewport_center: Vec2,
    pub intrinsic_size: Vec2,
    pub bbox_loc: Option<(f32, f32)>,
    pub window_geometry: Option<(f32, f32, f32, f32)>,
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

pub(crate) struct FullscreenState {
    pub(crate) fullscreen_active_node: HashMap<String, NodeId>,
    pub(crate) fullscreen_suspended_node: HashMap<String, NodeId>,
    pub(crate) fullscreen_restore: HashMap<NodeId, FullscreenSessionEntry>,
    pub(crate) fullscreen_motion: HashMap<NodeId, FullscreenMotion>,
    pub(crate) fullscreen_scale_anim: HashMap<NodeId, FullscreenScaleAnim>,
}
