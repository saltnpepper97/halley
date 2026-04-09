use crate::field::NodeId;
use crate::stacking::{layout_stacking_workspace, stacking_visible_limit};
use crate::tiling::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClusterWorkspaceLayoutKind {
    Tiling,
    Stacking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClusterCycleDirection {
    Prev,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClusterWorkspacePlacement {
    pub node_id: NodeId,
    pub rect: Rect,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClusterWorkspaceLayoutResult {
    pub kind: ClusterWorkspaceLayoutKind,
    pub placements: Vec<ClusterWorkspacePlacement>,
    pub queue_members: Vec<NodeId>,
}

#[inline]
pub fn cluster_visible_limit(kind: ClusterWorkspaceLayoutKind, limit_setting: usize) -> usize {
    match kind {
        ClusterWorkspaceLayoutKind::Tiling => {
            if limit_setting == 0 {
                usize::MAX
            } else {
                limit_setting.saturating_add(1)
            }
        }
        ClusterWorkspaceLayoutKind::Stacking => stacking_visible_limit(limit_setting),
    }
}

pub fn layout_cluster_workspace(
    kind: ClusterWorkspaceLayoutKind,
    bounds: Rect,
    inner_gap: f32,
    frame_pad: f32,
    members: &[NodeId],
    limit_setting: usize,
) -> ClusterWorkspaceLayoutResult {
    let visible_limit = cluster_visible_limit(kind, limit_setting);
    let visible_len = members.len().min(visible_limit);
    let visible_members = &members[..visible_len];
    let queue_members = if matches!(kind, ClusterWorkspaceLayoutKind::Tiling) {
        members[visible_len..].to_vec()
    } else {
        Vec::new()
    };

    let placements = match kind {
        ClusterWorkspaceLayoutKind::Tiling => {
            layout_tiling_workspace(bounds, inner_gap, visible_members)
        }
        ClusterWorkspaceLayoutKind::Stacking => {
            layout_stacking_workspace(bounds, visible_members, frame_pad)
        }
    };

    ClusterWorkspaceLayoutResult {
        kind,
        placements,
        queue_members,
    }
}

fn layout_tiling_workspace(
    bounds: Rect,
    inner_gap: f32,
    members: &[NodeId],
) -> Vec<ClusterWorkspacePlacement> {
    if members.is_empty() {
        return Vec::new();
    }

    if members.len() == 1 {
        return vec![ClusterWorkspacePlacement {
            node_id: members[0],
            rect: bounds,
            depth: 0,
        }];
    }

    let inner_gap = inner_gap.max(0.0);
    let split_w = (bounds.w - inner_gap).max(0.0);
    let master_w = (split_w * 0.6).clamp(0.0, split_w);
    let stack_w = (split_w - master_w).max(0.0);
    let stack_x = bounds.x + master_w + inner_gap;
    let stack_count = members.len() - 1;

    let mut placements = Vec::with_capacity(members.len());
    placements.push(ClusterWorkspacePlacement {
        node_id: members[0],
        rect: Rect {
            x: bounds.x,
            y: bounds.y,
            w: master_w,
            h: bounds.h,
        },
        depth: 0,
    });

    if stack_count == 1 {
        placements.push(ClusterWorkspacePlacement {
            node_id: members[1],
            rect: Rect {
                x: stack_x,
                y: bounds.y,
                w: stack_w,
                h: bounds.h,
            },
            depth: 1,
        });
        return placements;
    }

    let total_stack_gap = inner_gap * (stack_count.saturating_sub(1) as f32);
    let stack_window_h = ((bounds.h - total_stack_gap).max(0.0)) / stack_count as f32;
    let mut next_y = bounds.y;
    let bounds_bottom = bounds.y + bounds.h;

    for index in 0..stack_count {
        let remaining = stack_count - index;
        let height = if remaining == 1 {
            (bounds_bottom - next_y).max(0.0)
        } else {
            stack_window_h.max(0.0)
        };
        placements.push(ClusterWorkspacePlacement {
            node_id: members[index + 1],
            rect: Rect {
                x: stack_x,
                y: next_y,
                w: stack_w,
                h: height,
            },
            depth: index + 1,
        });
        next_y += height + inner_gap;
    }

    placements
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<NodeId> {
        (0..n).map(NodeId::new).collect()
    }

    #[test]
    fn tiling_limit_keeps_master_plus_stack() {
        let result = layout_cluster_workspace(
            ClusterWorkspaceLayoutKind::Tiling,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 700.0,
            },
            12.0,
            0.0,
            &ids(6),
            3,
        );

        assert_eq!(result.placements.len(), 4);
        assert_eq!(result.queue_members.len(), 2);
    }

    #[test]
    fn stacking_limit_counts_total_visible_cards_without_queue() {
        let result = layout_cluster_workspace(
            ClusterWorkspaceLayoutKind::Stacking,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 700.0,
            },
            0.0,
            12.0,
            &ids(6),
            3,
        );

        assert_eq!(result.placements.len(), 3);
        assert!(result.queue_members.is_empty());
    }

    #[test]
    fn zero_max_visible_means_unlimited_for_stacking() {
        let members = ids(5);
        let result = layout_cluster_workspace(
            ClusterWorkspaceLayoutKind::Stacking,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 900.0,
                h: 600.0,
            },
            0.0,
            12.0,
            &members,
            0,
        );

        assert_eq!(result.placements.len(), members.len());
        assert!(result.queue_members.is_empty());
    }
}
