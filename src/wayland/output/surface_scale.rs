//! Send scale preferences to an entire surface tree, including subsurfaces.
use smithay::desktop::utils::with_surfaces_surface_tree;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::send_surface_state;
use smithay::wayland::fractional_scale::with_fractional_scale;

pub fn send_tree(surface: &WlSurface, output: &Output) {
    with_surfaces_surface_tree(surface, |surface, states| {
        send_surface_state(
            surface,
            states,
            output.current_scale().integer_scale(),
            output.current_transform(),
        );
        with_fractional_scale(states, |fractional| {
            fractional.set_preferred_scale(output.current_scale().fractional_scale());
        });
    });
}
