use super::*;
use eventline::info;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::desktop::{PopupKind, find_popup_root_surface, utils::bbox_from_surface_tree};
use smithay::input::pointer::{MotionEvent, PointerHandle};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{
    Client, Resource, protocol::wl_output::WlOutput, protocol::wl_surface::WlSurface,
};
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::compositor::{add_blocker, with_states};
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufGlobal, DmabufHandler, ImportNotifier};
use smithay::wayland::drm_syncobj::{DrmSyncPoint, DrmSyncobjCachedState, DrmSyncobjHandler};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::pointer_constraints::{PointerConstraint, with_pointer_constraint};
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
};
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface, LayerSurfaceConfigure, WlrLayerShellHandler, WlrLayerShellState,
};
use smithay::wayland::shell::xdg::SurfaceCachedState;

use crate::input::active_node_surface_transform_screen_details;

fn client_for_surface(surface: Option<&WlSurface>) -> Option<Client> {
    surface.and_then(|wl| wl.client())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InitialToplevelSize {
    node_size: (i32, i32),
    configure_size: Option<(i32, i32)>,
}

fn detected_initial_toplevel_size(toplevel: &ToplevelSurface) -> Option<(i32, i32)> {
    let wl = toplevel.wl_surface();

    let geometry = with_states(wl, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
    });
    if let Some(geometry) = geometry {
        return Some((geometry.size.w.max(96), geometry.size.h.max(72)));
    }

    if let Some(size) = toplevel.current_state().size {
        return Some((size.w.max(96), size.h.max(72)));
    }

    let bbox = bbox_from_surface_tree(wl, (0, 0));
    if bbox.size.w > 0 && bbox.size.h > 0 {
        return Some((bbox.size.w.max(96), bbox.size.h.max(72)));
    }

    None
}

fn initial_toplevel_size(st: &HalleyWlState, toplevel: &ToplevelSurface) -> InitialToplevelSize {
    let detected = detected_initial_toplevel_size(toplevel);
    let node_size = detected.unwrap_or_else(|| {
        (
            (st.viewport.size.x * 0.46).round() as i32,
            (st.viewport.size.y * 0.42).round() as i32,
        )
    });
    let configure_size = detected.is_none().then_some(node_size);

    InitialToplevelSize {
        node_size,
        configure_size,
    }
}

impl SeatHandler for HalleyWlState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        if let Some(fullscreen_id) = self.fullscreen_active_node {
            let focused_is_layer = focused.is_some_and(|surface| self.is_layer_surface(surface));
            let fullscreen_surface_id = self
                .xdg_shell_state
                .toplevel_surfaces()
                .iter()
                .find_map(|top| {
                    (self.surface_to_node.get(&top.wl_surface().id()).copied()
                        == Some(fullscreen_id))
                    .then(|| top.wl_surface().id())
                });
            let focused_id = focused.map(|wl| wl.id());
            if !focused_is_layer && fullscreen_surface_id.is_some() && fullscreen_surface_id != focused_id {
                self.suspend_xdg_fullscreen(fullscreen_id, Instant::now());
            }
        }

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
delegate_pointer_constraints!(HalleyWlState);
delegate_relative_pointer!(HalleyWlState);
delegate_drm_syncobj!(HalleyWlState);

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
        self.install_drm_syncobj_blocker(surface);
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

impl DrmSyncobjHandler for HalleyWlState {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.drm_syncobj_state.as_mut()
    }
}

impl PointerConstraintsHandler for HalleyWlState {
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
        self.apply_cursor_position_hint(surface, pointer, location);
    }
}

impl HalleyWlState {
    fn install_drm_syncobj_blocker(&mut self, surface: &WlSurface) {
        if self.drm_syncobj_state.is_none() {
            return;
        }

        let acquire_point = with_states(surface, |states| {
            let mut cached = states.cached_state.get::<DrmSyncobjCachedState>();
            cached.pending().acquire_point.clone()
        });

        let Some(acquire_point) = acquire_point else {
            return;
        };

        let blocker_state = SyncobjCommitBlockerState::default();
        add_blocker(
            surface,
            SyncobjCommitBlocker {
                state: blocker_state.clone(),
            },
        );
        self.spawn_drm_syncobj_waiter(surface.id(), acquire_point, blocker_state);
    }

