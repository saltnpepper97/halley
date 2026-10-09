pub mod background_effect;
pub(crate) mod clipboard_helper;
pub mod compositor;
pub mod decoration;
pub mod display_scale;
pub mod dmabuf;
pub mod dnd;
pub mod ext_workspace;
pub mod focus;
pub mod foreign_toplevel;
pub mod frame_callbacks;
pub mod fullscreen;
pub mod idle_inhibit;
mod ime_clients;
pub mod layer_shell;
pub mod permissions;
pub mod popup;
pub mod presentation;
pub(crate) mod screenshot_clipboard;
pub mod selection;
pub mod session_lock;
mod surface_scale;
pub mod text_input;
mod virtual_keyboard;
pub mod wlr_gamma_control;
pub mod wlr_output_management;
pub mod wlr_screencopy;
pub mod xdg_shell;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use smithay::desktop::{LayerSurface, PopupManager, Space, Window};
use smithay::output::Output;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};
use smithay::wayland::background_effect::BackgroundEffectState;
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::content_type::ContentTypeState;
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufState};
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
use smithay::wayland::shell::xdg::dialog::XdgDialogState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::wayland::text_input::TextInputManagerState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;
use smithay::wayland::xdg_activation::XdgActivationState;
use smithay::wayland::xdg_foreign::XdgForeignState;
use smithay::wayland::xdg_toplevel_icon::XdgToplevelIconManager;

/// The one output responsible for painting a window. Smithay's `Space`
/// still owns output geometry and pointer routing; this is only Halley's
/// whole-window handoff policy, matching the original compositor.
struct WindowOutputState {
    assigned: RwLock<String>,
    inherited: RwLock<Option<Arc<WindowOutputState>>>,
}

struct WindowOutput(Arc<WindowOutputState>);

struct WindowPresentationOwner(RwLock<Option<u32>>);

fn new_window_output(name: String) -> WindowOutput {
    WindowOutput(Arc::new(WindowOutputState {
        assigned: RwLock::new(name),
        inherited: RwLock::new(None),
    }))
}

fn window_output_state_name(state: &Arc<WindowOutputState>, depth: usize) -> String {
    if depth < 8 {
        let inherited = state
            .inherited
            .read()
            .expect("window output inheritance lock poisoned")
            .clone();
        if let Some(inherited) = inherited {
            return window_output_state_name(&inherited, depth + 1);
        }
    }
    state
        .assigned
        .read()
        .expect("window output lock poisoned")
        .clone()
}

pub fn set_window_output(window: &Window, output: &Output) {
    let owner = window
        .user_data()
        .get_or_insert_threadsafe(|| new_window_output(output.name()));
    *owner
        .0
        .assigned
        .write()
        .expect("window output lock poisoned") = output.name();
    *owner
        .0
        .inherited
        .write()
        .expect("window output inheritance lock poisoned") = None;
    display_scale::send_window(window, output);
}

/// Makes `window` follow a managed owner's output without allowing writes to
/// the child to mutate the owner. Override-redirect X11 menus use this so an
/// already-open menu follows an owner handed to another output.
pub fn inherit_window_output(window: &Window, owner: &Window) -> bool {
    let Some(owner_output) = owner.user_data().get::<WindowOutput>() else {
        return false;
    };
    let name = window_output_state_name(&owner_output.0, 0);
    let child_output = window
        .user_data()
        .get_or_insert_threadsafe(|| new_window_output(name));
    *child_output
        .0
        .inherited
        .write()
        .expect("window output inheritance lock poisoned") = Some(owner_output.0.clone());
    true
}

pub fn window_output_name(window: &Window) -> Option<String> {
    window
        .user_data()
        .get::<WindowOutput>()
        .map(|owner| window_output_state_name(&owner.0, 0))
}

/// Associates an override-redirect X11 surface with the managed X11 window
/// whose presentation transform it must inherit.
pub fn set_window_presentation_owner(window: &Window, owner: Option<u32>) {
    let state = window
        .user_data()
        .get_or_insert_threadsafe(|| WindowPresentationOwner(RwLock::new(owner)));
    *state
        .0
        .write()
        .expect("window presentation owner lock poisoned") = owner;
}

