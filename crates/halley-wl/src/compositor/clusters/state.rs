use std::collections::{HashMap, HashSet};

use halley_core::cluster::ClusterId;
use halley_core::cluster_policy::ClusterFormationState;
use halley_core::field::{NodeId, Vec2};
use halley_core::tiling::Rect;
use halley_core::viewport::Viewport;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ClusterOverflowPromotionAnim {
    pub(crate) member_id: NodeId,
    pub(crate) started_at_ms: u64,
    pub(crate) reveal_at_ms: u64,
    pub(crate) source_strip_rect: Rect,
    pub(crate) source_center: Vec2,
    pub(crate) target_center: Vec2,
}

pub(crate) struct ClusterState {
    pub(crate) cluster_form_state: ClusterFormationState,
    pub(crate) active_cluster_workspaces: HashMap<String, ClusterId>,
    pub(crate) cluster_bloom_open: HashMap<String, ClusterId>,
    pub(crate) cluster_mode_selected_nodes: HashMap<String, HashSet<NodeId>>,
    pub(crate) workspace_hidden_nodes: HashMap<String, Vec<NodeId>>,
    pub(crate) workspace_prev_viewports: HashMap<String, Viewport>,
    pub(crate) workspace_core_positions: HashMap<String, Vec2>,
    pub(crate) cluster_overflow_members: HashMap<String, Vec<NodeId>>,
    pub(crate) cluster_overflow_rects: HashMap<String, Rect>,
    pub(crate) cluster_overflow_scroll_offsets: HashMap<String, usize>,
    pub(crate) cluster_overflow_reveal_started_at_ms: HashMap<String, u64>,
    pub(crate) cluster_overflow_visible_until_ms: HashMap<String, u64>,
    pub(crate) cluster_overflow_promotion_anim: HashMap<String, ClusterOverflowPromotionAnim>,
}
