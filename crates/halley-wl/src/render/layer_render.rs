use std::time::Instant;

use smithay::{
    backend::renderer::{
        element::{Kind, surface::render_elements_from_surface_tree},
        gles::GlesRenderer,
    },
    utils::{Logical, Physical, Size},
    wayland::shell::wlr_layer::Layer,
};

use crate::state::HalleyWlState;

pub(crate) fn collect_layer_surfaces(
    renderer: &mut GlesRenderer,
    st: &mut HalleyWlState,
    size: Size<i32, Physical>,
    _now: Instant,
) -> (
    Vec<smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>>,
    Vec<smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>>,
) {
    let mut under = Vec::new();
    let mut over = Vec::new();
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
            Layer::Background | Layer::Bottom => under.extend(elements),
            Layer::Top | Layer::Overlay => over.extend(elements),
        }
    }

    (under, over)
}
