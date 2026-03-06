use std::collections::{HashMap, HashSet};

use crate::field::NodeId;

/// Stable id for a trail entry (not a NodeId).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrailId(u64);

#[derive(Clone, Debug)]
struct Entry {
    node: NodeId,
    prev: Option<TrailId>,
    next: Option<TrailId>,
}

/// Focus history with browser-like back/forward behavior.
/// - `record(node)` appends and moves the cursor.
/// - If cursor isn't at tail, forward history is dropped.
/// - `forget_node(node)` removes all occurrences and preserves chain.
pub struct Trail {
    next_id: u64,

    head: Option<TrailId>,
    tail: Option<TrailId>,
    cursor: Option<TrailId>,

    entries: HashMap<TrailId, Entry>,

    // Allows O(k) removal of all occurrences of a node.
    by_node: HashMap<NodeId, HashSet<TrailId>>,
}

impl Trail {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            head: None,
            tail: None,
            cursor: None,
            entries: HashMap::new(),
            by_node: HashMap::new(),
        }
    }

    pub fn cursor(&self) -> Option<NodeId> {
        self.cursor
            .and_then(|id| self.entries.get(&id))
            .map(|e| e.node)
    }

    pub fn is_empty(&self) -> bool {
        self.head.is_none()
    }

    /// Record a focus visit.
    pub fn record(&mut self, node: NodeId) {
        // If we are not at the tail, drop everything after cursor (browser semantics).
        self.drop_forward();

        let id = TrailId(self.next_id);
        self.next_id += 1;

        let entry = Entry {
            node,
            prev: self.tail,
            next: None,
        };

        if let Some(tail_id) = self.tail {
            if let Some(tail) = self.entries.get_mut(&tail_id) {
                tail.next = Some(id);
            }
        } else {
            // first entry
            self.head = Some(id);
        }

        self.tail = Some(id);
        self.cursor = Some(id);

        self.entries.insert(id, entry);
        self.by_node.entry(node).or_default().insert(id);
    }

    /// Step back in history.
    pub fn back(&mut self) -> Option<NodeId> {
        let cur = self.cursor?;
        let prev = self.entries.get(&cur)?.prev?;
        self.cursor = Some(prev);
        self.cursor()
    }

    /// Step forward in history.
    pub fn forward(&mut self) -> Option<NodeId> {
        let cur = self.cursor?;
        let next = self.entries.get(&cur)?.next?;
        self.cursor = Some(next);
        self.cursor()
    }

    /// Remove all history entries for a given node id.
    pub fn forget_node(&mut self, node: NodeId) {
        let Some(ids) = self.by_node.remove(&node) else {
            return;
        };

        // We'll remove each entry id. Order doesn't matter.
        for id in ids {
            self.remove_entry(id);
        }
    }

    fn drop_forward(&mut self) {
        // If no cursor, nothing to drop.
        let Some(cur) = self.cursor else { return };

        // Find the first forward entry
        let next = match self.entries.get(&cur) {
            Some(e) => e.next,
            None => return,
        };

        // Detach forward chain from cursor
        if let Some(e) = self.entries.get_mut(&cur) {
            e.next = None;
        }
        self.tail = Some(cur);

        // Remove every node in the forward chain
        let mut it = next;
        while let Some(id) = it {
            let next_id = self.entries.get(&id).and_then(|e| e.next);
            self.remove_entry(id);
            it = next_id;
        }
    }

    fn remove_entry(&mut self, id: TrailId) {
        let Some(entry) = self.entries.remove(&id) else {
            return;
        };

        // unlink from by_node index
        if let Some(set) = self.by_node.get_mut(&entry.node) {
            set.remove(&id);
            if set.is_empty() {
                self.by_node.remove(&entry.node);
            }
        }

        // splice pointers
        match entry.prev {
            Some(p) => {
                if let Some(pe) = self.entries.get_mut(&p) {
                    pe.next = entry.next;
                }
            }
            None => {
                // removing head
                self.head = entry.next;
            }
        }

        match entry.next {
            Some(n) => {
                if let Some(ne) = self.entries.get_mut(&n) {
                    ne.prev = entry.prev;
                }
            }
            None => {
                // removing tail
                self.tail = entry.prev;
            }
        }

        // cursor policy: if cursor removed, go to prev else next else none
        if self.cursor == Some(id) {
            self.cursor = entry.prev.or(entry.next);
        }

        // if list now empty, clear cursor too
        if self.head.is_none() {
            self.cursor = None;
            self.tail = None;
        }
    }
}

impl Default for Trail {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::NodeId;

    #[test]
    fn record_sets_cursor_and_allows_back_forward() {
        let mut t = Trail::new();
        let a = NodeId::new(1);
        let b = NodeId::new(2);
        let c = NodeId::new(3);

        t.record(a);
        t.record(b);
        t.record(c);

        assert_eq!(t.cursor(), Some(c));
        assert_eq!(t.back(), Some(b));
        assert_eq!(t.back(), Some(a));
        assert_eq!(t.back(), None); // can't go past head
        assert_eq!(t.forward(), Some(b));
        assert_eq!(t.forward(), Some(c));
        assert_eq!(t.forward(), None); // can't go past tail
    }

    #[test]
    fn record_drops_forward_history() {
        let mut t = Trail::new();
        let a = NodeId::new(1);
        let b = NodeId::new(2);
        let c = NodeId::new(3);
        let d = NodeId::new(4);

        t.record(a);
        t.record(b);
        t.record(c);

        // go back to b
        assert_eq!(t.back(), Some(b));

        // record d; this should drop forward (c)
        t.record(d);

        assert_eq!(t.cursor(), Some(d));
        assert_eq!(t.forward(), None);
        assert_eq!(t.back(), Some(b));
    }

    #[test]
    fn forget_node_removes_all_occurrences_and_preserves_chain() {
        let mut t = Trail::new();
        let a = NodeId::new(1);
        let b = NodeId::new(2);
        let c = NodeId::new(3);

        // a -> b -> a -> c
        t.record(a);
        t.record(b);
        t.record(a);
        t.record(c);

        // remove 'a' entries
        t.forget_node(a);

        // history should now behave like: b -> c
        assert_eq!(t.cursor(), Some(c));
        assert_eq!(t.back(), Some(b));
        assert_eq!(t.back(), None);
        assert_eq!(t.forward(), Some(c));
    }

    #[test]
    fn forget_node_moves_cursor_safely() {
        let mut t = Trail::new();
        let a = NodeId::new(1);
        let b = NodeId::new(2);

        t.record(a);
        t.record(b);

        // cursor is b; forget b should move cursor to a
        t.forget_node(b);
        assert_eq!(t.cursor(), Some(a));

        // forget a should empty list
        t.forget_node(a);
        assert_eq!(t.cursor(), None);
        assert!(t.is_empty());
    }
}
