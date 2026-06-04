use std::time::Instant;

use crate::compositor::root::Halley;
use crate::compositor::screenshot::screenshot_controller;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TtyOutputAnimationRedrawState {
    pub active: bool,
    pub force_full_repaint: bool,
}

pub(crate) fn monitor_overlay_requires_full_repaint(st: &Halley, monitor: &str) -> bool {
    monitor_overlay_requires_full_repaint_at(st, monitor, st.now_ms(std::time::Instant::now()))
}

fn monitor_overlay_requires_full_repaint_at(st: &Halley, monitor: &str, now_ms: u64) -> bool {
    if now_ms < st.runtime.screenshot_full_repaint_until_ms {
        return true;
    }
    if st.runtime.tuning.debug.overlay_fps {
        return true;
    }
    st.cluster_mode_active_for_monitor(monitor)
        || st
            .model
            .cluster_state
            .cluster_bloom_open
            .contains_key(monitor)
        || st
            .model
            .cluster_state
            .cluster_overflow_visible_until_ms
            .get(monitor)
            .is_some_and(|visible_until_ms| *visible_until_ms > now_ms)
        || st
            .model
            .cluster_state
            .cluster_overflow_promotion_anim
            .contains_key(monitor)
        || crate::compositor::interaction::state::bloom_pull_preview_active_for_monitor(st, monitor)
        || st
            .ui
            .render_state
            .overlays
            .overlay_banner
            .contains_key(monitor)
        || st
            .ui
            .render_state
            .overlays
            .overlay_toast
            .contains_key(monitor)
        || st
            .model
            .focus_state
            .focus_ring_preview_until_ms
            .get(monitor)
            .is_some_and(|until_ms| *until_ms > now_ms)
        || st.input.interaction_state.focus_cycle_session.is_some()
        || st
            .model
            .cluster_state
            .cluster_name_prompt
            .contains_key(monitor)
        || screenshot_controller(st).screenshot_session_active()
        || st
            .ui
            .render_state
            .overlays
            .overlay_exit_confirm
            .contains_key(monitor)
}

pub(crate) fn tty_output_animation_redraw_state(
    st: &Halley,
    monitor: &str,
    now: Instant,
) -> TtyOutputAnimationRedrawState {
    let now_ms = st.now_ms(now);
    let node_transition_active = st.runtime.tuning.animations_enabled()
        && st.ui.render_state.animator.has_active_animations(now);
    let node_icon_fade_active = node_icon_fade_active_for_monitor(st, monitor, now);
    let active_transition_active = st.runtime.tuning.animations_enabled()
        && st
            .model
            .workspace_state
            .active_transitions
            .values()
            .any(|transition| transition.is_active(now_ms));
    let tiled_insert_reveal_active = st
        .model
        .spawn_state
        .pending_tiled_insert_reveal_at_ms
        .values()
        .any(|&until| until > now_ms);
    let spawn_activation_active = st
        .model
        .spawn_state
        .pending_spawn_activate_at_ms
        .values()
        .any(|&until| until > now_ms);
    let cluster_tile_active = st.runtime.tuning.tile_animation_enabled()
        && crate::animation::cluster_tile_tracks_animating(
            st.ui.render_state.cluster_tile_tracks(),
            now,
        );
    let close_window_active = st.runtime.tuning.window_close_animation_enabled()
        && st
            .ui
            .render_state
            .closing_window_animation_active_for_monitor(monitor, now);
    let stack_cycle_active = st.runtime.tuning.stack_animation_enabled()
        && st
            .ui
            .render_state
            .window_animations
            .stack_cycle_transition
            .get(monitor)
            .is_some_and(|transition| {
                (now.saturating_duration_since(transition.started_at)
                    .as_millis() as u64)
                    < transition.duration_ms
            });
    let raise_animation_active = st.runtime.tuning.raise_animation_enabled()
        && st.ui.render_state.raise_animation_active_for_monitor(
            &st.model.field,
            &st.model.monitor_state.node_monitor,
            monitor,
            now,
        );
    let landmark_slide_active = st.ui.render_state.landmark_slide_active_for_monitor(
        &st.model.field,
        &st.model.monitor_state.node_monitor,
        monitor,
        now,
    );
    let fullscreen_motion_active = !st.model.fullscreen_state.fullscreen_motion.is_empty()
        || !st.model.fullscreen_state.fullscreen_scale_anim.is_empty();
    let maximize_motion_active =
        crate::compositor::workspace::state::maximize_animation_active_for_monitor(st, monitor);
    let current_monitor = st.model.monitor_state.current_monitor.as_str();
    let viewport_pan_active = st
        .input
        .interaction_state
        .viewport_pan_anim
        .as_ref()
        .is_some_and(|anim| anim.monitor == monitor)
        || st
            .model
            .spawn_state
            .pending_spawn_pan_queue
            .iter()
            .any(|pan| {
                st.model
                    .monitor_state
                    .node_monitor
                    .get(&pan.node_id)
                    .is_some_and(|node_monitor| node_monitor == monitor)
            });
    let camera_smoothing_active = monitor == current_monitor
        && ((st.model.viewport.center.x - st.model.camera_target_center.x).abs() > 0.05
            || (st.model.viewport.center.y - st.model.camera_target_center.y).abs() > 0.05
            || (st.model.zoom_ref_size.x - st.model.camera_target_view_size.x).abs() > 0.05
            || (st.model.zoom_ref_size.y - st.model.camera_target_view_size.y).abs() > 0.05);
    let overlay_active = monitor_overlay_requires_full_repaint_at(st, monitor, now_ms)
        || st
            .ui
            .render_state
            .view
            .cluster_bloom_mix
            .get(monitor)
            .is_some_and(|state| state.mix > 0.01)
        || st
            .ui
            .render_state
            .view
            .bearings_mix
            .get(monitor)
            .is_some_and(|mix| *mix > 0.02);
    let fade_related = node_transition_active
        || node_icon_fade_active
        || active_transition_active
        || tiled_insert_reveal_active
        || spawn_activation_active
        || fullscreen_motion_active
        || maximize_motion_active;
    let active = fade_related
        || cluster_tile_active
        || close_window_active
        || stack_cycle_active
        || raise_animation_active
        || landmark_slide_active
        || viewport_pan_active
        || camera_smoothing_active
        || overlay_active;

    TtyOutputAnimationRedrawState {
        active,
        force_full_repaint: active,
    }
}

fn node_icon_fade_active_for_monitor(st: &Halley, monitor: &str, now: Instant) -> bool {
    if !st.runtime.tuning.animations_enabled() {
        return false;
    }

    let fade_until_ms = crate::render::NODE_ICON_FADE_DELAY_MS + crate::render::NODE_ICON_FADE_MS;
    st.model.field.nodes().keys().copied().any(|id| {
        let Some(node) = st.model.field.node(id) else {
            return false;
        };
        if !matches!(
            node.state,
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
        ) || !st.model.field.participates_in_field_view(id)
            || !st.model.field.is_visible(id)
            || !st
                .model
                .monitor_state
                .node_monitor
                .get(&id)
                .is_some_and(|node_monitor| node_monitor == monitor)
        {
            return false;
        }

        st.ui
            .render_state
            .anim_track_elapsed_for(id, node.state.clone(), now)
            .is_some_and(|elapsed| (elapsed.as_millis() as u64) < fade_until_ms)
    })
}
