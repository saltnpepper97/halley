use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::root::Halley;
use super::screenshot;
use crate::animation::AnimSpec;
use crate::compositor::activity::CommitActivity;
use crate::protocol::wayland::activation::ActivationRuntimeState;
use calloop::ping::Ping;
use eventline::warn;
use halley_config::RuntimeTuning;
use smithay::reexports::wayland_server::backend::ObjectId;

const FIXED_ANIM_STATE_CHANGE_MS: u64 = 360;
const FIXED_ANIM_BOUNCE: f32 = 1.45;

pub(crate) struct RuntimeState {
    pub(crate) tuning: RuntimeTuning,
    pub(crate) surface_activity: HashMap<ObjectId, CommitActivity>,
    pub(crate) exit_requested: bool,
    pub(crate) started_at: Instant,
    pub(crate) maintenance_dirty: bool,
    pub(crate) screenshot_full_repaint_until_ms: u64,
    pub(crate) maintenance_ping: Option<Ping>,
    pub(crate) tty_redraw_all: bool,
    pub(crate) tty_redraw_outputs: HashSet<String>,
    pub(crate) tty_frame_callback_sequence: HashMap<String, u32>,
    pub(crate) pending_drm_syncobj_surfaces: Arc<Mutex<Vec<ObjectId>>>,
    pub(crate) activation: ActivationRuntimeState,
    pub(crate) spawned_children: Vec<std::process::Child>,
    pub(crate) wayland_display: Option<String>,
}

pub fn now_ms(st: &Halley, now: Instant) -> u64 {
    now.duration_since(st.runtime.started_at).as_millis() as u64
}

pub fn exit_requested(st: &Halley) -> bool {
    st.runtime.exit_requested
}

pub fn next_maintenance_deadline(st: &Halley, now: Instant) -> Option<Instant> {
    if !st.model.focus_state.app_focused {
        return None;
    }

    let now_ms = now_ms(st, now);
    let next_ms = min_optional_deadlines([
        focus_deadline_ms(st, now_ms),
        resize_deadline_ms(st, now_ms),
        spawn_deadline_ms(st, now_ms),
        workspace_deadline_ms(st, now_ms),
        interaction_deadline_ms(st, now_ms),
        animation_deadline_ms(st, now_ms),
    ]);
    next_ms.map(|at_ms| {
        now.checked_add(std::time::Duration::from_millis(
            at_ms.saturating_sub(now_ms),
        ))
        .unwrap_or(now)
    })
}

fn min_optional_deadlines<const N: usize>(deadlines: [Option<u64>; N]) -> Option<u64> {
    deadlines.into_iter().flatten().min()
}

fn focus_deadline_ms(st: &Halley, now_ms: u64) -> Option<u64> {
    min_optional_deadlines([
        (st.model.focus_state.primary_interaction_focus.is_some()
            && st.model.focus_state.interaction_focus_until_ms > now_ms)
            .then_some(st.model.focus_state.interaction_focus_until_ms),
        st.input
            .interaction_state
            .pending_modal_focus_restore
            .as_ref()
            .map(|pending| pending.restore_at_ms),
    ])
}

fn resize_deadline_ms(st: &Halley, now_ms: u64) -> Option<u64> {
    (st.input.interaction_state.resize_static_node.is_some()
        && st.input.interaction_state.resize_static_until_ms > now_ms)
        .then_some(st.input.interaction_state.resize_static_until_ms)
}

