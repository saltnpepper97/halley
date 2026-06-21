use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::time::{Duration, Instant};

use halley_core::field::{NodeId, Vec2};
use smithay::backend::renderer::element::RenderElementStates;
use smithay::desktop::utils::{
    OutputPresentationFeedback, bbox_from_surface_tree,
    surface_presentation_feedback_flags_from_states, take_presentation_feedback_surface_tree,
};
use smithay::desktop::{PopupKind, find_popup_root_surface};
use smithay::input::pointer::CursorImageStatus;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
use smithay::utils::{Clock, IsAlive, Logical, Monotonic, Point, Rectangle};
use smithay::wayland::compositor::{
    SurfaceAttributes, SurfaceData, TraversalAction, with_surface_tree_downward,
};
use smithay::wayland::presentation::Refresh;

use crate::animation::AnimStyle;
use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::root::Halley;

mod activity;

pub(crate) use activity::{
    monitor_overlay_requires_full_repaint, tty_output_animation_redraw_state,
};

#[derive(Default)]
struct SurfaceFrameCallbackThrottle {
    last_sent_at: RefCell<Option<(String, u32)>>,
}

#[cfg(test)]
use crate::window::ActiveBorderRect;

pub(crate) fn begin_render_frame(st: &mut Halley, now: Instant) {
    st.ui.render_state.set_render_last_tick(now);
    st.platform.popup_manager.cleanup();
    let alive: HashSet<NodeId> = st.model.field.node_ids_all().into_iter().collect();
    let live_monitors: HashSet<String> = st.model.monitor_state.monitors.keys().cloned().collect();
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
        .retain_node_hover_mix(|id, _| alive.contains(id));
    st.ui.render_state.retain_node_preview_hover(|_, state| {
        state.node = state.node.filter(|id| alive.contains(id));
        state.node.is_some() || state.mix > 0.002
    });
    st.ui
        .render_state
        .retain_bearings_mix(|monitor, mix| live_monitors.contains(monitor) || *mix > 0.002);
    st.ui
        .render_state
        .retain_cluster_bloom_mix(|monitor, state| {
            live_monitors.contains(monitor) || state.mix > 0.002
        });
    st.ui
        .render_state
        .retain_cluster_tile_entry_pending(|id| alive.contains(id));
    let live_cluster_tile_tracks: HashSet<NodeId> = st
        .ui
        .render_state
        .cluster_tile_tracks()
        .keys()
        .copied()
        .collect();
    st.ui
        .render_state
        .retain_cluster_tile_frozen_geometry(|id, _| {
            alive.contains(id) && live_cluster_tile_tracks.contains(id)
        });
    // Keep every cluster member's offscreen texture warm (exempt from the idle
    // TTL) so re-opening a collapsed cluster later never rebuilds its textures.
    // Also keep the alt+tab switcher's on-screen preview cards warm for the life
    // of the session so their captured textures aren't pruned mid-cycle.
    let mut keep_warm: HashSet<NodeId> = st
        .model
        .field
        .clusters_iter()
        .flat_map(|cluster| cluster.members().iter().copied())
        .collect();
    if let Some(session) = st.input.interaction_state.focus_cycle_session.as_ref() {
        keep_warm.extend(
            session
                .visible_slots(crate::overlay::FOCUS_CYCLE_VISIBLE_RADIUS)
                .into_iter()
                .map(|(_, node_id)| node_id),
        );
    }
    if let Some(session) = st.input.interaction_state.apogee_session.as_ref() {
        keep_warm.extend(
            session
                .monitors
                .iter()
                .flat_map(|monitor_session| monitor_session.tiles.iter().map(|tile| tile.node_id)),
        );
    }
    st.ui
        .render_state
        .prune_window_offscreen_cache(&alive, &keep_warm, now);
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
    crate::compositor::actions::window::tick_pending_maximize(st, now);
    let _ = st.process_pending_cluster_slot_transition_for_current_monitor(now);
    st.tick_pending_spawn_pan(now, now_ms);
    crate::compositor::workspace::state::tick_maximize_animation(st, now);
    tick_active_drag(st, now);
    crate::compositor::focus::cycle::tick_focus_cycle_session(st, now);
    crate::compositor::interaction::state::tick_cluster_join_candidate_ready(st, now_ms);
    crate::compositor::interaction::state::tick_bloom_pull_preview(st, now_ms);
    tick_pending_core_hover_bloom(st, now_ms);
    st.tick_apogee(now);
    camera_controller(&mut *st).tick_smoothing(now);

    // Also ease the cameras of the other (non-active) monitors so a monitor that
    // is mid-zoom keeps settling into place instead of freezing the moment the
    // pointer crosses to another monitor (and resuming only when it returns).
    // Both monitors are physically visible, so each must keep animating + repainting
    // until it reaches its target.
    let current = st.model.monitor_state.current_monitor.clone();
    let others: Vec<String> = st
        .model
        .monitor_state
        .monitors
        .keys()
        .filter(|name| **name != current)
        .cloned()
        .collect();
    for name in others {
        let previous = st.begin_temporary_render_monitor(&name);
        if previous.is_some() {
            let moved = camera_controller(&mut *st).tick_smoothing_passive(now);
            if moved {
                st.request_tty_redraw_for_monitor(&name);
            }
        }
        st.end_temporary_render_monitor(previous);
    }
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
    let Some(active_drag) = st.input.interaction_state.active_drag.clone() else {
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
        let drag_monitor = active_drag.pointer_monitor.clone();
        let to = crate::compositor::interaction::state::dragged_node_cluster_core_clamp(
            st,
            drag_monitor.as_str(),
            node_id,
            desired_to,
        )
        .and_then(|(clamped, cid, _)| {
            (st.cluster_bloom_for_monitor(drag_monitor.as_str()) == Some(cid)).then_some(clamped)
        })
        .unwrap_or(desired_to);
        let source_monitor = st.model.monitor_state.node_monitor.get(&node_id).cloned();
        let monitor_changed = source_monitor.as_deref() != Some(drag_monitor.as_str());
        let floats_over_cluster =
            crate::compositor::spawn::state::node_floats_over_cluster(st, node_id);
        if monitor_changed
            && let Some(cid) = st.model.field.cluster_id_for_member_public(node_id)
            && source_monitor.as_deref().is_some_and(|monitor| {
                st.active_cluster_workspace_for_monitor(monitor) == Some(cid)
            })
            && let Some(pos) = st.model.field.node(node_id).map(|node| node.pos)
        {
            let _ = st.detach_member_from_cluster(cid, node_id, pos, now);
        }
        st.assign_node_to_monitor(node_id, drag_monitor.as_str());
        let absorbed_into_stack = if monitor_changed
            && !floats_over_cluster
            && st
                .model
                .field
                .cluster_id_for_member_public(node_id)
                .is_none()
            && let Some(cid) = st.active_cluster_workspace_for_monitor(drag_monitor.as_str())
        {
            st.absorb_node_into_cluster(cid, node_id, now)
                && matches!(
                    st.runtime.tuning.cluster_layout_kind(),
                    halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Stacking
                )
        } else {
            false
        };
        if absorbed_into_stack {
            true
        } else {
            st.carry_surface_non_overlap(node_id, to, false)
        }
    } else if !active_drag.edge_pan_eligible {
        crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
        let drag_monitor = active_drag.pointer_monitor.clone();
        let to = crate::compositor::interaction::state::dragged_node_cluster_core_clamp(
            st,
            drag_monitor.as_str(),
            node_id,
            desired_to,
        )
        .and_then(|(clamped, cid, _)| {
            (st.cluster_bloom_for_monitor(drag_monitor.as_str()) == Some(cid)).then_some(clamped)
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
        let mut active_drag = active_drag.clone();
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
    if let CursorImageStatus::Surface(surface) = st.platform.cursor_manager.cursor_image()
        && surface.alive()
    {
        send_frames_surface_tree(surface, time_ms);
    }
}

pub(crate) fn send_frame_callbacks_for_output(st: &mut Halley, output_name: &str, now: Instant) {
    let elapsed_ms = now.duration_since(st.runtime.started_at).as_millis();
    let time_ms = elapsed_ms.min(u32::MAX as u128) as u32;
    let sequence = st.tty_frame_callback_sequence(output_name);

    for layer in st.platform.wlr_layer_shell_state.layer_surfaces() {
        let surface = layer.wl_surface();
        if surface_on_output(st, surface, output_name) {
            send_frames_surface_tree_for_output(surface, time_ms, output_name, sequence);
        }
    }

    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let surface = top.wl_surface();
        if surface_on_output(st, surface, output_name) {
            send_frames_surface_tree_for_output(surface, time_ms, output_name, sequence);
        }
    }

    for popup in st.platform.xdg_shell_state.popup_surfaces() {
        let popup_kind = PopupKind::from(popup.clone());
        let Ok(root) = find_popup_root_surface(&popup_kind) else {
            continue;
        };
        if surface_on_output(st, &root, output_name) {
            send_frames_surface_tree_for_output(popup.wl_surface(), time_ms, output_name, sequence);
        }
    }

    if let CursorImageStatus::Surface(surface) = st.platform.cursor_manager.cursor_image()
        && surface.alive()
        && cursor_surface_on_output(st, surface, output_name)
    {
        send_frames_surface_tree_for_output(surface, time_ms, output_name, sequence);
    }
}

pub(crate) fn output_has_pending_frame_callbacks(st: &Halley, output_name: &str) -> bool {
    st.platform
        .wlr_layer_shell_state
        .layer_surfaces()
        .any(|layer| {
            let surface = layer.wl_surface();
            surface_on_output(st, surface, output_name)
                && surface_tree_has_pending_frame_callbacks(surface)
        })
        || st
            .platform
            .xdg_shell_state
            .toplevel_surfaces()
            .iter()
            .any(|top| {
                let surface = top.wl_surface();
                surface_frame_callback_relevant_on_output(st, surface, output_name)
                    && surface_tree_has_pending_frame_callbacks(surface)
            })
        || st
            .platform
            .xdg_shell_state
            .popup_surfaces()
            .iter()
            .any(|popup| {
                let popup_kind = PopupKind::from(popup.clone());
                let Ok(root) = find_popup_root_surface(&popup_kind) else {
                    return false;
                };
                surface_frame_callback_relevant_on_output(st, &root, output_name)
                    && surface_tree_has_pending_frame_callbacks(popup.wl_surface())
            })
        || matches!(st.platform.cursor_manager.cursor_image(), CursorImageStatus::Surface(surface) if surface.alive()
            && cursor_surface_on_output(st, surface, output_name)
            && surface_tree_has_pending_frame_callbacks(surface))
}

pub(crate) fn take_presentation_feedback_for_output(
    st: &Halley,
    output_name: &str,
) -> Option<OutputPresentationFeedback> {
    let Some(output) = st.model.monitor_state.outputs.get(output_name).cloned() else {
        return None;
    };

    let mut feedback = OutputPresentationFeedback::new(&output);

    for layer in st.platform.wlr_layer_shell_state.layer_surfaces() {
        let surface = layer.wl_surface();
        if surface_on_output(st, surface, output_name) {
            take_presentation_feedback_surface_tree(
                surface,
                &mut feedback,
                |_, _| Some(output.clone()),
                |_, _| wp_presentation_feedback::Kind::empty(),
            );
        }
    }

    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let surface = top.wl_surface();
        if surface_on_output(st, surface, output_name) {
            take_presentation_feedback_surface_tree(
                surface,
                &mut feedback,
                |_, _| Some(output.clone()),
                |_, _| wp_presentation_feedback::Kind::empty(),
            );
        }
    }

    for popup in st.platform.xdg_shell_state.popup_surfaces() {
        let popup_kind = PopupKind::from(popup.clone());
        let Ok(root) = find_popup_root_surface(&popup_kind) else {
            continue;
        };
        if surface_on_output(st, &root, output_name) {
            take_presentation_feedback_surface_tree(
                popup.wl_surface(),
                &mut feedback,
                |_, _| Some(output.clone()),
                |_, _| wp_presentation_feedback::Kind::empty(),
            );
        }
    }

    if let CursorImageStatus::Surface(surface) = st.platform.cursor_manager.cursor_image()
        && surface.alive()
        && cursor_surface_on_output(st, surface, output_name)
    {
        take_presentation_feedback_surface_tree(
            surface,
            &mut feedback,
            |_, _| Some(output.clone()),
            |_, _| wp_presentation_feedback::Kind::empty(),
        );
    }

    Some(feedback)
}

pub(crate) fn take_presentation_feedback_for_output_with_states(
    st: &Halley,
    output_name: &str,
    render_element_states: &RenderElementStates,
) -> Option<OutputPresentationFeedback> {
    let output = st.model.monitor_state.outputs.get(output_name).cloned()?;
    let mut feedback = OutputPresentationFeedback::new(&output);

    let primary_output = |surface: &WlSurface, _states: &SurfaceData| {
        render_element_states
            .element_was_presented(surface)
            .then(|| output.clone())
    };
    let feedback_flags = |surface: &WlSurface, _states: &SurfaceData| {
        surface_presentation_feedback_flags_from_states(surface, None, render_element_states)
    };

    for layer in st.platform.wlr_layer_shell_state.layer_surfaces() {
        let surface = layer.wl_surface();
        if surface_on_output(st, surface, output_name) {
            take_presentation_feedback_surface_tree(
                surface,
                &mut feedback,
                primary_output,
                feedback_flags,
            );
        }
    }

    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let surface = top.wl_surface();
        if surface_on_output(st, surface, output_name) {
            take_presentation_feedback_surface_tree(
                surface,
                &mut feedback,
                primary_output,
                feedback_flags,
            );
        }
    }

    for popup in st.platform.xdg_shell_state.popup_surfaces() {
        let popup_kind = PopupKind::from(popup.clone());
        let Ok(root) = find_popup_root_surface(&popup_kind) else {
            continue;
        };
        if surface_on_output(st, &root, output_name) {
            take_presentation_feedback_surface_tree(
                popup.wl_surface(),
                &mut feedback,
                primary_output,
                feedback_flags,
            );
        }
    }

    if let CursorImageStatus::Surface(surface) = st.platform.cursor_manager.cursor_image()
        && surface.alive()
        && cursor_surface_on_output(st, surface, output_name)
    {
        take_presentation_feedback_surface_tree(
            surface,
            &mut feedback,
            primary_output,
            feedback_flags,
        );
    }

    Some(feedback)
}

