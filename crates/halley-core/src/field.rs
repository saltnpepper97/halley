use crate::cluster::{Cluster, ClusterId};
use crate::decay::DecayLevel;
use crate::viewport::Viewport;
use crate::visual::{NodeVisual, VisualParams, build_visuals, build_visuals_in_view};

use std::collections::HashMap;

/// A stable identity for anything that exists in the Field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(u64);

impl NodeId {
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 2D point / vector in Field coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// Axis-aligned rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    pub fn width(self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn contains(self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    pub fn intersects(self, other: Rect) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
    }
}

/// Semantic visibility flags.
/// This is NOT rendering; it's "experience-layer existence":
/// - hidden nodes should be skipped by focus/nav/bearings/in_view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visibility(u8);

impl Visibility {
    pub const NONE: Self = Self(0);

    /// Hidden because user/system explicitly hid it.
    pub const HIDDEN_EXPLICIT: Self = Self(1 << 0);

    /// Hidden because its cluster is collapsed.
    pub const HIDDEN_BY_CLUSTER: Self = Self(1 << 1);

    /// Node exists in storage, but is currently detached from the experience layer.
    pub const DETACHED: Self = Self(1 << 2);

    pub fn is_hidden(self) -> bool {
        (self.0 & (Self::HIDDEN_EXPLICIT.0 | Self::HIDDEN_BY_CLUSTER.0 | Self::DETACHED.0)) != 0
    }

    pub fn has(self, flag: Self) -> bool {
        (self.0 & flag.0) != 0
    }

    pub fn set(&mut self, flag: Self, on: bool) {
        if on {
            self.0 |= flag.0;
        } else {
            self.0 &= !flag.0;
        }
    }

    pub fn clear(&mut self, flag: Self) {
        self.0 &= !flag.0;
    }
}

/// What kind of thing a node represents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Surface,
    Core, // collapsed cluster handle
}

/// Representation state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeState {
    Active,
    Drifting,
    Node, // dot with label
    Core, // only meaningful for Core kind
}

/// A Node is the universal "thing" that exists in the Field.
#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub state: NodeState,

    pub label: String,

    /// Center position in Field coordinates.
    pub pos: Vec2,

    pub intrinsic_size: Vec2, // "real" size for Active
    pub footprint: Vec2,      // spatial occupancy right now
    pub resize_footprint: Option<Vec2>,

    /// Pinned in place (movement constraint). This was previously called `anchored`.
    pub pinned: bool,

    /// Routing marker: important node that should always be surfaced in navigation
    /// (Bearings/Lens). Does NOT bypass visibility rules.
    pub anchor: bool,

    /// Semantic visibility / participation flags.
    pub visibility: Visibility,

    pub last_touch_ms: u64,
    pub decay: DecayLevel,
}

/// The infinite 2D space containing all Nodes.
pub struct Field {
    next_node: u64,
    nodes: HashMap<NodeId, Node>,

    next_cluster: u64,
    clusters: HashMap<ClusterId, Cluster>,
}

impl Field {
    pub fn new() -> Self {
        Self {
            next_node: 1,
            nodes: HashMap::new(),
            next_cluster: 1,
            clusters: HashMap::new(),
        }
    }

    pub fn nodes(&self) -> &HashMap<NodeId, Node> {
        &self.nodes
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    /// Spawn a basic Surface node.
    pub fn spawn_surface(&mut self, label: impl Into<String>, pos: Vec2, size: Vec2) -> NodeId {
        let id = NodeId(self.next_node);
        self.next_node += 1;

        let node = Node {
            id,
            kind: NodeKind::Surface,
            state: NodeState::Active,
            label: label.into(),
            pos,
            intrinsic_size: size,
            footprint: size,
            resize_footprint: None,
            pinned: false,
            anchor: false,
            visibility: Visibility::NONE,
            last_touch_ms: 0,
            decay: DecayLevel::Hot,
        };

        self.nodes.insert(id, node);
        id
    }

    /// Remove a node from the Field.
    pub fn remove(&mut self, id: NodeId) -> Option<Node> {
        self.nodes.remove(&id)
    }

    /// Set/unset movement pinning.
    pub fn set_pinned(&mut self, id: NodeId, on: bool) -> bool {
        let Some(n) = self.nodes.get_mut(&id) else {
            return false;
        };
        n.pinned = on;
        true
    }

    /// Back-compat alias: previously `anchor()` meant "pinned in place".
    /// Prefer `set_pinned()`. (We keep this to avoid churn in other modules.)
    pub fn anchor(&mut self, id: NodeId, on: bool) -> bool {
        self.set_pinned(id, on)
    }

    /// Set/unset routing anchor marker.
    pub fn set_anchor(&mut self, id: NodeId, on: bool) -> bool {
        let Some(n) = self.nodes.get_mut(&id) else {
            return false;
        };
        n.anchor = on;
        true
    }

    pub fn is_anchor(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|n| n.anchor)
    }

