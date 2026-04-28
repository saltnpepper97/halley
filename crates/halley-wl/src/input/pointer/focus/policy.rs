use std::time::Instant;

use crate::compositor::interaction::HitNode;
use crate::compositor::root::Halley;
use halley_config::InputFocusMode;

pub(crate) fn apply_hover_focus_mode(
    st: &mut Halley,
    hit: Option<HitNode>,
    blocked: bool,
    now: Instant,
) {
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
    if node.kind != halley_core::field::NodeKind::Surface || !st.model.field.is_visible(hit.node_id)
    {
        return;
    }
    if !matches!(
        node.state,
        halley_core::field::NodeState::Active
            | halley_core::field::NodeState::Drifting
            | halley_core::field::NodeState::Node
    ) {
        return;
    }

    if crate::compositor::surface::is_active_stacking_workspace_member(st, hit.node_id) {
        let monitor = st
            .model
            .monitor_state
            .node_monitor
            .get(&hit.node_id)
            .cloned()
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
        let front = crate::compositor::surface::active_stacking_front_member_for_monitor(
            st,
            monitor.as_str(),
        );
        if Some(hit.node_id) != front {
            return;
        }
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
