//! Delegate upstream protocols through Smithay's public Dispatch2 API.
//!
//! Use an explicit protocol set so Halley's lock and virtual-keyboard request
//! guards, and its own protocol implementations, keep their custom dispatch.
use smithay::reexports::wayland_server::Resource;

pub(crate) trait UpstreamProtocol: Resource {}

macro_rules! upstream_protocols {
    ($($protocol:path),* $(,)?) => {
        $(impl UpstreamProtocol for $protocol {})*
    };
}

upstream_protocols! {
    smithay::reexports::wayland_protocols::xdg::foreign::zv2::server::zxdg_exporter_v2::ZxdgExporterV2,
    smithay::reexports::wayland_protocols::xdg::foreign::zv2::server::zxdg_exported_v2::ZxdgExportedV2,
    smithay::reexports::wayland_protocols::xdg::foreign::zv2::server::zxdg_importer_v2::ZxdgImporterV2,
    smithay::reexports::wayland_protocols::xdg::foreign::zv2::server::zxdg_imported_v2::ZxdgImportedV2,
    smithay::reexports::wayland_protocols::ext::foreign_toplevel_list::v1::server::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
    smithay::reexports::wayland_protocols::ext::foreign_toplevel_list::v1::server::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    smithay::reexports::wayland_protocols::wp::content_type::v1::server::wp_content_type_manager_v1::WpContentTypeManagerV1,
    smithay::reexports::wayland_protocols::wp::content_type::v1::server::wp_content_type_v1::WpContentTypeV1,
    smithay::reexports::wayland_protocols::xdg::toplevel_icon::v1::server::xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1,
    smithay::reexports::wayland_protocols::xdg::toplevel_icon::v1::server::xdg_toplevel_icon_v1::XdgToplevelIconV1,
    smithay::reexports::wayland_protocols::wp::single_pixel_buffer::v1::server::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1,
    smithay::reexports::wayland_protocols::ext::background_effect::v1::server::ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1,
    smithay::reexports::wayland_protocols::ext::background_effect::v1::server::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
    smithay::reexports::wayland_protocols::ext::data_control::v1::server::ext_data_control_device_v1::ExtDataControlDeviceV1,
    smithay::reexports::wayland_protocols::ext::data_control::v1::server::ext_data_control_manager_v1::ExtDataControlManagerV1,
    smithay::reexports::wayland_protocols::ext::data_control::v1::server::ext_data_control_source_v1::ExtDataControlSourceV1,
    smithay::reexports::wayland_protocols::ext::idle_notify::v1::server::ext_idle_notification_v1::ExtIdleNotificationV1,
    smithay::reexports::wayland_protocols::ext::idle_notify::v1::server::ext_idle_notifier_v1::ExtIdleNotifierV1,
    smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_manager_v1::ExtSessionLockManagerV1,
    smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
    smithay::reexports::wayland_protocols::wp::cursor_shape::v1::server::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1,
    smithay::reexports::wayland_protocols::wp::cursor_shape::v1::server::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
    smithay::reexports::wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    smithay::reexports::wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_v1::WpFractionalScaleV1,
    smithay::reexports::wayland_protocols::wp::idle_inhibit::zv1::server::zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1,
    smithay::reexports::wayland_protocols::wp::idle_inhibit::zv1::server::zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
    smithay::reexports::wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::server::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
    smithay::reexports::wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::server::zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
    smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
    smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
    smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    smithay::reexports::wayland_protocols::wp::linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1,
    smithay::reexports::wayland_protocols::wp::linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1,
    smithay::reexports::wayland_protocols::wp::linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
    smithay::reexports::wayland_protocols::wp::pointer_constraints::zv1::server::zwp_confined_pointer_v1::ZwpConfinedPointerV1,
    smithay::reexports::wayland_protocols::wp::pointer_constraints::zv1::server::zwp_locked_pointer_v1::ZwpLockedPointerV1,
    smithay::reexports::wayland_protocols::wp::pointer_constraints::zv1::server::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
    smithay::reexports::wayland_protocols::wp::pointer_gestures::zv1::server::zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1,
    smithay::reexports::wayland_protocols::wp::pointer_gestures::zv1::server::zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1,
    smithay::reexports::wayland_protocols::wp::pointer_gestures::zv1::server::zwp_pointer_gesture_swipe_v1::ZwpPointerGestureSwipeV1,
    smithay::reexports::wayland_protocols::wp::pointer_gestures::zv1::server::zwp_pointer_gestures_v1::ZwpPointerGesturesV1,
    smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation::WpPresentation,
    smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::WpPresentationFeedback,
    smithay::reexports::wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
    smithay::reexports::wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
    smithay::reexports::wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
    smithay::reexports::wayland_protocols::wp::relative_pointer::zv1::server::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
    smithay::reexports::wayland_protocols::wp::relative_pointer::zv1::server::zwp_relative_pointer_v1::ZwpRelativePointerV1,
    smithay::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_manager_v3::ZwpTextInputManagerV3,
    smithay::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_v3::ZwpTextInputV3,
    smithay::reexports::wayland_protocols::wp::viewporter::server::wp_viewport::WpViewport,
    smithay::reexports::wayland_protocols::wp::viewporter::server::wp_viewporter::WpViewporter,
    smithay::reexports::wayland_protocols::xdg::activation::v1::server::xdg_activation_token_v1::XdgActivationTokenV1,
    smithay::reexports::wayland_protocols::xdg::activation::v1::server::xdg_activation_v1::XdgActivationV1,
    smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_popup::XdgPopup,
    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::XdgPositioner,
    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_surface::XdgSurface,
    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel,
    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase,
    smithay::reexports::wayland_protocols::xdg::xdg_output::zv1::server::zxdg_output_manager_v1::ZxdgOutputManagerV1,
    smithay::reexports::wayland_protocols::xdg::xdg_output::zv1::server::zxdg_output_v1::ZxdgOutputV1,
    smithay::reexports::wayland_protocols::xwayland::keyboard_grab::zv1::server::zwp_xwayland_keyboard_grab_manager_v1::ZwpXwaylandKeyboardGrabManagerV1,
    smithay::reexports::wayland_protocols::xwayland::keyboard_grab::zv1::server::zwp_xwayland_keyboard_grab_v1::ZwpXwaylandKeyboardGrabV1,
    smithay::reexports::wayland_protocols::xwayland::shell::v1::server::xwayland_shell_v1::XwaylandShellV1,
    smithay::reexports::wayland_protocols::xwayland::shell::v1::server::xwayland_surface_v1::XwaylandSurfaceV1,
    smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2,
    smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
    smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_v2::ZwpInputMethodV2,
    smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2,
    smithay::reexports::wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    smithay::reexports::wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_shell_v1::ZwlrLayerShellV1,
    smithay::reexports::wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    smithay::reexports::wayland_server::protocol::wl_callback::WlCallback,
    smithay::reexports::wayland_server::protocol::wl_compositor::WlCompositor,
    smithay::reexports::wayland_server::protocol::wl_data_device::WlDataDevice,
    smithay::reexports::wayland_server::protocol::wl_data_device_manager::WlDataDeviceManager,
    smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource,
    smithay::reexports::wayland_server::protocol::wl_keyboard::WlKeyboard,
    smithay::reexports::wayland_server::protocol::wl_output::WlOutput,
    smithay::reexports::wayland_server::protocol::wl_pointer::WlPointer,
    smithay::reexports::wayland_server::protocol::wl_region::WlRegion,
    smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
    smithay::reexports::wayland_server::protocol::wl_shm::WlShm,
    smithay::reexports::wayland_server::protocol::wl_shm_pool::WlShmPool,
    smithay::reexports::wayland_server::protocol::wl_subcompositor::WlSubcompositor,
    smithay::reexports::wayland_server::protocol::wl_subsurface::WlSubsurface,
    smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    smithay::reexports::wayland_server::protocol::wl_touch::WlTouch,
}

