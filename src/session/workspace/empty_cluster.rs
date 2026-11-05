use std::time::Duration;

use super::{Session, SessionDriver};
use crate::input::keybinds::ResolvedBind;
use crate::shell::empty_cluster::Target;

/// Keep hints on every open empty workspace. Actual focus and modal state
/// separately govern keyboard deletion; launchers are not workspace members.
pub(super) fn sync<D: SessionDriver>(session: &mut Session<D>, now: Duration) -> bool {
    let selected = crate::wayland::focus::selected_output(&session.wayland).map(|o| o.name());
    let hints = session
        .wayland
        .space
        .outputs()
        .filter_map(|output| {
            let output_name = output.name();
            let id = session.clusters.active_on(&output_name)?;
            if !session.clusters.member_ids(id).is_empty() {
                return None;
            }
            let context = super::input::binding_context_for_output(session, Some(&output_name));
            Some(Target {
                id,
                shortcut: close_shortcut(&session.keyboard.binds, context)?,
                output: output_name,
                name: session.clusters.metadata(id)?.name.clone(),
            })
        })
        .collect::<Vec<_>>();
    let target = hints
        .iter()
        .find(|hint| Some(&hint.output) == selected.as_ref())
        .cloned();
    let focused = crate::wayland::focus::current(
        &session.wayland,
        &session.fullscreen,
        &session.clusters,
        &session.nodes,
        now,
    );
    let layer_focused = matches!(
        focused,
        Some(
            crate::wayland::focus::KeyboardFocus::ExclusiveLayer(_)
                | crate::wayland::focus::KeyboardFocus::OnDemandLayer(_)
        )
    );
    let blocked = session.session_lock.active()
        || session.shell.overlays.confirmation_modal_active()
        || session.shell.overlays.basics_card_accepts_input()
        || session.capture.is_active()
        || session.shell.apogee.is_active()
        || session.shell.focus_cycle.is_open()
        || session.shell.cluster_composer.is_active()
        || session.clusters.accepts_modal_input()
        || layer_focused;
    // A local floating window still owns its close action. Its focus does not
    // change the fact that the underlying empty workspace needs a hint.
    let window_focused =
        if let Some(crate::wayland::focus::KeyboardFocus::Window(surface)) = focused {
            session
                .nodes
                .id_for_surface(&surface)
                .and_then(|id| session.nodes.record(id))
                .is_none_or(|record| Some(&record.output) == selected.as_ref())
        } else {
            false
        };
    session.shell.overlays.empty_cluster.sync_outputs(
        if blocked { Vec::new() } else { hints },
        target,
        blocked || window_focused,
        now,
    )
}

fn close_shortcut(binds: &[ResolvedBind], context: crate::input::BindingContext) -> Option<String> {
    crate::input::keybinds::shortcut_label(binds, context, |action| {
        *action == halley_config::Action::CloseFocusedWindow
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keybinds::{BackendKind, resolve_binds};
    #[test]
    fn shortcut_uses_resolved_backend_modifiers_and_custom_binding() {
        let mut keybinds = halley_config::Keybinds::default();
        keybinds
            .binds
            .retain(|bind| bind.action == halley_config::Action::CloseFocusedWindow);
        keybinds.binds[0].key = "x".into();
        keybinds.binds[0].modifiers.shift = true;
        let context = crate::input::BindingContext::cluster(true);
        assert_eq!(
            close_shortcut(&resolve_binds(&keybinds, BackendKind::Tty), context).as_deref(),
            Some("Super+Shift+X")
        );
        assert_eq!(
            close_shortcut(&resolve_binds(&keybinds, BackendKind::Winit), context).as_deref(),
            Some("Alt+Shift+X")
        );
    }
    #[test]
    fn field_only_missing_and_shadowed_bindings_are_not_advertised() {
        let mut binds = resolve_binds(&halley_config::Keybinds::default(), BackendKind::Tty);
        binds.retain(|b| b.action == halley_config::Action::CloseFocusedWindow);
        let context = crate::input::BindingContext::cluster(true);
        assert!(close_shortcut(&binds, context).is_some());
        binds[0].scope = halley_config::BindingScope::Field;
        assert!(close_shortcut(&binds, context).is_none());
        binds[0].scope = halley_config::BindingScope::Global;
        let mut shadow = binds[0].clone();
        shadow.action = halley_config::Action::OpenTerminal;
        binds.insert(0, shadow);
        assert!(close_shortcut(&binds, context).is_none());
    }
}
