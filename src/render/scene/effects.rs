use super::*;

#[derive(Clone, Copy)]
pub(super) struct OverlayEffectStyle {
    pub output_size: smithay::utils::Size<i32, Logical>,
    pub identity: &'static str,
    pub blur: halley_config::Blur,
    pub shadow: halley_config::ShadowLayer,
}

pub(super) fn append_compositor_overlay_blur(
    renderer: &mut GlesRenderer,
    output: &Output,
    style: OverlayEffectStyle,
    backdrop_blur_renderer: &mut crate::render::effects::backdrop_blur::BackdropBlurRenderer,
    shadow_renderer: &mut crate::render::effects::shadow::ShadowRenderer,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    if style.blur.overlays {
        let patches = elements
            .iter()
            .filter_map(|element| match element {
                SceneElement::NodeLabel(card) => {
                    Some(crate::render::effects::backdrop_blur::BlurPatch {
                        rect: card.geometry(Scale::from(1.0)),
                        radius: card.corner_radius(),
                        alpha: card.alpha(),
                        clip: None,
                    })
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if let Some(blur) = backdrop_blur_renderer.blur_element(
            renderer,
            &output.name(),
            crate::render::effects::backdrop_blur::BlurIdentity::Overlay(style.identity),
            style.output_size,
            patches,
            style.blur,
            0,
            output.current_transform(),
        )? {
            elements.push(SceneElement::BackdropBlur(blur));
        }
    }
    append_overlay_shadows(
        renderer,
        output,
        style.identity,
        style.shadow,
        shadow_renderer,
        elements,
    )?;
    Ok(())
}

pub(super) fn append_overlay_shadows(
    renderer: &mut GlesRenderer,
    output: &Output,
    identity: &str,
    shadow_config: halley_config::ShadowLayer,
    shadow_renderer: &mut crate::render::effects::shadow::ShadowRenderer,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let casters = elements
        .iter()
        .filter_map(|element| match element {
            SceneElement::NodeLabel(card) => Some((
                card.geometry(Scale::from(1.0)),
                card.corner_radius(),
                card.alpha(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (index, (caster, radius, alpha)) in casters.into_iter().enumerate() {
        if let Some(shadow) = shadow_renderer.element(
            renderer,
            format!("{}:overlay:{identity}:{index}", output.name()),
            caster,
            radius,
            alpha,
            shadow_config,
        )? {
            elements.push(SceneElement::Shadow(shadow));
        }
    }
    Ok(())
}

pub(super) fn layer_surface_scene_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    layer: Layer,
    layer_rules: &[halley_config::LayerRule],
    block_screencast: bool,
    blur_config: halley_config::Blur,
    backdrop_blur_renderer: &mut crate::render::effects::backdrop_blur::BackdropBlurRenderer,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let map = layer_map_for_output(output);
    let scale = Scale::from(1.0);
    let output_bounds = Rectangle::<i32, Physical>::from_size(output_geometry.size.to_physical(1));
    let mut elements = Vec::new();

    for surface in map.layers_on(layer).rev() {
        // A layer-shell object remains in Smithay's map after a null-buffer
        // commit so it can be configured and mapped again. Its cached surface
        // size also remains available, so without this mapped-state check we
        // would keep emitting the backdrop blur after the client disappeared.
        let has_buffer =
            with_renderer_surface_state(surface.wl_surface(), |state| state.buffer().is_some())
                .unwrap_or(false);
        if !has_buffer {
            continue;
        }
        let Some(geometry) = map.layer_geometry(surface) else {
            continue;
        };
        let rule = layer_rules
            .iter()
            .find(|rule| rule.matches(surface.namespace(), layer_shell_layer(surface.layer())));
        if block_screencast && rule.is_some_and(|rule| rule.block_screencast) {
            let mut protected: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
            for (popup, popup_offset) in PopupManager::popups_for_surface(surface.wl_surface()) {
                let origin = geometry.loc + popup_offset - popup.geometry().loc;
                protected.extend(
                    smithay::backend::renderer::element::surface::render_elements_from_surface_tree(
                        renderer,
                        popup.wl_surface(),
                        origin.to_f64().to_physical(scale).to_i32_round(),
                        scale,
                        1.0,
                        Kind::Unspecified,
                    ),
                );
            }
            protected.extend(
                smithay::backend::renderer::element::surface::render_elements_from_surface_tree(
                    renderer,
                    surface.wl_surface(),
                    geometry.loc.to_f64().to_physical(scale).to_i32_round(),
                    scale,
                    1.0,
                    Kind::Unspecified,
                ),
            );
            elements.extend(protected.into_iter().map(|element| {
                SceneElement::Border(super::super::solid_color_element(
                    Id::new(),
                    element.geometry(Scale::from(1.0)),
                    smithay::backend::renderer::Color32F::BLACK,
                ))
            }));
            continue;
        }
        let rule_blur = rule.and_then(|rule| rule.blur);
        for (popup, popup_offset) in PopupManager::popups_for_surface(surface.wl_surface()) {
            let popup_origin = geometry.loc + popup_offset - popup.geometry().loc;
            let location = popup_origin.to_f64().to_physical(scale).to_i32_round();
            elements.extend(
                smithay::backend::renderer::element::surface::render_elements_from_surface_tree(
                    renderer,
                    popup.wl_surface(),
                    location,
                    scale,
                    1.0,
                    Kind::ScanoutCandidate,
                )
                .into_iter()
                .map(SceneElement::Layer),
            );
            append_surface_backdrop_blur(
                renderer,
                output,
                output_geometry.size,
                output_bounds,
                popup.wl_surface(),
                popup_origin,
                scale,
                rule_blur,
                blur_config,
                backdrop_blur_renderer,
                &mut elements,
            )?;
        }

        let location = geometry.loc.to_f64().to_physical(scale).to_i32_round();
        elements.extend(
            smithay::backend::renderer::element::surface::render_elements_from_surface_tree(
                renderer,
                surface.wl_surface(),
                location,
                scale,
                1.0,
                Kind::ScanoutCandidate,
            )
            .into_iter()
            .map(SceneElement::Layer),
        );
        append_surface_backdrop_blur(
            renderer,
            output,
            output_geometry.size,
            output_bounds,
            surface.wl_surface(),
            geometry.loc,
            scale,
            rule_blur,
            blur_config,
            backdrop_blur_renderer,
            &mut elements,
        )?;
    }

    Ok(elements)
}

fn layer_shell_layer(layer: Layer) -> halley_config::LayerShellLayer {
    match layer {
        Layer::Background => halley_config::LayerShellLayer::Background,
        Layer::Bottom => halley_config::LayerShellLayer::Bottom,
        Layer::Top => halley_config::LayerShellLayer::Top,
        Layer::Overlay => halley_config::LayerShellLayer::Overlay,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn append_surface_backdrop_blur(
    renderer: &mut GlesRenderer,
    output: &Output,
    output_size: smithay::utils::Size<i32, Logical>,
    output_bounds: Rectangle<i32, Physical>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    surface_origin: smithay::utils::Point<i32, Logical>,
    scale: Scale<f64>,
    rule_blur: Option<bool>,
    blur_config: halley_config::Blur,
    backdrop_blur_renderer: &mut crate::render::effects::backdrop_blur::BackdropBlurRenderer,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let Some(surface_size) =
        with_renderer_surface_state(surface, |state| state.surface_size()).flatten()
    else {
        return Ok(());
    };
    let mut requested = if rule_blur != Some(false) {
        crate::wayland::background_effect::blur_rects(surface, surface_size)
    } else {
        Vec::new()
    };
    if requested.is_empty() && rule_blur == Some(true) {
        requested.push(Rectangle::from_size(surface_size));
    }
    let patches = requested
        .into_iter()
        .filter_map(|rect| {
            let rect = Rectangle::<i32, Logical>::new(surface_origin + rect.loc, rect.size)
                .to_f64()
                .to_physical(scale)
                .to_i32_up();
            rect.intersection(output_bounds).map(|rect| {
                crate::render::effects::backdrop_blur::BlurPatch {
                    rect,
                    radius: 0.0,
                    alpha: 1.0,
                    clip: None,
                }
            })
        })
        .collect::<Vec<_>>();
    if let Some(blur) = backdrop_blur_renderer.blur_element(
        renderer,
        &output.name(),
        crate::render::effects::backdrop_blur::BlurIdentity::Layer(Id::from_wayland_resource(
            surface,
        )),
        output_size,
        patches,
        blur_config,
        0,
        output.current_transform(),
    )? {
        elements.push(SceneElement::BackdropBlur(blur));
    }
    Ok(())
}
