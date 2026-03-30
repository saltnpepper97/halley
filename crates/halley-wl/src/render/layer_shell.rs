use std::collections::HashSet;
use std::time::Instant;

use smithay::{
    backend::renderer::{
        element::{surface::render_elements_from_surface_tree, Kind},
        gles::GlesRenderer,
    },
    desktop::{find_popup_root_surface, utils::bbox_from_surface_tree, PopupKind, PopupManager},
    reexports::wayland_server::Resource,
    utils::{Logical, Physical, Size},
    wayland::shell::wlr_layer::Layer,
};

use crate::compositor::root::Halley;

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

    for placement in
        crate::compositor::monitor::layer_shell::layer_shell_placements(st, logical_size)
    {
        let elements = render_elements_from_surface_tree(
            renderer,
            &placement.wl_surface,
            (placement.origin.x, placement.origin.y),
            1.0,
            1.0,
            Kind::Unspecified,
        );
        let mut layer_popups = Vec::new();
        let mut rendered_popups = HashSet::new();
        let mut popups: Vec<_> = PopupManager::popups_for_surface(&placement.wl_surface).collect();
        popups.reverse();
        for (popup, popup_offset) in popups {
            rendered_popups.insert(popup.wl_surface().id());
            let popup_geo = popup.geometry();
            let popup_origin = clamp_layer_popup_origin(
                &popup,
                (
                    placement.origin.x + popup_offset.x - popup_geo.loc.x,
                    placement.origin.y + popup_offset.y - popup_geo.loc.y,
                ),
                logical_size,
            );
            layer_popups.extend(render_elements_from_surface_tree(
                renderer,
                popup.wl_surface(),
                popup_origin,
                1.0,
                1.0,
                Kind::Unspecified,
            ));
        }

        for popup in st.platform.xdg_shell_state.popup_surfaces() {
            let popup_kind = PopupKind::from(popup.clone());
            if rendered_popups.contains(&popup.wl_surface().id()) {
                continue;
            }
            let Ok(root) = find_popup_root_surface(&popup_kind) else {
                continue;
            };
            if root.id() != placement.wl_surface.id() {
                continue;
            }
            let popup_geo = popup_kind.geometry();
            let popup_origin = clamp_layer_popup_origin(
                &popup_kind,
                (
                    placement.origin.x + popup_geo.loc.x,
                    placement.origin.y + popup_geo.loc.y,
                ),
                logical_size,
            );
            layer_popups.extend(render_elements_from_surface_tree(
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
        overlay.extend(layer_popups);
    }

    (background, bottom, top, overlay)
}
