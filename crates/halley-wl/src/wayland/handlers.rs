use super::*;
use eventline::info;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::desktop::{PopupKind, find_popup_root_surface, utils::bbox_from_surface_tree};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{
    Client, Resource, protocol::wl_output::WlOutput, protocol::wl_surface::WlSurface,
};
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufGlobal, DmabufHandler, ImportNotifier};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
};
use smithay::wayland::compositor::with_states;
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface, LayerSurfaceConfigure, WlrLayerShellHandler, WlrLayerShellState,
};
use smithay::wayland::shell::xdg::SurfaceCachedState;

fn client_for_surface(surface: Option<&WlSurface>) -> Option<Client> {
    surface.and_then(|wl| wl.client())
}

fn initial_toplevel_size(st: &HalleyWlState, toplevel: &ToplevelSurface) -> (i32, i32) {
    let wl = toplevel.wl_surface();

    let geometry = with_states(wl, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
    });
    if let Some(geometry) = geometry {
        return (geometry.size.w.max(96), geometry.size.h.max(72));
    }

    if let Some(size) = toplevel.current_state().size {
        return (size.w.max(96), size.h.max(72));
    }

    let bbox = bbox_from_surface_tree(wl, (0, 0));
    if bbox.size.w > 0 && bbox.size.h > 0 {
        return (bbox.size.w.max(96), bbox.size.h.max(72));
    }

    let fallback_w = (st.viewport.size.x * 0.4).round() as i32;
    let fallback_h = (st.viewport.size.y * 0.45).round() as i32;
    (fallback_w.max(96), fallback_h.max(72))
}

impl SeatHandler for HalleyWlState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        info!(
            "seat focus_changed -> {:?}",
            focused.map(|wl| format!("{:?}", wl.id()))
        );

        let client = client_for_surface(focused);
        set_data_device_focus(&self.display_handle, seat, client.clone());
        set_primary_focus(&self.display_handle, seat, client);
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_image_status = image;
    }
}

delegate_seat!(HalleyWlState);

impl SelectionHandler for HalleyWlState {
    type SelectionUserData = ();
}

impl DataDeviceHandler for HalleyWlState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for HalleyWlState {}

impl ServerDndGrabHandler for HalleyWlState {
    fn send(&mut self, _mime_type: String, _fd: std::os::unix::io::OwnedFd, _seat: Seat<Self>) {}
}

delegate_data_device!(HalleyWlState);

impl CompositorHandler for HalleyWlState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("ClientState missing")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.popup_manager.commit(surface);
        self.note_commit(surface, Instant::now());
    }
}

delegate_compositor!(HalleyWlState);
delegate_viewporter!(HalleyWlState);

impl ShmHandler for HalleyWlState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl BufferHandler for HalleyWlState {
    fn buffer_destroyed(
        &mut self,
        _buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    ) {
    }
}

delegate_shm!(HalleyWlState);

impl DmabufHandler for HalleyWlState {
    fn dmabuf_state(&mut self) -> &mut smithay::wayland::dmabuf::DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(&mut self, global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.dmabuf_global != Some(*global) {
            notifier.failed();
            return;
        }

        let Some(importer) = self.dmabuf_importer.as_ref() else {
            notifier.failed();
            return;
        };

        if importer.import_dmabuf(&dmabuf).is_ok() {
            let _ = notifier.successful::<Self>();
        } else {
            notifier.failed();
        }
    }

    fn new_surface_feedback(
        &mut self,
        _surface: &WlSurface,
        _global: &DmabufGlobal,
    ) -> Option<DmabufFeedback> {
        None
    }
}

impl XdgShellHandler for HalleyWlState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        // You MUST send an initial configure or many clients won’t map.
        // Leave size unset so clients can choose their own initial dimensions.
        toplevel.with_pending_state(|s| {
            s.states.set(xdg_toplevel::State::Activated);
        });
        toplevel.send_configure();

        // Detect child/transient toplevels (e.g. browser video-preview popups).
        // These are spawned by the client as logical children of an existing
        // surface and should not disturb the spatial layout of their parent.
        let is_transient = toplevel.parent().is_some();

        // Create a core node for the underlying wl_surface.
        let wl = toplevel.wl_surface().clone();
        let id = self.ensure_node_for_surface(&wl, "toplevel", initial_toplevel_size(self, &toplevel));
        let now = Instant::now();
        let _ = self.field.touch(id, self.now_ms(now));
        // New windows should be immediately typeable and stay focused until
        // the user explicitly focuses another surface.
        self.set_interaction_focus(Some(id), 30_000, now);

        if is_transient {
            // Cancel the delayed activation that would call
            // push_neighbors_for_activation. The surface is already Active and
            // Hot from ensure_node_for_surface; we just don’t want it shoving
            // the parent window aside when the preview pops up.
            self.pending_spawn_activate_at_ms.remove(&id);
            // Still play the appear animation so it doesn’t just snap in.
            self.mark_active_transition(id, now, 620);
        }

        self.resolve_surface_overlap();
    }

    fn new_popup(&mut self, popup: PopupSurface, _positioner: PositionerState) {
        let _ = self
            .popup_manager
            .track_popup(PopupKind::from(popup.clone()));
        let _ = popup.send_configure();
    }

    fn move_request(&mut self, _surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // Interactive move is compositor-driven (configured modifier + drag), not
        // client-request driven.
    }

    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
        // Interactive resize is compositor-driven (configured modifier + right-click),
        // not client-request driven.
    }

    fn grab(&mut self, surface: PopupSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        let popup = PopupKind::from(surface);
        if let Ok(root) = find_popup_root_surface(&popup) {
            let _ = self
                .popup_manager
                .grab_popup::<Self>(root, popup, &self.seat, serial);
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        _positioner: PositionerState,
        token: u32,
    ) {
        surface.send_repositioned(token);
        let _ = surface.send_configure();
    }

    fn popup_destroyed(&mut self, _surface: PopupSurface) {
        self.popup_manager.cleanup();
    }
}

delegate_xdg_shell!(HalleyWlState);

impl WlrLayerShellHandler for HalleyWlState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.wlr_layer_shell_state
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

delegate_layer_shell!(HalleyWlState);

impl OutputHandler for HalleyWlState {}

delegate_output!(HalleyWlState);

impl PrimarySelectionHandler for HalleyWlState {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

delegate_primary_selection!(HalleyWlState);

impl DataControlHandler for HalleyWlState {
    fn data_control_state(&self) -> &DataControlState {
        &self.data_control_state
    }
}

delegate_data_control!(HalleyWlState);
