#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LensMode {
    #[default]
    General,
    Apps,
    Clusters,
    Nodes,
    Actions,
    Config,
}

fn prefix_mode_from_token(token: &str) -> Option<LensMode> {
    match token.trim().to_ascii_lowercase().as_str() {
        "app" | "apps" | "/app" | "/apps" | "/a" => Some(LensMode::Apps),
        "cluster" | "clusters" | "/cluster" | "/clusters" | "/c" => Some(LensMode::Clusters),
        "node" | "nodes" | "/node" | "/nodes" | "/n" => Some(LensMode::Nodes),
        "action" | "actions" | "/action" | "/actions" => Some(LensMode::Actions),
        "config" | "/config" => Some(LensMode::Config),
        _ => None,
    }
}

pub fn parse_initial_mode(raw: &str) -> (LensMode, String) {
    (LensMode::General, raw.trim().to_string())
}

pub fn effective_mode_query(mode: LensMode, query: &str) -> (LensMode, String) {
    if mode != LensMode::General {
        return (mode, query.trim().to_string());
    }
    let trimmed = query.trim_start();
    let Some((token, rest)) = trimmed.split_once(char::is_whitespace) else {
        return match prefix_mode_from_token(trimmed) {
            Some(mode) => (mode, String::new()),
            None => (LensMode::General, query.trim().to_string()),
        };
    };
    match prefix_mode_from_token(token) {
        Some(mode) => (mode, rest.trim_start().to_string()),
        None => (LensMode::General, query.trim().to_string()),
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModeInputState {
    pub mode: LensMode,
    pub query: String,
}

impl ModeInputState {
    pub fn remove_badge(&mut self) {
        self.mode = LensMode::General;
    }

    pub fn backspace(&mut self) {
        if self.query.is_empty() {
            self.remove_badge();
        } else {
            self.query.pop();
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        self.query.push_str(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_required_modes() {
        let cases = [
            ("release", LensMode::General, "release"),
            ("/cluster release", LensMode::General, "/cluster release"),
            ("/clusters release", LensMode::General, "/clusters release"),
            ("/c release", LensMode::General, "/c release"),
            ("/node systemd", LensMode::General, "/node systemd"),
            ("/n systemd", LensMode::General, "/n systemd"),
            ("/app firefox", LensMode::General, "/app firefox"),
            ("/app", LensMode::General, "/app"),
            ("/a firefox", LensMode::General, "/a firefox"),
            ("/a", LensMode::General, "/a"),
            ("/c", LensMode::General, "/c"),
            ("/n", LensMode::General, "/n"),
            ("/config lens", LensMode::General, "/config lens"),
        ];
        for (raw, mode, query) in cases {
            assert_eq!(parse_initial_mode(raw), (mode, query.to_string()));
        }
    }

    #[test]
    fn removing_badge_returns_to_general() {
        let mut state = ModeInputState {
            mode: LensMode::Clusters,
            query: "release".into(),
        };
        state.remove_badge();
        assert_eq!(state.mode, LensMode::General);
        assert_eq!(state.query, "release");
    }

    #[test]
    fn backspace_at_empty_query_removes_badge() {
        let mut state = ModeInputState {
            mode: LensMode::Nodes,
            query: String::new(),
        };
        state.backspace();
        assert_eq!(state.mode, LensMode::General);
    }

    #[test]
    fn entering_new_slash_mode_replaces_existing_mode() {
        let mut state = ModeInputState {
            mode: LensMode::Nodes,
            query: String::new(),
        };
        state.insert_text("/app firefox");
        assert_eq!(state.mode, LensMode::Nodes);
        assert_eq!(state.query, "/app firefox");
    }

    #[test]
    fn effective_query_detects_mode_prefixes() {
        let mut state = ModeInputState::default();
        state.insert_text("action open");
        assert_eq!(state.mode, LensMode::General);
        assert_eq!(
            effective_mode_query(state.mode, state.query.as_str()),
            (LensMode::Actions, "open".into())
        );
    }

    #[test]
    fn effective_query_detects_cluster_prefixes_with_empty_filter() {
        assert_eq!(
            effective_mode_query(LensMode::General, "cluster"),
            (LensMode::Clusters, String::new())
        );
        assert_eq!(
            effective_mode_query(LensMode::General, "clusters firefox"),
            (LensMode::Clusters, "firefox".into())
        );
    }

    #[test]
    fn unknown_slash_command_stays_general_text() {
        assert_eq!(
            parse_initial_mode("/wat test"),
            (LensMode::General, "/wat test".into())
        );
    }
}
