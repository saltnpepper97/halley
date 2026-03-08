use super::*;
use eventline::info;
use halley_core::viewport::FocusZone;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource};
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::set_primary_focus;

pub(super) struct ViewportPanAnim {
    pub(super) start_ms: u64,
    pub(super) duration_ms: u64,
    pub(super) from_center: Vec2,
    pub(super) to_center: Vec2,
}

impl HalleyWlState {
    fn wl_surface_for_node(&self, id: NodeId) -> Option<WlSurface> {
        for top in self.xdg_shell_state.toplevel_surfaces() {
            let wl = top.wl_surface().clone();
            if self.surface_to_node.get(&wl.id()).copied() == Some(id) {
                return Some(wl);
            }
        }
        None
    }

    fn update_selection_focus_from_surface(&self, surface: Option<&WlSurface>) {
        let client = surface.and_then(|wl| wl.client());
        set_data_device_focus(&self.display_handle, &self.seat, client.clone());
        set_primary_focus(&self.display_handle, &self.seat, client);
    }

    fn apply_wayland_focus_state(&mut self, id: Option<NodeId>) {
        let focus_surface = id.and_then(|fid| self.wl_surface_for_node(fid));
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, focus_surface.clone(), SERIAL_COUNTER.next_serial());
        }
        self.update_selection_focus_from_surface(focus_surface.as_ref());

        for top in self.xdg_shell_state.toplevel_surfaces() {
            let key = top.wl_surface().id();
            let focused = match id {
                Some(fid) => self.surface_to_node.get(&key).copied() == Some(fid),
                None => false,
            };
            top.with_pending_state(|s| {
                if focused {
                    s.states.set(xdg_toplevel::State::Activated);
                } else {
                    s.states.unset(xdg_toplevel::State::Activated);
                }
            });
            top.send_configure();
        }
    }

    fn reassert_wayland_keyboard_focus_if_drifted(&mut self, id: Option<NodeId>) {
        let desired_focus = id.and_then(|fid| self.wl_surface_for_node(fid));
        if let Some(keyboard) = self.seat.get_keyboard() {
            let current_focus = keyboard.current_focus();
            let matches = match (&current_focus, &desired_focus) {
                (Some(current), Some(desired)) => current.id() == desired.id(),
                (None, None) => true,
                _ => false,
            };
            if !matches {
                info!(
                    "keyboard focus drift detected; reasserting desired focus={:?} current={:?}",
                    desired_focus.as_ref().map(|wl| format!("{:?}", wl.id())),
                    current_focus.as_ref().map(|wl| format!("{:?}", wl.id()))
                );
                keyboard.set_focus(self, desired_focus.clone(), SERIAL_COUNTER.next_serial());
                self.update_selection_focus_from_surface(desired_focus.as_ref());
            }
        }
    }

    pub fn note_pan_activity(&mut self, now: Instant) {
        self.viewport_pan_anim = None;
        let now_ms = self.now_ms(now);
        self.pan_dominant_until_ms = now_ms.saturating_add(220);
        if self.tuning.restore_last_active_on_pan_return {
            if self.pan_restore_active_focus.is_none() {
                self.pan_restore_active_focus = self.last_focused_active_surface_node();
            }
        }
        self.suspend_overlap_resolve = false;
        self.suspend_state_checks = false;
    }

    pub fn set_pan_restore_focus_target(&mut self, id: NodeId) {
        self.pan_restore_active_focus = Some(id);
    }

    pub fn animate_viewport_center_to(&mut self, target_center: Vec2, now: Instant) -> bool {
        let from = self.viewport.center;
        let dx = target_center.x - from.x;
        let dy = target_center.y - from.y;
        if dx.abs() < 0.25 && dy.abs() < 0.25 {
            return false;
        }
        self.viewport_pan_anim = Some(ViewportPanAnim {
            start_ms: self.now_ms(now),
            duration_ms: 260,
            from_center: from,
            to_center: target_center,
        });
        true
    }

    pub(super) fn tick_viewport_pan_animation(&mut self, now_ms: u64) {
        let Some(anim) = &self.viewport_pan_anim else {
            return;
        };
        let dur = anim.duration_ms.max(1);
        let t = ((now_ms.saturating_sub(anim.start_ms)) as f32 / dur as f32).clamp(0.0, 1.0);
        let e = if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
        };
        self.viewport.center = Vec2 {
            x: anim.from_center.x + (anim.to_center.x - anim.from_center.x) * e,
            y: anim.from_center.y + (anim.to_center.y - anim.from_center.y) * e,
        };
        self.tuning.viewport_center = self.viewport.center;
        self.tuning.viewport_size = self.viewport.size;
        if t >= 1.0 {
            self.viewport_pan_anim = None;
        }
    }

    fn last_focused_active_surface_node(&self) -> Option<NodeId> {
        if let Some(id) = self.interaction_focus {
            if self.field.node(id).is_some_and(|n| {
                self.field.is_visible(id)
                    && n.kind == halley_core::field::NodeKind::Surface
                    && n.state == halley_core::field::NodeState::Active
            }) {
                return Some(id);
            }
        }
        self.last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                self.field.node(id).and_then(|n| {
                    (self.field.is_visible(id)
                        && n.kind == halley_core::field::NodeKind::Surface
                        && n.state == halley_core::field::NodeState::Active)
                        .then_some((id, at))
                })
            })
            .max_by_key(|(id, at)| (*at, id.as_u64()))
            .map(|(id, _)| id)
    }

    pub(super) fn restore_pan_return_active_focus(&mut self, now: Instant) {
        if !self.tuning.restore_last_active_on_pan_return {
            self.pan_restore_active_focus = None;
            return;
        }
        let Some(id) = self.pan_restore_active_focus else {
            return;
        };
        let Some(n) = self.field.node(id) else {
            self.pan_restore_active_focus = None;
            return;
        };
        if !self.field.is_visible(id) || n.kind != halley_core::field::NodeKind::Surface {
            self.pan_restore_active_focus = None;
            return;
        }
        if n.state == halley_core::field::NodeState::Active {
            self.pan_restore_active_focus = None;
            return;
        }

        let focus_ring = self.active_focus_ring();
        if focus_ring.zone(self.viewport.center, n.pos) != FocusZone::Inside {
            return;
        }

        let partner = self
            .field
            .dock_partner(id)
            .filter(|&pid| self.field.dock_partner(pid) == Some(id));

        let _ = self.field.set_decay_level(id, DecayLevel::Hot);
        self.mark_active_transition(id, now, 280);

        if let Some(pid) = partner {
            if self.field.is_visible(pid)
                && self.field.node(pid).is_some_and(|pn| {
                    pn.kind == halley_core::field::NodeKind::Surface
                        && pn.state != halley_core::field::NodeState::Active
                })
            {
                let _ = self.field.set_decay_level(pid, DecayLevel::Hot);
                self.mark_active_transition(pid, now, 280);
            }
        }

        self.set_interaction_focus(Some(id), 12_000, now);
        self.pan_restore_active_focus = None;
    }

    pub fn begin_resize_interaction(&mut self, id: NodeId, now: Instant) {
        self.resize_active = Some(id);
        self.resize_static_node = Some(id);
        self.resize_static_lock_pos = None;
        self.resize_static_until_ms = self.now_ms(now).saturating_add(60_000);
        self.suspend_overlap_resolve = true;
        self.suspend_state_checks = true;
        self.set_interaction_focus(Some(id), 60_000, now);
        let now_ms = self.now_ms(now);
        let _ = self.field.touch(id, now_ms);
        let _ = self.field.set_decay_level(id, DecayLevel::Hot);
    }

    pub fn end_resize_interaction(&mut self, now: Instant) {
        let ended = self.resize_active.take();
        if let Some(id) = ended {
            self.resize_static_node = Some(id);
            self.resize_static_lock_pos = self.field.node(id).map(|n| n.pos);
            self.resize_static_until_ms = self.now_ms(now).saturating_add(120);
            self.set_interaction_focus(Some(id), 30_000, now);
        } else {
            self.resize_static_lock_pos = None;
            self.set_interaction_focus(None, 0, now);
        }
        self.suspend_state_checks = false;
        self.suspend_overlap_resolve = false;
        self.enforce_docked_pairs();
        self.resolve_surface_overlap();
    }

    pub fn resolve_overlap_now(&mut self) {
        let saved_suspend = self.suspend_overlap_resolve;
        self.suspend_overlap_resolve = false;
        self.resolve_surface_overlap();
        self.suspend_overlap_resolve = saved_suspend;
    }

    pub fn set_last_active_size_now(&mut self, id: NodeId, size: Vec2) {
        self.last_active_size.insert(id, size);
    }

    pub fn set_interaction_focus(&mut self, id: Option<NodeId>, hold_ms: u64, now: Instant) {
        let prev = self.interaction_focus;
        if prev == id {
            if id.is_some() {
                let now_ms = self.now_ms(now);
                let requested_until = now_ms.saturating_add(hold_ms.max(1));
                self.interaction_focus_until_ms =
                    self.interaction_focus_until_ms.max(requested_until);
                self.reassert_wayland_keyboard_focus_if_drifted(id);
            } else {
                self.interaction_focus_until_ms = 0;
                self.reassert_wayland_keyboard_focus_if_drifted(None);
            }
            return;
        }
        self.interaction_focus = id;
        if id.is_some() {
            let now_ms = self.now_ms(now);
            self.interaction_focus_until_ms = now_ms.saturating_add(hold_ms.max(1));
            if let Some(fid) = id {
                if self.field.node(fid).is_some_and(|n| {
                    n.kind == halley_core::field::NodeKind::Surface && self.field.is_visible(fid)
                }) {
                    self.last_surface_focus_ms.insert(fid, now_ms);
                    if self.tuning.restore_last_active_on_pan_return {
                        self.pan_restore_active_focus = Some(fid);
                    }
                }
            }
        } else {
            self.interaction_focus_until_ms = 0;
        }
        if prev != id {
            info!(
                "interaction focus changed: {:?} -> {:?} (hold_ms={})",
                prev.map(|n| n.as_u64()),
                id.map(|n| n.as_u64()),
                hold_ms
            );
        }
        self.apply_wayland_focus_state(id);
    }

    pub fn last_focused_surface_node(&self) -> Option<NodeId> {
        if let Some(id) = self.interaction_focus {
            let valid = self.field.node(id).is_some_and(|n| {
                self.field.is_visible(id)
                    && n.kind == halley_core::field::NodeKind::Surface
                    && matches!(
                        n.state,
                        halley_core::field::NodeState::Active
                            | halley_core::field::NodeState::Node
                            | halley_core::field::NodeState::Preview
                    )
            });
            if valid {
                return Some(id);
            }
        }
        self.last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                self.field.node(id).and_then(|n| {
                    (self.field.is_visible(id)
                        && n.kind == halley_core::field::NodeKind::Surface
                        && matches!(
                            n.state,
                            halley_core::field::NodeState::Active
                                | halley_core::field::NodeState::Node
                                | halley_core::field::NodeState::Preview
                        ))
                    .then_some((id, at))
                })
            })
            .max_by_key(|(id, at)| (*at, id.as_u64()))
            .map(|(id, _)| id)
    }

    pub fn last_input_surface_node(&self) -> Option<NodeId> {
        if let Some(id) = self.interaction_focus {
            let valid = self.field.node(id).is_some_and(|n| {
                self.field.is_visible(id) && n.kind == halley_core::field::NodeKind::Surface
            });
            if valid {
                return Some(id);
            }
        }
        self.last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                self.field.node(id).and_then(|n| {
                    (self.field.is_visible(id) && n.kind == halley_core::field::NodeKind::Surface)
                        .then_some((id, at))
                })
            })
            .max_by_key(|(id, at)| (*at, id.as_u64()))
            .map(|(id, _)| id)
    }

    pub fn toggle_last_focused_surface_node(&mut self, now: Instant) -> Option<NodeId> {
        let id = self.last_focused_surface_node()?;
        let partner = self
            .field
            .dock_partner(id)
            .filter(|&pid| self.field.dock_partner(pid) == Some(id));

        let state = self.field.node(id)?.state.clone();
        match state {
            halley_core::field::NodeState::Active => {
                let _ = self.field.set_decay_level(id, DecayLevel::Cold);
                if let Some(pid) = partner {
                    let _ = self.field.set_decay_level(pid, DecayLevel::Cold);
                }
                self.set_interaction_focus(Some(id), 30_000, now);
                Some(id)
            }
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Preview => {
                let _ = self.field.set_decay_level(id, DecayLevel::Hot);
                if let Some(pid) = partner {
                    let _ = self.field.set_decay_level(pid, DecayLevel::Hot);
                }
                self.mark_active_transition(id, now, 360);
                if let Some(pid) = partner {
                    self.mark_active_transition(pid, now, 360);
                }
                self.set_interaction_focus(Some(id), 30_000, now);
                Some(id)
            }
            _ => None,
        }
    }
}
