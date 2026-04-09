use halley_config::{
    InitialWindowClusterParticipation, InitialWindowOverlapPolicy, InitialWindowSpawnPlacement,
    WindowRule,
};
use halley_core::field::NodeId;
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
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
        let effective_spawn_placement = self.effective_spawn_placement();
        let effective_overlap_policy = self.effective_overlap_policy();
        crate::compositor::spawn::state::AppliedInitialWindowRule {
            overlap_policy: effective_overlap_policy,
            spawn_placement: self.rule.spawn_placement,
            cluster_participation: self.rule.cluster_participation,
            parent_node: self.parent_node,
            suppress_reveal_pan: !matches!(
                effective_spawn_placement,
                InitialWindowSpawnPlacement::Adjacent
            ) || effective_overlap_policy != InitialWindowOverlapPolicy::None
                || self.rule.cluster_participation == InitialWindowClusterParticipation::Float,
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
    let rule = matching_window_rule(st, app_id.as_deref(), title.as_deref());
    InitialWindowIntent {
        app_id,
        title,
        parent_node,
        rule: rule.unwrap_or_default(),
        matched_rule: rule.is_some(),
        is_transient: parent_node.is_some(),
        prefer_app_intent: rule
            .is_some_and(|rule| matches!(rule.spawn_placement, InitialWindowSpawnPlacement::App)),
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
    missing_title
        || missing_app_id
        || builtin_window_rule_may_match_later(intent.app_id.as_deref(), intent.title.as_deref())
}

fn rule_match(rule: &WindowRule) -> ResolvedInitialWindowRule {
    ResolvedInitialWindowRule {
        overlap_policy: rule.overlap_policy,
        spawn_placement: rule.spawn_placement,
        cluster_participation: rule.cluster_participation,
    }
}

fn builtin_float_center_overlap_rule() -> ResolvedInitialWindowRule {
    ResolvedInitialWindowRule {
        overlap_policy: InitialWindowOverlapPolicy::All,
        spawn_placement: InitialWindowSpawnPlacement::Center,
        cluster_participation: InitialWindowClusterParticipation::Float,
    }
}

fn portal_dialog_title_matches(title: &str) -> bool {
    ["File Upload", "Open File", "Save File", "Choose"]
        .into_iter()
        .any(|prefix| title.starts_with(prefix))
}

fn matching_user_window_rule<'a>(
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
                    .any(|candidate: &halley_config::WindowRulePattern| candidate.matches(app_id))
            })
        };
        let title_matches = if rule.titles.is_empty() {
            true
        } else {
            title.is_some_and(|title| {
                rule.titles
                    .iter()
                    .any(|candidate: &halley_config::WindowRulePattern| candidate.matches(title))
            })
        };
        app_matches && title_matches
    })
}

fn matching_builtin_window_rule(
    app_id: Option<&str>,
    title: Option<&str>,
) -> Option<ResolvedInitialWindowRule> {
    if app_id == Some("xdg-desktop-portal-gtk") && title.is_some_and(portal_dialog_title_matches) {
        return Some(builtin_float_center_overlap_rule());
    }

    if title == Some("Picture-in-Picture") {
        return Some(builtin_float_center_overlap_rule());
    }

    None
}

fn matching_window_rule(
    st: &Halley,
    app_id: Option<&str>,
    title: Option<&str>,
) -> Option<ResolvedInitialWindowRule> {
    matching_user_window_rule(st, app_id, title)
        .map(rule_match)
        .or_else(|| matching_builtin_window_rule(app_id, title))
}

