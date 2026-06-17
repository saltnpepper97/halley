#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LiftMode {
    #[default]
    General,
    Apps,
    Clusters,
    Nodes,
    Actions,
    Config,
    Term,
}

fn prefix_mode_from_token(token: &str) -> Option<LiftMode> {
    match token.trim().to_ascii_lowercase().as_str() {
        "app" | "apps" | "/app" | "/apps" | "/a" => Some(LiftMode::Apps),
        "cluster" | "clusters" | "/cluster" | "/clusters" | "/c" => Some(LiftMode::Clusters),
        "node" | "nodes" | "/node" | "/nodes" | "/n" => Some(LiftMode::Nodes),
        "action" | "actions" | "/action" | "/actions" => Some(LiftMode::Actions),
        "config" | "/config" => Some(LiftMode::Config),
        "term" | "/term" | "/t" => Some(LiftMode::Term),
        _ => None,
    }
}

pub fn parse_initial_mode(raw: &str) -> (LiftMode, String) {
    (LiftMode::General, raw.trim().to_string())
}

pub fn effective_mode_query(mode: LiftMode, query: &str) -> (LiftMode, String) {
    if mode != LiftMode::General {
        return (mode, query.trim().to_string());
    }
    let trimmed = query.trim_start();
    let Some((token, rest)) = trimmed.split_once(char::is_whitespace) else {
        return match prefix_mode_from_token(trimmed) {
            Some(mode) => (mode, String::new()),
            None => (LiftMode::General, query.trim().to_string()),
        };
    };
    match prefix_mode_from_token(token) {
        Some(mode) => (mode, rest.trim_start().to_string()),
        None => (LiftMode::General, query.trim().to_string()),
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModeInputState {
    pub mode: LiftMode,
    pub query: String,
}

impl ModeInputState {
    pub fn remove_badge(&mut self) {
        self.mode = LiftMode::General;
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
            ("release", LiftMode::General, "release"),
            ("/cluster release", LiftMode::General, "/cluster release"),
            ("/clusters release", LiftMode::General, "/clusters release"),
            ("/c release", LiftMode::General, "/c release"),
            ("/node systemd", LiftMode::General, "/node systemd"),
            ("/n systemd", LiftMode::General, "/n systemd"),
            ("/app firefox", LiftMode::General, "/app firefox"),
            ("/app", LiftMode::General, "/app"),
            ("/a firefox", LiftMode::General, "/a firefox"),
            ("/a", LiftMode::General, "/a"),
            ("/c", LiftMode::General, "/c"),
            ("/n", LiftMode::General, "/n"),
            ("/config lift", LiftMode::General, "/config lift"),
        ];
        for (raw, mode, query) in cases {
            assert_eq!(parse_initial_mode(raw), (mode, query.to_string()));
        }
    }

    #[test]
    fn removing_badge_returns_to_general() {
        let mut state = ModeInputState {
            mode: LiftMode::Clusters,
            query: "release".into(),
        };
        state.remove_badge();
        assert_eq!(state.mode, LiftMode::General);
        assert_eq!(state.query, "release");
    }

    #[test]
    fn backspace_at_empty_query_removes_badge() {
        let mut state = ModeInputState {
            mode: LiftMode::Nodes,
            query: String::new(),
        };
        state.backspace();
        assert_eq!(state.mode, LiftMode::General);
    }

    #[test]
    fn entering_new_slash_mode_replaces_existing_mode() {
        let mut state = ModeInputState {
            mode: LiftMode::Nodes,
            query: String::new(),
        };
        state.insert_text("/app firefox");
        assert_eq!(state.mode, LiftMode::Nodes);
        assert_eq!(state.query, "/app firefox");
    }

    #[test]
    fn effective_query_detects_mode_prefixes() {
        let mut state = ModeInputState::default();
        state.insert_text("action open");
        assert_eq!(state.mode, LiftMode::General);
        assert_eq!(
            effective_mode_query(state.mode, state.query.as_str()),
            (LiftMode::Actions, "open".into())
        );
    }

    #[test]
    fn effective_query_detects_term_prefix() {
        assert_eq!(
            effective_mode_query(LiftMode::General, "term echo hi"),
            (LiftMode::Term, "echo hi".into())
        );
        assert_eq!(
            effective_mode_query(LiftMode::General, "/t ls -la"),
            (LiftMode::Term, "ls -la".into())
        );
        assert_eq!(
            effective_mode_query(LiftMode::General, "term"),
            (LiftMode::Term, String::new())
        );
    }

    #[test]
    fn effective_query_detects_cluster_prefixes_with_empty_filter() {
        assert_eq!(
            effective_mode_query(LiftMode::General, "cluster"),
            (LiftMode::Clusters, String::new())
        );
        assert_eq!(
            effective_mode_query(LiftMode::General, "clusters firefox"),
            (LiftMode::Clusters, "firefox".into())
        );
    }

    #[test]
    fn unknown_slash_command_stays_general_text() {
        assert_eq!(
            parse_initial_mode("/wat test"),
            (LiftMode::General, "/wat test".into())
        );
    }
}
