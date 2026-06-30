use std::collections::{HashMap, HashSet};

use halley_core::field::{NodeId, Vec2};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FullscreenOrigin {
    UserKeybind,
    ClientRequest,
}

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

#[derive(Clone, Debug)]
pub(crate) struct FullscreenScaleAnim {
    pub monitor: String,
    pub from_pos: Vec2,
    pub to_pos: Vec2,
    pub from_size: Vec2,
    pub to_size: Vec2,
    pub start_ms: u64,
    pub duration_ms: u64,
    /// A hard-exit shrink holds the frozen snapshot past its visual duration
    /// until the client has committed a non-fullscreen buffer (or a safety
    /// timeout elapses), so the live surface is only revealed once it is already
    /// windowed-sized — otherwise the full-size buffer flashes for a few frames.
    /// `false` for the entry grow, which finalizes the moment its duration ends.
    pub settle: bool,
    /// When the settle hold completes, re-lay out the cluster workspace so the
    /// hidden siblings animate back in alongside the shrink landing instead of
    /// popping into place while the exiting window is still visually large.
    pub pending_cluster_relayout: bool,
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
    pub(crate) fullscreen_origin: HashMap<NodeId, FullscreenOrigin>,
    pub(crate) fullscreen_suspended_node: HashMap<String, NodeId>,
    pub(crate) fullscreen_soft_suspended_node: HashMap<String, NodeId>,
    pub(crate) fullscreen_restore: HashMap<NodeId, FullscreenSessionEntry>,
    pub(crate) fullscreen_motion: HashMap<NodeId, FullscreenMotion>,
    pub(crate) fullscreen_scale_anim: HashMap<NodeId, FullscreenScaleAnim>,
    /// Per-monitor camera (zoom + center) captured when fullscreen reset the monitor
    /// zoom to 1.0 on entry, restored on exit so leaving fullscreen returns to the
    /// pre-fullscreen zoom instead of plopping at 1.0.
    pub(crate) fullscreen_camera_restore:
        HashMap<String, crate::compositor::workspace::state::MaximizeCameraSnapshot>,
    pub(crate) direct_scanout: HashMap<String, FullscreenDirectScanoutState>,
    /// Cluster workspace siblings hidden while one member is fullscreen. Maps the
    /// fullscreen member node id → the sibling member ids that were hidden so the
    /// fullscreen window is the only cluster tile showing. Cleared on exit, which
    /// also re-lays out the cluster workspace so the tiles reappear.
    pub(crate) fullscreen_hidden_cluster_siblings: HashMap<NodeId, Vec<NodeId>>,
    /// Cluster members whose client fullscreen requests are ignored because the
    /// user explicitly exited fullscreen via the compositor keybind.
    pub(crate) client_fullscreen_blocked_nodes: HashSet<NodeId>,
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
