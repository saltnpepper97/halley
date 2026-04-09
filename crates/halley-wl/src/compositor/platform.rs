use std::os::unix::io::AsFd;
use std::rc::Rc;

use smithay::{
    desktop::PopupManager,
    input::{Seat, SeatState, pointer::CursorImageStatus},
    reexports::{
        wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgDecorationMode,
        wayland_server::{
            DisplayHandle, Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
        },
    },
    wayland::{
        compositor::{CompositorState, add_blocker, with_states},
        cursor_shape::CursorShapeManagerState,
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState},
        drm_syncobj::{DrmSyncPoint, DrmSyncobjCachedState, DrmSyncobjState},
        idle_notify::IdleNotifierState,
        output::OutputManagerState,
        pointer_constraints::PointerConstraintsState,
        relative_pointer::RelativePointerManagerState,
        selection::{
            data_device::DataDeviceState, primary_selection::PrimarySelectionState,
            wlr_data_control::DataControlState,
        },
        shell::wlr_layer::WlrLayerShellState,
        shell::xdg::{ToplevelState, XdgShellState, decoration::XdgDecorationState},
        shm::ShmState,
        viewporter::ViewporterState,
        xdg_activation::XdgActivationState,
    },
};

use super::root::Halley;
use crate::backend::interface::DmabufImportBackend;
use crate::protocol::wayland::ClientState;

fn preferred_xdg_decoration_mode_for(no_csd: bool) -> XdgDecorationMode {
    if no_csd {
        XdgDecorationMode::ServerSide
    } else {
        XdgDecorationMode::ClientSide
    }
}

fn should_apply_toplevel_tiled_hint(fullscreen: bool) -> bool {
    !fullscreen
}

#[allow(dead_code)]
pub(crate) struct PlatformState {
    pub(crate) display_handle: DisplayHandle,
    pub(crate) compositor_state: CompositorState,
    pub(crate) viewporter_state: ViewporterState,
    pub(crate) xdg_shell_state: XdgShellState,
    pub(crate) xdg_activation_state: XdgActivationState,
    pub(crate) xdg_decoration_state: XdgDecorationState,
    pub(crate) cursor_shape_manager_state: CursorShapeManagerState,
    pub(crate) popup_manager: PopupManager,
    pub(crate) wlr_layer_shell_state: WlrLayerShellState,
    pub(crate) pointer_constraints_state: PointerConstraintsState,
    pub(crate) relative_pointer_manager_state: RelativePointerManagerState,
    pub(crate) idle_notifier_state: IdleNotifierState<Halley>,
    pub(crate) drm_syncobj_state: Option<DrmSyncobjState>,
    pub(crate) output_manager_state: OutputManagerState,
    pub(crate) shm_state: ShmState,
    pub(crate) dmabuf_state: DmabufState,
    pub(crate) dmabuf_global: Option<DmabufGlobal>,
    pub(crate) seat_state: SeatState<Halley>,
    pub(crate) data_device_state: DataDeviceState,
    pub(crate) primary_selection_state: PrimarySelectionState,
    pub(crate) data_control_state: DataControlState,
    pub(crate) seat: Seat<Halley>,
    pub(crate) cursor_image_status: CursorImageStatus,
    pub(crate) dmabuf_importer: Option<Rc<dyn DmabufImportBackend>>,
}

pub(crate) fn preferred_xdg_decoration_mode(st: &Halley) -> XdgDecorationMode {
    preferred_xdg_decoration_mode_for(st.runtime.tuning.effective_no_csd())
}

pub(crate) fn apply_toplevel_tiled_hint(_st: &Halley, state: &mut ToplevelState) {
    let tiled = should_apply_toplevel_tiled_hint(state.states.contains(
        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen,
    ));
    for edge in [
        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::TiledTop,
        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::TiledBottom,
        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::TiledLeft,
        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::TiledRight,
    ] {
        if tiled {
            state.states.set(edge);
        } else {
            state.states.unset(edge);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::input::pointer::{CursorIcon, CursorImageStatus};

    #[test]
    fn preferred_decoration_mode_uses_effective_no_csd() {
        assert_eq!(
            preferred_xdg_decoration_mode_for(false),
            XdgDecorationMode::ClientSide
        );
        assert_eq!(
            preferred_xdg_decoration_mode_for(true),
            XdgDecorationMode::ServerSide
        );
    }

    #[test]
    fn tiled_hints_apply_to_all_non_fullscreen_toplevels() {
        assert!(should_apply_toplevel_tiled_hint(false));
        assert!(!should_apply_toplevel_tiled_hint(true));
    }

    #[test]
    fn idle_hide_does_not_hide_cursor_without_pointer_focus() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cursor.hide_after_ms = 5_000;
        let mut state = Halley::new_for_test(&dh, tuning);
        state.platform.cursor_image_status = CursorImageStatus::default_named();
        state.input.interaction_state.last_cursor_activity_at_ms = 0;

        assert!(matches!(
            effective_cursor_image_status(&state),
            CursorImageStatus::Named(_)
        ));
    }

    #[test]
    fn compositor_override_icon_still_applies_without_pointer_focus() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        state.platform.cursor_image_status = CursorImageStatus::Hidden;
        state.input.interaction_state.cursor_override_icon = Some(CursorIcon::Pointer);

        assert!(matches!(
            effective_cursor_image_status(&state),
            CursorImageStatus::Named(CursorIcon::Pointer)
        ));
    }
}

