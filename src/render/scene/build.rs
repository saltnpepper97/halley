//! Scene entry points, capture privacy, and output scaling.

use super::*;

pub fn build(
    renderer: &mut GlesRenderer,
    output: &Output,
    primary_output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    request: RenderRequest<'_>,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    build_with_privacy(
        renderer,
        output,
        primary_output,
        output_geometry,
        request,
        false,
    )
}

pub(crate) fn build_with_privacy(
    renderer: &mut GlesRenderer,
    output: &Output,
    primary_output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    request: RenderRequest<'_>,
    block_screencast: bool,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    build_with_capture_privacy(
        renderer,
        output,
        primary_output,
        output_geometry,
        request,
        block_screencast,
        false,
    )
}

pub(crate) fn build_with_screenshot_privacy(
    renderer: &mut GlesRenderer,
    output: &Output,
    primary_output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    request: RenderRequest<'_>,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let protect = request.desktop.window_rules.has_capture_blocks(true);
    build_with_capture_privacy(
        renderer,
        output,
        primary_output,
        output_geometry,
        request,
        protect,
        true,
    )
}

fn build_with_capture_privacy(
    renderer: &mut GlesRenderer,
    output: &Output,
    primary_output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    mut request: RenderRequest<'_>,
    block_screencast: bool,
    screenshot: bool,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let scale = output.current_scale().fractional_scale();
    let locked = request.desktop.session_lock.active();
    request.resources.ui_text.set_display_scale(scale);
    request.resources.backdrop_blur_renderer.set_display_scale(
        &output.name(),
        scale,
        crate::render::output_physical_size(output),
    );
    let privacy = CapturePrivacy::new(
        block_screencast,
        screenshot,
        request.desktop.window_rules.has_capture_blocks(screenshot),
        request
            .desktop
            .layer_rules
            .iter()
            .any(|rule| rule.block_screencast),
    );
    let elements = build_logical(
        renderer,
        output,
        primary_output,
        output_geometry,
        &mut request,
        privacy,
    )?;
    request
        .resources
        .backdrop_blur_renderer
        .end_scene(&output.name());
    let elements = if privacy.redact_cached_scene() {
        elements
            .into_iter()
            .filter(|element| {
                !matches!(
                    element,
                    SceneElement::ScreenshotPreview(_) | SceneElement::BackdropBlur(_)
                )
            })
            .collect::<Vec<_>>()
    } else {
        elements
    };
    Ok(elements
        .into_iter()
        .map(|element| crate::render::display_scale::element(element, scale, locked))
        .collect())
}
