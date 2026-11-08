//! The replacement scene for Cluster Composer.

use super::*;

pub(super) fn cluster_composer(
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    request: &mut RenderRequest<'_>,
    overlay_snapshot: crate::shell::overlay::OverlaySnapshot,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let mut elements = cluster_composer_elements(
        super::overview::OverviewScene {
            renderer,
            output,
            output_geometry,
            desktop: super::overview::OverviewDesktop::from_request(request),
            draw: super::overview::OverviewRenderers {
                node: request.resources.node_renderer,
                cluster: request.resources.cluster_renderer,
                titlebar: request.resources.titlebar_renderer,
                decoration: request.resources.window_decoration_renderer,
                text: request.resources.ui_text,
                previews: request.resources.overlay_previews,
            },
            now: request.frame.target_presentation_time,
        },
        request.overlays.cluster_composer,
        crate::shell::cluster_composer::ComposerStyle::default(),
        request.overlays.overlay_config,
    )?;
    append_overlay_shadows(
        renderer,
        output,
        "cluster-composer",
        request.visuals.shadows.overlay,
        request.resources.shadow_renderer,
        &mut elements,
    )?;
    elements.extend(
        crate::render::layer_surface_elements(renderer, output, Layer::Background)
            .into_iter()
            .map(SceneElement::Layer),
    );
    append_background(
        renderer,
        request.resources.background_renderer,
        BackgroundSceneRequest {
            output,
            output_geometry,
            cameras: request.desktop.cameras,
            config: request.visuals.background,
            config_dir: request.visuals.background_base,
            now: request.frame.target_presentation_time,
        },
        &mut elements,
    );

    let mut naming = crate::render::overlays::cluster_creation::elements(
        &mut crate::render::draw::DrawContext {
            renderer,
            node: request.resources.node_renderer,
            text: request.resources.ui_text,
        },
        request.resources.cluster_creation_overlay,
        output,
        output_geometry,
        request.desktop.clusters.creation(),
        request.cursor.cursor_position,
        request
            .overlays
            .cluster_composer
            .session()
            .map_or(1.0, |session| {
                session.naming_alpha(request.frame.target_presentation_time)
            }),
        request.overlays.overlay_config,
        request.visuals.decorations,
    )?;
    append_compositor_overlay_blur(
        renderer,
        output,
        OverlayEffectStyle {
            output_size: output_geometry.size,
            identity: "cluster-creation",
            blur: request.visuals.blur,
            shadow: request.visuals.shadows.overlay,
        },
        request.resources.backdrop_blur_renderer,
        request.resources.shadow_renderer,
        &mut naming,
    )?;
    elements.splice(0..0, naming);

    chrome::prepend_common_overlays(
        renderer,
        output,
        output_geometry,
        request,
        overlay_snapshot,
        &mut elements,
    )?;
    prepend_pointer_elements(
        renderer,
        output,
        output_geometry,
        &request.cursor,
        request.frame.target_presentation_time,
        true,
        &mut elements,
    )?;
    Ok(elements)
}
