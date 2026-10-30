use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::{LayerSurface, WindowSurfaceType, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::{LayerSurface as WlrLayerSurface, LayerSurfaceData};

use super::super::WaylandState;

/// Registers a protocol layer surface with the output-local Smithay map.
///
/// The map deliberately owns placement, anchors, margins, exclusive zones,
/// and output enter/leave events. Halley only chooses the output and tracks
/// whether the client has reached its buffer-mapped state.
pub fn new_surface(
    wayland: &mut WaylandState,
    surface: WlrLayerSurface,
    output: Option<Output>,
    namespace: String,
) {
    let Some(output) = output else {
        surface.send_close();
        return;
    };

    let wl_surface = surface.wl_surface().clone();
    let layer = LayerSurface::new(surface, namespace);
    match layer_map_for_output(&output).map_layer(&layer) {
        Ok(()) => {
            super::super::display_scale::send_tree(&wl_surface, &output);
            wayland.unmapped_layers.insert(wl_surface);
        }
        Err(err) => {
            eventline::warn!(
                "layer-shell: failed to map surface to {}: {err}",
                output.name()
            );
            layer.layer_surface().send_close();
        }
    }
}

/// Applies the layer-shell part of a root-surface commit.
///
/// Returns `true` when `root` belongs to layer shell. The desktop layer
/// remains registered in Smithay's map while buffer-unmapped because the map
/// is also the protocol's configure-size calculator.
pub fn handle_commit(wayland: &mut WaylandState, root: &WlSurface) -> bool {
    let Some((output, layer)) = layer_for_root(wayland, root) else {
        return false;
    };

    let mut map = layer_map_for_output(&output);
    map.arrange();
    super::super::display_scale::send_tree(root, &output);

    let has_buffer =
        with_renderer_surface_state(root, |state| state.buffer().is_some()).unwrap_or(false);
    if has_buffer {
        wayland.unmapped_layers.remove(root);
    } else {
        wayland.unmapped_layers.insert(root.clone());
        super::super::focus::forget_layer(wayland, root);

        if !initial_configure_sent(root) {
            layer.layer_surface().send_configure();
        }
    }

    true
}

pub fn destroyed(wayland: &mut WaylandState, surface: &WlrLayerSurface) {
    wayland.unmapped_layers.remove(surface.wl_surface());
    super::super::focus::forget_layer(wayland, surface.wl_surface());

    let found = wayland.space.outputs().find_map(|output| {
        let map = layer_map_for_output(output);
        map.layers()
            .find(|layer| layer.layer_surface() == surface)
            .cloned()
            .map(|layer| (output.clone(), layer))
    });

    if let Some((output, layer)) = found {
        layer_map_for_output(&output).unmap_layer(&layer);
    }
}

pub fn cleanup(wayland: &mut WaylandState) {
    for output in wayland.space.outputs() {
        layer_map_for_output(output).cleanup();
    }
    wayland.popup_manager.cleanup();
}

pub fn send_frames(output: &Output, elapsed: Duration, sequence: u32) {
    let map = layer_map_for_output(output);
    for layer in map.layers() {
        layer.send_frame(
            output,
            elapsed,
            super::super::frame_callbacks::FALLBACK_THROTTLE,
            |surface, states| {
                super::super::frame_callbacks::callback_output(
                    surface, states, output, sequence, true,
                )
            },
        );
    }
}

fn layer_for_root(wayland: &WaylandState, root: &WlSurface) -> Option<(Output, LayerSurface)> {
    wayland.space.outputs().find_map(|output| {
        let map = layer_map_for_output(output);
        map.layer_for_surface(root, WindowSurfaceType::TOPLEVEL)
            .cloned()
            .map(|layer| (output.clone(), layer))
    })
}

fn initial_configure_sent(surface: &WlSurface) -> bool {
    with_states(surface, |states| {
        states
            .data_map
            .get::<LayerSurfaceData>()
            .is_some_and(|data| data.lock().unwrap().initial_configure_sent)
    })
}
use std::time::Duration;
