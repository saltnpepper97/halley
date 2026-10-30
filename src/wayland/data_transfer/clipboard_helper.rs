//! wl-clipboard's unprivileged fallback borrows keyboard focus with a 1×1
//! toplevel to obtain a selection serial. It is a protocol helper, not a
//! workspace member. Remember the caller before granting that temporary focus.
use std::sync::Mutex;

use smithay::desktop::LayerSurface;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

#[derive(Clone, Default)]
pub(crate) struct SavedFocus {
    pub(crate) window: Option<WlSurface>,
    pub(crate) layer: Option<LayerSurface>,
}

struct HelperFocus(Mutex<SavedFocus>);

pub(crate) fn is_helper(surface: &WlSurface) -> bool {
    with_states(surface, |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .is_some_and(|data| {
                data.lock().unwrap().app_id.as_deref() == Some("io.github.bugaevc.wl-clipboard")
            })
    })
}

pub(crate) fn saved_focus(surface: &WlSurface) -> Option<SavedFocus> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<HelperFocus>()
            .map(|focus| focus.0.lock().unwrap().clone())
    })
}

pub(crate) fn remember_focus(
    surface: &WlSurface,
    window: Option<&WlSurface>,
    layer: Option<&LayerSurface>,
) {
    // Neovim can launch clipboard and primary-selection helpers together.
    // A second helper must return to the original app, even if the first
    // helper has already disappeared when the second one closes.
    let saved = window.and_then(saved_focus).unwrap_or_else(|| SavedFocus {
        window: window.cloned(),
        layer: layer.cloned(),
    });
    with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing(|| HelperFocus(Mutex::new(SavedFocus::default())));
        *states
            .data_map
            .get::<HelperFocus>()
            .unwrap()
            .0
            .lock()
            .unwrap() = saved;
    });
}