pub fn window_presentation_owner(window: &Window) -> Option<u32> {
    window
        .user_data()
        .get::<WindowPresentationOwner>()
        .and_then(|owner| {
            *owner
                .0
                .read()
                .expect("window presentation owner lock poisoned")
        })
}

pub fn window_is_on_output(window: &Window, output: &Output, primary: &Output) -> bool {
    window_output_name(window)
        .map(|owner| owner == output.name())
        // Windows are assigned during their initial map. Retaining this
        // fallback keeps an already-mapped window visible if that invariant
        // is ever broken while developing.
        .unwrap_or_else(|| output == primary)
}

#[cfg(test)]
mod window_output_tests {
    use super::{new_window_output, window_output_state_name};

    #[test]
    fn inherited_output_follows_owner_without_child_writes_mutating_it() {
        let owner = new_window_output("DP-1".to_owned());
        let child = new_window_output("fallback".to_owned());
        *child
            .0
            .inherited
            .write()
            .expect("child inheritance lock poisoned") = Some(owner.0.clone());

        *owner.0.assigned.write().expect("owner lock poisoned") = "DP-2".to_owned();
        assert_eq!(window_output_state_name(&child.0, 0), "DP-2");

        *child
            .0
            .inherited
            .write()
            .expect("child inheritance lock poisoned") = None;
        *child.0.assigned.write().expect("child lock poisoned") = "DP-3".to_owned();
        assert_eq!(window_output_state_name(&child.0, 0), "DP-3");
        assert_eq!(window_output_state_name(&owner.0, 0), "DP-2");
    }
}

