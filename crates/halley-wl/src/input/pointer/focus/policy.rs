use std::time::Instant;

use crate::compositor::interaction::HitNode;
use crate::compositor::root::Halley;
use halley_config::InputFocusMode;

pub(crate) fn apply_hover_focus_mode(st: &mut Halley, hit: Option<HitNode>, blocked: bool, now: Instant) {
    if !hover_focus_enabled(
        st.runtime.tuning.input.focus_mode,
        blocked,
        crate::compositor::monitor::layer_shell::keyboard_focus_is_layer_surface(st),
    ) {
        return;
    }
    let Some(hit) = hit else {
        return;
    };
    if hit.is_core {
        return;
    }
    let Some(node) = st.model.field.node(hit.node_id) else {
        return;
    };
    if node.kind != halley_core::field::NodeKind::Surface
        || !st.model.field.is_visible(hit.node_id)
        || !matches!(
            node.state,
            halley_core::field::NodeState::Active | halley_core::field::NodeState::Drifting
        )
    {
        return;
    }

    st.focus_pointer_target(hit.node_id, 30_000, now);
}

pub(crate) fn hover_focus_enabled(
    focus_mode: InputFocusMode,
    blocked: bool,
    layer_shell_keyboard_focus: bool,
) -> bool {
    !blocked && focus_mode == InputFocusMode::Hover && !layer_shell_keyboard_focus
}
