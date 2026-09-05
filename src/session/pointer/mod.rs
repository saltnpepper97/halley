mod constraints;

pub(super) use constraints::PointerConstraintLifecycle;

use smithay::input::pointer::{MotionEvent, PointerHandle, RelativeMotionEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use smithay::wayland::seat::WaylandFocus;

use super::{Session, SessionDriver};

#[derive(Debug)]
pub(super) struct ConstraintDiagnostic {
    pub(super) protocol_resource: Option<String>,
    pub(super) protocol_kind: Option<&'static str>,
    pub(super) protocol_active: bool,
    pub(super) halley_tracked: bool,
    pub(super) tracked_kind: Option<&'static str>,
    pub(super) tracked_surface_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AbsoluteMotionPolicy {
    Position,
    Surface,
    RoutedState,
}

fn should_emit_absolute_motion(
    policy: AbsoluteMotionPolicy,
    surface_changed: bool,
    routed_state_changed: bool,
    locked: bool,
) -> bool {
    // An active locked pointer must receive relative motion only. Sending
    // `wl_pointer.motion` while locked lets XWayland replace its emulated
    // cursor anchor and breaks relative-look clients.
    !locked
        && match policy {
            AbsoluteMotionPolicy::Position => true,
            AbsoluteMotionPolicy::Surface => surface_changed,
            AbsoluteMotionPolicy::RoutedState => routed_state_changed,
        }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClientPointerRoute {
    focus: Option<crate::input::pointer::SurfaceFocus>,
    location: Point<f64, Logical>,
}

impl ClientPointerRoute {
    fn from_route(route: &crate::input::pointer::PointerRoute) -> Self {
        Self {
            focus: route.focus.clone(),
            location: route.location,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct DesktopRefreshBlockers {
    pointer_grabbed: bool,
    constraint_active: bool,
    compositor_grab_active: bool,
    capture_active: bool,
    shell_active: bool,
    focus_cycle_open: bool,
    session_locked: bool,
    fullscreen_active: bool,
}

fn desktop_refresh_allowed(blockers: DesktopRefreshBlockers) -> bool {
    !blockers.pointer_grabbed
        && !blockers.constraint_active
        && !blockers.compositor_grab_active
        && !blockers.capture_active
        && !blockers.shell_active
        && !blockers.focus_cycle_open
        && !blockers.session_locked
        && !blockers.fullscreen_active
}

pub(super) fn route_client<D: SessionDriver>(
    session: &Session<D>,
) -> Option<crate::input::pointer::PointerRoute> {
    let mut route = crate::input::pointer::route_to_client(
        crate::input::pointer::PointerRoutingContext {
            space: &session.wayland.space,
            cameras: &session.cameras,
            clusters: &session.clusters,
            nodes: &session.nodes,
            window_animations: &session.window_animations,
            primary: session.driver.primary_output(),
            fullscreen: &session.fullscreen,
            maximize: &session.maximize,
            decorations: &session.settings.decorations,
            font: &session.settings.font,
            focused: session.wayland.focused_window.as_ref(),
            now: crate::frame_clock::monotonic_now(),
        },
        session.pointer.position(),
    )?;
    let output_geometry = session.wayland.space.output_geometry(&route.output)?;
    let camera = session.cameras.get(&route.output.name())?;
    let cluster_exclusive = crate::presentation::window::cluster_exclusive_presentation(
        &session.clusters,
        &session.nodes,
        &session.fullscreen,
        &session.maximize,
        &route.output,
        output_geometry,
        crate::frame_clock::monotonic_now(),
    )
    .is_some_and(|presentation| presentation.progress > 0.0);
    if !route.is_desktop_popup
        && !cluster_exclusive
        && session
            .nodes
            .hit_test(
                &route.output,
                output_geometry,
                camera,
                Point::from(session.pointer.position()),
            )
            .is_some()
    {
        let world = crate::input::grab::screen_to_world_on_output(
            session.pointer.position(),
            camera,
            output_geometry,
        );
        route.location = Point::from((world.x as f64, world.y as f64));
        route.focus = None;
        route.target = crate::input::pointer::PointerTarget::Background;
        route.visual_geometry = None;
        route.is_desktop_popup = false;
    }
    Some(route)
}

fn xwayland_relative_motion_allowed(
    routed_is_x11: bool,
    routed_is_immersive: bool,
    foreign_immersive_x11: bool,
) -> bool {
    // Xwayland exposes one relative-pointer object for the whole X server.
    // Forwarding relative motion while the pointer is on one X11 window
    // therefore becomes XI_RawMotion for every X11 client that selected it,
    // including an unfocused fullscreen game on another output. Absolute
    // wl_pointer motion still reaches the routed X11 window, so shield the
    // foreign immersive client without breaking ordinary Steam/UI input.
    !routed_is_x11 || routed_is_immersive || !foreign_immersive_x11
}

fn immersive_x11_window<D: SessionDriver>(
    session: &Session<D>,
    window: &smithay::desktop::Window,
) -> bool {
    crate::xwayland::is_x11(window)
        && (crate::xwayland::is_fullscreen(window)
            || window.wl_surface().is_some_and(|surface| {
                session
                    .fullscreen
                    .is_fullscreen_or_pending(surface.as_ref())
            }))
}

pub(super) fn relative_motion_allowed<D: SessionDriver>(
    session: &Session<D>,
    route: Option<&crate::input::pointer::PointerRoute>,
) -> bool {
    let Some(route) = route else {
        // PointerHandle::relative_motion ignores its supplied focus and uses
        // the handle's current focus. Do not let a failed hit test reuse a
        // stale client focus.
        return false;
    };
    let routed_window = match &route.target {
        crate::input::pointer::PointerTarget::Window(window)
        | crate::input::pointer::PointerTarget::Decoration { window, .. } => Some(window),
        crate::input::pointer::PointerTarget::Layer(_)
        | crate::input::pointer::PointerTarget::Background => None,
    };
    let routed_is_x11 = routed_window.is_some_and(crate::xwayland::is_x11);
    if !routed_is_x11 {
        return true;
    }
    let routed_is_immersive =
        routed_window.is_some_and(|window| immersive_x11_window(session, window));
    let foreign_immersive_x11 = session.wayland.space.elements().any(|window| {
        routed_window.is_none_or(|routed| routed != window) && immersive_x11_window(session, window)
    });
    xwayland_relative_motion_allowed(routed_is_x11, routed_is_immersive, foreign_immersive_x11)
}

/// Xwayland coalesces `wl_pointer.motion` and relative-pointer motion until the
/// pointer frame. When absolute motion arrives alone it emits XI_RawMotion from
/// its absolute device; Wine games can consume that even while another X11
/// window owns focus. Pair the absolute update with a zero relative delta so
/// Xwayland marks the absolute half `POINTER_NORAW` while Steam still receives
/// its normal core pointer motion.
fn pair_xwayland_ui_absolute_motion<D: SessionDriver>(
    session: &mut Session<D>,
    pointer: &PointerHandle<Session<D>>,
    route: &crate::input::pointer::PointerRoute,
    time: u32,
) {
    if relative_motion_allowed(session, Some(route)) {
        return;
    }
    let Some(focus) = route.focus.clone() else {
        return;
    };
    pointer.relative_motion(
        session,
        Some(focus),
        &RelativeMotionEvent {
            delta: Point::from((0.0, 0.0)),
            delta_unaccel: Point::from((0.0, 0.0)),
            utime: u64::from(time) * 1_000,
        },
    );
}

pub(super) fn has_active_constraint<D: SessionDriver>(session: &Session<D>) -> bool {
    session
        .seat
        .get_pointer()
        .is_some_and(|pointer| constraints::active(session, &pointer).is_some())
}

pub(super) fn has_active_confinement<D: SessionDriver>(session: &Session<D>) -> bool {
    session
        .seat
        .get_pointer()
        .is_some_and(|pointer| constraints::has_active_confinement(session, &pointer))
}

pub(super) fn constraint_diagnostic<D: SessionDriver>(
    session: &Session<D>,
    surface: &WlSurface,
) -> ConstraintDiagnostic {
    let Some(pointer) = session.seat.get_pointer() else {
        return ConstraintDiagnostic {
            protocol_resource: None,
            protocol_kind: None,
            protocol_active: false,
            halley_tracked: false,
            tracked_kind: None,
            tracked_surface_id: None,
        };
    };
    constraints::diagnostic(session, &pointer, surface)
}

fn constraint_requires_stable_presentation(kind: Option<constraints::ConstraintKind>) -> bool {
    kind != Some(constraints::ConstraintKind::Locked)
}

pub(super) fn new_constraint_requires_stable_presentation<D: SessionDriver>(
    surface: &WlSurface,
    pointer: &PointerHandle<Session<D>>,
) -> bool {
    constraint_requires_stable_presentation(constraints::constraint_kind(surface, pointer))
}

fn route_and_update_client_focus<D: SessionDriver>(
    session: &mut Session<D>,
    time: u32,
    policy: AbsoluteMotionPolicy,
) -> Option<crate::input::pointer::PointerRoute> {
    let route = route_client(session)?;
    let pointer = session
        .seat
        .get_pointer()
        .expect("pointer capability added at seat setup");
    let routed_surface = route.focus.as_ref().map(|(surface, _)| surface);
    let surface_changed = pointer.current_focus().as_ref() != routed_surface;
    let routed_state = ClientPointerRoute::from_route(&route);
    let routed_state_changed =
        session.interactions.client_pointer_route.as_ref() != Some(&routed_state);
    if should_emit_absolute_motion(
        policy,
        surface_changed,
        routed_state_changed,
        constraints::has_active_lock(session, &pointer),
    ) {
        if surface_changed {
            reset_client_cursor_image(session);
            constraints::deactivate_before_pointer_focus_change(session, routed_surface);
        }
        pointer.motion(
            session,
            route.focus.clone(),
            &MotionEvent {
                location: route.location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pair_xwayland_ui_absolute_motion(session, &pointer, &route, time);
        session.interactions.client_pointer_route = Some(routed_state);
    }
    Some(route)
}

fn reset_client_cursor_image<D: SessionDriver>(session: &mut Session<D>) {
    if let Some(previous) = session.cursor.reset_image() {
        crate::cursor::surface::clear_outputs(&previous, &session.wayland.space);
    }
    crate::cursor::surface::refresh_outputs(
        &session.cursor,
        &session.wayland.space,
        session.pointer.position(),
    );
}

pub(super) fn route_for_motion<D: SessionDriver>(
    session: &mut Session<D>,
    time: u32,
) -> Option<crate::input::pointer::PointerRoute> {
    route_and_update_client_focus(session, time, AbsoluteMotionPolicy::Position)
}

pub(super) fn route_for_discrete_input<D: SessionDriver>(
    session: &mut Session<D>,
    time: u32,
) -> Option<crate::input::pointer::PointerRoute> {
    route_and_update_client_focus(session, time, AbsoluteMotionPolicy::Surface)
}

pub(super) fn finish_frame<D: SessionDriver>(
    session: &mut Session<D>,
    pointer: &PointerHandle<Session<D>>,
) {
    pointer.frame(session);
    // Reconcile only after the current event batch is complete so an input
    // event cannot straddle the old and new constraint owners.
    constraints::reconcile(session, pointer, None);
    pointer.frame(session);
}

pub(super) fn update_client_state<D: SessionDriver>(session: &mut Session<D>, time: u32) {
    let pointer = session
        .seat
        .get_pointer()
        .expect("pointer capability added at seat setup");
    route_for_motion(session, time);
    finish_frame(session, &pointer);
}

/// Re-evaluates presentation-driven pointer focus without inventing physical
/// mouse motion. A moving cluster card may enter or leave the stationary
/// pointer, but changing its visual transform must not stream new absolute
/// coordinates to the same client every frame.
pub(super) fn refresh_client_focus<D: SessionDriver>(session: &mut Session<D>, time: u32) {
    let pointer = session
        .seat
        .get_pointer()
        .expect("pointer capability added at seat setup");
    route_for_discrete_input(session, time);
    finish_frame(session, &pointer);
}

/// Refreshes stationary-pointer routing for ordinary desktop surfaces.
///
/// Surface commits and X11 popup geometry changes can move client contents
/// without a physical input event. This path deliberately excludes every
/// immersive or grabbed route: games keep their existing relative-motion,
/// pointer-lock, confinement, and fullscreen behavior.
pub(crate) fn refresh_desktop_client_focus<D: SessionDriver>(session: &mut Session<D>, time: u32) {
    let pointer = session
        .seat
        .get_pointer()
        .expect("pointer capability added at seat setup");
    let Some(route) = route_client(session) else {
        return;
    };
    // Only the fullscreen client itself must be shielded: injecting a
    // stationary-pointer motion into a game replaces XWayland's emulated cursor
    // anchor and breaks relative look. A window that merely shares the output
    // with a fullscreen game still needs re-routing — an unrelated window going
    // stale whenever a game is running is exactly the reported symptom.
    let fullscreen_active = match &route.target {
        crate::input::pointer::PointerTarget::Window(window)
        | crate::input::pointer::PointerTarget::Decoration { window, .. } => {
            window.wl_surface().is_some_and(|surface| {
                session.fullscreen.is_fullscreen_or_pending(&surface)
                    || session.fullscreen.awaits_external_configure(&surface)
            }) || crate::xwayland::is_fullscreen(window)
        }
        crate::input::pointer::PointerTarget::Layer(_)
        | crate::input::pointer::PointerTarget::Background => session
            .fullscreen
            .has_fullscreen_activity_on_output_matching(&route.output, |surface| {
                crate::presentation::surface_workspace_is_active(
                    &session.clusters,
                    &session.nodes,
                    surface,
                    &route.output.name(),
                    crate::frame_clock::monotonic_now(),
                )
            }),
    };
    let blockers = DesktopRefreshBlockers {
        pointer_grabbed: pointer.is_grabbed(),
        constraint_active: has_active_constraint(session),
        compositor_grab_active: !matches!(
            session.interactions.grab,
            crate::input::grab::Grab::None
        ),
        capture_active: session.capture.is_active(),
        shell_active: session.shell.apogee.is_active(),
        focus_cycle_open: session.shell.focus_cycle.is_open(),
        session_locked: session.session_lock.active(),
        fullscreen_active,
    };
    if !desktop_refresh_allowed(blockers) {
        return;
    }

    let routed_state = ClientPointerRoute::from_route(&route);
    let routed_surface = route.focus.as_ref().map(|(surface, _)| surface);
    let surface_changed = pointer.current_focus().as_ref() != routed_surface;
    let routed_state_changed =
        session.interactions.client_pointer_route.as_ref() != Some(&routed_state);
    if !should_emit_absolute_motion(
        AbsoluteMotionPolicy::RoutedState,
        surface_changed,
        routed_state_changed,
        false,
    ) {
        return;
    }
    if surface_changed {
        reset_client_cursor_image(session);
        constraints::deactivate_before_pointer_focus_change(session, routed_surface);
    }
    pointer.motion(
        session,
        route.focus.clone(),
        &MotionEvent {
            location: route.location,
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    pair_xwayland_ui_absolute_motion(session, &pointer, &route, time);
    session.interactions.client_pointer_route = Some(routed_state);
    finish_frame(session, &pointer);
}

pub(crate) fn release_for_compositor_warp<D: SessionDriver>(session: &mut Session<D>) {
    constraints::deactivate_before_pointer_focus_change(session, None);
}

pub(crate) fn recover_after_session_resume<D: SessionDriver>(session: &mut Session<D>) {
    constraints::deactivate_before_pointer_focus_change(session, None);
    reset_client_cursor_image(session);
    session.cursor.clear_overrides();
    session.cursor_policy.pointer_activity();
    session.request_redraw();
}

pub(super) fn reconcile_state<D: SessionDriver>(session: &mut Session<D>) {
    let Some(pointer) = session.seat.get_pointer() else {
        return;
    };
    constraints::reconcile(session, &pointer, None);
    pointer.frame(session);
}

pub(super) enum ConstrainedMotion {
    Apply,
    /// Screen position the confinement projects the proposed position onto.
    Clamp(Point<f64, Logical>),
    RelativeOnly {
        surface: WlSurface,
        origin: Point<f64, Logical>,
    },
}

pub(super) fn constrain_motion<D: SessionDriver>(
    session: &mut Session<D>,
    pointer: &PointerHandle<Session<D>>,
) -> ConstrainedMotion {
    constraints::reconcile(session, pointer, None);
    if constraints::enforcement_suspended(session, pointer) {
        return ConstrainedMotion::Apply;
    }
    let active = constraints::active(session, pointer);
    let confined_correction = active
        .as_ref()
        .filter(|constraint| constraint.kind == constraints::ConstraintKind::Confined)
        .and_then(|constraint| constraints::confined_position(session, constraint));
    match constraints::motion_disposition(
        active.as_ref().map(|constraint| constraint.kind),
        confined_correction,
    ) {
        constraints::MotionDisposition::Apply => ConstrainedMotion::Apply,
        constraints::MotionDisposition::Clamp(position) => ConstrainedMotion::Clamp(position),
        constraints::MotionDisposition::RelativeOnly => {
            let constraint = active
                .as_ref()
                .expect("relative-only motion requires an active lock");
            ConstrainedMotion::RelativeOnly {
                surface: constraint.surface.clone(),
                origin: constraint.origin,
            }
        }
    }
}

pub(super) fn constraint_suspended_for_grab<D: SessionDriver>(session: &Session<D>) -> bool {
    session
        .seat
        .get_pointer()
        .is_some_and(|pointer| constraints::enforcement_suspended(session, &pointer))
}

pub(super) fn cursor_visible<D: SessionDriver>(session: &Session<D>) -> bool {
    if interactive_overlay_forces_cursor(
        session.shell.apogee.is_active(),
        session.capture.is_active(),
        session.clusters.accepts_modal_input(),
    ) {
        return true;
    }
    cursor_presentation_visible(
        session.cursor_policy.visible(),
        constraints::cursor_visible(session),
    )
}

pub(super) fn cursor_override<D: SessionDriver>(
    session: &Session<D>,
) -> Option<smithay::input::pointer::CursorIcon> {
    interactive_overlay_cursor_override(
        session.capture.is_active(),
        session.clusters.accepts_modal_input(),
    )
}

fn interactive_overlay_cursor_override(
    capture_active: bool,
    cluster_creation_active: bool,
) -> Option<smithay::input::pointer::CursorIcon> {
    (capture_active || cluster_creation_active)
        .then_some(smithay::input::pointer::CursorIcon::Default)
}

fn interactive_overlay_forces_cursor(
    apogee_active: bool,
    capture_active: bool,
    cluster_creation_active: bool,
) -> bool {
    apogee_active || capture_active || cluster_creation_active
}

fn cursor_presentation_visible(policy_visible: bool, constraint_visible: bool) -> bool {
    policy_visible && constraint_visible
}

pub(super) fn prepare_keyboard_focus_change<D: SessionDriver>(
    session: &mut Session<D>,
    next_focused_root: Option<&WlSurface>,
) {
    constraints::deactivate_before_focus_change(session, next_focused_root);
}

pub(super) fn prepare_unmap<D: SessionDriver>(session: &mut Session<D>, root: &WlSurface) {
    constraints::deactivate_before_unmap(session, root);
}

fn should_refresh_constraint_focus(
    pointer_grabbed: bool,
    active_constraint: bool,
    route_matches_request: bool,
    pointer_matches_route: bool,
) -> bool {
    !pointer_grabbed && !active_constraint && route_matches_request && !pointer_matches_route
}

/// Match niri and old Halley: when a new constraint arrives, refresh the
/// stationary pointer from a fresh hit-test before deciding whether the
/// requesting surface owns pointer focus. This repairs stale focus without
/// letting an arbitrary client pull focus to itself.
fn refresh_new_constraint_focus<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &WlSurface,
    pointer: &PointerHandle<Session<D>>,
) {
    let Some(route) = route_client(session) else {
        return;
    };
    let routed_surface = route.focus.as_ref().map(|(surface, _)| surface);
    let route_matches_request = routed_surface == Some(surface);
    let pointer_matches_route = pointer.current_focus().as_ref() == routed_surface;
    if !should_refresh_constraint_focus(
        pointer.is_grabbed(),
        has_active_constraint(session),
        route_matches_request,
        pointer_matches_route,
    ) {
        return;
    }

    let time = session.start_time.elapsed().as_millis() as u32;
    reset_client_cursor_image(session);
    constraints::deactivate_before_pointer_focus_change(session, routed_surface);
    pointer.motion(
        session,
        route.focus.clone(),
        &MotionEvent {
            location: route.location,
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    pair_xwayland_ui_absolute_motion(session, pointer, &route, time);
    session.interactions.client_pointer_route = Some(ClientPointerRoute::from_route(&route));
    eventline::debug!("pointer-constraint: refreshed focus from fresh route surface={surface:?}");
}

pub(super) fn activate_new<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &WlSurface,
    pointer: &PointerHandle<Session<D>>,
) {
    refresh_new_constraint_focus(session, surface, pointer);
    constraints::reconcile(session, pointer, Some(surface));
    pointer.frame(session);
}

pub(super) fn apply_position_hint<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &WlSurface,
    pointer: &PointerHandle<Session<D>>,
    location: Point<f64, Logical>,
) {
    constraints::apply_position_hint(session, surface, pointer, location);
}

#[cfg(test)]
mod tests {
    use super::{
        AbsoluteMotionPolicy, DesktopRefreshBlockers, constraint_requires_stable_presentation,
        constraints, cursor_presentation_visible, desktop_refresh_allowed,
        interactive_overlay_cursor_override, interactive_overlay_forces_cursor,
        should_emit_absolute_motion, should_refresh_constraint_focus,
        xwayland_relative_motion_allowed,
    };

    #[test]
    fn interactive_overlays_force_the_cursor_visible() {
        assert!(interactive_overlay_forces_cursor(true, false, false));
        assert!(interactive_overlay_forces_cursor(false, true, false));
        assert!(interactive_overlay_forces_cursor(false, false, true));
        assert!(!interactive_overlay_forces_cursor(false, false, false));
    }

    #[test]
    fn interactive_text_overlay_replaces_a_hidden_client_cursor_image() {
        assert_eq!(
            interactive_overlay_cursor_override(false, true),
            Some(smithay::input::pointer::CursorIcon::Default)
        );
        assert_eq!(interactive_overlay_cursor_override(false, false), None);
    }

    #[test]
    fn locked_pointer_suppresses_every_absolute_motion_policy() {
        assert!(!should_emit_absolute_motion(
            AbsoluteMotionPolicy::Position,
            true,
            true,
            true
        ));
        assert!(!should_emit_absolute_motion(
            AbsoluteMotionPolicy::Surface,
            true,
            true,
            true
        ));
    }

    #[test]
    fn discrete_input_only_refreshes_a_changed_unlocked_surface() {
        assert!(!should_emit_absolute_motion(
            AbsoluteMotionPolicy::Surface,
            false,
            true,
            false
        ));
        assert!(should_emit_absolute_motion(
            AbsoluteMotionPolicy::Surface,
            true,
            true,
            false
        ));
    }

    #[test]
    fn physical_motion_updates_an_unlocked_pointer_position() {
        assert!(should_emit_absolute_motion(
            AbsoluteMotionPolicy::Position,
            false,
            false,
            false
        ));
    }

    #[test]
    fn foreign_fullscreen_x11_client_uses_a_zero_relative_pair_for_ui_motion() {
        assert!(!xwayland_relative_motion_allowed(true, false, true));
        assert!(xwayland_relative_motion_allowed(true, true, true));
        assert!(xwayland_relative_motion_allowed(true, false, false));
        assert!(xwayland_relative_motion_allowed(false, false, true));
    }

    #[test]
    fn desktop_refresh_only_emits_when_the_routed_state_changes() {
        assert!(!should_emit_absolute_motion(
            AbsoluteMotionPolicy::RoutedState,
            false,
            false,
            false,
        ));
        assert!(should_emit_absolute_motion(
            AbsoluteMotionPolicy::RoutedState,
            false,
            true,
            false,
        ));
        assert!(!should_emit_absolute_motion(
            AbsoluteMotionPolicy::RoutedState,
            false,
            true,
            true,
        ));
    }

    #[test]
    fn new_constraint_refreshes_only_a_stale_matching_route() {
        assert!(should_refresh_constraint_focus(false, false, true, false));
        assert!(!should_refresh_constraint_focus(false, false, true, true));
        assert!(!should_refresh_constraint_focus(false, false, false, false));
        assert!(!should_refresh_constraint_focus(true, false, true, false));
        assert!(!should_refresh_constraint_focus(false, true, true, false));
    }

    #[test]
    fn desktop_refresh_excludes_constraints_grabs_and_fullscreen() {
        assert!(desktop_refresh_allowed(DesktopRefreshBlockers::default()));
        for blockers in [
            DesktopRefreshBlockers {
                pointer_grabbed: true,
                ..Default::default()
            },
            DesktopRefreshBlockers {
                constraint_active: true,
                ..Default::default()
            },
            DesktopRefreshBlockers {
                compositor_grab_active: true,
                ..Default::default()
            },
            DesktopRefreshBlockers {
                fullscreen_active: true,
                ..Default::default()
            },
        ] {
            assert!(!desktop_refresh_allowed(blockers));
        }
    }

    #[test]
    fn locks_allow_visual_motion_while_confinement_requires_stable_geometry() {
        assert!(!constraint_requires_stable_presentation(Some(
            constraints::ConstraintKind::Locked
        )));
        assert!(constraint_requires_stable_presentation(Some(
            constraints::ConstraintKind::Confined
        )));
        assert!(constraint_requires_stable_presentation(None));
    }

    #[test]
    fn policy_hiding_and_pointer_lock_independently_mask_presentation() {
        assert!(cursor_presentation_visible(true, true));
        assert!(!cursor_presentation_visible(false, true));
        assert!(!cursor_presentation_visible(true, false));
        assert!(!cursor_presentation_visible(false, false));
    }
}
