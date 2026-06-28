use std::time::{Duration, Instant};

use super::super::button::{ButtonFrame, active_pointer_binding};
use crate::backend::interface::BackendView;
use crate::compositor::interaction::state::ActiveDragState;
use crate::compositor::interaction::{DragAxisMode, DragCtx, HitNode, ModState, PointerState};
use crate::compositor::root::Halley;
use crate::compositor::surface::{
    active_stacking_front_member_for_monitor, is_active_stacking_workspace_member,
    node_blocks_interactive_transform,
};

pub(crate) fn node_is_pointer_draggable(st: &Halley, node_id: halley_core::field::NodeId) -> bool {
    if st.is_fullscreen_active(node_id) {
        return false;
    }
    if node_blocks_interactive_transform(st, node_id) {
        return false;
    }
    if crate::compositor::workspace::state::node_in_maximize_session(st, node_id) {
        return false;
    }
    if is_active_stacking_workspace_member(st, node_id) {
        let front = st
            .model
            .monitor_state
            .node_monitor
            .get(&node_id)
            .and_then(|monitor| active_stacking_front_member_for_monitor(st, monitor.as_str()));
        if front != Some(node_id) {
            return false;
        }
    }
    st.model.field.node(node_id).is_some_and(|n| match n.kind {
        halley_core::field::NodeKind::Surface => st.model.field.is_visible(node_id),
        halley_core::field::NodeKind::Core => n.state == halley_core::field::NodeState::Core,
    })
}

enum ActiveStackingMemberDrop {
    ReturnToLayout(String),
    Detach(halley_core::cluster::ClusterId),
}

fn active_stacking_member_drop(
    st: &Halley,
    node_id: halley_core::field::NodeId,
    world_pos: halley_core::field::Vec2,
) -> Option<ActiveStackingMemberDrop> {
    if !matches!(
        st.runtime.tuning.cluster_layout_kind(),
        halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Stacking
    ) {
        return None;
    }
    let Some(cid) = st.model.field.cluster_id_for_member_public(node_id) else {
        return None;
    };
    let Some(monitor) = st.model.monitor_state.node_monitor.get(&node_id) else {
        return None;
    };
    if st.active_cluster_workspace_for_monitor(monitor.as_str()) != Some(cid) {
        return None;
    }

    let Some(cluster) = st.model.field.cluster(cid) else {
        return None;
    };
    let inside_stack = cluster.members().iter().copied().any(|member| {
        st.active_cluster_tile_rect_for_member(monitor.as_str(), member)
            .is_some_and(|rect| {
                world_pos.x >= rect.x
                    && world_pos.x <= rect.x + rect.w
                    && world_pos.y >= rect.y
                    && world_pos.y <= rect.y + rect.h
            })
    });

    Some(if inside_stack {
        ActiveStackingMemberDrop::ReturnToLayout(monitor.clone())
    } else {
        ActiveStackingMemberDrop::Detach(cid)
    })
}

fn join_active_stacking_layout_at(
    st: &mut Halley,
    monitor: &str,
    node_id: halley_core::field::NodeId,
    world_pos: halley_core::field::Vec2,
    now: Instant,
    now_ms: u64,
) -> bool {
    if !matches!(
        st.runtime.tuning.cluster_layout_kind(),
        halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Stacking
    ) || st
        .model
        .field
        .cluster_id_for_member_public(node_id)
        .is_some()
    {
        return false;
    }
    let Some(cid) = st.active_cluster_workspace_for_monitor(monitor) else {
        return false;
    };
    let Some(cluster) = st.model.field.cluster(cid) else {
        return false;
    };
    let inside_active_stack = cluster.members().iter().copied().any(|member| {
        st.active_cluster_tile_rect_for_member(monitor, member)
            .is_some_and(|rect| {
                world_pos.x >= rect.x
                    && world_pos.x <= rect.x + rect.w
                    && world_pos.y >= rect.y
                    && world_pos.y <= rect.y + rect.h
            })
    });
    if !inside_active_stack || !st.absorb_node_into_cluster(cid, node_id, now) {
        return false;
    }

    st.layout_active_cluster_workspace_for_monitor(monitor, now_ms);
    true
}

fn drag_edge_pan_eligible(
    st: &Halley,
    node_id: halley_core::field::NodeId,
    drag_monitor: &str,
    allow_monitor_transfer: bool,
) -> bool {
    !allow_monitor_transfer
        && st.model.field.node(node_id).is_some_and(|n| {
            n.kind == halley_core::field::NodeKind::Surface
                && n.state == halley_core::field::NodeState::Active
                && st.model.field.is_visible(node_id)
        })
        && crate::compositor::interaction::state::node_fully_visible_on_monitor(
            st,
            drag_monitor,
            node_id,
        )
        .unwrap_or(false)
}

fn drag_allows_monitor_transfer_for_mods(st: &Halley, mods: &ModState, drag: DragCtx) -> bool {
    if !drag.requires_drag_modifier {
        return drag.allow_monitor_transfer;
    }

    match active_pointer_binding(st, mods, 0x110) {
        Some(halley_config::PointerBindingAction::PanField) => false,
        Some(halley_config::PointerBindingAction::MoveWindow) => true,
        _ => drag.allow_monitor_transfer,
    }
}

