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
/// it would collapse the window the user is working in instead.
/// `zoom_fit_key` is the chord actually bound to `zoom-fit`; without one, the
/// notice only offers the click and invents no key.
pub(super) fn collapsed_into_node_message(
    title: &str,
    app_id: Option<&str>,
    zoom_fit_key: Option<&str>,
) -> String {
    let name = application_name(title, app_id).unwrap_or("A window");
    match zoom_fit_key {
        Some(key) => format!(
            "{name} was collapsed into a node. It's still running. Click here to bring it back, or press {key} to zoom out and find it."
        ),
        None => format!(
            "{name} was collapsed into a node. It's still running. Click here to bring it back."
        ),
    }
}

/// The chord bound to `zoom-fit` in the Field, where decay happens.
pub(super) fn zoom_fit_key(binds: &[crate::input::keybinds::ResolvedBind]) -> Option<String> {
    crate::input::keybinds::shortcut_label(binds, crate::input::BindingContext::field(), |action| {
        *action == halley_config::Action::ZoomFit
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

/// The collapsed node a click at `position` would restore, if the decay
/// notice is under it. Modal owners, grabs and pointer constraints keep the
/// click, as they do for the screenshot card.
pub(super) fn click_target<D: super::SessionDriver>(
    session: &super::Session<D>,
    position: smithay::utils::Point<f64, smithay::utils::Logical>,
) -> Option<halley_core::field::NodeId> {
    if session.session_lock.active()
        || session.capture.is_active()
        || session.shell.overlays.confirmation_modal_active()
        || session.shell.cluster_composer.is_active()
        || session.interactions.has_grab()
        || session.seat.get_pointer().is_some_and(|p| p.is_grabbed())
        || super::pointer::has_active_constraint(session)
    {
        return None;
    }
    let now = crate::frame_clock::monotonic_now();
    session.wayland.space.outputs().find_map(|output| {
        let geometry = session.wayland.space.output_geometry(output)?;
        if !geometry.to_f64().contains(position) {
            return None;
        }
        let local = position - geometry.loc.to_f64();
        session
            .shell
            .overlays
            .notice_target_at(&output.name(), local, now)
    })
}

/// Restores the window a clicked decay notice describes: it is revealed on
/// screen and focused, and the notice fades out. Nothing else changes focus,
/// so the window the user was working in is only left by this deliberate
/// click.
pub(super) fn restore_target<D: super::SessionDriver>(
    session: &mut super::Session<D>,
    id: halley_core::field::NodeId,
) {
    session
        .shell
        .overlays
        .dismiss_notice(crate::frame_clock::monotonic_now());
    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
    if !crate::nodes::focus_or_reveal_node(session, id, serial, true) {
        eventline::info!(
            "decay notice: node {} is gone; nothing to restore",
            id.as_u64()
        );
    }
    session.request_redraw();
}

/// Claims a left click on the decay notice. Its release is suppressed with
/// the screenshot card's, so the client beneath sees neither half.
pub(super) fn handle_pointer<D, B>(
    session: &mut super::Session<D>,
    event: &smithay::backend::input::InputEvent<B>,
) -> bool
where
    D: super::SessionDriver,
    B: smithay::backend::input::InputBackend,
{
    use smithay::backend::input::{ButtonState, InputEvent, PointerButtonEvent};
    let InputEvent::PointerButton { event } = event else {
        return false;
    };
    if event.state() != ButtonState::Pressed
        || event.button_code() != crate::input::buttons::BTN_LEFT
    {
        return false;
    }
    let Some(id) = click_target(session, session.pointer.position().into()) else {
        return false;
    };
    session
        .interactions
        .screenshot_buttons
        .suppress(event.button_code());
    restore_target(session, id);
    true
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
            "Firefox was collapsed into a node. It's still running. Click here to bring it back, or press Super+O to zoom out and find it."
        );
        assert!(!message.contains("Mod+N"));
    }

    #[test]
    fn the_message_names_only_a_bound_zoom_fit_key() {
        assert_eq!(
            collapsed_into_node_message("Untitled", None, None),
            "A window was collapsed into a node. It's still running. Click here to bring it back."
        );
        assert!(
            collapsed_into_node_message("", Some("org.gnome.Nautilus"), Some("Alt+O"))
                .starts_with("org.gnome.Nautilus was collapsed")
        );
    }

    #[test]
    fn the_zoom_fit_key_follows_the_configured_binding() {
        use crate::input::keybinds::{BackendKind, resolve_binds};
        let mut keybinds = halley_config::Keybinds::default();
        for bind in &mut keybinds.binds {
            if bind.action == halley_config::Action::ZoomFit {
                bind.key = "g".into();
            }
        }
        let binds = resolve_binds(&keybinds, BackendKind::Tty);
        assert_eq!(zoom_fit_key(&binds).as_deref(), Some("Super+G"));
        keybinds
            .binds
            .retain(|bind| bind.action != halley_config::Action::ZoomFit);
        assert_eq!(
            zoom_fit_key(&resolve_binds(&keybinds, BackendKind::Tty)),
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