pub(crate) fn refresh_xdg_decoration_mode(st: &mut Halley) {
    let mode = preferred_xdg_decoration_mode(st);
    for toplevel in st.platform.xdg_shell_state.toplevel_surfaces() {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
            apply_toplevel_tiled_hint(st, state);
        });
        toplevel.send_configure();
    }
}

pub(crate) fn effective_cursor_image_status(st: &Halley) -> CursorImageStatus {
    let pointer_has_client_focus = st
        .platform
        .seat
        .get_pointer()
        .and_then(|pointer| pointer.current_focus())
        .is_some();

    if st.input.interaction_state.cursor_hidden_by_typing {
        return CursorImageStatus::Hidden;
    }

    let hide_after_ms = st.runtime.tuning.cursor.hide_after_ms;
    if hide_after_ms > 0 && pointer_has_client_focus {
        let now_ms = st.now_ms(std::time::Instant::now());
        if now_ms.saturating_sub(st.input.interaction_state.last_cursor_activity_at_ms)
            >= hide_after_ms
        {
            return CursorImageStatus::Hidden;
        }
    }

    if matches!(st.platform.cursor_image_status, CursorImageStatus::Hidden)
        && pointer_has_client_focus
    {
        return CursorImageStatus::Hidden;
    }

    st.input
        .interaction_state
        .cursor_override_icon
        .map(CursorImageStatus::Named)
        .unwrap_or_else(|| st.platform.cursor_image_status.clone())
}

pub(crate) fn install_drm_syncobj_blocker(st: &mut Halley, surface: &WlSurface) {
    if st.platform.drm_syncobj_state.is_none() {
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
    spawn_drm_syncobj_waiter(st, surface.id(), acquire_point, blocker_state);
}

fn spawn_drm_syncobj_waiter(
    st: &Halley,
    surface_id: ObjectId,
    acquire_point: DrmSyncPoint,
    blocker_state: SyncobjCommitBlockerState,
) {
    let pending_surfaces = st.runtime.pending_drm_syncobj_surfaces.clone();
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

pub(crate) fn drain_drm_syncobj_blockers(st: &mut Halley) {
    let surface_ids = match st.runtime.pending_drm_syncobj_surfaces.lock() {
        Ok(mut pending) => std::mem::take(&mut *pending),
        Err(_) => return,
    };
    let dh = st.platform.display_handle.clone();

    for surface_id in surface_ids {
        let Ok(client) = dh.get_client(surface_id) else {
            continue;
        };
        let Some(client_state) = client.get_data::<ClientState>() else {
            continue;
        };
        client_state.compositor_state.blocker_cleared(st, &dh);
    }
}

pub(crate) fn configure_dmabuf_importer(
    st: &mut Halley,
    importer: Rc<dyn DmabufImportBackend>,
    main_device: Option<rustix::fs::Dev>,
) {
    let formats = importer.dmabuf_formats();
    if formats.is_empty() {
        return;
    }

    let global = match main_device {
        Some(device) => {
            let feedback = DmabufFeedbackBuilder::new(device, formats.iter().copied())
                .build()
                .expect("renderer dmabuf feedback should be constructible");
            st.platform
                .dmabuf_state
                .create_global_with_default_feedback::<Halley>(
                    &st.platform.display_handle,
                    &feedback,
                )
        }
        None => st
            .platform
            .dmabuf_state
            .create_global::<Halley>(&st.platform.display_handle, formats.iter().copied()),
    };

    st.platform.dmabuf_importer = Some(importer);
    st.platform.dmabuf_global = Some(global);
}

pub(crate) fn configure_dmabuf_importer_for_fd<Fd: AsFd>(
    st: &mut Halley,
    importer: Rc<dyn DmabufImportBackend>,
    device_fd: Fd,
) {
    let main_device = rustix::fs::fstat(device_fd).ok().map(|stat| stat.st_rdev);
    configure_dmabuf_importer(st, importer, main_device);
}

#[inline]
pub fn note_input_activity(st: &mut Halley) {
    st.platform
        .idle_notifier_state
        .notify_activity(&st.platform.seat);
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
