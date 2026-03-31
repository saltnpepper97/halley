use halley_config::{
    InitialWindowClusterParticipation, InitialWindowOverlapPolicy, InitialWindowSpawnPlacement,
    WindowRule,
};
use halley_core::field::NodeId;
use smithay::reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgToplevelSurfaceData};

use crate::compositor::root::Halley;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedInitialWindowRule {
    pub(crate) overlap_policy: InitialWindowOverlapPolicy,
    pub(crate) spawn_placement: InitialWindowSpawnPlacement,
    pub(crate) cluster_participation: InitialWindowClusterParticipation,
}

impl Default for ResolvedInitialWindowRule {
    fn default() -> Self {
        Self {
            overlap_policy: InitialWindowOverlapPolicy::None,
            spawn_placement: InitialWindowSpawnPlacement::Adjacent,
            cluster_participation: InitialWindowClusterParticipation::Layout,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InitialWindowIntent {
    pub(crate) app_id: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) parent_node: Option<NodeId>,
    pub(crate) rule: ResolvedInitialWindowRule,
    pub(crate) matched_rule: bool,
    pub(crate) is_transient: bool,
    pub(crate) prefer_app_intent: bool,
}

impl InitialWindowIntent {
    pub(crate) fn bypassed(&self) -> Self {
        Self {
            app_id: self.app_id.clone(),
            title: self.title.clone(),
            parent_node: self.parent_node,
            rule: ResolvedInitialWindowRule::default(),
            matched_rule: false,
            is_transient: self.is_transient,
            prefer_app_intent: false,
        }
    }

    pub(crate) fn applied_rule_for_node(
        &self,
    ) -> crate::compositor::spawn::state::AppliedInitialWindowRule {
        crate::compositor::spawn::state::AppliedInitialWindowRule {
            overlap_policy: self.effective_overlap_policy(),
            spawn_placement: self.rule.spawn_placement,
            cluster_participation: self.rule.cluster_participation,
            parent_node: self.parent_node,
            suppress_reveal_pan: self.matched_rule,
        }
    }

    pub(crate) fn effective_overlap_policy(&self) -> InitialWindowOverlapPolicy {
        match (self.rule.overlap_policy, self.parent_node) {
            (InitialWindowOverlapPolicy::ParentOnly, None) => InitialWindowOverlapPolicy::None,
            (policy, _) => policy,
        }
    }

    pub(crate) fn effective_spawn_placement(&self) -> InitialWindowSpawnPlacement {
        match self.rule.spawn_placement {
            InitialWindowSpawnPlacement::App if self.parent_node.is_some() => {
                InitialWindowSpawnPlacement::Center
            }
            InitialWindowSpawnPlacement::App => InitialWindowSpawnPlacement::Adjacent,
            placement => placement,
        }
    }
}

pub(crate) fn resolve_initial_window_intent(
    st: &Halley,
    toplevel: &ToplevelSurface,
) -> InitialWindowIntent {
    resolve_initial_window_intent_for_surface(st, toplevel.wl_surface())
}

pub(crate) fn resolve_initial_window_intent_for_surface(
    st: &Halley,
    surface: &WlSurface,
) -> InitialWindowIntent {
    let root = surface_tree_root(surface);
    resolve_initial_window_intent_from_identity(
        st,
        surface_app_id(&root),
        surface_title(&root),
        parent_node_for_surface(st, &root),
    )
}

pub(crate) fn resolve_initial_window_intent_from_identity(
    st: &Halley,
    app_id: Option<String>,
    title: Option<String>,
    parent_node: Option<NodeId>,
) -> InitialWindowIntent {
    let matched_rule = matching_window_rule(st, app_id.as_deref(), title.as_deref());
    let rule = matched_rule.map(rule_match).unwrap_or_default();
    InitialWindowIntent {
        app_id,
        title,
        parent_node,
        rule,
        matched_rule: matched_rule.is_some(),
        is_transient: parent_node.is_some(),
        prefer_app_intent: matches!(rule.spawn_placement, InitialWindowSpawnPlacement::App),
    }
}

pub(crate) fn needs_deferred_rule_recheck(st: &Halley, intent: &InitialWindowIntent) -> bool {
    if intent.matched_rule {
        return false;
    }
    let missing_title = intent.title.is_none()
        && st
            .runtime
            .tuning
            .window_rules
            .iter()
            .any(|rule| !rule.titles.is_empty());
    let missing_app_id = intent.app_id.is_none()
        && st
            .runtime
            .tuning
            .window_rules
            .iter()
            .any(|rule| !rule.app_ids.is_empty());
    missing_title || missing_app_id
}

fn rule_match(rule: &WindowRule) -> ResolvedInitialWindowRule {
    ResolvedInitialWindowRule {
        overlap_policy: rule.overlap_policy,
        spawn_placement: rule.spawn_placement,
        cluster_participation: rule.cluster_participation,
    }
}

fn matching_window_rule<'a>(
    st: &'a Halley,
    app_id: Option<&str>,
    title: Option<&str>,
) -> Option<&'a WindowRule> {
    st.runtime.tuning.window_rules.iter().find(|rule| {
        let app_matches = if rule.app_ids.is_empty() {
            true
        } else {
            app_id.is_some_and(|app_id| {
                rule.app_ids
                    .iter()
                    .any(|candidate| candidate.matches(app_id))
            })
        };
        let title_matches = if rule.titles.is_empty() {
            true
        } else {
            title.is_some_and(|title| rule.titles.iter().any(|candidate| candidate.matches(title)))
        };
        app_matches && title_matches
    })
}

