use std::env;
use std::sync::OnceLock;

mod input_events;
mod input_utils;
mod key_actions;
mod move_anim;
mod pointer_button;
mod pointer_focus;
mod pointer_motion;
mod resize_helpers;

pub(crate) use input_events::{BackendInputEventData, handle_backend_input_event};
pub(crate) use move_anim::advance_node_move_anim;
pub(crate) use resize_helpers::{active_node_screen_rect, active_resize_geometry_screen};

pub(crate) fn pointer_map_debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        env::var("HALLEY_WL_DEBUG_POINTER_MAP")
            .ok()
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    })
}
