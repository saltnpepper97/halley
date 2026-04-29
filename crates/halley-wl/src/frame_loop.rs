use std::collections::HashSet;
use std::time::Instant;

use halley_core::field::{NodeId, Vec2};
use smithay::desktop::{PopupKind, find_popup_root_surface};
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
use smithay::wayland::compositor::{
    SurfaceAttributes, TraversalAction, with_surface_tree_downward,
};

use crate::animation::AnimStyle;
use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::root::Halley;
use crate::compositor::screenshot::screenshot_controller;

#[cfg(test)]
use crate::window::ActiveBorderRect;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TtyOutputAnimationRedrawState {
    pub active: bool,
    pub force_full_repaint: bool,
}

pub(crate) fn monitor_overlay_requires_full_repaint(st: &Halley, monitor: &str) -> bool {
    if st.now_ms(std::time::Instant::now()) < st.runtime.screenshot_full_repaint_until_ms {
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
            .is_some_and(|visible_until_ms| {
                *visible_until_ms > st.now_ms(std::time::Instant::now())
            })
        || st
            .model
            .cluster_state
            .cluster_overflow_promotion_anim
            .contains_key(monitor)
        || crate::compositor::interaction::state::bloom_pull_preview_active_for_monitor(st, monitor)
        || st.ui.render_state.overlay_banner.contains_key(monitor)
        || st.ui.render_state.overlay_toast.contains_key(monitor)
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
    let active_transition_active = st.runtime.tuning.animations_enabled()
        && st
            .model
            .workspace_state
            .active_transition_until_ms
            .values()
            .any(|&until| until > now_ms);
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
            &st.ui.render_state.cluster_tile_tracks,
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
            .stack_cycle_transition
            .get(monitor)
            .is_some_and(|transition| {
                (now.saturating_duration_since(transition.started_at)
                    .as_millis() as u64)
                    < transition.duration_ms
            });
    let fullscreen_motion_active = !st.model.fullscreen_state.fullscreen_motion.is_empty()
        || !st.model.fullscreen_state.fullscreen_scale_anim.is_empty();
    let maximize_motion_active = crate::compositor::workspace::state::maximize_animation_active(st);
    let current_monitor = st.model.monitor_state.current_monitor.as_str();
    let viewport_pan_active = monitor == current_monitor
        && (st.input.interaction_state.viewport_pan_anim.is_some()
            || !st.model.spawn_state.pending_spawn_pan_queue.is_empty());
    let camera_smoothing_active = monitor == current_monitor
        && ((st.model.viewport.center.x - st.model.camera_target_center.x).abs() > 0.05
            || (st.model.viewport.center.y - st.model.camera_target_center.y).abs() > 0.05
            || (st.model.zoom_ref_size.x - st.model.camera_target_view_size.x).abs() > 0.05
            || (st.model.zoom_ref_size.y - st.model.camera_target_view_size.y).abs() > 0.05);
    let overlay_active = monitor_overlay_requires_full_repaint(st, monitor)
        || st
            .ui
            .render_state
            .cluster_bloom_mix
            .get(monitor)
            .is_some_and(|state| state.mix > 0.01)
        || st
            .ui
            .render_state
            .bearings_mix
            .get(monitor)
            .is_some_and(|mix| *mix > 0.02);
    let fade_related = node_transition_active
        || active_transition_active
        || tiled_insert_reveal_active
        || spawn_activation_active
        || fullscreen_motion_active
        || maximize_motion_active;
    let active = fade_related
        || cluster_tile_active
        || close_window_active
        || stack_cycle_active
        || viewport_pan_active
        || camera_smoothing_active
        || overlay_active;

    TtyOutputAnimationRedrawState {
        active,
        force_full_repaint: active,
    }
}

pub(crate) fn begin_render_frame(st: &mut Halley, now: Instant) {
    st.ui.render_state.render_last_tick = now;
    st.platform.popup_manager.cleanup();
    let alive: HashSet<NodeId> = st.model.field.node_ids_all().into_iter().collect();
    st.input
        .interaction_state
        .physics_velocity
        .retain(|id, _| alive.contains(id));
    st.input
        .interaction_state
        .smoothed_render_pos
        .retain(|id, _| alive.contains(id));
    st.ui
        .render_state
        .node_hover_mix
        .retain(|id, _| alive.contains(id));
    st.ui.render_state.node_preview_hover.retain(|_, state| {
        state.node = state.node.filter(|id| alive.contains(id));
        state.node.is_some() || state.mix > 0.002
    });
    st.ui.render_state.bearings_mix.retain(|monitor, mix| {
        st.model.monitor_state.monitors.contains_key(monitor) || *mix > 0.002
    });
    st.ui
        .render_state
        .cluster_bloom_mix
        .retain(|monitor, state| {
            st.model.monitor_state.monitors.contains_key(monitor) || state.mix > 0.002
        });
    st.ui
        .render_state
        .cluster_tile_entry_pending
        .retain(|id| alive.contains(id));
    st.ui
        .render_state
        .cluster_tile_frozen_geometry
        .retain(|id, _| {
            alive.contains(id) && st.ui.render_state.cluster_tile_tracks.contains_key(id)
        });
    st.ui.render_state.prune_window_offscreen_cache(&alive, now);
    st.ui.render_state.prune_ui_text_cache(now);
}

pub(crate) fn anim_style_for(
    st: &Halley,
    id: NodeId,
    state: halley_core::field::NodeState,
    now: Instant,
) -> AnimStyle {
    if !st.runtime.tuning.animations_enabled() {
        return AnimStyle::default();
    }

    let now_ms = st.now_ms(now);
    if st.input.interaction_state.resize_active == Some(id)
        || (st.input.interaction_state.resize_static_node == Some(id)
            && now_ms < st.input.interaction_state.resize_static_until_ms)
    {
        return AnimStyle::default();
    }

    st.ui.render_state.animator.style_for(id, state, now)
}

pub(crate) fn tick_animator_frame(st: &mut Halley, now: Instant) {
    if !st.runtime.tuning.animations_enabled() {
        return;
    }
    st.ui.render_state.tick_animator_frame(&st.model.field, now);
}

pub(crate) fn tick_frame_effects(st: &mut Halley, now: Instant) {
    let now_ms = st.now_ms(now);
    st.tick_viewport_pan_animation(now_ms);
    let _ = st.process_pending_cluster_slot_transition_for_current_monitor(now);
    st.tick_pending_spawn_pan(now, now_ms);
    crate::compositor::workspace::state::tick_maximize_animation(st, now);
    tick_active_drag(st, now);
    crate::compositor::interaction::state::tick_cluster_join_candidate_ready(st, now_ms);
    crate::compositor::interaction::state::tick_bloom_pull_preview(st, now_ms);
    tick_pending_core_hover_bloom(st, now_ms);
    camera_controller(&mut *st).tick_smoothing(now);
}

#[inline]
fn drag_edge_pan_screen_speed_pxps(cam_scale: f32) -> f32 {
    const DRAG_EDGE_PAN_BASE_SPEED_PXPS: f32 = 240.0;

    // Keep zoomed-out edge pan at the same feel, but slow it down further as
    // the camera zooms in so tighter views don't move as aggressively.
    DRAG_EDGE_PAN_BASE_SPEED_PXPS * cam_scale.recip().clamp(0.0, 1.0)
}

#[inline]
fn drag_edge_pan_pressure_multiplier(pressure: f32) -> f32 {
    const EDGE_PAN_PRESSURE_THRESHOLD: f32 = 56.0;
    const EDGE_PAN_PRESSURE_FULL_SPEED: f32 = EDGE_PAN_PRESSURE_THRESHOLD + 120.0;
    const EDGE_PAN_MAX_BOOST: f32 = 0.24;

    let t = ((pressure - EDGE_PAN_PRESSURE_THRESHOLD)
        / (EDGE_PAN_PRESSURE_FULL_SPEED - EDGE_PAN_PRESSURE_THRESHOLD))
        .clamp(0.0, 1.0);
    let eased = t * t * (3.0 - 2.0 * t);
    1.0 + eased * EDGE_PAN_MAX_BOOST
}

fn tick_pending_core_hover_bloom(st: &mut Halley, now_ms: u64) {
    let Some(pending_hover) = st.input.interaction_state.pending_core_hover.clone() else {
        return;
    };
    if now_ms
        < pending_hover
            .started_at_ms
            .saturating_add(crate::compositor::interaction::CORE_BLOOM_HOLD_MS)
    {
        return;
    }

    st.input.interaction_state.pending_core_hover = None;
    if let Some(cid) = st
        .model
        .field
        .cluster_id_for_core_public(pending_hover.node_id)
        && st.cluster_bloom_for_monitor(pending_hover.monitor.as_str()) != Some(cid)
    {
        st.input.interaction_state.overlay_hover_target = None;
        let _ = st.open_cluster_bloom_for_monitor(pending_hover.monitor.as_str(), cid);
    }
}

fn tick_active_drag(st: &mut Halley, now: Instant) {
    let Some(mut active_drag) = st.input.interaction_state.active_drag.clone() else {
        crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
        return;
    };

    let Some(node_id) = st.input.interaction_state.drag_authority_node else {
        st.input.interaction_state.active_drag = None;
        return;
    };
    if node_id != active_drag.node_id {
        st.input.interaction_state.active_drag = None;
        crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
        return;
    }
    let pointer_world = crate::spatial::screen_to_world(
        st,
        active_drag.pointer_workspace_size.0,
        active_drag.pointer_workspace_size.1,
        active_drag.pointer_screen_local.0,
        active_drag.pointer_screen_local.1,
    );
    let desired_to = Vec2 {
        x: pointer_world.x - active_drag.current_offset.x,
        y: pointer_world.y - active_drag.current_offset.y,
    };

    let moved = if active_drag.allow_monitor_transfer {
        crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
        st.assign_node_to_monitor(node_id, active_drag.pointer_monitor.as_str());
        let to = crate::compositor::interaction::state::dragged_node_cluster_core_clamp(
            st,
            active_drag.pointer_monitor.as_str(),
            node_id,
            desired_to,
        )
        .and_then(|(clamped, cid, _)| {
            (st.cluster_bloom_for_monitor(active_drag.pointer_monitor.as_str()) == Some(cid))
                .then_some(clamped)
        })
        .unwrap_or(desired_to);
        st.carry_surface_non_overlap(node_id, to, false)
    } else if !active_drag.edge_pan_eligible {
        crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
        let to = crate::compositor::interaction::state::dragged_node_cluster_core_clamp(
            st,
            active_drag.pointer_monitor.as_str(),
            node_id,
            desired_to,
        )
        .and_then(|(clamped, cid, _)| {
            (st.cluster_bloom_for_monitor(active_drag.pointer_monitor.as_str()) == Some(cid))
                .then_some(clamped)
        })
        .unwrap_or(desired_to);
        st.carry_surface_non_overlap(node_id, to, false)
    } else if let Some((clamped_center, edge_contact)) =
        crate::compositor::interaction::state::dragged_node_edge_pan_clamp(
            st,
            active_drag.pointer_monitor.as_str(),
            node_id,
            desired_to,
            Vec2 {
                x: active_drag.edge_pan_x.sign(),
                y: active_drag.edge_pan_y.sign(),
            },
        )
    {
        if active_drag.edge_pan_x.sign() != 0.0 && edge_contact.x != active_drag.edge_pan_x.sign() {
            active_drag.edge_pan_x = crate::compositor::interaction::DragAxisMode::Free;
        }
        if active_drag.edge_pan_y.sign() != 0.0 && edge_contact.y != active_drag.edge_pan_y.sign() {
            active_drag.edge_pan_y = crate::compositor::interaction::DragAxisMode::Free;
        }

        let direction = Vec2 {
            x: active_drag.edge_pan_x.sign(),
            y: active_drag.edge_pan_y.sign(),
        };
        let edge_pan_active = direction.x != 0.0 || direction.y != 0.0;
        st.input.interaction_state.grabbed_edge_pan_active = edge_pan_active;
        st.input.interaction_state.grabbed_edge_pan_direction = direction;
        st.input.interaction_state.grabbed_edge_pan_monitor =
            edge_pan_active.then(|| active_drag.pointer_monitor.clone());

        let to = clamped_center;
        if edge_pan_active {
            let dt = now
                .saturating_duration_since(active_drag.last_edge_pan_at)
                .as_secs_f32()
                .clamp(1.0 / 240.0, 1.0 / 30.0);
            let cam_scale = st.camera_render_scale().max(0.001);
            let edge_pan_speed_pxps = drag_edge_pan_screen_speed_pxps(cam_scale);
            let pan_delta = Vec2 {
                x: direction.x
                    * (edge_pan_speed_pxps / cam_scale)
                    * drag_edge_pan_pressure_multiplier(
                        st.input.interaction_state.grabbed_edge_pan_pressure.x,
                    )
                    * dt,
                y: direction.y
                    * (edge_pan_speed_pxps / cam_scale)
                    * drag_edge_pan_pressure_multiplier(
                        st.input.interaction_state.grabbed_edge_pan_pressure.y,
                    )
                    * dt,
            };
            st.note_pan_activity(now);
            camera_controller(&mut *st).pan_target(pan_delta);
            st.note_pan_viewport_change(now);
        }
        active_drag.last_edge_pan_at = now;
        let drag_monitor = active_drag.pointer_monitor.clone();
        st.input.interaction_state.active_drag = Some(active_drag.clone());
        let to = crate::compositor::interaction::state::dragged_node_cluster_core_clamp(
            st,
            drag_monitor.as_str(),
            node_id,
            to,
        )
        .and_then(|(clamped, cid, _)| {
            (st.cluster_bloom_for_monitor(drag_monitor.as_str()) == Some(cid)).then_some(clamped)
        })
        .unwrap_or(to);
        st.carry_surface_non_overlap(node_id, to, false)
    } else {
        st.input.interaction_state.active_drag = None;
        crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
        return;
    };
    let live_reordered = if st.model.field.is_active_cluster_member(node_id) {
        st.move_active_cluster_member_to_drop_tile(
            active_drag.pointer_monitor.as_str(),
            node_id,
            pointer_world,
            st.now_ms(now),
        )
    } else {
        false
    };
    if moved || live_reordered {
        st.request_maintenance();
    }
}

pub(crate) fn tick_live_overlap(st: &mut Halley) {
    if st.input.interaction_state.suspend_state_checks
        || st.input.interaction_state.resize_active.is_some()
        || crate::compositor::workspace::state::maximize_animation_active(st)
        || !st.model.fullscreen_state.fullscreen_motion.is_empty()
        || !st.model.fullscreen_state.fullscreen_scale_anim.is_empty()
    {
        return;
    }
    st.resolve_surface_overlap();
}

pub(crate) fn send_frame_callbacks(st: &mut Halley, now: Instant) {
    let elapsed_ms = now.duration_since(st.runtime.started_at).as_millis();
    let time_ms = elapsed_ms.min(u32::MAX as u128) as u32;
    for layer in st.platform.wlr_layer_shell_state.layer_surfaces() {
        send_frames_surface_tree(layer.wl_surface(), time_ms);
    }
    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        send_frames_surface_tree(top.wl_surface(), time_ms);
    }
    for popup in st.platform.xdg_shell_state.popup_surfaces() {
        send_frames_surface_tree(popup.wl_surface(), time_ms);
    }
}

