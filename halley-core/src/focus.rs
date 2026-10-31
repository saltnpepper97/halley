use std::collections::HashMap;

use crate::cluster::tiling::Rect;
use crate::field::{Field, NodeId, NodeState, Vec2};
use crate::world::PortalDir;

/// Current interaction target (separate from history).
/// Focus only applies to nodes that are present and experience-visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focus {
    focused: Option<NodeId>,
}

impl Focus {
    pub fn new() -> Self {
        Self { focused: None }
    }

    pub fn current(&self) -> Option<NodeId> {
        self.focused
    }

    pub fn is_focused(&self, id: NodeId) -> bool {
        self.focused == Some(id)
    }

    /// Set focus to `id` if it exists and is experience-visible.
    pub fn set(&mut self, field: &Field, id: NodeId) -> bool {
        if !field.is_visible(id) {
            return false;
        }
        self.focused = Some(id);
        true
    }

    pub fn clear(&mut self) {
        self.focused = None;
    }

    /// If the focused node is removed, clear focus.
    pub fn on_removed(&mut self, removed: NodeId) {
        if self.focused == Some(removed) {
            self.focused = None;
        }
    }

    /// If the focused node becomes hidden (collapse/hide/detach), clear focus.
    pub fn on_hidden(&mut self, field: &Field, id: NodeId) {
        if self.focused == Some(id) && !field.is_visible(id) {
            self.focused = None;
        }
    }
}

impl Default for Focus {
    fn default() -> Self {
        Self::new()
    }
}

/// True if `id` is eligible to receive focus via cycling (alt-tab-style):
/// visible and in a representation state that's actually a real window
/// (`Active` or the decayed `Node` dot) rather than a collapsed-cluster
/// core handle. Ported from `cycle::is_focus_cycle_candidate` - the old
/// version also checked `kind == NodeKind::Surface`, which is now
/// redundant: with `NodeKind` gone, `matches!(state, Active | Node)`
/// already excludes `Core` on its own.
pub fn is_focus_cycle_candidate(field: &Field, id: NodeId) -> bool {
    field.node(id).is_some_and(|node| {
        field.is_visible(id) && matches!(node.state, NodeState::Active | NodeState::Node)
    })
}

/// Build an ordered list of focus-cycle candidates: most-recently-focused
/// first (ties broken by id, descending, for determinism), with `origin`
/// (if given and still a candidate) moved to the front so cycling always
/// starts from where the user currently is.
///
/// Ported from `cycle::build_candidates`. `last_focus_ms` is passed in
/// explicitly rather than read off `Field` because it isn't a `Field`/
/// `Node` concept - it's `wl`'s own `FocusState::last_surface_focus_ms`
/// timestamp map (distinct from `Node::last_touch_ms`, which tracks decay
/// aging, not focus history - don't conflate the two).
pub fn focus_cycle_candidates(
    field: &Field,
    last_focus_ms: &HashMap<NodeId, u64>,
    origin: Option<NodeId>,
) -> Vec<NodeId> {
    let mut candidates: Vec<NodeId> = field
        .node_ids_all()
        .into_iter()
        .filter(|&id| is_focus_cycle_candidate(field, id))
        .collect();

    candidates.sort_by(|a, b| {
        let a_at = last_focus_ms.get(a).copied().unwrap_or(0);
        let b_at = last_focus_ms.get(b).copied().unwrap_or(0);
        b_at.cmp(&a_at).then_with(|| b.as_u64().cmp(&a.as_u64()))
    });

    if let Some(origin) = origin
        && let Some(index) = candidates.iter().position(|&id| id == origin)
    {
        let o = candidates.remove(index);
        candidates.insert(0, o);
    }

    candidates
}

/// World-space (field) rectangle for a visible directional-focus target.
/// Uses the node's current representation footprint so expanded windows,
/// collapsed nodes, and cluster cores all participate with their visible size.
pub fn node_field_rect(field: &Field, id: NodeId) -> Option<Rect> {
    let node = field.node(id)?;
    if !field.is_visible(id) {
        return None;
    }
    let size = node.footprint;
    let w = size.x.max(1.0);
    let h = size.y.max(1.0);
    Some(Rect {
        x: node.pos.x - w * 0.5,
        y: node.pos.y - h * 0.5,
        w,
        h,
    })
}

