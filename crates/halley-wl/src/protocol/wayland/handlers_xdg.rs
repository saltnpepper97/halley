use super::*;
use crate::compositor::{fullscreen, monitor, spawn, workspace};
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
        let id = workspace::lifecycle::ensure_node_for_surface(
            &mut self.surface_lifecycle_ctx(),
            &wl,
            "toplevel",
            initial_size.node_size,
            &intent,
        );
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
        spawn::reveal::reveal_new_toplevel_node(&mut self.spawn_ctx(), id, is_transient, now);
        if !handled_by_active_cluster {
            self.resolve_surface_overlap();
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
        workspace::lifecycle::on_toplevel_destroyed(&mut self.surface_lifecycle_ctx(), surface);
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
