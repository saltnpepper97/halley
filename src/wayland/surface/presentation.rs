use smithay::backend::renderer::element::RenderElementStates;
use smithay::desktop::utils::{
    OutputPresentationFeedback, surface_presentation_feedback_flags_from_states,
    take_presentation_feedback_surface_tree,
};
use smithay::desktop::{Space, Window, layer_map_for_output};
use smithay::output::Output;

use crate::wayland::window_is_on_output;

/// Takes presentation callbacks for the surface trees painted by one
/// submitted output frame.
///
/// Halley assigns each managed window to one output, so that assignment is
/// the authoritative primary presentation output even while compositor-owned
/// transforms animate its contents. Per-surface render states still determine
/// whether Smithay may truthfully advertise zero-copy presentation.
pub fn take_output_feedback(
    output: &Output,
    primary_output: &Output,
    space: &Space<Window>,
    session_lock: &crate::wayland::session_lock::State,
    element_states: &RenderElementStates,
) -> OutputPresentationFeedback {
    let mut feedback = OutputPresentationFeedback::new(output);

    if session_lock.active() {
        for surface in session_lock.surfaces_for_output(output) {
            take_presentation_feedback_surface_tree(
                surface.wl_surface(),
                &mut feedback,
                |surface, _| {
                    element_states
                        .element_was_presented(surface)
                        .then(|| output.clone())
                },
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(surface, None, element_states)
                },
            );
        }
        return feedback;
    }

    space
        .elements()
        .filter(|window| window_is_on_output(window, output, primary_output))
        .for_each(|window| {
            window.take_presentation_feedback(
                &mut feedback,
                |surface, _| {
                    element_states
                        .element_was_presented(surface)
                        .then(|| output.clone())
                },
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(surface, None, element_states)
                },
            );
        });

    let map = layer_map_for_output(output);
    for layer in map.layers() {
        layer.take_presentation_feedback(
            &mut feedback,
            |surface, _| {
                element_states
                    .element_was_presented(surface)
                    .then(|| output.clone())
            },
            |surface, _| {
                surface_presentation_feedback_flags_from_states(surface, None, element_states)
            },
        );
    }

    feedback
}
