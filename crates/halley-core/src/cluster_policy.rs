use std::collections::{HashMap, HashSet};

use crate::cluster::ClusterId;
use crate::field::{Field, NodeId, NodeKind, Vec2};

/// Configuration for auto cluster formation.
///
/// Default idea:
/// - Nodes that remain within `distance_px` of each other for `dwell_ms`
///   can be clustered (as a connected component).
/// - Can be disabled (`enabled = false`) or tuned.
#[derive(Clone, Copy, Debug)]
pub struct ClusterPolicy {
    pub enabled: bool,

    /// Max distance between nodes to be considered "near".
    pub distance_px: f32,

    /// How long nodes must remain near to qualify (ms).
    pub dwell_ms: u64,

    /// Minimum component size to form a cluster.
    pub min_members: usize,

    /// Include anchored nodes in auto-clustering.
    pub include_anchored: bool,

    /// Include currently Active windows in auto-clustering.
    /// Keeping this false avoids grouping freshly opened app windows.
    pub include_active: bool,
}

impl Default for ClusterPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            distance_px: 220.0,
            dwell_ms: 1_500,
            min_members: 2,
            include_anchored: false,
            include_active: false,
        }
    }
}

/// Stateful tracker for cluster formation.
/// Keep this in the outer loop / world manager, not inside Field.
#[derive(Clone, Debug, Default)]
pub struct ClusterFormationState {
    /// When did we first observe this pair within threshold?
    /// Key is ordered (min, max) for stability.
    near_since: HashMap<(NodeId, NodeId), u64>,
}

// Squared edge-to-edge gap between two axis-aligned node footprints.
// 0 means touching or overlapping.
fn footprint_gap2(a_pos: Vec2, a_size: Vec2, b_pos: Vec2, b_size: Vec2) -> f32 {
    let dx = (a_pos.x - b_pos.x).abs() - (a_size.x.abs() * 0.5 + b_size.x.abs() * 0.5);
    let dy = (a_pos.y - b_pos.y).abs() - (a_size.y.abs() * 0.5 + b_size.y.abs() * 0.5);
    let gx = dx.max(0.0);
    let gy = dy.max(0.0);
    gx * gx + gy * gy
}

fn ordered_pair(a: NodeId, b: NodeId) -> (NodeId, NodeId) {
    if a.as_u64() <= b.as_u64() {
        (a, b)
    } else {
        (b, a)
    }
}

