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

/// The sentence shown for the first automatic collapse: what happened, that
/// nothing was closed, and two ways to reach that window again.
///
/// It deliberately never suggests `toggle-state` (`Mod+N`): that acts on the
/// focused window, and decay only ever collapses unfocused ones, so following
/// it would collapse the window the user is working in instead. `apogee_key`
/// is the chord actually bound to Apogee; without one, Apogee is still named
/// but no key is invented.
pub(super) fn collapsed_into_node_message(
    title: &str,
    app_id: Option<&str>,
    apogee_key: Option<&str>,
) -> String {
    let name = application_name(title, app_id).unwrap_or("A window");
    let apogee = match apogee_key {
        Some(key) => format!("Apogee ({key})"),
        None => String::from("Apogee"),
    };
    format!(
        "{name} was collapsed into a node. It's still running. Click its node, or find it in {apogee}."
    )
}

/// The chord bound to Apogee in the Field, where decay happens.
pub(super) fn apogee_key(binds: &[crate::input::keybinds::ResolvedBind]) -> Option<String> {
    crate::input::keybinds::shortcut_label(binds, crate::input::BindingContext::field(), |action| {
        *action == halley_config::Action::Apogee
    })
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
    fn the_message_points_at_the_collapsed_window_not_the_focused_one() {
        let message = collapsed_into_node_message("Firefox", None, Some("Super+O"));
        assert_eq!(
            message,
            "Firefox was collapsed into a node. It's still running. Click its node, or find it in Apogee (Super+O)."
        );
        assert!(!message.contains("Mod+N"));
    }

    #[test]
    fn the_message_names_only_a_bound_apogee_key() {
        assert_eq!(
            collapsed_into_node_message("Untitled", None, None),
            "A window was collapsed into a node. It's still running. Click its node, or find it in Apogee."
        );
        assert!(
            collapsed_into_node_message("", Some("org.gnome.Nautilus"), Some("Alt+O"))
                .starts_with("org.gnome.Nautilus was collapsed")
        );
    }

    #[test]
    fn the_apogee_key_follows_the_configured_binding() {
        use crate::input::keybinds::{BackendKind, resolve_binds};
        let mut keybinds = halley_config::Keybinds::default();
        for bind in &mut keybinds.binds {
            if bind.action == halley_config::Action::Apogee {
                bind.key = "g".into();
            }
        }
        let binds = resolve_binds(&keybinds, BackendKind::Tty);
        assert_eq!(apogee_key(&binds).as_deref(), Some("Super+G"));
        keybinds
            .binds
            .retain(|bind| bind.action != halley_config::Action::Apogee);
        assert_eq!(
            apogee_key(&resolve_binds(&keybinds, BackendKind::Tty)),
            None
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