fn rect_center(rect: Rect) -> Vec2 {
    Vec2 {
        x: rect.x + rect.w * 0.5,
        y: rect.y + rect.h * 0.5,
    }
}

fn range_gap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    if a1 < b0 {
        b0 - a1
    } else if b1 < a0 {
        a0 - b1
    } else {
        0.0
    }
}

/// Directional-focus neighbor scoring: lower `(orth_gap, main_delta,
/// dist2, tie_axis)` (compared lexicographically) is a better match.
/// Returns `None` if `candidate` isn't actually positioned in `direction`
/// from `current` (`main_delta <= 0.5`).
///
/// Ported from `clusters::system::directional_candidate_score` (shared
/// there by both free-field and tiled-cluster directional focus), swapping
/// `halley_config::DirectionalAction` - a `wl`/config-crate type this crate
/// can't depend on - for `world::PortalDir`, which already models the same
/// four directions (`N`=up, `S`=down, `E`=right, `W`=left).
pub fn directional_candidate_score(
    current: Rect,
    candidate: Rect,
    direction: PortalDir,
) -> Option<(f32, f32, f32, f32)> {
    let current_center = rect_center(current);
    let candidate_center = rect_center(candidate);
    let (main_delta, orth_gap, tie_axis) = match direction {
        PortalDir::W => (
            current_center.x - candidate_center.x,
            range_gap(
                current.y,
                current.y + current.h,
                candidate.y,
                candidate.y + candidate.h,
            ),
            candidate_center.y,
        ),
        PortalDir::E => (
            candidate_center.x - current_center.x,
            range_gap(
                current.y,
                current.y + current.h,
                candidate.y,
                candidate.y + candidate.h,
            ),
            candidate_center.y,
        ),
        PortalDir::N => (
            current_center.y - candidate_center.y,
            range_gap(
                current.x,
                current.x + current.w,
                candidate.x,
                candidate.x + candidate.w,
            ),
            candidate_center.x,
        ),
        PortalDir::S => (
            candidate_center.y - current_center.y,
            range_gap(
                current.x,
                current.x + current.w,
                candidate.x,
                candidate.x + candidate.w,
            ),
            candidate_center.x,
        ),
    };
    if main_delta <= 0.5 {
        return None;
    }
    let dx = candidate_center.x - current_center.x;
    let dy = candidate_center.y - current_center.y;
    Some((orth_gap, main_delta, dx * dx + dy * dy, tie_axis))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Vec2;

    #[test]
    fn focus_accepts_only_visible_existing_nodes() {
        let mut field = Field::new();
        let id = field.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let mut focus = Focus::new();
        assert_eq!(focus.current(), None);
        assert!(!focus.set(&field, NodeId::new(999)));
        assert_eq!(focus.current(), None);
        assert!(field.set_hidden(id, true));
        assert!(!focus.set(&field, id));
        assert_eq!(focus.current(), None);
        assert!(field.set_hidden(id, false));
        assert!(focus.set(&field, id));
        assert_eq!(focus.current(), Some(id));
        assert!(focus.is_focused(id));
    }

    #[test]
    fn focus_clears_when_node_is_removed_or_hidden() {
        for hidden in [false, true] {
            let mut field = Field::new();
            let id = field.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
            let mut focus = Focus::new();
            assert!(focus.set(&field, id));
            if hidden {
                assert!(field.set_hidden(id, true));
                focus.on_hidden(&field, id);
            } else {
                field.remove(id);
                focus.on_removed(id);
            }
            assert_eq!(focus.current(), None, "hidden={hidden}");
        }
    }

    #[test]
    fn cycle_candidate_excludes_hidden_and_core_state() {
        let mut field = Field::new();
        let active = field.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let hidden = field.spawn_surface("B", Vec2 { x: 10.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let core = field.spawn_surface("C", Vec2 { x: 20.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        field.set_hidden(hidden, true);
        field.set_state(core, NodeState::Core);

        assert!(is_focus_cycle_candidate(&field, active));
        assert!(!is_focus_cycle_candidate(&field, hidden));
        assert!(!is_focus_cycle_candidate(&field, core));
    }

    #[test]
    fn cycle_candidates_order_by_recency_then_move_origin_to_front() {
        let mut field = Field::new();
        let a = field.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let b = field.spawn_surface("B", Vec2 { x: 10.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let c = field.spawn_surface("C", Vec2 { x: 20.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let mut last_focus_ms = HashMap::new();
        last_focus_ms.insert(a, 100);
        last_focus_ms.insert(b, 300);
        last_focus_ms.insert(c, 200);

        // No origin: strictly most-recent-first.
        let candidates = focus_cycle_candidates(&field, &last_focus_ms, None);
        assert_eq!(candidates, vec![b, c, a]);

        // With an origin, it moves to the front even though it isn't
        // the most recent.
        let candidates = focus_cycle_candidates(&field, &last_focus_ms, Some(a));
        assert_eq!(candidates, vec![a, b, c]);
    }

    #[test]
    fn cycle_candidates_defaults_missing_timestamps_to_zero() {
        let mut field = Field::new();
        let known = field.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let unknown = field.spawn_surface("B", Vec2 { x: 10.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let mut last_focus_ms = HashMap::new();
        last_focus_ms.insert(known, 50);

        let candidates = focus_cycle_candidates(&field, &last_focus_ms, None);
        // known has a real timestamp and sorts before unknown (defaults to 0).
        assert_eq!(candidates, vec![known, unknown]);
    }

    #[test]
    fn node_field_rect_uses_visible_window_node_and_core_footprints() {
        let mut field = Field::new();
        let active = field.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let dot = field.spawn_surface("B", Vec2 { x: 100.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        field.set_state(dot, NodeState::Node);
        let core = field.spawn_surface("C", Vec2 { x: 200.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        field.set_state(core, NodeState::Core);
        let hidden =
            field.spawn_surface("D", Vec2 { x: 300.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        field.set_hidden(hidden, true);

        let rect = node_field_rect(&field, active).unwrap();
        assert_eq!(rect.x, -50.0);
        assert_eq!(rect.y, -25.0);
        assert_eq!(rect.w, 100.0);
        assert_eq!(rect.h, 50.0);

        let dot_rect = node_field_rect(&field, dot).unwrap();
        assert!(dot_rect.w < rect.w);
        assert!(dot_rect.h < rect.h);
        assert_eq!(node_field_rect(&field, core).unwrap().w, 48.0);
        assert!(node_field_rect(&field, hidden).is_none());
    }

    fn active_rect(field: &mut Field, x: f32, y: f32) -> Rect {
        let id = field.spawn_surface("N", Vec2 { x, y }, Vec2 { x: 100.0, y: 100.0 });
        node_field_rect(field, id).unwrap()
    }

    #[test]
    fn directional_score_picks_nearest_neighbor_per_direction() {
        // Mirrors halley-wl's directional_focus_picks_nearest_neighbor_per_direction.
        let mut field = Field::new();
        let center = active_rect(&mut field, 400.0, 300.0);
        let right = active_rect(&mut field, 700.0, 300.0);
        let up = active_rect(&mut field, 400.0, 60.0);
        let overlap = active_rect(&mut field, 460.0, 300.0);

        let best = |current: Rect, candidates: &[Rect], dir: PortalDir| -> Option<Rect> {
            candidates
                .iter()
                .copied()
                .filter_map(|c| directional_candidate_score(current, c, dir).map(|s| (s, c)))
                .min_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap())
                .map(|(_, c)| c)
        };

        assert_eq!(best(center, &[right, up, overlap], PortalDir::N), Some(up));
        // The closely-stacked peer should win over the far-right window.
        assert_eq!(
            best(center, &[right, up, overlap], PortalDir::E),
            Some(overlap)
        );
        assert_eq!(
            best(overlap, &[right, up, center], PortalDir::E),
            Some(right)
        );
        // Nothing to the west of center -> no candidate scores at all.
        assert_eq!(best(center, &[right, up, overlap], PortalDir::W), None);
    }
}
