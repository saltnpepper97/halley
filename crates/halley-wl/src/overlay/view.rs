use halley_config::RuntimeTuning;
use halley_core::field::{Field, NodeId, Vec2};
use halley_core::tiling::Rect;
use halley_core::viewport::Viewport;

use crate::compositor::clusters::state::ClusterState;
use crate::compositor::interaction::state::InteractionState;
use crate::compositor::monitor::state::MonitorState;
use crate::compositor::root::Halley;
use crate::render::state::{NodeAppIconCacheEntry, RenderState};

pub(crate) struct OverlayView<'a> {
    pub(crate) field: &'a Field,
    pub(crate) cluster_state: &'a ClusterState,
    pub(crate) monitor_state: &'a MonitorState,
    pub(crate) interaction_state: &'a InteractionState,
    pub(crate) render_state: &'a RenderState,
    pub(crate) tuning: &'a RuntimeTuning,
    pub(crate) node_app_ids: &'a std::collections::HashMap<NodeId, String>,
    pub(crate) viewport: Viewport,
    pub(crate) camera_view_size: Vec2,
}

impl<'a> OverlayView<'a> {
    pub(crate) fn from_halley(st: &'a Halley) -> Self {
        Self {
            field: &st.model.field,
            cluster_state: &st.model.cluster_state,
            monitor_state: &st.model.monitor_state,
            interaction_state: &st.input.interaction_state,
            render_state: &st.ui.render_state,
            tuning: &st.runtime.tuning,
            node_app_ids: &st.model.node_app_ids,
            viewport: st.model.viewport,
            camera_view_size: st.camera_view_size(),
        }
    }

    pub(crate) fn cluster_overflow_visible_for_monitor(&self, monitor: &str, now_ms: u64) -> bool {
        self.cluster_state
            .cluster_overflow_visible_until_ms
            .get(monitor)
            .is_some_and(|visible_until_ms| *visible_until_ms > now_ms)
    }

    pub(crate) fn cluster_overflow_rect_for_monitor(&self, monitor: &str) -> Option<Rect> {
        self.cluster_state
            .cluster_overflow_rects
            .get(monitor)
            .copied()
    }

    pub(crate) fn cluster_overflow_member_ids_for_monitor(&self, monitor: &str) -> &[NodeId] {
        self.cluster_state
            .cluster_overflow_members
            .get(monitor)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn cluster_overflow_scroll_offset_for_monitor(&self, monitor: &str) -> usize {
        self.cluster_state
            .cluster_overflow_scroll_offsets
            .get(monitor)
            .copied()
            .unwrap_or(0)
    }

    pub(crate) fn cluster_overflow_drag_preview_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<(NodeId, (f32, f32))> {
        self.interaction_state
            .cluster_overflow_drag_preview
            .as_ref()
            .filter(|preview| preview.monitor == monitor)
            .map(|preview| (preview.member_id, preview.screen_local))
    }

    pub(crate) fn cluster_overflow_promotion_anim_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<crate::compositor::clusters::state::ClusterOverflowPromotionAnim> {
        self.cluster_state
            .cluster_overflow_promotion_anim
            .get(monitor)
            .copied()
    }

    pub(crate) fn node_visible_on_current_monitor(&self, node_id: NodeId) -> bool {
        self.field.is_visible(node_id)
            && self
                .monitor_state
                .node_monitor
                .get(&node_id)
                .is_some_and(|name| name == &self.monitor_state.current_monitor)
    }

    pub(crate) fn node_app_icon_entry(&self, node_id: NodeId) -> Option<&'a NodeAppIconCacheEntry> {
        self.node_app_ids
            .get(&node_id)
            .and_then(|app_id| self.render_state.node_app_icon_cache.get(app_id))
    }

    pub(crate) fn world_to_screen(&self, w: i32, h: i32, x: f32, y: f32) -> (i32, i32) {
        let vw = self.camera_view_size.x.max(1.0);
        let vh = self.camera_view_size.y.max(1.0);
        let nx = ((x - self.viewport.center.x) / vw) + 0.5;
        let ny = ((y - self.viewport.center.y) / vh) + 0.5;
        let sx = (nx * w as f32).round() as i32;
        let sy = (ny * h as f32).round() as i32;
        (sx, sy)
    }
}