pub(crate) fn begin_drag(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
    frame: ButtonFrame,
    world_now: halley_core::field::Vec2,
    allow_monitor_transfer: bool,
    requires_drag_modifier: bool,
) {
    let now = Instant::now();
    st.input.interaction_state.pending_core_press = None;
    st.input.interaction_state.pending_core_click = None;
    st.input.interaction_state.pending_collapsed_node_press = None;
    st.input.interaction_state.pending_collapsed_node_click = None;
    // Grabbing a core whose bloom is open (from hover) collapses the bloom first so
    // the drag moves the core itself rather than leaving the fan-out behind. Single
    // choke point for every core-drag entry (plain press, drag/move bindings).
    if hit.is_core {
        let _ = crate::input::pointer::button::collapse_bloom_for_core_if_open(st, hit.node_id);
    }
    if let Some(monitor) =
        crate::compositor::workspace::state::maximize_session_monitor_for_node(st, hit.node_id)
    {
        let _ = crate::compositor::workspace::state::abort_maximize_session_for_monitor(
            st,
            monitor.as_str(),
        );
    }
    let drag_monitor = st.monitor_for_node_or_current(hit.node_id);
    let edge_pan_eligible = drag_edge_pan_eligible(
        st,
        hit.node_id,
        drag_monitor.as_str(),
        allow_monitor_transfer,
    );
    let mut drag_ctx = DragCtx {
        node_id: hit.node_id,
        allow_monitor_transfer,
        requires_drag_modifier,
        edge_pan_eligible,
        current_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
        center_latched: false,
        started_active: false,
        edge_pan_x: DragAxisMode::Free,
        edge_pan_y: DragAxisMode::Free,
        edge_pan_pressure: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
        last_pointer_world: world_now,
        last_update_at: Instant::now(),
        release_velocity: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
    };
    if let Some(n) = st.model.field.node(hit.node_id) {
        drag_ctx.started_active = n.state == halley_core::field::NodeState::Active;
        let off = halley_core::field::Vec2 {
            x: world_now.x - n.pos.x,
            y: world_now.y - n.pos.y,
        };
        if st.runtime.tuning.center_window_to_mouse {
            drag_ctx.current_offset = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
            drag_ctx.center_latched = true;
        } else {
            drag_ctx.current_offset = off;
        }
    }
    if !node_is_pointer_draggable(st, hit.node_id) {
        return;
    }
    ps.drag = Some(drag_ctx);
    st.assign_node_to_monitor(hit.node_id, drag_monitor.as_str());
    st.input
        .interaction_state
        .physics_velocity
        .remove(&hit.node_id);
    st.input.interaction_state.drag_authority_velocity =
        halley_core::field::Vec2 { x: 0.0, y: 0.0 };
    crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
    st.input.interaction_state.active_drag = Some(ActiveDragState {
        node_id: hit.node_id,
        allow_monitor_transfer,
        edge_pan_eligible,
        current_offset: drag_ctx.current_offset,
        pointer_monitor: drag_monitor,
        pointer_workspace_size: (frame.ws_w, frame.ws_h),
        pointer_screen_local: (frame.sx, frame.sy),
        edge_pan_x: DragAxisMode::Free,
        edge_pan_y: DragAxisMode::Free,
        last_edge_pan_at: now,
    });
    crate::compositor::carry::system::set_drag_authority_node(st, Some(hit.node_id));
    crate::compositor::carry::system::begin_carry_state_tracking(st, hit.node_id);
    crate::compositor::interaction::pointer::set_cursor_override_icon(
        st,
        Some(smithay::input::pointer::CursorIcon::Grabbing),
    );
    if !hit.is_core {
        // Dragging a window must not resume a soft-suspended fullscreen session.
        st.input
            .interaction_state
            .suppress_fullscreen_resume_on_focus = true;
        st.focus_pointer_target(hit.node_id, 30_000, now);
        st.input
            .interaction_state
            .suppress_fullscreen_resume_on_focus = false;
    }
    let to = halley_core::field::Vec2 {
        x: world_now.x - drag_ctx.current_offset.x,
        y: world_now.y - drag_ctx.current_offset.y,
    };
    let _ = st.carry_surface_non_overlap(hit.node_id, to, false);
    backend.request_redraw();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::interface::BackendView;
    use crate::compositor::interaction::PointerState;
    use crate::compositor::root::Halley;
    use smithay::reexports::wayland_server::Display;

    struct TestBackend;

    impl BackendView for TestBackend {
        fn window_size_i32(&self) -> (i32, i32) {
            (1600, 1200)
        }

        fn request_redraw(&self) {}
    }

    fn move_window_mods() -> ModState {
        ModState {
            alt_down: true,
            left_alt_down: true,
            ..ModState::default()
        }
    }

    fn pan_field_mods() -> ModState {
        ModState {
            alt_down: true,
            left_alt_down: true,
            shift_down: true,
            left_shift_down: true,
            ..ModState::default()
        }
    }

    fn stacking_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Stacking;
        tuning.stacking_max_visible = 3;
        tuning
    }

    fn open_stacking_cluster(st: &mut Halley, labels: &[&str]) -> Vec<halley_core::field::NodeId> {
        let monitor = st.model.monitor_state.current_monitor.clone();
        let members = labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let id = st.model.field.spawn_surface(
                    (*label).to_string(),
                    halley_core::field::Vec2 {
                        x: 100.0 + index as f32 * 60.0,
                        y: 100.0,
                    },
                    halley_core::field::Vec2 { x: 320.0, y: 240.0 },
                );
                st.assign_node_to_monitor(id, monitor.as_str());
                id
            })
            .collect::<Vec<_>>();
        let cid = st.create_cluster(members.clone()).expect("cluster");
        let core = st.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, monitor.as_str());
        assert!(st.enter_cluster_workspace_by_core(core, monitor.as_str(), Instant::now()));
        members
    }

    #[test]
    fn ending_collapsed_node_drag_snaps_marker_animation() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = st.model.field.spawn_surface(
            "node",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        let start = Instant::now();
        st.ui
            .render_state
            .animator
            .observe_field(&st.model.field, start);
        let _ = st
            .model
            .field
            .set_state(id, halley_core::field::NodeState::Node);
        crate::frame_loop::tick_animator_frame(
            &mut st,
            start + std::time::Duration::from_millis(1),
        );
        let before = crate::frame_loop::anim_style_for(
            &st,
            id,
            halley_core::field::NodeState::Node,
            start + std::time::Duration::from_millis(16),
        );
        assert!(before.scale > 0.30);

        crate::compositor::carry::system::begin_carry_state_tracking(&mut st, id);
        crate::compositor::carry::system::end_carry_state_tracking(&mut st, id);

        let after = crate::frame_loop::anim_style_for(
            &st,
            id,
            halley_core::field::NodeState::Node,
            start + std::time::Duration::from_millis(16),
        );
        assert_eq!(after.scale, 0.30);
        assert_eq!(after.alpha, 1.0);
    }

    #[test]
    fn finishing_collapsed_node_drag_does_not_use_active_window_static_lock() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = st.model.field.spawn_surface(
            "node",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        let now = Instant::now();
        let _ = st
            .model
            .field
            .set_state(id, halley_core::field::NodeState::Node);

        let mut ps = PointerState::default();
        finish_pointer_drag(
            &mut st,
            &mut ps,
            id,
            false,
            halley_core::field::Vec2 { x: 40.0, y: 40.0 },
            now,
        );

        assert_ne!(st.input.interaction_state.resize_static_node, Some(id));
        let style = crate::frame_loop::anim_style_for(
            &st,
            id,
            halley_core::field::NodeState::Node,
            now + std::time::Duration::from_millis(16),
        );
        assert_eq!(style.scale, 0.30);
    }

    #[test]
    fn move_window_drag_does_not_enable_edge_pan() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = st.model.field.spawn_surface(
            "dragged",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        let hit = HitNode {
            node_id: id,
            move_surface: false,
            is_core: false,
        };
        let world_now = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
        let frame = ButtonFrame {
            global_sx: 200.0,
            global_sy: 120.0,
            sx: 200.0,
            sy: 120.0,
            ws_w: 1600,
            ws_h: 1200,
            world_now,
            workspace_active: true,
        };
        let mut ps = PointerState::default();

        begin_drag(
            &mut st,
            &mut ps,
            &TestBackend,
            hit,
            frame,
            world_now,
            true,
            false,
        );

        assert!(ps.drag.is_some_and(|drag| !drag.edge_pan_eligible));
        assert!(!st.input.interaction_state.grabbed_edge_pan_active);
    }

    #[test]
    fn shift_pressed_during_move_window_drag_enables_edge_pan_when_visible() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = st.model.field.spawn_surface(
            "dragged",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        let hit = HitNode {
            node_id: id,
            move_surface: false,
            is_core: false,
        };
        let world_now = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
        let frame = ButtonFrame {
            global_sx: 200.0,
            global_sy: 120.0,
            sx: 200.0,
            sy: 120.0,
            ws_w: 1600,
            ws_h: 1200,
            world_now,
            workspace_active: true,
        };
        let mut ps = PointerState::default();
        begin_drag(
            &mut st,
            &mut ps,
            &TestBackend,
            hit,
            frame,
            world_now,
            true,
            true,
        );

        let monitor = st.monitor_for_node_or_current(id);
        assert!(handle_drag_motion(
            &mut st,
            &TestBackend,
            &pan_field_mods(),
            &mut ps,
            true,
            monitor.as_str(),
            1600,
            1200,
            200.0,
            120.0,
            world_now,
            Instant::now(),
        ));

        let drag = ps.drag.expect("active drag");
        assert!(!drag.allow_monitor_transfer);
        assert!(drag.edge_pan_eligible);
        assert!(
            st.input
                .interaction_state
                .active_drag
                .as_ref()
                .is_some_and(|drag| !drag.allow_monitor_transfer && drag.edge_pan_eligible)
        );
    }

    #[test]
    fn releasing_shift_during_pan_field_drag_returns_to_move_window_mode() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = st.model.field.spawn_surface(
            "dragged",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        let hit = HitNode {
            node_id: id,
            move_surface: false,
            is_core: false,
        };
        let world_now = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
        let frame = ButtonFrame {
            global_sx: 200.0,
            global_sy: 120.0,
            sx: 200.0,
            sy: 120.0,
            ws_w: 1600,
            ws_h: 1200,
            world_now,
            workspace_active: true,
        };
        let mut ps = PointerState::default();
        begin_drag(
            &mut st,
            &mut ps,
            &TestBackend,
            hit,
            frame,
            world_now,
            false,
            true,
        );

        let monitor = st.monitor_for_node_or_current(id);
        assert!(handle_drag_motion(
            &mut st,
            &TestBackend,
            &move_window_mods(),
            &mut ps,
            true,
            monitor.as_str(),
            1600,
            1200,
            200.0,
            120.0,
            world_now,
            Instant::now(),
        ));

        let drag = ps.drag.expect("active drag");
        assert!(drag.allow_monitor_transfer);
        assert!(!drag.edge_pan_eligible);
        assert!(
            st.input
                .interaction_state
                .active_drag
                .as_ref()
                .is_some_and(|drag| drag.allow_monitor_transfer && !drag.edge_pan_eligible)
        );
    }

    #[test]
    fn partially_offscreen_active_surface_drag_does_not_enable_edge_pan() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = st.model.field.spawn_surface(
            "dragged",
            halley_core::field::Vec2 { x: 930.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        let hit = HitNode {
            node_id: id,
            move_surface: false,
            is_core: false,
        };
        let world_now = halley_core::field::Vec2 { x: 930.0, y: 0.0 };
        let frame = ButtonFrame {
            global_sx: 200.0,
            global_sy: 120.0,
            sx: 200.0,
            sy: 120.0,
            ws_w: 1600,
            ws_h: 1200,
            world_now,
            workspace_active: true,
        };
        let mut ps = PointerState::default();

        begin_drag(
            &mut st,
            &mut ps,
            &TestBackend,
            hit,
            frame,
            world_now,
            false,
            false,
        );

        assert!(ps.drag.is_some_and(|drag| !drag.edge_pan_eligible));
        assert!(!st.input.interaction_state.grabbed_edge_pan_active);
    }

    #[test]
    fn maximized_session_surface_is_not_pointer_draggable() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.animations.maximize.enabled = false;
        let mut st = Halley::new_for_test(&dh, tuning);
        let id = st.model.field.spawn_surface(
            "dragged",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        st.assign_node_to_current_monitor(id);
        let monitor = st.focused_monitor().to_string();

        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut st,
                id,
                Instant::now(),
                monitor.as_str(),
            )
        );

        assert!(!node_is_pointer_draggable(&st, id));
    }

    #[test]
    fn only_front_stacking_member_is_pointer_draggable() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, stacking_tuning());
        let members = open_stacking_cluster(&mut st, &["front", "back"]);

        assert!(node_is_pointer_draggable(&st, members[0]));
        assert!(!node_is_pointer_draggable(&st, members[1]));
    }

    #[test]
    fn finishing_stacking_member_drag_returns_to_stack_layout() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, stacking_tuning());
        let members = open_stacking_cluster(&mut st, &["front", "back"]);
        let front = members[0];
        let original_pos = st.model.field.node(front).expect("front").pos;

        st.input.interaction_state.drag_authority_node = Some(front);
        st.input.interaction_state.active_drag = Some(ActiveDragState {
            node_id: front,
            allow_monitor_transfer: true,
            edge_pan_eligible: false,
            current_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            pointer_monitor: st.model.monitor_state.current_monitor.clone(),
            pointer_workspace_size: (1600, 1200),
            pointer_screen_local: (200.0, 120.0),
            edge_pan_x: DragAxisMode::Free,
            edge_pan_y: DragAxisMode::Free,
            last_edge_pan_at: Instant::now(),
        });
        st.model.field.node_mut(front).expect("front").pos = halley_core::field::Vec2 {
            x: 2000.0,
            y: 2000.0,
        };

        let mut ps = PointerState::default();
        finish_pointer_drag(&mut st, &mut ps, front, true, original_pos, Instant::now());

        let final_pos = st.model.field.node(front).expect("front").pos;
        assert!((final_pos.x - original_pos.x).abs() <= 0.5);
        assert!((final_pos.y - original_pos.y).abs() <= 0.5);
    }

    #[test]
    fn dropping_two_window_stack_member_outside_leaves_singleton_cluster() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, stacking_tuning());
        let members = open_stacking_cluster(&mut st, &["front", "back"]);
        let front = members[0];
        let cid = st
            .model
            .field
            .cluster_id_for_member_public(front)
            .expect("cluster");
        let drop_pos = halley_core::field::Vec2 {
            x: 5000.0,
            y: 5000.0,
        };

        st.input.interaction_state.drag_authority_node = Some(front);
        st.input.interaction_state.active_drag = Some(ActiveDragState {
            node_id: front,
            allow_monitor_transfer: true,
            edge_pan_eligible: false,
            current_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            pointer_monitor: st.model.monitor_state.current_monitor.clone(),
            pointer_workspace_size: (1600, 1200),
            pointer_screen_local: (200.0, 120.0),
            edge_pan_x: DragAxisMode::Free,
            edge_pan_y: DragAxisMode::Free,
            last_edge_pan_at: Instant::now(),
        });

        let mut ps = PointerState::default();
        finish_pointer_drag(&mut st, &mut ps, front, true, drop_pos, Instant::now());

        let cluster = st.model.field.cluster(cid).expect("cluster");
        assert_eq!(cluster.members().len(), 1);
        assert!(!cluster.contains(front));
        assert_eq!(st.model.field.cluster_id_for_member_public(front), None);
        let final_pos = st.model.field.node(front).expect("front").pos;
        assert!((final_pos.x - drop_pos.x).abs() <= 0.5);
        assert!((final_pos.y - drop_pos.y).abs() <= 0.5);
    }

    #[test]
    fn dropping_three_window_stack_member_outside_detaches_member() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, stacking_tuning());
        let members = open_stacking_cluster(&mut st, &["front", "middle", "back"]);
        let front = members[0];
        let cid = st
            .model
            .field
            .cluster_id_for_member_public(front)
            .expect("cluster");
        let drop_pos = halley_core::field::Vec2 {
            x: 5000.0,
            y: 5000.0,
        };

        st.input.interaction_state.drag_authority_node = Some(front);
        st.input.interaction_state.active_drag = Some(ActiveDragState {
            node_id: front,
            allow_monitor_transfer: true,
            edge_pan_eligible: false,
            current_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            pointer_monitor: st.model.monitor_state.current_monitor.clone(),
            pointer_workspace_size: (1600, 1200),
            pointer_screen_local: (200.0, 120.0),
            edge_pan_x: DragAxisMode::Free,
            edge_pan_y: DragAxisMode::Free,
            last_edge_pan_at: Instant::now(),
        });

        let mut ps = PointerState::default();
        finish_pointer_drag(&mut st, &mut ps, front, true, drop_pos, Instant::now());

        let cluster = st.model.field.cluster(cid).expect("cluster");
        assert_eq!(cluster.members().len(), 2);
        assert!(!cluster.contains(front));
        assert_eq!(st.model.field.cluster_id_for_member_public(front), None);
    }

    #[test]
    fn dropping_floating_window_on_active_stack_joins_front() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, stacking_tuning());
        let members = open_stacking_cluster(&mut st, &["front", "back"]);
        let monitor = st.model.monitor_state.current_monitor.clone();
        let cid = st
            .model
            .field
            .cluster_id_for_member_public(members[0])
            .expect("cluster");
        let top_rect = st
            .active_cluster_tile_rect_for_member(monitor.as_str(), members[0])
            .expect("top rect");
        let drop_pos = halley_core::field::Vec2 {
            x: top_rect.x + top_rect.w * 0.5,
            y: top_rect.y + top_rect.h * 0.5,
        };
        let floating = st.model.field.spawn_surface(
            "floating",
            halley_core::field::Vec2 {
                x: -500.0,
                y: -500.0,
            },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(floating, monitor.as_str());
        st.input.interaction_state.drag_authority_node = Some(floating);
        st.input.interaction_state.active_drag = Some(ActiveDragState {
            node_id: floating,
            allow_monitor_transfer: true,
            edge_pan_eligible: false,
            current_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            pointer_monitor: monitor.clone(),
            pointer_workspace_size: (1600, 1200),
            pointer_screen_local: (200.0, 120.0),
            edge_pan_x: DragAxisMode::Free,
            edge_pan_y: DragAxisMode::Free,
            last_edge_pan_at: Instant::now(),
        });

        let mut ps = PointerState::default();
        finish_pointer_drag(&mut st, &mut ps, floating, true, drop_pos, Instant::now());

        let cluster = st.model.field.cluster(cid).expect("cluster");
        assert_eq!(cluster.members().first().copied(), Some(floating));
        let final_rect = st
            .active_cluster_tile_rect_for_member(monitor.as_str(), floating)
            .expect("floating rect");
        let final_pos = st.model.field.node(floating).expect("floating").pos;
        assert!((final_pos.x - (final_rect.x + final_rect.w * 0.5)).abs() <= 0.5);
        assert!((final_pos.y - (final_rect.y + final_rect.h * 0.5)).abs() <= 0.5);
    }

    #[test]
    fn finishing_active_drag_raises_dropped_window_to_front() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let existing = st.model.field.spawn_surface(
            "existing",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        let dragged = st.model.field.spawn_surface(
            "dragged",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 400.0, y: 260.0 },
        );
        st.assign_node_to_current_monitor(existing);
        st.assign_node_to_current_monitor(dragged);
        let _ = st
            .model
            .field
            .set_state(existing, halley_core::field::NodeState::Active);
        let _ = st
            .model
            .field
            .set_state(dragged, halley_core::field::NodeState::Active);
        assert!(st.raise_overlap_policy_node(existing));
        assert!(st.overlap_policy_stack_rank(existing) > st.overlap_policy_stack_rank(dragged));

        let mut ps = PointerState::default();
        st.input.interaction_state.active_drag = Some(ActiveDragState {
            node_id: dragged,
            allow_monitor_transfer: true,
            edge_pan_eligible: false,
            current_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            pointer_monitor: st.model.monitor_state.current_monitor.clone(),
            pointer_workspace_size: (1600, 1200),
            pointer_screen_local: (200.0, 120.0),
            edge_pan_x: DragAxisMode::Free,
            edge_pan_y: DragAxisMode::Free,
            last_edge_pan_at: Instant::now(),
        });

        finish_pointer_drag(
            &mut st,
            &mut ps,
            dragged,
            true,
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            Instant::now(),
        );

        // The dropped window is now raised above the previously-frontmost peer.
        assert!(st.overlap_policy_stack_rank(dragged) > st.overlap_policy_stack_rank(existing));
        assert_eq!(
            st.model.focus_state.primary_interaction_focus,
            Some(dragged)
        );
    }
}

