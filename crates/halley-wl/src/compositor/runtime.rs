use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use calloop::ping::Ping;
use halley_config::RuntimeTuning;
use smithay::reexports::wayland_server::backend::ObjectId;

use super::monitor::camera::camera_controller;
use super::root::Halley;
use super::screenshot::screenshot_controller;
use crate::activity::CommitActivity;
use crate::animation::AnimSpec;
use crate::protocol::wayland::activation::ActivationRuntimeState;

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
    pub(crate) pending_drm_syncobj_surfaces: Arc<Mutex<Vec<ObjectId>>>,
    pub(crate) activation: ActivationRuntimeState,
    pub(crate) spawned_children: Vec<std::process::Child>,
}

pub(crate) struct RuntimeController<T> {
    st: T,
}

pub(crate) fn runtime_controller<T>(st: T) -> RuntimeController<T> {
    RuntimeController { st }
}

impl<T: Deref<Target = Halley>> Deref for RuntimeController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for RuntimeController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

impl<T: Deref<Target = Halley>> RuntimeController<T> {
    pub fn now_ms(&self, now: Instant) -> u64 {
        now.duration_since(self.runtime.started_at).as_millis() as u64
    }

    pub(crate) fn debug_dump(&self) {}

    pub fn exit_requested(&self) -> bool {
        self.runtime.exit_requested
    }

