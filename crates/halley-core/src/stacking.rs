use crate::cluster_layout::{ClusterCycleDirection, ClusterWorkspacePlacement};
use crate::field::NodeId;
use crate::tiling::Rect;

pub fn stacking_visible_limit(max_visible: usize) -> usize {
    if max_visible == 0 {
        usize::MAX
    } else {
        max_visible
    }
}

pub fn layout_stacking_workspace(
    bounds: Rect,
    members: &[NodeId],
    frame_pad: f32,
) -> Vec<ClusterWorkspacePlacement> {
    if members.is_empty() {
        return Vec::new();
    }

    const STACK_CARD_WIDTH_FRAC: f32 = 0.74;
    const STACK_CARD_HEIGHT_FRAC: f32 = 0.84;
    const STACK_PEEK_X_FRAC: f32 = 0.04;
    const STACK_SCALE_STEP: f32 = 0.035;
    const STACK_MIN_SCALE: f32 = 0.84;

    let frame_pad = frame_pad.max(0.0);
    let base_frame_w = (bounds.w * STACK_CARD_WIDTH_FRAC).clamp(1.0, bounds.w.max(1.0));
    let base_frame_h = (bounds.h * STACK_CARD_HEIGHT_FRAC).clamp(1.0, bounds.h.max(1.0));
    let center_x = bounds.x + bounds.w * 0.5;
    let center_y = bounds.y + bounds.h * 0.5;
    let active_frame_x = center_x - base_frame_w * 0.5;
    let active_frame_y = center_y - base_frame_h * 0.5;
    let peek_x = (bounds.w * STACK_PEEK_X_FRAC).max(24.0) + frame_pad * 2.0;

    let mut layered = Vec::with_capacity(members.len());
    for (index, node_id) in members.iter().copied().enumerate() {
        let scale = (1.0 - index as f32 * STACK_SCALE_STEP).clamp(STACK_MIN_SCALE, 1.0);
        let frame_w = (base_frame_w * scale).clamp(1.0, bounds.w.max(1.0));
        let frame_h = (base_frame_h * scale).clamp(1.0, bounds.h.max(1.0));
        let width = (frame_w - frame_pad * 2.0).clamp(1.0, bounds.w.max(1.0));
        let height = (frame_h - frame_pad * 2.0).clamp(1.0, bounds.h.max(1.0));
        let offset_x = peek_x * index as f32;
        let rect = Rect {
            x: active_frame_x + offset_x + frame_pad,
            y: active_frame_y + frame_pad,
            w: width,
            h: height,
        };
        layered.push((index, node_id, rect));
    }

    layered.sort_by_key(|(index, _, _)| std::cmp::Reverse(*index));
    layered
        .into_iter()
        .enumerate()
        .map(|(depth, (_, node_id, rect))| ClusterWorkspacePlacement {
            node_id,
            rect,
            depth,
        })
        .collect()
}

pub fn cycle_stacking_members(
    members: &mut Vec<NodeId>,
    direction: ClusterCycleDirection,
) -> Option<NodeId> {
    if members.is_empty() {
        return None;
    }

    match direction {
        ClusterCycleDirection::Prev => members.rotate_right(1),
        ClusterCycleDirection::Next => members.rotate_left(1),
    }
    members.first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<NodeId> {
        (0..n).map(NodeId::new).collect()
    }

    #[test]
    fn zero_max_visible_means_unlimited_for_stacking() {
        assert_eq!(stacking_visible_limit(0), usize::MAX);
    }

    #[test]
    fn stacking_front_card_is_centered_and_front_most() {
        let members = ids(4);
        let result = layout_stacking_workspace(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 600.0,
            },
            &members,
            12.0,
        );

        let active = result
            .iter()
            .find(|placement| placement.node_id == members[0])
            .expect("active placement");

        assert_eq!(active.depth, result.len() - 1);
        assert!((active.rect.x - 12.0 + (active.rect.w + 24.0) * 0.5 - 500.0).abs() <= 0.5);
        assert!((active.rect.y - 12.0 + (active.rect.h + 24.0) * 0.5 - 300.0).abs() <= 0.5);
    }

    #[test]
    fn stacking_visible_cards_step_outward_in_one_direction() {
        let members = ids(5);
        let result = layout_stacking_workspace(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 600.0,
            },
            &members,
            12.0,
        );

        let active = result
            .iter()
            .find(|placement| placement.node_id == members[0])
            .expect("active placement");
        let second = result
            .iter()
            .find(|placement| placement.node_id == members[1])
            .expect("second placement");
        let third = result
            .iter()
            .find(|placement| placement.node_id == members[2])
            .expect("third placement");

        assert!(second.rect.x > active.rect.x);
        assert!((second.rect.y - active.rect.y).abs() <= 0.5);
        assert!(third.rect.x > second.rect.x);
        assert!((third.rect.y - second.rect.y).abs() <= 0.5);
        assert!(second.rect.w < active.rect.w);
        assert!(third.rect.w < second.rect.w);
        assert!(second.rect.h < active.rect.h);
        assert!(third.rect.h < second.rect.h);
        assert!(second.rect.x - active.rect.x > 24.0);
    }

    #[test]
    fn cycling_stacking_members_rotates_order() {
        let mut members = ids(4);

        assert_eq!(
            cycle_stacking_members(&mut members, ClusterCycleDirection::Next),
            Some(NodeId::new(1))
        );
        assert_eq!(
            members,
            vec![
                NodeId::new(1),
                NodeId::new(2),
                NodeId::new(3),
                NodeId::new(0)
            ]
        );
    }
}
