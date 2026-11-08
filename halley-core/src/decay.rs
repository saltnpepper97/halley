use crate::field::{Field, NodeId, NodeState};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecayLevel {
    Hot,  // Active
    Cold, // Node
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecayClass {
    Protected,
    OnScreen,
    OffScreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedDecayPolicy {
    pub inside_ms: u64,
    pub outside_ms: u64,
}

impl Default for TimedDecayPolicy {
    fn default() -> Self {
        Self {
            inside_ms: 1_800_000,
            outside_ms: 180_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Eligibility {
    class: DecayClass,
    since_ms: u64,
}

/// Pure monotonic eligibility state. A class change always starts a new
/// clock, which is the key invariant that prevents an old last-focus time
/// from collapsing a window as soon as focus leaves it.
#[derive(Default)]
pub struct DecayTracker {
    eligibility: HashMap<NodeId, Eligibility>,
}

impl DecayTracker {
    pub fn update(
        &mut self,
        id: NodeId,
        class: DecayClass,
        now_ms: u64,
        policy: TimedDecayPolicy,
    ) -> Option<u64> {
        let entry = self.eligibility.entry(id).or_insert(Eligibility {
            class,
            since_ms: now_ms,
        });
        if entry.class != class {
            *entry = Eligibility {
                class,
                since_ms: now_ms,
            };
        }
        match class {
            DecayClass::Protected => None,
            DecayClass::OnScreen => {
                let deadline = entry.since_ms.saturating_add(policy.inside_ms);
                (now_ms >= deadline).then_some(deadline)
            }
            DecayClass::OffScreen => {
                let deadline = entry.since_ms.saturating_add(policy.outside_ms);
                (now_ms >= deadline).then_some(deadline)
            }
        }
    }

    pub fn next_deadline(&self, now_ms: u64, policy: TimedDecayPolicy) -> Option<u64> {
        self.eligibility
            .values()
            .filter_map(|entry| match entry.class {
                DecayClass::Protected => None,
                DecayClass::OnScreen => Some(entry.since_ms.saturating_add(policy.inside_ms)),
                DecayClass::OffScreen => Some(entry.since_ms.saturating_add(policy.outside_ms)),
            })
            .filter(|deadline| *deadline > now_ms)
            .min()
    }

    pub fn remove(&mut self, id: NodeId) {
        self.eligibility.remove(&id);
    }

    pub fn reset(&mut self) {
        self.eligibility.clear();
    }
}

#[cfg(test)]
mod timed_tests {
    use super::{DecayClass, DecayTracker, TimedDecayPolicy};
    use crate::field::NodeId;

    fn id(value: u64) -> NodeId {
        NodeId::new(value)
    }

    #[test]
    fn class_change_restarts_the_timer() {
        let mut tracker = DecayTracker::default();
        let policy = TimedDecayPolicy {
            outside_ms: 100,
            inside_ms: 1_000,
        };
        assert_eq!(
            tracker.update(id(1), DecayClass::OffScreen, 10, policy),
            None
        );
        assert_eq!(
            tracker.update(id(1), DecayClass::OnScreen, 109, policy),
            None
        );
        assert_eq!(
            tracker.update(id(1), DecayClass::OnScreen, 1_108, policy),
            None
        );
        assert_eq!(
            tracker.update(id(1), DecayClass::OnScreen, 1_109, policy),
            Some(1_109)
        );
    }

    #[test]
    fn focus_protection_does_not_reuse_an_old_deadline() {
        let mut tracker = DecayTracker::default();
        let policy = TimedDecayPolicy {
            outside_ms: 100,
            inside_ms: 1_000,
        };
        tracker.update(id(1), DecayClass::OffScreen, 0, policy);
        tracker.update(id(1), DecayClass::Protected, 99, policy);
        assert_eq!(
            tracker.update(id(1), DecayClass::OffScreen, 500, policy),
            None
        );
        assert_eq!(
            tracker.update(id(1), DecayClass::OffScreen, 600, policy),
            Some(600)
        );
    }

    #[test]
    fn next_deadline_ignores_protected_nodes() {
        let mut tracker = DecayTracker::default();
        let policy = TimedDecayPolicy {
            outside_ms: 100,
            inside_ms: 1_000,
        };
        tracker.update(id(1), DecayClass::Protected, 0, policy);
        tracker.update(id(2), DecayClass::OnScreen, 10, policy);
        tracker.update(id(3), DecayClass::OffScreen, 20, policy);
        assert_eq!(tracker.next_deadline(20, policy), Some(120));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecayPolicy {
    /// Age >= node_after_ms => Cold/Node
    pub node_after_ms: u64,
}

impl DecayPolicy {
    pub fn new(node_after_ms: u64) -> Self {
        Self { node_after_ms }
    }
}

/// Advance representation decay for all nodes based on time since last touch.
/// - `now_ms` is a monotonic ms counter controlled by the outer loop.
/// - `focused` is pinned Hot.
/// - Core nodes do not decay (they remain handles).
pub fn tick_decay(field: &mut Field, now_ms: u64, policy: DecayPolicy, focused: Option<NodeId>) {
    let ids: Vec<NodeId> = field.nodes().keys().copied().collect();

    for id in ids {
        let Some(n) = field.node(id) else { continue };

        if n.state == NodeState::Core {
            continue;
        }

        // NOTE: this used to also skip cluster members here via
        // `field.cluster_id_for_member_public()`. `Field` is permanently
        // cluster-blind now, so that exclusion is the caller's job: filter
        // `ids` before calling this, using `World`'s per-space
        // `ClusterRegistry` (see `cluster.rs`/`world.rs`).

        if Some(id) == focused {
            let _ = field.set_decay_level(id, DecayLevel::Hot);
            continue;
        }

        let age = now_ms.saturating_sub(n.last_touch_ms);

        if age >= policy.node_after_ms {
            let _ = field.set_decay_level(id, DecayLevel::Cold);
        } else {
            let _ = field.set_decay_level(id, DecayLevel::Hot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Vec2;

    #[test]
    fn decays_hot_to_cold() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        assert!(f.touch(a, 0));
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Hot);
        assert_eq!(f.node(a).unwrap().state, NodeState::Active);

        let policy = DecayPolicy::new(5000);

        tick_decay(&mut f, 1500, policy, None);
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Hot);
        assert_eq!(f.node(a).unwrap().state, NodeState::Active);

        tick_decay(&mut f, 6000, policy, None);
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Cold);
        assert_eq!(f.node(a).unwrap().state, NodeState::Node);
    }

    #[test]
    fn focused_node_stays_hot() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        assert!(f.touch(a, 0));

        let policy = DecayPolicy::new(5000);

        tick_decay(&mut f, 6000, policy, Some(a));
        assert_eq!(f.node(a).unwrap().decay, DecayLevel::Hot);
        assert_eq!(f.node(a).unwrap().state, NodeState::Active);
    }

    // `core_does_not_decay` and `clustered_members_do_not_decay` used to
    // live here, exercising `Field`'s old built-in cluster awareness
    // (`create_cluster`/`collapse_cluster`). That awareness is gone for
    // good - `Field` no longer has any cluster concept at all - so those
    // scenarios don't belong in this file anymore; equivalent coverage now
    // lives in `cluster.rs`'s own tests (e.g. `collapse_cluster_creates_
    // core_and_shrinks_members`), and cluster-membership exclusion from
    // decay ticking is exercised wherever the caller does that filtering.
}
