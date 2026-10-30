//! Install the Wayland globals shared by both session backends.

use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::background_effect::BackgroundEffectState;
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::content_type::ContentTypeState;
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::DmabufState;
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::idle_inhibit::IdleInhibitManagerState;
use smithay::wayland::input_method::InputMethodManagerState;
use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::pointer_constraints::PointerConstraintsState;
use smithay::wayland::pointer_gestures::PointerGesturesState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::ext_data_control::DataControlState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::wayland::text_input::TextInputManagerState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;
use smithay::wayland::xdg_activation::XdgActivationState;
use smithay::wayland::xdg_toplevel_icon::XdgToplevelIconManager;

use super::{Session, SessionDriver};
use crate::wayland::WaylandState;

impl<D: SessionDriver> Session<D> {
    pub fn create_wayland_state(display_handle: DisplayHandle, driver: &mut D) -> WaylandState {
        let capabilities = driver.dmabuf_capabilities();
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_global = crate::wayland::dmabuf::create_global::<Self>(
            &mut dmabuf_state,
            &display_handle,
            &capabilities,
        );
        let primary_selection_state = PrimarySelectionState::new::<Self>(&display_handle);
        let ext_data_control_state = DataControlState::new::<Self, _>(
            &display_handle,
            Some(&primary_selection_state),
            |client| {
                crate::wayland::permissions::allowed(
                    client,
                    crate::wayland::permissions::Capability::Clipboard,
                )
            },
        );

        WaylandState::new(
            display_handle.clone(),
            CompositorState::new_v6::<Self>(&display_handle),
            smithay::wayland::fifo::FifoManagerState::new::<Self>(&display_handle),
            smithay::wayland::commit_timing::CommitTimingManagerState::unmanaged::<Self>(
                &display_handle,
            ),
            dmabuf_state,
            dmabuf_global,
            XdgShellState::new::<Self>(&display_handle),
            smithay::wayland::shell::xdg::dialog::XdgDialogState::new::<Self>(&display_handle),
            smithay::wayland::xdg_foreign::XdgForeignState::new::<Self>(&display_handle),
            XdgActivationState::new::<Self>(&display_handle),
            WlrLayerShellState::new::<Self>(&display_handle),
            BackgroundEffectState::new::<Self>(&display_handle),
            SinglePixelBufferState::new::<Self>(&display_handle),
            ContentTypeState::new::<Self>(&display_handle),
            XdgToplevelIconManager::new::<Self>(&display_handle),
            XdgDecorationState::new::<Self>(&display_handle),
            ViewporterState::new::<Self>(&display_handle),
            FractionalScaleManagerState::new::<Self>(&display_handle),
            IdleInhibitManagerState::new::<Self>(&display_handle),
            RelativePointerManagerState::new::<Self>(&display_handle),
            PointerConstraintsState::new::<Self>(&display_handle),
            smithay::wayland::pointer_warp::PointerWarpManager::new::<Self>(&display_handle),
            PointerGesturesState::new::<Self>(&display_handle),
            CursorShapeManagerState::new::<Self>(&display_handle),
            VirtualKeyboardManagerState::new::<Self, _>(&display_handle, |client| {
                crate::wayland::permissions::allowed(
                    client,
                    crate::wayland::permissions::Capability::VirtualKeyboard,
                )
            }),
            TextInputManagerState::new::<Self>(&display_handle),
            InputMethodManagerState::new::<Self, _>(&display_handle, |client| {
                crate::wayland::permissions::allowed(
                    client,
                    crate::wayland::permissions::Capability::InputMethod,
                )
            }),
            KeyboardShortcutsInhibitState::new::<Self>(&display_handle),
            ShmState::new::<Self>(&display_handle, vec![]),
            OutputManagerState::new_with_xdg_output::<Self>(&display_handle),
            crate::wayland::wlr_output_management::State::new::<Self>(&display_handle),
            crate::wayland::wlr_gamma_control::State::new::<Self>(
                &display_handle,
                D::BACKEND_KIND == crate::input::keybinds::BackendKind::Tty,
            ),
            crate::wayland::wlr_output_power::State::new::<Self>(
                &display_handle,
                D::BACKEND_KIND == crate::input::keybinds::BackendKind::Tty,
                |client| {
                    crate::wayland::permissions::allowed(
                        client,
                        crate::wayland::permissions::Capability::OutputPower,
                    )
                },
            ),
            crate::wayland::wlr_screencopy::State::new::<Self>(&display_handle),
            crate::wayland::ext_image_copy_capture::State::new::<Self>(&display_handle, |client| {
                crate::wayland::permissions::allowed(
                    client,
                    crate::wayland::permissions::Capability::Capture,
                )
            }),
            std::collections::HashMap::new(),
            DataDeviceState::new::<Self>(&display_handle),
            primary_selection_state,
            ext_data_control_state,
            crate::wayland::ext_workspace::State::new::<Self>(&display_handle),
            crate::wayland::foreign_toplevel::State::new::<Self>(&display_handle),
        )
    }
}
