//! Shared shell and FPS overlay composition for all scene modes.

use super::*;

pub(super) fn prepend_common_overlays(
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    request: &mut RenderRequest<'_>,
    overlay_snapshot: crate::shell::overlay::OverlaySnapshot,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let mut overlay_elements = crate::render::overlays::shell::elements(
        renderer,
        output_geometry,
        overlay_snapshot,
        request.overlays.overlay_config,
        request.resources.node_renderer,
        request.resources.ui_text,
    )?;
    append_compositor_overlay_blur(
        renderer,
        output,
        OverlayEffectStyle {
            output_size: output_geometry.size,
            identity: "shell-overlay",
            blur: request.visuals.blur,
            shadow: request.visuals.shadows.overlay,
        },
        request.resources.backdrop_blur_renderer,
        request.resources.shadow_renderer,
        &mut overlay_elements,
    )?;
    elements.splice(0..0, overlay_elements);

    if request.visuals.debug.overlay_fps {
        let mut fps_elements = crate::render::overlays::fps::elements(
            renderer,
            &output.name(),
            request.frame.target_presentation_time,
            request.resources.debug_fps_overlay,
            request.overlays.overlay_config,
            request.visuals.decorations,
            request.resources.node_renderer,
            request.resources.ui_text,
        )?;
        append_compositor_overlay_blur(
            renderer,
            output,
            OverlayEffectStyle {
                output_size: output_geometry.size,
                identity: "debug-fps",
                blur: request.visuals.blur,
                shadow: request.visuals.shadows.overlay,
            },
            request.resources.backdrop_blur_renderer,
            request.resources.shadow_renderer,
            &mut fps_elements,
        )?;
        elements.splice(0..0, fps_elements);
    }
    Ok(())
}
