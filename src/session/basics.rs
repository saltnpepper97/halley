//! Durable user state for Halley's one-time onboarding facts.
//!
//! Halley keeps one small user-state file outside the configuration:
//! `$XDG_STATE_HOME/halley/state.rune`, falling back to
//! `~/.local/state/halley/state.rune`. It records whether a freshly generated
//! configuration still owes its first-run basics card, whether that card has
//! already been dismissed, and whether the one-time automatic-decay explanation
//! has already been shown. All of these are one-time onboarding facts, not
//! runtime policy, so they do not belong in `halley.rune` and are never
//! migrated or rewritten by configuration loading.
//!
//! Deleting the file is always safe: Halley then treats the card as neither
//! pending nor dismissed and the decay explanation as never shown, so the card
//! can only reappear if Halley generates another fresh configuration.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const STATE_DIR: &str = "halley";
const STATE_FILE: &str = "state.rune";
const PENDING_CONFIG_KEY: &str = "basics-card-pending-config";
const DISMISSED_KEY: &str = "basics-card-dismissed";
const DECAY_NOTICE_KEY: &str = "decay-notice-shown";

const HEADER: &str = "\
# Halley user state, written by the compositor.
# Separate from halley.rune: this file is not configuration and is never
# migrated. Deleting it is safe; it only forgets one-time onboarding state.
";

/// One-time onboarding state, loaded from the user state file.
///
/// `path` is `None` when no state directory can be resolved (no
/// `XDG_STATE_HOME` and no `HOME`), in which case the state stays in memory for
/// this session and nothing is persisted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct UserState {
    path: Option<PathBuf>,
    basics_card_pending_config: Option<String>,
    basics_card_dismissed: bool,
    decay_notice_shown: bool,
}

impl UserState {
    /// Loads the user state file, treating a missing or unreadable file as an
    /// empty state rather than an error.
    pub fn load() -> Self {
        match state_path() {
            Some(path) => Self::load_from(path),
            None => Self::default(),
        }
    }

    /// Loads state from an explicit path. Retains the path even when reading
    /// fails, so a later write can still create the file.
    fn load_from(path: PathBuf) -> Self {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Self {
                    path: Some(path),
                    ..Self::default()
                };
            }
            Err(error) => {
                eventline::warn!(
                    "state: failed to read {}: {error}; starting from empty state",
                    path.display()
                );
                return Self {
                    path: Some(path),
                    ..Self::default()
                };
            }
        };
        let mut state = Self {
            path: Some(path),
            ..Self::default()
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let value = value.trim();
            match key {
                PENDING_CONFIG_KEY => {
                    state.basics_card_pending_config = unquote(value).map(str::to_string);
                }
                DISMISSED_KEY => state.basics_card_dismissed = value.eq_ignore_ascii_case("true"),
                DECAY_NOTICE_KEY => state.decay_notice_shown = value.eq_ignore_ascii_case("true"),
                _ => {}
            }
        }
        state
    }

    /// Records that Halley just generated `config_path`, so its first native
    /// session can offer the one-time basics card.
    ///
    /// Called only while handling the configuration bootstrap; an existing
    /// configuration never reaches this path, so upgraded installations are
    /// left alone.
    pub fn record_fresh_config(&mut self, config_path: &Path) {
        let Some(value) = plain(config_path) else {
            return;
        };
        self.basics_card_pending_config = Some(value.to_string());
        self.persist();
    }

    /// Whether Halley still owes a first-run card to this exact configuration.
    pub fn basics_card_pending_for(&self, config_path: &Path) -> bool {
        plain(config_path)
            .is_some_and(|current| self.basics_card_pending_config.as_deref() == Some(current))
    }

    pub fn basics_card_dismissed(&self) -> bool {
        self.basics_card_dismissed
    }

    /// Whether the one-time automatic-decay explanation has already been shown
    /// to this installation.
    pub fn decay_notice_shown(&self) -> bool {
        self.decay_notice_shown
    }

    /// Persists the one-time fact that the first automatic collapse explained
    /// itself, so no later collapse — of this window or any other — explains
    /// anything again.
    pub fn record_decay_notice_shown(&mut self) {
        self.decay_notice_shown = true;
        self.persist();
    }

    /// Persists the one-time fact that the card was dismissed, so it never
    /// reappears automatically. Manual reopening is unaffected.
    pub fn dismiss_basics_card(&mut self) {
        self.basics_card_dismissed = true;
        self.basics_card_pending_config = None;
        self.persist();
    }

    fn persist(&self) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        if let Err(error) = self.write_to(path) {
            eventline::warn!(
                "state: failed to write {}: {error}; one-time onboarding notices may be offered again",
                path.display()
            );
        }
    }

    fn write_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = String::from(HEADER);
        if let Some(pending) = self.basics_card_pending_config.as_deref() {
            text.push_str(&format!("{PENDING_CONFIG_KEY} \"{pending}\"\n"));
        }
        text.push_str(&format!(
            "{DISMISSED_KEY} {}\n",
            if self.basics_card_dismissed {
                "true"
            } else {
                "false"
            }
        ));
        text.push_str(&format!(
            "{DECAY_NOTICE_KEY} {}\n",
            if self.decay_notice_shown {
                "true"
            } else {
                "false"
            }
        ));
        // Write through a temporary sibling so a crash cannot leave a
        // truncated state file behind.
        let temporary = path.with_extension("rune.new");
        fs::write(&temporary, text)?;
        fs::rename(&temporary, path)
    }
}

