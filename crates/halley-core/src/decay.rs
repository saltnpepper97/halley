use crate::field::{Field, NodeId, NodeKind, Vec2};

use crate::viewport::{FocusRings, RingZone, Viewport};

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

    // Collect ids first to avoid borrow fights.
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
pub struct RingDecayPolicy {
    /// In the Primary ring:
    /// - age < primary_to_preview_ms => Hot/Active
    /// - age < primary_to_preview_ms + primary_preview_to_node_ms => Warm/Preview
    /// - otherwise => Cold/Node
    pub primary_to_preview_ms: u64,

    /// Additional time spent in Preview before Node once primary_to_preview_ms is reached.
    pub primary_preview_to_node_ms: u64,

    /// When in Secondary ring:
    /// - immediately Warm
    pub secondary_preview: bool,

    /// When in Secondary ring and age >= secondary_to_node_ms => Cold
    pub secondary_to_node_ms: u64,

    /// When Outside secondary ring => immediately Cold
    pub outside_immediate_cold: bool,
}

impl RingDecayPolicy {
    pub fn new(secondary_to_node_ms: u64) -> Self {
        Self {
            primary_to_preview_ms: 1_200_000,
            primary_preview_to_node_ms: 60_000,
            secondary_preview: true,
            secondary_to_node_ms,
            outside_immediate_cold: true,
        }
    }
}

/// Ring-aware decay:
/// - Primary ring: Hot, then Preview, then Node based on primary timers
/// - Secondary ring: Warm immediately; after secondary_to_node_ms => Cold
/// - Outside: Cold immediately
/// - Focused node: Hot
/// - Core nodes do not decay
pub fn tick_decay_rings(
    field: &mut Field,
    vp: &Viewport,
    now_ms: u64,
    rings: FocusRings,
    policy: RingDecayPolicy,
    focused: Option<NodeId>,
) {
    // Collect ids first to avoid borrow fights.
    let ids: Vec<NodeId> = field.nodes().keys().copied().collect();

    for id in ids {
        // Copy the pieces we need, then release the immutable borrow before mutating.
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

        let zone = dominant_ring_zone(rings, vp.center, pos, intrinsic_size);

        match zone {
            RingZone::Primary => {
                let age = now_ms.saturating_sub(last_touch_ms);
                let to_preview = policy.primary_to_preview_ms;
                let to_node = to_preview.saturating_add(policy.primary_preview_to_node_ms);
                if age >= to_node {
                    field.set_decay_level(id, DecayLevel::Cold);
                } else if age >= to_preview {
                    field.set_decay_level(id, DecayLevel::Warm);
                } else {
                    field.set_decay_level(id, DecayLevel::Hot);
                }
            }
            RingZone::Secondary => {
                // Warm immediately (or Hot if configured)
                if policy.secondary_preview {
                    field.set_decay_level(id, DecayLevel::Warm);
                } else {
                    field.set_decay_level(id, DecayLevel::Hot);
                }

                let age = now_ms.saturating_sub(last_touch_ms);
                if age >= policy.secondary_to_node_ms {
                    field.set_decay_level(id, DecayLevel::Cold);
                }
            }
            RingZone::Outside => {
                if policy.outside_immediate_cold {
                    field.set_decay_level(id, DecayLevel::Cold);
                } else {
                    field.set_decay_level(id, DecayLevel::Cold);
                }
            }
        }
    }
}

