//! Field directional focus — move keyboard focus to the nearest window in a
//! direction on the free field (the vim-style `hjkl` navigation).
//!
//! This is the free-field complement to the tiling-cluster directional focus
//! (`clusters::system::directional_target_member`): it reuses the same neighbor
//! scoring (`directional_candidate_score`) but operates over the loose,
//! free-positioned surface nodes on the current monitor rather than a tiled layout.
//! Cross-monitor movement is handled separately by the monitor-focus binding.

use std::time::Instant;

use halley_config::DirectionalAction;
use halley_core::field::{NodeId, NodeKind, NodeState};
use halley_core::tiling::Rect;

use crate::compositor::actions::window::focus_or_reveal_surface_node;
use crate::compositor::clusters::system::directional_candidate_score;
use crate::compositor::root::Halley;

/// World-space (field) rectangle for a surface node, derived from its centre `pos`
/// and `intrinsic_size` — the same convention as `focus::state::active_surface_world_rect`.
fn node_field_rect(st: &Halley, node_id: NodeId) -> Option<Rect> {
    let node = st.model.field.node(node_id)?;
    if node.kind != NodeKind::Surface
        || node.state != NodeState::Active
        || !st.model.field.is_visible(node_id)
    {
        return None;
    }
    let size = node.intrinsic_size;
    let w = size.x.max(1.0);
    let h = size.y.max(1.0);
    Some(Rect {
        x: node.pos.x - w * 0.5,
        y: node.pos.y - h * 0.5,
        w,
        h,
    })
}

/// Move focus to the nearest visible window in `direction` from the currently
/// focused window, restricted to the focused window's monitor. Returns true when
/// focus changed.
pub(crate) fn focus_directional(
    st: &mut Halley,
    direction: DirectionalAction,
    now: Instant,
) -> bool {
    let monitor = st.focused_monitor().to_string();
    let Some(current) = st
        .model
        .focus_state
        .primary_interaction_focus
        .or_else(|| st.last_focused_surface_node_for_monitor(monitor.as_str()))
    else {
        return false;
    };
    let Some(current_rect) = node_field_rect(st, current) else {
        return false;
    };
    // Candidates share the current window's monitor so directional focus walks the
    // local field; jumping to another monitor is the monitor-focus binding's job.
    let current_monitor = st
        .model
        .monitor_state
        .node_monitor
        .get(&current)
        .cloned()
        .unwrap_or(monitor);

    let target = st
        .model
        .field
        .nodes()
        .iter()
        .filter(|&(&id, _)| id != current)
        .filter(|&(&id, _)| {
            st.model
                .monitor_state
                .node_monitor
                .get(&id)
                .is_some_and(|m| *m == current_monitor)
        })
        .filter_map(|(&id, _)| {
            let rect = node_field_rect(st, id)?;
            directional_candidate_score(current_rect, rect, direction).map(|score| (score, id))
        })
        .min_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, id)| id);

    match target {
        Some(id) => focus_or_reveal_surface_node(st, id, now),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::focus_directional;
    use crate::compositor::root::Halley;
    use halley_config::DirectionalAction;
    use halley_core::field::{NodeId, NodeState, Vec2};
    use smithay::reexports::wayland_server::Display;
    use std::time::Instant;

    fn single_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        }];
        tuning
    }

    fn spawn_active(st: &mut Halley, name: &str, x: f32, y: f32) -> NodeId {
        let id = st
            .model
            .field
            .spawn_surface(name, Vec2 { x, y }, Vec2 { x: 100.0, y: 100.0 });
        st.assign_node_to_monitor(id, "monitor_a");
        let _ = st.model.field.set_state(id, NodeState::Active);
        id
    }

    #[test]
    fn directional_focus_picks_nearest_neighbor_per_direction() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        assert!(st.activate_monitor("monitor_a"));
        st.set_focused_monitor("monitor_a");

        let center = spawn_active(&mut st, "center", 400.0, 300.0);
        let right = spawn_active(&mut st, "right", 700.0, 300.0);
        let up = spawn_active(&mut st, "up", 400.0, 60.0);
        // A window stacked almost directly behind the centre one: the near-overlapping
        // peer should win a rightward move over the farther `right` window.
        let overlap = spawn_active(&mut st, "overlap", 460.0, 300.0);

        let now = Instant::now();

        st.model.focus_state.primary_interaction_focus = Some(center);
        assert!(focus_directional(&mut st, DirectionalAction::Up, now));
        assert_eq!(st.model.focus_state.primary_interaction_focus, Some(up));

        st.model.focus_state.primary_interaction_focus = Some(center);
        assert!(focus_directional(&mut st, DirectionalAction::Right, now));
        assert_eq!(
            st.model.focus_state.primary_interaction_focus,
            Some(overlap),
            "the closely stacked peer should win over the far-right window"
        );

        st.model.focus_state.primary_interaction_focus = Some(overlap);
        assert!(focus_directional(&mut st, DirectionalAction::Right, now));
        assert_eq!(st.model.focus_state.primary_interaction_focus, Some(right));

        // Nothing to the left of the centre window → no change.
        st.model.focus_state.primary_interaction_focus = Some(center);
        assert!(!focus_directional(&mut st, DirectionalAction::Left, now));
        assert_eq!(st.model.focus_state.primary_interaction_focus, Some(center));
    }
}