/// Tick auto cluster formation.
///
/// Returns any newly created ClusterIds.
pub fn tick_cluster_formation(
    field: &mut Field,
    now_ms: u64,
    policy: ClusterPolicy,
    state: &mut ClusterFormationState,
) -> Vec<ClusterId> {
    if !policy.enabled {
        state.near_since.clear();
        return Vec::new();
    }

    // Exclude nodes already belonging to any cluster.
    let mut already_clustered: HashSet<NodeId> = HashSet::new();
    for c in field.clusters_iter() {
        for &m in &c.members {
            already_clustered.insert(m);
        }
        if let Some(core) = c.core {
            already_clustered.insert(core);
        }
    }

    // Candidate nodes: visible, Surface (not Core), not detached/hidden, not already in cluster.
    let mut candidates: Vec<NodeId> = field
        .nodes()
        .keys()
        .copied()
        .filter(|&id| field.is_visible(id))
        .filter(|&id| !already_clustered.contains(&id))
        .filter(|&id| field.node(id).is_some_and(|n| n.kind == NodeKind::Surface))
        .filter(|&id| {
            if policy.include_active {
                true
            } else {
                field.node(id).is_some_and(|n| {
                    n.state != crate::field::NodeState::Active
                        && n.state != crate::field::NodeState::Core
                })
            }
        })
        .filter(|&id| {
            if policy.include_anchored {
                true
            } else {
                field.node(id).is_some_and(|n| !n.pinned)
            }
        })
        .collect();

    // Deterministic ordering
    candidates.sort_by_key(|id| id.as_u64());

    let thr2 = policy.distance_px * policy.distance_px;

    // Track which pairs are "currently near" this tick.
    let mut near_now: HashSet<(NodeId, NodeId)> = HashSet::new();

    // Identify near pairs and update timers.
    for i in 0..candidates.len() {
        for j in (i + 1)..candidates.len() {
            let a = candidates[i];
            let b = candidates[j];

            let (pa, sa, pb, sb) = match (field.node(a), field.node(b)) {
                (Some(na), Some(nb)) => (na.pos, na.footprint, nb.pos, nb.footprint),
                _ => continue,
            };

            if footprint_gap2(pa, sa, pb, sb) <= thr2 {
                let key = ordered_pair(a, b);
                near_now.insert(key);
                state.near_since.entry(key).or_insert(now_ms);
            }
        }
    }

    // Drop any pairs that are no longer near.
    state.near_since.retain(|k, _| near_now.contains(k));

    // Build a graph of "mature" near-pairs (dwell satisfied).
    let mut adj: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for (&(a, b), &since) in state.near_since.iter() {
        if now_ms.saturating_sub(since) >= policy.dwell_ms {
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
    }

    // Find connected components among candidates based on mature edges.
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut created: Vec<ClusterId> = Vec::new();

    for &start in &candidates {
        if seen.contains(&start) {
            continue;
        }
        if !adj.contains_key(&start) {
            seen.insert(start);
            continue;
        }

        // DFS
        let mut stack = vec![start];
        let mut comp: Vec<NodeId> = Vec::new();
        seen.insert(start);

        while let Some(x) = stack.pop() {
            comp.push(x);
            if let Some(neis) = adj.get(&x) {
                for &y in neis {
                    if !seen.contains(&y) {
                        seen.insert(y);
                        stack.push(y);
                    }
                }
            }
        }

        // Only form a cluster if large enough.
        if comp.len() >= policy.min_members {
            // Attempt to create the cluster.
            if let Some(cid) = field.create_cluster(comp.clone()) {
                created.push(cid);

                // Clear any pair timers involving these nodes to avoid instant re-cluster.
                let comp_set: HashSet<NodeId> = comp.into_iter().collect();
                state
                    .near_since
                    .retain(|&(a, b), _| !comp_set.contains(&a) && !comp_set.contains(&b));
            }
        }
    }

    created
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Vec2;

    #[test]
    fn forms_cluster_after_dwell() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let b = f.spawn_surface("B", Vec2 { x: 50.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let mut st = ClusterFormationState::default();
        let policy = ClusterPolicy {
            enabled: true,
            distance_px: 100.0,
            dwell_ms: 1_000,
            min_members: 2,
            include_anchored: false,
            include_active: true,
        };

        // t=0: start timer but not mature
        let c0 = tick_cluster_formation(&mut f, 0, policy, &mut st);
        assert!(c0.is_empty());

        // t=999: still not mature
        let c1 = tick_cluster_formation(&mut f, 999, policy, &mut st);
        assert!(c1.is_empty());

        // t=1000: should form
        let c2 = tick_cluster_formation(&mut f, 1000, policy, &mut st);
        assert_eq!(c2.len(), 1);

        // nodes should now be in a cluster
        let cid = c2[0];
        let cl = f.cluster(cid).unwrap();
        assert!(cl.contains(a));
        assert!(cl.contains(b));
    }

    #[test]
    fn disabled_clears_state_and_does_nothing() {
        let mut f = Field::new();
        let _a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let _b = f.spawn_surface("B", Vec2 { x: 50.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let mut st = ClusterFormationState::default();
        st.near_since.insert((NodeId::new(1), NodeId::new(2)), 0);

        let policy = ClusterPolicy {
            enabled: false,
            ..Default::default()
        };
        let out = tick_cluster_formation(&mut f, 1234, policy, &mut st);
        assert!(out.is_empty());
        assert!(st.near_since.is_empty());
    }

    #[test]
    fn moving_apart_resets_dwell_timer() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let b = f.spawn_surface("B", Vec2 { x: 50.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let mut st = ClusterFormationState::default();
        let policy = ClusterPolicy {
            enabled: true,
            distance_px: 100.0,
            dwell_ms: 1_000,
            min_members: 2,
            include_anchored: false,
            include_active: true,
        };

        // Start near
        assert!(tick_cluster_formation(&mut f, 0, policy, &mut st).is_empty());

        // Move far away before dwell
        assert!(f.carry(
            b,
            Vec2 {
                x: 10_000.0,
                y: 0.0
            }
        ));
        assert!(tick_cluster_formation(&mut f, 500, policy, &mut st).is_empty());

        // Move back near; timer should restart
        assert!(f.carry(b, Vec2 { x: 50.0, y: 0.0 }));
        assert!(tick_cluster_formation(&mut f, 800, policy, &mut st).is_empty());

        // Not enough time since "back near" (800 -> 1700 is 900)
        assert!(tick_cluster_formation(&mut f, 1700, policy, &mut st).is_empty());

        // Now enough since restarted (800 -> 1800 is 1000)
        let out = tick_cluster_formation(&mut f, 1800, policy, &mut st);
        assert_eq!(out.len(), 1);
        let cid = out[0];
        let cl = f.cluster(cid).unwrap();
        assert!(cl.contains(a));
        assert!(cl.contains(b));
    }
}