/// Wayland protocol and shell-model state shared by both session drivers.
///
/// Backend rendering, cameras, input devices, and redraw scheduling stay out
/// of this type. That keeps the Smithay globals and shell lifecycle together
/// without creating a compositor-wide god object.
pub struct WaylandState {
    pub ime_clients: ime_clients::ImeClients,
    pub display_handle: DisplayHandle,
    pub compositor_state: CompositorState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    pub xdg_shell_state: XdgShellState,
    _xdg_dialog_state: XdgDialogState,
    pub xdg_foreign_state: XdgForeignState,
    pub xdg_activation_state: XdgActivationState,
    pub layer_shell_state: WlrLayerShellState,
    // Retained for the lifetime of the advertised ext-background-effect
    // global. Committed per-surface regions live in Smithay's surface cache.
    _background_effect_state: BackgroundEffectState,
    _single_pixel_buffer_state: SinglePixelBufferState,
    _content_type_state: ContentTypeState,
    _xdg_toplevel_icon_manager: XdgToplevelIconManager,
    // Retained for the lifetime of its advertised global.
    _xdg_decoration_state: XdgDecorationState,
    _viewporter_state: ViewporterState,
    _fractional_scale_manager_state: FractionalScaleManagerState,
    _idle_inhibit_manager_state: IdleInhibitManagerState,
    _relative_pointer_manager_state: RelativePointerManagerState,
    _pointer_constraints_state: PointerConstraintsState,
    _pointer_gestures_state: PointerGesturesState,
    _cursor_shape_manager_state: CursorShapeManagerState,
    _virtual_keyboard_manager_state: VirtualKeyboardManagerState,
    _text_input_manager_state: TextInputManagerState,
    _input_method_manager_state: InputMethodManagerState,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    pub shm_state: ShmState,
    // Retained alongside the wl_output globals it serves.
    _output_manager_state: OutputManagerState,
    pub wlr_output_management_state: wlr_output_management::State,
    pub wlr_gamma_control_state: wlr_gamma_control::State,
    pub wlr_screencopy_state: wlr_screencopy::State,
    pub(crate) idle_inhibitors: HashMap<WlSurface, usize>,
    output_globals: HashMap<String, GlobalId>,
    /// Clipboard (ctrl+c/ctrl+v) and drag-and-drop. Smithay owns the actual
    /// transfer - it hands the source client's fd straight to the receiving
    /// client - so this is only the global's state, with no compositor-side
    /// buffering of clipboard contents.
    pub data_device_state: DataDeviceState,
    pub dnd_icon: Option<dnd::DndIcon>,
    /// Middle-click paste. A separate protocol from `data_device_state`, not
    /// a mode of it: clients set the two selections independently and
    /// expect them to hold different contents.
    pub primary_selection_state: PrimarySelectionState,
    /// Clipboard-manager access through the standardized ext-data-control
    /// protocol. It shares the same seat selections as wl_data_device and
    /// primary-selection instead of buffering a second clipboard in Halley.
    pub ext_data_control_state: DataControlState,
    /// Staging `ext-workspace-v1`. Every manager binding, group handle, and
    /// workspace handle the compositor has advertised lives here; the cluster
    /// model itself stays in `Session::clusters`.
    pub ext_workspace_state: ext_workspace::State,
    pub foreign_toplevel_state: foreign_toplevel::State,
    /// Tracks popup trees once for both xdg-toplevel and layer-shell roots.
    /// Rendering and input can then ask Smithay for the same canonical tree
    /// instead of each subsystem inventing its own parent/offset bookkeeping.
    pub popup_manager: PopupManager,
    /// Real, visible windows - a surface only enters `space` once it has
    /// actually attached a buffer (see `unmapped`), not merely once its
    /// toplevel role exists.
    pub space: Space<Window>,
    /// Managed-window stacking independent of render-space implementation
    /// details. Focus succession and future window policy read this order.
    pub managed_windows: crate::window::ManagedWindowStack,
    /// Toplevels that exist but haven't mapped a buffer yet, keyed by their
    /// `wl_surface`. This defers entering `space` until there is a real
    /// buffer to show, without mixing placement policy into surface
    /// lifecycle tracking.
    pub unmapped: HashMap<WlSurface, Window>,
    /// Buffer-backed toplevels intentionally removed from `space` because
    /// Halley is representing them as nodes. Keeping the protocol object
    /// here lets commits, null-buffer unmaps, and destruction continue to
    /// follow the normal Wayland lifecycle while the client is collapsed.
    pub collapsed: HashMap<WlSurface, Window>,
    /// Last mapped position retained across a null-buffer unmap. Initial maps
    /// have no entry and use normal placement policy.
    pub unmapped_locations: HashMap<WlSurface, Point<i32, Logical>>,
    /// Layer surfaces that have not attached a buffer since creation (or
    /// since a null-buffer unmap). The Smithay `LayerMap` retains the role so
    /// it can calculate and send the next configure; this set records the
    /// separate Wayland mapped/unmapped lifecycle.
    pub unmapped_layers: HashSet<WlSurface>,
    /// The one on-demand layer surface selected by a click. Exclusive layer
    /// focus is derived from mapped protocol state and is never stored here.
    pub focused_layer: Option<LayerSurface>,
    /// Persistent normal-window focus. Layer-shell focus is resolved
    /// separately so a temporary launcher or exclusive overlay does not
    /// erase the window that should regain focus after it closes.
    pub focused_window: Option<WlSurface>,
    /// The output selected by the most recent focus click. New toplevels
    /// use it as their spawn output; storing only the stable output name
    /// keeps output handles and camera state in their existing owners.
    pub focused_output: Option<String>,
}

