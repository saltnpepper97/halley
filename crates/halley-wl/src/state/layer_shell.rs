use smithay::{
    reexports::wayland_server::{Resource, protocol::wl_output::WlOutput, protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size},
    wayland::{
        compositor::with_states,
        shell::wlr_layer::{
            Anchor, ExclusiveZone, KeyboardInteractivity, Layer, LayerSurface, LayerSurfaceCachedState,
        },
    },
};

use super::HalleyWlState;

#[derive(Clone)]
pub(crate) struct LayerPlacement {
    pub wl_surface: WlSurface,
    pub layer: Layer,
    pub origin: Point<i32, Logical>,
    pub keyboard_interactivity: KeyboardInteractivity,
}

impl HalleyWlState {
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

        let _ = (output, layer, namespace);
    }

    pub(crate) fn remove_layer_surface(&mut self, surface: &LayerSurface) {
        if let Some(output) = &self.primary_output {
            output.leave(surface.wl_surface());
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
            *states.cached_state.get::<LayerSurfaceCachedState>().current()
        })
    }

    pub(crate) fn configure_layer_shell_surfaces(&mut self, output_size: Size<i32, Logical>) {
        let output_rect = Rectangle::from_size(output_size);
        let mut zone = output_rect;

        for surface in self.layer_shell_surfaces_sorted() {
            let data = Self::layer_cached_state(&surface);
            let (_, size) = compute_layer_placement(output_rect, &mut zone, data);
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
        let Some(keyboard) = self.seat.get_keyboard() else {
            return false;
        };
        keyboard
            .current_focus()
            .is_some_and(|focus| self.is_layer_surface(&focus))
    }

    pub(crate) fn is_layer_surface(&self, surface: &WlSurface) -> bool {
        self.wlr_layer_shell_state
            .layer_surfaces()
            .any(|layer| layer.wl_surface().id() == surface.id())
    }

    pub(crate) fn focus_layer_surface(&mut self, surface: &WlSurface) -> bool {
        let Some(interactivity) = self.layer_shell_placements(self.layer_output_size()).into_iter().find_map(|placement| {
            (placement.wl_surface.id() == surface.id()).then_some(placement.keyboard_interactivity)
        }) else {
            return false;
        };

        if interactivity == KeyboardInteractivity::None {
            return false;
        }

        self.interaction_focus = None;
        self.interaction_focus_until_ms = 0;

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
}

fn layer_depth(layer: Layer) -> u8 {
    match layer {
        Layer::Background => 0,
        Layer::Bottom => 1,
        Layer::Top => 2,
        Layer::Overlay => 3,
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
