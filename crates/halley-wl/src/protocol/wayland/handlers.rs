use super::*;
use crate::compositor::{focus, fullscreen, interaction, monitor, spawn, workspace};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::input::TabletToolDescriptor;
use smithay::input::pointer::PointerHandle;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgDecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{
    Client, Resource, protocol::wl_surface::WlSurface,
};
use smithay::wayland::compositor::{add_blocker, with_states};
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufGlobal, DmabufHandler, ImportNotifier};
use smithay::wayland::drm_syncobj::{DrmSyncPoint, DrmSyncobjCachedState, DrmSyncobjHandler};
use smithay::wayland::fractional_scale::FractionalScaleHandler;
use smithay::wayland::output::OutputHandler;
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState,
};
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::shell::wlr_layer::{
    LayerSurface, LayerSurfaceConfigure, WlrLayerShellHandler, WlrLayerShellState,
};
use smithay::wayland::tablet_manager::TabletSeatHandler;

pub(super) fn initial_toplevel_size(
    st: &mut Halley,
    toplevel: &ToplevelSurface,
    intent: &spawn::rules::InitialWindowIntent,
) -> spawn::reveal::InitialToplevelSize {
    let mut ctx = crate::compositor::ctx::spawn_ctx(st);
    spawn::reveal::initial_toplevel_size(&mut ctx, toplevel, intent)
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
        fullscreen::system::on_seat_focus_changed(self, focused, now);
        focus::system::on_seat_focus_changed(
            &crate::compositor::ctx::focus_ctx(self),
            seat,
            focused,
        );
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.platform.cursor_manager.set_cursor_image(image);
        crate::compositor::platform::refresh_cursor_surface_outputs(self);
        self.runtime.tty_redraw_all = true;
        self.request_maintenance();
    }
}

delegate_seat!(Halley);
delegate_cursor_shape!(Halley);
delegate_pointer_constraints!(Halley);
delegate_pointer_gestures!(Halley);
delegate_relative_pointer!(Halley);
delegate_drm_syncobj!(Halley);
delegate_idle_notify!(Halley);

impl TabletSeatHandler for Halley {
    fn tablet_tool_image(&mut self, _tool: &TabletToolDescriptor, image: CursorImageStatus) {
        self.platform.cursor_manager.set_cursor_image(image);
        crate::compositor::platform::refresh_cursor_surface_outputs(self);
        self.runtime.tty_redraw_all = true;
        self.request_maintenance();
    }
}

impl SelectionHandler for Halley {
    type SelectionUserData = ();
}

impl IdleNotifierHandler for Halley {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.platform.idle_notifier_state
    }
}

impl DataDeviceHandler for Halley {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.platform.data_device_state
    }
}

impl WaylandDndGrabHandler for Halley {
    // Start the actual server-side drag-and-drop grab in response to a client's
    // `wl_data_device.start_drag`. The default trait impl just cancels the source,
    // which silently kills every client DnD (Firefox tab moves, file/clip drops,
    // etc.); we must promote the implicit pointer/touch grab into a `DnDGrab`.
    fn dnd_requested<S: smithay::input::dnd::Source>(
        &mut self,
        source: S,
        _icon: Option<WlSurface>,
        seat: smithay::input::Seat<Self>,
        serial: smithay::utils::Serial,
        type_: smithay::input::dnd::GrabType,
    ) {
        let dh = self.platform.display_handle.clone();
        match type_ {
            smithay::input::dnd::GrabType::Pointer => {
                match seat.get_pointer().and_then(|pointer| {
                    pointer
                        .grab_start_data()
                        .map(|start_data| (pointer, start_data))
                }) {
                    Some((pointer, start_data)) => {
                        let grab = smithay::input::dnd::DnDGrab::new_pointer(
                            &dh, start_data, source, seat,
                        );
                        pointer.set_grab(self, grab, serial, smithay::input::pointer::Focus::Keep);
                    }
                    None => source.cancel(),
                }
            }
            smithay::input::dnd::GrabType::Touch => {
                match seat.get_touch().and_then(|touch| {
                    touch
                        .grab_start_data()
                        .map(|start_data| (touch, start_data))
                }) {
                    Some((touch, start_data)) => {
                        let grab =
                            smithay::input::dnd::DnDGrab::new_touch(&dh, start_data, source, seat);
                        touch.set_grab(self, grab, serial);
                    }
                    None => source.cancel(),
                }
            }
        }
    }
}

impl smithay::input::dnd::DndGrabHandler for Halley {}

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
        crate::compositor::platform::install_drm_syncobj_blocker(self, surface);
        self.platform.popup_manager.commit(surface);
        if crate::render::handle_cursor_surface_commit(
            self.platform.cursor_manager.cursor_image(),
            surface,
        ) {
            crate::compositor::platform::refresh_cursor_surface_outputs(self);
            let output_name = self
                .input
                .interaction_state
                .cursor.last_screen_global
                .and_then(|(sx, sy)| self.monitor_for_screen(sx, sy))
                .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone());
            self.request_tty_redraw_for_monitor(output_name.as_str());
            self.request_maintenance();
        }
        workspace::lifecycle::on_surface_commit(
            &mut self.surface_lifecycle_ctx(),
            surface,
            Instant::now(),
        );
    }
}

delegate_compositor!(Halley);
delegate_viewporter!(Halley);

impl FractionalScaleHandler for Halley {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        monitor::state::refresh_surface_preferred_scale(self, &surface);
    }
}

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
        surface: &WlSurface,
        global: &DmabufGlobal,
    ) -> Option<DmabufFeedback> {
        if self.platform.dmabuf_global != Some(*global) {
            return None;
        }

        let monitor = monitor::state::monitor_for_surface_or_current(self, surface);
        self.platform
            .dmabuf_output_feedbacks
            .get(monitor.as_str())
            .cloned()
    }
}

impl DrmSyncobjHandler for Halley {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.platform.drm_syncobj_state.as_mut()
    }
}

impl PointerConstraintsHandler for Halley {
    fn new_constraint(&mut self, surface: &WlSurface, _pointer: &PointerHandle<Self>) {
        interaction::pointer::activate_new_pointer_constraint(self, surface);
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) {
        interaction::pointer::cursor_position_hint(
            &mut crate::compositor::ctx::pointer_ctx(self),
            surface,
            pointer,
            location,
        );
    }
}

#[cfg(test)]
mod tests {}

impl PrimarySelectionHandler for Halley {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.platform.primary_selection_state
    }
}

delegate_primary_selection!(Halley);

impl DataControlHandler for Halley {
    fn data_control_state(&mut self) -> &mut DataControlState {
        &mut self.platform.data_control_state
    }
}

delegate_data_control!(Halley);
