use smithay::desktop::{LayerSurface, Window};
use smithay::utils::Serial;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::KeyboardInteractivity;

use super::{Session, SessionDriver};

#[derive(Clone, Copy)]
enum FocusOrigin {
    Explicit,
    Pointer,
    Hover,
    CloseSuccession,
}

fn should_raise(origin: FocusOrigin, raise_on_click: bool) -> bool {
    match origin {
        FocusOrigin::Explicit => true,
        FocusOrigin::Pointer => raise_on_click,
        FocusOrigin::Hover | FocusOrigin::CloseSuccession => false,
    }
}

pub(crate) fn focus_layer<D: SessionDriver>(
    session: &mut Session<D>,
    layer: Option<LayerSurface>,
    serial: Serial,
) {
    crate::wayland::focus::select_layer(&mut session.wayland, layer);
    super::sync_keyboard_focus(session, serial);
}

/// Focus used by explicit activation and mapping policy. These paths retain
/// their established behavior and always raise.
pub(crate) fn focus_window<D: SessionDriver>(
    session: &mut Session<D>,
    window: &Window,
    serial: Serial,
) {
    resume_fullscreen_presentation(session, window, true);
    focus_window_with_raise(
        session,
        window,
        serial,
        should_raise(FocusOrigin::Explicit, session.settings.input.raise_on_click),
    );
}

/// Pointer-initiated focus follows the configured click policy. Hover uses a
/// separate entry point below and never raises.
pub(crate) fn focus_window_from_pointer<D: SessionDriver>(
    session: &mut Session<D>,
    window: &Window,
    serial: Serial,
) {
    resume_fullscreen_presentation(session, window, false);
    focus_window_with_raise(
        session,
        window,
        serial,
        should_raise(FocusOrigin::Pointer, session.settings.input.raise_on_click),
    );
}

/// Focuses the successor to a window that just closed without disturbing the
/// surviving windows' stack order, including windows on other outputs.
pub(crate) fn focus_window_after_close<D: SessionDriver>(
    session: &mut Session<D>,
    window: &Window,
    serial: Serial,
) {
    focus_window_with_raise(
        session,
        window,
        serial,
        should_raise(
            FocusOrigin::CloseSuccession,
            session.settings.input.raise_on_click,
        ),
    );
}

fn resume_fullscreen_presentation<D: SessionDriver>(
    session: &mut Session<D>,
    window: &Window,
    explicit_focus: bool,
) {
    let Some(surface) = window.wl_surface() else {
        return;
    };
    let output_name = if explicit_focus {
        session
            .fullscreen
            .resume_presentation_on_explicit_focus(surface.as_ref())
    } else {
        session.fullscreen.resume_presentation(surface.as_ref())
    };
    let Some(output_name) = output_name else {
        return;
    };
    let output = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)
        .cloned();
    if let Some(output) = output.as_ref() {
        session.sync_fullscreen_camera(output, crate::frame_clock::monotonic_now());
    }
    session.request_redraw();
}

fn focus_window_with_raise<D: SessionDriver>(
    session: &mut Session<D>,
    window: &Window,
    serial: Serial,
    raise: bool,
) {
    let window = stacking_front_window(session, window).unwrap_or_else(|| window.clone());
    let modal = modal_focus_target(session, &window);
    let raise = raise || modal.is_some();
    let window = modal.unwrap_or(window);
    if !crate::window::accepts_wm_focus(&window) {
        return;
    }
    crate::window::focus(&mut session.wayland, &window, raise);
    if let Some(surface) = window.wl_surface() {
        let changed = session.nodes.focus_surface(
            surface.as_ref(),
            session.start_time.elapsed().as_millis() as u64,
        );
        if changed && let Some(id) = session.nodes.id_for_surface(surface.as_ref()) {
            session.record_trail_focus(id);
        }
    }
    if raise {
        session.xwayland.raise_window(&window);
    }
    super::sync_keyboard_focus(session, serial);
}

fn modal_focus_target<D: SessionDriver>(
    session: &Session<D>,
    requested: &Window,
) -> Option<Window> {
    let now = crate::frame_clock::monotonic_now();
    crate::window::dialog::focus_target(&session.wayland.space, requested, |candidate| {
        let Some(surface) = candidate.wl_surface() else {
            return false;
        };
        let Some(record) = session
            .nodes
            .id_for_surface(surface.as_ref())
            .and_then(|id| session.nodes.record(id))
        else {
            return false;
        };
        record.attached
            && !record.collapsed
            && crate::window::accepts_wm_focus(candidate)
            && crate::presentation::surface_workspace_is_active(
                &session.clusters,
                &session.nodes,
                surface.as_ref(),
                &record.output,
                now,
            )
    })
}

