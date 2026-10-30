//! State owned by session startup and process launching.

use super::{
    autostart::Autostart, environment::LaunchEnvironment, startup_clusters::StartupClusters,
};

pub(super) struct LaunchState {
    environment: LaunchEnvironment,
    autostart: Autostart,
    pub(super) clusters: StartupClusters,
    publish_environment: bool,
}

impl LaunchState {
    pub(super) fn new(
        environment: LaunchEnvironment,
        autostart: Autostart,
        publish_environment: bool,
    ) -> Self {
        Self {
            environment,
            autostart,
            clusters: StartupClusters::default(),
            publish_environment,
        }
    }
    pub(super) fn environment(&self) -> &LaunchEnvironment {
        &self.environment
    }

    pub(super) fn reload_environment(
        &mut self,
        values: &std::collections::BTreeMap<String, String>,
        cursor_theme: &str,
    ) -> Option<std::ffi::OsString> {
        self.environment.reload(values);
        self.environment.set_cursor_theme(cursor_theme);
        self.environment.path()
    }

    pub(super) fn publish_cursor_settings(&self, theme: &str, size: u8) {
        if self.publish_environment {
            super::environment::publish_cursor_settings(theme, size);
        }
    }

    pub(super) fn arm_once(&mut self, display: &std::ffi::OsStr, commands: Vec<String>) {
        self.autostart.arm_once(display, commands);
    }

    pub(super) fn run_once(&mut self, x11_display: Option<&std::ffi::OsStr>, cursor_size: u8) {
        self.autostart
            .run_once(x11_display, cursor_size, &self.environment);
    }

    pub(super) fn run_reload(
        &mut self,
        commands: &[String],
        x11_display: Option<&std::ffi::OsStr>,
        cursor_size: u8,
    ) {
        self.autostart
            .run_reload(commands, x11_display, cursor_size, &self.environment);
    }

    pub(super) fn reap_finished(&mut self) {
        self.autostart.reap_finished();
    }

    pub(super) fn shutdown(&mut self) {
        self.autostart.shutdown();
    }
}