pub(crate) fn finish_pointer_drag(
    st: &mut Halley,
    ps: &mut PointerState,
    node_id: halley_core::field::NodeId,
    started_active: bool,
    world_now: halley_core::field::Vec2,
    now: Instant,
) {
    let now_ms = st.now_ms(now);
    let drag_monitor = st
        .input
        .interaction_state
        .active_drag
        .as_ref()
        .map(|drag| drag.pointer_monitor.clone())
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
    st.input.interaction_state.active_drag = None;
    let joined =
        crate::compositor::clusters::system::commit_ready_cluster_join_for_node(st, node_id, now)
            || join_active_stacking_layout_at(
                st,
                drag_monitor.as_str(),
                node_id,
                world_now,
                now,
                now_ms,
            );
    // A drop onto an active *tiled* cluster workspace must let the cluster layout own the
    // window's final position. Clear the carry authority *before* the drop re-layout so the
    // member animates to its slot in a single tile track (while authority is set, the layout
    // treats it as still-dragging and skips it, so it only animates on a later layout — and
    // the release-position static lock below fights that, producing the double "glitch into
    // place"). Mirrors the keybind move, which has neither carry nor static lock.
    let tiled_cluster_member = !joined
        && started_active
        && crate::compositor::clusters::system::node_is_active_tiled_cluster_member(
            st,
            drag_monitor.as_str(),
            node_id,
        );
    if !joined {
        if started_active {
            if tiled_cluster_member {
                crate::compositor::carry::system::set_drag_authority_node(st, None);
            }
            let moved = st.move_active_cluster_member_to_drop_tile(
                drag_monitor.as_str(),
                node_id,
                world_now,
                now_ms,
            );
            // Dropped on its own slot (no reorder): still re-tile so it animates back
            // instead of being left at the release position.
            if tiled_cluster_member && !moved {
                st.layout_active_cluster_workspace_for_monitor(drag_monitor.as_str(), now_ms);
            }
            crate::compositor::carry::system::finalize_mouse_drag_state(
                st,
                node_id,
                halley_core::field::Vec2 { x: 0.0, y: 0.0 },
                now,
            );
        }
    } else {
        st.input.interaction_state.cluster_join_candidate = None;
    }
    let active_stacking_drop =
        started_active.then(|| active_stacking_member_drop(st, node_id, world_now));
    crate::compositor::carry::system::set_drag_authority_node(st, None);
    crate::compositor::carry::system::end_carry_state_tracking(st, node_id);
    match active_stacking_drop.flatten() {
        Some(ActiveStackingMemberDrop::ReturnToLayout(monitor)) => {
            st.layout_active_cluster_workspace_for_monitor(monitor.as_str(), now_ms);
        }
        Some(ActiveStackingMemberDrop::Detach(cid)) => {
            let _ = st.detach_member_from_cluster(cid, node_id, world_now, now);
        }
        None => {}
    }
    if started_active {
        st.set_recent_top_node(node_id, now + Duration::from_millis(1200));
        st.set_interaction_focus(Some(node_id), 30_000, now);
        // An explicit drag-and-drop is a strong "put this on top" intent, so always
        // bring the dropped window to the front (no-ops if already frontmost). This is
        // independent of `raise_on_click`, and is what keeps a window dropped over peers
        // on another monitor from landing behind them.
        let _ = st.raise_overlap_policy_node(node_id);
    }
    // Hold the just-dropped window fixed at its release position so the overlap
    // resolver pushes *neighbors* apart instead of snapping the dropped window.
    // Reuses the resize static-lock that the solver already treats as immovable.
    if started_active
        && !tiled_cluster_member
        && st.input.interaction_state.resize_static_node.is_none()
        && let Some(pos) = st.model.field.node(node_id).map(|node| node.pos)
    {
        st.input.interaction_state.resize_static_node = Some(node_id);
        st.input.interaction_state.resize_static_lock_pos = Some(pos);
        st.input.interaction_state.resize_static_until_ms = now_ms + 350;
    }
    st.resolve_surface_overlap();
    ps.hover_node = None;
    ps.hover_started_at = None;
    ps.preview_block_until = Some(now + Duration::from_millis(360));
    ps.drag = None;
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_drag_motion(
    st: &mut Halley,
    backend: &impl BackendView,
    mods: &ModState,
    ps: &mut PointerState,
    drag_mod_ok: bool,
    target_monitor: &str,
    local_w: i32,
    local_h: i32,
    local_sx: f32,
    local_sy: f32,
    pointer_world: halley_core::field::Vec2,
    now: Instant,
) -> bool {
    let Some(drag) = ps.drag else {
        st.input.interaction_state.cluster_join_candidate = None;
        return false;
    };

    let drag_allowed = !drag.requires_drag_modifier || drag_mod_ok;
    if ps.resize.is_some() || !drag_allowed {
        let joined = !drag_allowed
            && crate::compositor::clusters::system::commit_ready_cluster_join_for_node(
                st,
                drag.node_id,
                now,
            );
        crate::compositor::carry::system::set_drag_authority_node(st, None);
        crate::compositor::carry::system::end_carry_state_tracking(st, drag.node_id);
        ps.drag = None;
        st.input.interaction_state.active_drag = None;
        crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
        if joined {
            backend.request_redraw();
        }
        return joined;
    }

    let mut next_drag = drag;
    let drag_allow_monitor_transfer = drag_allows_monitor_transfer_for_mods(st, mods, next_drag);
    next_drag.allow_monitor_transfer = drag_allow_monitor_transfer;
    let edge_pan_monitor = st.monitor_for_node_or_current(drag.node_id);
    next_drag.edge_pan_eligible = drag_edge_pan_eligible(
        st,
        drag.node_id,
        edge_pan_monitor.as_str(),
        drag_allow_monitor_transfer,
    );
    let dt = now
        .saturating_duration_since(next_drag.last_update_at)
        .as_secs_f32()
        .max(1.0 / 240.0);
    let raw_velocity = halley_core::field::Vec2 {
        x: (pointer_world.x - next_drag.last_pointer_world.x) / dt,
        y: (pointer_world.y - next_drag.last_pointer_world.y) / dt,
    };
    let max_drag_speed = 800.0f32;
    let clamp_axis = |v: f32| v.clamp(-max_drag_speed, max_drag_speed);
    next_drag.release_velocity = halley_core::field::Vec2 {
        x: next_drag.release_velocity.x * 0.35 + clamp_axis(raw_velocity.x) * 0.65,
        y: next_drag.release_velocity.y * 0.35 + clamp_axis(raw_velocity.y) * 0.65,
    };
    next_drag.last_pointer_world = pointer_world;
    next_drag.last_update_at = now;
    let desired_to = halley_core::field::Vec2 {
        x: pointer_world.x - next_drag.current_offset.x,
        y: pointer_world.y - next_drag.current_offset.y,
    };

    update_drag_edge_pan(
        st,
        drag.node_id,
        target_monitor,
        desired_to,
        dt,
        drag_allow_monitor_transfer,
        &mut next_drag,
    );

    let should_center = st.runtime.tuning.center_window_to_mouse
        && (!next_drag.center_latched
            || next_drag.current_offset.x.abs() > f32::EPSILON
            || next_drag.current_offset.y.abs() > f32::EPSILON);
    if should_center {
        next_drag.current_offset = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
        next_drag.center_latched = true;
    }
    st.input.interaction_state.drag_authority_velocity = next_drag.release_velocity;
    st.input.interaction_state.active_drag = Some(ActiveDragState {
        node_id: drag.node_id,
        allow_monitor_transfer: drag_allow_monitor_transfer,
        edge_pan_eligible: next_drag.edge_pan_eligible,
        current_offset: next_drag.current_offset,
        pointer_monitor: target_monitor.to_string(),
        pointer_workspace_size: (local_w, local_h),
        pointer_screen_local: (local_sx, local_sy),
        edge_pan_x: next_drag.edge_pan_x,
        edge_pan_y: next_drag.edge_pan_y,
        last_edge_pan_at: st
            .input
            .interaction_state
            .active_drag
            .as_ref()
            .map(|drag| drag.last_edge_pan_at)
            .unwrap_or(now),
    });
    ps.drag = Some(next_drag);
    crate::compositor::interaction::pointer::set_cursor_override_icon(
        st,
        Some(smithay::input::pointer::CursorIcon::Grabbing),
    );
    let _ = super::cluster::update_cluster_join_candidate(
        st,
        drag.node_id,
        target_monitor,
        desired_to,
        now,
    );
    backend.request_redraw();
    true
}

fn update_drag_edge_pan(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    target_monitor: &str,
    desired_to: halley_core::field::Vec2,
    dt: f32,
    drag_allow_monitor_transfer: bool,
    next_drag: &mut crate::compositor::interaction::DragCtx,
) {
    if !drag_allow_monitor_transfer
        && next_drag.edge_pan_eligible
        && let Some(owner_monitor) = st
            .model
            .monitor_state
            .node_monitor
            .get(&node_id)
            .cloned()
            .or_else(|| Some(target_monitor.to_string()))
        && let Some((clamped_center, edge_contact)) =
            crate::compositor::interaction::state::dragged_node_edge_pan_clamp(
                st,
                owner_monitor.as_str(),
                node_id,
                desired_to,
                halley_core::field::Vec2 {
                    x: next_drag.edge_pan_x.sign(),
                    y: next_drag.edge_pan_y.sign(),
                },
            )
    {
        const EDGE_PAN_PRESSURE_THRESHOLD: f32 = 56.0;
        const EDGE_PAN_PRESSURE_DECAY_PER_SEC: f32 = 44.0;
        const EDGE_PAN_PRESSURE_BUILD_PER_SEC: f32 = 86.0;
        const EDGE_PAN_PRESSURE_DEPTH_NORM: f32 = 18.0;
        const EDGE_PAN_RELEASE_DISTANCE: f32 = 24.0;

        next_drag.edge_pan_pressure.x =
            (next_drag.edge_pan_pressure.x - EDGE_PAN_PRESSURE_DECAY_PER_SEC * dt).max(0.0);
        next_drag.edge_pan_pressure.y =
            (next_drag.edge_pan_pressure.y - EDGE_PAN_PRESSURE_DECAY_PER_SEC * dt).max(0.0);

        if edge_contact.x < 0.0 {
            let depth = (clamped_center.x - desired_to.x).max(0.0);
            let build = (depth / EDGE_PAN_PRESSURE_DEPTH_NORM).clamp(0.0, 1.25);
            next_drag.edge_pan_pressure.x += EDGE_PAN_PRESSURE_BUILD_PER_SEC * build * dt;
        } else if edge_contact.x > 0.0 {
            let depth = (desired_to.x - clamped_center.x).max(0.0);
            let build = (depth / EDGE_PAN_PRESSURE_DEPTH_NORM).clamp(0.0, 1.25);
            next_drag.edge_pan_pressure.x += EDGE_PAN_PRESSURE_BUILD_PER_SEC * build * dt;
        } else {
            next_drag.edge_pan_pressure.x = 0.0;
        }

        if edge_contact.y < 0.0 {
            let depth = (clamped_center.y - desired_to.y).max(0.0);
            let build = (depth / EDGE_PAN_PRESSURE_DEPTH_NORM).clamp(0.0, 1.25);
            next_drag.edge_pan_pressure.y += EDGE_PAN_PRESSURE_BUILD_PER_SEC * build * dt;
        } else if edge_contact.y > 0.0 {
            let depth = (desired_to.y - clamped_center.y).max(0.0);
            let build = (depth / EDGE_PAN_PRESSURE_DEPTH_NORM).clamp(0.0, 1.25);
            next_drag.edge_pan_pressure.y += EDGE_PAN_PRESSURE_BUILD_PER_SEC * build * dt;
        } else {
            next_drag.edge_pan_pressure.y = 0.0;
        }

        next_drag.edge_pan_x = match next_drag.edge_pan_x {
            DragAxisMode::Free => {
                if edge_contact.x < 0.0
                    && next_drag.edge_pan_pressure.x >= EDGE_PAN_PRESSURE_THRESHOLD
                {
                    DragAxisMode::EdgePanNeg
                } else if edge_contact.x > 0.0
                    && next_drag.edge_pan_pressure.x >= EDGE_PAN_PRESSURE_THRESHOLD
                {
                    DragAxisMode::EdgePanPos
                } else {
                    DragAxisMode::Free
                }
            }
            DragAxisMode::EdgePanNeg => {
                if desired_to.x > clamped_center.x + EDGE_PAN_RELEASE_DISTANCE {
                    next_drag.edge_pan_pressure.x = 0.0;
                    DragAxisMode::Free
                } else {
                    DragAxisMode::EdgePanNeg
                }
            }
            DragAxisMode::EdgePanPos => {
                if desired_to.x < clamped_center.x - EDGE_PAN_RELEASE_DISTANCE {
                    next_drag.edge_pan_pressure.x = 0.0;
                    DragAxisMode::Free
                } else {
                    DragAxisMode::EdgePanPos
                }
            }
        };
        next_drag.edge_pan_y = match next_drag.edge_pan_y {
            DragAxisMode::Free => {
                if edge_contact.y < 0.0
                    && next_drag.edge_pan_pressure.y >= EDGE_PAN_PRESSURE_THRESHOLD
                {
                    DragAxisMode::EdgePanNeg
                } else if edge_contact.y > 0.0
                    && next_drag.edge_pan_pressure.y >= EDGE_PAN_PRESSURE_THRESHOLD
                {
                    DragAxisMode::EdgePanPos
                } else {
                    DragAxisMode::Free
                }
            }
            DragAxisMode::EdgePanNeg => {
                if desired_to.y > clamped_center.y + EDGE_PAN_RELEASE_DISTANCE {
                    next_drag.edge_pan_pressure.y = 0.0;
                    DragAxisMode::Free
                } else {
                    DragAxisMode::EdgePanNeg
                }
            }
            DragAxisMode::EdgePanPos => {
                if desired_to.y < clamped_center.y - EDGE_PAN_RELEASE_DISTANCE {
                    next_drag.edge_pan_pressure.y = 0.0;
                    DragAxisMode::Free
                } else {
                    DragAxisMode::EdgePanPos
                }
            }
        };

        let edge_pan_direction = halley_core::field::Vec2 {
            x: next_drag.edge_pan_x.sign(),
            y: next_drag.edge_pan_y.sign(),
        };
        let edge_pan_active = edge_pan_direction.x != 0.0 || edge_pan_direction.y != 0.0;
        let indicator_direction = if edge_pan_active {
            edge_pan_direction
        } else {
            edge_contact
        };

        st.input.interaction_state.grabbed_edge_pan_active = edge_pan_active;
        st.input.interaction_state.grabbed_edge_pan_direction = indicator_direction;
        st.input.interaction_state.grabbed_edge_pan_pressure = next_drag.edge_pan_pressure;
        st.input.interaction_state.grabbed_edge_pan_monitor = ((indicator_direction.x != 0.0
            || indicator_direction.y != 0.0)
            && (next_drag.edge_pan_pressure.x > 0.0 || next_drag.edge_pan_pressure.y > 0.0))
            .then(|| owner_monitor.clone());
        return;
    }

    st.input.interaction_state.grabbed_edge_pan_active = false;
    st.input.interaction_state.grabbed_edge_pan_direction =
        halley_core::field::Vec2 { x: 0.0, y: 0.0 };
    st.input.interaction_state.grabbed_edge_pan_pressure =
        halley_core::field::Vec2 { x: 0.0, y: 0.0 };
    st.input.interaction_state.grabbed_edge_pan_monitor = None;
    next_drag.edge_pan_x = DragAxisMode::Free;
    next_drag.edge_pan_y = DragAxisMode::Free;
    next_drag.edge_pan_pressure = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
}
