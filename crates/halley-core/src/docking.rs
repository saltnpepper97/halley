use std::collections::{HashMap, HashSet};

use crate::field::{Field, NodeId, NodeKind, NodeState, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DockSide {
    Left,
    Right,
    Top,
    Bottom,
}

impl DockSide {
    #[inline]
    pub fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DockPreview {
    pub mover_id: NodeId,
    pub target_id: NodeId,
    pub side: DockSide,
    pub snap_pos: Vec2,
    pub armed: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct DockPair {
    pub a: NodeId,
    pub b: NodeId,
    pub a_side: DockSide,
    pub b_side: DockSide,
}

#[derive(Debug, Default)]
pub struct DockingState {
    preview: Option<DockPreview>,
    pairs: HashMap<NodeId, DockPair>,
}

impl DockingState {
    // Preserve current loose feel only as a ceiling.
    const MAX_ARM_SLACK_ONSCREEN: f32 = 140.0;
    const MAX_ARM_SLACK_OFFSCREEN: f32 = 40.0;

    // Actual defaults: tighter and more intentional.
    const DEFAULT_ARM_SLACK_ONSCREEN: f32 = 44.0;
    const DEFAULT_ARM_SLACK_OFFSCREEN: f32 = 24.0;

    // Candidate gating: too far away = no preview at all.
    const DEFAULT_CANDIDATE_SLACK_ONSCREEN: f32 = 72.0;
    const DEFAULT_CANDIDATE_SLACK_OFFSCREEN: f32 = 32.0;

    const CROSS_AXIS_FACTOR: f32 = 0.35;
    const CROSS_AXIS_MIN: f32 = 28.0;
    const CROSS_AXIS_MAX: f32 = 72.0;

    // Approximate visual node marker width for horizontal docking.
    //
    // Why this exists:
    // - field footprint for Node/Preview is only 24x24
    // - render width is actually dot + gap + label + padding/outline
    // - docking.rs does not have HalleyWlState, so it cannot call the exact
    //   renderer metric path directly
    //
    // This is intentionally a little generous so left/right window->node
    // docking lines up with what the user sees instead of the raw 24px dot.
    const NODE_DOT_DIAMETER_EST: f32 = 32.0;
    const NODE_GAP_EST: f32 = 12.0;
    const NODE_SIDE_PADDING_EST: f32 = 16.0;
    const NODE_CHAR_WIDTH_EST: f32 = 9.0;
    const NODE_LABEL_BASE_EST: f32 = 18.0;
    const NODE_MIN_VISUAL_WIDTH_EST: f32 = 96.0;

    #[inline]
    pub fn clear_preview(&mut self) {
        self.preview = None;
    }

    #[inline]
    pub fn preview(&self) -> Option<DockPreview> {
        self.preview
    }

    pub fn partner(&self, node_id: NodeId) -> Option<NodeId> {
        let pair = self.pairs.get(&node_id)?;
        if pair.a == node_id {
            Some(pair.b)
        } else if pair.b == node_id {
            Some(pair.a)
        } else {
            None
        }
    }

    pub fn sides_for_pair(&self, a: NodeId, b: NodeId) -> Option<(DockSide, DockSide)> {
        let pair = self.pairs.get(&a)?;
        if pair.a == a && pair.b == b {
            Some((pair.a_side, pair.b_side))
        } else if pair.a == b && pair.b == a {
            Some((pair.b_side, pair.a_side))
        } else {
            None
        }
    }

    pub fn pairs(&self) -> Vec<(NodeId, NodeId)> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();

        for (&id, pair) in &self.pairs {
            let other = if pair.a == id { pair.b } else { pair.a };
            let lo = id.as_u64().min(other.as_u64());
            let hi = id.as_u64().max(other.as_u64());
            if seen.insert((lo, hi)) {
                out.push((id, other));
            }
        }

        out
    }

    pub fn undock(&mut self, node_id: NodeId) -> bool {
        let Some(pair) = self.pairs.remove(&node_id) else {
            return false;
        };

        self.pairs.remove(&pair.a);
        self.pairs.remove(&pair.b);

        if self
            .preview
            .is_some_and(|p| p.mover_id == node_id || p.target_id == node_id)
        {
            self.preview = None;
        }

        true
    }

    fn can_participate(field: &Field, node_id: NodeId) -> bool {
        let Some(node) = field.node(node_id) else {
            return false;
        };

        if !field.is_visible(node_id) {
            return false;
        }

        if node.kind != NodeKind::Surface {
            return false;
        }

        matches!(node.state, NodeState::Active | NodeState::Node | NodeState::Preview)
    }

    fn node_extent(field: &Field, node_id: NodeId) -> Option<Vec2> {
        let node = field.node(node_id)?;
        Some(Vec2 {
            x: node.footprint.x.max(1.0),
            y: node.footprint.y.max(1.0),
        })
    }

    #[inline]
    fn estimate_node_visual_horizontal_width(label: &str) -> f32 {
        let label_w = Self::NODE_LABEL_BASE_EST + (label.chars().count() as f32 * Self::NODE_CHAR_WIDTH_EST);

        (Self::NODE_DOT_DIAMETER_EST
            + Self::NODE_GAP_EST
            + label_w
            + (Self::NODE_SIDE_PADDING_EST * 2.0))
            .max(Self::NODE_MIN_VISUAL_WIDTH_EST)
    }

    #[inline]
    fn docking_extent_for_side(field: &Field, node_id: NodeId, side: DockSide) -> Option<Vec2> {
        let node = field.node(node_id)?;
        let mut size = Self::node_extent(field, node_id)?;

        if matches!(node.state, NodeState::Node | NodeState::Preview) {
            match side {
                DockSide::Left | DockSide::Right => {
                    size.x = Self::estimate_node_visual_horizontal_width(&node.label)
                        .max(size.x)
                        .max(node.intrinsic_size.x.min(220.0));
                }
                DockSide::Top | DockSide::Bottom => {
                    // Vertical already feels correct; keep raw footprint semantics.
                }
            }
        }

        Some(size)
    }

    fn choose_side(delta: Vec2) -> DockSide {
        if delta.x.abs() >= delta.y.abs() {
            if delta.x >= 0.0 {
                DockSide::Right
            } else {
                DockSide::Left
            }
        } else if delta.y >= 0.0 {
            DockSide::Top
        } else {
            DockSide::Bottom
        }
    }

    fn snap_position_for_target(
        field: &Field,
        mover_id: NodeId,
        target_id: NodeId,
        side: DockSide,
    ) -> Option<Vec2> {
        let mover_size = Self::docking_extent_for_side(field, mover_id, side)?;
        let target = field.node(target_id)?;
        let target_size = Self::docking_extent_for_side(field, target_id, side)?;

        let mut snap = target.pos;

        match side {
            DockSide::Left => {
                snap.x = target.pos.x - ((target_size.x + mover_size.x) * 0.5);
                snap.y = target.pos.y;
            }
            DockSide::Right => {
                snap.x = target.pos.x + ((target_size.x + mover_size.x) * 0.5);
                snap.y = target.pos.y;
            }
            DockSide::Top => {
                snap.x = target.pos.x;
                snap.y = target.pos.y + ((target_size.y + mover_size.y) * 0.5);
            }
            DockSide::Bottom => {
                snap.x = target.pos.x;
                snap.y = target.pos.y - ((target_size.y + mover_size.y) * 0.5);
            }
        }

        Some(snap)
    }

    fn node_intersects_viewport(
        field: &Field,
        node_id: NodeId,
        viewport_center: Vec2,
        viewport_size: Vec2,
    ) -> bool {
        let Some(node) = field.node(node_id) else {
            return false;
        };
        let size = Self::node_extent(field, node_id).unwrap_or(Vec2 { x: 1.0, y: 1.0 });
        let half_vw = viewport_size.x * 0.5;
        let half_vh = viewport_size.y * 0.5;
        let vl = viewport_center.x - half_vw;
        let vr = viewport_center.x + half_vw;
        let vt = viewport_center.y - half_vh;
        let vb = viewport_center.y + half_vh;
        let nl = node.pos.x - size.x * 0.5;
        let nr = node.pos.x + size.x * 0.5;
        let nt = node.pos.y - size.y * 0.5;
        let nb = node.pos.y + size.y * 0.5;
        nr > vl && nl < vr && nb > vt && nt < vb
    }

    fn pos_inside_viewport(pos: Vec2, viewport_center: Vec2, viewport_size: Vec2) -> bool {
        let half_vw = viewport_size.x * 0.5;
        let half_vh = viewport_size.y * 0.5;
        pos.x >= viewport_center.x - half_vw
            && pos.x <= viewport_center.x + half_vw
            && pos.y >= viewport_center.y - half_vh
            && pos.y <= viewport_center.y + half_vh
    }

    #[inline]
    fn clamped_slack(default_value: f32, max_value: f32) -> f32 {
        default_value.min(max_value).max(0.0)
    }

    #[inline]
    fn cross_axis_slack(target_span: f32, mover_span: f32) -> f32 {
        (target_span.min(mover_span) * Self::CROSS_AXIS_FACTOR)
            .clamp(Self::CROSS_AXIS_MIN, Self::CROSS_AXIS_MAX)
    }

    fn candidate_is_close_enough(
        field: &Field,
        mover_id: NodeId,
        target_id: NodeId,
        side: DockSide,
        snap_pos: Vec2,
        viewport_center: Vec2,
        viewport_size: Vec2,
    ) -> bool {
        let Some(mover) = field.node(mover_id) else {
            return false;
        };
        let Some(target) = field.node(target_id) else {
            return false;
        };

        let dx = mover.pos.x - target.pos.x;
        let dy = mover.pos.y - target.pos.y;

        let mover_size =
            Self::docking_extent_for_side(field, mover_id, side).unwrap_or(Vec2 { x: 1.0, y: 1.0 });
        let target_size =
            Self::docking_extent_for_side(field, target_id, side).unwrap_or(Vec2 { x: 1.0, y: 1.0 });

        let snap_on_screen = Self::pos_inside_viewport(snap_pos, viewport_center, viewport_size);
        let axis_slack = if snap_on_screen {
            Self::clamped_slack(
                Self::DEFAULT_CANDIDATE_SLACK_ONSCREEN,
                Self::MAX_ARM_SLACK_ONSCREEN,
            )
        } else {
            Self::clamped_slack(
                Self::DEFAULT_CANDIDATE_SLACK_OFFSCREEN,
                Self::MAX_ARM_SLACK_OFFSCREEN,
            )
        };

        match side {
            DockSide::Left | DockSide::Right => {
                let desired = (mover_size.x + target_size.x) * 0.5;
                let axis_delta = (dx.abs() - desired).abs();
                let cross_delta = dy.abs();
                let cross_slack = Self::cross_axis_slack(target_size.y, mover_size.y);
                axis_delta <= axis_slack && cross_delta <= cross_slack
            }
            DockSide::Top | DockSide::Bottom => {
                let desired = (mover_size.y + target_size.y) * 0.5;
                let axis_delta = (dy.abs() - desired).abs();
                let cross_delta = dx.abs();
                let cross_slack = Self::cross_axis_slack(target_size.x, mover_size.x);
                axis_delta <= axis_slack && cross_delta <= cross_slack
            }
        }
    }

    fn is_armed(
        field: &Field,
        mover_id: NodeId,
        target_id: NodeId,
        side: DockSide,
        snap_pos: Vec2,
        viewport_center: Vec2,
        viewport_size: Vec2,
    ) -> bool {
        let Some(mover) = field.node(mover_id) else {
            return false;
        };
        let Some(target) = field.node(target_id) else {
            return false;
        };

        let dx = mover.pos.x - target.pos.x;
        let dy = mover.pos.y - target.pos.y;

        let mover_size =
            Self::docking_extent_for_side(field, mover_id, side).unwrap_or(Vec2 { x: 1.0, y: 1.0 });
        let target_size =
            Self::docking_extent_for_side(field, target_id, side).unwrap_or(Vec2 { x: 1.0, y: 1.0 });

        let snap_on_screen = Self::pos_inside_viewport(snap_pos, viewport_center, viewport_size);
        let arm_slack = if snap_on_screen {
            Self::clamped_slack(
                Self::DEFAULT_ARM_SLACK_ONSCREEN,
                Self::MAX_ARM_SLACK_ONSCREEN,
            )
        } else {
            Self::clamped_slack(
                Self::DEFAULT_ARM_SLACK_OFFSCREEN,
                Self::MAX_ARM_SLACK_OFFSCREEN,
            )
        };

        match side {
            DockSide::Left | DockSide::Right => {
                let desired = (mover_size.x + target_size.x) * 0.5;
                let axis_ok = (dx.abs() - desired).abs() <= arm_slack;
                let cross_ok = dy.abs() <= Self::cross_axis_slack(target_size.y, mover_size.y);
                axis_ok && cross_ok
            }
            DockSide::Top | DockSide::Bottom => {
                let desired = (mover_size.y + target_size.y) * 0.5;
                let axis_ok = (dy.abs() - desired).abs() <= arm_slack;
                let cross_ok = dx.abs() <= Self::cross_axis_slack(target_size.x, mover_size.x);
                axis_ok && cross_ok
            }
        }
    }

    fn choose_target_for_mover(
        &self,
        field: &Field,
        mover_id: NodeId,
        viewport_center: Vec2,
        viewport_size: Vec2,
    ) -> Option<(NodeId, DockSide, Vec2, bool)> {
        let mover = field.node(mover_id)?;

        let mut best: Option<(NodeId, DockSide, Vec2, bool, f32)> = None;

        for (&candidate_id, candidate) in field.nodes() {
            if candidate_id == mover_id {
                continue;
            }
            if !Self::can_participate(field, candidate_id) {
                continue;
            }
            if !Self::node_intersects_viewport(field, candidate_id, viewport_center, viewport_size)
            {
                continue;
            }

            let delta = Vec2 {
                x: mover.pos.x - candidate.pos.x,
                y: mover.pos.y - candidate.pos.y,
            };

            let side = Self::choose_side(delta);
            let Some(snap_pos) =
                Self::snap_position_for_target(field, mover_id, candidate_id, side)
            else {
                continue;
            };

            if !Self::candidate_is_close_enough(
                field,
                mover_id,
                candidate_id,
                side,
                snap_pos,
                viewport_center,
                viewport_size,
            ) {
                continue;
            }

            let dist2 = {
                let sx = mover.pos.x - snap_pos.x;
                let sy = mover.pos.y - snap_pos.y;
                (sx * sx) + (sy * sy)
            };

            let armed = Self::is_armed(
                field,
                mover_id,
                candidate_id,
                side,
                snap_pos,
                viewport_center,
                viewport_size,
            );

            match best {
                None => best = Some((candidate_id, side, snap_pos, armed, dist2)),
                Some((_, _, _, best_armed, best_dist2)) => {
                    let prefer =
                        (armed && !best_armed) || (armed == best_armed && dist2 < best_dist2);
                    if prefer {
                        best = Some((candidate_id, side, snap_pos, armed, dist2));
                    }
                }
            }
        }

        best.map(|(target_id, side, snap_pos, armed, _)| (target_id, side, snap_pos, armed))
    }

    pub fn update_preview(
        &mut self,
        field: &Field,
        mover_id: NodeId,
        viewport_center: Vec2,
        viewport_size: Vec2,
    ) -> Option<DockPreview> {
        if !Self::can_participate(field, mover_id) {
            self.preview = None;
            return None;
        }

        let Some((target_id, side, snap_pos, armed)) =
            self.choose_target_for_mover(field, mover_id, viewport_center, viewport_size)
        else {
            self.preview = None;
            return None;
        };

        let preview = DockPreview {
            mover_id,
            target_id,
            side,
            snap_pos,
            armed,
        };

        self.preview = Some(preview);
        Some(preview)
    }

    pub fn commit_preview(&mut self, field: &mut Field, mover_id: NodeId) -> bool {
        let Some(preview) = self.preview else {
            return false;
        };

        if preview.mover_id != mover_id || !preview.armed {
            return false;
        }

        if !Self::can_participate(field, preview.mover_id)
            || !Self::can_participate(field, preview.target_id)
        {
            self.preview = None;
            return false;
        }

        self.undock(preview.mover_id);
        self.undock(preview.target_id);

        if let Some(mover) = field.node_mut(preview.mover_id) {
            mover.pos = preview.snap_pos;
        } else {
            self.preview = None;
            return false;
        }

        let pair = DockPair {
            a: preview.mover_id,
            b: preview.target_id,
            a_side: preview.side,
            b_side: preview.side.opposite(),
        };

        self.pairs.insert(pair.a, pair);
        self.pairs.insert(pair.b, pair);
        self.preview = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decay::DecayLevel;
    use crate::field::{Field, NodeState, Vec2};

    fn test_viewport() -> (Vec2, Vec2) {
        (
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 10_000.0,
                y: 10_000.0,
            },
        )
    }

    fn spawn_pair() -> (Field, NodeId, NodeId) {
        let mut f = Field::new();
        let a = f.spawn_surface(
            "A",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let b = f.spawn_surface(
            "B",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        (f, a, b)
    }

    #[test]
    fn dock_side_opposite_roundtrips() {
        assert_eq!(DockSide::Left.opposite(), DockSide::Right);
        assert_eq!(DockSide::Right.opposite(), DockSide::Left);
        assert_eq!(DockSide::Top.opposite(), DockSide::Bottom);
        assert_eq!(DockSide::Bottom.opposite(), DockSide::Top);
    }

    #[test]
    fn choose_side_prefers_horizontal_when_dx_dominates() {
        assert_eq!(
            DockingState::choose_side(Vec2 { x: 50.0, y: 10.0 }),
            DockSide::Right
        );
        assert_eq!(
            DockingState::choose_side(Vec2 { x: -50.0, y: 10.0 }),
            DockSide::Left
        );
    }

    #[test]
    fn choose_side_prefers_vertical_when_dy_dominates() {
        assert_eq!(
            DockingState::choose_side(Vec2 { x: 10.0, y: 50.0 }),
            DockSide::Top
        );
        assert_eq!(
            DockingState::choose_side(Vec2 { x: 10.0, y: -50.0 }),
            DockSide::Bottom
        );
    }

    #[test]
    fn snap_position_for_left_right_uses_half_width_sum() {
        let (f, a, b) = spawn_pair();

        let left = DockingState::snap_position_for_target(&f, a, b, DockSide::Left).unwrap();
        let right = DockingState::snap_position_for_target(&f, a, b, DockSide::Right).unwrap();

        assert_eq!(left, Vec2 { x: 200.0, y: 0.0 });
        assert_eq!(right, Vec2 { x: 400.0, y: 0.0 });
    }

    #[test]
    fn snap_position_for_top_bottom_uses_half_height_sum() {
        let (mut f, a, b) = spawn_pair();
        if let Some(n) = f.node_mut(a) {
            n.intrinsic_size = Vec2 { x: 100.0, y: 60.0 };
            n.footprint = n.intrinsic_size;
        }
        if let Some(n) = f.node_mut(b) {
            n.intrinsic_size = Vec2 { x: 100.0, y: 80.0 };
            n.footprint = n.intrinsic_size;
        }

        let top = DockingState::snap_position_for_target(&f, a, b, DockSide::Top).unwrap();
        let bottom = DockingState::snap_position_for_target(&f, a, b, DockSide::Bottom).unwrap();

        assert_eq!(top, Vec2 { x: 300.0, y: 70.0 });
        assert_eq!(bottom, Vec2 { x: 300.0, y: -70.0 });
    }

    #[test]
    fn hidden_nodes_cannot_participate() {
        let (mut f, a, _) = spawn_pair();
        assert!(DockingState::can_participate(&f, a));

        assert!(f.set_hidden(a, true));
        assert!(!DockingState::can_participate(&f, a));
    }

    #[test]
    fn core_nodes_cannot_participate() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 80.0 });
        let b = f.spawn_surface("B", Vec2 { x: 20.0, y: 0.0 }, Vec2 { x: 100.0, y: 80.0 });

        let cid = f.create_cluster(vec![a, b]).unwrap();
        let core = f.collapse_cluster(cid).unwrap();

        assert!(!DockingState::can_participate(&f, core));
    }

    #[test]
    fn update_preview_selects_target_and_sets_armed_when_close_enough() {
        let mut f = Field::new();
        let mover = f.spawn_surface(
            "Mover",
            Vec2 { x: 205.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let target = f.spawn_surface(
            "Target",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        let mut docking = DockingState::default();
        let preview = docking
            .update_preview(&f, mover, test_viewport().0, test_viewport().1)
            .unwrap();

        assert_eq!(preview.mover_id, mover);
        assert_eq!(preview.target_id, target);
        assert_eq!(preview.side, DockSide::Left);
        assert!(preview.armed);
        assert_eq!(preview.snap_pos, Vec2 { x: 200.0, y: 0.0 });
    }

    #[test]
    fn update_preview_is_none_when_far() {
        let (f, mover, _) = spawn_pair();
        let mut docking = DockingState::default();

        let preview = docking.update_preview(&f, mover, test_viewport().0, test_viewport().1);
        assert!(preview.is_none());
    }

    #[test]
    fn commit_preview_creates_bidirectional_pair_and_moves_mover() {
        let mut f = Field::new();
        let mover = f.spawn_surface(
            "Mover",
            Vec2 { x: 205.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let target = f.spawn_surface(
            "Target",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        let mut docking = DockingState::default();
        let preview = docking
            .update_preview(&f, mover, test_viewport().0, test_viewport().1)
            .unwrap();
        assert!(preview.armed);

        assert!(docking.commit_preview(&mut f, mover));
        assert_eq!(docking.partner(mover), Some(target));
        assert_eq!(docking.partner(target), Some(mover));
        assert_eq!(
            docking.sides_for_pair(mover, target),
            Some((DockSide::Left, DockSide::Right))
        );
        assert_eq!(f.node(mover).unwrap().pos, Vec2 { x: 200.0, y: 0.0 });
        assert!(docking.preview().is_none());
    }

    #[test]
    fn commit_preview_rejects_wrong_mover() {
        let mut f = Field::new();
        let mover = f.spawn_surface(
            "Mover",
            Vec2 { x: 205.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let target = f.spawn_surface(
            "Target",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let other = f.spawn_surface(
            "Other",
            Vec2 { x: 600.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        let mut docking = DockingState::default();
        let preview = docking
            .update_preview(&f, mover, test_viewport().0, test_viewport().1)
            .unwrap();
        assert_eq!(preview.target_id, target);

        assert!(!docking.commit_preview(&mut f, other));
        assert!(docking.partner(mover).is_none());
        assert!(docking.partner(target).is_none());
    }

    #[test]
    fn commit_preview_rejects_without_preview() {
        let (mut f, mover, _) = spawn_pair();
        let mut docking = DockingState::default();

        assert!(!docking.commit_preview(&mut f, mover));
    }

    #[test]
    fn undock_removes_pair_from_both_nodes() {
        let mut f = Field::new();
        let mover = f.spawn_surface(
            "Mover",
            Vec2 { x: 205.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let target = f.spawn_surface(
            "Target",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        let mut docking = DockingState::default();
        let _ = docking.update_preview(&f, mover, test_viewport().0, test_viewport().1);
        assert!(docking.commit_preview(&mut f, mover));

        assert!(docking.undock(mover));
        assert_eq!(docking.partner(mover), None);
        assert_eq!(docking.partner(target), None);
        assert!(docking.pairs().is_empty());
    }

    #[test]
    fn committing_new_pair_undocks_old_pair_first() {
        let mut f = Field::new();

        let a = f.spawn_surface("A", Vec2 { x: 205.0, y: 0.0 }, Vec2 { x: 100.0, y: 80.0 });
        let b = f.spawn_surface("B", Vec2 { x: 300.0, y: 0.0 }, Vec2 { x: 100.0, y: 80.0 });
        let c = f.spawn_surface("C", Vec2 { x: 405.0, y: 0.0 }, Vec2 { x: 100.0, y: 80.0 });

        let mut docking = DockingState::default();

        let _ = docking.update_preview(&f, a, test_viewport().0, test_viewport().1);
        assert!(docking.commit_preview(&mut f, a));
        assert_eq!(docking.partner(a), Some(b));

        if let Some(n) = f.node_mut(a) {
            n.pos = Vec2 { x: 305.0, y: 0.0 };
        }

        let _ = docking.update_preview(&f, a, test_viewport().0, test_viewport().1);
        assert!(docking.commit_preview(&mut f, a));

        assert_eq!(docking.partner(a), Some(c));
        assert_eq!(docking.partner(c), Some(a));
        assert_eq!(docking.partner(b), None);
    }

    #[test]
    fn pairs_returns_each_pair_once() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 205.0, y: 0.0 }, Vec2 { x: 100.0, y: 80.0 });
        let b = f.spawn_surface("B", Vec2 { x: 300.0, y: 0.0 }, Vec2 { x: 100.0, y: 80.0 });

        let mut docking = DockingState::default();
        let _ = docking.update_preview(&f, a, test_viewport().0, test_viewport().1);
        assert!(docking.commit_preview(&mut f, a));

        let pairs = docking.pairs();
        assert_eq!(pairs.len(), 1);

        let (x, y) = pairs[0];
        assert!(
            (x == a && y == b) || (x == b && y == a),
            "unexpected pair: ({x}, {y})"
        );
    }

    #[test]
    fn preview_clears_when_node_cannot_participate() {
        let (mut f, mover, _) = spawn_pair();
        let mut docking = DockingState::default();

        assert!(
            docking
                .update_preview(&f, mover, test_viewport().0, test_viewport().1)
                .is_none()
        );

        if let Some(n) = f.node_mut(mover) {
            n.pos = Vec2 { x: 205.0, y: 0.0 };
        }

        assert!(
            docking
                .update_preview(&f, mover, test_viewport().0, test_viewport().1)
                .is_some()
        );
        assert!(f.set_hidden(mover, true));
        assert!(
            docking
                .update_preview(&f, mover, test_viewport().0, test_viewport().1)
                .is_none()
        );
        assert!(docking.preview().is_none());
    }

    #[test]
    fn node_state_preview_is_allowed_to_participate() {
        let mut f = Field::new();
        let mover = f.spawn_surface(
            "Mover",
            Vec2 { x: 205.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let target = f.spawn_surface(
            "Target",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        assert!(f.set_state(mover, NodeState::Preview));
        let mut docking = DockingState::default();
        let preview = docking
            .update_preview(&f, mover, test_viewport().0, test_viewport().1)
            .unwrap();

        assert_eq!(preview.target_id, target);
    }

    #[test]
    fn hidden_target_is_not_selected() {
        let mut f = Field::new();
        let mover = f.spawn_surface(
            "Mover",
            Vec2 { x: 205.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let hidden = f.spawn_surface(
            "Hidden",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let visible = f.spawn_surface(
            "Visible",
            Vec2 { x: 300.0, y: 200.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        assert!(f.set_hidden(hidden, true));

        let mut docking = DockingState::default();
        let preview = docking
            .update_preview(&f, mover, test_viewport().0, test_viewport().1)
            .unwrap();

        assert_eq!(preview.target_id, visible);
    }

    #[test]
    fn detached_target_is_not_selected() {
        let mut f = Field::new();
        let mover = f.spawn_surface(
            "Mover",
            Vec2 { x: 205.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let detached = f.spawn_surface(
            "Detached",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        assert!(f.set_detached(detached, true));

        let mut docking = DockingState::default();
        let preview = docking.update_preview(&f, mover, test_viewport().0, test_viewport().1);

        assert!(preview.is_none());
    }

    #[test]
    fn state_node_can_participate_in_docking() {
        let mut f = Field::new();
        let mover = f.spawn_surface(
            "Mover",
            Vec2 { x: 205.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let target = f.spawn_surface(
            "Target",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        assert!(f.set_decay_level(target, DecayLevel::Cold));
        assert_eq!(f.node(target).unwrap().state, NodeState::Node);

        let mut docking = DockingState::default();
        let preview = docking
            .update_preview(&f, mover, test_viewport().0, test_viewport().1)
            .unwrap();
        assert_eq!(preview.target_id, target);
    }

    #[test]
    fn preview_is_none_when_candidate_is_too_far_from_snap_band() {
        let mut f = Field::new();
        let mover = f.spawn_surface(
            "Mover",
            Vec2 { x: 80.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let _target = f.spawn_surface(
            "Target",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        let mut docking = DockingState::default();
        let preview = docking.update_preview(&f, mover, test_viewport().0, test_viewport().1);

        assert!(preview.is_none(), "far candidate should not even preview");
    }

    #[test]
    fn preview_is_none_when_cross_axis_offset_is_too_large() {
        let mut f = Field::new();
        let mover = f.spawn_surface(
            "Mover",
            Vec2 { x: 205.0, y: 85.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let _target = f.spawn_surface(
            "Target",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        let mut docking = DockingState::default();
        let preview = docking.update_preview(&f, mover, test_viewport().0, test_viewport().1);

        assert!(preview.is_none(), "misaligned candidate should not preview");
    }

    #[test]
    fn horizontal_window_to_node_uses_visual_label_width_estimate() {
        let mut f = Field::new();
        let mover = f.spawn_surface(
            "Mover",
            Vec2 { x: 175.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let target = f.spawn_surface(
            "Browser",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        assert!(f.set_state(target, NodeState::Node));
        assert_eq!(f.node(target).unwrap().footprint, Vec2 { x: 24.0, y: 24.0 });

        let visual_w = DockingState::docking_extent_for_side(&f, target, DockSide::Left)
            .unwrap()
            .x;

        assert!(visual_w > 24.0);

        let mut docking = DockingState::default();
        let preview = docking
            .update_preview(&f, mover, test_viewport().0, test_viewport().1)
            .unwrap();

        assert_eq!(preview.target_id, target);
        assert_eq!(preview.side, DockSide::Left);
        assert!(preview.armed);
    }

    #[test]
    fn longer_node_labels_produce_wider_horizontal_capture() {
        let mut f = Field::new();
        let short = f.spawn_surface(
            "Term",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let long = f.spawn_surface(
            "Firefox Developer Edition",
            Vec2 { x: 600.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        assert!(f.set_state(short, NodeState::Node));
        assert!(f.set_state(long, NodeState::Node));

        let short_w = DockingState::docking_extent_for_side(&f, short, DockSide::Left)
            .unwrap()
            .x;
        let long_w = DockingState::docking_extent_for_side(&f, long, DockSide::Left)
            .unwrap()
            .x;

        assert!(long_w > short_w);
    }

    #[test]
    fn vertical_window_to_node_keeps_raw_node_height_behavior() {
        let mut f = Field::new();
        let mover = f.spawn_surface(
            "Mover",
            Vec2 { x: 300.0, y: 165.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let target = f.spawn_surface(
            "NodeTarget",
            Vec2 { x: 300.0, y: 300.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        assert!(f.set_state(target, NodeState::Node));
        assert_eq!(f.node(target).unwrap().footprint, Vec2 { x: 24.0, y: 24.0 });

        let vertical_size = DockingState::docking_extent_for_side(&f, target, DockSide::Bottom)
            .unwrap();

        assert_eq!(vertical_size.y, 24.0);

        let mut docking = DockingState::default();
        let preview = docking
            .update_preview(&f, mover, test_viewport().0, test_viewport().1)
            .unwrap();

        assert_eq!(preview.target_id, target);
        assert_eq!(preview.side, DockSide::Bottom);
        assert!(preview.armed);
    }
}