pub(crate) fn send_frame_callbacks_for_output(st: &mut Halley, output_name: &str, now: Instant) {
    let elapsed_ms = now.duration_since(st.runtime.started_at).as_millis();
    let time_ms = elapsed_ms.min(u32::MAX as u128) as u32;

    for layer in st.platform.wlr_layer_shell_state.layer_surfaces() {
        let surface = layer.wl_surface();
        if surface_on_output(st, surface, output_name) {
            send_frames_surface_tree(surface, time_ms);
        }
    }

    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let surface = top.wl_surface();
        if surface_on_output(st, surface, output_name) {
            send_frames_surface_tree(surface, time_ms);
        }
    }

    for popup in st.platform.xdg_shell_state.popup_surfaces() {
        let popup_kind = PopupKind::from(popup.clone());
        let Ok(root) = find_popup_root_surface(&popup_kind) else {
            continue;
        };
        if surface_on_output(st, &root, output_name) {
            send_frames_surface_tree(popup.wl_surface(), time_ms);
        }
    }
}

fn surface_on_output(st: &Halley, surface: &WlSurface, output_name: &str) -> bool {
    if let Some(node_id) = st.model.surface_to_node.get(&surface.id()).copied() {
        return st
            .model
            .monitor_state
            .node_monitor
            .get(&node_id)
            .is_some_and(|monitor| monitor == output_name);
    }

    st.model
        .monitor_state
        .layer_surface_monitor
        .get(&surface.id())
        .is_some_and(|monitor| monitor == output_name)
}

