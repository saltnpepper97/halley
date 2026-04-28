use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use eventline::{debug, warn};
use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};
use smithay::utils::Serial;

use crate::compositor::root::Halley;

pub(crate) const ACTIVATION_TOKEN_TTL_MS: u64 = 10_000;
const RECENT_INPUT_SERIAL_TTL_MS: u64 = 10_000;

#[derive(Clone, Copy, Debug)]
struct RecentInputSerial {
    serial: Serial,
    at_ms: u64,
}

#[derive(Clone, Copy, Debug)]
struct IssuedExternalToken {
    issued_at_ms: u64,
}

#[derive(Clone, Copy, Debug)]
struct PendingSurfaceActivation {
    requested_at_ms: u64,
}

#[derive(Debug, Default)]
pub(crate) struct ActivationRuntimeState {
    recent_input_serials: VecDeque<RecentInputSerial>,
    issued_external_tokens: HashMap<String, IssuedExternalToken>,
    pending_surface_activations: HashMap<ObjectId, PendingSurfaceActivation>,
}

pub(crate) fn note_input_serial(st: &mut Halley, serial: Serial, now_ms: u64) {
    prune_recent_input_serials(st, now_ms);
    st.runtime
        .activation
        .recent_input_serials
        .retain(|entry| entry.serial != serial);
    st.runtime
        .activation
        .recent_input_serials
        .push_back(RecentInputSerial {
            serial,
            at_ms: now_ms,
        });
}

pub(crate) fn issue_external_token(st: &mut Halley, now_ms: u64) -> String {
    let (token, _) = st.platform.xdg_activation_state.create_external_token(None);
    let token = token.as_str().to_string();
    st.runtime.activation.issued_external_tokens.insert(
        token.clone(),
        IssuedExternalToken {
            issued_at_ms: now_ms,
        },
    );
    token
}

pub(crate) fn consume_pending_surface_activation(st: &mut Halley, surface: &WlSurface) -> bool {
    let now_ms = st.now_ms(Instant::now());
    st.runtime
        .activation
        .pending_surface_activations
        .remove(&surface_tree_root(surface).id())
        .is_some_and(|pending| {
            now_ms.saturating_sub(pending.requested_at_ms) <= ACTIVATION_TOKEN_TTL_MS
        })
}

pub(crate) fn clear_surface_activation(st: &mut Halley, surface: &WlSurface) {
    clear_surface_activation_for_root(st, surface_tree_root(surface).id());
}

pub(crate) fn clear_surface_activation_for_root(st: &mut Halley, surface_id: ObjectId) {
    st.runtime
        .activation
        .pending_surface_activations
        .remove(&surface_id);
}

pub(crate) fn prune_expired(st: &mut Halley, now: Instant, now_ms: u64) {
    prune_recent_input_serials(st, now_ms);
    st.runtime
        .activation
        .issued_external_tokens
        .retain(|_, issued| now_ms.saturating_sub(issued.issued_at_ms) <= ACTIVATION_TOKEN_TTL_MS);
    st.runtime
        .activation
        .pending_surface_activations
        .retain(|_, pending| {
            now_ms.saturating_sub(pending.requested_at_ms) <= ACTIVATION_TOKEN_TTL_MS
        });
    st.platform
        .xdg_activation_state
        .retain_tokens(|_, data| age_ms(now, data.timestamp) <= ACTIVATION_TOKEN_TTL_MS);
}

