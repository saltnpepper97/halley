use std::time::Instant;

use super::root::Halley;

pub(crate) fn active(st: &Halley) -> bool {
    st.ui.render_state.exit_confirm_visible()
}

pub(crate) fn show(st: &mut Halley) {
    st.begin_modal_keyboard_capture();
    let mut monitors: Vec<String> = st.model.monitor_state.monitors.keys().cloned().collect();
    if monitors.is_empty() {
        monitors.push(st.model.monitor_state.current_monitor.clone());
    }
    for monitor in monitors {
        st.ui.render_state.show_exit_confirm(monitor.as_str());
    }
}

pub(crate) fn clear(st: &mut Halley) {
    let mut monitors: Vec<String> = st
        .ui
        .render_state
        .overlays
        .overlay_exit_confirm
        .keys()
        .cloned()
        .collect();
    if monitors.is_empty() {
        monitors.push(st.model.monitor_state.current_monitor.clone());
    }
    for monitor in monitors {
        st.ui.render_state.clear_exit_confirm(monitor.as_str());
    }
    let restore_focus = st
        .last_input_surface_node_for_monitor(st.model.monitor_state.current_monitor.as_str())
        .or(st.last_input_surface_node());
    st.schedule_modal_focus_restore(restore_focus, Instant::now());
}