fn spawn_deadline_ms(st: &Halley, now_ms: u64) -> Option<u64> {
    let pending_spawn_activate_at_ms = st
        .model
        .spawn_state
        .pending_spawn_activate_at_ms
        .values()
        .copied()
        .min()
        .filter(|&at_ms| at_ms > now_ms);
    let pending_tiled_insert_reveal_at_ms = st
        .model
        .spawn_state
        .pending_tiled_insert_reveal_at_ms
        .values()
        .copied()
        .min();
    let active_spawn_pan_at_ms = st.model.spawn_state.active_spawn_pan.and_then(|pan| {
        [pan.pan_start_at_ms, pan.reveal_at_ms]
            .into_iter()
            .filter(|&at_ms| at_ms > now_ms)
            .min()
    });
    let pending_pan_activate_at_ms = st
        .model
        .spawn_state
        .pending_pan_activate
        .map(|(_, at_ms)| at_ms)
        .filter(|&at_ms| at_ms > now_ms);
    min_optional_deadlines([
        pending_spawn_activate_at_ms,
        pending_tiled_insert_reveal_at_ms,
        active_spawn_pan_at_ms,
        pending_pan_activate_at_ms,
    ])
}

fn workspace_deadline_ms(st: &Halley, now_ms: u64) -> Option<u64> {
    let active_transition_at_ms = st
        .model
        .workspace_state
        .active_transitions
        .values()
        .map(|transition| transition.until_ms())
        .min()
        .filter(|&at_ms| at_ms > now_ms);
    let primary_promote_cooldown_until_ms = st
        .model
        .workspace_state
        .primary_promote_cooldown_until_ms
        .values()
        .copied()
        .min()
        .filter(|&at_ms| at_ms > now_ms);
    let pending_silent_close_until_ms = st
        .model
        .workspace_state
        .pending_silent_close_until_ms
        .values()
        .copied()
        .min()
        .filter(|&at_ms| at_ms > now_ms);
    min_optional_deadlines([
        active_transition_at_ms,
        primary_promote_cooldown_until_ms,
        pending_silent_close_until_ms,
    ])
}

fn interaction_deadline_ms(st: &Halley, now_ms: u64) -> Option<u64> {
    let pending_core_click_deadline_ms = st
        .input
        .interaction_state
        .pending_core_click
        .as_ref()
        .map(|pending| pending.deadline_ms)
        .filter(|&deadline_ms| deadline_ms > now_ms);
    let pending_collapsed_node_click_deadline_ms = st
        .input
        .interaction_state
        .pending_collapsed_node_click
        .as_ref()
        .map(|pending| pending.deadline_ms)
        .filter(|&deadline_ms| deadline_ms > now_ms);
    let cluster_name_prompt_repeat_at_ms = st
        .input
        .interaction_state
        .cluster_name_prompt_repeat
        .as_ref()
        .map(|repeat| repeat.next_repeat_ms);
    let pending_screenshot_capture_at_ms = st
        .input
        .interaction_state
        .pending_screenshot_capture
        .as_ref()
        .map(|pending| pending.execute_at_ms);
    let inflight_screenshot_capture_at_ms = st
        .input
        .interaction_state
        .inflight_screenshot_capture
        .is_some()
        .then_some(now_ms.saturating_add(33));
    min_optional_deadlines([
        pending_core_click_deadline_ms,
        pending_collapsed_node_click_deadline_ms,
        cluster_name_prompt_repeat_at_ms,
        pending_screenshot_capture_at_ms,
        st.input.interaction_state.cursor_override_until_ms,
        inflight_screenshot_capture_at_ms,
    ])
}

fn animation_deadline_ms(st: &Halley, now_ms: u64) -> Option<u64> {
    let bloom_pull_preview_at_ms =
        crate::compositor::interaction::state::bloom_pull_preview_needs_animation(st)
            .then_some(now_ms.saturating_add(16));
    let cluster_overflow_reveal_at_ms = st
        .model
        .cluster_state
        .cluster_overflow_reveal_started_at_ms
        .iter()
        .any(|(monitor, started_at_ms)| {
            let visible_until_ms = st
                .model
                .cluster_state
                .cluster_overflow_visible_until_ms
                .get(monitor)
                .copied();
            visible_until_ms.is_some_and(|visible_until_ms| {
                visible_until_ms > now_ms
                    && (now_ms.saturating_sub(*started_at_ms) < 220
                        || visible_until_ms.saturating_sub(now_ms) < 220)
            })
        })
        .then_some(now_ms.saturating_add(16));
    let cluster_overflow_promotion_at_ms = st
        .model
        .cluster_state
        .cluster_overflow_promotion_anim
        .values()
        .any(|anim| now_ms < anim.reveal_at_ms)
        .then_some(now_ms.saturating_add(16));
    min_optional_deadlines([
        bloom_pull_preview_at_ms,
        cluster_overflow_reveal_at_ms,
        cluster_overflow_promotion_at_ms,
    ])
}

