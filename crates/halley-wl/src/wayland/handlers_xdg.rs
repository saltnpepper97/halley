use super::*;
use smithay::desktop::{PopupKind, find_popup_root_surface};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgDecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface};

impl XdgDecorationHandler for Halley {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        let mode = self.preferred_xdg_decoration_mode();
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
            self.apply_toplevel_tiled_hint(state);
        });
        toplevel.send_configure();
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: XdgDecorationMode) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
            self.apply_toplevel_tiled_hint(state);
        });
        toplevel.send_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = None;
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
        let initial_size = super::handlers::initial_toplevel_size(self, &toplevel);
        toplevel.with_pending_state(|s| {
            s.states.set(xdg_toplevel::State::Activated);
            if let Some((w, h)) = initial_size.configure_size {
                s.size = Some((w, h).into());
            }
            s.decoration_mode = Some(self.preferred_xdg_decoration_mode());
            self.apply_toplevel_tiled_hint(s);
        });
        toplevel.send_configure();

        let is_transient = toplevel.parent().is_some();
        let wl = toplevel.wl_surface().clone();
        let id = self.ensure_node_for_surface(&wl, "toplevel", initial_size.node_size);
        let now = Instant::now();
        let node_monitor = self.model.monitor_state.node_monitor.get(&id).cloned();
        let handled_by_active_cluster = self
            .model
            .field
            .cluster_id_for_member_public(id)
            .zip(node_monitor.as_deref())
            .is_some_and(|(cid, monitor)| {
                self.active_cluster_workspace_for_monitor(monitor) == Some(cid)
            });
        if !handled_by_active_cluster {
            let _ = self.model.field.touch(id, self.now_ms(now));
        }
        self.reveal_new_toplevel_node(id, is_transient, now);
        if !handled_by_active_cluster {
            self.resolve_surface_overlap();
            self.request_maintenance();
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.refresh_node_identity_for_surface(surface.wl_surface(), "Window");
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.refresh_node_identity_for_surface(surface.wl_surface(), "Window");
    }

    fn new_popup(&mut self, popup: PopupSurface, positioner: PositionerState) {
        let _ = self
            .platform
            .popup_manager
            .track_popup(PopupKind::from(popup.clone()));
        super::handlers::constrain_layer_popup(self, &popup, positioner);
        let _ = popup.send_configure();
    }

    fn move_request(&mut self, _surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
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
        self.enter_xdg_fullscreen(node_id, output, Instant::now());
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.model.surface_to_node.get(&key).copied() else {
            surface.send_configure();
            return;
        };
        self.exit_xdg_fullscreen(node_id, Instant::now());
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
        super::handlers::constrain_layer_popup(self, &surface, positioner);
        surface.send_repositioned(token);
        let _ = surface.send_configure();
    }

    fn popup_destroyed(&mut self, _surface: PopupSurface) {
        self.platform.popup_manager.cleanup();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let key = surface.wl_surface().id();
        let closing_id = self.model.surface_to_node.get(&key).copied();
        let had_keyboard_focus = self
            .platform
            .seat
            .get_keyboard()
            .and_then(|kb| kb.current_focus())
            .is_some_and(|focused| focused.id() == key);
        let had_pointer_focus = self
            .platform
            .seat
            .get_pointer()
            .and_then(|ptr| ptr.current_focus())
            .is_some_and(|focused| focused.id() == key);
        let focused_monitor = self
            .model
            .surface_to_node
            .get(&key)
            .and_then(|id| self.model.monitor_state.node_monitor.get(id))
            .cloned();

        if had_keyboard_focus || had_pointer_focus {
            eventline::info!(
                "toplevel_destroyed with active focus (keyboard={} pointer={}); scheduling input state reset",
                had_keyboard_focus,
                had_pointer_focus
            );
            self.input.interaction_state.reset_input_state_requested = true;
            if let Some(ref focused_monitor) = focused_monitor {
                self.model.spawn_state.pending_spawn_monitor = Some(focused_monitor.clone());
                eventline::info!(
                    "pending spawn monitor latched from destroyed toplevel: {}",
                    focused_monitor
                );
            }
        }

        if had_keyboard_focus {
            self.clear_keyboard_focus();
        }

        if had_keyboard_focus
            && self.runtime.tuning.close_restore_focus
            && let (Some(closing_id), Some(focused_monitor)) =
                (closing_id, focused_monitor.as_deref())
        {
            let now = Instant::now();
            if self
                .active_cluster_workspace_for_monitor(focused_monitor)
                .is_some()
            {
                if let Some(previous) =
                    self.previous_window_from_trail_on_close(focused_monitor, closing_id)
                {
                    self.set_interaction_focus(Some(previous), 30_000, now);
                } else if let Some(fallback) = self
                    .last_focused_surface_node_for_monitor(focused_monitor)
                    .filter(|&id| id != closing_id)
                {
                    self.set_interaction_focus(Some(fallback), 30_000, now);
                }
            } else if let Some(previous) =
                self.previous_window_from_trail_on_close(focused_monitor, closing_id)
            {
                let _ = self.restore_focus_to_node_after_close(focused_monitor, previous, now);
            } else if let Some(fallback) = self
                .last_focused_surface_node_for_monitor(focused_monitor)
                .filter(|&id| id != closing_id)
                .or_else(|| {
                    self.last_focused_surface_node()
                        .filter(|&id| id != closing_id)
                })
            {
                let _ = self.restore_focus_to_node_after_close(focused_monitor, fallback, now);
            }
        } else if had_keyboard_focus
            && !self.runtime.tuning.close_restore_focus
            && let Some(focused_monitor) = focused_monitor.as_deref()
        {
            self.model
                .focus_state
                .blocked_monitor_focus_restore
                .insert(focused_monitor.to_string());
        }
        if had_pointer_focus {
            self.clear_pointer_focus();
        }
    }
}

delegate_xdg_shell!(Halley);

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
        self.register_layer_surface(surface, output, layer, namespace);
    }

    fn ack_configure(&mut self, _surface: WlSurface, _configure: LayerSurfaceConfigure) {}

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        self.remove_layer_surface(&surface);
    }
}

delegate_layer_shell!(Halley);

impl OutputHandler for Halley {}

delegate_output!(Halley);
