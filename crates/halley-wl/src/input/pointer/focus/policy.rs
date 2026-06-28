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
    // While a seat grab is active (e.g. an open xdg_popup menu, or a drag), the
    // grab owns focus. Changing the interaction focus here would reassert keyboard
    // focus onto a window and break the popup keyboard grab, dismissing the menu.
    if st
        .platform
        .seat
        .get_pointer()
        .is_some_and(|pointer| pointer.is_grabbed())
    {
        return;
    }
    if !hover_focus_enabled(
        st.runtime.tuning.input.focus_mode,
        blocked,
        crate::compositor::monitor::layer_shell::layer_keyboard_focus_is_modal(st),
    ) {
        return;
    }
    let Some(hit) = hit else {
        return;
    };
    let Some(node) = st.model.field.node(hit.node_id) else {
        return;
    };
    if !st.model.field.is_visible(hit.node_id) {
        return;
    }
    let focusable = match node.kind {
        halley_core::field::NodeKind::Surface => matches!(
            node.state,
            halley_core::field::NodeState::Active
                | halley_core::field::NodeState::Drifting
                | halley_core::field::NodeState::Node
        ),
        halley_core::field::NodeKind::Core => node.state == halley_core::field::NodeState::Core,
    };
    if !focusable {
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

    // Hover-focus is not a deliberate pick: don't let pointer-enter resume a
    // soft-suspended fullscreen window (e.g. a game you alt+tabbed away from).
    st.input
        .interaction_state
        .suppress_fullscreen_resume_on_focus = true;
    st.focus_pointer_target(hit.node_id, 30_000, now);
    st.input
        .interaction_state
        .suppress_fullscreen_resume_on_focus = false;
}

pub(crate) fn hover_focus_enabled(
    focus_mode: InputFocusMode,
    blocked: bool,
    layer_shell_keyboard_focus: bool,
) -> bool {
    !blocked && focus_mode == InputFocusMode::Hover && !layer_shell_keyboard_focus
}