fn dominant_ring_zone(rings: FocusRings, vp_center: Vec2, pos: Vec2, footprint: Vec2) -> RingZone {
    // Approximate "where the window mostly is" using a small deterministic sample grid.
    // Majority semantics:
    // - >50% in primary => Primary
    // - else >50% inside secondary (primary+secondary) => Secondary
    // - else => Outside
    let w = footprint.x.abs();
    let h = footprint.y.abs();
    if w < 1.0 || h < 1.0 {
        return rings.zone(vp_center, pos);
    }

    let sx = 5usize;
    let sy = 5usize;
    let mut c_primary = 0usize;
    let mut c_secondary = 0usize;

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
            match rings.zone(vp_center, p) {
                RingZone::Primary => c_primary += 1,
                RingZone::Secondary => c_secondary += 1,
                RingZone::Outside => {}
            }
        }
    }

    let total = (sx * sy) as f32;
    let p_primary = c_primary as f32 / total;
    let p_inside_secondary = (c_primary + c_secondary) as f32 / total;

    if p_primary > 0.5 {
        RingZone::Primary
    } else if p_inside_secondary > 0.5 {
        RingZone::Secondary
    } else {
        RingZone::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Vec2;
    use crate::viewport::{EyeRing, FocusRings};

    fn default_rings() -> FocusRings {
        FocusRings {
            primary: EyeRing::new(50.0, 30.0, 0.0),
            secondary: EyeRing::new(200.0, 120.0, 0.0),
        }
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
    fn primary_ring_never_decays() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        assert!(f.touch(a, 0));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let rings = default_rings();
        let policy = RingDecayPolicy::new(5 * 60 * 1000);

        tick_decay_rings(&mut f, &vp, 999_999, rings, policy, None);

        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Hot);
        assert_eq!(f.node(a).unwrap().state, NodeState::Active);
    }

    #[test]
    fn near_primary_edge_defaults_to_preview() {
        let mut f = Field::new();
        // default_rings().primary radius_x is 50; x=49 is barely inside.
        let a = f.spawn_surface("A", Vec2 { x: 49.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        assert!(f.touch(a, 0));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let rings = default_rings();
        let policy = RingDecayPolicy::new(5_000);

        tick_decay_rings(&mut f, &vp, 1_000, rings, policy, None);
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Warm);
        assert_eq!(f.node(a).unwrap().state, NodeState::Preview);
    }

    #[test]
    fn majority_secondary_beats_center_inside_primary() {
        let mut f = Field::new();
        // Center is just inside primary ring (x=49 in rx=50),
        // but wide footprint puts most samples in secondary.
        let a = f.spawn_surface("A", Vec2 { x: 49.0, y: 0.0 }, Vec2 { x: 200.0, y: 40.0 });
        assert!(f.touch(a, 0));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let rings = default_rings();
        let mut policy = RingDecayPolicy::new(5_000);
        policy.primary_hot_inner_frac = 1.0; // disable inner hot cut to isolate zone classification

        tick_decay_rings(&mut f, &vp, 1_000, rings, policy, None);
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Warm);
        assert_eq!(f.node(a).unwrap().state, NodeState::Preview);
    }

    #[test]
    fn mostly_outside_secondary_goes_node_immediately() {
        let mut f = Field::new();
        // Only a small left slice is inside secondary; most of area is outside.
        let a = f.spawn_surface("A", Vec2 { x: 210.0, y: 0.0 }, Vec2 { x: 80.0, y: 80.0 });
        assert!(f.touch(a, 0));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let rings = default_rings();
        let policy = RingDecayPolicy::new(5_000);

        tick_decay_rings(&mut f, &vp, 1_000, rings, policy, None);
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Cold);
        assert_eq!(f.node(a).unwrap().state, NodeState::Node);
    }

    #[test]
    fn secondary_ring_goes_preview_then_node_after_threshold() {
        let mut f = Field::new();
        // Place in secondary ring but outside primary.
        let a = f.spawn_surface("A", Vec2 { x: 100.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        assert!(f.touch(a, 0));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let rings = default_rings();
        let policy = RingDecayPolicy::new(5_000);

        tick_decay_rings(&mut f, &vp, 1_000, rings, policy, None);
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Warm);
        assert_eq!(f.node(a).unwrap().state, NodeState::Preview);

        tick_decay_rings(&mut f, &vp, 6_000, rings, policy, None);
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Cold);
        assert_eq!(f.node(a).unwrap().state, NodeState::Node);
    }

    #[test]
    fn outside_secondary_immediately_cold() {
        let mut f = Field::new();
        let a = f.spawn_surface(
            "A",
            Vec2 {
                x: 10_000.0,
                y: 0.0,
            },
            Vec2 { x: 10.0, y: 10.0 },
        );
        assert!(f.touch(a, 0));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let rings = default_rings();
        let policy = RingDecayPolicy::new(5_000);

        tick_decay_rings(&mut f, &vp, 1_000, rings, policy, None);

        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Cold);
        assert_eq!(f.node(a).unwrap().state, NodeState::Node);
    }
}