pub fn apply_tuning(st: &mut Halley, mut tuning: RuntimeTuning) {
    let prev_runtime_viewport = st.model.viewport;
    let prev_config_viewport = st.runtime.tuning.viewport();
    let prev_decorations = st.runtime.tuning.decorations;
    let prev_font = st.runtime.tuning.font.clone();
    let prev_input = st.runtime.tuning.input.clone();
    let prev_physics_enabled = st.runtime.tuning.physics_enabled;
    let prev_focus = st.last_input_surface_node();
    let previous_output_names: std::collections::HashSet<String> = st
        .model
        .monitor_state
        .monitors
        .keys()
        .cloned()
        .chain(
            st.runtime
                .tuning
                .tty_viewports
                .iter()
                .map(|v| v.connector.clone()),
        )
        .collect();

    tuning.enforce_guards();
    tuning.apply_process_env();

    let next_viewport = tuning.viewport();
    let logical_viewport_changed = prev_config_viewport.center != next_viewport.center
        || prev_config_viewport.size != next_viewport.size;
    if logical_viewport_changed {
        st.model.viewport = next_viewport;
        st.model.zoom_ref_size = tuning.viewport_size;
        st.model.camera_target_center = st.model.viewport.center;
        st.model.camera_target_view_size = st.model.zoom_ref_size;
        if prev_runtime_viewport.center != next_viewport.center
            || prev_runtime_viewport.size != next_viewport.size
        {
            st.input.interaction_state.viewport_pan_anim = None;
        }
    }

    st.ui.render_state.animator.set_spec(AnimSpec {
        state_change_ms: FIXED_ANIM_STATE_CHANGE_MS,
        bounce: FIXED_ANIM_BOUNCE,
    });

    if prev_physics_enabled && !tuning.physics_enabled {
        st.input.interaction_state.drag_authority_node = None;
        st.input.interaction_state.physics_velocity.clear();
        st.input.interaction_state.smoothed_render_pos.clear();
        st.model.camera_target_center = st.model.viewport.center;
        st.model.camera_target_view_size = st.model.zoom_ref_size;
    }

    let next_output_names: std::collections::HashSet<String> = previous_output_names
        .iter()
        .cloned()
        .chain(tuning.tty_viewports.iter().map(|v| v.connector.clone()))
        .collect();
    let now = Instant::now();
    let now_ms = now_ms(st, now);
    if tuning.debug.show_ring_when_resizing {
        for output_name in next_output_names {
            if st
                .runtime
                .tuning
                .focus_ring_for_output(output_name.as_str())
                != tuning.focus_ring_for_output(output_name.as_str())
            {
                st.model.focus_state.focus_ring_preview_until_ms.insert(
                    output_name,
                    now_ms.saturating_add(crate::compositor::focus::state::FOCUS_RING_PREVIEW_MS),
                );
            }
        }
    } else {
        st.model.focus_state.focus_ring_preview_until_ms.clear();
    }

    st.runtime.tuning = tuning;
    crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(st);
    let repeat_changed = st.runtime.tuning.input.repeat_rate != prev_input.repeat_rate
        || st.runtime.tuning.input.repeat_delay != prev_input.repeat_delay;
    let keyboard_config_changed = st.runtime.tuning.input.keyboard != prev_input.keyboard;
    if let Some(keyboard) = st.platform.seat.get_keyboard() {
        if repeat_changed {
            keyboard.change_repeat_info(
                st.runtime.tuning.input.repeat_rate,
                st.runtime.tuning.input.repeat_delay,
            );
        }
        if keyboard_config_changed {
            let xkb_config =
                crate::backend::ResolvedXkbConfig::from_input(&st.runtime.tuning.input);
            if let Err(err) = keyboard.set_xkb_config(st, xkb_config.as_smithay()) {
                warn!(
                    "failed to apply keyboard layout='{}' variant='{}' options='{}' on reload: {}",
                    st.runtime.tuning.input.keyboard.layout,
                    st.runtime.tuning.input.keyboard.variant,
                    st.runtime.tuning.input.keyboard.options,
                    err
                );
            }
        }
    }
    let device_config_changed = st.runtime.tuning.input.touchpad != prev_input.touchpad
        || st.runtime.tuning.input.mouse != prev_input.mouse
        || st.runtime.tuning.input.devices != prev_input.devices;
    if device_config_changed {
        // Clone so the device loop borrows `st.input` mutably without holding a
        // borrow on `st.runtime` (which deref splitting can't prove disjoint here).
        let input_cfg = st.runtime.tuning.input.clone();
        for device in st.input.devices.iter_mut() {
            crate::input::device_config::apply_device_config(device, &input_cfg);
        }
    }
    if st.runtime.tuning.font != prev_font {
        st.ui.render_state.invalidate_ui_text_cache();
    }
    if st.runtime.tuning.decorations != prev_decorations {
        st.ui.render_state.clear_window_offscreen_caches();
    }
    if !st.runtime.tuning.cursor.hide_while_typing {
        st.input.interaction_state.cursor_hidden_by_typing = false;
    }
    crate::compositor::platform::refresh_xdg_decoration_mode(st);
    request_maintenance(st);

    if let Some(id) = prev_focus {
        st.set_interaction_focus(Some(id), 30_000, now);
    }
}

