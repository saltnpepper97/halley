use std::collections::HashMap;

use crate::field::{Field, NodeId, Vec2};
use crate::viewport::Viewport;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SpaceId(u64);

impl SpaceId {
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PortalDir {
    N,
    E,
    S,
    W,
}

impl PortalDir {
    pub fn opposite(self) -> Self {
        match self {
            PortalDir::N => PortalDir::S,
            PortalDir::E => PortalDir::W,
            PortalDir::S => PortalDir::N,
            PortalDir::W => PortalDir::E,
        }
    }
}

/// Multiple independent infinite Fields (one per monitor space),
/// plus optional adjacency (portals) between them.
pub struct World {
    spaces: HashMap<SpaceId, Field>,
    neighbors: HashMap<(SpaceId, PortalDir), SpaceId>,
}

impl World {
    pub fn new() -> Self {
        Self {
            spaces: HashMap::new(),
            neighbors: HashMap::new(),
        }
    }

    pub fn add_space(&mut self, id: SpaceId, field: Field) {
        self.spaces.insert(id, field);
    }

    pub fn space(&self, id: SpaceId) -> Option<&Field> {
        self.spaces.get(&id)
    }

    pub fn space_mut(&mut self, id: SpaceId) -> Option<&mut Field> {
        self.spaces.get_mut(&id)
    }

    /// Define a portal edge from `a` in direction `dir` to `b`.
    /// You typically set both directions (a->b and b->a).
    pub fn set_neighbor(&mut self, a: SpaceId, dir: PortalDir, b: SpaceId) {
        self.neighbors.insert((a, dir), b);
    }

    pub fn neighbor(&self, a: SpaceId, dir: PortalDir) -> Option<SpaceId> {
        self.neighbors.get(&(a, dir)).copied()
    }

    /// Move a single node from one space to its neighbor space through `dir`.
    /// The compositor should call this ONLY when the "transfer modifier" is held.
    pub fn transfer_node(
        &mut self,
        from_space: SpaceId,
        node: NodeId,
        dir: PortalDir,
        from_vp: &Viewport,
        to_vp: &Viewport,
    ) -> bool {
        let to_space = match self.neighbor(from_space, dir) {
            Some(s) => s,
            None => return false,
        };

        let (pos, node_data) = {
            let from = match self.space_mut(from_space) {
                Some(f) => f,
                None => return false,
            };

            let n = match from.node(node) {
                Some(n) => n,
                None => return false,
            };

            // Movement constraint: pinned nodes can't be transferred.
            if n.pinned {
                return false;
            }

            // compute new position first (pure)
            let new_pos = map_across_portal(from_vp, to_vp, dir, n.pos);

            // remove node payload
            let removed = match from.remove(node) {
                Some(x) => x,
                None => return false,
            };

            (new_pos, removed)
        };

        // insert into target field, preserving NodeId
        let to = match self.space_mut(to_space) {
            Some(f) => f,
            None => return false,
        };

        let mut insert = node_data;
        insert.pos = pos;
        to.insert_existing(insert);

        true
    }