/// Whether the one-time card may appear without the user asking for it.
///
/// Only a configuration Halley generated itself, reaching its first successful
/// native session, and not yet dismissed, qualifies. Nested development
/// sessions never show the card automatically, and neither do existing
/// configurations.
pub(super) fn first_run_eligible(
    native_session: bool,
    pending_for_this_config: bool,
    dismissed: bool,
) -> bool {
    native_session && pending_for_this_config && !dismissed
}

/// How a keybind base modifier is written on the card. Mirrors the names used
/// by `docs/keybinds.md` rather than the rune spelling.
pub(super) fn modifier_label(modifier: halley_config::keybinds::ModifierKey) -> &'static str {
    use halley_config::keybinds::ModifierKey;
    match modifier {
        ModifierKey::Super => "Super",
        ModifierKey::LeftSuper => "Left Super",
        ModifierKey::RightSuper => "Right Super",
        ModifierKey::Alt => "Alt",
        ModifierKey::LeftAlt => "Left Alt",
        ModifierKey::RightAlt => "Right Alt",
        ModifierKey::Ctrl => "Ctrl",
        ModifierKey::LeftCtrl => "Left Ctrl",
        ModifierKey::RightCtrl => "Right Ctrl",
        ModifierKey::Shift => "Shift",
        ModifierKey::LeftShift => "Left Shift",
        ModifierKey::RightShift => "Right Shift",
    }
}

/// `$XDG_STATE_HOME/halley/state.rune`, falling back to
/// `~/.local/state/halley/state.rune`. Returns `None` when neither variable is
/// set.
fn state_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })?;
    Some(base.join(STATE_DIR).join(STATE_FILE))
}

/// A path as a single line of state-file text. Returns `None` for paths that
/// cannot be represented on one line, which is recorded as "nothing pending"
/// rather than as a corrupt entry.
fn plain(path: &Path) -> Option<&str> {
    let text = path.to_str()?;
    (!text.is_empty() && !text.contains(['"', '\n'])).then_some(text)
}