    fn spawn_drm_syncobj_waiter(
        &self,
        surface_id: smithay::reexports::wayland_server::backend::ObjectId,
        acquire_point: DrmSyncPoint,
        blocker_state: SyncobjCommitBlockerState,
    ) {
        let pending_surfaces = self.pending_drm_syncobj_surfaces.clone();
        std::thread::spawn(move || {
            let state = if acquire_point.wait(i64::MAX).is_ok() {
                SyncobjCommitBlockerStatus::Released
            } else {
                SyncobjCommitBlockerStatus::Cancelled
            };
            blocker_state.store(state);
            if let Ok(mut pending) = pending_surfaces.lock() {
                pending.push(surface_id);
            }
        });
    }

    pub(crate) fn drain_drm_syncobj_blockers(&mut self) {
        let surface_ids = match self.pending_drm_syncobj_surfaces.lock() {
            Ok(mut pending) => std::mem::take(&mut *pending),
            Err(_) => return,
        };
        let dh = self.display_handle.clone();

        for surface_id in surface_ids {
            let Ok(client) = dh.get_client(surface_id) else {
                continue;
            };
            let Some(client_state) = client.get_data::<ClientState>() else {
                continue;
            };
            client_state.compositor_state.blocker_cleared(self, &dh);
        }
    }

    pub(crate) fn activate_pointer_constraint_for_surface(&mut self, surface: &WlSurface) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        with_pointer_constraint(surface, &pointer, |constraint| {
            if let Some(constraint) = constraint
                && !constraint.is_active()
            {
                constraint.activate();
            }
        });
    }

    pub(crate) fn clear_pointer_focus(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if pointer.is_grabbed() {
            pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), 0);
        }
        let location = pointer.current_location();
        pointer.motion(
            self,
            None,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: 0,
            },
        );
        pointer.frame(self);
    }

    pub(crate) fn clear_keyboard_focus(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
        self.update_selection_focus_from_surface(None);
    }

    fn apply_cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) {
        let Some(node_id) = self.surface_to_node.get(&surface.id()).copied() else {
            return;
        };
        let ws_w = self.tuning.viewport_size.x.max(1.0).round() as i32;
        let ws_h = self.tuning.viewport_size.y.max(1.0).round() as i32;
        let Some(xform) = active_node_surface_transform_screen_details(
            self,
            ws_w,
            ws_h,
            node_id,
            Instant::now(),
            None,
        ) else {
            return;
        };

        let sx = (xform.origin_x + location.x as f32 * xform.scale)
            .clamp(0.0, (ws_w.max(1) - 1) as f32);
        let sy = (xform.origin_y + location.y as f32 * xform.scale)
            .clamp(0.0, (ws_h.max(1) - 1) as f32);
        self.pending_pointer_screen_hint = Some((sx, sy));

        let cam_scale = self.camera_render_scale().max(0.001) as f64;
        let focus_origin = smithay::utils::Point::<f64, smithay::utils::Logical>::from((
            xform.origin_x as f64 / cam_scale,
            xform.origin_y as f64 / cam_scale,
        ));
        pointer.motion(
            self,
            Some((surface.clone(), focus_origin)),
            &MotionEvent {
                location: (sx as f64 / cam_scale, sy as f64 / cam_scale).into(),
                serial: SERIAL_COUNTER.next_serial(),
                time: 0,
            },
        );
        pointer.frame(self);
    }

    pub(crate) fn release_active_pointer_constraint(&mut self) -> bool {
        let Some(pointer) = self.seat.get_pointer() else {
            return false;
        };
        let Some(surface) = pointer.current_focus() else {
            return false;
        };
        let mut released = false;
        with_pointer_constraint(&surface, &pointer, |constraint| {
            if let Some(constraint) = constraint
                && constraint.is_active()
            {
                constraint.deactivate();
                released = true;
            }
        });
        if released {
            self.clear_pointer_focus();
            self.reset_input_state_requested = true;
        }
        released
    }

    pub(crate) fn active_locked_pointer_surface(&self) -> Option<WlSurface> {
        let pointer = self.seat.get_pointer()?;
        let surface = pointer.current_focus()?;
        let locked = with_pointer_constraint(&surface, &pointer, |constraint| {
            matches!(constraint.as_deref(), Some(PointerConstraint::Locked(_)))
                && constraint
                    .as_deref()
                    .is_some_and(PointerConstraint::is_active)
        });
        locked.then_some(surface)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SyncobjCommitBlockerStatus {
    Pending,
    Released,
    Cancelled,
}

#[derive(Clone, Debug)]
struct SyncobjCommitBlockerState(std::sync::Arc<std::sync::atomic::AtomicU8>);

impl Default for SyncobjCommitBlockerState {
    fn default() -> Self {
        Self(std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
            SyncobjCommitBlockerStatus::Pending as u8,
        )))
    }
}