    /// Return all experience-visible anchors (stable order).
    pub fn anchors(&self) -> Vec<NodeId> {
        let mut out: Vec<NodeId> = self
            .nodes
            .iter()
            .filter_map(|(&id, n)| (self.is_visible(id) && n.anchor).then_some(id))
            .collect();
        out.sort_by_key(|id| id.as_u64());
        out
    }

    /// Carry a node to a new position (respects pinning).
    pub fn carry(&mut self, id: NodeId, to: Vec2) -> bool {
        let Some(n) = self.nodes.get_mut(&id) else {
            return false;
        };
        if n.pinned {
            return false;
        }
        n.pos = to;
        true
    }

    /// Axis-aligned bounds in Field space.
    pub fn bounds(&self, id: NodeId) -> Option<Rect> {
        let n = self.nodes.get(&id)?;
        Some(Self::bounds_for_node(n))
    }

    fn bounds_for_node(n: &Node) -> Rect {
        let half = Vec2 {
            x: n.footprint.x * 0.5,
            y: n.footprint.y * 0.5,
        };
        Rect {
            min: Vec2 {
                x: n.pos.x - half.x,
                y: n.pos.y - half.y,
            },
            max: Vec2 {
                x: n.pos.x + half.x,
                y: n.pos.y + half.y,
            },
        }
    }

    /// Return nodes that intersect the view rect AND are experience-visible.
    pub fn in_view(&self, view: Rect) -> Vec<NodeId> {
        self.nodes
            .keys()
            .copied()
            .filter(|&id| self.is_visible(id))
            .filter(|&id| self.bounds(id).is_some_and(|b| b.intersects(view)))
            .collect()
    }

    /// Return all nodes that intersect the view rect (includes hidden nodes).
    pub fn in_view_all(&self, view: Rect) -> Vec<NodeId> {
        self.nodes
            .keys()
            .copied()
            .filter(|&id| self.bounds(id).is_some_and(|b| b.intersects(view)))
            .collect()
    }

