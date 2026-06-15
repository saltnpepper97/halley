use super::*;
use crate::compositor::{actions, fullscreen, monitor, spawn, surface, workspace};
use crate::compositor::spawn::rules::ResolvedInitialWindowRule;
use smithay::desktop::{PopupKind, find_popup_root_surface};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgDecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface};
use smithay::utils::{Logical, Rectangle};

fn popup_positioner_geometry(positioner: PositionerState) -> Rectangle<i32, Logical> {
    positioner.get_geometry()
}

fn configure_popup_position(st: &mut Halley, popup: &PopupSurface, positioner: PositionerState) {
    popup.with_pending_state(|state| {
        state.positioner = positioner;
        state.geometry = popup_positioner_geometry(positioner);
    });
    super::handlers::constrain_layer_popup(st, popup, positioner);
}

fn restore_drag_offset_for_maximized_move(
    st: &Halley,
    node_id: halley_core::field::NodeId,
    monitor: &str,
    press_global_sx: f32,
    press_global_sy: f32,
    now: std::time::Instant,
) -> Option<halley_core::field::Vec2> {
    let session = st.model.workspace_state.maximize_sessions.get(monitor)?;
    let snapshot = session.node_snapshots.get(&node_id)?;
    let (visual_center, visual_size) =
        crate::compositor::workspace::state::maximized_visual_for_node_on_monitor_at(
            st, node_id, monitor, now,
        )?;
    let (ws_w, ws_h, local_sx, local_sy) =
        st.local_screen_in_monitor(monitor, press_global_sx, press_global_sy);
    let pointer_world = crate::spatial::screen_to_world(st, ws_w, ws_h, local_sx, local_sy);
    let ratio_x = ((pointer_world.x - (visual_center.x - visual_size.x * 0.5))
        / visual_size.x.max(1.0))
    .clamp(0.0, 1.0);
    let ratio_y = ((pointer_world.y - (visual_center.y - visual_size.y * 0.5))
        / visual_size.y.max(1.0))
    .clamp(0.0, 1.0);

    Some(halley_core::field::Vec2 {
        x: (ratio_x - 0.5) * snapshot.size.x,
        y: (ratio_y - 0.5) * snapshot.size.y,
    })
}

impl XdgDecorationHandler for Halley {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        let mode = self.preferred_xdg_decoration_mode();
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
            self.apply_toplevel_tiled_hint(state);
        });
        toplevel.send_configure();
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: XdgDecorationMode) {
        let mode = self.preferred_xdg_decoration_mode();
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
            self.apply_toplevel_tiled_hint(state);
        });
        toplevel.send_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        let mode = self.preferred_xdg_decoration_mode();
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
            self.apply_toplevel_tiled_hint(state);
        });
        toplevel.send_configure();
    }
}

delegate_xdg_decoration!(Halley);

