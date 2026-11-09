/// Renderer-independent state owned by the compositor shell.
///
/// Shell features remain separate state machines, but their shared ownership
/// is explicit at the session boundary instead of appearing as unrelated
/// global fields.
pub struct ShellState {
    pub(crate) overlays: super::overlay::OverlayManager,
    pub(crate) bearings: super::bearings::BearingsState,
    pub(crate) focus_cycle: super::focus_cycle::FocusCycleState,
    pub(crate) cluster_composer: super::cluster_composer::ClusterComposerState,
}

impl ShellState {
    pub fn new(config: &halley_config::RuntimeConfig) -> Self {
        Self {
            overlays: super::overlay::OverlayManager::default(),
            bearings: super::bearings::BearingsState::new(config.bearings),
            focus_cycle: super::focus_cycle::FocusCycleState::default(),
            cluster_composer: super::cluster_composer::ClusterComposerState::default(),
        }
    }
}
