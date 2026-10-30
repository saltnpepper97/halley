//! State owned by session startup and process launching.

use super::{
    autostart::Autostart, environment::LaunchEnvironment, startup_clusters::StartupClusters,
};

pub(super) struct LaunchState {
    pub(super) environment: LaunchEnvironment,
    pub(super) autostart: Autostart,
    pub(super) clusters: StartupClusters,
    pub(super) publish_environment: bool,
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
}
