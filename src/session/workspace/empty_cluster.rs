use std::time::Duration;

use super::{Session, SessionDriver};
use crate::input::keybinds::{ResolvedBind, ResolvedTrigger};
use crate::shell::empty_cluster::Target;

/// Reconcile against actual focus and modal state, not just window count:
/// layer-shell launchers are not workspace members.
pub(super) fn sync<D: SessionDriver>(session: &mut Session<D>, now: Duration) -> bool {
    let output = crate::wayland::focus::selected_output(&session.wayland);
    let target = output.as_ref().and_then(|output| {
        let output_name = output.name();
        let id = session.clusters.active_on(&output_name)?;
        if !session.clusters.member_ids(id).is_empty() {
            return None;
        }
        let context = super::input::binding_context_for_output(session, Some(&output_name));
        let shortcut = close_shortcut(&session.keyboard.binds, context)?;
        Some(Target {
            id,
            output: output_name,
            name: session.clusters.metadata(id)?.name.clone(),
            shortcut,
        })
    });
    if target.is_none() {
        return session.shell.overlays.empty_cluster.sync(None, false, now);
    }
    let blocked = session.session_lock.active()
        || session.shell.overlays.confirmation_modal_active()
        || session.shell.overlays.basics_card_accepts_input()
        || session.capture.is_active()
        || session.shell.apogee.is_active()
        || session.shell.focus_cycle.is_open()
        || session.shell.cluster_composer.is_active()
        || session.clusters.accepts_modal_input()
        || crate::wayland::focus::current(
            &session.wayland,
            &session.fullscreen,
            &session.clusters,
            &session.nodes,
            now,
        )
        .is_some();
    session
        .shell
        .overlays
        .empty_cluster
        .sync(target, blocked, now)
}

fn close_shortcut(binds: &[ResolvedBind], context: crate::input::BindingContext) -> Option<String> {
    binds.iter().enumerate().find_map(|(index, bind)| {
        if bind.action != halley_config::Action::CloseFocusedWindow || !context.allows(bind.scope) {
            return None;
        }
        // The input matcher takes the first active chord; don't advertise a
        // shadowed close binding or a pointer-only trigger as a keyboard key.
        if binds[..index].iter().any(|other| {
            context.allows(other.scope)
                && other.modifiers == bind.modifiers
                && other.trigger == bind.trigger
        }) {
            return None;
        }
        let key = match bind.trigger {
            ResolvedTrigger::Keysym(key) => {
                let name = key.name()?;
                let name = name.strip_prefix("XK_").unwrap_or(name);
                if name.len() == 1 {
                    name.to_uppercase()
                } else {
                    name.to_string()
                }
            }
            ResolvedTrigger::Keycode(key) => format!("Keycode {}", key.raw().saturating_sub(8)),
            _ => return None,
        };
        let m = bind.modifiers;
        let mut parts = Vec::new();
        for (generic, left, right, label) in [
            (m.super_key, m.left_super, m.right_super, "Super"),
            (m.ctrl, m.left_ctrl, m.right_ctrl, "Ctrl"),
            (m.alt, m.left_alt, m.right_alt, "Alt"),
            (m.shift, m.left_shift, m.right_shift, "Shift"),
        ] {
            if generic {
                parts.push(label.to_string());
            } else {
                if left {
                    parts.push(format!("Left {label}"));
                }
                if right {
                    parts.push(format!("Right {label}"));
                }
            }
        }
        parts.push(key);
        Some(parts.join("+"))
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