/// Reconcile late modal hints and parent changes as well as focus paths which
/// write the persistent window directly. Preserve temporary layer focus.
pub(super) fn reconcile_modal_focus<D: SessionDriver>(session: &mut Session<D>) {
    let requested = session.wayland.focused_window.as_ref().and_then(|surface| {
        session
            .wayland
            .space
            .elements()
            .find(|window| {
                window
                    .wl_surface()
                    .is_some_and(|candidate| candidate.as_ref() == surface)
            })
            .cloned()
    });
    let Some(target) = requested.and_then(|window| modal_focus_target(session, &window)) else {
        return;
    };
    let layer = session.wayland.focused_layer.take();
    crate::window::focus(&mut session.wayland, &target, true);
    session.wayland.focused_layer = layer;
    session.request_redraw();
}

/// A stacking workspace has one keyboard-active card: its front member.
/// Pointer hover, close succession, and generic activation must not revive a
/// rear layout card merely because Smithay's underlying space order still
/// mentions it. Floating members and transient/non-member windows remain
/// independently focusable.
fn stacking_front_window<D: SessionDriver>(
    session: &Session<D>,
    requested: &Window,
) -> Option<Window> {
    let requested_id = requested
        .wl_surface()
        .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))?;
    let cluster = session.clusters.cluster_for_member(requested_id)?;
    let metadata = session.clusters.metadata(cluster)?;
    if !should_redirect_to_stacking_front(
        metadata.layout,
        session.clusters.active_on(&metadata.output) == Some(cluster),
        session.clusters.is_member_floating(requested_id),
    ) {
        return None;
    }
    session
        .clusters
        .member_ids(cluster)
        .into_iter()
        .filter_map(|member| session.nodes.record(member))
        .find(|record| {
            record.attached
                && !record.collapsed
                && !session.clusters.is_member_floating(record.id)
                && session
                    .wayland
                    .space
                    .elements()
                    .any(|mapped| mapped == &record.window)
        })
        .map(|record| record.window.clone())
}

fn should_redirect_to_stacking_front(
    layout: halley_core::cluster::layout::ClusterWorkspaceLayoutKind,
    cluster_active: bool,
    requested_floating: bool,
) -> bool {
    layout == halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Stacking
        && cluster_active
        && !requested_floating
}

pub(super) fn focus_node_from_hover<D: SessionDriver>(
    session: &mut Session<D>,
    id: halley_core::field::NodeId,
    output: &smithay::output::Output,
    serial: Serial,
) {
    if session.settings.input.focus_mode != halley_config::FocusMode::Hover
        || hover_is_blocked(session)
        || session.nodes.focused() == Some(id)
        || !session
            .nodes
            .record(id)
            .is_some_and(|record| record.attached && record.collapsed)
    {
        return;
    }
    focus_collapsed_node(session, id, output, serial);
}

/// Cluster cores participate in hover focus exactly like ordinary collapsed
/// nodes.  Cores do not have a `NodeRecord`, so they need their own entry
/// point instead of going through `focus_node_from_hover`'s surface checks.
pub(super) fn focus_cluster_core_from_hover<D: SessionDriver>(
    session: &mut Session<D>,
    id: halley_core::field::NodeId,
    output: &smithay::output::Output,
    serial: Serial,
) {
    if session.settings.input.focus_mode != halley_config::FocusMode::Hover
        || hover_is_blocked(session)
        || session.nodes.focused() == Some(id)
        || session.clusters.cluster_for_core(id).is_none()
    {
        return;
    }
    focus_collapsed_node(session, id, output, serial);
}

pub(super) fn focus_node_from_pointer<D: SessionDriver>(
    session: &mut Session<D>,
    id: halley_core::field::NodeId,
    output: &smithay::output::Output,
    serial: Serial,
) {
    if !session
        .nodes
        .record(id)
        .is_some_and(|record| record.attached && record.collapsed)
    {
        return;
    }
    focus_collapsed_node(session, id, output, serial);
}

