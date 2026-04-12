use std::time::{Duration, Instant};

use smithay::reexports::wayland_server::Display;

use crate::backend::interface::TtyBackendHandle;
use crate::compositor::interaction::{HitNode, PointerState, ResizeHandle};
use crate::compositor::root::Halley;

use crate::input::pointer::button::ButtonFrame;
use super::{
active_node_screen_rect, advance_resize_anim, begin_resize, finalize_resize,
handle_resize_motion, resize_rect_nearly_eq,
};
use super::handles::{commit_handle_from_drag, weights_from_handle};

fn single_monitor_tiling_tuning() -> halley_config::RuntimeTuning {
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

fn resize_button_frame() -> ButtonFrame {
    ButtonFrame {
        ws_w: 800,
        ws_h: 600,
        global_sx: 400.0,
        global_sy: 300.0,
        sx: 400.0,
        sy: 300.0,
        world_now: halley_core::field::Vec2 { x: 400.0, y: 300.0 },
        workspace_active: false,
    }
}

#[test]
fn drag_direction_maps_to_y_down_resize_handles() {
    assert_eq!(commit_handle_from_drag(0.0, -40.0), ResizeHandle::Top);
    assert_eq!(commit_handle_from_drag(0.0, 40.0), ResizeHandle::Bottom);
    assert_eq!(commit_handle_from_drag(40.0, -40.0), ResizeHandle::TopRight);
    assert_eq!(commit_handle_from_drag(-40.0, 40.0), ResizeHandle::BottomLeft);
}

#[test]
fn top_and_bottom_weights_follow_screen_space_motion() {
    assert_eq!(weights_from_handle(ResizeHandle::Top), (0.0, 0.0, 1.0, 0.0));
    assert_eq!(weights_from_handle(ResizeHandle::Bottom), (0.0, 0.0, 0.0, 1.0));
    assert_eq!(weights_from_handle(ResizeHandle::TopLeft), (1.0, 0.0, 1.0, 0.0));
    assert_eq!(
        weights_from_handle(ResizeHandle::BottomRight),
        (0.0, 1.0, 0.0, 1.0)
    );
}

#[test]
fn begin_resize_blocks_active_tiled_workspace_members() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tiling_tuning());
    let backend = TtyBackendHandle::new(800, 600);

    let master = st.model.field.spawn_surface(
        "master",
        halley_core::field::Vec2 { x: 120.0, y: 120.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        halley_core::field::Vec2 { x: 520.0, y: 120.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [master, stack] {
        st.assign_node_to_monitor(id, "monitor_a");
    }
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

    let mut ps = PointerState::default();
    begin_resize(
        &mut st,
        &mut ps,
        &backend,
        HitNode {
            node_id: master,
            on_titlebar: false,
            is_core: false,
        },
        resize_button_frame(),
    );

    assert!(ps.resize.is_none());
    assert!(st.input.interaction_state.resize_active.is_none());
}

#[test]
fn begin_resize_allows_non_tiled_active_windows() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tiling_tuning());
    let backend = TtyBackendHandle::new(800, 600);

    let window = st.model.field.spawn_surface(
        "window",
        halley_core::field::Vec2 { x: 300.0, y: 220.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(window, "monitor_a");

    let mut ps = PointerState::default();
    begin_resize(
        &mut st,
        &mut ps,
        &backend,
        HitNode {
            node_id: window,
            on_titlebar: false,
            is_core: false,
        },
        resize_button_frame(),
    );

    assert!(ps.resize.is_some());
    assert_eq!(st.input.interaction_state.resize_active, Some(window));
}

#[test]
fn smooth_resize_continues_advancing_across_quick_pointer_updates() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut tuning = single_monitor_tiling_tuning();
    tuning.animations.smooth_resize.enabled = true;
    tuning.animations.smooth_resize.duration_ms = 400;
    let mut st = Halley::new_for_test(&dh, tuning);
    let backend = TtyBackendHandle::new(800, 600);

    let window = st.model.field.spawn_surface(
        "window",
        halley_core::field::Vec2 { x: 300.0, y: 220.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(window, "monitor_a");

    let mut ps = PointerState::default();
    begin_resize(
        &mut st,
        &mut ps,
        &backend,
        HitNode {
            node_id: window,
            on_titlebar: false,
            is_core: false,
        },
        resize_button_frame(),
    );

    assert!(handle_resize_motion(
        &mut st, &mut ps, 800, 600, 520.0, 380.0, &backend,
    ));

    let first = ps.resize.expect("resize in progress");
    assert!(
        !resize_rect_nearly_eq(first.preview_right_px, first.target_right_px)
            || !resize_rect_nearly_eq(first.preview_bottom_px, first.target_bottom_px),
        "smooth resize should lag behind the cursor-driven target while dragging"
    );
    let first_preview_right = first.preview_right_px;
    let first_target_right = first.target_right_px;

    let tick_one_at = first.last_smooth_tick_at + Duration::from_millis(16);
    let ticked = advance_resize_anim(&mut st, &mut ps, tick_one_at).expect("resize tick one");
    assert_eq!(ticked, window);

    let after_tick_one = ps.resize.expect("resize after first tick");
    assert!(
        after_tick_one.preview_right_px > first_preview_right + 0.1,
        "preview should move continuously toward the target during drag"
    );
    assert!(after_tick_one.preview_right_px < after_tick_one.target_right_px);

    assert!(handle_resize_motion(
        &mut st, &mut ps, 800, 600, 620.0, 460.0, &backend,
    ));

    let second = ps.resize.expect("resize after second pointer update");
    assert!(
        second.target_right_px > first_target_right,
        "second pointer update should move the target farther out"
    );
    assert!(
        second.preview_right_px >= after_tick_one.preview_right_px - 0.5,
        "preview should not reset or jump backward on rapid pointer updates"
    );

    let tick_two_at = second.last_smooth_tick_at + Duration::from_millis(16);
    let ticked = advance_resize_anim(&mut st, &mut ps, tick_two_at).expect("resize tick two");
    assert_eq!(ticked, window);

    let after_tick_two = ps.resize.expect("resize after second tick");
    assert!(
        after_tick_two.preview_right_px > second.preview_right_px + 0.1,
        "preview should keep advancing instead of hesitating after repeated quick motion"
    );
    assert!(after_tick_two.preview_right_px < after_tick_two.target_right_px);
    let release_preview_right = after_tick_two.preview_right_px;
    let release_target_right = after_tick_two.target_right_px;

    finalize_resize(&mut st, &mut ps, &backend);
    assert!(
        ps.resize.is_none(),
        "resize should stop immediately at release instead of entering a settle animation"
    );
    let final_rect = active_node_screen_rect(&st, 800, 600, window, Instant::now(), None)
        .expect("final window rect");
    assert!(
        final_rect.2 < release_target_right - 8.0,
        "release should not finish the full trajectory to the old cursor target"
    );
    assert!(
        final_rect.2 >= release_preview_right - 4.0,
        "release should stop near the current preview instead of snapping backward"
    );
    assert!(
        st.input.interaction_state.resize_active.is_none(),
        "resize interaction should end immediately on release"
    );
}