fn parent_node_for_surface(st: &Halley, surface: &WlSurface) -> Option<NodeId> {
    let parent_surface = with_states(surface, |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| {
                data.lock()
                    .expect("xdg toplevel surface data")
                    .parent
                    .clone()
            })
    })?;
    st.model.surface_to_node.get(&parent_surface.id()).copied()
}

fn surface_tree_root(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
        root = parent;
    }
    root
}

fn surface_app_id(surface: &WlSurface) -> Option<String> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| {
                data.lock()
                    .expect("xdg toplevel surface data")
                    .app_id
                    .clone()
                    .filter(|value| !value.trim().is_empty())
            })
    })
}

fn surface_title(surface: &WlSurface) -> Option<String> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| {
                data.lock()
                    .expect("xdg toplevel surface data")
                    .title
                    .clone()
                    .filter(|value| !value.trim().is_empty())
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_config::{RuntimeTuning, WindowRulePattern};
    use smithay::reexports::wayland_server::Display;

    #[test]
    fn first_matching_rule_wins() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = RuntimeTuning::default();
        tuning.window_rules = vec![
            WindowRule {
                app_ids: vec![WindowRulePattern::Exact("firefox".to_string())],
                titles: Vec::new(),
                overlap_policy: InitialWindowOverlapPolicy::All,
                spawn_placement: InitialWindowSpawnPlacement::Center,
                cluster_participation: InitialWindowClusterParticipation::Float,
            },
            WindowRule {
                app_ids: vec![WindowRulePattern::Exact("firefox".to_string())],
                titles: Vec::new(),
                overlap_policy: InitialWindowOverlapPolicy::None,
                spawn_placement: InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: InitialWindowClusterParticipation::Layout,
            },
        ];
        let state = Halley::new_for_test(&dh, tuning);

        let matched = matching_window_rule(&state, Some("firefox"), None).expect("match");
        assert_eq!(matched.overlap_policy, InitialWindowOverlapPolicy::All);
        assert_eq!(matched.spawn_placement, InitialWindowSpawnPlacement::Center);
    }

    #[test]
    fn title_match_works_without_app_id() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = RuntimeTuning::default();
        tuning.window_rules = vec![WindowRule {
            app_ids: Vec::new(),
            titles: vec![WindowRulePattern::Exact("Picture-in-Picture".to_string())],
            overlap_policy: InitialWindowOverlapPolicy::All,
            spawn_placement: InitialWindowSpawnPlacement::Center,
            cluster_participation: InitialWindowClusterParticipation::Float,
        }];
        let state = Halley::new_for_test(&dh, tuning);

        let matched =
            matching_window_rule(&state, None, Some("Picture-in-Picture")).expect("match");
        assert_eq!(matched.spawn_placement, InitialWindowSpawnPlacement::Center);
    }

    #[test]
    fn app_id_and_title_match_as_and_condition() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = RuntimeTuning::default();
        tuning.window_rules = vec![WindowRule {
            app_ids: vec![WindowRulePattern::Exact("firefox".to_string())],
            titles: vec![WindowRulePattern::Exact("Picture-in-Picture".to_string())],
            overlap_policy: InitialWindowOverlapPolicy::All,
            spawn_placement: InitialWindowSpawnPlacement::Center,
            cluster_participation: InitialWindowClusterParticipation::Float,
        }];
        let state = Halley::new_for_test(&dh, tuning);

        assert!(
            matching_window_rule(&state, Some("firefox"), Some("Picture-in-Picture")).is_some()
        );
        assert!(matching_window_rule(&state, Some("firefox"), Some("Other")).is_none());
        assert!(matching_window_rule(&state, None, Some("Picture-in-Picture")).is_none());
    }

    #[test]
    fn regex_title_match_works() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = RuntimeTuning::default();
        tuning.window_rules = vec![WindowRule {
            app_ids: Vec::new(),
            titles: vec![WindowRulePattern::Regex(
                regex::Regex::new("File Upload.*").expect("regex"),
            )],
            overlap_policy: InitialWindowOverlapPolicy::All,
            spawn_placement: InitialWindowSpawnPlacement::Center,
            cluster_participation: InitialWindowClusterParticipation::Float,
        }];
        let state = Halley::new_for_test(&dh, tuning);

        assert!(matching_window_rule(&state, None, Some("File Upload - Firefox")).is_some());
    }
}
