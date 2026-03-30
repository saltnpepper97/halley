use super::*;
use crate::compositor::{focus, fullscreen, interaction, monitor, spawn, workspace};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::input::pointer::PointerHandle;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgDecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{
    Client, Resource, protocol::wl_surface::WlSurface,
};
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::compositor::{add_blocker, with_states};
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufGlobal, DmabufHandler, ImportNotifier};
use smithay::wayland::drm_syncobj::{DrmSyncPoint, DrmSyncobjCachedState, DrmSyncobjHandler};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::pointer_constraints::with_pointer_constraint;
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState,
};
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::shell::wlr_layer::{
    LayerSurface, LayerSurfaceConfigure, WlrLayerShellHandler, WlrLayerShellState,
};

pub(super) fn initial_toplevel_size(
    st: &mut Halley,
    toplevel: &ToplevelSurface,
) -> spawn::reveal::InitialToplevelSize {
    let mut ctx = st.spawn_ctx();
    spawn::reveal::initial_toplevel_size(&mut ctx, toplevel)
}

pub(super) fn constrain_layer_popup(
    st: &mut Halley,
    popup: &PopupSurface,
    positioner: PositionerState,
) {
    let mut ctx = st.layer_shell_ctx();
    monitor::layer_shell::constrain_layer_popup(&mut ctx, popup, positioner);
}

impl SeatHandler for Halley {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.platform.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let now = Instant::now();
        fullscreen::system::on_seat_focus_changed(&mut self.fullscreen_ctx(), focused, now);
        focus::system::on_seat_focus_changed(&mut self.focus_ctx(), seat, focused);
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.platform.cursor_image_status = image;
    }
}

delegate_seat!(Halley);
delegate_pointer_constraints!(Halley);
delegate_relative_pointer!(Halley);
delegate_drm_syncobj!(Halley);
delegate_idle_notify!(Halley);

impl SelectionHandler for Halley {
    type SelectionUserData = ();
}

impl IdleNotifierHandler for Halley {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.platform.idle_notifier_state
    }
}

impl DataDeviceHandler for Halley {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.platform.data_device_state
    }
}

impl ClientDndGrabHandler for Halley {}

impl ServerDndGrabHandler for Halley {
    fn send(&mut self, _mime_type: String, _fd: std::os::unix::io::OwnedFd, _seat: Seat<Self>) {}
}

delegate_data_device!(Halley);

impl CompositorHandler for Halley {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.platform.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("ClientState missing")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.install_drm_syncobj_blocker(surface);
        self.platform.popup_manager.commit(surface);
        workspace::lifecycle::on_surface_commit(
            &mut self.surface_lifecycle_ctx(),
            surface,
            Instant::now(),
        );
    }
}

delegate_compositor!(Halley);
delegate_viewporter!(Halley);

impl ShmHandler for Halley {
    fn shm_state(&self) -> &ShmState {
        &self.platform.shm_state
    }
}

impl BufferHandler for Halley {
    fn buffer_destroyed(
        &mut self,
        _buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    ) {
    }
}

delegate_shm!(Halley);

impl DmabufHandler for Halley {
    fn dmabuf_state(&mut self) -> &mut smithay::wayland::dmabuf::DmabufState {
        &mut self.platform.dmabuf_state
    }

    fn dmabuf_imported(&mut self, global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.platform.dmabuf_global != Some(*global) {
            notifier.failed();
            return;
        }

        let Some(importer) = self.platform.dmabuf_importer.as_ref() else {
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

impl DrmSyncobjHandler for Halley {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.platform.drm_syncobj_state.as_mut()
    }
}

impl PointerConstraintsHandler for Halley {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        if pointer.current_focus().as_ref() != Some(surface) {
            return;
        }
        with_pointer_constraint(surface, pointer, |constraint| {
            if let Some(constraint) = constraint
                && !constraint.is_active()
            {
                constraint.activate();
            }
        });
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) {
        interaction::pointer::cursor_position_hint(
            &mut self.pointer_ctx(),
            surface,
            pointer,
            location,
        );
    }
}

#[cfg(test)]
mod tests {}

impl PrimarySelectionHandler for Halley {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.platform.primary_selection_state
    }
}

delegate_primary_selection!(Halley);

impl DataControlHandler for Halley {
    fn data_control_state(&self) -> &DataControlState {
        &self.platform.data_control_state
    }
}

delegate_data_control!(Halley);