pub(crate) fn send_presentation_feedback_for_output(st: &Halley, output_name: &str) {
    let Some(mut feedback) = take_presentation_feedback_for_output(st, output_name) else {
        return;
    };
    let Some(output) = st.model.monitor_state.outputs.get(output_name) else {
        return;
    };
    let presentation_time = Clock::<Monotonic>::new().now();
    let refresh = refresh_for_output(output);
    feedback.presented(
        presentation_time,
        refresh,
        0,
        wp_presentation_feedback::Kind::Vsync,
    );
}

fn refresh_for_output(output: &Output) -> Refresh {
    output
        .current_mode()
        .map(|mode| mode.refresh)
        .filter(|refresh_millihz| *refresh_millihz > 0)
        .map(|refresh_millihz| {
            Refresh::fixed(Duration::from_nanos(
                1_000_000_000_000u64 / refresh_millihz as u64,
            ))
        })
        .unwrap_or(Refresh::Unknown)
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

fn surface_frame_callback_relevant_on_output(
    st: &Halley,
    surface: &WlSurface,
    output_name: &str,
) -> bool {
    if let Some(node_id) = st.model.surface_to_node.get(&surface.id()).copied() {
        let fullscreen_on_output = st
            .model
            .fullscreen_state
            .fullscreen_active_node
            .get(output_name)
            .is_some_and(|active| *active == node_id);
        return (st.model.field.is_visible(node_id) || fullscreen_on_output)
            && st
                .model
                .monitor_state
                .node_monitor
                .get(&node_id)
                .is_some_and(|monitor| monitor == output_name);
    }

    surface_on_output(st, surface, output_name)
}

fn cursor_surface_on_output(st: &Halley, surface: &WlSurface, output_name: &str) -> bool {
    let Some((sx, sy)) = cursor_global_position(st) else {
        return false;
    };
    let Some(monitor) = st.model.monitor_state.monitors.get(output_name) else {
        return false;
    };

    let (hotspot_x, hotspot_y) = crate::render::cursor_surface_hotspot(surface);
    let surface_pos: Point<i32, Logical> =
        (sx.round() as i32 - hotspot_x, sy.round() as i32 - hotspot_y).into();
    let bbox = bbox_from_surface_tree(surface, surface_pos);
    let output_geo = Rectangle::new(
        (monitor.offset_x, monitor.offset_y).into(),
        (monitor.width, monitor.height).into(),
    );
    output_geo.overlaps(bbox)
}

fn cursor_global_position(st: &Halley) -> Option<(f32, f32)> {
    if let Some(pos) = st.input.interaction_state.last_pointer_screen_global {
        return Some(pos);
    }

    let pointer = st.platform.seat.get_pointer()?;
    let location = pointer.current_location();
    let cam_scale = st.camera_render_scale().max(0.001) as f64;
    let monitor = st.model.monitor_state.current_monitor.as_str();
    let (offset_x, offset_y) = st
        .model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| (space.offset_x as f32, space.offset_y as f32))
        .unwrap_or((0.0, 0.0));
    Some((
        offset_x + (location.x * cam_scale) as f32,
        offset_y + (location.y * cam_scale) as f32,
    ))
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

fn surface_tree_has_pending_frame_callbacks(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> bool {
    let pending = Cell::new(false);
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_, states, &()| {
            pending.set(
                pending.get()
                    || !states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .frame_callbacks
                        .is_empty(),
            );
        },
        |_, _, &()| !pending.get(),
    );
    pending.get()
}

fn send_frames_surface_tree_for_output(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    time_ms: u32,
    output_name: &str,
    sequence: u32,
) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_, states, &()| {
            let has_callbacks = !states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .is_empty();
            if !has_callbacks {
                return;
            }
            if !should_send_frame_callback(states, output_name, sequence) {
                return;
            }
            let mut surface_attributes = states.cached_state.get::<SurfaceAttributes>();
            let callbacks = &mut surface_attributes.current().frame_callbacks;
            for callback in callbacks.drain(..) {
                callback.done(time_ms);
            }
        },
        |_, _, &()| true,
    );
}

