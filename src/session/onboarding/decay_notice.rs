//! Wording and eligibility for the one-time automatic-decay explanation.
//!
//! Halley collapses an inactive window into a node silently and continuously;
//! the first time automatic decay does that, the user has not seen the gesture
//! before and the window simply seems to vanish. The first automatic collapse
//! therefore explains itself once, in a non-modal notice.
//!
//! Two properties are deliberate:
//!
//! - **Automatic only.** A manual `Mod+N` collapse is the user's own action, is
//!   visible as it animates, and teaches the same restore gesture by itself, so
//!   it never explains anything. Both paths share the same collapse seam, so
//!   the trigger — not the call site — decides.
//! - **Once per user state.** The explanation is a one-time onboarding fact,
//!   not per-application or per-session guidance. It is recorded in
//!   `$XDG_STATE_HOME/halley/state.rune` (see `session::basics`), so an
//!   installation sees it at most once no matter how many windows decay or how
//!   many times the session restarts.

/// The placeholder `nodes::metadata` stores when a client reports no title, so
/// it is not mistaken for a real application name.
const PLACEHOLDER_TITLE: &str = "Untitled";

/// How a window arrived at being a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CollapseTrigger {
    /// Automatic decay collapsed an inactive window while the user was
    /// elsewhere.
    Automatic,
    /// The user collapsed the window themselves, with `Mod+N` or the node's
    /// own collapse affordance.
    Manual,
}

impl CollapseTrigger {
    /// The trigger for a collapse. `decay` is Halley's own marker for the
    /// automatic decay path; every manual, client-minimize, and server-titlebar
    /// collapse shares the other branch.
    pub(super) fn for_decay(decay: bool) -> Self {
        if decay { Self::Automatic } else { Self::Manual }
    }
}

/// Whether the one-time explanation may appear for this collapse.
///
/// Only an automatic collapse may explain itself, and only until the
/// installation has seen the explanation once.
pub(super) fn explanation_due(trigger: CollapseTrigger, already_shown: bool) -> bool {
    trigger == CollapseTrigger::Automatic && !already_shown
}

/// The sentence shown for the first automatic collapse: what happened and the
/// two ways to reverse it.
pub(super) fn collapsed_into_node_message(title: &str, app_id: Option<&str>) -> String {
    match application_name(title, app_id) {
        Some(name) => format!(
            "{name} was collapsed into a node. Click the node or press Mod+N to restore it."
        ),
        None => String::from(
            "A window was collapsed into a node. Click the node or press Mod+N to restore it.",
        ),
    }
}

/// The name shown for a collapsed window: its title, which is also the label on
/// the node the user now sees, falling back to its application id. A client
/// that reported neither still gets a complete, honest sentence.
fn application_name<'a>(title: &'a str, app_id: Option<&'a str>) -> Option<&'a str> {
    let title = title.trim();
    if !title.is_empty() && title != PLACEHOLDER_TITLE {
        return Some(title);
    }
    app_id.map(str::trim).filter(|app_id| !app_id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_names_the_window_and_the_restore_gesture() {
        assert_eq!(
            collapsed_into_node_message("Firefox", Some("firefox")),
            "Firefox was collapsed into a node. Click the node or press Mod+N to restore it."
        );
    }

    #[test]
    fn the_application_id_is_the_fallback_for_a_missing_title() {
        assert_eq!(
            collapsed_into_node_message("", Some("org.gnome.Nautilus")),
            "org.gnome.Nautilus was collapsed into a node. Click the node or press Mod+N to restore it."
        );
        assert_eq!(
            collapsed_into_node_message("   ", Some("kitty")),
            "kitty was collapsed into a node. Click the node or press Mod+N to restore it."
        );
    }

    #[test]
    fn the_untitled_placeholder_is_not_shown_as_an_application_name() {
        assert_eq!(
            collapsed_into_node_message("Untitled", Some("xterm")),
            "xterm was collapsed into a node. Click the node or press Mod+N to restore it."
        );
    }

    #[test]
    fn a_window_without_a_title_or_application_id_gets_the_generic_fallback() {
        let expected =
            "A window was collapsed into a node. Click the node or press Mod+N to restore it.";
        assert_eq!(collapsed_into_node_message("", None), expected);
        assert_eq!(collapsed_into_node_message("Untitled", None), expected);
        assert_eq!(collapsed_into_node_message("", Some("  ")), expected);
    }

    #[test]
    fn only_the_automatic_decay_path_explains_a_collapse() {
        assert!(explanation_due(CollapseTrigger::for_decay(true), false));
        assert!(
            !explanation_due(CollapseTrigger::for_decay(false), false),
            "a manual Mod+N collapse must never explain itself"
        );
        assert!(
            !explanation_due(CollapseTrigger::for_decay(false), true),
            "a manual Mod+N collapse stays silent even after the explanation was shown once"
        );
    }

    #[test]
    fn the_explanation_is_one_shot_per_user_state() {
        assert!(
            !explanation_due(CollapseTrigger::for_decay(true), true),
            "automatic decay explains itself only once, not once per application"
        );
    }
}