impl SyncobjCommitBlockerState {
    fn store(&self, status: SyncobjCommitBlockerStatus) {
        self.0
            .store(status as u8, std::sync::atomic::Ordering::SeqCst);
    }

    fn load(&self) -> SyncobjCommitBlockerStatus {
        match self.0.load(std::sync::atomic::Ordering::SeqCst) {
            1 => SyncobjCommitBlockerStatus::Released,
            2 => SyncobjCommitBlockerStatus::Cancelled,
            _ => SyncobjCommitBlockerStatus::Pending,
        }
    }
}

#[derive(Clone, Debug)]
struct SyncobjCommitBlocker {
    state: SyncobjCommitBlockerState,
}

impl smithay::wayland::compositor::Blocker for SyncobjCommitBlocker {
    fn state(&self) -> smithay::wayland::compositor::BlockerState {
        match self.state.load() {
            SyncobjCommitBlockerStatus::Pending => {
                smithay::wayland::compositor::BlockerState::Pending
            }
            SyncobjCommitBlockerStatus::Released => {
                smithay::wayland::compositor::BlockerState::Released
            }
            SyncobjCommitBlockerStatus::Cancelled => {
                smithay::wayland::compositor::BlockerState::Cancelled
            }
        }
    }
}

impl XdgShellHandler for HalleyWlState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        let initial_size = initial_toplevel_size(self, &toplevel);
        // You MUST send an initial configure or many clients won’t map.
        // Only send an explicit size when the client's initial proposal is
        // absent or obviously outside the viewport-relative startup range.
        toplevel.with_pending_state(|s| {
            s.states.set(xdg_toplevel::State::Activated);
            if let Some((w, h)) = initial_size.configure_size {
                s.size = Some((w, h).into());
            }
        });
        toplevel.send_configure();

        // Detect child/transient toplevels (e.g. browser video-preview popups).
        // These are spawned by the client as logical children of an existing
        // surface and should not disturb the spatial layout of their parent.
        let is_transient = toplevel.parent().is_some();

        // Create a core node for the underlying wl_surface.
        let wl = toplevel.wl_surface().clone();
        let id = self.ensure_node_for_surface(&wl, "toplevel", initial_size.node_size);
        let now = Instant::now();
        let _ = self.field.touch(id, self.now_ms(now));
        self.reveal_new_toplevel_node(id, is_transient, now);

        self.resolve_surface_overlap();
        self.request_maintenance();
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.refresh_node_identity_for_surface(surface.wl_surface(), "Window");
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.refresh_node_identity_for_surface(surface.wl_surface(), "Window");
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

    fn fullscreen_request(&mut self, surface: ToplevelSurface, output: Option<WlOutput>) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.surface_to_node.get(&key).copied() else {
            surface.send_configure();
            return;
        };
        self.enter_xdg_fullscreen(node_id, output, Instant::now());
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.surface_to_node.get(&key).copied() else {
            surface.send_configure();
            return;
        };
        self.exit_xdg_fullscreen(node_id, Instant::now());
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

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let key = surface.wl_surface().id();
        let had_keyboard_focus = self
            .seat
            .get_keyboard()
            .and_then(|kb| kb.current_focus())
            .is_some_and(|focused| focused.id() == key);
        let had_pointer_focus = self
            .seat
            .get_pointer()
            .and_then(|ptr| ptr.current_focus())
            .is_some_and(|focused| focused.id() == key);

        if had_keyboard_focus || had_pointer_focus {
            info!(
                "toplevel_destroyed with active focus (keyboard={} pointer={}); scheduling input state reset",
                had_keyboard_focus, had_pointer_focus
            );
            self.reset_input_state_requested = true;
        }

        if had_keyboard_focus {
            self.clear_keyboard_focus();
        }
        if had_pointer_focus {
            self.clear_pointer_focus();
        }
    }
}

#[cfg(test)]
mod tests {}

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
