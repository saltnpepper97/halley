use smithay::{
    reexports::wayland_server::{
        Resource, protocol::wl_output::WlOutput, protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size},
    wayland::{
        compositor::with_states,
        shell::wlr_layer::{
            Anchor, ExclusiveZone, KeyboardInteractivity, Layer, LayerSurface,
            LayerSurfaceCachedState,
        },
    },
};
use std::time::Instant;

use super::HalleyWlState;

#[derive(Clone)]
pub(crate) struct LayerPlacement {
    pub wl_surface: WlSurface,
    pub layer: Layer,
    pub origin: Point<i32, Logical>,
    pub keyboard_interactivity: KeyboardInteractivity,
}

impl HalleyWlState {
    fn apply_layer_surface_focus(
        &mut self,
        surface: &WlSurface,
        interactivity: KeyboardInteractivity,
    ) -> bool {
        if interactivity == KeyboardInteractivity::None {
            return false;
        }

        self.interaction_focus = None;
        self.interaction_focus_until_ms = 0;
        self.layer_keyboard_focus = Some(surface.id());
        if self
            .active_locked_pointer_surface()
            .is_some_and(|locked_surface| locked_surface.id() != surface.id())
        {
            self.release_active_pointer_constraint();
        }

        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(surface.clone()), SERIAL_COUNTER.next_serial());
        }
        self.update_selection_focus_from_surface(Some(surface));

        for top in self.xdg_shell_state.toplevel_surfaces() {
            let changed = top.with_pending_state(|state| {
                let was_active = state.states.contains(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated,
                );
                state.states.unset(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated,
                );
                was_active
            });
            if changed {
                top.send_configure();
            }
        }

        true
    }

    fn layer_focus_candidate_surface(&self) -> Option<WlSurface> {
        let mut placements = self.layer_shell_placements(self.layer_output_size());
        placements.sort_by_key(|placement| std::cmp::Reverse(layer_depth(placement.layer)));
        placements.into_iter().find_map(|placement| {
            layer_surface_can_take_keyboard_focus(placement.keyboard_interactivity)
                .then_some(placement.wl_surface)
        })
    }

    pub(crate) fn register_layer_surface(
        &mut self,
        surface: LayerSurface,
        output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        let size = self.layer_output_size();
        surface.with_pending_state(|state| {
            state.size = Some(size);
        });
        surface.send_configure();

        let assigned_monitor = if let Some(requested_output) = output.as_ref() {
            self.outputs
                .iter()
                .find_map(|(name, output)| output.owns(requested_output).then_some(name.clone()))
                .unwrap_or_else(|| self.current_monitor.clone())
        } else {
            self.current_monitor.clone()
        };
        self.assign_layer_surface_to_monitor(surface.wl_surface(), assigned_monitor);

        if let Some(requested_output) = output.as_ref() {
            for output in self.outputs.values() {
                if output.owns(requested_output) {
                    output.enter(surface.wl_surface());
                }
            }
        } else if let Some(primary_output) = &self.primary_output {
            primary_output.enter(surface.wl_surface());
        }

        let interactivity = Self::layer_cached_state(&surface).keyboard_interactivity;
        if layer_surface_can_take_keyboard_focus(interactivity) {
            let _ = self.apply_layer_surface_focus(surface.wl_surface(), interactivity);
        }

        let _ = (layer, namespace);
    }

    /// Called on every surface commit. If this surface is a layer surface that
    /// requests keyboard focus and doesn't already have it, grant it now.
    /// This is the correct time to do it — `register_layer_surface` fires before
    /// the client has committed its desired `keyboard_interactivity`, so the
    /// cached state is still the default `None` at that point.
    pub(crate) fn maybe_grant_layer_surface_focus_on_commit(&mut self, surface: &WlSurface) {
        // Bail fast if this surface already holds layer focus.
        if self.layer_keyboard_focus == Some(surface.id()) {
            return;
        }

        // Check whether this surface is a layer surface and read its
        // *current* (post-commit) keyboard interactivity.
        let Some(interactivity) = self
            .wlr_layer_shell_state
            .layer_surfaces()
            .find_map(|layer| {
                (layer.wl_surface().id() == surface.id())
                    .then_some(Self::layer_cached_state(&layer).keyboard_interactivity)
            })
        else {
            return; // not a layer surface
        };

        if layer_surface_can_take_keyboard_focus(interactivity) {
            let _ = self.apply_layer_surface_focus(surface, interactivity);
        }
    }

    pub(crate) fn remove_layer_surface(&mut self, surface: &LayerSurface) {
        let removed_focused_layer = self.layer_keyboard_focus == Some(surface.wl_surface().id());
        self.layer_surface_monitor.remove(&surface.wl_surface().id());
        if removed_focused_layer {
            self.layer_keyboard_focus = None;
        }
        for output in self.outputs.values() {
            output.leave(surface.wl_surface());
        }
        if !removed_focused_layer {
            return;
        }

        if let Some(next_layer) = self.layer_focus_candidate_surface() {
            let _ = self.focus_layer_surface(&next_layer);
            return;
        }

        if let Some(id) = self.last_input_surface_node() {
            self.set_interaction_focus(Some(id), 30_000, Instant::now());
        }
    }

    pub(crate) fn layer_output_size(&self) -> Size<i32, Logical> {
        (
            self.zoom_ref_size.x.round().max(1.0) as i32,
            self.zoom_ref_size.y.round().max(1.0) as i32,
        )
            .into()
    }

    fn layer_cached_state(surface: &LayerSurface) -> LayerSurfaceCachedState {
        with_states(surface.wl_surface(), |states| {
            *states
                .cached_state
                .get::<LayerSurfaceCachedState>()
                .current()
        })
    }

    pub(crate) fn configure_layer_shell_surfaces(&mut self, output_size: Size<i32, Logical>) {
        let output_rect = Rectangle::from_size(output_size);
        let mut zone = output_rect;

        for surface in self.layer_shell_surfaces_sorted() {
            if !self.layer_surface_on_current_monitor(surface.wl_surface()) {
                continue;
            }
            let data = Self::layer_cached_state(&surface);
            let (_, size) = compute_layer_placement(output_rect, &mut zone, data);
            if data.size == size {
                continue;
            }
            surface.with_pending_state(|state| {
                state.size = Some(size);
            });
            let _ = surface.send_pending_configure();
        }
    }

    pub(crate) fn layer_shell_placements(
        &self,
        output_size: Size<i32, Logical>,
    ) -> Vec<LayerPlacement> {
        let output_rect = Rectangle::from_size(output_size);
        let mut zone = output_rect;
        let mut placements = Vec::new();

        for surface in self.layer_shell_surfaces_sorted() {
            if !self.layer_surface_on_current_monitor(surface.wl_surface()) {
                continue;
            }
            let data = Self::layer_cached_state(&surface);
            let (origin, _) = compute_layer_placement(output_rect, &mut zone, data);
            placements.push(LayerPlacement {
                wl_surface: surface.wl_surface().clone(),
                layer: data.layer,
                origin,
                keyboard_interactivity: data.keyboard_interactivity,
            });
        }

        placements
    }

    fn layer_shell_surfaces_sorted(&self) -> Vec<LayerSurface> {
        let mut surfaces: Vec<_> = self
            .wlr_layer_shell_state
            .layer_surfaces()
            .filter(|surface| surface.alive())
            .collect();
        surfaces.sort_by_key(|surface| layer_depth(Self::layer_cached_state(surface).layer));
        surfaces
    }

    pub(crate) fn keyboard_focus_is_layer_surface(&self) -> bool {
        if self.layer_keyboard_focus.is_some() {
            return true;
        }
        let Some(keyboard) = self.seat.get_keyboard() else {
            return false;
        };
        keyboard
            .current_focus()
            .is_some_and(|focus| self.is_layer_surface(&focus))
    }

    fn layer_focus_surface(&self) -> Option<WlSurface> {
        let focus_id = self.layer_keyboard_focus.clone()?;
        self.wlr_layer_shell_state
            .layer_surfaces()
            .find_map(|layer| {
                (layer.wl_surface().id() == focus_id).then(|| layer.wl_surface().clone())
            })
    }

    pub(crate) fn reassert_layer_surface_keyboard_focus_if_drifted(&mut self) {
        let desired_focus = self.layer_focus_surface();
        if desired_focus.is_none() {
            self.layer_keyboard_focus = None;
            return;
        }

        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let current_focus = keyboard.current_focus();
        let matches = match (&current_focus, &desired_focus) {
            (Some(current), Some(desired)) => current.id() == desired.id(),
            (None, None) => true,
            _ => false,
        };
        if matches {
            return;
        }

        keyboard.set_focus(self, desired_focus.clone(), SERIAL_COUNTER.next_serial());
        self.update_selection_focus_from_surface(desired_focus.as_ref());
    }

    pub(crate) fn is_layer_surface(&self, surface: &WlSurface) -> bool {
        self.wlr_layer_shell_state
            .layer_surfaces()
            .any(|layer| layer.wl_surface().id() == surface.id())
    }

    pub(crate) fn focus_layer_surface(&mut self, surface: &WlSurface) -> bool {
        let Some(interactivity) = self
            .wlr_layer_shell_state
            .layer_surfaces()
            .find_map(|layer| {
                (layer.wl_surface().id() == surface.id())
                    .then_some(Self::layer_cached_state(&layer).keyboard_interactivity)
            })
        else {
            return false;
        };

        self.apply_layer_surface_focus(surface, interactivity)
    }
}

