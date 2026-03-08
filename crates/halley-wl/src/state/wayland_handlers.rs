use super::*;
use eventline::info;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{Client, Resource, protocol::wl_surface::WlSurface};
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
};
use smithay::wayland::selection::wlr_data_control::DataControlState;

fn client_for_surface(surface: Option<&WlSurface>) -> Option<Client> {
    surface.and_then(|wl| wl.client())
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
    fn send(
        &mut self,
        _mime_type: String,
        _fd: std::os::unix::io::OwnedFd,
        _seat: Seat<Self>,
    ) {
    }
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
        self.note_commit(surface, Instant::now());
    }
}

delegate_compositor!(HalleyWlState);

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

impl XdgShellHandler for HalleyWlState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        // You MUST send an initial configure or many clients won’t map.
        // We'll pick a sane default until we have real output sizing.
        toplevel.with_pending_state(|s| {
            s.size = Some((900, 600).into());
            s.states.set(xdg_toplevel::State::Activated);
        });
        toplevel.send_configure();

        // Create a core node for the underlying wl_surface.
        let wl = toplevel.wl_surface().clone();
        let id = self.ensure_node_for_surface(&wl, "toplevel", (900, 600));
        let now = Instant::now();
        let _ = self.field.touch(id, self.now_ms(now));
        // New windows should be immediately typeable and stay focused until
        // the user explicitly focuses another surface.
        self.set_interaction_focus(Some(id), 30_000, now);
        self.resolve_surface_overlap();
    }

    fn new_popup(&mut self, popup: PopupSurface, _positioner: PositionerState) {
        let _ = popup.send_configure();
    }

    fn move_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
    ) {
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

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        _positioner: PositionerState,
        token: u32,
    ) {
        surface.send_repositioned(token);
        let _ = surface.send_configure();
    }
}

delegate_xdg_shell!(HalleyWlState);

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