impl XdgShellHandler for Halley {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.platform.xdg_shell_state
    }

    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        let intent = spawn::rules::resolve_initial_window_intent(self, &toplevel);
        let initial_size = super::handlers::initial_toplevel_size(self, &toplevel, &intent);
        let is_default = intent.rule == ResolvedInitialWindowRule::default()
            && intent.parent_node.is_none()
            && !intent.prefer_app_intent;
        let monitor = if is_default {
            self.model
                .spawn_state
                .pending_spawn_monitor
                .clone()
                .filter(|m| self.model.monitor_state.monitors.contains_key(m))
                .unwrap_or_else(|| self.spawn_target_monitor_for_intent(&intent))
        } else {
            self.spawn_target_monitor_for_intent(&intent)
        };

        let view = self.usable_viewport_for_monitor(&monitor);
        let bounds_w = view.size.x as i32;
        let bounds_h = view.size.y as i32;
        toplevel.with_pending_state(|s| {
            if let Some((w, h)) = initial_size.configure_size {
                s.size = Some((w, h).into());
                s.bounds = Some((bounds_w, bounds_h).into());
            }
            s.decoration_mode = Some(self.preferred_xdg_decoration_mode());
            self.apply_toplevel_tiled_hint(s);
        });
        toplevel.send_configure();

        let is_transient = toplevel.parent().is_some();
        let wl = toplevel.wl_surface().clone();
        let id = workspace::lifecycle::ensure_node_for_surface(
            &mut self.surface_lifecycle_ctx(),
            &wl,
            "toplevel",
            initial_size.node_size,
            &intent,
        );
        let now = Instant::now();
        let _ = crate::protocol::wayland::activation::consume_pending_surface_activation(self, &wl);
        let node_monitor = self.model.monitor_state.node_monitor.get(&id).cloned();
        let handled_by_active_cluster = self
            .model
            .field
            .cluster_id_for_member_public(id)
            .zip(node_monitor.as_deref())
            .is_some_and(|(cid, monitor)| {
                self.active_cluster_workspace_for_monitor(monitor) == Some(cid)
            });
        if !is_transient && !handled_by_active_cluster {
            self.model.spawn_state.pending_initial_reveal.insert(id);
            let _ = self.model.field.set_detached(id, true);
        }
        if !handled_by_active_cluster {
            let _ = self.model.field.touch(id, self.now_ms(now));
        }
        spawn::reveal::reveal_new_toplevel_node(&mut self.spawn_ctx(), id, is_transient, now);
        if !handled_by_active_cluster {
            if !self.model.spawn_state.pending_initial_reveal.contains(&id) {
                self.resolve_surface_overlap();
            }
            self.request_maintenance();
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        workspace::lifecycle::refresh_surface_identity(
            &mut self.surface_lifecycle_ctx(),
            surface.wl_surface(),
            "Window",
        );
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        workspace::lifecycle::refresh_surface_identity(
            &mut self.surface_lifecycle_ctx(),
            surface.wl_surface(),
            "Window",
        );
    }

    fn new_popup(&mut self, popup: PopupSurface, positioner: PositionerState) {
        let _ = self
            .platform
            .popup_manager
            .track_popup(PopupKind::from(popup.clone()));
        configure_popup_position(self, &popup, positioner);
        let _ = popup.send_configure();
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.model.surface_to_node.get(&key).copied() else {
            return;
        };
        let now = Instant::now();
        let restore_drag_offset = self
            .input
            .interaction_state
            .last_pointer_screen_global
            .and_then(|(press_global_sx, press_global_sy)| {
                crate::compositor::workspace::state::maximize_session_monitor_for_node(
                    self, node_id,
                )
                .and_then(|monitor| {
                    restore_drag_offset_for_maximized_move(
                        self,
                        node_id,
                        monitor.as_str(),
                        press_global_sx,
                        press_global_sy,
                        now,
                    )
                })
            });
        if let Some(monitor) =
            crate::compositor::workspace::state::maximize_session_monitor_for_node(self, node_id)
        {
            let _ = crate::compositor::workspace::state::abort_maximize_session_for_monitor(
                self,
                monitor.as_str(),
            );
        }
        let focus_target = surface::stack_focus_target_for_node(self, node_id).unwrap_or(node_id);
        self.set_recent_top_node(focus_target, now + std::time::Duration::from_millis(1200));
        self.set_interaction_focus(Some(focus_target), 700, now);
        if let Some((press_global_sx, press_global_sy)) =
            self.input.interaction_state.last_pointer_screen_global
        {
            self.input.interaction_state.pending_move_press =
                Some(crate::compositor::interaction::state::PendingMovePress {
                    node_id,
                    press_global_sx,
                    press_global_sy,
                    workspace_active: self.has_active_cluster_workspace(),
                    restore_drag_offset,
                });
        }
        self.request_maintenance();
    }

    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, output: Option<WlOutput>) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.model.surface_to_node.get(&key).copied() else {
            surface.send_configure();
            return;
        };
        fullscreen::system::enter_xdg_fullscreen(
            &mut self.fullscreen_ctx(),
            node_id,
            output,
            Instant::now(),
        );
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.model.surface_to_node.get(&key).copied() else {
            surface.send_configure();
            return;
        };
        fullscreen::system::exit_xdg_fullscreen(
            &mut self.fullscreen_ctx(),
            node_id,
            Instant::now(),
        );
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.model.surface_to_node.get(&key).copied() else {
            surface.send_configure();
            return;
        };
        let monitor = self
            .model
            .monitor_state
            .node_monitor
            .get(&node_id)
            .cloned()
            .unwrap_or_else(|| self.focused_monitor().to_string());
        let _ = actions::window::toggle_node_state(self, node_id, Instant::now(), monitor.as_str());
        surface.send_configure();
    }

    fn grab(&mut self, surface: PopupSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        let popup = PopupKind::from(surface);
        if let Ok(root) = find_popup_root_surface(&popup) {
            let _ = self.platform.popup_manager.grab_popup::<Self>(
                root,
                popup,
                &self.platform.seat,
                serial,
            );
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        configure_popup_position(self, &surface, positioner);
        surface.send_repositioned(token);
        self.request_maintenance();
    }

    fn popup_destroyed(&mut self, _surface: PopupSurface) {
        self.platform.popup_manager.cleanup();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        workspace::lifecycle::on_toplevel_destroyed(&mut self.surface_lifecycle_ctx(), surface);
    }
}

delegate_xdg_shell!(Halley);

impl XdgActivationHandler for Halley {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.platform.xdg_activation_state
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        crate::protocol::wayland::activation::request_surface_activation(
            self,
            &surface,
            token.as_str(),
            &token_data,
            Instant::now(),
        );
    }
}

delegate_xdg_activation!(Halley);

#[cfg(test)]
mod tests {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner;
    use smithay::utils::{Logical, Rectangle, Size};

    use super::*;

    #[test]
    fn normal_popup_geometry_uses_client_positioner() {
        let mut positioner = PositionerState {
            rect_size: Size::<i32, Logical>::from((120, 48)),
            anchor_rect: Rectangle::<i32, Logical>::new((24, 30).into(), (80, 20).into()),
            anchor_edges: xdg_positioner::Anchor::BottomLeft,
            gravity: xdg_positioner::Gravity::BottomRight,
            ..Default::default()
        };
        positioner.offset = (7, 11).into();

        let geometry = popup_positioner_geometry(positioner);

        assert_eq!(geometry, positioner.get_geometry());
        assert_ne!(geometry.loc, (0, 0).into());
    }
}

impl WlrLayerShellHandler for Halley {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.platform.wlr_layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        monitor::layer_shell::register_layer_surface(
            &mut self.layer_shell_ctx(),
            surface,
            output,
            layer,
            namespace,
        );
    }

    fn ack_configure(&mut self, _surface: WlSurface, _configure: LayerSurfaceConfigure) {}

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        monitor::layer_shell::remove_layer_surface(&mut self.layer_shell_ctx(), &surface);
    }
}

delegate_layer_shell!(Halley);

impl OutputHandler for Halley {}

delegate_output!(Halley);
