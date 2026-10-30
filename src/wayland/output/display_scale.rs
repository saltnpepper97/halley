//! Output ownership determines the rendering scale, even for hidden windows.

use smithay::desktop::{PopupManager, Window, WindowSurfaceType, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::seat::WaylandFocus;

use super::super::WaylandState;

pub use super::super::surface_scale::send_tree;

pub fn send_window(window: &Window, output: &Output) {
    if let Some(root) = window.wl_surface() {
        send_tree(root.as_ref(), output);
        for (popup, _) in PopupManager::popups_for_surface(root.as_ref()) {
            send_tree(popup.wl_surface(), output);
        }
    }
}

pub fn output_for_surface(wayland: &WaylandState, surface: &WlSurface) -> Option<Output> {
    let root = super::super::compositor::root_surface(surface);
    let root = wayland
        .popup_manager
        .find_popup(&root)
        .and_then(|popup| smithay::desktop::find_popup_root_surface(&popup).ok())
        .unwrap_or(root);
    if let Some(name) = wayland
        .space
        .elements()
        .chain(wayland.unmapped.values())
        .chain(wayland.collapsed.values())
        .find(|window| {
            window
                .wl_surface()
                .is_some_and(|candidate| candidate.as_ref() == &root)
        })
        .and_then(super::super::window_output_name)
    {
        return wayland
            .space
            .outputs()
            .find(|output| output.name() == name)
            .cloned();
    }
    wayland
        .space
        .outputs()
        .find(|output| {
            layer_map_for_output(output)
                .layer_for_surface(&root, WindowSurfaceType::ALL)
                .is_some()
        })
        .cloned()
}

pub fn refresh(wayland: &WaylandState, primary: &Output) {
    for window in wayland
        .space
        .elements()
        .chain(wayland.unmapped.values())
        .chain(wayland.collapsed.values())
    {
        let output = wayland
            .space
            .outputs()
            .find(|output| super::super::window_is_on_output(window, output, primary))
            .unwrap_or(primary);
        send_window(window, output);
    }
    for output in wayland.space.outputs() {
        for layer in layer_map_for_output(output).layers() {
            send_tree(layer.wl_surface(), output);
            for (popup, _) in PopupManager::popups_for_surface(layer.wl_surface()) {
                send_tree(popup.wl_surface(), output);
            }
        }
    }
}
