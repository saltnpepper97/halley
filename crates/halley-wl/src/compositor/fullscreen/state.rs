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

#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub(crate) struct FullscreenDirectScanoutState {
    pub(crate) candidate_node: Option<NodeId>,
    pub(crate) active_node: Option<NodeId>,
    pub(crate) reason: Option<String>,
}

pub(crate) struct FullscreenState {
    pub(crate) fullscreen_active_node: HashMap<String, NodeId>,
    pub(crate) fullscreen_suspended_node: HashMap<String, NodeId>,
    pub(crate) fullscreen_restore: HashMap<NodeId, FullscreenSessionEntry>,
    pub(crate) fullscreen_motion: HashMap<NodeId, FullscreenMotion>,
    pub(crate) fullscreen_scale_anim: HashMap<NodeId, FullscreenScaleAnim>,
    pub(crate) direct_scanout: HashMap<String, FullscreenDirectScanoutState>,
}

impl FullscreenState {
    pub(crate) fn set_direct_scanout_status(
        &mut self,
        monitor: &str,
        candidate_node: Option<NodeId>,
        active_node: Option<NodeId>,
        reason: Option<String>,
    ) {
        self.direct_scanout.insert(
            monitor.to_string(),
            FullscreenDirectScanoutState {
                candidate_node,
                active_node,
                reason,
            },
        );
    }

    pub(crate) fn clear_direct_scanout_for_monitor(&mut self, monitor: &str) {
        self.direct_scanout.remove(monitor);
    }

    pub(crate) fn clear_direct_scanout_for_node(&mut self, node_id: NodeId) {
        self.direct_scanout.retain(|_, state| {
            state.candidate_node != Some(node_id) && state.active_node != Some(node_id)
        });
    }
}