fn builtin_window_rule_may_match_later(app_id: Option<&str>, title: Option<&str>) -> bool {
    title.is_none() || (app_id.is_none() && title.is_some_and(portal_dialog_title_matches))
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
    fn user_rule_overrides_builtin_pip() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = RuntimeTuning::default();
        tuning.window_rules = vec![WindowRule {
            app_ids: Vec::new(),
            titles: vec![WindowRulePattern::Exact("Picture-in-Picture".to_string())],
            overlap_policy: InitialWindowOverlapPolicy::None,
            spawn_placement: InitialWindowSpawnPlacement::Adjacent,
            cluster_participation: InitialWindowClusterParticipation::Layout,
        }];
        let state = Halley::new_for_test(&dh, tuning);

        let matched =
            matching_window_rule(&state, None, Some("Picture-in-Picture")).expect("match");
        assert_eq!(matched.overlap_policy, InitialWindowOverlapPolicy::None);
        assert_eq!(
            matched.spawn_placement,
            InitialWindowSpawnPlacement::Adjacent
        );
        assert_eq!(
            matched.cluster_participation,
            InitialWindowClusterParticipation::Layout
        );
    }

    #[test]
    fn builtin_portal_dialog_matches() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let state = Halley::new_for_test(&dh, RuntimeTuning::default());

        let matched =
            matching_window_rule(&state, Some("xdg-desktop-portal-gtk"), Some("Open File"))
                .expect("match");
        assert_eq!(matched.overlap_policy, InitialWindowOverlapPolicy::All);
        assert_eq!(matched.spawn_placement, InitialWindowSpawnPlacement::Center);
        assert_eq!(
            matched.cluster_participation,
            InitialWindowClusterParticipation::Float
        );
    }

    #[test]
    fn builtin_pip_matches() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let state = Halley::new_for_test(&dh, RuntimeTuning::default());

        let matched =
            matching_window_rule(&state, None, Some("Picture-in-Picture")).expect("match");
        assert_eq!(matched.overlap_policy, InitialWindowOverlapPolicy::All);
        assert_eq!(matched.spawn_placement, InitialWindowSpawnPlacement::Center);
        assert_eq!(
            matched.cluster_participation,
            InitialWindowClusterParticipation::Float
        );
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
            matching_user_window_rule(&state, Some("firefox"), Some("Picture-in-Picture"))
                .is_some()
        );
        assert!(matching_user_window_rule(&state, Some("firefox"), Some("Other")).is_none());
        assert!(matching_user_window_rule(&state, None, Some("Picture-in-Picture")).is_none());
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

    #[test]
    fn deferred_recheck_considers_builtin_title_rules() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let state = Halley::new_for_test(&dh, RuntimeTuning::default());
        let intent = InitialWindowIntent {
            app_id: Some("xdg-desktop-portal-gtk".to_string()),
            title: None,
            parent_node: None,
            rule: ResolvedInitialWindowRule::default(),
            matched_rule: false,
            is_transient: false,
            prefer_app_intent: false,
        };

        assert!(needs_deferred_rule_recheck(&state, &intent));
    }

    #[test]
    fn float_and_overlap_rules_suppress_reveal_pan() {
        let adjacent = InitialWindowIntent {
            app_id: Some("firefox".to_string()),
            title: None,
            parent_node: None,
            rule: ResolvedInitialWindowRule {
                overlap_policy: InitialWindowOverlapPolicy::All,
                spawn_placement: InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: InitialWindowClusterParticipation::Float,
            },
            matched_rule: true,
            is_transient: false,
            prefer_app_intent: false,
        };
        let center = InitialWindowIntent {
            rule: ResolvedInitialWindowRule {
                spawn_placement: InitialWindowSpawnPlacement::Center,
                ..adjacent.rule
            },
            ..adjacent.clone()
        };
        let layout_adjacent = InitialWindowIntent {
            rule: ResolvedInitialWindowRule {
                overlap_policy: InitialWindowOverlapPolicy::None,
                spawn_placement: InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: InitialWindowClusterParticipation::Layout,
            },
            ..adjacent.clone()
        };

        assert!(adjacent.applied_rule_for_node().suppress_reveal_pan);
        assert!(center.applied_rule_for_node().suppress_reveal_pan);
        assert!(!layout_adjacent.applied_rule_for_node().suppress_reveal_pan);
    }
}
