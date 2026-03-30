use super::*;
use crate::compositor::clusters::state::ClusterState;
use crate::compositor::monitor::state::MonitorState;
use crate::render::active_window_frame_pad_px;
use halley_core::cluster::{CLUSTER_VISIBLE_CAPACITY, ClusterId};

pub(super) struct ClusterReadController<'a> {
    pub(super) field: &'a Field,
    pub(super) cluster_state: &'a ClusterState,
    pub(super) monitor_state: &'a MonitorState,
    pub(super) tuning: &'a RuntimeTuning,
}

pub(super) struct EnterClusterWorkspacePlan {
    pub(super) cid: ClusterId,
    pub(super) core_id: NodeId,
    pub(super) core_pos: Vec2,
    pub(super) current_viewport: halley_core::viewport::Viewport,
    pub(super) hidden_ids: Vec<NodeId>,
}

pub(super) struct ExitClusterWorkspacePlan {
    pub(super) cid: ClusterId,
    pub(super) core_id: Option<NodeId>,
    pub(super) core_pos: Option<Vec2>,
    pub(super) hidden_ids: Vec<NodeId>,
}

pub(super) struct ClusterTilePlacement {
    pub(super) node_id: NodeId,
    pub(super) rect: halley_core::tiling::Rect,
}

pub(super) struct ClusterLayoutPlan {
    pub(super) tiles: Vec<ClusterTilePlacement>,
}

impl<'a> ClusterReadController<'a> {
    const OVERFLOW_STRIP_PAD_PX: f32 = 18.0;
    const OVERFLOW_STRIP_W_PX: f32 = 56.0;
    const OVERFLOW_ICON_PAD_PX: f32 = 8.0;
    const OVERFLOW_ICON_SIZE_PX: f32 = 40.0;
    const OVERFLOW_ICON_GAP_PX: f32 = 8.0;

    pub(super) fn cluster_bloom_for_monitor(&self, monitor: &str) -> Option<ClusterId> {
        self.cluster_state.cluster_bloom_open.get(monitor).copied()
    }

    pub(super) fn preferred_monitor_for_cluster(
        &self,
        cid: ClusterId,
        preferred: Option<&str>,
    ) -> Option<String> {
        preferred
            .map(str::to_string)
            .or_else(|| {
                self.cluster_state
                    .active_cluster_workspaces
                    .iter()
                    .find_map(|(monitor, active_cid)| (*active_cid == cid).then(|| monitor.clone()))
            })
            .or_else(|| {
                self.cluster_state
                    .cluster_bloom_open
                    .iter()
                    .find_map(|(monitor, open_cid)| (*open_cid == cid).then(|| monitor.clone()))
            })
            .or_else(|| {
                self.field
                    .cluster(cid)
                    .and_then(|cluster| cluster.core)
                    .and_then(|core_id| self.monitor_state.node_monitor.get(&core_id).cloned())
            })
            .or_else(|| {
                self.field.cluster(cid).and_then(|cluster| {
                    cluster
                        .members()
                        .iter()
                        .find_map(|member| self.monitor_state.node_monitor.get(member).cloned())
                })
            })
            .or_else(|| Some(self.monitor_state.current_monitor.clone()))
    }

    pub(super) fn workspace_viewport_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<halley_core::viewport::Viewport> {
        self.monitor_state
            .monitors
            .get(monitor)
            .map(|space| space.usable_viewport)
    }

    fn opened_cluster_world_rect_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<halley_core::tiling::Rect> {
        let viewport = self.workspace_viewport_for_monitor(monitor)?;
        Some(
            halley_core::tiling::Rect {
                x: viewport.center.x - viewport.size.x * 0.5,
                y: viewport.center.y - viewport.size.y * 0.5,
                w: viewport.size.x,
                h: viewport.size.y,
            }
            .inset(self.tuning.tile_gaps_outer_px.max(0.0)),
        )
    }