    /// True iff the node exists and is not hidden by any visibility reason.
    pub fn is_visible(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|n| !n.visibility.is_hidden())
    }

    /// Explicit hide/show (does not touch cluster-hidden).
    pub fn set_hidden(&mut self, id: NodeId, on: bool) -> bool {
        let Some(n) = self.node_mut(id) else {
            return false;
        };
        n.visibility.set(Visibility::HIDDEN_EXPLICIT, on);
        true
    }

    /// Detach/attach.
    pub fn set_detached(&mut self, id: NodeId, on: bool) -> bool {
        let Some(n) = self.node_mut(id) else {
            return false;
        };
        n.visibility.set(Visibility::DETACHED, on);
        true
    }

    /// Record interaction with a node.
    pub fn touch(&mut self, id: NodeId, now_ms: u64) -> bool {
        let Some(n) = self.node_mut(id) else {
            return false;
        };
        n.last_touch_ms = now_ms;
        n.decay = DecayLevel::Hot;

        // Core is a handle; it doesn't switch representation via touch.
        if n.kind != NodeKind::Core {
            n.state = NodeState::Active;
            n.footprint = n.resize_footprint.unwrap_or(n.intrinsic_size);
        }

        true
    }

    /// Apply a decay level to a node by mapping it to representation state.
    pub fn set_decay_level(&mut self, id: NodeId, level: DecayLevel) -> bool {
        let Some(n) = self.node(id) else {
            return false;
        };

        // Core is a handle; it doesn't decay away.
        if n.kind == NodeKind::Core {
            return true;
        }

        let state = match level {
            DecayLevel::Hot => NodeState::Active,
            DecayLevel::Cold => NodeState::Node,
        };

        if let Some(nm) = self.node_mut(id) {
            nm.decay = level;
        }
        self.set_state(id, state)
    }

    pub fn set_state(&mut self, id: NodeId, state: NodeState) -> bool {
        const DOT: Vec2 = Vec2 { x: 24.0, y: 24.0 };
        const CORE: Vec2 = Vec2 { x: 48.0, y: 48.0 };

        let Some(n) = self.nodes.get_mut(&id) else {
            return false;
        };

        n.state = state.clone();
        n.footprint = match state {
            NodeState::Active => n.resize_footprint.unwrap_or(n.intrinsic_size),
            NodeState::Drifting => n.footprint,
            NodeState::Node => DOT,
            NodeState::Core => CORE,
        };

        true
    }

    pub fn set_resize_footprint(&mut self, id: NodeId, size: Option<Vec2>) -> bool {
        let Some(n) = self.nodes.get_mut(&id) else {
            return false;
        };

        n.resize_footprint = size;
        if matches!(n.state, NodeState::Active) {
            n.footprint = n.resize_footprint.unwrap_or(n.intrinsic_size);
        }

        true
    }

    pub fn sync_active_footprint_to_intrinsic(&mut self, id: NodeId) -> bool {
        let Some(n) = self.nodes.get_mut(&id) else {
            return false;
        };
        n.resize_footprint = None;
        if matches!(n.state, NodeState::Active) {
            n.footprint = n.intrinsic_size;
        }
        true
    }

    /// Canonical visuals feed: for full behavior, use `build_visuals()` directly.
    /// These helpers delegate to the same implementation to avoid drift.
    pub fn visuals_visible(&self) -> Vec<NodeVisual> {
        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 0.0, y: 0.0 });
        build_visuals(self, &vp, VisualParams::default())
    }

    pub fn visuals_in_view(&self, view: Rect) -> Vec<NodeVisual> {
        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 0.0, y: 0.0 });
        build_visuals_in_view(self, &vp, view, VisualParams::default())
    }

    pub fn cluster(&self, id: ClusterId) -> Option<&Cluster> {
        self.clusters.get(&id)
    }

    pub fn cluster_mut(&mut self, id: ClusterId) -> Option<&mut Cluster> {
        self.clusters.get_mut(&id)
    }

    /// Remove a cluster record (needed for cross-space transfer).
    pub fn remove_cluster(&mut self, id: ClusterId) -> Option<Cluster> {
        self.clusters.remove(&id)
    }

    /// Insert an existing cluster record (needed for cross-space transfer).
    pub fn insert_cluster(&mut self, cluster: Cluster) {
        // keep ids stable; bump next_cluster so future creates don’t collide
        self.next_cluster = self.next_cluster.max(cluster.id.as_u64() + 1);
        self.clusters.insert(cluster.id, cluster);
    }

    pub fn create_cluster(&mut self, members: Vec<NodeId>) -> Option<ClusterId> {
        if members.is_empty() {
            return None;
        }

        if members.iter().any(|&id| self.node(id).is_none()) {
            return None;
        }

        let id = ClusterId::new(self.next_cluster);
        self.next_cluster += 1;

        self.clusters.insert(id, Cluster::new(id, members));
        Some(id)
    }

    pub fn cluster_id_for_core_public(&self, core: NodeId) -> Option<ClusterId> {
        self.clusters
            .iter()
            .find_map(|(&cid, c)| (c.core == Some(core)).then_some(cid))
    }

    pub fn cluster_id_for_member_public(&self, member: NodeId) -> Option<ClusterId> {
        self.clusters
            .iter()
            .find_map(|(&cid, c)| c.members.contains(&member).then_some(cid))
    }

    pub fn add_member_to_cluster(&mut self, id: ClusterId, member: NodeId) -> bool {
        if self.node(member).is_none() || self.cluster_id_for_member_public(member).is_some() {
            return false;
        }
        let Some(cluster) = self.clusters.get_mut(&id) else {
            return false;
        };
        if cluster.members.contains(&member) {
            return false;
        }
        cluster.members.push(member);
        true
    }

    pub fn remove_member_from_cluster(&mut self, id: ClusterId, member: NodeId) -> bool {
        let Some(cluster) = self.clusters.get_mut(&id) else {
            return false;
        };
        let before = cluster.members.len();
        cluster.members.retain(|&id| id != member);
        if let Some(active) = cluster.active.as_mut() {
            active.weights.remove(&member);
            if let Some(majors) = active.majors_override.as_mut() {
                majors.retain(|&id| id != member);
                if majors.is_empty() {
                    active.majors_override = None;
                }
            }
        }
        before != cluster.members.len()
    }

    pub fn sync_cluster_core_from_members(&mut self, id: ClusterId) -> Option<NodeId> {
        let (members, core_id) = {
            let cluster = self.clusters.get(&id)?;
            (cluster.members.clone(), cluster.core)
        };
        if members.is_empty() {
            return core_id;
        }
        let mut sum = Vec2 { x: 0.0, y: 0.0 };
        for member in &members {
            let node = self.node(*member)?;
            sum.x += node.pos.x;
            sum.y += node.pos.y;
        }
        let len = members.len() as f32;
        let center = Vec2 {
            x: sum.x / len,
            y: sum.y / len,
        };
        if let Some(core_id) = core_id
            && let Some(node) = self.node_mut(core_id)
        {
            node.pos = center;
        }
        core_id
    }

    /// Drag the cluster by its core handle.
    pub fn carry_cluster_by_core(&mut self, core: NodeId, to: Vec2) -> bool {
        let cid = match self.cluster_id_for_core_public(core) {
            Some(cid) => cid,
            None => return false,
        };

        let core_pos = match self.node(core) {
            Some(n) => n.pos,
            None => return false,
        };

        if self.node(core).is_some_and(|n| n.pinned) {
            return false;
        }

        let members = match self.cluster(cid) {
            Some(c) => c.members.clone(),
            None => return false,
        };

        if members
            .iter()
            .any(|&m| self.node(m).is_some_and(|n| n.pinned))
        {
            return false;
        }

        let delta = Vec2 {
            x: to.x - core_pos.x,
            y: to.y - core_pos.y,
        };

        if let Some(n) = self.node_mut(core) {
            n.pos.x += delta.x;
            n.pos.y += delta.y;
        } else {
            return false;
        }

        for m in members {
            if let Some(n) = self.node_mut(m) {
                n.pos.x += delta.x;
                n.pos.y += delta.y;
            } else {
                return false;
            }
        }

        true
    }

    /// Collapse the cluster into a Core node.
    pub fn collapse_cluster(&mut self, id: ClusterId) -> Option<NodeId> {
        let (members, already_collapsed, existing_core) = {
            let c = self.clusters.get(&id)?;
            (c.members.clone(), c.is_collapsed(), c.core)
        };

        if already_collapsed {
            return existing_core;
        }

        for m in &members {
            self.set_state(*m, NodeState::Node);
            if let Some(n) = self.node_mut(*m) {
                n.visibility.set(Visibility::HIDDEN_BY_CLUSTER, true);
            }
        }

        let mut sum = Vec2 { x: 0.0, y: 0.0 };
        for m in &members {
            let n = self.node(*m)?;
            sum.x += n.pos.x;
            sum.y += n.pos.y;
        }
        let k = members.len() as f32;
        let core_pos = Vec2 {
            x: sum.x / k,
            y: sum.y / k,
        };

        let core_id = match existing_core {
            Some(cid) => cid,
            None => {
                let cid = NodeId::new(self.next_node);
                self.next_node += 1;

                let core = Node {
                    id: cid,
                    kind: NodeKind::Core,
                    state: NodeState::Core,
                    label: format!("Core {}", id.as_u64()),
                    pos: core_pos,
                    intrinsic_size: Vec2 { x: 48.0, y: 48.0 },
                    footprint: Vec2 { x: 48.0, y: 48.0 },
                    resize_footprint: None,
                    pinned: false,
                    anchor: false,
                    visibility: Visibility::NONE,
                    last_touch_ms: 0,
                    decay: DecayLevel::Hot,
                };
                self.nodes.insert(cid, core);
                cid
            }
        };

        if let Some(n) = self.node_mut(core_id) {
            n.pos = core_pos;
            n.kind = NodeKind::Core;
            n.state = NodeState::Core;
            n.footprint = Vec2 { x: 48.0, y: 48.0 };
            n.intrinsic_size = Vec2 { x: 48.0, y: 48.0 };

            n.visibility.clear(Visibility::HIDDEN_BY_CLUSTER);
            n.visibility.clear(Visibility::DETACHED);
        }

        let c = self.clusters.get_mut(&id)?;
        c.set_collapsed(true);
        c.core = Some(core_id);

        Some(core_id)
    }

    /// Expand the cluster.
    pub fn expand_cluster(&mut self, id: ClusterId) -> bool {
        let members = {
            let c = match self.clusters.get(&id) {
                Some(c) => c,
                None => return false,
            };
            if !c.is_collapsed() {
                return true;
            }
            c.members.clone()
        };

        for m in members {
            self.set_state(m, NodeState::Active);
            if let Some(n) = self.node_mut(m) {
                n.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
            }
        }

        if let Some(c) = self.clusters.get_mut(&id) {
            c.set_collapsed(false);
        }
        true
    }

    pub fn insert_existing(&mut self, node: Node) {
        // keep ids stable; bump next_node if needed so future spawns don’t collide
        self.next_node = self.next_node.max(node.id.as_u64() + 1);
        self.nodes.insert(node.id, node);
    }

    pub fn clusters_iter(&self) -> impl Iterator<Item = &Cluster> {
        self.clusters.values()
    }
}

