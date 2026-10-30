use std::collections::HashMap;

use halley_config::{WindowClusterParticipation, WindowRule, WindowSpawnPlacement};
use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedWindowRule {
    pub open_on_output: Option<String>,
    pub open_focused: Option<bool>,
    pub open_position: Option<(i32, i32)>,
    pub open_fullscreen: bool,
    pub open_maximized: bool,
    pub initial_size: (Option<u32>, Option<u32>),
    pub opacity: f32,
    pub shadow: Option<bool>,
    pub blur: Option<bool>,
    pub block_screencast: bool,
    pub game: Option<bool>,
    pub spawn_placement: WindowSpawnPlacement,
    pub cluster_participation: WindowClusterParticipation,
    pub matched: bool,
}

impl Default for ResolvedWindowRule {
    fn default() -> Self {
        Self {
            open_on_output: None,
            open_focused: None,
            open_position: None,
            open_fullscreen: false,
            open_maximized: false,
            initial_size: (None, None),
            opacity: 1.0,
            shadow: None,
            blur: None,
            block_screencast: false,
            game: None,
            spawn_placement: WindowSpawnPlacement::Default,
            cluster_participation: WindowClusterParticipation::Layout,
            matched: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowIdentity {
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub is_transient: bool,
}

#[derive(Debug)]
pub struct WindowRulesState {
    rules: Vec<WindowRule>,
    start_time: std::time::Instant,
    applied: HashMap<WlSurface, ResolvedWindowRule>,
    pub(crate) games: super::games::GameApplications,
}

impl Default for WindowRulesState {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl WindowRulesState {
    pub fn new(rules: Vec<WindowRule>) -> Self {
        Self {
            rules,
            start_time: std::time::Instant::now(),
            applied: HashMap::new(),
            games: super::games::GameApplications::load(),
        }
    }

    pub fn has_screencast_blocks(&self) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.block_screencast == Some(true))
    }

    pub fn reload(&mut self, rules: Vec<WindowRule>) {
        self.rules = rules;
        self.games = super::games::GameApplications::load();
    }

    pub fn resolve_identity(&self, identity: &WindowIdentity) -> ResolvedWindowRule {
        let mut resolved = ResolvedWindowRule::default();
        for rule in &self.rules {
            if !rule.matches_with_context(
                identity.app_id.as_deref(),
                identity.title.as_deref(),
                identity.is_transient,
                self.start_time.elapsed() < std::time::Duration::from_secs(60),
            ) {
                continue;
            }
            resolved.matched = true;
            if let Some(v) = rule.open_fullscreen {
                resolved.open_fullscreen = v;
            }
            if let Some(v) = rule.open_maximized {
                resolved.open_maximized = v;
            }
            if let Some(v) = rule.open_position {
                resolved.open_position = Some(v);
            }
            if let Some(v) = rule.open_focused {
                resolved.open_focused = Some(v);
            }
            if let Some(v) = &rule.open_on_output {
                resolved.open_on_output = Some(v.clone());
            }
            if rule.initial_size.0.is_some() {
                resolved.initial_size.0 = rule.initial_size.0;
            }
            if rule.initial_size.1.is_some() {
                resolved.initial_size.1 = rule.initial_size.1;
            }
            if let Some(v) = rule.shadow {
                resolved.shadow = Some(v);
            }
            if let Some(v) = rule.opacity {
                resolved.opacity = v.clamp(0.0, 1.0);
            }
            if let Some(v) = rule.blur {
                resolved.blur = Some(v);
            }
            if let Some(v) = rule.block_screencast {
                resolved.block_screencast = v;
            }
            if let Some(v) = rule.game {
                resolved.game = Some(v);
            }
            if let Some(v) = rule.spawn_placement {
                resolved.spawn_placement = v;
            }
            if let Some(v) = rule.cluster_participation {
                resolved.cluster_participation = v;
            }
            if !rule.compose {
                break;
            }
        }
        resolved
    }

    pub fn track_window(&mut self, window: &Window) -> ResolvedWindowRule {
        let resolved = self.resolve_identity(&identity(window));
        if let Some(surface) = window.wl_surface().map(|surface| surface.into_owned()) {
            self.applied.insert(surface, resolved.clone());
        }
        resolved
    }

    pub fn applied(&self, surface: &WlSurface) -> ResolvedWindowRule {
        self.applied.get(surface).cloned().unwrap_or_default()
    }

    pub fn opacity(&self, surface: &WlSurface) -> f32 {
        self.applied(surface).opacity
    }

    pub fn blur(&self, surface: &WlSurface) -> Option<bool> {
        self.applied(surface).blur
    }

    pub fn forget(&mut self, surface: &WlSurface) {
        self.applied.remove(surface);
    }
}

pub fn identity(window: &Window) -> WindowIdentity {
    if let Some(toplevel) = window.toplevel() {
        let is_transient = toplevel.parent().is_some();
        return with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok())
                .map(|data| WindowIdentity {
                    app_id: nonempty(data.app_id.clone()),
                    title: nonempty(data.title.clone()),
                    is_transient,
                })
                .unwrap_or_default()
        });
    }
    if let Some((title, class)) = crate::xwayland::metadata(window) {
        return WindowIdentity {
            app_id: nonempty(Some(class)),
            title: nonempty(Some(title)),
            is_transient: crate::xwayland::is_transient(window),
        };
    }
    WindowIdentity::default()
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_config::WindowRulePattern;

    #[test]
    fn first_matching_rule_wins_and_defaults_are_inert() {
        let state = WindowRulesState::new(vec![
            WindowRule {
                open_on_output: None,
                open_focused: None,
                open_position: None,
                open_fullscreen: None,
                open_maximized: None,
                compose: false,
                match_all: false,
                matches_any: vec![],
                excludes: vec![],
                app_ids: vec![WindowRulePattern::Exact("app".to_string())],
                titles: Vec::new(),
                initial_size: (Some(800), Some(600)),
                shadow: None,
                opacity: Some(0.7),
                blur: Some(true),
                block_screencast: Some(true),
                game: Some(true),
                spawn_placement: Some(WindowSpawnPlacement::Center),
                cluster_participation: Some(WindowClusterParticipation::Float),
            },
            WindowRule {
                open_on_output: None,
                open_focused: None,
                open_position: None,
                open_fullscreen: None,
                open_maximized: None,
                compose: false,
                match_all: false,
                matches_any: vec![],
                excludes: vec![],
                app_ids: vec![WindowRulePattern::Exact("app".to_string())],
                titles: Vec::new(),
                initial_size: (None, None),
                shadow: None,
                opacity: Some(0.2),
                blur: Some(false),
                block_screencast: Some(false),
                game: Some(false),
                spawn_placement: Some(WindowSpawnPlacement::Cursor),
                cluster_participation: Some(WindowClusterParticipation::Layout),
            },
        ]);
        let resolved = state.resolve_identity(&WindowIdentity {
            app_id: Some("app".to_string()),
            title: None,
            is_transient: false,
        });
        assert_eq!(resolved.opacity, 0.7);
        assert!(resolved.block_screencast);
        assert_eq!(resolved.game, Some(true));
        assert_eq!(resolved.initial_size, (Some(800), Some(600)));
        assert_eq!(
            resolved.cluster_participation,
            WindowClusterParticipation::Float
        );
        assert_eq!(
            state.resolve_identity(&WindowIdentity::default()),
            ResolvedWindowRule::default()
        );
    }
    #[test]
    fn composed_rules_override_only_explicit_properties_and_exclude() {
        let config = rune_cfg::RuneConfig::from_str(
            r#"
rules:
  window-rule-mode "compose"
  window-rule:
    match-all true
    opacity 0.9
    blur true
    shadow true
    block-screencast true
  end
  window-rule:
    match:
      app-id "foot"
      title "Monitoring"
    end
    match:
      app-id "other"
    end
    exclude:
      title "Secret"
    end
    blur false
    shadow false
    block-screencast false
    open-maximized true
  end
end
"#,
        )
        .unwrap();
        let state = WindowRulesState::new(halley_config::parse_window_rules(&config).unwrap());
        let monitoring = state.resolve_identity(&WindowIdentity {
            app_id: Some("foot".into()),
            title: Some("Monitoring".into()),
            is_transient: false,
        });
        assert_eq!(monitoring.opacity, 0.9);
        assert_eq!(monitoring.blur, Some(false));
        assert_eq!(monitoring.shadow, Some(false));
        assert!(monitoring.open_maximized);
        assert!(!monitoring.block_screencast);
        let secret = state.resolve_identity(&WindowIdentity {
            app_id: Some("other".into()),
            title: Some("Secret".into()),
            is_transient: false,
        });
        assert_eq!(secret.blur, Some(true));
        assert_eq!(secret.shadow, Some(true));
        assert!(secret.block_screencast);
    }
    #[test]
    fn transient_matcher_distinguishes_dialogs_from_main_windows() {
        let config = rune_cfg::RuneConfig::from_str(
            r#"
rules:
  window-rule:
    match:
      app-id "app"
      is-transient true
    end
    opacity 0.5
  end
end
"#,
        )
        .unwrap();
        let state = WindowRulesState::new(halley_config::parse_window_rules(&config).unwrap());
        let mut identity = WindowIdentity {
            app_id: Some("app".into()),
            title: None,
            is_transient: false,
        };
        assert_eq!(state.resolve_identity(&identity).opacity, 1.0);
        identity.is_transient = true;
        assert_eq!(state.resolve_identity(&identity).opacity, 0.5);
    }
    #[test]
    fn startup_match_uses_session_age_and_reload_does_not_reset_it() {
        let config = rune_cfg::RuneConfig::from_str(
            r#"
rules:
  window-rule:
    match:
      app-id "app"
      at-startup true
    end
    width 1000
  end
end
"#,
        )
        .unwrap();
        let rules = halley_config::parse_window_rules(&config).unwrap();
        let mut state = WindowRulesState::new(rules.clone());
        let identity = WindowIdentity {
            app_id: Some("app".into()),
            ..Default::default()
        };
        assert_eq!(state.resolve_identity(&identity).initial_size.0, Some(1000));
        state.start_time = std::time::Instant::now() - std::time::Duration::from_secs(61);
        state.reload(rules);
        assert_eq!(state.resolve_identity(&identity).initial_size.0, None);
    }
}
