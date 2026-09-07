//! Visibility-aware idle inhibition.
//!
//! The protocol object count is retained in `WaylandState`; whether those
//! objects currently inhibit idle is derived from renderer visibility.

use smithay::backend::renderer::element::{
    RenderElementStates, default_primary_scanout_output_compare,
};
use smithay::desktop::utils::{
    surface_primary_scanout_output, update_surface_primary_scanout_output,
    with_surfaces_surface_tree,
};
use smithay::desktop::{PopupManager, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;
use smithay::wayland::idle_inhibit::IdleInhibitHandler;
use smithay::wayland::seat::WaylandFocus;

use crate::session::{Session, SessionDriver};

impl<D: SessionDriver> IdleInhibitHandler for Session<D> {
    fn inhibit(&mut self, surface: WlSurface) {
        *self.wayland.idle_inhibitors.entry(surface).or_default() += 1;
        self.refresh_idle_inhibit();
        self.request_redraw();
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        if let Some(count) = self.wayland.idle_inhibitors.get_mut(&surface) {
            *count -= 1;
            if *count == 0 {
                self.wayland.idle_inhibitors.remove(&surface);
            }
        }
        self.refresh_idle_inhibit();
    }
}

impl<D: SessionDriver> Session<D> {
    /// Updates inhibitor ownership after composing an output. Smithay's
    /// element states distinguish actually visible surfaces from mapped but
    /// fully occluded ones.
    pub fn update_idle_inhibit_visibility(
        &mut self,
        output: &Output,
        element_states: &RenderElementStates,
    ) {
        for window in self.wayland.space.elements() {
            window.with_surfaces(|surface, states| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    states,
                    None,
                    element_states,
                    default_primary_scanout_output_compare,
                );
            });
            if let Some(root) = window.wl_surface() {
                for (popup, _) in PopupManager::popups_for_surface(root.as_ref()) {
                    with_surfaces_surface_tree(popup.wl_surface(), |surface, states| {
                        update_surface_primary_scanout_output(
                            surface,
                            output,
                            states,
                            None,
                            element_states,
                            default_primary_scanout_output_compare,
                        );
                    });
                }
            }
        }
        for layer in layer_map_for_output(output).layers() {
            // Layer popups are separate surface trees. Match LayerSurface's
            // frame-callback traversal so visible menus receive output-rate
            // callbacks instead of the hidden-surface fallback throttle.
            layer.with_surfaces(|surface, states| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    states,
                    None,
                    element_states,
                    default_primary_scanout_output_compare,
                );
            });
        }
        for lock_surface in self.session_lock.surfaces_for_output(output) {
            with_surfaces_surface_tree(lock_surface.wl_surface(), |surface, states| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    states,
                    None,
                    element_states,
                    default_primary_scanout_output_compare,
                );
            });
        }
        if let Some(icon) = self.wayland.dnd_icon.as_ref() {
            with_surfaces_surface_tree(&icon.surface, |surface, states| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    states,
                    None,
                    element_states,
                    default_primary_scanout_output_compare,
                );
            });
        }
        for surface in self.wayland.idle_inhibitors.keys() {
            with_states(surface, |states| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    states,
                    None,
                    element_states,
                    default_primary_scanout_output_compare,
                );
            });
        }
        self.refresh_idle_inhibit();
    }

    pub fn clear_idle_inhibit_output(&mut self, output: &Output) {
        self.update_idle_inhibit_visibility(output, &RenderElementStates::default());
    }

    pub(crate) fn refresh_idle_inhibit(&mut self) {
        self.wayland
            .idle_inhibitors
            .retain(|surface, count| surface.is_alive() && *count > 0);
        let visible = self.wayland.idle_inhibitors.keys().any(|surface| {
            with_states(surface, |states| {
                surface_primary_scanout_output(surface, states).is_some()
            })
        });
        self.idle_notifier_state.set_is_inhibited(visible);
    }
}
