//! Collect live and exclusive window scenes for the current output.

use super::*;

pub(super) struct LiveWindowCollection {
    pub normal: Vec<(usize, LiveWindowScene)>,
    pub exclusive: Vec<(usize, LiveWindowScene)>,
}

pub(super) fn collect(
    renderer: &mut GlesRenderer,
    primary_output: &Output,
    context: LiveWindowContext<'_>,
    mut renderers: LiveWindowRenderers<'_>,
    close_animations: &crate::render::close::WindowCloseAnimations,
    screencast: &crate::capture::screencast::ScreencastState,
    privacy: CapturePrivacy,
) -> Result<LiveWindowCollection, Box<dyn Error>> {
    let output = context.output;
    let mut live_windows = Vec::new();
    let mut exclusive_windows = Vec::new();
    for (stack_index, window) in context.space.elements().enumerate() {
        if !crate::wayland::window_is_on_output(window, output, primary_output) {
            continue;
        }
        if window.wl_surface().is_some_and(|surface| {
            let animations = &close_animations;
            animations.has_pending(surface.as_ref()) || animations.is_active(surface.as_ref())
        }) {
            continue;
        }
        let mut window_scene =
            live_window_elements(renderer, window, context, renderers.reborrow())?;
        let protected = if privacy.enabled() {
            let rule = window
                .wl_surface()
                .map(|surface| context.window_rules.applied(surface.as_ref()));
            privacy.redact_window(
                rule.as_ref(),
                privacy.redact_cached_windows() && crate::xwayland::is_override_redirect(window),
            )
        } else {
            false
        };
        if protected {
            omit_window_scene(&mut window_scene);
        } else if !privacy.enabled()
            && window
                .wl_surface()
                .is_some_and(|surface| screencast.is_window_cast(surface.as_ref()))
        {
            indicate_window_cast(&mut window_scene, context.decorations);
        }
        if window_scene.cluster_exclusive {
            exclusive_windows.push((stack_index, window_scene));
        } else if !window_scene.elements.is_empty() || !window_scene.popup_elements.is_empty() {
            live_windows.push((stack_index, window_scene));
        }
        let extra_presentation = window
            .wl_surface()
            .and_then(|surface| context.nodes.id_for_surface(surface.as_ref()))
            .and_then(|id| {
                context.clusters.extra_window_presentation(
                    id,
                    &output.name(),
                    context.target_presentation_time,
                )
            });
        if let Some(extra_presentation) = extra_presentation {
            let mut extra_scene = live_window_elements(
                renderer,
                window,
                LiveWindowContext {
                    cluster_presentation_override: Some(extra_presentation),
                    instance_identity: Some("stack-cycle-extra"),
                    ..context
                },
                renderers.reborrow(),
            )?;
            if protected {
                omit_window_scene(&mut extra_scene);
            } else if !privacy.enabled()
                && window
                    .wl_surface()
                    .is_some_and(|surface| screencast.is_window_cast(surface.as_ref()))
            {
                indicate_window_cast(&mut extra_scene, context.decorations);
            }
            if !extra_scene.elements.is_empty() || !extra_scene.popup_elements.is_empty() {
                live_windows.push((stack_index, extra_scene));
            }
        }
    }
    Ok(LiveWindowCollection {
        normal: live_windows,
        exclusive: exclusive_windows,
    })
}
