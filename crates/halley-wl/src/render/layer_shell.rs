use std::collections::HashSet;
use std::time::Instant;

use smithay::{
    backend::renderer::{
        element::{Element, Kind, surface::render_elements_from_surface_tree},
        gles::GlesRenderer,
    },
    desktop::{PopupKind, PopupManager, find_popup_root_surface, utils::bbox_from_surface_tree},
    reexports::wayland_server::Resource,
    utils::{Logical, Physical, Rectangle, Scale, Size},
    wayland::shell::wlr_layer::Layer,
};

use crate::compositor::root::Halley;
use crate::protocol::wayland::background_effect::surface_wants_background_blur;

type LayerElements =
    Vec<smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>>;

pub(crate) struct LayerSurfaceRenderGroup {
    pub(crate) elements: LayerElements,
    pub(crate) dst: Rectangle<i32, Physical>,
    pub(crate) blur: bool,
    pub(crate) is_aperture: bool,
}

pub(crate) type LayerGroups = Vec<LayerSurfaceRenderGroup>;

fn layer_shell_blur_enabled(st: &Halley, layer: Layer) -> bool {
    if !st.runtime.tuning.effects.blur.enabled {
        return false;
    }
    match st.runtime.tuning.effects.blur.layer_shell {
        halley_config::ClientBlurMode::Off => false,
        halley_config::ClientBlurMode::Auto => matches!(layer, Layer::Top | Layer::Overlay),
        halley_config::ClientBlurMode::Always => !matches!(layer, Layer::Background),
    }
}

fn layer_group_dst(
    elements: &[smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<
        GlesRenderer,
    >],
    origin: (i32, i32),
    size: Size<i32, Logical>,
) -> Rectangle<i32, Physical> {
    let mut out: Option<Rectangle<i32, Physical>> = None;
    for element in elements {
        let geometry = element.geometry(Scale::from(1.0));
        out = Some(match out {
            Some(existing) => existing.merge(geometry),
            None => geometry,
        });
    }
    out.unwrap_or_else(|| {
        Rectangle::<i32, Physical>::new(origin.into(), (size.w.max(1), size.h.max(1)).into())
    })
}

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
) -> (LayerGroups, LayerGroups, LayerGroups, LayerGroups) {
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
        let mut layer_popups_blur = false;
        let mut rendered_popups = HashSet::new();
        let mut popups: Vec<_> = PopupManager::popups_for_surface(&placement.wl_surface).collect();
        popups.reverse();
        for (popup, popup_offset) in popups {
            rendered_popups.insert(popup.wl_surface().id());
            layer_popups_blur |= surface_wants_background_blur(popup.wl_surface());
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
            layer_popups_blur |= surface_wants_background_blur(popup.wl_surface());
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

        let is_aperture =
            crate::compositor::monitor::layer_shell::surface_is_aperture(st, &placement.wl_surface);
        let aperture_blur = {
            #[cfg(feature = "aperture")]
            {
                st.aperture_config().peek.blur && is_aperture
            }
            #[cfg(not(feature = "aperture"))]
            {
                false
            }
        };
        let group = LayerSurfaceRenderGroup {
            dst: layer_group_dst(
                &elements,
                (placement.origin.x, placement.origin.y),
                placement.size,
            ),
            blur: layer_shell_blur_enabled(st, placement.layer)
                || surface_wants_background_blur(&placement.wl_surface)
                || aperture_blur,
            is_aperture,
            elements,
        };

        match placement.layer {
            Layer::Background => background.push(group),
            Layer::Bottom => bottom.push(group),
            Layer::Top => top.push(group),
            Layer::Overlay => overlay.push(group),
        }
        if !layer_popups.is_empty() {
            let dst = layer_group_dst(&layer_popups, (0, 0), logical_size);
            overlay.push(LayerSurfaceRenderGroup {
                elements: layer_popups,
                dst,
                blur: layer_popups_blur,
                is_aperture: false,
            });
        }
    }

    (background, bottom, top, overlay)
}
