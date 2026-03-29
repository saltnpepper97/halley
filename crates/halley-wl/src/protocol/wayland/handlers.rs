use super::*;
use eventline::info;
use halley_core::field::NodeId;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::desktop::{PopupKind, find_popup_root_surface, utils::bbox_from_surface_tree};
use smithay::input::pointer::{MotionEvent, PointerHandle};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgDecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::{xdg_positioner, xdg_toplevel};
use smithay::reexports::wayland_server::{
    Client, Resource, protocol::wl_output::WlOutput, protocol::wl_surface::WlSurface,
};
use smithay::utils::{Logical, Rectangle, SERIAL_COUNTER};
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
pub(super) struct InitialToplevelSize {
    pub(super) node_size: (i32, i32),
    pub(super) configure_size: Option<(i32, i32)>,
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

pub(super) fn initial_toplevel_size(
    st: &Halley,
    toplevel: &ToplevelSurface,
) -> InitialToplevelSize {
    let predicted_monitor = st
        .model
        .spawn_state
        .pending_spawn_monitor
        .as_ref()
        .filter(|monitor| {
            st.model
                .monitor_state
                .monitors
                .contains_key(monitor.as_str())
        })
        .cloned()
        .unwrap_or_else(|| {
            let focused = st.focused_monitor().to_string();
            if st
                .model
                .monitor_state
                .monitors
                .contains_key(focused.as_str())
            {
                focused
            } else {
                st.interaction_monitor().to_string()
            }
        });
    if let Some(cid) = st.active_cluster_workspace_for_monitor(predicted_monitor.as_str())
        && let Some(rect) = st.cluster_spawn_rect_for_new_member(predicted_monitor.as_str(), cid)
    {
        let width = rect.w.max(64.0).round() as i32;
        let height = rect.h.max(64.0).round() as i32;
        return InitialToplevelSize {
            node_size: (width, height),
            configure_size: Some((width, height)),
        };
    }

    let detected = detected_initial_toplevel_size(toplevel);
    let node_size = detected.unwrap_or_else(|| {
        (
            (st.model.viewport.size.x * 0.46).round() as i32,
            (st.model.viewport.size.y * 0.42).round() as i32,
        )
    });
    let configure_size = None;

    InitialToplevelSize {
        node_size,
        configure_size,
    }
}

fn layer_popup_constraint_target(
    st: &Halley,
    popup: &PopupSurface,
) -> Option<Rectangle<i32, Logical>> {
    let popup = PopupKind::from(popup.clone());
    let root = find_popup_root_surface(&popup).ok()?;
    if !st.is_layer_surface(&root) {
        return None;
    }

    let monitor = st.layer_surface_monitor_name(&root);
    let placement = st
        .layer_shell_placements_for_monitor(monitor.as_str())
        .into_iter()
        .find(|placement| placement.wl_surface.id() == root.id())?;
    let output_size = st.layer_output_size_for_monitor(monitor.as_str());

    Some(Rectangle::new(
        (-placement.origin.x, -placement.origin.y).into(),
        output_size,
    ))
}

pub(super) fn constrain_layer_popup(
    st: &Halley,
    popup: &PopupSurface,
    positioner: PositionerState,
) {
    let Some(target) = layer_popup_constraint_target(st, popup) else {
        return;
    };
    let mut geometry = positioner.get_unconstrained_geometry(target);
    if !rectangle_fits_within(target, geometry) {
        let mut fallback_positioner = positioner;
        fallback_positioner.constraint_adjustment |= xdg_positioner::ConstraintAdjustment::FlipX
            | xdg_positioner::ConstraintAdjustment::FlipY
            | xdg_positioner::ConstraintAdjustment::SlideX
            | xdg_positioner::ConstraintAdjustment::SlideY
            | xdg_positioner::ConstraintAdjustment::ResizeX
            | xdg_positioner::ConstraintAdjustment::ResizeY;
        geometry = fallback_positioner.get_unconstrained_geometry(target);
    }

    popup.with_pending_state(|state| {
        state.geometry = geometry;
    });
}

fn rectangle_fits_within(target: Rectangle<i32, Logical>, rect: Rectangle<i32, Logical>) -> bool {
    rect.loc.x >= target.loc.x
        && rect.loc.y >= target.loc.y
        && rect.loc.x + rect.size.w <= target.loc.x + target.size.w
        && rect.loc.y + rect.size.h <= target.loc.y + target.size.h
}

impl SeatHandler for Halley {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.platform.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        // Only suspend a fullscreen if focus moves to a surface on the *same monitor*.
        // Focus changing on a different monitor must never disturb an unrelated fullscreen.
        let focused_is_layer = focused.is_some_and(|surface| self.is_layer_surface(surface));
        if !focused_is_layer {
            let focused_id = focused.map(|wl| wl.id());

            // Which monitor is the newly-focused surface on?
            let focused_monitor: Option<String> = focused_id.as_ref().and_then(|fid| {
                let node_id = self.model.surface_to_node.get(fid).copied()?;
                Some(
                    self.model
                        .monitor_state
                        .node_monitor
                        .get(&node_id)
                        .cloned()
                        .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone()),
                )
            });

