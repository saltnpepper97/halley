use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::input::pointer::{MotionEvent, PointerHandle};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Point, Rectangle, SERIAL_COUNTER, Size};
use smithay::wayland::compositor::{RegionAttributes, SurfaceAttributes, with_states};
use smithay::wayland::pointer_constraints::{PointerConstraint, with_pointer_constraint};
use smithay::wayland::seat::WaylandFocus;

use crate::presentation::window::WindowPresentation;
use crate::session::{Session, SessionDriver};

use super::ConstraintDiagnostic;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConstraintKind {
    Confined,
    Locked,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MotionDisposition {
    Apply,
    /// The proposed position left the confinement; move to this screen
    /// position instead. Clamping rather than holding is what lets motion
    /// along a boundary keep its tangential component.
    Clamp(Point<f64, Logical>),
    RelativeOnly,
}

pub(super) fn motion_disposition(
    kind: Option<ConstraintKind>,
    confined_correction: Option<Point<f64, Logical>>,
) -> MotionDisposition {
    match kind {
        Some(ConstraintKind::Locked) => MotionDisposition::RelativeOnly,
        Some(ConstraintKind::Confined) => match confined_correction {
            Some(position) => MotionDisposition::Clamp(position),
            None => MotionDisposition::Apply,
        },
        None => MotionDisposition::Apply,
    }
}

#[derive(Clone)]
struct TrackedConstraint {
    surface: WlSurface,
    kind: ConstraintKind,
    position_hint: Option<Point<f64, Logical>>,
    geometry: ConstraintGeometry,
}

#[derive(Clone)]
struct ConstraintGeometry {
    origin: Point<f64, Logical>,
    surface_size: Size<i32, Logical>,
    region: Option<RegionAttributes>,
    input_region: Option<RegionAttributes>,
    presentation: WindowPresentation,
}

/// Halley's single record of the protocol constraint currently allowed to
/// affect input. Protocol resources remain owned by Smithay; every transition
/// into or out of their active state is serialized through `reconcile`.
#[derive(Default)]
pub struct PointerConstraintLifecycle {
    active: Option<TrackedConstraint>,
    suspended: Option<SuspendedConstraint>,
}

#[derive(Clone)]
struct SuspendedConstraint {
    surface: WlSurface,
    kind: ConstraintKind,
    lock_anchor: Option<Point<f64, Logical>>,
}

#[derive(Clone)]
struct ConstraintDescriptor {
    kind: ConstraintKind,
    active: bool,
    region: Option<RegionAttributes>,
    position_hint: Option<Point<f64, Logical>>,
}

struct OwnerContext {
    presentation: WindowPresentation,
    surface_size: Size<i32, Logical>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReconcileDecision {
    deactivate_current: bool,
    activate_candidate: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeactivationReason {
    SurfaceDestroyed,
    OwnerUnmapped,
    CompositorFocusChanged,
    KeyboardFocusChanged,
    PointerFocusChanged,
    ConstraintRemoved,
    ProtocolDeactivated,
    ConstraintKindChanged,
    CandidateChanged,
    RequestedKeyboardFocusChange,
    RequestedPointerFocusChange,
    RequestedOwnerUnmap,
    EffectiveRegionEmpty,
}

fn reconcile_decision(
    current_present: bool,
    current_is_candidate: bool,
    current_valid: bool,
    candidate_present: bool,
    candidate_protocol_active: bool,
) -> ReconcileDecision {
    ReconcileDecision {
        deactivate_current: current_present && (!current_is_candidate || !current_valid),
        activate_candidate: candidate_present && !candidate_protocol_active,
    }
}

fn preferred_matches_focus(preferred_matches: Option<bool>) -> bool {
    preferred_matches.unwrap_or(true)
}

fn retain_cached_geometry(current_valid: bool, current_is_candidate: bool) -> bool {
    current_valid && current_is_candidate
}

fn retain_while_unfocused_request_waits(
    current_valid: bool,
    preferred_present: bool,
    candidate_present: bool,
) -> bool {
    current_valid && preferred_present && !candidate_present
}

fn descriptor<D: SessionDriver>(
    surface: &WlSurface,
    pointer: &PointerHandle<Session<D>>,
) -> Option<ConstraintDescriptor> {
    with_pointer_constraint(surface, pointer, |constraint| {
        let constraint = constraint?;
        let (kind, position_hint) = match &*constraint {
            PointerConstraint::Confined(_) => (ConstraintKind::Confined, None),
            PointerConstraint::Locked(locked) => {
                (ConstraintKind::Locked, locked.cursor_position_hint())
            }
        };
        Some(ConstraintDescriptor {
            kind,
            active: constraint.is_active(),
            region: constraint.region().cloned(),
            position_hint,
        })
    })
}

pub(super) fn diagnostic<D: SessionDriver>(
    session: &Session<D>,
    pointer: &PointerHandle<Session<D>>,
    surface: &WlSurface,
) -> ConstraintDiagnostic {
    let protocol = with_pointer_constraint(surface, pointer, |constraint| {
        constraint.map(|constraint| {
            let kind = match &*constraint {
                PointerConstraint::Confined(_) => "confined",
                PointerConstraint::Locked(_) => "locked",
            };
            let constraint_data: &PointerConstraint = &constraint;
            let debug = format!("{constraint_data:?}");
            let resource = debug
                .split(", region:")
                .next()
                .unwrap_or(debug.as_str())
                .to_string();
            (resource, kind, constraint.is_active())
        })
    });
    let root = crate::wayland::compositor::root_surface(surface);
    let tracked = session
        .interactions
        .pointer_constraints
        .active
        .as_ref()
        .filter(|tracked| crate::wayland::compositor::root_surface(&tracked.surface) == root);
    ConstraintDiagnostic {
        protocol_resource: protocol.as_ref().map(|(resource, _, _)| resource.clone()),
        protocol_kind: protocol.as_ref().map(|(_, kind, _)| *kind),
        protocol_active: protocol.is_some_and(|(_, _, active)| active),
        halley_tracked: tracked.is_some(),
        tracked_kind: tracked.map(|tracked| match tracked.kind {
            ConstraintKind::Confined => "confined",
            ConstraintKind::Locked => "locked",
        }),
        tracked_surface_id: tracked.map(|tracked| tracked.surface.id().protocol_id()),
    }
}

pub(super) fn constraint_kind<D: SessionDriver>(
    surface: &WlSurface,
    pointer: &PointerHandle<Session<D>>,
) -> Option<ConstraintKind> {
    descriptor(surface, pointer).map(|constraint| constraint.kind)
}

fn surface_size(surface: &WlSurface) -> Option<Size<i32, Logical>> {
    with_renderer_surface_state(surface, |state| state.surface_size()).flatten()
}

fn surface_input_region(surface: &WlSurface) -> Option<RegionAttributes> {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceAttributes>()
            .current()
            .input_region
            .clone()
    })
}

fn owner_context<D: SessionDriver>(
    session: &Session<D>,
    surface: &WlSurface,
) -> Option<OwnerContext> {
    if !owner_is_authoritative(session, surface) {
        return None;
    }
    let presentation = WindowPresentation::for_surface(
        &session.wayland.space,
        &session.cameras,
        Some(&session.clusters),
        Some(&session.nodes),
        session.driver.primary_output(),
        &session.window_animations,
        &session.fullscreen,
        &session.maximize,
        &session.settings.decorations,
        &session.settings.font,
        surface,
        crate::frame_clock::monotonic_now(),
    )?;
    Some(OwnerContext {
        presentation,
        surface_size: surface_size(surface)?,
    })
}

fn constraint_authority_root<D: SessionDriver>(
    session: &Session<D>,
    surface: &WlSurface,
) -> WlSurface {
    crate::xwayland::pointer_constraint_proxy_authority(&session.wayland.space, surface)
        .unwrap_or_else(|| crate::wayland::compositor::root_surface(surface))
}

fn owner_state_loss(
    alive: bool,
    mapped: bool,
    wm_focus_exempt: bool,
    compositor_focused: bool,
    keyboard_focused: bool,
) -> Option<DeactivationReason> {
    if !alive {
        Some(DeactivationReason::SurfaceDestroyed)
    } else if !mapped {
        Some(DeactivationReason::OwnerUnmapped)
    } else if wm_focus_exempt {
        None
    } else if !compositor_focused {
        Some(DeactivationReason::CompositorFocusChanged)
    } else if !keyboard_focused {
        Some(DeactivationReason::KeyboardFocusChanged)
    } else {
        None
    }
}

fn owner_authority_loss<D: SessionDriver>(
    session: &Session<D>,
    surface: &WlSurface,
) -> Option<DeactivationReason> {
    let root = crate::wayland::compositor::root_surface(surface);
    let mapped = session.wayland.space.elements().any(|window| {
        window
            .wl_surface()
            .is_some_and(|candidate| candidate.as_ref() == &root)
    });
    // An XWayland grab proxy deliberately keeps WM and keyboard focus on its
    // managed owner. Pointer focus plus the explicit focus-change lifecycle
    // below are authoritative for this one surface class.
    let wm_focus_exempt =
        crate::xwayland::pointer_constraint_proxy_authority(&session.wayland.space, surface)
            .is_some();
    let compositor_focused = session.wayland.focused_window.as_ref() == Some(&root);
    let keyboard_root = session
        .seat
        .get_keyboard()
        .and_then(|keyboard| keyboard.current_focus())
        .and_then(|focus| focus.wl_surface().map(|surface| surface.into_owned()))
        .map(|surface| crate::wayland::compositor::root_surface(&surface));
    owner_state_loss(
        surface.alive(),
        mapped,
        wm_focus_exempt,
        compositor_focused,
        keyboard_root.as_ref() == Some(&root),
    )
}

fn owner_is_authoritative<D: SessionDriver>(session: &Session<D>, surface: &WlSurface) -> bool {
    owner_authority_loss(session, surface).is_none()
}

fn grab_owns_surface<D: SessionDriver>(session: &Session<D>, surface: &WlSurface) -> bool {
    crate::input::grab::belongs_to_surface(&session.interactions.grab, surface)
}

fn sync_grab_suspension<D: SessionDriver>(
    session: &mut Session<D>,
    pointer: &PointerHandle<Session<D>>,
) {
    let tracked = session.interactions.pointer_constraints.active.clone();
    let should_suspend = tracked
        .as_ref()
        .is_some_and(|tracked| grab_owns_surface(session, &tracked.surface));
    if should_suspend {
        let tracked = tracked.expect("suspension requires a tracked constraint");
        let already_suspended = session
            .interactions
            .pointer_constraints
            .suspended
            .as_ref()
            .is_some_and(|suspended| suspended.surface == tracked.surface);
        if !already_suspended {
            let screen = Point::from(session.pointer.position());
            let lock_anchor = (tracked.kind == ConstraintKind::Locked)
                .then(|| {
                    tracked
                        .geometry
                        .presentation
                        .surface_from_screen(&tracked.surface, screen)
                })
                .flatten();
            session.interactions.pointer_constraints.suspended = Some(SuspendedConstraint {
                surface: tracked.surface,
                kind: tracked.kind,
                lock_anchor,
            });
        }
        return;
    }

    let Some(suspended) = session.interactions.pointer_constraints.suspended.take() else {
        return;
    };
    let Some(tracked) = tracked
        .filter(|tracked| tracked.surface == suspended.surface && tracked.kind == suspended.kind)
    else {
        return;
    };
    if suspended.kind == ConstraintKind::Locked
        && let Some(anchor) = suspended.lock_anchor
        && let Some(screen) = tracked
            .geometry
            .presentation
            .screen_from_surface(&tracked.surface, anchor)
        && let Some(screen) = session
            .pointer
            .set_position_on_outputs(screen, &session.wayland.space)
        && let Some(local) = tracked
            .geometry
            .presentation
            .surface_from_screen(&tracked.surface, screen)
    {
        pointer.set_location(tracked.geometry.origin + local);
    }
}

pub(super) fn enforcement_suspended<D: SessionDriver>(
    session: &Session<D>,
    pointer: &PointerHandle<Session<D>>,
) -> bool {
    let Some(tracked) = session.interactions.pointer_constraints.active.as_ref() else {
        return false;
    };
    valid_active_owner(session, pointer, tracked)
        && session
            .interactions
            .pointer_constraints
            .suspended
            .as_ref()
            .is_some_and(|suspended| suspended.surface == tracked.surface)
}

fn constraint_geometry(
    surface: &WlSurface,
    context: OwnerContext,
    constraint: &ConstraintDescriptor,
) -> Option<ConstraintGeometry> {
    Some(ConstraintGeometry {
        origin: context.presentation.surface_origin(surface)?,
        surface_size: context.surface_size,
        region: constraint.region.clone(),
        input_region: surface_input_region(surface),
        presentation: context.presentation,
    })
}

fn active_owner_loss<D: SessionDriver>(
    session: &Session<D>,
    pointer: &PointerHandle<Session<D>>,
    tracked: &TrackedConstraint,
) -> Option<DeactivationReason> {
    if pointer.current_focus().as_ref() != Some(&tracked.surface) {
        return Some(DeactivationReason::PointerFocusChanged);
    }
    if let Some(reason) = owner_authority_loss(session, &tracked.surface) {
        return Some(reason);
    }
    let Some(constraint) = descriptor(&tracked.surface, pointer) else {
        return Some(DeactivationReason::ConstraintRemoved);
    };
    if !constraint.active {
        return Some(DeactivationReason::ProtocolDeactivated);
    }
    if constraint.kind != tracked.kind {
        return Some(DeactivationReason::ConstraintKindChanged);
    }
    None
}

fn valid_active_owner<D: SessionDriver>(
    session: &Session<D>,
    pointer: &PointerHandle<Session<D>>,
    tracked: &TrackedConstraint,
) -> bool {
    active_owner_loss(session, pointer, tracked).is_none()
}

fn deactivate_protocol<D: SessionDriver>(
    surface: &WlSurface,
    pointer: &PointerHandle<Session<D>>,
    kind: ConstraintKind,
) {
    with_pointer_constraint(surface, pointer, |constraint| {
        if let Some(constraint) = constraint
            && constraint_kind_of(&constraint) == kind
        {
            constraint.deactivate();
        }
    });
}

fn constraint_kind_of(constraint: &PointerConstraint) -> ConstraintKind {
    match constraint {
        PointerConstraint::Confined(_) => ConstraintKind::Confined,
        PointerConstraint::Locked(_) => ConstraintKind::Locked,
    }
}

fn deactivation_applies_position_hint(reason: DeactivationReason) -> bool {
    // A keyboard focus change already has a compositor-selected destination.
    // Letting the old client move the pointer during that handoff creates an
    // intermediate Xwayland anchor which the unfocused game can consume when
    // its lock is released.
    reason != DeactivationReason::RequestedKeyboardFocusChange
}

fn deactivate_tracked<D: SessionDriver>(
    session: &mut Session<D>,
    pointer: &PointerHandle<Session<D>>,
    tracked: &TrackedConstraint,
    reason: DeactivationReason,
) {
    crate::session::trace::surface_event(
        session,
        &tracked.surface,
        "constraint-deactivate",
        format_args!("kind={:?} reason={:?}", tracked.kind, reason),
    );
    eventline::debug!(
        "pointer-constraint: deactivate kind={:?} surface={:?} reason={:?}",
        tracked.kind,
        tracked.surface,
        reason,
    );
    let state = descriptor(&tracked.surface, pointer);
    // A focus switch already has a compositor-selected destination. Applying
    // the old client's unlock hint first produces an intermediate cursor snap
    // and can rebuild XWayland capture around the stale anchor.
    let position_hint = deactivation_applies_position_hint(reason)
        .then(|| {
            tracked
                .position_hint
                .or_else(|| state.as_ref().and_then(|state| state.position_hint))
        })
        .flatten();
    if let Some(position_hint) = position_hint {
        let geometry = owner_context(session, &tracked.surface)
            .and_then(|context| {
                let descriptor = state.as_ref()?;
                constraint_geometry(&tracked.surface, context, descriptor)
            })
            .unwrap_or_else(|| tracked.geometry.clone());
        if let Some(local) = nearest_valid_point(
            position_hint,
            geometry.surface_size,
            state
                .as_ref()
                .and_then(|state| state.region.as_ref())
                .or(geometry.region.as_ref()),
            surface_input_region(&tracked.surface)
                .as_ref()
                .or(geometry.input_region.as_ref()),
            None,
        ) && let Some(screen) = geometry
            .presentation
            .screen_from_surface(&tracked.surface, local)
        {
            session
                .pointer
                .set_position_on_outputs(screen, &session.wayland.space);
        }
    }
    deactivate_protocol(&tracked.surface, pointer, tracked.kind);
}

fn activate<D: SessionDriver>(surface: &WlSurface, pointer: &PointerHandle<Session<D>>) -> bool {
    with_pointer_constraint(surface, pointer, |constraint| {
        let Some(constraint) = constraint else {
            return false;
        };
        constraint.activate();
        true
    })
}

fn candidate_surface<D: SessionDriver>(
    session: &Session<D>,
    pointer: &PointerHandle<Session<D>>,
    preferred: Option<&WlSurface>,
) -> Option<WlSurface> {
    let focused = pointer.current_focus()?;
    if !preferred_matches_focus(preferred.map(|surface| surface == &focused)) {
        return None;
    }
    (owner_is_authoritative(session, &focused) && descriptor(&focused, pointer).is_some())
        .then_some(focused)
}

fn nearest_valid_point(
    desired: Point<f64, Logical>,
    surface_size: Size<i32, Logical>,
    region: Option<&RegionAttributes>,
    input_region: Option<&RegionAttributes>,
    visible_bounds: Option<Rectangle<f64, Logical>>,
) -> Option<Point<f64, Logical>> {
    if surface_size.w <= 0 || surface_size.h <= 0 {
        return None;
    }
    let bounds = Rectangle::from_size(surface_size).to_f64();
    let bounds = match visible_bounds {
        Some(visible) => bounds.intersection(visible)?,
        None => bounds,
    };
    if bounds.size.w <= 0.0 || bounds.size.h <= 0.0 {
        return None;
    }
    let allowed = |point: Point<f64, Logical>| {
        bounds.contains(point)
            && region.is_none_or(|region| region.contains(point.to_i32_floor()))
            && input_region.is_none_or(|region| region.contains(point.to_i32_floor()))
    };
    let rounded: Point<i32, Logical> = desired.to_i32_round();
    if allowed(desired) {
        return Some(desired);
    }

    let mut xs = vec![
        0,
        surface_size.w - 1,
        rounded.x.clamp(0, surface_size.w - 1),
    ];
    let mut ys = vec![
        0,
        surface_size.h - 1,
        rounded.y.clamp(0, surface_size.h - 1),
    ];
    if let Some(region) = region {
        for (_, rect) in &region.rects {
            for x in [
                rect.loc.x - 1,
                rect.loc.x,
                rect.loc.x + rect.size.w - 1,
                rect.loc.x + rect.size.w,
            ] {
                xs.push(x.clamp(0, surface_size.w - 1));
            }
            for y in [
                rect.loc.y - 1,
                rect.loc.y,
                rect.loc.y + rect.size.h - 1,
                rect.loc.y + rect.size.h,
            ] {
                ys.push(y.clamp(0, surface_size.h - 1));
            }
        }
    }
    if let Some(region) = input_region {
        for (_, rect) in &region.rects {
            for x in [
                rect.loc.x - 1,
                rect.loc.x,
                rect.loc.x + rect.size.w - 1,
                rect.loc.x + rect.size.w,
            ] {
                xs.push(x.clamp(0, surface_size.w - 1));
            }
            for y in [
                rect.loc.y - 1,
                rect.loc.y,
                rect.loc.y + rect.size.h - 1,
                rect.loc.y + rect.size.h,
            ] {
                ys.push(y.clamp(0, surface_size.h - 1));
            }
        }
    }
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();

    let xs: Vec<_> = xs
        .into_iter()
        .map(|x| f64::from(x).clamp(bounds.loc.x, (bounds.loc.x + bounds.size.w).next_down()))
        .collect();
    let ys: Vec<_> = ys
        .into_iter()
        .map(|y| f64::from(y).clamp(bounds.loc.y, (bounds.loc.y + bounds.size.h).next_down()))
        .collect();
    xs.into_iter()
        .flat_map(|x| ys.iter().copied().map(move |y| Point::from((x, y))))
        .filter(|point| allowed(*point))
        .min_by(|left, right| {
            let left_dx = left.x - desired.x;
            let left_dy = left.y - desired.y;
            let right_dx = right.x - desired.x;
            let right_dy = right.y - desired.y;
            (left_dx * left_dx + left_dy * left_dy)
                .total_cmp(&(right_dx * right_dx + right_dy * right_dy))
        })
}

/// Clip confinement to its owner's output before projecting into shaped
/// regions. A surface-valid point may be offscreen after a Field pan or move.
fn visible_surface_bounds<D: SessionDriver>(
    session: &Session<D>,
    surface: &WlSurface,
    presentation: &WindowPresentation,
) -> Option<Rectangle<f64, Logical>> {
    let output = crate::wayland::display_scale::output_for_surface(&session.wayland, surface)
        .unwrap_or_else(|| session.driver.primary_output().clone());
    let mut bounds = session.wayland.space.output_geometry(&output)?;
    if bounds.size.w <= 0 || bounds.size.h <= 0 {
        return None;
    }
    // Keep the far edge inside the output through both affine conversions.
    bounds.size -= (1, 1).into();
    let start = presentation.surface_from_screen(surface, bounds.loc.to_f64())?;
    let end = presentation
        .surface_from_screen(surface, (bounds.loc + bounds.size.to_point()).to_f64())?;
    // Include the last logical pixel: Rectangle's far edge is exclusive.
    Some(Rectangle::from_extremities(
        start,
        (end.x.next_up(), end.y.next_up()),
    ))
}

fn prepare_candidate<D: SessionDriver>(
    session: &mut Session<D>,
    pointer: &PointerHandle<Session<D>>,
    surface: &WlSurface,
    context: &OwnerContext,
    constraint: &ConstraintDescriptor,
) -> Option<ConstraintGeometry> {
    if pointer.current_focus().as_ref() != Some(surface) {
        return None;
    }
    let screen = Point::from(session.pointer.position());
    let local = context.presentation.surface_from_screen(surface, screen)?;
    let bounds = Rectangle::from_size(context.surface_size);
    if !bounds.to_f64().contains(local)
        || !constraint
            .region
            .as_ref()
            .is_none_or(|region| region.contains(local.to_i32_floor()))
        || !surface_input_region(surface)
            .as_ref()
            .is_none_or(|region| region.contains(local.to_i32_floor()))
    {
        return None;
    }
    let geometry = constraint_geometry(
        surface,
        OwnerContext {
            presentation: context.presentation.clone(),
            surface_size: context.surface_size,
        },
        constraint,
    )?;
    let origin = geometry.origin;
    let location = origin + local;
    // Refresh Smithay's coordinate bookkeeping without manufacturing a
    // client-visible enter/motion event. Protocol activation is valid only
    // for the surface that already owns pointer focus.
    pointer.set_location(location);
    Some(geometry)
}

/// Reconciles Halley's tracked owner, Smithay's protocol state, compositor
/// pointer focus, mapping, and live presentation geometry in one ordered
/// transition.
///
/// Existing pointer focus is verified before activation. Conversely, an old
/// constraint is explicitly deactivated before focus changes, so one-shot
/// lifetime and client notifications remain deterministic.
pub(super) fn reconcile<D: SessionDriver>(
    session: &mut Session<D>,
    pointer: &PointerHandle<Session<D>>,
    preferred: Option<&WlSurface>,
) {
    let tracked = session.interactions.pointer_constraints.active.clone();
    let candidate = candidate_surface(session, pointer, preferred);
    let current_loss = tracked
        .as_ref()
        .and_then(|tracked| active_owner_loss(session, pointer, tracked));
    let current_valid = tracked.is_some() && current_loss.is_none();
    let retain_for_deferred_request = retain_while_unfocused_request_waits(
        current_valid,
        preferred.is_some(),
        candidate.is_some(),
    );
    if let Some(preferred) = preferred
        && candidate.is_none()
    {
        eventline::debug!(
            "pointer-constraint: defer kind={:?} surface={:?} pointer_focus={:?}",
            descriptor(preferred, pointer).map(|constraint| constraint.kind),
            preferred,
            pointer.current_focus()
        );
        if retain_for_deferred_request {
            return;
        }
    }
    let current_is_candidate = tracked
        .as_ref()
        .zip(candidate.as_ref())
        .is_some_and(|(tracked, candidate)| &tracked.surface == candidate);
    let candidate_descriptor = candidate
        .as_ref()
        .and_then(|surface| descriptor(surface, pointer));
    let decision = reconcile_decision(
        tracked.is_some(),
        current_is_candidate,
        current_valid,
        candidate.is_some(),
        candidate_descriptor
            .as_ref()
            .is_some_and(|constraint| constraint.active),
    );

    if decision.deactivate_current
        && let Some(tracked) = tracked.as_ref()
    {
        let reason = current_loss.unwrap_or(DeactivationReason::CandidateChanged);
        deactivate_tracked(session, pointer, tracked, reason);
        session.interactions.pointer_constraints.active = None;
        session.interactions.pointer_constraints.suspended = None;
    }

    let Some(candidate) = candidate else {
        session.interactions.pointer_constraints.active = None;
        session.interactions.pointer_constraints.suspended = None;
        return;
    };
    let Some(context) = owner_context(session, &candidate) else {
        if retain_cached_geometry(current_valid, current_is_candidate) {
            session.interactions.pointer_constraints.active = tracked;
            sync_grab_suspension(session, pointer);
        } else {
            session.interactions.pointer_constraints.active = None;
            session.interactions.pointer_constraints.suspended = None;
        }
        return;
    };
    let Some(mut constraint) = descriptor(&candidate, pointer) else {
        session.interactions.pointer_constraints.active = None;
        session.interactions.pointer_constraints.suspended = None;
        return;
    };

    let refreshing_active = current_valid && current_is_candidate && constraint.active;
    let grab_suspends_candidate = refreshing_active && grab_owns_surface(session, &candidate);
    let geometry = if refreshing_active {
        constraint_geometry(&candidate, context, &constraint)
    } else {
        prepare_candidate(session, pointer, &candidate, &context, &constraint)
    };
    let Some(geometry) = geometry else {
        // Live presentation data can be transiently absent while an already
        // active owner is resizing or changing output. Keep its last valid
        // inverse transform until an authoritative lifecycle transition.
        if retain_cached_geometry(current_valid, current_is_candidate) {
            session.interactions.pointer_constraints.active = tracked;
            sync_grab_suspension(session, pointer);
        } else {
            session.interactions.pointer_constraints.active = None;
            session.interactions.pointer_constraints.suspended = None;
        }
        return;
    };
    if refreshing_active {
        let screen = Point::from(session.pointer.position());
        let local = geometry
            .presentation
            .surface_from_screen(&candidate, screen);
        match constraint.kind {
            ConstraintKind::Locked => {
                if let Some(local) = local {
                    pointer.set_location(geometry.origin + local);
                }
            }
            ConstraintKind::Confined if !grab_suspends_candidate => {
                let corrected = local.and_then(|local| {
                    let visible =
                        visible_surface_bounds(session, &candidate, &geometry.presentation)?;
                    clamp_into_constraint(
                        local,
                        geometry.surface_size,
                        geometry.region.as_ref(),
                        geometry.input_region.as_ref(),
                        Some(visible),
                    )
                    .map(|corrected| (local, corrected))
                });
                let Some((local, corrected)) = corrected else {
                    if let Some(tracked) = tracked.as_ref() {
                        deactivate_tracked(
                            session,
                            pointer,
                            tracked,
                            DeactivationReason::EffectiveRegionEmpty,
                        );
                    }
                    session.interactions.pointer_constraints.active = None;
                    session.interactions.pointer_constraints.suspended = None;
                    return;
                };
                pointer.set_location(geometry.origin + corrected);
                if corrected != local
                    && let Some(screen) = geometry
                        .presentation
                        .screen_from_surface(&candidate, corrected)
                {
                    session.pointer.set_position((screen.x, screen.y));
                    pointer.motion(
                        session,
                        Some((candidate.clone(), geometry.origin)),
                        &MotionEvent {
                            location: geometry.origin + corrected,
                            serial: SERIAL_COUNTER.next_serial(),
                            time: smithay::backend::input::InputTime::from_millis(
                                session.start_time.elapsed().as_millis() as u32,
                            ),
                        },
                    );
                }
            }
            ConstraintKind::Confined => {
                if let Some(local) = local {
                    pointer.set_location(geometry.origin + local);
                }
            }
        }
    }
    if decision.activate_candidate {
        if !activate(&candidate, pointer) {
            session.interactions.pointer_constraints.active = None;
            session.interactions.pointer_constraints.suspended = None;
            return;
        }
        eventline::debug!(
            "pointer-constraint: activate kind={:?} surface={:?}",
            constraint.kind,
            candidate
        );
        crate::session::trace::surface_event(
            session,
            &candidate,
            "constraint-activate",
            format_args!("kind={:?}", constraint.kind),
        );
        constraint.active = true;
    }
    if pointer.current_focus().as_ref() == Some(&candidate) && constraint.active {
        session.interactions.pointer_constraints.active = Some(TrackedConstraint {
            surface: candidate,
            kind: constraint.kind,
            position_hint: constraint.position_hint,
            geometry,
        });
        sync_grab_suspension(session, pointer);
        session.request_redraw();
    } else {
        session.interactions.pointer_constraints.active = None;
        session.interactions.pointer_constraints.suspended = None;
    }
}

pub(super) fn deactivate_before_focus_change<D: SessionDriver>(
    session: &mut Session<D>,
    next_focused_root: Option<&WlSurface>,
) {
    let Some(pointer) = session.seat.get_pointer() else {
        return;
    };
    let should_deactivate = session
        .interactions
        .pointer_constraints
        .active
        .as_ref()
        .is_some_and(|tracked| {
            let authority = constraint_authority_root(session, &tracked.surface);
            Some(&authority) != next_focused_root
        });
    if should_deactivate {
        if let Some(tracked) = session.interactions.pointer_constraints.active.take() {
            session.interactions.pointer_constraints.suspended = None;
            deactivate_tracked(
                session,
                &pointer,
                &tracked,
                DeactivationReason::RequestedKeyboardFocusChange,
            );
        }
        pointer.frame(session);
    }
}

pub(super) fn deactivate_before_pointer_focus_change<D: SessionDriver>(
    session: &mut Session<D>,
    next_focus: Option<&WlSurface>,
) {
    let Some(pointer) = session.seat.get_pointer() else {
        return;
    };
    let should_deactivate = session
        .interactions
        .pointer_constraints
        .active
        .as_ref()
        .is_some_and(|tracked| Some(&tracked.surface) != next_focus);
    if should_deactivate {
        if let Some(tracked) = session.interactions.pointer_constraints.active.take() {
            session.interactions.pointer_constraints.suspended = None;
            deactivate_tracked(
                session,
                &pointer,
                &tracked,
                DeactivationReason::RequestedPointerFocusChange,
            );
        }
        pointer.frame(session);
    }
}

pub(super) fn deactivate_before_unmap<D: SessionDriver>(
    session: &mut Session<D>,
    root: &WlSurface,
) {
    let Some(pointer) = session.seat.get_pointer() else {
        return;
    };
    let should_deactivate = session
        .interactions
        .pointer_constraints
        .active
        .as_ref()
        .is_some_and(|tracked| {
            crate::wayland::compositor::root_surface(&tracked.surface) == *root
                || constraint_authority_root(session, &tracked.surface) == *root
        });
    if should_deactivate {
        if let Some(tracked) = session.interactions.pointer_constraints.active.take() {
            session.interactions.pointer_constraints.suspended = None;
            deactivate_tracked(
                session,
                &pointer,
                &tracked,
                DeactivationReason::RequestedOwnerUnmap,
            );
        }
        pointer.frame(session);
    }
}

pub(super) fn active<D: SessionDriver>(
    session: &Session<D>,
    pointer: &PointerHandle<Session<D>>,
) -> Option<ActiveConstraint> {
    let tracked = session.interactions.pointer_constraints.active.as_ref()?;
    if !valid_active_owner(session, pointer, tracked) {
        return None;
    }
    Some(ActiveConstraint {
        surface: tracked.surface.clone(),
        origin: tracked.geometry.origin,
        kind: tracked.kind,
        surface_size: tracked.geometry.surface_size,
        region: tracked.geometry.region.clone(),
        input_region: tracked.geometry.input_region.clone(),
        presentation: tracked.geometry.presentation.clone(),
    })
}

pub(super) fn has_active_lock<D: SessionDriver>(
    session: &Session<D>,
    pointer: &PointerHandle<Session<D>>,
) -> bool {
    active(session, pointer).is_some_and(|constraint| constraint.kind == ConstraintKind::Locked)
}

pub(super) fn has_active_confinement<D: SessionDriver>(
    session: &Session<D>,
    pointer: &PointerHandle<Session<D>>,
) -> bool {
    active(session, pointer).is_some_and(|constraint| constraint.kind == ConstraintKind::Confined)
}

pub(super) struct ActiveConstraint {
    pub surface: WlSurface,
    pub origin: Point<f64, Logical>,
    pub kind: ConstraintKind,
    surface_size: Size<i32, Logical>,
    region: Option<RegionAttributes>,
    input_region: Option<RegionAttributes>,
    presentation: WindowPresentation,
}

pub(super) fn cursor_visible<D: SessionDriver>(session: &Session<D>) -> bool {
    session.seat.get_pointer().is_none_or(|pointer| {
        active(session, &pointer).is_none_or(|constraint| {
            constraint.kind != ConstraintKind::Locked || enforcement_suspended(session, &pointer)
        })
    })
}

/// The screen position the pointer must be moved to in order to satisfy
/// `constraint`, or `None` when it is already somewhere the confinement
/// allows.
///
/// The surface bounds are half the answer: `confine_pointer` with a `NULL`
/// region means "confine to this surface", which the region alone cannot
/// express, and [`WindowPresentation::surface_from_screen`] is a plain
/// affine transform that happily reports coordinates outside the surface.
pub(super) fn confined_position<D: SessionDriver>(
    session: &Session<D>,
    constraint: &ActiveConstraint,
) -> Option<Point<f64, Logical>> {
    let screen = Point::from(session.pointer.position());
    let local = constraint
        .presentation
        .surface_from_screen(&constraint.surface, screen)?;
    let corrected = clamp_into_constraint(
        local,
        constraint.surface_size,
        constraint.region.as_ref(),
        constraint.input_region.as_ref(),
        Some(visible_surface_bounds(
            session,
            &constraint.surface,
            &constraint.presentation,
        )?),
    )?;
    if corrected == local {
        return None;
    }
    constraint
        .presentation
        .screen_from_surface(&constraint.surface, corrected)
}

/// Projects `local` back into the confinement, in surface coordinates.
///
/// The bounds clamp stays in `f64` rather than going through
/// [`nearest_valid_point`]: pushing the pointer into an edge must preserve
/// the tangential component exactly, and a lattice projection would quantise
/// the slide to whole pixels. The lattice projection is only needed once a
/// region carves the surface into shapes a per-axis clamp cannot describe.
fn clamp_into_constraint(
    local: Point<f64, Logical>,
    surface_size: Size<i32, Logical>,
    region: Option<&RegionAttributes>,
    input_region: Option<&RegionAttributes>,
    visible_bounds: Option<Rectangle<f64, Logical>>,
) -> Option<Point<f64, Logical>> {
    if surface_size.w <= 0 || surface_size.h <= 0 {
        return None;
    }
    let bounds = Rectangle::from_size(surface_size).to_f64();
    let bounds = match visible_bounds {
        Some(visible) => bounds.intersection(visible)?,
        None => bounds,
    };
    if bounds.size.w <= 0.0 || bounds.size.h <= 0.0 {
        return None;
    }
    // `Rectangle::contains` is exclusive at the far edge, so the last
    // representable position inside the surface is just below it.
    let clamped = Point::from((
        local
            .x
            .clamp(bounds.loc.x, (bounds.loc.x + bounds.size.w).next_down()),
        local
            .y
            .clamp(bounds.loc.y, (bounds.loc.y + bounds.size.h).next_down()),
    ));
    if region.is_none_or(|region| region.contains(clamped.to_i32_floor()))
        && input_region.is_none_or(|region| region.contains(clamped.to_i32_floor()))
    {
        return Some(clamped);
    }
    nearest_valid_point(local, surface_size, region, input_region, visible_bounds)
}

pub(super) fn apply_position_hint<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &WlSurface,
    _pointer: &PointerHandle<Session<D>>,
    location: Point<f64, Logical>,
) {
    if let Some(tracked) = session.interactions.pointer_constraints.active.as_mut()
        && tracked.surface == *surface
    {
        tracked.position_hint = Some(location);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::wayland::compositor::RectangleKind;

    fn decision(
        current_present: bool,
        current_is_candidate: bool,
        current_valid: bool,
        candidate_present: bool,
        candidate_protocol_active: bool,
    ) -> ReconcileDecision {
        reconcile_decision(
            current_present,
            current_is_candidate,
            current_valid,
            candidate_present,
            candidate_protocol_active,
        )
    }

    #[test]
    fn startup_churn_retains_an_already_valid_owner() {
        assert_eq!(
            decision(true, true, true, true, true),
            ReconcileDecision {
                deactivate_current: false,
                activate_candidate: false,
            }
        );
    }

    #[test]
    fn new_constraint_never_manufactures_pointer_focus() {
        assert!(preferred_matches_focus(None));
        assert!(preferred_matches_focus(Some(true)));
        assert!(!preferred_matches_focus(Some(false)));
    }

    #[test]
    fn xwayland_grab_proxy_uses_pointer_focus_without_wm_focus() {
        assert_eq!(owner_state_loss(true, true, true, false, false), None);
        assert_eq!(
            owner_state_loss(true, true, false, false, false),
            Some(DeactivationReason::CompositorFocusChanged)
        );
        assert_eq!(
            owner_state_loss(true, false, true, false, false),
            Some(DeactivationReason::OwnerUnmapped)
        );
    }

    #[test]
    fn keyboard_focus_switch_skips_the_old_client_position_hint() {
        assert!(!deactivation_applies_position_hint(
            DeactivationReason::RequestedKeyboardFocusChange
        ));
        assert!(deactivation_applies_position_hint(
            DeactivationReason::RequestedPointerFocusChange
        ));
    }

    #[test]
    fn transient_presentation_gap_retains_only_the_authoritative_owner() {
        assert!(retain_cached_geometry(true, true));
        assert!(!retain_cached_geometry(false, true));
        assert!(!retain_cached_geometry(true, false));
    }

    #[test]
    fn unfocused_new_request_does_not_displace_the_active_owner() {
        assert!(retain_while_unfocused_request_waits(true, true, false));
        assert!(!retain_while_unfocused_request_waits(false, true, false));
        assert!(!retain_while_unfocused_request_waits(true, true, true));
    }

    #[test]
    fn stale_constraint_is_replaced_only_with_an_exact_focus_candidate() {
        assert_eq!(
            decision(true, true, false, true, false),
            ReconcileDecision {
                deactivate_current: true,
                activate_candidate: true,
            }
        );
    }

    #[test]
    fn focus_loss_and_unmap_deactivate_without_replacement() {
        assert_eq!(
            decision(true, false, false, false, false),
            ReconcileDecision {
                deactivate_current: true,
                activate_candidate: false,
            }
        );
    }

    #[test]
    fn persistent_constraint_reactivates_only_after_remap_and_exact_focus() {
        let unmapped = decision(true, false, false, false, false);
        let remapped_without_focus = decision(false, false, false, false, false);
        let remapped_and_focused = decision(false, false, false, true, false);

        assert!(unmapped.deactivate_current);
        assert_eq!(
            remapped_without_focus,
            ReconcileDecision {
                deactivate_current: false,
                activate_candidate: false,
            }
        );
        assert_eq!(
            remapped_and_focused,
            ReconcileDecision {
                deactivate_current: false,
                activate_candidate: true,
            }
        );
    }

    #[test]
    fn removed_oneshot_constraint_does_not_reactivate_after_remap() {
        let remapped_without_protocol_candidate = decision(false, false, false, false, false);
        assert_eq!(
            remapped_without_protocol_candidate,
            ReconcileDecision {
                deactivate_current: false,
                activate_candidate: false,
            }
        );
    }

    #[test]
    fn owner_replacement_deactivates_then_activates_exact_focus() {
        assert_eq!(
            decision(true, false, false, true, false),
            ReconcileDecision {
                deactivate_current: true,
                activate_candidate: true,
            }
        );
    }

    #[test]
    fn unfocused_or_unmapped_constraints_have_no_candidate() {
        assert_eq!(
            decision(false, false, false, false, false),
            ReconcileDecision {
                deactivate_current: false,
                activate_candidate: false,
            }
        );
    }

    #[test]
    fn confinement_chooses_nearest_point_outside_a_subtracted_hole() {
        let region = RegionAttributes {
            rects: vec![
                (
                    RectangleKind::Add,
                    Rectangle::new((0, 0).into(), (100, 100).into()),
                ),
                (
                    RectangleKind::Subtract,
                    Rectangle::new((40, 40).into(), (20, 20).into()),
                ),
            ],
        };

        assert_eq!(
            nearest_valid_point(
                (50.0, 50.0).into(),
                (100, 100).into(),
                Some(&region),
                None,
                None,
            ),
            Some(Point::from((50.0, 60.0)))
        );
    }

    #[test]
    fn locked_motion_is_relative_only() {
        assert_eq!(
            motion_disposition(Some(ConstraintKind::Locked), None),
            MotionDisposition::RelativeOnly
        );
    }

    #[test]
    fn only_a_confinement_correction_clamps_motion() {
        let correction = Point::from((12.0, 34.0));
        assert_eq!(
            motion_disposition(Some(ConstraintKind::Confined), Some(correction)),
            MotionDisposition::Clamp(correction)
        );
        assert_eq!(
            motion_disposition(Some(ConstraintKind::Confined), None),
            MotionDisposition::Apply
        );
        assert_eq!(motion_disposition(None, None), MotionDisposition::Apply);
    }

    #[test]
    fn a_null_region_confines_to_the_surface() {
        let surface = Size::from((100, 80));

        // Inside stays untouched, so no correction is reported upstream.
        assert_eq!(
            clamp_into_constraint((40.5, 20.25).into(), surface, None, None, None),
            Some(Point::from((40.5, 20.25)))
        );
        // Outside on both axes is pulled back onto the surface.
        assert_eq!(
            clamp_into_constraint((-30.0, 500.0).into(), surface, None, None, None),
            Some(Point::from((0.0, f64::from(80).next_down())))
        );
    }

    #[test]
    fn clamping_preserves_the_tangential_component_of_an_edge_slide() {
        let surface = Size::from((100, 80));
        let slide = clamp_into_constraint((-5.0, 33.75).into(), surface, None, None, None)
            .expect("a positive surface size always yields a projection");

        assert_eq!(slide.x, 0.0);
        // The component parallel to the edge survives at full precision; a
        // lattice projection would have quantised it to a whole pixel.
        assert_eq!(slide.y, 33.75);
    }

    #[test]
    fn a_region_hole_falls_back_to_the_lattice_projection() {
        let region = RegionAttributes {
            rects: vec![
                (
                    RectangleKind::Add,
                    Rectangle::new((0, 0).into(), (100, 100).into()),
                ),
                (
                    RectangleKind::Subtract,
                    Rectangle::new((40, 40).into(), (20, 20).into()),
                ),
            ],
        };

        // A bounds clamp cannot describe the hole, so the projection runs.
        assert_eq!(
            clamp_into_constraint(
                (50.0, 50.0).into(),
                (100, 100).into(),
                Some(&region),
                None,
                None,
            ),
            Some(Point::from((50.0, 60.0)))
        );
    }

    #[test]
    fn a_degenerate_surface_has_no_valid_position() {
        assert_eq!(
            clamp_into_constraint((5.0, 5.0).into(), (0, 40).into(), None, None, None),
            None
        );
    }

    #[test]
    fn confinement_intersects_constraint_and_surface_input_regions() {
        let constraint = RegionAttributes {
            rects: vec![(
                RectangleKind::Add,
                Rectangle::new((0, 0).into(), (80, 80).into()),
            )],
        };
        let input = RegionAttributes {
            rects: vec![(
                RectangleKind::Add,
                Rectangle::new((40, 20).into(), (50, 50).into()),
            )],
        };

        assert_eq!(
            clamp_into_constraint(
                (10.0, 30.0).into(),
                (100, 100).into(),
                Some(&constraint),
                Some(&input),
                None,
            ),
            Some(Point::from((40.0, 30.0)))
        );
        assert_eq!(
            clamp_into_constraint(
                (90.0, 30.0).into(),
                (100, 100).into(),
                Some(&constraint),
                Some(&input),
                None,
            ),
            Some(Point::from((79.0, 30.0)))
        );
    }

    #[test]
    fn confinement_intersects_visible_fractional_bounds_without_losing_edge_sliding() {
        let bounds = Rectangle::new((180.25, 0.0).into(), (40.5, 160.0).into());
        let slide = clamp_into_constraint(
            (120.0, 80.625).into(),
            (240, 160).into(),
            None,
            None,
            Some(bounds),
        )
        .unwrap();
        assert_eq!(slide, Point::from((180.25, 80.625)));

        let region = RegionAttributes {
            rects: vec![
                (
                    RectangleKind::Add,
                    Rectangle::new((0, 0).into(), (240, 160).into()),
                ),
                (
                    RectangleKind::Subtract,
                    Rectangle::new((175, 40).into(), (35, 80).into()),
                ),
            ],
        };
        let projected = clamp_into_constraint(
            (120.0, 80.0).into(),
            (240, 160).into(),
            Some(&region),
            None,
            Some(bounds),
        )
        .unwrap();
        assert!(bounds.contains(projected));
        assert!(region.contains(projected.to_i32_floor()));
    }

    #[test]
    fn offscreen_or_shaped_empty_confinement_has_no_valid_position() {
        let empty = Rectangle::new((20.0, 0.0).into(), (0.0, 160.0).into());
        assert_eq!(
            clamp_into_constraint(
                (50.0, 80.0).into(),
                (240, 160).into(),
                None,
                None,
                Some(empty)
            ),
            None,
        );
        let hidden = Rectangle::new((300.0, 0.0).into(), (40.0, 160.0).into());
        assert_eq!(
            clamp_into_constraint(
                (50.0, 80.0).into(),
                (240, 160).into(),
                None,
                None,
                Some(hidden)
            ),
            None
        );
        let visible = Rectangle::new((180.0, 0.0).into(), (60.0, 160.0).into());
        let region = RegionAttributes {
            rects: vec![(
                RectangleKind::Add,
                Rectangle::new((0, 0).into(), (100, 160).into()),
            )],
        };
        assert_eq!(
            clamp_into_constraint(
                (50.0, 80.0).into(),
                (240, 160).into(),
                Some(&region),
                None,
                Some(visible)
            ),
            None
        );
    }
}