    pub(super) fn cluster_spawn_rect_for_new_member(
        &self,
        monitor: &str,
        cid: ClusterId,
    ) -> Option<halley_core::tiling::Rect> {
        let cluster = self.field.cluster(cid)?;
        let tile_rect = self.opened_cluster_world_rect_for_monitor(monitor)?;
        let tile_inset = (self.tuning.tile_gaps_inner_px * 0.5
            + active_window_frame_pad_px(self.tuning) as f32)
            .clamp(4.0, 28.0);
        let layout = cluster.workspace_layout(tile_rect);
        if cluster.members().len() >= CLUSTER_VISIBLE_CAPACITY {
            return layout
                .tiles
                .iter()
                .find(|tile| tile.id == cluster.members()[CLUSTER_VISIBLE_CAPACITY - 1])
                .map(|tile| tile.rect.inset(tile_inset));
        }
        layout
            .tiles
            .into_iter()
            .map(|tile| tile.rect)
            .last()
            .map(|rect| rect.inset(tile_inset))
    }

    pub(super) fn overflow_strip_rect_for_monitor(
        &self,
        monitor: &str,
        overflow_len: usize,
    ) -> Option<halley_core::tiling::Rect> {
        if overflow_len == 0 {
            return None;
        }
        let space = self.monitor_state.monitors.get(monitor)?;
        let visible_slots = overflow_len.min(6) as f32;
        let height = Self::OVERFLOW_ICON_PAD_PX * 2.0
            + visible_slots * Self::OVERFLOW_ICON_SIZE_PX
            + (visible_slots - 1.0).max(0.0) * Self::OVERFLOW_ICON_GAP_PX;
        Some(halley_core::tiling::Rect {
            x: (space.width as f32 - Self::OVERFLOW_STRIP_W_PX - Self::OVERFLOW_STRIP_PAD_PX)
                .max(0.0),
            y: ((space.height as f32 - height) * 0.5).max(Self::OVERFLOW_STRIP_PAD_PX),
            w: Self::OVERFLOW_STRIP_W_PX,
            h: height,
        })
    }

    pub(super) fn plan_enter_cluster_workspace(
        &self,
        core_id: NodeId,
        monitor: &str,
    ) -> Option<EnterClusterWorkspacePlan> {
        let cid = self.field.cluster_id_for_core_public(core_id)?;
        let cluster = self.field.cluster(cid)?;
        let members = cluster.members().to_vec();
        let core_pos = self.field.node(core_id)?.pos;
        let current_viewport = self.workspace_viewport_for_monitor(monitor)?;
        let ids: Vec<NodeId> = self.field.nodes().keys().copied().collect();
        let mut hidden_ids = Vec::new();
        for id in ids {
            if members.contains(&id) || id == core_id {
                continue;
            }
            if self
                .monitor_state
                .node_monitor
                .get(&id)
                .is_some_and(|node_monitor| node_monitor != monitor)
            {
                continue;
            }
            let already_detached = self
                .field
                .node(id)
                .is_some_and(|n| n.visibility.has(Visibility::DETACHED));
            if !already_detached {
                hidden_ids.push(id);
            }
        }
        Some(EnterClusterWorkspacePlan {
            cid,
            core_id,
            core_pos,
            current_viewport,
            hidden_ids,
        })
    }

    pub(super) fn plan_exit_cluster_workspace(
        &self,
        monitor: &str,
    ) -> Option<ExitClusterWorkspacePlan> {
        let cid = self
            .cluster_state
            .active_cluster_workspaces
            .get(monitor)
            .copied()?;
        let hidden_ids = self
            .cluster_state
            .workspace_hidden_nodes
            .get(monitor)
            .cloned()
            .unwrap_or_default();
        let core_id = self.field.cluster(cid).and_then(|c| c.core);
        let core_pos = core_id.and_then(|id| self.field.node(id).map(|node| node.pos));
        Some(ExitClusterWorkspacePlan {
            cid,
            core_id,
            core_pos,
            hidden_ids,
        })
    }

    pub(super) fn plan_active_cluster_layout(&self, monitor: &str) -> Option<ClusterLayoutPlan> {
        let cid = self
            .cluster_state
            .active_cluster_workspaces
            .get(monitor)
            .copied()?;
        let cluster = self.field.cluster(cid)?;
        let world_rect = self.opened_cluster_world_rect_for_monitor(monitor)?;
        let tile_inset = (self.tuning.tile_gaps_inner_px * 0.5
            + active_window_frame_pad_px(self.tuning) as f32)
            .clamp(4.0, 28.0);
        let tiles = cluster
            .workspace_layout(world_rect)
            .tiles
            .into_iter()
            .map(|tile| ClusterTilePlacement {
                node_id: tile.id,
                rect: tile.rect.inset(tile_inset),
            })
            .collect::<Vec<_>>();
        Some(ClusterLayoutPlan { tiles })
    }
}
