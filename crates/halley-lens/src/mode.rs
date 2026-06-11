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

impl LensMode {
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::General => None,
            Self::Apps => Some("Apps"),
            Self::Clusters => Some("Clusters"),
            Self::Nodes => Some("Nodes"),
            Self::Actions => Some("Actions"),
            Self::Config => Some("Config"),
        }
    }
}

pub fn mode_from_token(token: &str) -> Option<LensMode> {
    match token.trim().to_ascii_lowercase().as_str() {
        "/app" | "/apps" | "/a" => Some(LensMode::Apps),
        "/cluster" | "/clusters" | "/c" => Some(LensMode::Clusters),
        "/node" | "/nodes" | "/n" => Some(LensMode::Nodes),
        "/action" | "/actions" => Some(LensMode::Actions),
        "/config" => Some(LensMode::Config),
        _ => None,
    }
}

pub fn parse_initial_mode(raw: &str) -> (LensMode, String) {
    let trimmed = raw.trim_start();
    let Some((token, rest)) = trimmed.split_once(char::is_whitespace) else {
        return (LensMode::General, raw.trim().to_string());
    };
    match mode_from_token(token) {
        Some(mode) => (mode, rest.trim_start().to_string()),
        None => (LensMode::General, raw.trim().to_string()),
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
        if self.query.starts_with('/')
            && let Some((token, rest)) = self.query.split_once(char::is_whitespace)
            && let Some(mode) = mode_from_token(token)
        {
            self.mode = mode;
            self.query = rest.trim_start().to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_required_modes() {
        let cases = [
            ("release", LensMode::General, "release"),
            ("/cluster release", LensMode::Clusters, "release"),
            ("/clusters release", LensMode::Clusters, "release"),
            ("/c release", LensMode::Clusters, "release"),
            ("/node systemd", LensMode::Nodes, "systemd"),
            ("/n systemd", LensMode::Nodes, "systemd"),
            ("/app firefox", LensMode::Apps, "firefox"),
            ("/a firefox", LensMode::Apps, "firefox"),
            ("/config lens", LensMode::Config, "lens"),
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
        assert_eq!(state.mode, LensMode::Apps);
        assert_eq!(state.query, "firefox");
    }

    #[test]
    fn unknown_slash_command_stays_general_text() {
        assert_eq!(
            parse_initial_mode("/wat test"),
            (LensMode::General, "/wat test".into())
        );
    }
}