            let to_suspend: Vec<NodeId> = self
                .model
                .fullscreen_state
                .fullscreen_active_node
                .iter()
                .filter_map(|(monitor, &fullscreen_id)| {
                    // Skip fullscreens on other monitors entirely.
                    let same_monitor = focused_monitor
                        .as_deref()
                        .is_some_and(|fm| fm == monitor.as_str());
                    if !same_monitor {
                        return None;
                    }
                    let fullscreen_surface_id = self
                        .platform
                        .xdg_shell_state
                        .toplevel_surfaces()
                        .iter()
                        .find_map(|top| {
                            (self
                                .model
                                .surface_to_node
                                .get(&top.wl_surface().id())
                                .copied()
                                == Some(fullscreen_id))
                            .then(|| top.wl_surface().id())
                        });
                    (fullscreen_surface_id.is_some() && fullscreen_surface_id != focused_id)
                        .then_some(fullscreen_id)
                })
                .collect();

            let now = Instant::now();
            for fullscreen_id in to_suspend {
                self.suspend_xdg_fullscreen(fullscreen_id, now);
            }
        }

        info!(
            "seat focus_changed -> {:?}",
            focused.map(|wl| format!("{:?}", wl.id()))
        );

        let client = client_for_surface(focused);
        set_data_device_focus(&self.platform.display_handle, seat, client.clone());
        set_primary_focus(&self.platform.display_handle, seat, client);
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
        self.note_commit(surface, Instant::now());
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
        self.apply_cursor_position_hint(surface, pointer, location);
    }
}

impl Halley {
    pub(crate) fn preferred_xdg_decoration_mode(&self) -> XdgDecorationMode {
        if self.runtime.tuning.effective_no_csd() {
            XdgDecorationMode::ServerSide
        } else {
            XdgDecorationMode::ClientSide
        }
    }

    pub(crate) fn apply_toplevel_tiled_hint(
        &self,
        state: &mut smithay::wayland::shell::xdg::ToplevelState,
    ) {
        let tiled = self.runtime.tuning.effective_no_csd()
            && !state.states.contains(xdg_toplevel::State::Fullscreen);
        for edge in [
            xdg_toplevel::State::TiledTop,
            xdg_toplevel::State::TiledBottom,
            xdg_toplevel::State::TiledLeft,
            xdg_toplevel::State::TiledRight,
        ] {
            if tiled {
                state.states.set(edge);
            } else {
                state.states.unset(edge);
            }
        }
    }

    pub(crate) fn refresh_xdg_decoration_mode(&mut self) {
        let mode = self.preferred_xdg_decoration_mode();
        for toplevel in self.platform.xdg_shell_state.toplevel_surfaces() {
            toplevel.with_pending_state(|state| {
                state.decoration_mode = Some(mode);
                self.apply_toplevel_tiled_hint(state);
            });
            toplevel.send_configure();
        }
    }

    fn install_drm_syncobj_blocker(&mut self, surface: &WlSurface) {
        if self.platform.drm_syncobj_state.is_none() {
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
        let pending_surfaces = self.runtime.pending_drm_syncobj_surfaces.clone();
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
        let surface_ids = match self.runtime.pending_drm_syncobj_surfaces.lock() {
            Ok(mut pending) => std::mem::take(&mut *pending),
            Err(_) => return,
        };
        let dh = self.platform.display_handle.clone();

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
        let Some(pointer) = self.platform.seat.get_pointer() else {
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
        let Some(pointer) = self.platform.seat.get_pointer() else {
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
        let Some(keyboard) = self.platform.seat.get_keyboard() else {
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
        let Some(node_id) = self.model.surface_to_node.get(&surface.id()).copied() else {
            return;
        };
        let monitor = self
            .model
            .monitor_state
            .node_monitor
            .get(&node_id)
            .cloned()
            .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone());
        let (ws_w, ws_h, _, _) = self.local_screen_in_monitor(monitor.as_str(), 0.0, 0.0);
        let previous_monitor = self.begin_temporary_render_monitor(monitor.as_str());
        let Some(xform) = active_node_surface_transform_screen_details(
            self,
            ws_w,
            ws_h,
            node_id,
            Instant::now(),
            None,
        ) else {
            self.end_temporary_render_monitor(previous_monitor);
            return;
        };

        let sx =
            (xform.origin_x + location.x as f32 * xform.scale).clamp(0.0, (ws_w.max(1) - 1) as f32);
        let sy =
            (xform.origin_y + location.y as f32 * xform.scale).clamp(0.0, (ws_h.max(1) - 1) as f32);
        self.input.interaction_state.pending_pointer_screen_hint = Some((sx, sy));

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
        self.end_temporary_render_monitor(previous_monitor);
    }

    pub(crate) fn release_active_pointer_constraint(&mut self) -> bool {
        let Some(pointer) = self.platform.seat.get_pointer() else {
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
            self.input.interaction_state.reset_input_state_requested = true;
        }
        released
    }

    pub(crate) fn active_locked_pointer_surface(&self) -> Option<WlSurface> {
        let pointer = self.platform.seat.get_pointer()?;
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
