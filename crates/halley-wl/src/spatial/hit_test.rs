use std::cmp::{Ordering, Reverse};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::compositor::interaction::{HitNode, ResizeCtx};
use crate::compositor::root::Halley;
use crate::compositor::spawn::state::{is_persistent_rule_top, node_floats_over_active_cluster};
use crate::compositor::surface::active_stacking_visible_members_for_monitor;
use crate::frame_loop::anim_style_for;
use crate::input::active_node_screen_rect;
use crate::presentation::{node_marker_metrics, world_to_screen};
use halley_core::field::{Field, NodeId, Vec2};
use halley_core::viewport::{FocusRing, FocusZone};

struct ActiveAreaView<'a> {
    field: &'a Field,
    node_monitor: &'a HashMap<NodeId, String>,
    monitor: &'a str,
    focus_ring: FocusRing,
    focus_center: Vec2,
}

struct ActiveHitOrderingView {
    stack_ranks: HashMap<NodeId, usize>,
    overlap_ranks: HashMap<NodeId, (u64, u64)>,
    draws_above_fullscreen: HashSet<NodeId>,
    persistent_top: HashSet<NodeId>,
}

impl ActiveHitOrderingView {
    fn from_halley(
        st: &Halley,
        _now: Instant,
        stack_visible_front_to_back: &[NodeId],
        hits: &[HitNode],
    ) -> Self {
        let stack_ranks = stack_visible_front_to_back
            .iter()
            .enumerate()
            .map(|(index, &node_id)| (node_id, index))
            .collect();
        let mut overlap_ranks = HashMap::new();
        let mut draws_above_fullscreen = HashSet::new();
        let mut persistent_top = HashSet::new();
        for hit in hits {
            let node_id = hit.node_id;
            let (rank, tie) = st.overlap_policy_stack_rank(node_id);
            let rank = if node_floats_over_active_cluster(st, node_id) {
                rank.saturating_add(1_u64 << 62)
            } else {
                rank
            };
            overlap_ranks.insert(node_id, (rank, tie));
            if st.node_draws_above_fullscreen_on_current_monitor(node_id) {
                draws_above_fullscreen.insert(node_id);
            }
            if is_persistent_rule_top(st, node_id) {
                persistent_top.insert(node_id);
            }
        }
        Self {
            stack_ranks,
            overlap_ranks,
            draws_above_fullscreen,
            persistent_top,
        }
    }

    fn compare(&self, a: &HitNode, b: &HitNode) -> Ordering {
        let compare_bool = |lhs: bool, rhs: bool| rhs.cmp(&lhs);
        compare_bool(
            self.draws_above_fullscreen.contains(&a.node_id),
            self.draws_above_fullscreen.contains(&b.node_id),
        )
        .then_with(|| {
            compare_bool(
                self.persistent_top.contains(&a.node_id),
                self.persistent_top.contains(&b.node_id),
            )
        })
        .then_with(|| {
            match (
                self.stack_ranks.get(&a.node_id),
                self.stack_ranks.get(&b.node_id),
            ) {
                (Some(a_rank), Some(b_rank)) => a_rank.cmp(b_rank),
                _ => Ordering::Equal,
            }
        })
        .then_with(|| {
            let a_rank = self
                .overlap_ranks
                .get(&a.node_id)
                .copied()
                .unwrap_or((0, a.node_id.as_u64()));
            let b_rank = self
                .overlap_ranks
                .get(&b.node_id)
                .copied()
                .unwrap_or((0, b.node_id.as_u64()));
            b_rank.cmp(&a_rank)
        })
        .then_with(|| Reverse(a.node_id.as_u64()).cmp(&Reverse(b.node_id.as_u64())))
    }
}

impl ActiveAreaView<'_> {
    fn contains(&self, node_id: NodeId) -> bool {
        let Some(node) = self.field.node(node_id) else {
            return false;
        };
        if self
            .node_monitor
            .get(&node_id)
            .is_some_and(|owner| owner != self.monitor)
        {
            return false;
        }
        matches!(
            self.focus_ring.zone(self.focus_center, node.pos),
            FocusZone::Inside
        )
    }
}

