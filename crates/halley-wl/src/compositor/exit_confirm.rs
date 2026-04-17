use std::ops::{Deref, DerefMut};
use std::time::Instant;

use super::root::Halley;

pub(crate) struct ExitConfirmController<T> {
    st: T,
}

pub(crate) fn exit_confirm_controller<T>(st: T) -> ExitConfirmController<T> {
    ExitConfirmController { st }
}

impl<T: Deref<Target = Halley>> Deref for ExitConfirmController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for ExitConfirmController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

impl<T: Deref<Target = Halley>> ExitConfirmController<T> {
    pub(crate) fn active(&self) -> bool {
        self.ui.render_state.exit_confirm_visible()
    }
}

impl<T: DerefMut<Target = Halley>> ExitConfirmController<T> {
    pub(crate) fn show(&mut self) {
        self.begin_modal_keyboard_capture();
        let mut monitors: Vec<String> = self.model.monitor_state.monitors.keys().cloned().collect();
        if monitors.is_empty() {
            monitors.push(self.model.monitor_state.current_monitor.clone());
        }
        for monitor in monitors {
            self.ui.render_state.show_exit_confirm(monitor.as_str());
        }
    }

    pub(crate) fn clear(&mut self) {
        let mut monitors: Vec<String> = self
            .ui
            .render_state
            .overlay_exit_confirm
            .keys()
            .cloned()
            .collect();
        if monitors.is_empty() {
            monitors.push(self.model.monitor_state.current_monitor.clone());
        }
        for monitor in monitors {
            self.ui.render_state.clear_exit_confirm(monitor.as_str());
        }
        let restore_focus = self
            .last_input_surface_node_for_monitor(self.model.monitor_state.current_monitor.as_str())
            .or(self.last_input_surface_node());
        self.schedule_modal_focus_restore(restore_focus, Instant::now());
    }
}
