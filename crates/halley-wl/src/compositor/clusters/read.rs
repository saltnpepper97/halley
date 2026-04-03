use super::*;
use crate::compositor::clusters::state::ClusterState;
use crate::compositor::monitor::state::MonitorState;
use crate::render::active_window_frame_pad_px;
use halley_core::cluster::ClusterId;
use halley_core::cluster_layout::{ClusterWorkspaceLayoutKind, layout_cluster_workspace};
use halley_core::tiling::Rect;

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
    pub(super) kind: ClusterWorkspaceLayoutKind,
    pub(super) tiles: Vec<ClusterTilePlacement>,
    pub(super) overflow_members: Vec<NodeId>,
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

    fn cluster_layout_kind(&self) -> ClusterWorkspaceLayoutKind {
        self.tuning.cluster_layout_kind()
    }

    fn cluster_layout_bounds(
        &self,
        viewport: halley_core::viewport::Viewport,
    ) -> (halley_core::tiling::Rect, f32) {
        let (outer_gap, inner_gap) = compensated_cluster_gaps(
            self.tuning.tile_gaps_outer_px.max(0.0),
            self.tuning.tile_gaps_inner_px.max(0.0),
            self.tuning.border_size_px.max(0) as f32,
        );
        let outer_gap = outer_gap.max(0.0);
        let viewport_left = viewport.center.x - viewport.size.x * 0.5;
        let viewport_top = viewport.center.y - viewport.size.y * 0.5;
        (
            halley_core::tiling::Rect {
                x: viewport_left + outer_gap,
                y: viewport_top + outer_gap,
                w: (viewport.size.x - outer_gap * 2.0).max(0.0),
                h: (viewport.size.y - outer_gap * 2.0).max(0.0),
            },
            inner_gap.max(0.0),
        )
    }

    fn cluster_layout_plan_for_members(
        &self,
        viewport: halley_core::viewport::Viewport,
        members: &[NodeId],
    ) -> ClusterLayoutPlan {
        let kind = self.cluster_layout_kind();
        let (bounds, inner_gap) = self.cluster_layout_bounds(viewport);
        let result = layout_cluster_workspace(
            kind,
            bounds,
            inner_gap,
            active_window_frame_pad_px(self.tuning) as f32,
            members,
            self.tuning.active_cluster_visible_limit(),
        );
        let overflow_members = if matches!(kind, ClusterWorkspaceLayoutKind::Tiling) {
            result.queue_members.clone()
        } else {
            Vec::new()
        };
        let tiles = result
            .placements
            .into_iter()
            .map(|placement| ClusterTilePlacement {
                node_id: placement.node_id,
                rect: placement.rect,
            })
            .collect::<Vec<_>>();
        ClusterLayoutPlan {
            kind,
            tiles,
            overflow_members,
        }
    }

    pub(super) fn cluster_spawn_rect_for_new_member(
        &self,
        monitor: &str,
        cid: ClusterId,
    ) -> Option<halley_core::tiling::Rect> {
        let cluster = self.field.cluster(cid)?;
        let viewport = self.workspace_viewport_for_monitor(monitor)?;
        let mut preview_members = cluster.members().to_vec();
        let visible_limit = halley_core::cluster_layout::cluster_visible_limit(
            self.cluster_layout_kind(),
            self.tuning.active_cluster_visible_limit(),
        );
        if visible_limit == usize::MAX || preview_members.len() < visible_limit {
            if matches!(
                self.cluster_layout_kind(),
                ClusterWorkspaceLayoutKind::Stacking
            ) {
                preview_members.insert(0, NodeId::new(u64::MAX));
            } else {
                preview_members.push(NodeId::new(u64::MAX));
            }
        }
        self.cluster_layout_plan_for_members(viewport, &preview_members)
            .tiles
            .into_iter()
            .last()
            .map(|tile| tile.rect)
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
        let viewport = self.workspace_viewport_for_monitor(monitor)?;
        Some(self.cluster_layout_plan_for_members(viewport, cluster.members()))
    }

    pub(super) fn stack_layout_rects_for_members(
        &self,
        monitor: &str,
        members: &[NodeId],
    ) -> Option<std::collections::HashMap<NodeId, Rect>> {
        let viewport = self.workspace_viewport_for_monitor(monitor)?;
        Some(
            self.cluster_layout_plan_for_members(viewport, members)
                .tiles
                .into_iter()
                .map(|tile| (tile.node_id, tile.rect))
                .collect(),
        )
    }
}