    pub fn next_maintenance_deadline(&self, now: Instant) -> Option<Instant> {
        if !self.model.focus_state.app_focused {
            return None;
        }

        let now_ms = self.now_ms(now);
        let mut next_ms: Option<u64> = None;
        let mut consider = |at_ms: u64| {
            next_ms = Some(next_ms.map_or(at_ms, |cur| cur.min(at_ms)));
        };

        if self.model.focus_state.primary_interaction_focus.is_some()
            && self.model.focus_state.interaction_focus_until_ms > now_ms
        {
            consider(self.model.focus_state.interaction_focus_until_ms);
        }
        if self.input.interaction_state.resize_static_node.is_some()
            && self.input.interaction_state.resize_static_until_ms > now_ms
        {
            consider(self.input.interaction_state.resize_static_until_ms);
        }
        if let Some(at_ms) = self
            .model
            .spawn_state
            .pending_spawn_activate_at_ms
            .values()
            .copied()
            .min()
            && at_ms > now_ms
        {
            consider(at_ms);
        }
        if let Some(at_ms) = self
            .model
            .spawn_state
            .pending_tiled_insert_reveal_at_ms
            .values()
            .copied()
            .min()
        {
            consider(at_ms);
        }
        if let Some(at_ms) = self
            .model
            .workspace_state
            .active_transition_until_ms
            .values()
            .copied()
            .min()
            && at_ms > now_ms
        {
            consider(at_ms);
        }
        if let Some(at_ms) = self
            .model
            .workspace_state
            .primary_promote_cooldown_until_ms
            .values()
            .copied()
            .min()
            && at_ms > now_ms
        {
            consider(at_ms);
        }
        if let Some(deadline_ms) = self
            .input
            .interaction_state
            .pending_core_click
            .as_ref()
            .map(|pending| pending.deadline_ms)
            && deadline_ms > now_ms
        {
            consider(deadline_ms);
        }
        if let Some(repeat_at_ms) = self
            .input
            .interaction_state
            .cluster_name_prompt_repeat
            .as_ref()
            .map(|repeat| repeat.next_repeat_ms)
        {
            consider(repeat_at_ms);
        }
        if let Some(capture_at_ms) = self
            .input
            .interaction_state
            .pending_screenshot_capture
            .as_ref()
            .map(|pending| pending.execute_at_ms)
        {
            consider(capture_at_ms);
        }
        if let Some(restore_at_ms) = self
            .input
            .interaction_state
            .pending_modal_focus_restore
            .as_ref()
            .map(|pending| pending.restore_at_ms)
        {
            consider(restore_at_ms);
        }
        if let Some(until_ms) = self.input.interaction_state.cursor_override_until_ms {
            consider(until_ms);
        }
        if self
            .input
            .interaction_state
            .inflight_screenshot_capture
            .is_some()
        {
            consider(now_ms.saturating_add(33));
        }
        if crate::compositor::interaction::state::bloom_pull_preview_needs_animation(self) {
            consider(now_ms.saturating_add(16));
        }
        if self
            .model
            .cluster_state
            .cluster_overflow_reveal_started_at_ms
            .iter()
            .any(|(monitor, started_at_ms)| {
                let visible_until_ms = self
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
        {
            consider(now_ms.saturating_add(16));
        }
        if self
            .model
            .cluster_state
            .cluster_overflow_promotion_anim
            .values()
            .any(|anim| now_ms < anim.reveal_at_ms)
        {
            consider(now_ms.saturating_add(16));
        }
        next_ms.map(|at_ms| {
            now.checked_add(std::time::Duration::from_millis(
                at_ms.saturating_sub(now_ms),
            ))
            .unwrap_or(now)
        })
    }
}

impl<T: DerefMut<Target = Halley>> RuntimeController<T> {
    pub fn apply_tuning(&mut self, mut tuning: RuntimeTuning) {
        let prev_runtime_viewport = self.model.viewport;
        let prev_config_viewport = self.runtime.tuning.viewport();
        let prev_effective_no_csd = self.runtime.tuning.effective_no_csd();
        let prev_font = self.runtime.tuning.font.clone();
        let prev_physics_enabled = self.runtime.tuning.physics_enabled;
        let prev_focus = self.last_input_surface_node();
        let previous_output_names: std::collections::HashSet<String> = self
            .model
            .monitor_state
            .monitors
            .keys()
            .cloned()
            .chain(
                self.runtime
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
            self.model.viewport = next_viewport;
            self.model.zoom_ref_size = tuning.viewport_size;
            self.model.camera_target_center = self.model.viewport.center;
            self.model.camera_target_view_size = self.model.zoom_ref_size;
            if prev_runtime_viewport.center != next_viewport.center
                || prev_runtime_viewport.size != next_viewport.size
            {
                self.input.interaction_state.viewport_pan_anim = None;
            }
        }

        self.ui.render_state.animator.set_spec(AnimSpec {
            state_change_ms: FIXED_ANIM_STATE_CHANGE_MS,
            bounce: FIXED_ANIM_BOUNCE,
        });

        if prev_physics_enabled && !tuning.physics_enabled {
            self.input.interaction_state.drag_authority_node = None;
            self.input.interaction_state.physics_velocity.clear();
            self.input.interaction_state.smoothed_render_pos.clear();
            self.model.camera_target_center = self.model.viewport.center;
            self.model.camera_target_view_size = self.model.zoom_ref_size;
        }

        let next_output_names: std::collections::HashSet<String> = previous_output_names
            .iter()
            .cloned()
            .chain(tuning.tty_viewports.iter().map(|v| v.connector.clone()))
            .collect();
        let now = Instant::now();
        let now_ms = self.now_ms(now);
        for output_name in next_output_names {
            if self
                .runtime
                .tuning
                .focus_ring_for_output(output_name.as_str())
                != tuning.focus_ring_for_output(output_name.as_str())
            {
                self.model.focus_state.focus_ring_preview_until_ms.insert(
                    output_name,
                    now_ms.saturating_add(crate::compositor::focus::state::FOCUS_RING_PREVIEW_MS),
                );
            }
        }

        self.runtime.tuning = tuning;
        if self.runtime.tuning.font != prev_font {
            self.ui.render_state.invalidate_ui_text_cache();
        }
        if !self.runtime.tuning.cursor.hide_while_typing {
            self.input.interaction_state.cursor_hidden_by_typing = false;
        }
        if prev_effective_no_csd != self.runtime.tuning.effective_no_csd() {
            self.refresh_xdg_decoration_mode();
        }
        self.request_maintenance();

        if let Some(id) = prev_focus {
            self.set_interaction_focus(Some(id), 30_000, now);
        }
    }

    pub fn request_exit(&mut self) {
        self.runtime.exit_requested = true;
    }

    #[inline]
    pub fn request_maintenance(&mut self) {
        self.runtime.maintenance_dirty = true;
        if let Some(ping) = &self.runtime.maintenance_ping {
            ping.ping();
        }
    }

    #[inline]
    pub fn run_maintenance_if_needed(&mut self, now: Instant) {
        let due = self
            .next_maintenance_deadline(now)
            .is_some_and(|deadline| deadline <= now);
        if self.runtime.maintenance_dirty || due {
            self.run_maintenance(now);
        }
    }

    #[inline]
    pub fn run_maintenance(&mut self, now: Instant) {
        self.runtime.maintenance_dirty = false;
        if !self.model.focus_state.app_focused {
            return;
        }
        crate::compositor::workspace::lifecycle::reconcile_surface_bindings(self);
        let now_ms = now.duration_since(self.runtime.started_at).as_millis() as u64;
        crate::protocol::wayland::activation::prune_expired(self, now, now_ms);
        let _ = self.recent_top_node_active(now);
        if let Some(pending) = self.input.interaction_state.pending_core_click.clone()
            && now_ms >= pending.deadline_ms
        {
            self.input.interaction_state.pending_core_click = None;
        }
        let _ = crate::compositor::clusters::system::cluster_system_controller(&mut **self)
            .repeat_cluster_name_prompt_input_if_due(now_ms);
        screenshot_controller(&mut **self).run_pending_screenshot_capture_if_due(now_ms);
        if let Some(pending) = self
            .input
            .interaction_state
            .pending_modal_focus_restore
            .clone()
            && now_ms >= pending.restore_at_ms
        {
            self.input.interaction_state.pending_modal_focus_restore = None;
            self.apply_wayland_focus_state(pending.target);
        }
        if self
            .input
            .interaction_state
            .cursor_override_until_ms
            .is_some_and(|until_ms| now_ms >= until_ms)
        {
            self.input.interaction_state.cursor_override_until_ms = None;
            self.input.interaction_state.cursor_override_icon = None;
        }
        if self.has_any_active_cluster_workspace() {
            let active_monitors = self
                .model
                .cluster_state
                .active_cluster_workspaces
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            for monitor in active_monitors {
                self.layout_active_cluster_workspace_for_monitor(monitor.as_str(), now_ms);
            }
        }
        if let Some(fid) = self.model.focus_state.primary_interaction_focus
            && now_ms >= self.model.focus_state.interaction_focus_until_ms
        {
            let keep = self.model.field.node(fid).is_some_and(|n| {
                self.model.field.is_visible(fid) && n.kind == halley_core::field::NodeKind::Surface
            });
            if keep {
                self.model.focus_state.interaction_focus_until_ms = now_ms.saturating_add(30_000);
            } else {
                self.set_interaction_focus(None, 0, now);
            }
        }
        if crate::protocol::wayland::session_lock::session_lock_active(self) {
            crate::protocol::wayland::session_lock::reassert_keyboard_focus_if_drifted(self);
        } else if self.model.focus_state.primary_interaction_focus.is_none()
            && self.model.monitor_state.layer_keyboard_focus.is_some()
        {
            crate::compositor::monitor::layer_shell::reassert_layer_surface_keyboard_focus_if_drifted(self);
        }
        self.model
            .workspace_state
            .active_transition_until_ms
            .retain(|_, &mut until| until > now_ms);
        self.model
            .workspace_state
            .primary_promote_cooldown_until_ms
            .retain(|_, &mut until| until > now_ms);
        let alive_ids: std::collections::HashSet<_> =
            self.model.field.node_ids_all().into_iter().collect();
        self.model
            .carry_state
            .carry_zone_hint
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .carry_state
            .carry_zone_last_change_ms
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .carry_state
            .carry_zone_pending
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .carry_state
            .carry_zone_pending_since_ms
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .carry_state
            .carry_activation_anim_armed
            .retain(|id| alive_ids.contains(id));
        self.model
            .carry_state
            .carry_state_hold
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .focus_state
            .last_surface_focus_ms
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .workspace_state
            .manual_collapsed_nodes
            .retain(|id| alive_ids.contains(id));
        self.model
            .spawn_state
            .pending_tiled_insert_reveal_at_ms
            .retain(|id, _| alive_ids.contains(id));
        self.model
            .spawn_state
            .pending_tiled_insert_preserve_focus
            .retain(|id| alive_ids.contains(id));
        self.model
            .cluster_state
            .cluster_overflow_promotion_anim
            .retain(|_, anim| alive_ids.contains(&anim.member_id) && now_ms < anim.reveal_at_ms);

        self.process_pending_spawn_activations(now, now_ms);
        let resize_settling = self
            .input
            .interaction_state
            .resize_static_node
            .is_some_and(|_| now_ms < self.input.interaction_state.resize_static_until_ms);
        if resize_settling
            && let (Some(id), Some(lock_pos)) = (
                self.input.interaction_state.resize_static_node,
                self.input.interaction_state.resize_static_lock_pos,
            )
            && let Some(n) = self.model.field.node(id)
            && ((n.pos.x - lock_pos.x).abs() > 0.05 || (n.pos.y - lock_pos.y).abs() > 0.05)
        {
            let _ = self.model.field.carry(id, lock_pos);
        }
        if self
            .input
            .interaction_state
            .resize_static_node
            .is_some_and(|_| now_ms >= self.input.interaction_state.resize_static_until_ms)
        {
            self.input.interaction_state.resize_static_node = None;
            self.input.interaction_state.resize_static_lock_pos = None;
            self.input.interaction_state.resize_static_until_ms = 0;
        }
        if !self.input.interaction_state.suspend_state_checks {
            crate::compositor::interaction::state::enforce_pan_dominant_zone_states(self, now_ms);
            crate::compositor::carry::state::enforce_carry_zone_states(self);
        }
        if let Some(id) = self.input.interaction_state.resize_active {
            let _ = self.model.field.touch(id, now_ms);
            let _ = self
                .model
                .field
                .set_decay_level(id, halley_core::decay::DecayLevel::Hot);
        }
        if self.input.interaction_state.resize_active.is_none()
            && !(self.input.interaction_state.resize_static_node.is_some()
                && now_ms < self.input.interaction_state.resize_static_until_ms)
        {
            camera_controller(&mut **self).update_zoom_live_surface_sizes();
        }
        let cluster_policy = halley_core::cluster_policy::ClusterPolicy {
            enabled: false,
            distance_px: self.runtime.tuning.cluster_distance_px,
            dwell_ms: self.runtime.tuning.cluster_dwell_ms,
            ..Default::default()
        };
        let model = &mut self.model;
        let _ = halley_core::cluster_policy::tick_cluster_formation(
            &mut model.field,
            now_ms,
            cluster_policy,
            &mut model.cluster_state.cluster_form_state,
        );
        self.enforce_single_primary_active_unit();
        if !self.input.interaction_state.suspend_state_checks
            && self.input.interaction_state.resize_active.is_none()
        {
            self.resolve_surface_overlap();
        }
        self.restore_pan_return_active_focus(now);
        let animations_enabled = self.runtime.tuning.animations_enabled();
        let crate::compositor::root::Halley { model, ui, .. } = &mut **self;
        if animations_enabled {
            ui.render_state.animator.observe_field(&model.field, now);
        }
    }
}
