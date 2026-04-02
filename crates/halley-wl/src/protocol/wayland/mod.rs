#![allow(unused_imports)]

use std::time::Instant;

use smithay::{
    backend::allocator::dmabuf::Dmabuf,
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_control, delegate_data_device, delegate_dmabuf,
    delegate_drm_syncobj, delegate_idle_notify, delegate_layer_shell, delegate_output,
    delegate_pointer_constraints, delegate_primary_selection, delegate_relative_pointer,
    delegate_seat, delegate_shm, delegate_viewporter, delegate_xdg_activation,
    delegate_xdg_decoration, delegate_xdg_shell,
    input::{Seat, SeatHandler, SeatState, pointer::CursorImageStatus},
    output::Output,
    reexports::wayland_server::{Client, Resource, backend::ObjectId, protocol::wl_seat},
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState},
        dmabuf::{DmabufFeedback, DmabufGlobal, DmabufHandler, ImportNotifier},
        drm_syncobj::{DrmSyncobjHandler, DrmSyncobjState},
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
        output::{OutputHandler, OutputManagerState},
        pointer_constraints::{PointerConstraintsHandler, PointerConstraintsState},
        relative_pointer::RelativePointerManagerState,
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
            primary_selection::{
                PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
            },
            wlr_data_control::{DataControlHandler, DataControlState},
        },
        shell::{
            wlr_layer::{
                Layer, LayerSurface, LayerSurfaceConfigure, WlrLayerShellHandler,
                WlrLayerShellState,
            },
            xdg::{
                PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
                decoration::{XdgDecorationHandler, XdgDecorationState},
            },
        },
        shm::{ShmHandler, ShmState},
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
    },
};

use crate::compositor::root::Halley;

pub(crate) mod activation;
pub(crate) mod client_state;
mod handlers;
mod handlers_xdg;
mod screencopy;

pub use client_state::ClientState;
