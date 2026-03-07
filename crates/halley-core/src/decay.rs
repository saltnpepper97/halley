use crate::field::{Field, NodeId, NodeKind, Vec2};
use crate::viewport::{FocusRing, FocusZone, Viewport};

#[cfg(test)]
use crate::field::NodeState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecayLevel {
    Hot,  // Active
    Warm, // Preview
    Cold, // Node
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecayPolicy {
    /// Age >= preview_after_ms => Warm/Preview
    pub preview_after_ms: u64,
    /// Age >= node_after_ms => Cold/Node (must be >= preview_after_ms)
    pub node_after_ms: u64,
}

impl DecayPolicy {
    pub fn new(preview_after_ms: u64, node_after_ms: u64) -> Self {
        Self {
            preview_after_ms,
            node_after_ms,
        }
    }
}

/// Advance representation decay for all nodes based on time since last touch.
/// - `now_ms` is a monotonic ms counter controlled by the outer loop.
/// - `focused` is pinned Hot.
/// - Core nodes do not decay (they remain handles).
pub fn tick_decay(field: &mut Field, now_ms: u64, policy: DecayPolicy, focused: Option<NodeId>) {
    debug_assert!(policy.node_after_ms >= policy.preview_after_ms);

    let ids: Vec<NodeId> = field.nodes().keys().copied().collect();

    for id in ids {
        let Some(n) = field.node(id) else { continue };

        if n.kind == NodeKind::Core {
            continue;
        }

        if Some(id) == focused {
            field.set_decay_level(id, DecayLevel::Hot);
            continue;
        }

        let age = now_ms.saturating_sub(n.last_touch_ms);

        if age >= policy.node_after_ms {
            field.set_decay_level(id, DecayLevel::Cold);
        } else if age >= policy.preview_after_ms {
            field.set_decay_level(id, DecayLevel::Warm);
        } else {
            field.set_decay_level(id, DecayLevel::Hot);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusRingDecayPolicy {
    /// Inside the focus ring:
    /// - age < inside_to_preview_ms => Hot/Active
    /// - age < inside_to_preview_ms + preview_to_node_ms => Warm/Preview
    /// - otherwise => Cold/Node
    pub inside_to_preview_ms: u64,

    /// Additional time spent in Preview before Node once inside_to_preview_ms is reached.
    pub preview_to_node_ms: u64,

    /// Outside the focus ring:
    /// - if true => immediately Cold/Node
    pub outside_immediate_cold: bool,
}

impl FocusRingDecayPolicy {
    pub fn new() -> Self {
        Self {
            inside_to_preview_ms: 1_200_000,
            preview_to_node_ms: 60_000,
            outside_immediate_cold: true,
        }
    }
}

/// Focus-ring-aware decay:
/// - Inside focus ring: Hot, then Preview, then Node based on timers
/// - Outside focus ring: Cold immediately
/// - Focused node: Hot
/// - Core nodes do not decay
pub fn tick_decay_focus_ring(
    field: &mut Field,
    vp: &Viewport,
    now_ms: u64,
    focus_ring: FocusRing,
    policy: FocusRingDecayPolicy,
    focused: Option<NodeId>,
) {
    let ids: Vec<NodeId> = field.nodes().keys().copied().collect();

    for id in ids {
        let (kind, pos, intrinsic_size, last_touch_ms) = {
            let Some(n) = field.node(id) else { continue };
            (n.kind.clone(), n.pos, n.intrinsic_size, n.last_touch_ms)
        };

        if kind == NodeKind::Core {
            continue;
        }

        if Some(id) == focused {
            field.set_decay_level(id, DecayLevel::Hot);
            continue;
        }

        let zone = dominant_focus_zone(focus_ring, vp.center, pos, intrinsic_size);

        match zone {
            FocusZone::Inside => {
                let age = now_ms.saturating_sub(last_touch_ms);
                let to_preview = policy.inside_to_preview_ms;
                let to_node = to_preview.saturating_add(policy.preview_to_node_ms);

                if age >= to_node {
                    field.set_decay_level(id, DecayLevel::Cold);
                } else if age >= to_preview {
                    field.set_decay_level(id, DecayLevel::Warm);
                } else {
                    field.set_decay_level(id, DecayLevel::Hot);
                }
            }
            FocusZone::Outside => {
                if policy.outside_immediate_cold {
                    field.set_decay_level(id, DecayLevel::Cold);
                } else {
                    let age = now_ms.saturating_sub(last_touch_ms);
                    let to_preview = policy.inside_to_preview_ms;
                    let to_node = to_preview.saturating_add(policy.preview_to_node_ms);

                    if age >= to_node {
                        field.set_decay_level(id, DecayLevel::Cold);
                    } else if age >= to_preview {
                        field.set_decay_level(id, DecayLevel::Warm);
                    } else {
                        field.set_decay_level(id, DecayLevel::Hot);
                    }
                }
            }
        }
    }
}

fn dominant_focus_zone(
    focus_ring: FocusRing,
    vp_center: Vec2,
    pos: Vec2,
    footprint: Vec2,
) -> FocusZone {
    // Approximate "where the window mostly is" using a small deterministic sample grid.
    // Majority semantics:
    // - >50% inside => Inside
    // - otherwise => Outside
    let w = footprint.x.abs();
    let h = footprint.y.abs();

    if w < 1.0 || h < 1.0 {
        return focus_ring.zone(vp_center, pos);
    }

    let sx = 5usize;
    let sy = 5usize;
    let mut inside = 0usize;

    let min_x = pos.x - w * 0.5;
    let min_y = pos.y - h * 0.5;

    for iy in 0..sy {
        for ix in 0..sx {
            let tx = (ix as f32 + 0.5) / sx as f32;
            let ty = (iy as f32 + 0.5) / sy as f32;
            let p = Vec2 {
                x: min_x + tx * w,
                y: min_y + ty * h,
            };

            if focus_ring.zone(vp_center, p) == FocusZone::Inside {
                inside += 1;
            }
        }
    }

    let total = (sx * sy) as f32;
    let frac_inside = inside as f32 / total;

    if frac_inside > 0.5 {
        FocusZone::Inside
    } else {
        FocusZone::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Vec2;

    fn default_focus_ring() -> FocusRing {
        FocusRing::new(50.0, 30.0, 0.0)
    }

    #[test]
    fn decays_hot_to_warm_to_cold() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        assert!(f.touch(a, 0));
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Hot);
        assert_eq!(f.node(a).unwrap().state, NodeState::Active);

        let policy = DecayPolicy::new(1000, 5000);

        tick_decay(&mut f, 1500, policy, None);
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Warm);
        assert_eq!(f.node(a).unwrap().state, NodeState::Preview);

        tick_decay(&mut f, 6000, policy, None);
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Cold);
        assert_eq!(f.node(a).unwrap().state, NodeState::Node);
    }

    #[test]
    fn focused_node_stays_hot() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        assert!(f.touch(a, 0));

        let policy = DecayPolicy::new(1000, 5000);

        tick_decay(&mut f, 6000, policy, Some(a));
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Hot);
        assert_eq!(f.node(a).unwrap().state, NodeState::Active);
    }

    #[test]
    fn core_does_not_decay() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let b = f.spawn_surface("B", Vec2 { x: 10.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let cid = f.create_cluster(vec![a, b]).unwrap();
        let core = f.collapse_cluster(cid).unwrap();

        let policy = DecayPolicy::new(1000, 5000);
        tick_decay(&mut f, 999_999, policy, None);

        let n = f.node(core).unwrap();
        assert_eq!(n.kind, NodeKind::Core);
        assert_eq!(n.state, NodeState::Core);
    }

    #[test]
    fn inside_focus_ring_near_center_stays_hot() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        assert!(f.touch(a, 0));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let ring = default_focus_ring();
        let policy = FocusRingDecayPolicy::new();

        tick_decay_focus_ring(&mut f, &vp, 999_999, ring, policy, None);

        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Hot);
        assert_eq!(f.node(a).unwrap().state, NodeState::Active);
    }

    #[test]
    fn inside_focus_ring_can_decay_to_preview() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 49.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        assert!(f.touch(a, 0));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let ring = default_focus_ring();
        let mut policy = FocusRingDecayPolicy::new();
        policy.inside_to_preview_ms = 1000;
        policy.preview_to_node_ms = 5000;

        tick_decay_focus_ring(&mut f, &vp, 1500, ring, policy, None);

        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Warm);
        assert_eq!(f.node(a).unwrap().state, NodeState::Preview);
    }

    #[test]
    fn inside_focus_ring_can_decay_to_node() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        assert!(f.touch(a, 0));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let ring = default_focus_ring();
        let mut policy = FocusRingDecayPolicy::new();
        policy.inside_to_preview_ms = 1000;
        policy.preview_to_node_ms = 5000;

        tick_decay_focus_ring(&mut f, &vp, 7000, ring, policy, None);

        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Cold);
        assert_eq!(f.node(a).unwrap().state, NodeState::Node);
    }

    #[test]
    fn outside_focus_ring_goes_cold_immediately() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 500.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        assert!(f.touch(a, 0));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let ring = default_focus_ring();
        let policy = FocusRingDecayPolicy::new();

        tick_decay_focus_ring(&mut f, &vp, 1000, ring, policy, None);

        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Cold);
        assert_eq!(f.node(a).unwrap().state, NodeState::Node);
    }

    #[test]
    fn focused_node_stays_hot_with_focus_ring_policy() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 500.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        assert!(f.touch(a, 0));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let ring = default_focus_ring();
        let policy = FocusRingDecayPolicy::new();

        tick_decay_focus_ring(&mut f, &vp, 999_999, ring, policy, Some(a));

        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Hot);
        assert_eq!(f.node(a).unwrap().state, NodeState::Active);
    }
}