fn focus_collapsed_node<D: SessionDriver>(
    session: &mut Session<D>,
    id: halley_core::field::NodeId,
    output: &smithay::output::Output,
    serial: Serial,
) {
    crate::wayland::focus::select_output(&mut session.wayland, output);
    crate::window::clear_focus(&mut session.wayland);
    session
        .nodes
        .focus(Some(id), session.start_time.elapsed().as_millis() as u64);
    super::sync_keyboard_focus(session, serial);
    session.request_redraw();
}

pub(super) fn update_hover<D: SessionDriver>(
    session: &mut Session<D>,
    route: &crate::input::pointer::PointerRoute,
    serial: Serial,
) {
    if session.settings.input.focus_mode != halley_config::FocusMode::Hover
        || hover_is_blocked(session)
    {
        return;
    }

    crate::wayland::focus::select_output(&mut session.wayland, &route.output);
    match &route.target {
        crate::input::pointer::PointerTarget::Window(window) => {
            let window = stacking_front_window(session, window).unwrap_or_else(|| window.clone());
            let surface = window.wl_surface().map(std::borrow::Cow::into_owned);
            let node = surface
                .as_ref()
                .and_then(|surface| session.nodes.id_for_surface(surface));
            if surface.is_some()
                && (session.wayland.focused_window != surface || session.nodes.focused() != node)
            {
                focus_window_with_raise(
                    session,
                    &window,
                    serial,
                    should_raise(FocusOrigin::Hover, session.settings.input.raise_on_click),
                );
            }
        }
        crate::input::pointer::PointerTarget::Decoration { window, .. } => {
            let window = stacking_front_window(session, window).unwrap_or_else(|| window.clone());
            let surface = window.wl_surface().map(std::borrow::Cow::into_owned);
            let node = surface
                .as_ref()
                .and_then(|surface| session.nodes.id_for_surface(surface));
            if surface.is_some()
                && (session.wayland.focused_window != surface || session.nodes.focused() != node)
            {
                focus_window_with_raise(
                    session,
                    &window,
                    serial,
                    should_raise(FocusOrigin::Hover, session.settings.input.raise_on_click),
                );
            }
        }
        crate::input::pointer::PointerTarget::Layer(layer)
            if layer.cached_state().keyboard_interactivity == KeyboardInteractivity::OnDemand =>
        {
            if session
                .wayland
                .focused_layer
                .as_ref()
                .is_none_or(|focused| focused != layer)
            {
                focus_layer(session, Some(layer.clone()), serial);
            }
        }
        crate::input::pointer::PointerTarget::Layer(_)
        | crate::input::pointer::PointerTarget::Background => {}
    }
}

fn hover_is_blocked<D: SessionDriver>(session: &Session<D>) -> bool {
    let pointer_grabbed = session
        .seat
        .get_pointer()
        .is_some_and(|pointer| pointer.is_grabbed());
    let exclusive_layer = matches!(
        crate::wayland::focus::current(
            &session.wayland,
            &session.fullscreen,
            &session.clusters,
            &session.nodes,
            crate::frame_clock::monotonic_now(),
        ),
        Some(crate::wayland::focus::KeyboardFocus::ExclusiveLayer(_))
    );

    pointer_grabbed
        || !matches!(session.interactions.grab, crate::input::grab::Grab::None)
        || session.capture.is_active()
        || session.shell.apogee.is_active()
        || session.shell.focus_cycle.is_open()
        || super::pointer::has_active_constraint(session)
        || exclusive_layer
}

#[cfg(test)]
mod tests {
    use halley_core::cluster::layout::ClusterWorkspaceLayoutKind;

    use super::{FocusOrigin, should_raise, should_redirect_to_stacking_front};

    #[test]
    fn focus_origins_choose_the_expected_raise_policy() {
        assert!(should_raise(FocusOrigin::Explicit, false));
        assert!(should_raise(FocusOrigin::Explicit, true));
        assert!(!should_raise(FocusOrigin::Pointer, false));
        assert!(should_raise(FocusOrigin::Pointer, true));
        assert!(!should_raise(FocusOrigin::Hover, false));
        assert!(!should_raise(FocusOrigin::Hover, true));
        assert!(!should_raise(FocusOrigin::CloseSuccession, false));
        assert!(!should_raise(FocusOrigin::CloseSuccession, true));
    }

    #[test]
    fn floating_member_bypasses_stacking_front_redirection() {
        assert!(should_redirect_to_stacking_front(
            ClusterWorkspaceLayoutKind::Stacking,
            true,
            false,
        ));
        assert!(!should_redirect_to_stacking_front(
            ClusterWorkspaceLayoutKind::Stacking,
            true,
            true,
        ));
    }
}