#[cfg(test)]
fn direct_cluster_layout_rects(
    viewport: halley_core::viewport::Viewport,
    member_count: usize,
    outer_gap: f32,
    inner_gap: f32,
    master_width_frac: f32,
) -> Vec<halley_core::tiling::Rect> {
    if member_count == 0 {
        return Vec::new();
    }

    let outer_gap = outer_gap.max(0.0);
    let inner_gap = inner_gap.max(0.0);
    let viewport_left = viewport.center.x - viewport.size.x * 0.5;
    let viewport_top = viewport.center.y - viewport.size.y * 0.5;
    let content_x = viewport_left + outer_gap;
    let content_y = viewport_top + outer_gap;
    let content_w = (viewport.size.x - outer_gap * 2.0).max(0.0);
    let content_h = (viewport.size.y - outer_gap * 2.0).max(0.0);

    if member_count == 1 {
        return vec![halley_core::tiling::Rect {
            x: content_x,
            y: content_y,
            w: content_w,
            h: content_h,
        }];
    }

    let split_w = (content_w - inner_gap).max(0.0);
    let master_w = (split_w * master_width_frac.clamp(0.0, 1.0)).clamp(0.0, split_w);
    let stack_w = (split_w - master_w).max(0.0);
    let stack_x = content_x + master_w + inner_gap;
    let stack_count = member_count - 1;

    let mut rects = Vec::with_capacity(member_count);
    rects.push(halley_core::tiling::Rect {
        x: content_x,
        y: content_y,
        w: master_w,
        h: content_h,
    });

    if stack_count == 1 {
        rects.push(halley_core::tiling::Rect {
            x: stack_x,
            y: content_y,
            w: stack_w,
            h: content_h,
        });
        return rects;
    }

    let total_stack_gap = inner_gap * (stack_count.saturating_sub(1) as f32);
    let stack_window_h = ((content_h - total_stack_gap).max(0.0)) / stack_count as f32;
    let mut next_y = content_y;
    let content_bottom = content_y + content_h;

    for index in 0..stack_count {
        let remaining = stack_count - index;
        let h = if remaining == 1 {
            (content_bottom - next_y).max(0.0)
        } else {
            stack_window_h.max(0.0)
        };
        rects.push(halley_core::tiling::Rect {
            x: stack_x,
            y: next_y,
            w: stack_w,
            h,
        });
        next_y += h + inner_gap;
    }

    rects
}

fn compensated_cluster_gaps(outer_gap: f32, inner_gap: f32, border_px: f32) -> (f32, f32) {
    let border_px = border_px.max(0.0);
    (
        outer_gap.max(0.0) + border_px,
        inner_gap.max(0.0) + border_px * 2.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_viewport() -> halley_core::viewport::Viewport {
        halley_core::viewport::Viewport::new(
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 800.0, y: 600.0 },
        )
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= 0.01,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn compensated_cluster_gaps_reserve_outward_border_space() {
        let (outer, inner) = compensated_cluster_gaps(10.0, 10.0, 3.0);

        assert_close(outer, 13.0);
        assert_close(inner, 16.0);
    }

    #[test]
    fn two_window_layout_uses_full_height_and_exact_horizontal_gap() {
        let rects = direct_cluster_layout_rects(test_viewport(), 2, 0.0, 10.0, 0.6);
        assert_eq!(rects.len(), 2);

        let master = rects[0];
        let stack = rects[1];

        assert_close(master.y, 0.0);
        assert_close(master.h, 600.0);
        assert_close(stack.y, 0.0);
        assert_close(stack.h, 600.0);
        assert_close(stack.x - master.right(), 10.0);
    }

    #[test]
    fn three_window_layout_keeps_exact_vertical_stack_gap() {
        let rects = direct_cluster_layout_rects(test_viewport(), 3, 10.0, 10.0, 0.6);
        assert_eq!(rects.len(), 3);

        let master = rects[0];
        let upper = rects[1];
        let lower = rects[2];

        assert_close(master.x, 10.0);
        assert_close(master.y, 10.0);
        assert_close(master.bottom(), 590.0);
        assert_close(upper.x - master.right(), 10.0);
        assert_close(upper.y, 10.0);
        assert_close(lower.y - upper.bottom(), 10.0);
        assert_close(lower.bottom(), 590.0);
    }
}