fn should_send_frame_callback(states: &SurfaceData, output_name: &str, sequence: u32) -> bool {
    let throttling = states
        .data_map
        .get_or_insert(SurfaceFrameCallbackThrottle::default);
    let mut last_sent_at = throttling.last_sent_at.borrow_mut();
    if last_sent_at
        .as_ref()
        .is_some_and(|(last_output, last_sequence)| {
            last_output == output_name && *last_sequence == sequence
        })
    {
        return false;
    }
    *last_sent_at = Some((output_name.to_string(), sequence));
    true
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
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
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

    fn set_active_transfer_drag(st: &mut Halley, node_id: NodeId, target_monitor: &str) {
        st.input.interaction_state.drag_authority_node = Some(node_id);
        st.input.interaction_state.active_drag =
            Some(crate::compositor::interaction::state::ActiveDragState {
                node_id,
                allow_monitor_transfer: true,
                edge_pan_eligible: false,
                current_offset: Vec2 { x: 0.0, y: 0.0 },
                pointer_monitor: target_monitor.to_string(),
                pointer_workspace_size: (800, 600),
                pointer_screen_local: (400.0, 300.0),
                edge_pan_x: crate::compositor::interaction::DragAxisMode::Free,
                edge_pan_y: crate::compositor::interaction::DragAxisMode::Free,
                last_edge_pan_at: Instant::now(),
            });
    }

    fn open_test_cluster(
        st: &mut Halley,
        monitor: &str,
        labels: &[&str],
    ) -> halley_core::cluster::ClusterId {
        let members = labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let id = st.model.field.spawn_surface(
                    (*label).to_string(),
                    Vec2 {
                        x: 100.0 + index as f32 * 80.0,
                        y: 100.0,
                    },
                    Vec2 { x: 320.0, y: 240.0 },
                );
                st.assign_node_to_monitor(id, monitor);
                id
            })
            .collect::<Vec<_>>();
        let cid = st.create_cluster(members).expect("cluster");
        let core = st.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, monitor);
        assert!(st.enter_cluster_workspace_by_core(core, monitor, Instant::now()));
        cid
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
    fn monitor_transfer_moves_cluster_member_between_active_layouts() {
        let mut state = multi_monitor_state();
        let left_cid = open_test_cluster(&mut state, "left", &["left-a", "left-b", "left-c"]);
        let right_cid = open_test_cluster(&mut state, "right", &["right-a", "right-b"]);
        let moved = state.model.field.cluster(left_cid).unwrap().members()[2];

        set_active_transfer_drag(&mut state, moved, "right");
        tick_active_drag(&mut state, Instant::now());

        assert!(!state.model.field.cluster(left_cid).unwrap().contains(moved));
        assert!(
            state
                .model
                .field
                .cluster(right_cid)
                .unwrap()
                .contains(moved)
        );
        assert_eq!(
            state
                .model
                .monitor_state
                .node_monitor
                .get(&moved)
                .map(String::as_str),
            Some("right")
        );
    }

    #[test]
    fn stacking_monitor_transfer_detaches_from_source_without_target_cluster() {
        let mut state = multi_monitor_state();
        state.runtime.tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Stacking;
        let left_cid = open_test_cluster(&mut state, "left", &["left-a", "left-b", "left-c"]);
        let moved = state.model.field.cluster(left_cid).unwrap().members()[0];

        set_active_transfer_drag(&mut state, moved, "right");
        tick_active_drag(&mut state, Instant::now());

        assert!(!state.model.field.cluster(left_cid).unwrap().contains(moved));
        assert_eq!(state.model.field.cluster_id_for_member_public(moved), None);
        assert_eq!(
            state
                .model
                .monitor_state
                .node_monitor
                .get(&moved)
                .map(String::as_str),
            Some("right")
        );
    }

    #[test]
    fn monitor_transfer_keeps_float_rule_window_out_of_target_cluster() {
        let mut state = multi_monitor_state();
        let right_cid = open_test_cluster(&mut state, "right", &["right-a", "right-b"]);
        let floating = state.model.field.spawn_surface(
            "floating",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(floating, "left");
        state.model.spawn_state.applied_window_rules.insert(
            floating,
            crate::compositor::spawn::state::AppliedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::None,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Float,
                opacity: 1.0,
                parent_node: None,
                suppress_reveal_pan: true,
                builtin_rule: None,
            },
        );

        set_active_transfer_drag(&mut state, floating, "right");
        tick_active_drag(&mut state, Instant::now());

        assert!(
            !state
                .model
                .field
                .cluster(right_cid)
                .unwrap()
                .contains(floating)
        );
        assert!(crate::compositor::surface::node_allows_interactive_resize(
            &state, floating
        ));
        assert_eq!(
            state
                .model
                .monitor_state
                .node_monitor
                .get(&floating)
                .map(String::as_str),
            Some("right")
        );
    }

    #[test]
    fn viewport_pan_only_marks_animation_monitor_active() {
        let mut state = multi_monitor_state();
        let _ = state.activate_monitor("right");
        state.input.interaction_state.viewport_pan_anim =
            Some(crate::compositor::interaction::state::ViewportPanAnim {
                monitor: "right".to_string(),
                start_ms: 0,
                delay_ms: 0,
                duration_ms: 120,
                from_center: state.model.viewport.center,
                to_center: Vec2 {
                    x: state.model.viewport.center.x + 100.0,
                    y: state.model.viewport.center.y,
                },
            });
        let _ = state.activate_monitor("left");

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
            1.0,
            1.0,
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
    fn node_icon_fade_keeps_target_monitor_redrawing() {
        let mut state = multi_monitor_state();
        let start = Instant::now();
        let id = state.model.field.spawn_surface(
            "icon-fade".to_string(),
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(id, "right");
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

        let during_icon_fade =
            start + std::time::Duration::from_millis(crate::render::NODE_ICON_FADE_DELAY_MS + 40);
        assert!(tty_output_animation_redraw_state(&state, "right", during_icon_fade).active);
        assert!(!tty_output_animation_redraw_state(&state, "left", during_icon_fade).active);

        let after_icon_fade = start
            + std::time::Duration::from_millis(
                crate::render::NODE_ICON_FADE_DELAY_MS + crate::render::NODE_ICON_FADE_MS + 80,
            );
        assert!(!tty_output_animation_redraw_state(&state, "right", after_icon_fade).active);
    }

    #[test]
    fn active_transition_alpha_uses_configured_duration() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = state.model.field.spawn_surface(
            "anim",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        let start = Instant::now();

        crate::compositor::workspace::state::mark_active_transition(&mut state, id, start, 1000);
        let alpha = crate::compositor::workspace::state::active_transition_alpha(
            &state,
            id,
            start + std::time::Duration::from_millis(500),
        );

        assert!((alpha - 0.5).abs() < 0.02, "alpha was {alpha}");
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