fn unquote(value: &str) -> Option<&str> {
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .filter(|inner| !inner.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "halley-basics-{}-{}-{}",
                std::process::id(),
                name,
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).expect("create scratch dir");
            Self(dir)
        }

        fn path(&self) -> PathBuf {
            self.0.join(STATE_DIR).join(STATE_FILE)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn config(name: &str) -> PathBuf {
        PathBuf::from("/home/user/.config/halley").join(name)
    }

    #[test]
    fn a_missing_state_file_starts_empty() {
        let scratch = ScratchDir::new("missing");
        let state = UserState::load_from(scratch.path());

        assert!(!state.basics_card_dismissed());
        assert!(!state.basics_card_pending_for(&config("halley.rune")));
    }

    #[test]
    fn a_fresh_config_is_only_pending_for_its_own_path() {
        let scratch = ScratchDir::new("pending");
        let mut state = UserState::load_from(scratch.path());
        state.record_fresh_config(&config("halley.rune"));

        assert!(state.basics_card_pending_for(&config("halley.rune")));
        assert!(
            !state.basics_card_pending_for(&config("other.rune")),
            "a card owed to one configuration must not leak into another"
        );
        assert!(!state.basics_card_dismissed());
    }

    #[test]
    fn dismissal_persists_across_reload_and_clears_the_pending_card() {
        let scratch = ScratchDir::new("dismiss");
        let mut state = UserState::load_from(scratch.path());
        state.record_fresh_config(&config("halley.rune"));
        state.dismiss_basics_card();

        let reloaded = UserState::load_from(scratch.path());
        assert!(reloaded.basics_card_dismissed());
        assert!(
            !reloaded.basics_card_pending_for(&config("halley.rune")),
            "a dismissed card is no longer pending for its configuration"
        );
        assert!(
            !first_run_eligible(
                true,
                reloaded.basics_card_pending_for(&config("halley.rune")),
                reloaded.basics_card_dismissed()
            ),
            "the card must never reappear automatically after dismissal"
        );
    }

    #[test]
    fn dismissal_is_recorded_even_when_no_card_was_pending() {
        let scratch = ScratchDir::new("dismiss-without-pending");
        let mut state = UserState::load_from(scratch.path());
        state.dismiss_basics_card();

        let reloaded = UserState::load_from(scratch.path());
        assert!(reloaded.basics_card_dismissed());
        let text = fs::read_to_string(scratch.path()).unwrap();
        assert!(text.contains("basics-card-dismissed true"));
        assert!(!text.contains(PENDING_CONFIG_KEY));
    }

    #[test]
    fn an_unparseable_state_file_falls_back_to_empty_state() {
        let scratch = ScratchDir::new("corrupt");
        let path = scratch.path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "basics-card-dismissed maybe\n\u{0}\nnot a key\n").unwrap();

        let mut state = UserState::load_from(path.clone());
        assert!(!state.basics_card_dismissed());

        // The retained path still allows a later write to repair the file.
        state.record_fresh_config(&config("halley.rune"));
        let reloaded = UserState::load_from(path);
        assert!(reloaded.basics_card_pending_for(&config("halley.rune")));
    }

    #[test]
    fn paths_with_spaces_round_trip() {
        let scratch = ScratchDir::new("spaces");
        let mut state = UserState::load_from(scratch.path());
        let spaced = PathBuf::from("/home/user/my configs/halley.rune");
        state.record_fresh_config(&spaced);

        let reloaded = UserState::load_from(scratch.path());
        assert!(reloaded.basics_card_pending_for(&spaced));
        assert!(!reloaded.basics_card_pending_for(&config("halley.rune")));
    }

    #[test]
    fn a_path_that_cannot_be_represented_is_not_recorded() {
        let scratch = ScratchDir::new("unrepresentable");
        let mut state = UserState::load_from(scratch.path());
        state.record_fresh_config(Path::new("/home/user/\"quoted\"/halley.rune"));

        assert!(!state.basics_card_pending_for(Path::new("/home/user/\"quoted\"/halley.rune")));
    }

    #[test]
    fn eligibility_requires_a_fresh_config_a_native_session_and_no_dismissal() {
        assert!(first_run_eligible(true, true, false));
        assert!(
            !first_run_eligible(false, true, false),
            "nested development sessions never show the card automatically"
        );
        assert!(
            !first_run_eligible(true, false, false),
            "existing configurations never show the card automatically"
        );
        assert!(
            !first_run_eligible(true, true, true),
            "a dismissed card stays dismissed"
        );
        assert!(!first_run_eligible(false, false, true));
    }

    #[test]
    fn state_without_a_resolvable_directory_stays_in_memory() {
        let mut state = UserState::default();
        state.record_fresh_config(&config("halley.rune"));
        state.dismiss_basics_card();
        assert!(state.basics_card_dismissed());
        assert!(!state.basics_card_pending_for(&config("halley.rune")));
    }

    #[test]
    fn modifier_labels_match_the_documented_spellings() {
        use halley_config::keybinds::ModifierKey;
        assert_eq!(modifier_label(ModifierKey::Super), "Super");
        assert_eq!(modifier_label(ModifierKey::LeftSuper), "Left Super");
        assert_eq!(modifier_label(ModifierKey::Alt), "Alt");
        assert_eq!(modifier_label(ModifierKey::Ctrl), "Ctrl");
    }

    #[test]
    fn the_decay_explanation_is_not_shown_before_it_happens() {
        let scratch = ScratchDir::new("decay-notice-fresh");
        let state = UserState::load_from(scratch.path());

        assert!(
            !state.decay_notice_shown(),
            "a fresh installation still owes the one-time explanation"
        );
    }

    #[test]
    fn the_decay_explanation_is_one_shot_across_reloads() {
        let scratch = ScratchDir::new("decay-notice-once");
        let mut state = UserState::load_from(scratch.path());
        state.record_decay_notice_shown();

        let mut reloaded = UserState::load_from(scratch.path());
        assert!(
            reloaded.decay_notice_shown(),
            "the explanation must not reappear in a later session"
        );
        let text = fs::read_to_string(scratch.path()).unwrap();
        assert!(text.contains("decay-notice-shown true"));

        // Recording it again is harmless and stays recorded.
        reloaded.record_decay_notice_shown();
        assert!(UserState::load_from(scratch.path()).decay_notice_shown());
    }

    #[test]
    fn the_decay_explanation_does_not_disturb_the_basics_card_state() {
        let scratch = ScratchDir::new("decay-notice-independence");
        let mut state = UserState::load_from(scratch.path());
        state.record_fresh_config(&config("halley.rune"));
        state.record_decay_notice_shown();

        let reloaded = UserState::load_from(scratch.path());
        assert!(reloaded.decay_notice_shown());
        assert!(
            reloaded.basics_card_pending_for(&config("halley.rune")),
            "recording the explanation must not consume the pending card"
        );
        assert!(!reloaded.basics_card_dismissed());
    }

    #[test]
    fn a_state_file_without_the_decay_key_still_loads() {
        let scratch = ScratchDir::new("decay-notice-old-file");
        let path = scratch.path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "basics-card-dismissed true\n").unwrap();

        let mut state = UserState::load_from(path.clone());
        assert!(
            !state.decay_notice_shown(),
            "state written before this key existed reads as not yet shown"
        );

        state.record_decay_notice_shown();
        assert!(UserState::load_from(path).decay_notice_shown());
    }

    #[test]
    fn state_without_a_resolvable_directory_still_records_the_explanation() {
        let mut state = UserState::default();
        state.record_decay_notice_shown();
        assert!(state.decay_notice_shown());
    }
}
