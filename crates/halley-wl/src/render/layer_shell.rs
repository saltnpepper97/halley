use std::time::Instant;

use smithay::{
    backend::renderer::{
        element::{Kind, surface::render_elements_from_surface_tree},
        gles::GlesRenderer,
    },
    utils::{Logical, Physical, Size},
    wayland::shell::wlr_layer::Layer,
};

use crate::state::Halley;

type LayerElements =
    Vec<smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>>;

pub(crate) fn collect_layer_surfaces(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
    size: Size<i32, Physical>,
    _now: Instant,
) -> (LayerElements, LayerElements, LayerElements, LayerElements) {
    let mut background = Vec::new();
    let mut bottom = Vec::new();
    let mut top = Vec::new();
    let mut overlay = Vec::new();

    let logical_size: Size<i32, Logical> = (size.w, size.h).into();

    for placement in st.layer_shell_placements(logical_size) {
        let elements = render_elements_from_surface_tree(
            renderer,
            &placement.wl_surface,
            (placement.origin.x, placement.origin.y),
            1.0,
            1.0,
            Kind::Unspecified,
        );

        match placement.layer {
            Layer::Background => background.extend(elements),
            Layer::Bottom => bottom.extend(elements),
            Layer::Top => top.extend(elements),
            Layer::Overlay => overlay.extend(elements),
        }
    }

    (background, bottom, top, overlay)
}