fn send_frames_surface_tree(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    time_ms: u32,
) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time_ms);
            }
        },
        |_, _, &()| true,
    );
}

#[cfg(test)]
mod tests {
    use halley_core::viewport::FocusRing;
    use smithay::backend::renderer::Color32F;
    use smithay::utils::{Physical, Size};

    use super::*;

    fn focus_ring_screen_radii(
        view_size: Vec2,
        output_size: Size<i32, Physical>,
        focus_ring: FocusRing,
    ) -> (f32, f32) {
        let px_per_world_x = output_size.w as f32 / view_size.x.max(1.0);
        let px_per_world_y = output_size.h as f32 / view_size.y.max(1.0);
        (
            focus_ring.radius_x * px_per_world_x,
            focus_ring.radius_y * px_per_world_y,
        )
    }

    fn multi_monitor_state() -> Halley {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];

        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        Halley::new_for_test(&dh, tuning)
    }

    #[test]
    fn camera_smoothing_only_marks_current_monitor_active() {
        let mut state = multi_monitor_state();
        let _ = state.activate_monitor("right");

        state.model.camera_target_center.x += 240.0;

        let now = Instant::now();
        assert!(tty_output_animation_redraw_state(&state, "right", now).active);
        assert!(!tty_output_animation_redraw_state(&state, "left", now).active);
    }

    #[test]
    fn viewport_pan_only_marks_current_monitor_active() {
        let mut state = multi_monitor_state();
        let _ = state.activate_monitor("right");
        state.input.interaction_state.viewport_pan_anim =
            Some(crate::compositor::interaction::state::ViewportPanAnim {
                start_ms: 0,
                delay_ms: 0,
                duration_ms: 120,
                from_center: state.model.viewport.center,
                to_center: Vec2 {
                    x: state.model.viewport.center.x + 100.0,
                    y: state.model.viewport.center.y,
                },
            });

        let now = Instant::now();
        assert!(tty_output_animation_redraw_state(&state, "right", now).active);
        assert!(!tty_output_animation_redraw_state(&state, "left", now).active);
    }

    #[test]
    fn closing_window_animation_only_marks_target_monitor_active() {
        let mut state = multi_monitor_state();
        let start = Instant::now();

        state.ui.render_state.start_closing_window_animation(
            NodeId::new(77),
            "right",
            start,
            250,
            halley_config::WindowCloseAnimationStyle::Shrink,
            vec![ActiveBorderRect {
                x: 100,
                y: 100,
                w: 300,
                h: 220,
                inner_offset_x: 3.0,
                inner_offset_y: 3.0,
                inner_w: 300.0,
                inner_h: 220.0,
                alpha: 1.0,
                border_px: 3.0,
                corner_radius: 0.0,
                inner_corner_radius: 0.0,
                border_color: Color32F::new(1.0, 1.0, 1.0, 1.0),
            }],
            Vec::new(),
        );

        assert!(tty_output_animation_redraw_state(&state, "right", start).active);
        assert!(!tty_output_animation_redraw_state(&state, "left", start).active);
        assert!(
            !tty_output_animation_redraw_state(
                &state,
                "right",
                start + std::time::Duration::from_millis(300)
            )
            .active
        );
    }

    #[test]
    fn closing_node_animation_only_marks_target_monitor_active() {
        let mut state = multi_monitor_state();
        let start = Instant::now();

        state.ui.render_state.start_closing_node_animation(
            NodeId::new(78),
            "right",
            start,
            250,
            Vec2 { x: 100.0, y: 120.0 },
            "node".to_string(),
            halley_core::field::NodeState::Node,
        );

        assert!(tty_output_animation_redraw_state(&state, "right", start).active);
        assert!(!tty_output_animation_redraw_state(&state, "left", start).active);
        assert!(
            !tty_output_animation_redraw_state(
                &state,
                "right",
                start + std::time::Duration::from_millis(300)
            )
            .active
        );
    }

    #[test]
    fn focus_ring_preview_radii_follow_zoomed_camera_view() {
        let focus_ring = FocusRing::new(200.0, 100.0, 0.0, 0.0);
        let output_size = Size::<i32, Physical>::from((1920, 1080));

        let (screen_rx, screen_ry) = focus_ring_screen_radii(
            Vec2 {
                x: 3840.0,
                y: 2160.0,
            },
            output_size,
            focus_ring,
        );

        assert_eq!(screen_rx, 100.0);
        assert_eq!(screen_ry, 50.0);
    }

    #[test]
    fn animations_continue_when_physics_is_disabled() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.physics_enabled = false;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let id = state.model.field.spawn_surface(
            "anim",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        let start = Instant::now();

        state
            .ui
            .render_state
            .animator
            .observe_field(&state.model.field, start);
        let _ = state
            .model
            .field
            .set_state(id, halley_core::field::NodeState::Node);
        tick_animator_frame(&mut state, start + std::time::Duration::from_millis(16));

        let anim = anim_style_for(
            &state,
            id,
            halley_core::field::NodeState::Node,
            start + std::time::Duration::from_millis(32),
        );
        assert!(
            anim.scale < 1.0,
            "node transition animation should still run when physics is disabled: {anim:?}"
        );

        crate::compositor::workspace::state::mark_active_transition(&mut state, id, start, 620);
        assert!(
            crate::compositor::workspace::state::active_transition_alpha(
                &state,
                id,
                start + std::time::Duration::from_millis(32),
            ) > 0.0,
            "active transition alpha should still be tracked when physics is disabled"
        );
    }

    #[test]
    fn edge_pan_screen_speed_slows_down_when_zoomed_in() {
        assert!((drag_edge_pan_screen_speed_pxps(0.5) - 240.0).abs() < 0.01);
        assert!((drag_edge_pan_screen_speed_pxps(1.0) - 240.0).abs() < 0.01);
        assert!((drag_edge_pan_screen_speed_pxps(1.25) - 192.0).abs() < 0.01);
    }

    #[test]
    fn edge_pan_pressure_multiplier_is_smooth_and_low_ceiling() {
        assert!((drag_edge_pan_pressure_multiplier(0.0) - 1.0).abs() < 0.01);
        assert!((drag_edge_pan_pressure_multiplier(56.0) - 1.0).abs() < 0.01);
        assert!(drag_edge_pan_pressure_multiplier(96.0) > 1.05);
        assert!(drag_edge_pan_pressure_multiplier(96.0) < 1.07);
        assert!(drag_edge_pan_pressure_multiplier(196.0) <= 1.24);
    }
}