impl Default for Field {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_create_rejects_missing_nodes() {
        let mut f = Field::new();
        let missing = NodeId::new(999);
        assert!(f.create_cluster(vec![missing]).is_none());
    }

    #[test]
    fn collapse_cluster_creates_core_and_shrinks_members() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let b = f.spawn_surface("B", Vec2 { x: 10.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });

        let cid = f.create_cluster(vec![a, b]).unwrap();
        let core = f.collapse_cluster(cid).unwrap();

        assert_eq!(f.node(a).unwrap().state, NodeState::Node);
        assert_eq!(f.node(b).unwrap().state, NodeState::Node);
        assert_eq!(f.node(a).unwrap().footprint, Vec2 { x: 24.0, y: 24.0 });

        assert!(
            f.node(a)
                .unwrap()
                .visibility
                .has(Visibility::HIDDEN_BY_CLUSTER)
        );
        assert!(
            f.node(b)
                .unwrap()
                .visibility
                .has(Visibility::HIDDEN_BY_CLUSTER)
        );
        assert!(!f.is_visible(a));
        assert!(!f.is_visible(b));

        let cn = f.node(core).unwrap();
        assert_eq!(cn.kind, NodeKind::Core);
        assert_eq!(cn.state, NodeState::Core);
        assert_eq!(cn.footprint, Vec2 { x: 48.0, y: 48.0 });
        assert!(f.is_visible(core));