pub(crate) fn request_surface_activation(
    st: &mut Halley,
    surface: &WlSurface,
    token: &str,
    token_data: &smithay::wayland::xdg_activation::XdgActivationTokenData,
    now: Instant,
) {
    let root = surface_tree_root(surface);
    let now_ms = st.now_ms(now);

    if !activation_token_is_valid(st, token, token_data, now, now_ms) {
        debug!(
            "ignoring invalid activation request token={} surface={:?}",
            token,
            root.id()
        );
        forget_token(st, token);
        return;
    }

    forget_token(st, token);

    if let Some(id) = st.model.surface_to_node.get(&root.id()).copied() {
        let activated =
            crate::compositor::actions::window::focus_surface_node_without_reveal(st, id, now);
        debug!(
            "applied activation request token={} node={} surface={:?} activated={}",
            token,
            id.as_u64(),
            root.id(),
            activated
        );
        return;
    }

    st.runtime.activation.pending_surface_activations.insert(
        root.id(),
        PendingSurfaceActivation {
            requested_at_ms: now_ms,
        },
    );
    debug!(
        "queued activation request token={} for pending surface {:?}",
        token,
        root.id()
    );
}

fn activation_token_is_valid(
    st: &Halley,
    token: &str,
    token_data: &smithay::wayland::xdg_activation::XdgActivationTokenData,
    now: Instant,
    now_ms: u64,
) -> bool {
    if st
        .runtime
        .activation
        .issued_external_tokens
        .get(token)
        .is_some_and(|issued| now_ms.saturating_sub(issued.issued_at_ms) <= ACTIVATION_TOKEN_TTL_MS)
    {
        return true;
    }

    if age_ms(now, token_data.timestamp) > ACTIVATION_TOKEN_TTL_MS {
        return false;
    }

    let Some(serial) = token_data.serial.as_ref().map(|(serial, _seat)| *serial) else {
        return false;
    };

    has_recent_input_serial(st, serial, now_ms)
}

fn has_recent_input_serial(st: &Halley, serial: Serial, now_ms: u64) -> bool {
    st.runtime
        .activation
        .recent_input_serials
        .iter()
        .any(|entry| {
            entry.serial == serial
                && now_ms.saturating_sub(entry.at_ms) <= RECENT_INPUT_SERIAL_TTL_MS
        })
}

fn forget_token(st: &mut Halley, token: &str) {
    st.runtime.activation.issued_external_tokens.remove(token);
    let removed = st
        .platform
        .xdg_activation_state
        .remove_token(&token.to_string().into());
    if !removed {
        warn!("activation token {} was not tracked by smithay", token);
    }
}

fn prune_recent_input_serials(st: &mut Halley, now_ms: u64) {
    while st
        .runtime
        .activation
        .recent_input_serials
        .front()
        .is_some_and(|entry| now_ms.saturating_sub(entry.at_ms) > RECENT_INPUT_SERIAL_TTL_MS)
    {
        st.runtime.activation.recent_input_serials.pop_front();
    }
}

fn age_ms(now: Instant, timestamp: Instant) -> u64 {
    now.checked_duration_since(timestamp)
        .unwrap_or_default()
        .as_millis() as u64
}

fn surface_tree_root(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
        root = parent;
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_token_is_valid_until_ttl() {
        let mut activation = ActivationRuntimeState::default();
        activation.issued_external_tokens.insert(
            "token".to_string(),
            IssuedExternalToken { issued_at_ms: 100 },
        );

        assert!(
            activation
                .issued_external_tokens
                .get("token")
                .is_some_and(|issued| 10_099u64.saturating_sub(issued.issued_at_ms)
                    <= ACTIVATION_TOKEN_TTL_MS)
        );
        assert!(
            !activation
                .issued_external_tokens
                .get("token")
                .is_some_and(|issued| 10_101u64.saturating_sub(issued.issued_at_ms)
                    <= ACTIVATION_TOKEN_TTL_MS)
        );
    }

    #[test]
    fn recent_input_serial_check_is_age_bounded() {
        let serial = Serial::from(77);
        let recent = RecentInputSerial { serial, at_ms: 300 };

        assert!(recent.serial == serial);
        assert!(9_900u64.saturating_sub(recent.at_ms) <= RECENT_INPUT_SERIAL_TTL_MS);
        assert!(10_301u64.saturating_sub(recent.at_ms) > RECENT_INPUT_SERIAL_TTL_MS);
    }
}