impl WaylandState {
    // Protocol construction is centralized by the generic session. This
    // constructor only assembles the resulting globals with shell state.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        display_handle: DisplayHandle,
        compositor_state: CompositorState,
        dmabuf_state: DmabufState,
        dmabuf_global: Option<DmabufGlobal>,
        xdg_shell_state: XdgShellState,
        xdg_dialog_state: XdgDialogState,
        xdg_foreign_state: XdgForeignState,
        xdg_activation_state: XdgActivationState,
        layer_shell_state: WlrLayerShellState,
        background_effect_state: BackgroundEffectState,
        single_pixel_buffer_state: SinglePixelBufferState,
        content_type_state: ContentTypeState,
        xdg_toplevel_icon_manager: XdgToplevelIconManager,
        xdg_decoration_state: XdgDecorationState,
        viewporter_state: ViewporterState,
        fractional_scale_manager_state: FractionalScaleManagerState,
        idle_inhibit_manager_state: IdleInhibitManagerState,
        relative_pointer_manager_state: RelativePointerManagerState,
        pointer_constraints_state: PointerConstraintsState,
        pointer_gestures_state: PointerGesturesState,
        cursor_shape_manager_state: CursorShapeManagerState,
        virtual_keyboard_manager_state: VirtualKeyboardManagerState,
        text_input_manager_state: TextInputManagerState,
        input_method_manager_state: InputMethodManagerState,
        keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
        shm_state: ShmState,
        output_manager_state: OutputManagerState,
        wlr_output_management_state: wlr_output_management::State,
        wlr_gamma_control_state: wlr_gamma_control::State,
        wlr_screencopy_state: wlr_screencopy::State,
        output_globals: HashMap<String, GlobalId>,
        data_device_state: DataDeviceState,
        primary_selection_state: PrimarySelectionState,
        ext_data_control_state: DataControlState,
        ext_workspace_state: ext_workspace::State,
        foreign_toplevel_state: foreign_toplevel::State,
    ) -> Self {
        Self {
            display_handle,
            compositor_state,
            dmabuf_state,
            dmabuf_global,
            xdg_shell_state,
            _xdg_dialog_state: xdg_dialog_state,
            xdg_foreign_state,
            xdg_activation_state,
            layer_shell_state,
            _background_effect_state: background_effect_state,
            _single_pixel_buffer_state: single_pixel_buffer_state,
            _content_type_state: content_type_state,
            _xdg_toplevel_icon_manager: xdg_toplevel_icon_manager,
            _xdg_decoration_state: xdg_decoration_state,
            _viewporter_state: viewporter_state,
            _fractional_scale_manager_state: fractional_scale_manager_state,
            _idle_inhibit_manager_state: idle_inhibit_manager_state,
            _relative_pointer_manager_state: relative_pointer_manager_state,
            _pointer_constraints_state: pointer_constraints_state,
            _pointer_gestures_state: pointer_gestures_state,
            _cursor_shape_manager_state: cursor_shape_manager_state,
            _virtual_keyboard_manager_state: virtual_keyboard_manager_state,
            ime_clients: Default::default(),
            _text_input_manager_state: text_input_manager_state,
            _input_method_manager_state: input_method_manager_state,
            keyboard_shortcuts_inhibit_state,
            shm_state,
            _output_manager_state: output_manager_state,
            wlr_output_management_state,
            wlr_gamma_control_state,
            wlr_screencopy_state,
            idle_inhibitors: HashMap::new(),
            output_globals,
            data_device_state,
            dnd_icon: None,
            primary_selection_state,
            ext_data_control_state,
            ext_workspace_state,
            foreign_toplevel_state,
            popup_manager: PopupManager::default(),
            space: Space::default(),
            managed_windows: crate::window::ManagedWindowStack::default(),
            unmapped: HashMap::new(),
            collapsed: HashMap::new(),
            unmapped_locations: HashMap::new(),
            unmapped_layers: HashSet::new(),
            focused_layer: None,
            focused_window: None,
            focused_output: None,
        }
    }

    pub fn ensure_output_global<D>(&mut self, output: &Output)
    where
        D: smithay::reexports::wayland_server::GlobalDispatch<
                smithay::reexports::wayland_server::protocol::wl_output::WlOutput,
                smithay::wayland::output::WlOutputData,
            > + 'static,
    {
        self.output_globals
            .entry(output.name())
            .or_insert_with(|| output.create_global::<D>(&self.display_handle));
    }

    pub fn disable_output_global<D: 'static>(&mut self, output: &Output) {
        if let Some(global) = self.output_globals.remove(&output.name()) {
            self.display_handle.disable_global::<D>(global);
        }
    }
}

/// Per-client Wayland state - one instance per connected client.
#[derive(Default)]
pub struct ClientState {
    pub permissions: permissions::Permissions,
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}

    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        match reason {
            DisconnectReason::ConnectionClosed => {
                static CLOSED_CLIENTS: AtomicU64 = AtomicU64::new(0);
                let count = CLOSED_CLIENTS.fetch_add(1, Ordering::Relaxed) + 1;
                if count == 1 || count.is_multiple_of(256) {
                    eventline::debug!(
                        "wayland clients disconnected normally count={count} latest={client_id:?}"
                    );
                }
            }
            DisconnectReason::ProtocolError(error) => {
                eventline::warn!(
                    "wayland client {client_id:?} disconnected after protocol error: {error:?}"
                );
            }
        }
    }
}
