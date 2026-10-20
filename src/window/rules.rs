use std::collections::HashMap;

use halley_config::{WindowClusterParticipation, WindowRule, WindowSpawnPlacement};
use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedWindowRule {
    pub initial_size: Option<(u32, u32)>,
    pub opacity: f32,
    pub blur: Option<bool>,
    pub game: Option<bool>,
    pub spawn_placement: WindowSpawnPlacement,
    pub cluster_participation: WindowClusterParticipation,
    pub matched: bool,
}

impl Default for ResolvedWindowRule {
    fn default() -> Self {
        Self {
            initial_size: None,
            opacity: 1.0,
            blur: None,
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
}

#[derive(Debug, Default)]
pub struct WindowRulesState {
    rules: Vec<WindowRule>,
    applied: HashMap<WlSurface, ResolvedWindowRule>,
    pub(crate) games: super::games::GameApplications,
}

impl WindowRulesState {
    pub fn new(rules: Vec<WindowRule>) -> Self {
        Self {
            rules,
            applied: HashMap::new(),
            games: super::games::GameApplications::load(),
        }
    }

    pub fn reload(&mut self, rules: Vec<WindowRule>) {
        self.rules = rules;
        self.games = super::games::GameApplications::load();
    }

    pub fn resolve_identity(&self, identity: &WindowIdentity) -> ResolvedWindowRule {
        self.rules
            .iter()
            .find(|rule| rule.matches(identity.app_id.as_deref(), identity.title.as_deref()))
            .map(|rule| ResolvedWindowRule {
                initial_size: rule.initial_size,
                opacity: rule.opacity.unwrap_or(1.0).clamp(0.0, 1.0),
                blur: rule.blur,
                game: rule.game,
                spawn_placement: rule.spawn_placement,
                cluster_participation: rule.cluster_participation,
                matched: true,
            })
            .unwrap_or_default()
    }

    pub fn track_window(&mut self, window: &Window) -> ResolvedWindowRule {
        let resolved = self.resolve_identity(&identity(window));
        if let Some(surface) = window.wl_surface().map(|surface| surface.into_owned()) {
            self.applied.insert(surface, resolved);
        }
        resolved
    }

    pub fn applied(&self, surface: &WlSurface) -> ResolvedWindowRule {
        self.applied.get(surface).copied().unwrap_or_default()
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
        return with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok())
                .map(|data| WindowIdentity {
                    app_id: nonempty(data.app_id.clone()),
                    title: nonempty(data.title.clone()),
                })
                .unwrap_or_default()
        });
    }
    if let Some((title, class)) = crate::xwayland::metadata(window) {
        return WindowIdentity {
            app_id: nonempty(Some(class)),
            title: nonempty(Some(title)),
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
                app_ids: vec![WindowRulePattern::Exact("app".to_string())],
                titles: Vec::new(),
                initial_size: Some((800, 600)),
                opacity: Some(0.7),
                blur: Some(true),
                game: Some(true),
                spawn_placement: WindowSpawnPlacement::Center,
                cluster_participation: WindowClusterParticipation::Float,
            },
            WindowRule {
                app_ids: vec![WindowRulePattern::Exact("app".to_string())],
                titles: Vec::new(),
                initial_size: None,
                opacity: Some(0.2),
                blur: Some(false),
                game: Some(false),
                spawn_placement: WindowSpawnPlacement::Cursor,
                cluster_participation: WindowClusterParticipation::Layout,
            },
        ]);
        let resolved = state.resolve_identity(&WindowIdentity {
            app_id: Some("app".to_string()),
            title: None,
        });
        assert_eq!(resolved.opacity, 0.7);
        assert_eq!(resolved.game, Some(true));
        assert_eq!(resolved.initial_size, Some((800, 600)));
        assert_eq!(
            resolved.cluster_participation,
            WindowClusterParticipation::Float
        );
        assert_eq!(
            state.resolve_identity(&WindowIdentity::default()),
            ResolvedWindowRule::default()
        );
    }
}
