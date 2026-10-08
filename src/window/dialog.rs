//! Focus policy for client-declared modal dialogs. Parenting and stacking are
//! shared with ordinary transients; modal hints only redirect family focus.

use smithay::desktop::{Space, Window};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::{XdgToplevelSurfaceData, dialog::ToplevelDialogHint};

/// Resolve the frontmost eligible modal descendant, including nested dialogs.
/// Restrict the search to mapped windows and the requested parent's family:
/// modal hints must never make an unrelated application lose focus.
pub(crate) fn focus_target(
    space: &Space<Window>,
    requested: &Window,
    eligible: impl Fn(&Window) -> bool,
) -> Option<Window> {
    space
        .elements()
        .rev()
        .find(|candidate| {
            let Some(toplevel) = candidate.toplevel().filter(|toplevel| toplevel.alive()) else {
                return false;
            };
            let modal = with_states(toplevel.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .is_some_and(|data| {
                        data.lock().unwrap().dialog_hint == ToplevelDialogHint::Modal
                    })
            });
            modal
                && eligible(candidate)
                && super::stacking::is_descendant_of(space, candidate, requested)
        })
        .cloned()
}