    /// Move a cluster by its core handle across spaces.
    /// This moves the core + members as a unit by rehoming all involved nodes.
    pub fn transfer_cluster_by_core(
        &mut self,
        from_space: SpaceId,
        core: NodeId,
        dir: PortalDir,
        from_vp: &Viewport,
        to_vp: &Viewport,
    ) -> bool {
        let to_space = match self.neighbor(from_space, dir) {
            Some(s) => s,
            None => return false,
        };

        // -------- PRE-FLIGHT (NO MUTATION) --------
        let (cid, members, core_pos) = {
            let from = match self.space(from_space) {
                Some(f) => f,
                None => return false,
            };

            let cid = match from.cluster_id_for_core_public(core) {
                Some(cid) => cid,
                None => return false,
            };

            let cluster = match from.cluster(cid) {
                Some(c) => c,
                None => return false,
            };

            let core_node = match from.node(core) {
                Some(n) => n,
                None => return false,
            };

            // Movement constraint checks: pinned nodes block transfer.
            if core_node.pinned {
                return false;
            }

            for m in cluster.members() {
                match from.node(*m) {
                    Some(n) if !n.pinned => {}
                    _ => return false,
                }
            }

            (cid, cluster.members().to_vec(), core_node.pos)
        };

        // Compute mapping delta
        let mapped = map_across_portal(from_vp, to_vp, dir, core_pos);
        let delta = Vec2 {
            x: mapped.x - core_pos.x,
            y: mapped.y - core_pos.y,
        };

        // -------- REMOVE FROM SOURCE (atomic intent) --------
        let (cluster_obj, core_payload, member_payloads) = {
            let from = match self.space_mut(from_space) {
                Some(f) => f,
                None => return false,
            };

            let cluster_obj = match from.remove_cluster(cid) {
                Some(c) => c,
                None => return false,
            };

            let core_node = match from.remove(core) {
                Some(n) => n,
                None => {
                    from.insert_cluster(cluster_obj);
                    return false;
                }
            };

            let mut members_out = Vec::with_capacity(members.len());
            for m in &members {
                match from.remove(*m) {
                    Some(n) => members_out.push(n),
                    None => {
                        // rollback
                        from.insert_existing(core_node);
                        from.insert_cluster(cluster_obj);
                        return false;
                    }
                }
            }

            (cluster_obj, core_node, members_out)
        };

        // -------- INSERT INTO TARGET --------
        let to = match self.space_mut(to_space) {
            Some(f) => f,
            None => return false,
        };

        // Insert cluster record first
        to.insert_cluster(cluster_obj);

        // Insert core
        let mut core_insert = core_payload;
        core_insert.pos = mapped;
        to.insert_existing(core_insert);

        // Insert members
        for mut mp in member_payloads {
            mp.pos.x += delta.x;
            mp.pos.y += delta.y;
            to.insert_existing(mp);
        }

        true
    }
}

/// Edge-preserving mapping between monitor spaces.
///
/// Rule:
/// - If crossing E: new x = left edge + epsilon, preserve y offset from viewport center.
/// - If crossing W: new x = right edge - epsilon, preserve y offset.
/// - If crossing N: new y = bottom edge - epsilon, preserve x offset.
/// - If crossing S: new y = top edge + epsilon, preserve x offset.
pub fn map_across_portal(from_vp: &Viewport, to_vp: &Viewport, dir: PortalDir, pos: Vec2) -> Vec2 {
    let to = to_vp.rect();
    let eps = 1.0;

    let rel = Vec2 {
        x: pos.x - from_vp.center.x,
        y: pos.y - from_vp.center.y,
    };

    match dir {
        PortalDir::E => Vec2 {
            x: to.min.x + eps,
            y: to_vp.center.y + rel.y,
        },
        PortalDir::W => Vec2 {
            x: to.max.x - eps,
            y: to_vp.center.y + rel.y,
        },
        PortalDir::N => Vec2 {
            x: to_vp.center.x + rel.x,
            y: to.max.y - eps,
        },
        PortalDir::S => Vec2 {
            x: to_vp.center.x + rel.x,
            y: to.min.y + eps,
        },
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Vec2;

    #[test]
    fn map_preserves_tangent_offset_east() {
        let from_vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });
        let to_vp = Viewport::new(
            Vec2 {
                x: 1000.0,
                y: 500.0,
            },
            Vec2 { x: 100.0, y: 100.0 },
        );

        let pos = Vec2 { x: 49.0, y: 10.0 }; // near east edge of from
        let mapped = map_across_portal(&from_vp, &to_vp, PortalDir::E, pos);

        // x placed near left edge of to
        assert!(mapped.x > to_vp.rect().min.x);
        // y keeps rel offset from center
        assert_eq!(mapped.y, to_vp.center.y + (pos.y - from_vp.center.y));
    }

    #[test]
    fn transfer_node_moves_between_spaces() {
        let mut w = World::new();
        let mut fa = Field::new();
        let fb = Field::new();

        let a = SpaceId::new(1);
        let b = SpaceId::new(2);

        let n = fa.spawn_surface("A", Vec2 { x: 10.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        w.add_space(a, fa);
        w.add_space(b, fb);

        w.set_neighbor(a, PortalDir::E, b);
        w.set_neighbor(b, PortalDir::W, a);

        let from_vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });
        let to_vp = Viewport::new(Vec2 { x: 1000.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });

        assert!(w.transfer_node(a, n, PortalDir::E, &from_vp, &to_vp));

        assert!(w.space(a).unwrap().node(n).is_none());
        assert!(w.space(b).unwrap().node(n).is_some());
    }

    #[test]
    fn transfer_cluster_moves_cluster_record() {
        let mut w = World::new();

        let mut fa = Field::new();
        let fb = Field::new();

        let a = SpaceId::new(1);
        let b = SpaceId::new(2);

        let n1 = fa.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let n2 = fa.spawn_surface("B", Vec2 { x: 10.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let cid = fa.create_cluster(vec![n1, n2]).unwrap();
        let core = fa.collapse_cluster(cid).unwrap();

        w.add_space(a, fa);
        w.add_space(b, fb);

        w.set_neighbor(a, PortalDir::E, b);
        w.set_neighbor(b, PortalDir::W, a);

        let from_vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });
        let to_vp = Viewport::new(Vec2 { x: 1000.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });

        assert!(w.transfer_cluster_by_core(a, core, PortalDir::E, &from_vp, &to_vp));

        // Cluster should no longer exist in space A
        assert!(w.space(a).unwrap().cluster(cid).is_none());

        // Cluster should now exist in space B
        assert!(w.space(b).unwrap().cluster(cid).is_some());

        // Core lookup should work
        let dest = w.space(b).unwrap();
        assert_eq!(dest.cluster_id_for_core_public(core), Some(cid));
    }
}