pub(crate) fn hit_nodes_at(
    st: &Halley,
    w: i32,
    h: i32,
    sx: f32,
    sy: f32,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Vec<HitNode> {
    let mut active: Vec<HitNode> = Vec::new();
    let mut node_dot: Vec<HitNode> = Vec::new();
    let stack_visible_front_to_back = active_stacking_visible_members_for_monitor(
        st,
        st.model.monitor_state.current_monitor.as_str(),
    );
    for id in st.model.field.node_ids_all() {
        let Some(n) = st.model.field.node(id) else {
            continue;
        };
        if !st.model.field.is_visible(id) || !st.node_visible_on_current_monitor(id) {
            continue;
        }
        if !matches!(
            n.state,
            halley_core::field::NodeState::Active
                | halley_core::field::NodeState::Node
                | halley_core::field::NodeState::Core
        ) {
            continue;
        }
        let anim = anim_style_for(st, id, n.state.clone(), now);
        let hit = match n.state {
            halley_core::field::NodeState::Active => {
                if let Some((left, top, right, bottom)) =
                    active_node_screen_rect(st, w, h, id, now, resize_preview)
                {
                    let x = left.round() as i32;
                    let y = top.round() as i32;
                    let ww = (right - left).max(1.0).round() as i32;
                    let hh = (bottom - top).max(1.0).round() as i32;
                    if sx >= x as f32
                        && sx <= (x + ww) as f32
                        && sy >= y as f32
                        && sy <= (y + hh) as f32
                    {
                        Some(HitNode {
                            node_id: id,
                            move_surface: false,
                            is_core: false,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core => {
                let (cx, cy) = world_to_screen(st, w, h, n.pos.x, n.pos.y);
                let (dot_half, _, _, _) = node_marker_metrics(st, n.label.len(), anim.scale);
                let radius = if n.state == halley_core::field::NodeState::Core {
                    34.0
                } else {
                    (dot_half as f32 * 1.5).round().max(1.0)
                };
                let dx = sx - cx as f32;
                let dy = sy - cy as f32;
                if dx * dx + dy * dy <= radius * radius {
                    Some(HitNode {
                        node_id: id,
                        move_surface: false,
                        is_core: n.state == halley_core::field::NodeState::Core,
                    })
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some(hit) = hit else { continue };
        match n.state {
            halley_core::field::NodeState::Active => active.push(hit),
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core => {
                node_dot.push(hit)
            }
            _ => {}
        };
    }

    let active_ordering =
        ActiveHitOrderingView::from_halley(st, now, &stack_visible_front_to_back, &active);
    active.sort_by(|a, b| active_ordering.compare(a, b));
    node_dot.sort_by_key(|h| Reverse(h.node_id.as_u64()));

    active.extend(node_dot);
    active
}

pub(crate) fn pick_hit_node_at(
    st: &Halley,
    w: i32,
    h: i32,
    sx: f32,
    sy: f32,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Option<HitNode> {
    hit_nodes_at(st, w, h, sx, sy, now, resize_preview)
        .into_iter()
        .next()
}

pub(crate) fn node_in_active_area(st: &Halley, node_id: NodeId) -> bool {
    node_in_active_area_for_monitor(st, node_id, st.model.monitor_state.current_monitor.as_str())
}

pub(crate) fn node_in_active_area_for_monitor(st: &Halley, node_id: NodeId, monitor: &str) -> bool {
    ActiveAreaView {
        field: &st.model.field,
        node_monitor: &st.model.monitor_state.node_monitor,
        monitor,
        focus_ring: st.focus_ring_for_monitor(monitor),
        focus_center: st.view_center_for_monitor(monitor),
    }
    .contains(node_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::active_node_screen_rect;
    use smithay::reexports::wayland_server::Display;

    fn single_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
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

    fn single_monitor_stacking_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = single_monitor_tuning();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Stacking;
        tuning.stacking_max_visible = 3;
        tuning
    }

    fn active_surface_hit_with_tuning(
        tuning: halley_config::RuntimeTuning,
    ) -> crate::compositor::interaction::HitNode {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, tuning);
        let node_id = st.model.field.spawn_surface(
            "test",
            halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(node_id, "monitor_a");
        let _ = st
            .model
            .field
            .set_state(node_id, halley_core::field::NodeState::Active);

        let now = Instant::now();
        let (left, top, right, _) =
            active_node_screen_rect(&st, 800, 600, node_id, now, None).expect("active rect");
        let sx = (left + right) * 0.5;
        let sy = top + 4.0;

        pick_hit_node_at(&st, 800, 600, sx, sy, now, None).expect("surface hit")
    }

    #[test]
    fn active_surface_hits_do_not_synthesize_move_zones() {
        let hit = active_surface_hit_with_tuning(single_monitor_tuning());
        assert!(!hit.move_surface);
        assert!(!hit.is_core);
    }

    #[test]
    fn overlap_policy_window_hits_above_same_monitor_fullscreen() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let overlap = st.model.field.spawn_surface(
            "overlap",
            halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            halley_core::field::Vec2 { x: 220.0, y: 160.0 },
        );
        let fullscreen = st.model.field.spawn_surface(
            "fullscreen",
            halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            halley_core::field::Vec2 { x: 800.0, y: 600.0 },
        );
        st.assign_node_to_monitor(overlap, "monitor_a");
        st.assign_node_to_monitor(fullscreen, "monitor_a");
        let _ = st
            .model
            .field
            .set_state(overlap, halley_core::field::NodeState::Active);
        let _ = st
            .model
            .field
            .set_state(fullscreen, halley_core::field::NodeState::Active);
        st.model
            .fullscreen_state
            .fullscreen_active_node
            .insert("monitor_a".to_string(), fullscreen);
        st.model.spawn_state.applied_window_rules.insert(
            overlap,
            crate::compositor::spawn::state::AppliedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::All,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Float,
                opacity: 1.0,
                parent_node: None,
                suppress_reveal_pan: true,
                builtin_rule: None,
            },
        );

        let hit = pick_hit_node_at(&st, 800, 600, 400.0, 300.0, Instant::now(), None)
            .expect("surface hit");

        assert_eq!(hit.node_id, overlap);
    }

    #[test]
    fn overlap_policy_hit_order_uses_raise_stack() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let raised = st.model.field.spawn_surface(
            "raised",
            halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            halley_core::field::Vec2 { x: 220.0, y: 160.0 },
        );
        let newer = st.model.field.spawn_surface(
            "newer",
            halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            halley_core::field::Vec2 { x: 220.0, y: 160.0 },
        );
        for node_id in [raised, newer] {
            st.assign_node_to_monitor(node_id, "monitor_a");
            let _ = st
                .model
                .field
                .set_state(node_id, halley_core::field::NodeState::Active);
            st.model.spawn_state.applied_window_rules.insert(
                node_id,
                crate::compositor::spawn::state::AppliedInitialWindowRule {
                    overlap_policy: halley_config::InitialWindowOverlapPolicy::All,
                    spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                    cluster_participation: halley_config::InitialWindowClusterParticipation::Float,
                    opacity: 1.0,
                    parent_node: None,
                    suppress_reveal_pan: true,
                    builtin_rule: None,
                },
            );
        }
        st.model.focus_state.overlap_raise_order.insert(raised, 20);
        st.model.focus_state.overlap_raise_order.insert(newer, 10);

        let hit = pick_hit_node_at(&st, 800, 600, 400.0, 300.0, Instant::now(), None)
            .expect("surface hit");

        assert_eq!(hit.node_id, raised);
    }

    #[test]
    fn active_stack_hit_order_prefers_front_card_over_raise_rank() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_stacking_tuning());

        let front = st.model.field.spawn_surface(
            "front",
            halley_core::field::Vec2 { x: 100.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        let back = st.model.field.spawn_surface(
            "back",
            halley_core::field::Vec2 { x: 180.0, y: 100.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        for node_id in [front, back] {
            st.assign_node_to_monitor(node_id, "monitor_a");
        }
        let cid = st.create_cluster(vec![front, back]).expect("cluster");
        let core = st.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");
        assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));
        st.model.focus_state.overlap_raise_order.insert(back, 100);

        let now = Instant::now();
        let front_rect =
            active_node_screen_rect(&st, 800, 600, front, now, None).expect("front rect");
        let back_rect = active_node_screen_rect(&st, 800, 600, back, now, None).expect("back rect");
        let left = front_rect.0.max(back_rect.0);
        let top = front_rect.1.max(back_rect.1);
        let right = front_rect.2.min(back_rect.2);
        let bottom = front_rect.3.min(back_rect.3);
        assert!(right > left && bottom > top);

        let hit = pick_hit_node_at(
            &st,
            800,
            600,
            (left + right) * 0.5,
            (top + bottom) * 0.5,
            now,
            None,
        )
        .expect("surface hit");

        assert_eq!(hit.node_id, front);
        assert!(crate::input::pointer::motion::node_is_pointer_draggable(
            &st, front
        ));
    }

    #[test]
    fn maximized_window_uses_normal_raise_order_for_hits() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.animations.maximize.enabled = false;
        let mut st = Halley::new_for_test(&dh, tuning);

        let maximized = st.model.field.spawn_surface(
            "maximized",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        let floating = st.model.field.spawn_surface(
            "floating",
            halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            halley_core::field::Vec2 { x: 220.0, y: 160.0 },
        );
        for node_id in [maximized, floating] {
            st.assign_node_to_monitor(node_id, "monitor_a");
            let _ = st
                .model
                .field
                .set_state(node_id, halley_core::field::NodeState::Active);
        }
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut st,
                maximized,
                Instant::now(),
                "monitor_a",
            )
        );

        let hit = pick_hit_node_at(&st, 800, 600, 400.0, 300.0, Instant::now(), None)
            .expect("surface hit");
        assert_eq!(hit.node_id, floating);

        assert!(st.raise_overlap_policy_node(maximized));
        let hit = pick_hit_node_at(&st, 800, 600, 400.0, 300.0, Instant::now(), None)
            .expect("surface hit");
        assert_eq!(hit.node_id, maximized);
    }

    #[test]
    fn fullscreen_window_uses_normal_raise_order_for_hits() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let fullscreen = st.model.field.spawn_surface(
            "fullscreen",
            halley_core::field::Vec2 { x: 120.0, y: 140.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        let floating = st.model.field.spawn_surface(
            "floating",
            halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            halley_core::field::Vec2 { x: 220.0, y: 160.0 },
        );
        for node_id in [fullscreen, floating] {
            st.assign_node_to_monitor(node_id, "monitor_a");
            let _ = st
                .model
                .field
                .set_state(node_id, halley_core::field::NodeState::Active);
        }
        st.model
            .fullscreen_state
            .fullscreen_active_node
            .insert("monitor_a".to_string(), fullscreen);

        let hit = pick_hit_node_at(&st, 800, 600, 400.0, 300.0, Instant::now(), None)
            .expect("surface hit");
        assert_eq!(hit.node_id, floating);

        assert!(st.raise_overlap_policy_node(fullscreen));
        let hit = pick_hit_node_at(&st, 800, 600, 400.0, 300.0, Instant::now(), None)
            .expect("surface hit");
        assert_eq!(hit.node_id, fullscreen);
    }

    #[test]
    fn overlap_policy_hover_focus_does_not_change_hit_order() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let top = st.model.field.spawn_surface(
            "top",
            halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            halley_core::field::Vec2 { x: 220.0, y: 160.0 },
        );
        let hovered = st.model.field.spawn_surface(
            "hovered",
            halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            halley_core::field::Vec2 { x: 220.0, y: 160.0 },
        );
        for node_id in [top, hovered] {
            st.assign_node_to_monitor(node_id, "monitor_a");
            let _ = st
                .model
                .field
                .set_state(node_id, halley_core::field::NodeState::Active);
            st.model.spawn_state.applied_window_rules.insert(
                node_id,
                crate::compositor::spawn::state::AppliedInitialWindowRule {
                    overlap_policy: halley_config::InitialWindowOverlapPolicy::All,
                    spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                    cluster_participation: halley_config::InitialWindowClusterParticipation::Float,
                    opacity: 1.0,
                    parent_node: None,
                    suppress_reveal_pan: true,
                    builtin_rule: None,
                },
            );
        }
        st.model.focus_state.overlap_raise_order.insert(top, 20);
        st.model.focus_state.overlap_raise_order.insert(hovered, 10);
        st.set_recent_top_node(
            hovered,
            Instant::now() + std::time::Duration::from_millis(1200),
        );

        let hit = pick_hit_node_at(&st, 800, 600, 400.0, 300.0, Instant::now(), None)
            .expect("surface hit");

        assert_eq!(hit.node_id, top);
    }
}
