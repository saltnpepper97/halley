use std::time::Instant;

use smithay::{
    backend::renderer::{
        element::{Kind, surface::render_elements_from_surface_tree},
        gles::GlesRenderer,
    },
    desktop::{PopupManager, utils::bbox_from_surface_tree},
    utils::{Logical, Physical, Size},
    wayland::shell::wlr_layer::Layer,
};

use crate::state::Halley;

type LayerElements =
    Vec<smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>>;

fn clamp_layer_popup_origin(
    popup: &smithay::desktop::PopupKind,
    popup_origin: (i32, i32),
    output_size: Size<i32, Logical>,
) -> (i32, i32) {
    let bbox = bbox_from_surface_tree(popup.wl_surface(), (0, 0));
    let mut origin_x = popup_origin.0;
    let mut origin_y = popup_origin.1;

    let left = origin_x + bbox.loc.x;
    let right = left + bbox.size.w;
    if left < 0 {
        origin_x -= left;
    } else if right > output_size.w {
        origin_x -= right - output_size.w;
    }

    let top = origin_y + bbox.loc.y;
    let bottom = top + bbox.size.h;
    if top < 0 {
        origin_y -= top;
    } else if bottom > output_size.h {
        origin_y -= bottom - output_size.h;
    }

    (origin_x, origin_y)
}

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
        let mut elements = render_elements_from_surface_tree(
            renderer,
            &placement.wl_surface,
            (placement.origin.x, placement.origin.y),
            1.0,
            1.0,
            Kind::Unspecified,
        );
        let mut popups: Vec<_> = PopupManager::popups_for_surface(&placement.wl_surface).collect();
        popups.reverse();
        for (popup, popup_offset) in popups {
            let popup_geo = popup.geometry();
            let popup_origin = clamp_layer_popup_origin(
                &popup,
                (
                    placement.origin.x + popup_offset.x - popup_geo.loc.x,
                    placement.origin.y + popup_offset.y - popup_geo.loc.y,
                ),
                logical_size,
            );
            elements.extend(render_elements_from_surface_tree(
                renderer,
                popup.wl_surface(),
                popup_origin,
                1.0,
                1.0,
                Kind::Unspecified,
            ));
        }

        match placement.layer {
            Layer::Background => background.extend(elements),
            Layer::Bottom => bottom.extend(elements),
            Layer::Top => top.extend(elements),
            Layer::Overlay => overlay.extend(elements),
        }
    }

    (background, bottom, top, overlay)
}