macro_rules! delegate_upstream_protocols {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $state:ty $(, $guard:path)?) => {
        impl<$( $( $lt $( : $clt $(+ $dlt )* )? ),+, )? I, U>
            smithay::reexports::wayland_server::Dispatch<I, U> for $state
        where
            I: crate::upstream_protocols::UpstreamProtocol,
            U: smithay::wayland::Dispatch2<I, $state>,
            I::Request: 'static,
        {
            fn request(
                state: &mut Self,
                client: &smithay::reexports::wayland_server::Client,
                resource: &I,
                request: I::Request,
                data: &U,
                display: &smithay::reexports::wayland_server::DisplayHandle,
                init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>,
            ) {
                $(if !$guard(state, client, resource, &request, display) {
                    return;
                })?
                data.request(state, client, resource, request, display, init);
            }

            fn destroyed(
                state: &mut Self,
                client: smithay::reexports::wayland_server::backend::ClientId,
                resource: &I,
                data: &U,
            ) {
                data.destroyed(state, client, resource);
            }
        }

        impl<$( $( $lt $( : $clt $(+ $dlt )* )? ),+, )? I, U>
            smithay::reexports::wayland_server::GlobalDispatch<I, U> for $state
        where
            I: crate::upstream_protocols::UpstreamProtocol,
            U: smithay::wayland::GlobalDispatch2<I, $state>,
        {
            fn bind(
                state: &mut Self,
                display: &smithay::reexports::wayland_server::DisplayHandle,
                client: &smithay::reexports::wayland_server::Client,
                resource: smithay::reexports::wayland_server::New<I>,
                data: &U,
                init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>,
            ) {
                data.bind(state, display, client, resource, init);
            }

            fn can_view(client: smithay::reexports::wayland_server::Client, data: &U) -> bool {
                data.can_view(&client)
            }
        }
    };
}

pub(crate) use delegate_upstream_protocols;