fn layer_surface_can_take_keyboard_focus(interactivity: KeyboardInteractivity) -> bool {
    interactivity != KeyboardInteractivity::None
}

fn layer_depth(layer: Layer) -> u8 {
    match layer {
        Layer::Background => 0,
        Layer::Bottom => 1,
        Layer::Top => 2,
        Layer::Overlay => 3,
    }
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::layer_surface_can_take_keyboard_focus;
    use smithay::wayland::shell::wlr_layer::KeyboardInteractivity;

    #[test]
    fn keyboard_interactive_layer_surfaces_are_focus_eligible() {
        assert!(!layer_surface_can_take_keyboard_focus(
            KeyboardInteractivity::None
        ));
        assert!(layer_surface_can_take_keyboard_focus(
            KeyboardInteractivity::OnDemand
        ));
        assert!(layer_surface_can_take_keyboard_focus(
            KeyboardInteractivity::Exclusive
        ));
    }
}

fn compute_layer_placement(
    output_rect: Rectangle<i32, Logical>,
    zone: &mut Rectangle<i32, Logical>,
    data: LayerSurfaceCachedState,
) -> (Point<i32, Logical>, Size<i32, Logical>) {
    let mut source = match data.exclusive_zone {
        ExclusiveZone::Exclusive(_) | ExclusiveZone::Neutral => *zone,
        ExclusiveZone::DontCare => output_rect,
    };

    if data.anchor.contains(Anchor::LEFT) {
        source.size.w -= data.margin.left;
    }
    if data.anchor.contains(Anchor::RIGHT) {
        source.size.w -= data.margin.right;
    }
    if data.anchor.contains(Anchor::TOP) {
        source.size.h -= data.margin.top;
    }
    if data.anchor.contains(Anchor::BOTTOM) {
        source.size.h -= data.margin.bottom;
    }

    let mut size = data.size;
    size.w = size.w.min(source.size.w);
    size.h = size.h.min(source.size.h);
    if size.w == 0 {
        size.w = source.size.w / 2;
    }
    if size.h == 0 {
        size.h = source.size.h / 2;
    }
    if data.anchor.anchored_horizontally() {
        size.w = source.size.w;
    }
    if data.anchor.anchored_vertically() {
        size.h = source.size.h;
    }
    size.w = size.w.max(1);
    size.h = size.h.max(1);

    let x = if data.anchor.contains(Anchor::LEFT) {
        source.loc.x + data.margin.left
    } else if data.anchor.contains(Anchor::RIGHT) {
        source.loc.x + (source.size.w - size.w)
    } else {
        source.loc.x + ((source.size.w / 2) - (size.w / 2))
    };

    let y = if data.anchor.contains(Anchor::TOP) {
        source.loc.y + data.margin.top
    } else if data.anchor.contains(Anchor::BOTTOM) {
        source.loc.y + (source.size.h - size.h)
    } else {
        source.loc.y + ((source.size.h / 2) - (size.h / 2))
    };

    if let ExclusiveZone::Exclusive(amount) = data.exclusive_zone {
        let amount = amount as i32;
        match data.anchor {
            anchors if anchors.contains(Anchor::TOP) && !anchors.contains(Anchor::BOTTOM) => {
                zone.loc.y += amount + data.margin.top;
                zone.size.h -= amount + data.margin.top;
            }
            anchors if anchors.contains(Anchor::BOTTOM) && !anchors.contains(Anchor::TOP) => {
                zone.size.h -= amount + data.margin.bottom;
            }
            anchors if anchors.contains(Anchor::LEFT) && !anchors.contains(Anchor::RIGHT) => {
                zone.loc.x += amount + data.margin.left;
                zone.size.w -= amount + data.margin.left;
            }
            anchors if anchors.contains(Anchor::RIGHT) && !anchors.contains(Anchor::LEFT) => {
                zone.size.w -= amount + data.margin.right;
            }
            _ => {}
        }
    }

    ((x, y).into(), size)
}
