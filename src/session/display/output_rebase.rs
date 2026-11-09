//! Keep Field geometry attached to its monitor when effective layout positions change.

use smithay::utils::{Logical, Point};
use smithay::wayland::seat::WaylandFocus;

use super::{Session, SessionDriver};

impl<D: SessionDriver> Session<D> {
    pub(crate) fn translate_output_geometry(&mut self, output: &str, delta: Point<i32, Logical>) {
        if delta == (0, 0).into() {
            return;
        }
        let windows = self
            .wayland
            .space
            .elements()
            .filter(|window| crate::wayland::window_output_name(window).as_deref() == Some(output))
            .cloned()
            .collect::<Vec<_>>();
        for window in &windows {
            self.xwayland.translate_window(window, delta);
            if let Some(location) = self.wayland.space.element_location(window) {
                self.wayland
                    .space
                    .relocate_element(window, location + delta);
            }
            if let Some(surface) = window.wl_surface() {
                self.window_animations
                    .translate_source(surface.as_ref(), delta);
                self.opening_origins
                    .translate_surface(surface.as_ref(), delta);
            }
        }
        let cores = self.clusters.translate_output(output, delta);
        self.nodes.translate_output(output, delta, &cores);
        self.maximize.translate_output(output, delta);
        self.fullscreen.translate_output(output, delta);
        self.interactions
            .field_arrange
            .translate_output(output, delta);
        self.shell.cluster_composer.translate_output(output, delta);
        self.interactions
            .translate_output_grab(output, delta, &self.nodes);
    }
}