        let c = f.cluster(cid).unwrap();
        assert!(c.is_collapsed());
        assert_eq!(c.core, Some(core));
    }

    #[test]
    fn expand_cluster_restores_members_active_and_visible() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let b = f.spawn_surface("B", Vec2 { x: 10.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });

        let cid = f.create_cluster(vec![a, b]).unwrap();
        f.collapse_cluster(cid).unwrap();

        assert!(f.expand_cluster(cid));

        assert_eq!(f.node(a).unwrap().state, NodeState::Active);
        assert_eq!(f.node(b).unwrap().state, NodeState::Active);
        assert_eq!(f.node(a).unwrap().footprint, Vec2 { x: 100.0, y: 50.0 });

        assert!(
            !f.node(a)
                .unwrap()
                .visibility
                .has(Visibility::HIDDEN_BY_CLUSTER)
        );
        assert!(f.is_visible(a));

        let c = f.cluster(cid).unwrap();
        assert!(!c.is_collapsed());
    }

    #[test]
    fn carry_respects_pinned() {
        let mut f = Field::new();
        let id = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        assert!(f.carry(id, Vec2 { x: 5.0, y: 5.0 }));
        assert_eq!(f.node(id).unwrap().pos, Vec2 { x: 5.0, y: 5.0 });

        assert!(f.set_pinned(id, true));
        assert!(!f.carry(id, Vec2 { x: 9.0, y: 9.0 }));
        assert_eq!(f.node(id).unwrap().pos, Vec2 { x: 5.0, y: 5.0 });
    }

    #[test]
    fn in_view_finds_intersections() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let _b = f.spawn_surface("B", Vec2 { x: 100.0, y: 100.0 }, Vec2 { x: 10.0, y: 10.0 });

        let view = Rect {
            min: Vec2 { x: -20.0, y: -20.0 },
            max: Vec2 { x: 20.0, y: 20.0 },
        };

        let ids = f.in_view_all(view);
        assert_eq!(ids, vec![a]);
    }

    #[test]
    fn in_view_skips_hidden_nodes() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        assert!(f.set_hidden(a, true));

        let view = Rect {
            min: Vec2 { x: -20.0, y: -20.0 },
            max: Vec2 { x: 20.0, y: 20.0 },
        };

        let ids = f.in_view(view);
        assert!(ids.is_empty());
        assert!(!f.is_visible(a));
    }

    #[test]
    fn set_state_changes_footprint() {
        let mut f = Field::new();
        let id = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });

        assert_eq!(f.node(id).unwrap().footprint, Vec2 { x: 100.0, y: 50.0 });

        assert!(f.set_state(id, NodeState::Node));
        assert_eq!(f.node(id).unwrap().footprint, Vec2 { x: 24.0, y: 24.0 });

        assert!(f.set_state(id, NodeState::Active));
        assert_eq!(f.node(id).unwrap().footprint, Vec2 { x: 100.0, y: 50.0 });
    }

    #[test]
    fn touch_sets_last_touch_and_wakes_node() {
        let mut f = Field::new();
        let id = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        assert!(f.set_decay_level(id, DecayLevel::Cold));
        assert_eq!(f.node(id).unwrap().state, NodeState::Node);

        assert!(f.touch(id, 1234));
        let n = f.node(id).unwrap();
        assert_eq!(n.last_touch_ms, 1234);
        assert_eq!(n.decay, DecayLevel::Hot);
        assert_eq!(n.state, NodeState::Active);
    }

    #[test]
    fn set_decay_level_maps_to_representation_state() {
        let mut f = Field::new();
        let id = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        assert!(f.set_decay_level(id, DecayLevel::Hot));
        assert_eq!(f.node(id).unwrap().decay, DecayLevel::Hot);
        assert_eq!(f.node(id).unwrap().state, NodeState::Active);

        assert!(f.set_decay_level(id, DecayLevel::Cold));
        assert_eq!(f.node(id).unwrap().decay, DecayLevel::Cold);
        assert_eq!(f.node(id).unwrap().state, NodeState::Node);
    }

    #[test]
    fn core_ignores_set_decay_level() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let b = f.spawn_surface("B", Vec2 { x: 10.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let cid = f.create_cluster(vec![a, b]).unwrap();
        let core = f.collapse_cluster(cid).unwrap();

        assert!(f.set_decay_level(core, DecayLevel::Cold));
        let n = f.node(core).unwrap();
        assert_eq!(n.kind, NodeKind::Core);
        assert_eq!(n.state, NodeState::Core);
    }

    #[test]
    fn carry_cluster_by_core_moves_core_and_members() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let b = f.spawn_surface("B", Vec2 { x: 10.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let cid = f.create_cluster(vec![a, b]).unwrap();
        let core = f.collapse_cluster(cid).unwrap();

        let core_before = f.node(core).unwrap().pos;
        let a_before = f.node(a).unwrap().pos;
        let b_before = f.node(b).unwrap().pos;

        assert!(f.carry_cluster_by_core(core, Vec2 { x: 100.0, y: 50.0 }));

        let core_after = f.node(core).unwrap().pos;
        let a_after = f.node(a).unwrap().pos;
        let b_after = f.node(b).unwrap().pos;

        let dx = core_after.x - core_before.x;
        let dy = core_after.y - core_before.y;

        assert_eq!(core_after, Vec2 { x: 100.0, y: 50.0 });
        assert_eq!(
            a_after,
            Vec2 {
                x: a_before.x + dx,
                y: a_before.y + dy
            }
        );
        assert_eq!(
            b_after,
            Vec2 {
                x: b_before.x + dx,
                y: b_before.y + dy
            }
        );
    }

    #[test]
    fn carry_cluster_by_core_respects_pinned() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let b = f.spawn_surface("B", Vec2 { x: 10.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let cid = f.create_cluster(vec![a, b]).unwrap();
        let core = f.collapse_cluster(cid).unwrap();

        assert!(f.set_pinned(a, true));

        let core_pos = f.node(core).unwrap().pos;
        let a_pos = f.node(a).unwrap().pos;
        let b_pos = f.node(b).unwrap().pos;

        assert!(!f.carry_cluster_by_core(core, Vec2 { x: 999.0, y: 999.0 }));

        assert_eq!(f.node(core).unwrap().pos, core_pos);
        assert_eq!(f.node(a).unwrap().pos, a_pos);
        assert_eq!(f.node(b).unwrap().pos, b_pos);
    }

    #[test]
    fn visuals_skip_hidden_nodes() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let b = f.spawn_surface("B", Vec2 { x: 50.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        assert!(f.set_hidden(b, true));

        let vis = f.visuals_visible();
        assert_eq!(vis.len(), 1);
        assert_eq!(vis[0].id, a);
    }
}