pub fn request_exit(st: &mut Halley) {
    st.runtime.exit_requested = true;
}

#[inline]
pub fn request_maintenance(st: &mut Halley) {
    st.runtime.maintenance_dirty = true;
    st.runtime.tty_redraw_all = true;
    if let Some(ping) = &st.runtime.maintenance_ping {
        ping.ping();
    }
}

#[inline]
pub fn run_maintenance_if_needed(st: &mut Halley, now: Instant) {
    let due = next_maintenance_deadline(st, now).is_some_and(|deadline| deadline <= now);
    if st.runtime.maintenance_dirty || due {
        run_maintenance(st, now);
    }
}

#[inline]
pub fn run_maintenance(st: &mut Halley, now: Instant) {
    st.runtime.maintenance_dirty = false;
    if !st.model.focus_state.app_focused {
        return;
    }
    crate::compositor::workspace::lifecycle::reconcile_surface_bindings(st);
    let now_ms = now.duration_since(st.runtime.started_at).as_millis() as u64;
    crate::protocol::wayland::activation::prune_expired(st, now, now_ms);
    let _ = crate::compositor::focus::state::recent_top_node_active(st, now);
    let pointer_contents_changed =
        crate::compositor::interaction::pointer::update_pointer_contents_at_last_screen(
            st, None, now,
        );
    if pointer_contents_changed {
        if let Some((sx, sy)) = st.input.interaction_state.last_pointer_screen_global
            && let Some(output_name) = st.monitor_for_screen(sx, sy)
        {
            st.request_tty_redraw_for_monitor(output_name.as_str());
        } else {
            st.runtime.tty_redraw_all = true;
        }
    }
    if let Some(pending) = st.input.interaction_state.pending_core_click.clone()
        && now_ms >= pending.deadline_ms
    {
        st.input.interaction_state.pending_core_click = None;
    }
    if let Some(pending) = st
        .input
        .interaction_state
        .pending_collapsed_node_click
        .clone()
        && now_ms >= pending.deadline_ms
    {
        st.input.interaction_state.pending_collapsed_node_click = None;
    }
    let _ = crate::compositor::clusters::system::repeat_cluster_name_prompt_input_if_due(
        &mut *st, now_ms,
    );
    screenshot::run_pending_screenshot_capture_if_due(&mut *st, now_ms);
    if let Some(pending) = st
        .input
        .interaction_state
        .pending_modal_focus_restore
        .clone()
        && now_ms >= pending.restore_at_ms
    {
        st.input.interaction_state.pending_modal_focus_restore = None;
        st.apply_wayland_focus_state(pending.target);
    }
    if st
        .input
        .interaction_state
        .cursor_override_until_ms
        .is_some_and(|until_ms| now_ms >= until_ms)
    {
        st.input.interaction_state.cursor_override_until_ms = None;
        st.input.interaction_state.cursor_override_icon = None;
    }
    if crate::compositor::clusters::system::has_any_active_cluster_workspace(st) {
        let active_monitors = st
            .model
            .cluster_state
            .active_cluster_workspaces
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for monitor in active_monitors {
            st.layout_active_cluster_workspace_for_monitor(monitor.as_str(), now_ms);
        }
    }
    // Flush any aperture work-area change deferred while a layout session was
    // active. `refresh` re-checks each monitor: a monitor whose session has
    // ended applies its true reservation, while a still-locked one stays
    // frozen for the whole session (so no end-of-slide snap). Only run when a
    // pending monitor is actually unlocked — otherwise the refresh would
    // re-defer every entry and needlessly invalidate the aperture mode cache
    // each tick for the whole session.
    if crate::compositor::monitor::layer_shell::any_pending_workarea_unlocked(st) {
        crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(st);
    }
    if let Some(fid) = st.model.focus_state.primary_interaction_focus
        && now_ms >= st.model.focus_state.interaction_focus_until_ms
    {
        let keep = st.model.field.node(fid).is_some_and(|n| {
            st.model.field.is_visible(fid) && n.kind == halley_core::field::NodeKind::Surface
        });
        if keep {
            st.model.focus_state.interaction_focus_until_ms = now_ms.saturating_add(30_000);
        } else {
            st.set_interaction_focus(None, 0, now);
        }
    }
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        crate::protocol::wayland::session_lock::reassert_keyboard_focus_if_drifted(st);
    } else if st.model.focus_state.primary_interaction_focus.is_none()
        && st.model.monitor_state.layer_keyboard_focus.is_some()
    {
        crate::compositor::monitor::layer_shell::reassert_layer_surface_keyboard_focus_if_drifted(
            st,
        );
    }
    st.model
        .workspace_state
        .active_transitions
        .retain(|_, transition| transition.is_active(now_ms));
    st.model
        .workspace_state
        .primary_promote_cooldown_until_ms
        .retain(|_, &mut until| until > now_ms);
    let expired_silent_close = st
        .model
        .workspace_state
        .pending_silent_close_until_ms
        .iter()
        .filter_map(|(&id, &until)| (until <= now_ms).then_some(id))
        .collect::<Vec<_>>();
    for id in expired_silent_close {
        st.model
            .workspace_state
            .pending_silent_close_until_ms
            .remove(&id);
        if !st.model.field.is_cluster_member(id)
            && let Some(node) = st.model.field.node_mut(id)
        {
            node.visibility
                .clear(halley_core::field::Visibility::HIDDEN_BY_CLUSTER);
        }
    }
    let alive_ids: std::collections::HashSet<_> =
        st.model.field.node_ids_all().into_iter().collect();
    st.model
        .carry_state
        .carry_zone_hint
        .retain(|id, _| alive_ids.contains(id));
    st.model
        .carry_state
        .carry_zone_last_change_ms
        .retain(|id, _| alive_ids.contains(id));
    st.model
        .carry_state
        .carry_zone_pending
        .retain(|id, _| alive_ids.contains(id));
    st.model
        .carry_state
        .carry_zone_pending_since_ms
        .retain(|id, _| alive_ids.contains(id));
    st.model
        .carry_state
        .carry_activation_anim_armed
        .retain(|id| alive_ids.contains(id));
    st.model
        .carry_state
        .carry_state_hold
        .retain(|id, _| alive_ids.contains(id));
    st.model
        .focus_state
        .last_surface_focus_ms
        .retain(|id, _| alive_ids.contains(id));
    st.model
        .focus_state
        .overlap_raise_order
        .retain(|id, _| alive_ids.contains(id));
    st.model
        .workspace_state
        .manual_collapsed_nodes
        .retain(|id| alive_ids.contains(id));
    st.model
        .spawn_state
        .pending_tiled_insert_reveal_at_ms
        .retain(|id, _| alive_ids.contains(id));
    st.model
        .spawn_state
        .pending_tiled_insert_preserve_focus
        .retain(|id| alive_ids.contains(id));
    st.model
        .cluster_state
        .cluster_overflow_promotion_anim
        .retain(|_, anim| alive_ids.contains(&anim.member_id) && now_ms < anim.reveal_at_ms);

    crate::compositor::spawn::state::process_pending_spawn_activations(st, now, now_ms);
    let resize_settling = st
        .input
        .interaction_state
        .resize_static_node
        .is_some_and(|_| now_ms < st.input.interaction_state.resize_static_until_ms);
    if resize_settling
        && let (Some(id), Some(lock_pos)) = (
            st.input.interaction_state.resize_static_node,
            st.input.interaction_state.resize_static_lock_pos,
        )
        && let Some(n) = st.model.field.node(id)
        && ((n.pos.x - lock_pos.x).abs() > 0.05 || (n.pos.y - lock_pos.y).abs() > 0.05)
    {
        let _ = st.model.field.carry(id, lock_pos);
    }
    if st
        .input
        .interaction_state
        .resize_static_node
        .is_some_and(|_| now_ms >= st.input.interaction_state.resize_static_until_ms)
    {
        st.input.interaction_state.resize_static_node = None;
        st.input.interaction_state.resize_static_lock_pos = None;
        st.input.interaction_state.resize_static_until_ms = 0;
    }
    if !st.input.interaction_state.suspend_state_checks {
        crate::compositor::interaction::state::enforce_pan_dominant_zone_states(st, now_ms);
        crate::compositor::carry::state::enforce_carry_zone_states(st);
    }
    if let Some(id) = st.input.interaction_state.resize_active {
        let _ = st.model.field.touch(id, now_ms);
        let _ = st
            .model
            .field
            .set_decay_level(id, halley_core::decay::DecayLevel::Hot);
    }
    if st.input.interaction_state.resize_active.is_none()
        && !(st.input.interaction_state.resize_static_node.is_some()
            && now_ms < st.input.interaction_state.resize_static_until_ms)
    {
        crate::compositor::monitor::camera::update_zoom_live_surface_sizes(&mut *st);
    }
    let cluster_policy = halley_core::cluster_policy::ClusterPolicy {
        enabled: false,
        distance_px: st.runtime.tuning.cluster_distance_px,
        dwell_ms: st.runtime.tuning.cluster_dwell_ms,
        ..Default::default()
    };
    let model = &mut st.model;
    let _ = halley_core::cluster_policy::tick_cluster_formation(
        &mut model.field,
        now_ms,
        cluster_policy,
        &mut model.cluster_state.cluster_form_state,
    );
    st.enforce_single_primary_active_unit();
    if !st.input.interaction_state.suspend_state_checks
        && st.input.interaction_state.resize_active.is_none()
    {
        st.resolve_surface_overlap();
    }
    crate::compositor::focus::state::restore_pan_return_active_focus(st, now);
    let animations_enabled = st.runtime.tuning.animations_enabled();
    let crate::compositor::root::Halley { model, ui, .. } = st;
    if animations_enabled {
        ui.render_state.animator.observe_field(&model.field, now);
    }
}
