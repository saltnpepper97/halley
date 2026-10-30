use std::ffi::OsStr;

use smithay::backend::input::{
    Axis, ButtonState, Event, InputBackend, InputEvent, KeyState, KeyboardKeyEvent,
    PointerAxisEvent, PointerButtonEvent, PointerMotionEvent, TouchEvent,
};
use smithay::desktop::{Space, Window};
use smithay::input::keyboard::{FilterResult, Keysym};
use smithay::input::pointer::{ButtonEvent, RelativeMotionEvent};
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size};
use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat;
use smithay::wayland::seat::WaylandFocus;

use super::{Session, SessionDriver};
use crate::input::pointer::{axis_frame_filtered, process_wheel_bindings};
use crate::input::{
    PointerBindingResult, match_keyboard_binding, match_pointer_bind, match_wheel_bind,
    process_pointer_binding,
};
use crate::wayland;

pub(crate) mod actions;
mod axis;
mod button;
mod keyboard;
mod modal;
mod motion;
pub(crate) mod repeat;

const BTN_LEFT: u32 = 0x110;
#[cfg(test)]
const BTN_RIGHT: u32 = 0x111;
const NODE_DRAG_THRESHOLD_PX: f64 = 8.0;
const LIFT_LAYER_NAMESPACE: &str = "halley-lift";

fn sampled_drag_velocity(
    previous: halley_core::field::Vec2,
    current: halley_core::field::Vec2,
    previous_velocity: halley_core::field::Vec2,
    last_update: std::time::Duration,
    now: std::time::Duration,
) -> halley_core::field::Vec2 {
    let dt = now.saturating_sub(last_update).as_secs_f32();
    if dt <= f32::EPSILON {
        return previous_velocity;
    }
    let clamp = |value: f32| value.clamp(-800.0, 800.0);
    halley_core::field::Vec2 {
        x: previous_velocity.x * 0.35 + clamp((current.x - previous.x) / dt) * 0.65,
        y: previous_velocity.y * 0.35 + clamp((current.y - previous.y) / dt) * 0.65,
    }
}

fn collapsed_node_drop_origin(
    center: halley_core::field::Vec2,
    size: Size<i32, Logical>,
    output_geometry: Rectangle<i32, Logical>,
) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (
            (center.x - size.w as f32 * 0.5).round() as i32 - output_geometry.loc.x,
            (center.y - size.h as f32 * 0.5).round() as i32 - output_geometry.loc.y,
        )
            .into(),
        size,
    )
}

fn drag_threshold_reached(press: Point<f64, Logical>, current: (f64, f64)) -> bool {
    let dx = current.0 - press.x;
    let dy = current.1 - press.y;
    dx.hypot(dy) >= NODE_DRAG_THRESHOLD_PX
}

fn begin_cluster_core_direct_motion(
    nodes: &mut crate::nodes::NodesState,
    core: Option<halley_core::field::NodeId>,
) {
    if let Some(core) = core {
        nodes.clear_direct_motion(core);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingWindowMoveMotion {
    Wait,
    Cancel,
    Activate,
}

fn pending_window_move_motion(
    pointer_grab_valid: bool,
    press: Point<f64, Logical>,
    current: (f64, f64),
) -> PendingWindowMoveMotion {
    if !pointer_grab_valid {
        PendingWindowMoveMotion::Cancel
    } else if drag_threshold_reached(press, current) {
        PendingWindowMoveMotion::Activate
    } else {
        PendingWindowMoveMotion::Wait
    }
}

fn releases_pending_window_move(pending_button: u32, event_button: u32, released: bool) -> bool {
    released && pending_button == event_button
}

/// A camera pan belongs to the button whose press began it: releasing any
/// other button (for example a left click made while a middle-button pan is
/// running) must neither end the drag nor be mistaken for its completion.
fn releases_field_pan(grab: &crate::input::grab::Grab, event_button: u32, released: bool) -> bool {
    released
        && matches!(
            grab,
            crate::input::grab::Grab::Pan { button, .. } if *button == event_button
        )
}

fn forward_pointer_button(intercepted: bool, finishing_client_move: bool) -> bool {
    !intercepted || finishing_client_move
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BasicsInputRouting {
    Dismiss,
    SuppressOwned,
    Forward,
}

fn basics_pointer_button_routing(
    accepts_input: bool,
    button: u32,
    state: ButtonState,
    owned: &mut crate::input::SuppressedButtons,
) -> BasicsInputRouting {
    match state {
        ButtonState::Pressed if accepts_input => {
            owned.suppress(button);
            BasicsInputRouting::Dismiss
        }
        ButtonState::Released if owned.release_is_suppressed(button) => {
            BasicsInputRouting::SuppressOwned
        }
        _ => BasicsInputRouting::Forward,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BasicsTouchPhase {
    Down,
    Motion,
    Up,
}

fn basics_touch_routing(
    accepts_input: bool,
    slot: smithay::backend::input::TouchSlot,
    phase: BasicsTouchPhase,
    owned: &mut crate::input::SuppressedReleases<smithay::backend::input::TouchSlot>,
) -> BasicsInputRouting {
    match phase {
        BasicsTouchPhase::Down if accepts_input => {
            owned.suppress(slot);
            BasicsInputRouting::Dismiss
        }
        BasicsTouchPhase::Motion if owned.contains(slot) => BasicsInputRouting::SuppressOwned,
        BasicsTouchPhase::Up if owned.release_is_suppressed(slot) => {
            BasicsInputRouting::SuppressOwned
        }
        _ => BasicsInputRouting::Forward,
    }
}

fn outside_lift_press_dismisses(
    button: u32,
    state: ButtonState,
    lift_is_mapped: bool,
    route_is_lift: bool,
) -> bool {
    button == BTN_LEFT && state == ButtonState::Pressed && lift_is_mapped && !route_is_lift
}

/// Lift intentionally uses a palette-sized exclusive layer surface. That lets an
/// outside click route to the underlying client, but means Lift itself cannot observe
/// the click. Close the mapped first-party layer here while leaving the press unconsumed.
fn dismiss_lift_on_outside_press<D: SessionDriver>(
    session: &mut Session<D>,
    route: Option<&crate::input::pointer::PointerRoute>,
    button: u32,
    state: ButtonState,
) {
    let lift = session.wayland.space.outputs().find_map(|output| {
        let map = smithay::desktop::layer_map_for_output(output);
        map.layers()
            .find(|layer| {
                layer.namespace() == LIFT_LAYER_NAMESPACE
                    && !session.wayland.unmapped_layers.contains(layer.wl_surface())
            })
            .cloned()
    });
    let route_is_lift = lift.as_ref().is_some_and(|lift| {
        matches!(
            route.map(|route| &route.target),
            Some(crate::input::pointer::PointerTarget::Layer(layer)) if layer == lift
        )
    });
    if outside_lift_press_dismisses(button, state, lift.is_some(), route_is_lift)
        && let Some(lift) = lift
    {
        lift.layer_surface().send_close();
    }
}

// Retire the previous press before any input interception can return early.
// Only the final client-forwarding path may arm a new press or use this release.
fn take_steam_close_press<T>(pending: &mut Option<T>, button: u32) -> Option<T> {
    if button == BTN_LEFT {
        pending.take()
    } else {
        None
    }
}

fn forwarded_steam_close_button<T: PartialEq>(
    pending: &mut Option<T>,
    previous: Option<T>,
    button: u32,
    state: ButtonState,
    target: Option<T>,
) -> Option<T> {
    if button != BTN_LEFT {
        return None;
    }
    match state {
        ButtonState::Pressed => {
            *pending = target;
            None
        }
        ButtonState::Released => target.filter(|target| previous.as_ref() == Some(target)),
    }
}

fn steam_client_close_target(
    route: Option<&crate::input::pointer::PointerRoute>,
) -> Option<Window> {
    let route = route?;
    let crate::input::pointer::PointerTarget::Window(window) = &route.target else {
        return None;
    };
    let (_, surface_origin) = route.focus.as_ref()?;
    let local = route.location - *surface_origin;
    crate::xwayland::is_steam_client_close_hit(window, local).then(|| window.clone())
}

fn plain_background_press_dismisses_bloom(intercepted: bool, on_background: bool) -> bool {
    !intercepted && on_background
}

fn typing_abandons_bloom(window_drag_active: bool, focused_owns_bloom: bool) -> bool {
    !window_drag_active && !focused_owns_bloom
}

fn is_modifier_keysym(keysym: Keysym) -> bool {
    matches!(
        keysym,
        Keysym::Shift_L
            | Keysym::Shift_R
            | Keysym::Control_L
            | Keysym::Control_R
            | Keysym::Alt_L
            | Keysym::Alt_R
            | Keysym::Super_L
            | Keysym::Super_R
    )
}

fn shortcut_policy_allows_bindings(session_locked: bool, inhibitor_active: bool) -> bool {
    !session_locked && !inhibitor_active
}

pub(super) fn bindings_enabled<D: SessionDriver>(session: &Session<D>) -> bool {
    let focus = wayland::focus::current(
        &session.wayland,
        &session.fullscreen,
        &session.clusters,
        &session.nodes,
        crate::frame_clock::monotonic_now(),
    );
    let inhibitor_active = focus
        .map(|focus| focus.surface())
        .and_then(|surface| {
            session
                .seat
                .keyboard_shortcuts_inhibitor_for_surface(&surface)
        })
        .is_some_and(|inhibitor| inhibitor.is_active());
    // Exclusive layer focus owns forwarded keys, but does not itself inhibit
    // compositor shortcuts. Lock-screen isolation and explicit client
    // inhibitors remain authoritative.
    shortcut_policy_allows_bindings(session.session_lock.active(), inhibitor_active)
}

pub(super) fn binding_context_for_output<D: SessionDriver>(
    session: &Session<D>,
    output_name: Option<&str>,
) -> crate::input::BindingContext {
    let Some(output_name) = output_name else {
        return crate::input::BindingContext::default();
    };
    let Some(cluster) = session.clusters.active_on(output_name) else {
        return crate::input::BindingContext::field();
    };
    let tiling = session.clusters.metadata(cluster).is_some_and(|metadata| {
        metadata.layout == halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Tiling
    });
    crate::input::BindingContext::cluster(tiling)
}

pub(super) fn keyboard_binding_context<D: SessionDriver>(
    session: &Session<D>,
) -> crate::input::BindingContext {
    let selected =
        crate::wayland::focus::selected_output(&session.wayland).map(|output| output.name());
    let output = selected;
    binding_context_for_output(session, output.as_deref())
}

fn dispatch_pointer_grab_action<D: SessionDriver>(
    session: &mut Session<D>,
    action: &halley_config::Action,
    route: Option<&crate::input::pointer::PointerRoute>,
    button: u32,
    serial: smithay::utils::Serial,
) -> bool {
    match action {
        halley_config::Action::PointerMoveWindow => {
            let Some(route) = route else {
                return false;
            };
            if pointer_move_falls_back_to_field_pan(&route.target) {
                return begin_field_pan(session, route, button, serial);
            }
            let window = match &route.target {
                crate::input::pointer::PointerTarget::Window(window)
                | crate::input::pointer::PointerTarget::Decoration { window, .. } => window,
                _ => return false,
            };
            if crate::window::accepts_popup_move(window) {
                let geometry = route.visual_geometry.unwrap_or_else(|| window.geometry());
                let pointer = session.pointer.position();
                session.interactions.grab = crate::input::grab::Grab::MovePopup {
                    window: window.clone(),
                    button,
                    offset: (
                        f64::from(geometry.loc.x) - pointer.0,
                        f64::from(geometry.loc.y) - pointer.1,
                    )
                        .into(),
                };
                session.cursor.set_override(
                    crate::cursor::OverrideSource::Grab,
                    Some(smithay::input::pointer::CursorIcon::Grabbing),
                );
                return true;
            }
            if !crate::window::accepts_compositor_grab(window)
                || window.wl_surface().is_some_and(|surface| {
                    session
                        .fullscreen
                        .is_fullscreen_or_pending(surface.as_ref())
                })
            {
                return false;
            }
            wayland::focus::select_output(&mut session.wayland, &route.output);
            super::begin_pointer_move(session, window, serial, button)
        }
        halley_config::Action::PointerResizeWindow => {
            let Some(route) = route else {
                return false;
            };
            let (window, border_handle) = match &route.target {
                crate::input::pointer::PointerTarget::Window(window) => (window, None),
                crate::input::pointer::PointerTarget::Decoration {
                    window,
                    hit: crate::titlebar::Hit::Resize(handle),
                } => (window, Some(*handle)),
                crate::input::pointer::PointerTarget::Decoration { window, .. } => (window, None),
                _ => return false,
            };
            if !crate::window::accepts_compositor_grab(window)
                || window.wl_surface().is_some_and(|surface| {
                    session
                        .fullscreen
                        .is_fullscreen_or_pending(surface.as_ref())
                })
            {
                return false;
            }
            let Some(start_rect) = session.wayland.space.element_geometry(window) else {
                return false;
            };
            let world = halley_core::field::Vec2 {
                x: route.location.x as f32,
                y: route.location.y as f32,
            };
            let handle = border_handle.unwrap_or_else(|| {
                crate::input::grab::handle_from_press_position(start_rect, world)
            });
            wayland::focus::select_output(&mut session.wayland, &route.output);
            super::begin_window_resize(
                session,
                window,
                handle,
                button,
                world,
                route.visual_geometry.unwrap_or(start_rect),
                serial,
            )
        }
        halley_config::Action::PointerPanField => {
            let Some(route) = route else {
                return false;
            };
            begin_field_pan(session, route, button, serial)
        }
        halley_config::Action::PointerPanFieldAlways => {
            let Some(route) = route else {
                return false;
            };
            begin_field_pan_always(session, route, button, serial)
        }
        halley_config::Action::PointerDragPan => {
            let Some(route) = route else {
                return false;
            };
            let window = match &route.target {
                crate::input::pointer::PointerTarget::Window(window)
                | crate::input::pointer::PointerTarget::Decoration { window, .. } => window,
                _ => return false,
            };
            if !crate::window::accepts_compositor_grab(window)
                || window.wl_surface().is_some_and(|surface| {
                    session
                        .fullscreen
                        .is_fullscreen_or_pending(surface.as_ref())
                })
            {
                return false;
            }
            wayland::focus::select_output(&mut session.wayland, &route.output);
            super::begin_pointer_edge_pan(session, window, serial, button)
        }
        _ => false,
    }
}

fn pointer_move_falls_back_to_field_pan(target: &crate::input::pointer::PointerTarget) -> bool {
    matches!(target, crate::input::pointer::PointerTarget::Background)
}

fn begin_field_pan<D: SessionDriver>(
    session: &mut Session<D>,
    route: &crate::input::pointer::PointerRoute,
    button: u32,
    serial: smithay::utils::Serial,
) -> bool {
    if !matches!(
        &route.target,
        crate::input::pointer::PointerTarget::Background
    ) || node_at_pointer(session).is_some()
    {
        return false;
    }
    if !start_field_pan_grab(session, route, button) {
        return false;
    }
    wayland::focus::select_output(&mut session.wayland, &route.output);
    super::focus::focus_layer(session, None, serial);
    true
}

/// Layer-shell surfaces beneath the windows (wallpaper daemons, desktop
/// widgets) are part of the desktop surface, so `pan-field-always` pans over
/// them. Panels, launchers, notifications, and lock screens live on the Top and
/// Overlay layers: they are screen-fixed UI that must keep their own clicks.
fn layer_allows_field_pan_always(layer: smithay::wayland::shell::wlr_layer::Layer) -> bool {
    use smithay::wayland::shell::wlr_layer::Layer;
    matches!(layer, Layer::Background | Layer::Bottom)
}

/// Windows `pan-field-always` may begin over. It follows `drag-pan`'s policy
/// for a window the compositor may grab (X11 override-redirect menus and
/// tooltips are excluded) and additionally leaves fullscreen windows, including
/// ones whose fullscreen request is still pending, entirely to the client.
fn window_allows_field_pan_always<D: SessionDriver>(session: &Session<D>, window: &Window) -> bool {
    crate::window::accepts_compositor_grab(window)
        && !crate::xwayland::is_fullscreen(window)
        && !window.wl_surface().is_some_and(|surface| {
            session
                .fullscreen
                .is_fullscreen_or_pending(surface.as_ref())
        })
}

/// Begins a camera pan for `pan-field-always`.
///
/// `pan-field` starts only on empty background because, bound to a bare
/// button, it would otherwise hijack every click. This variant is meant for a
/// modified chord and starts over windows, their titlebars and borders,
/// collapsed nodes, and desktop-level layer surfaces as well. The drag then
/// reuses [`Grab::Pan`](crate::input::grab::Grab::Pan), so motion, momentum,
/// zoom scaling, and release behave exactly like an ordinary background pan.
///
/// Every refusal returns `false` before any state changes, so the press
/// reaches the client precisely as if the chord were unbound. That is what
/// keeps fullscreen apps, games holding a pointer lock or confinement, and a
/// client's in-progress drag untouched.
fn begin_field_pan_always<D: SessionDriver>(
    session: &mut Session<D>,
    route: &crate::input::pointer::PointerRoute,
    button: u32,
    serial: smithay::utils::Serial,
) -> bool {
    let target_allowed = match &route.target {
        crate::input::pointer::PointerTarget::Background => true,
        crate::input::pointer::PointerTarget::Layer(layer) => {
            layer_allows_field_pan_always(layer.layer())
        }
        crate::input::pointer::PointerTarget::Window(window)
        | crate::input::pointer::PointerTarget::Decoration { window, .. } => {
            window_allows_field_pan_always(session, window)
        }
    };
    if !target_allowed {
        return false;
    }
    // Fullscreen, Field maximize, and an open cluster workspace own the camera.
    // Starting a grab the motion handler cannot honor would only eat the click.
    if !session.cameras.is_pannable(&route.output.name()) {
        return false;
    }
    // A client that is mid-drag or holds a pointer lock/confinement keeps the
    // pointer; a pan started now would freeze or fight it.
    if session
        .seat
        .get_pointer()
        .is_some_and(|pointer| pointer.is_grabbed())
        || super::pointer::has_active_constraint(session)
    {
        return false;
    }
    if !start_field_pan_grab(session, route, button) {
        return false;
    }
    wayland::focus::select_output(&mut session.wayland, &route.output);
    // Dragging empty background drops layer keyboard focus, exactly as
    // `pan-field` does. Over a window the drag is navigation, not interaction,
    // so keyboard focus stays where it was.
    if matches!(
        &route.target,
        crate::input::pointer::PointerTarget::Background
    ) {
        super::focus::focus_layer(session, None, serial);
    }
    true
}

fn start_field_pan_grab<D: SessionDriver>(
    session: &mut Session<D>,
    route: &crate::input::pointer::PointerRoute,
    button: u32,
) -> bool {
    if !session
        .interactions
        .grab
        .try_begin_pan(route.output.name(), button)
    {
        return false;
    }
    session.cursor.set_override(
        crate::cursor::OverrideSource::Grab,
        Some(smithay::input::pointer::CursorIcon::Grabbing),
    );
    true
}

pub(crate) fn tick_grabbed_window_edge_pan<D: SessionDriver>(
    session: &mut Session<D>,
    now: std::time::Duration,
) -> Option<String> {
    let (id, window, drag_size, anchor, edge_pan) = match &session.interactions.grab {
        crate::input::grab::Grab::MoveWindow {
            id: Some(id),
            window,
            cluster_drag: None,
            drag_size,
            anchor,
            edge_pan: Some(edge_pan),
            ..
        } => (*id, window.clone(), *drag_size, *anchor, edge_pan.clone()),
        _ => return None,
    };
    let output = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == edge_pan.output)?
        .clone();
    let output_geometry = session.wayland.space.output_geometry(&output)?;
    let camera = *session.cameras.get(&edge_pan.output)?;
    let size = drag_size.unwrap_or_else(|| {
        session
            .wayland
            .space
            .element_geometry(&window)
            .map(|geometry| geometry.size)
            .unwrap_or((1, 1).into())
    });
    let frame_extents = crate::titlebar::frame_extents(
        &window,
        &session.settings.decorations,
        &session.settings.font,
    );
    let placement = crate::input::grab::window_edge_pan_placement(
        session.pointer.position(),
        anchor,
        size,
        frame_extents,
        &camera,
        output_geometry,
    );
    let Some(placement) = placement else {
        if let crate::input::grab::Grab::MoveWindow {
            edge_pan: Some(live),
            ..
        } = &mut session.interactions.grab
        {
            live.update_contact(halley_core::field::Vec2 { x: 0.0, y: 0.0 }, now);
            live.last_tick = now;
        }
        return None;
    };

    let (ready, dt, contact) = match &mut session.interactions.grab {
        crate::input::grab::Grab::MoveWindow {
            edge_pan: Some(live),
            last_world,
            last_update,
            velocity,
            ..
        } => {
            live.update_contact(placement.contact, now);
            let ready = live.ready(now);
            let dt = now
                .saturating_sub(live.last_tick)
                .as_secs_f32()
                .min(1.0 / 20.0);
            live.last_tick = now;
            *last_world = placement.center;
            *last_update = now;
            *velocity = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
            (ready, dt, live.contact)
        }
        _ => return None,
    };
    if contact.x == 0.0 && contact.y == 0.0 {
        return None;
    }

    if !session.nodes.physics.enabled {
        let _ = crate::nodes::move_grabbed_body_rigid(session, id, placement.center);
    }
    if ready && dt > 0.0 {
        let speed = crate::input::grab::WINDOW_EDGE_PAN_SPEED_PXPS
            / crate::presentation::camera::scale(&camera).max(0.05);
        if let Some(camera) = session.cameras.get_mut(&edge_pan.output) {
            camera.pan_target(halley_core::field::Vec2 {
                x: contact.x * speed * dt,
                y: contact.y * speed * dt,
            });
        }
    }
    Some(edge_pan.output)
}

pub(crate) fn grabbed_window_edge_pan_active_on<D: SessionDriver>(
    session: &Session<D>,
    output_name: &str,
) -> bool {
    matches!(
        &session.interactions.grab,
        crate::input::grab::Grab::MoveWindow {
            edge_pan: Some(edge_pan),
            ..
        } if edge_pan.output == output_name
            && (edge_pan.contact.x != 0.0 || edge_pan.contact.y != 0.0)
    )
}

fn output_at_pointer(
    space: &Space<Window>,
    position: (f64, f64),
) -> Option<(Output, Rectangle<i32, Logical>)> {
    let output = space.output_under(position).next()?.clone();
    let geometry = space.output_geometry(&output)?;
    Some((output, geometry))
}

fn work_area_for_output(
    space: &Space<Window>,
    output_name: &str,
) -> Option<Rectangle<i32, Logical>> {
    let output = space
        .outputs()
        .find(|output| output.name() == output_name)?;
    Some(smithay::desktop::layer_map_for_output(output).non_exclusive_zone())
}

fn cluster_exclusive_on_output<D: SessionDriver>(
    session: &Session<D>,
    output: &Output,
    geometry: Rectangle<i32, Logical>,
    now: std::time::Duration,
) -> bool {
    crate::presentation::window::cluster_exclusive_presentation(
        &session.clusters,
        &session.nodes,
        &session.fullscreen,
        &session.maximize,
        output,
        geometry,
        now,
    )
    .is_some_and(|presentation| presentation.progress > 0.0)
}

fn node_at_pointer<D: SessionDriver>(
    session: &Session<D>,
) -> Option<(halley_core::field::NodeId, Output)> {
    let position = session.pointer.position();
    let (output, geometry) = output_at_pointer(&session.wayland.space, position)?;
    if super::pointer::presentation_covers_field_point(
        session,
        &output,
        Point::from(position),
        crate::frame_clock::monotonic_now(),
    ) || cluster_exclusive_on_output(
        session,
        &output,
        geometry,
        crate::frame_clock::monotonic_now(),
    ) {
        return None;
    }
    let camera = session.cameras.get(&output.name())?;
    let id = session.nodes.hit_test(
        &output,
        geometry,
        camera,
        Point::<f64, Logical>::from(position),
    )?;
    Some((id, output))
}

fn cluster_at_pointer<D: SessionDriver>(
    session: &Session<D>,
) -> Option<(halley_core::cluster::ClusterId, Output)> {
    let position = session.pointer.position();
    let (output, geometry) = output_at_pointer(&session.wayland.space, position)?;
    if super::pointer::presentation_covers_field_point(
        session,
        &output,
        Point::from(position),
        crate::frame_clock::monotonic_now(),
    ) || cluster_exclusive_on_output(
        session,
        &output,
        geometry,
        crate::frame_clock::monotonic_now(),
    ) {
        return None;
    }
    let camera = session.cameras.get(&output.name())?;
    let id = session.clusters.core_hit_test(
        &output.name(),
        camera,
        geometry,
        Point::<f64, Logical>::from(position),
    )?;
    Some((id, output))
}

fn cluster_overflow_at_pointer<D: SessionDriver>(
    session: &Session<D>,
    now: std::time::Duration,
) -> Option<(halley_core::field::NodeId, Output, Point<f64, Logical>)> {
    let position = session.pointer.position();
    let (output, geometry) = output_at_pointer(&session.wayland.space, position)?;
    if cluster_exclusive_on_output(session, &output, geometry, now) {
        return None;
    }
    let work_area = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
    let local = Point::<f64, Logical>::from((
        position.0 - f64::from(geometry.loc.x),
        position.1 - f64::from(geometry.loc.y),
    ));
    let member = session
        .clusters
        .overflow_hit_test(&output.name(), work_area, local, now)?;
    Some((member, output, local))
}

fn cluster_action_at_pointer<D: SessionDriver>(
    session: &Session<D>,
    now: std::time::Duration,
) -> Option<(
    halley_core::cluster::ClusterId,
    crate::clusters::ClusterActionControl,
    Output,
)> {
    let position = session.pointer.position();
    let (output, geometry) = output_at_pointer(&session.wayland.space, position)?;
    if cluster_exclusive_on_output(session, &output, geometry, now) {
        return None;
    }
    let output_name = output.name();
    let cluster = session.clusters.bloom_edit_target_on_output(&output_name)?;
    let metadata = session.clusters.metadata(cluster)?;
    let core_position =
        session
            .clusters
            .core_node(cluster)
            .map_or(metadata.core_position, |core| {
                session
                    .nodes
                    .landmark_position(core, metadata.core_position, now)
            });
    let camera = session.cameras.get(&output_name)?;
    let center = crate::nodes::screen_from_world(core_position, camera, geometry);
    crate::clusters::action_button_rects(center, geometry)
        .into_iter()
        .find_map(|(control, rect)| {
            rect.to_f64()
                .contains(Point::<f64, Logical>::from(position))
                .then_some((cluster, control, output.clone()))
        })
}

fn cluster_bloom_at_pointer<D: SessionDriver>(
    session: &Session<D>,
    now: std::time::Duration,
) -> Option<(crate::clusters::TokenLayout, Output)> {
    let position = session.pointer.position();
    let (output, geometry) = output_at_pointer(&session.wayland.space, position)?;
    if cluster_exclusive_on_output(session, &output, geometry, now) {
        return None;
    }
    let camera = session.cameras.get(&output.name())?;
    let token = session.clusters.bloom_hit_test(
        &output.name(),
        camera,
        geometry,
        Point::<f64, Logical>::from(position),
        now,
    )?;
    Some((token, output))
}

struct OverflowHover {
    output: Output,
    member: Option<halley_core::field::NodeId>,
    intercepts_desktop: bool,
    changed: bool,
}

fn update_overflow_hover<D: SessionDriver>(
    session: &mut Session<D>,
    now: std::time::Duration,
) -> Option<OverflowHover> {
    let position = session.pointer.position();
    let (output, geometry) = output_at_pointer(&session.wayland.space, position)?;
    if cluster_exclusive_on_output(session, &output, geometry, now) {
        return None;
    }
    let output_name = output.name();
    if !session.clusters.has_overflow(&output_name) {
        return None;
    }
    let work_area = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
    let local = Point::<f64, Logical>::from((
        position.0 - f64::from(geometry.loc.x),
        position.1 - f64::from(geometry.loc.y),
    ));
    let at_reveal_edge =
        local.x >= f64::from(work_area.loc.x + work_area.size.w) - crate::clusters::REVEAL_EDGE_PX;
    let over_strip = session
        .clusters
        .overflow_strip_contains(&output_name, work_area, local, now);
    let changed = if at_reveal_edge || over_strip {
        session.clusters.reveal_overflow(&output_name, now)
    } else {
        false
    };
    let member = session
        .clusters
        .overflow_hit_test(&output_name, work_area, local, now);
    Some(OverflowHover {
        output,
        member,
        intercepts_desktop: at_reveal_edge || over_strip || member.is_some(),
        changed,
    })
}

fn scroll_cluster_overflow_at_pointer<D, B, E>(
    session: &mut Session<D>,
    event: &E,
    now: std::time::Duration,
) -> bool
where
    D: SessionDriver,
    B: InputBackend,
    E: PointerAxisEvent<B>,
{
    let position = session.pointer.position();
    let Some((output, geometry)) = output_at_pointer(&session.wayland.space, position) else {
        return false;
    };
    if cluster_exclusive_on_output(session, &output, geometry, now) {
        return false;
    }
    let output_name = output.name();
    let work_area = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
    let local = Point::<f64, Logical>::from((
        position.0 - f64::from(geometry.loc.x),
        position.1 - f64::from(geometry.loc.y),
    ));
    if !session
        .clusters
        .overflow_strip_contains(&output_name, work_area, local, now)
    {
        return false;
    }

    let delta_steps = event
        .amount_v120(Axis::Vertical)
        .map(|value| value / 120.0)
        .or_else(|| event.amount(Axis::Vertical).map(|value| value / 15.0));
    let Some(delta_steps) = delta_steps else {
        return false;
    };
    session.clusters.scroll_overflow(
        &output_name,
        work_area,
        delta_steps,
        delta_steps == 0.0,
        now,
    )
}

pub(crate) fn cluster_owns_focus<D: SessionDriver>(
    session: &Session<D>,
    id: halley_core::cluster::ClusterId,
) -> bool {
    let logical = session.nodes.focused().is_some_and(|focused| {
        session.clusters.cluster_for_member(focused) == Some(id)
            || session.clusters.cluster_for_core(focused) == Some(id)
    });
    let keyboard = session
        .seat
        .get_keyboard()
        .and_then(|keyboard| keyboard.current_focus())
        .and_then(|focus| focus.wl_surface().map(|surface| surface.into_owned()))
        .and_then(|surface| session.nodes.id_for_surface(&surface))
        .is_some_and(|focused| session.clusters.cluster_for_member(focused) == Some(id));
    logical || keyboard
}

pub(crate) fn show_cluster_indicator<D: SessionDriver>(
    session: &mut Session<D>,
    id: halley_core::cluster::ClusterId,
    now: std::time::Duration,
) {
    let Some((output, name, layout)) = session.clusters.metadata(id).map(|metadata| {
        (
            metadata.output.clone(),
            metadata.name.clone(),
            metadata.layout,
        )
    }) else {
        return;
    };
    session
        .shell
        .overlays
        .show_cluster_indicator(&output, &name, layout, now);
}

fn activation_shows_cluster_indicator(first_member: Option<halley_core::field::NodeId>) -> bool {
    first_member.is_none()
}

pub(crate) fn sync_cluster_activation_focus<D: SessionDriver>(
    session: &mut Session<D>,
    output: &Output,
    id: halley_core::cluster::ClusterId,
    collapsed_should_focus: bool,
    serial: smithay::utils::Serial,
) {
    let now = crate::frame_clock::monotonic_now();
    let output_name = output.name();
    super::sync_cluster_camera(session, &output_name, now);
    if session.clusters.active_on(&output_name) == Some(id) {
        let first_member = session.clusters.first_member(id);
        if let Some(member) = first_member {
            if let Some(window) = session
                .nodes
                .record(member)
                .map(|record| record.window.clone())
            {
                super::focus_window(session, &window, serial);
            }
        } else {
            // An empty workspace has no client surface to own keyboard focus,
            // but its persistent core is still the logical selection. Identify
            // the otherwise blank workspace; populated clusters reveal their
            // contents directly and do not need an activation card.
            crate::window::clear_focus(&mut session.wayland);
            session.nodes.focus(
                session.clusters.core_node(id),
                session.start_time.elapsed().as_millis() as u64,
            );
            super::sync_keyboard_focus(session, serial);
        }
        if activation_shows_cluster_indicator(first_member) {
            show_cluster_indicator(session, id, now);
        }
    } else if collapsed_should_focus {
        crate::window::clear_focus(&mut session.wayland);
        session.nodes.focus(
            session.clusters.core_node(id),
            session.start_time.elapsed().as_millis() as u64,
        );
        super::sync_keyboard_focus(session, serial);
    }
}

fn close_blooms_for_keybind<D: SessionDriver>(
    session: &mut Session<D>,
    preferred_output: Option<&str>,
) -> bool {
    let open = session
        .wayland
        .space
        .outputs()
        .filter_map(|output| {
            let output_name = output.name();
            let cluster = session.clusters.bloom_open_on_output(&output_name)?;
            let core = session.clusters.core_node(cluster)?;
            Some((output_name, core))
        })
        .collect::<Vec<_>>();
    if open.is_empty() {
        return false;
    }
    let focus_core = preferred_output
        .and_then(|preferred| {
            open.iter()
                .find(|(output, _)| output == preferred)
                .map(|(_, core)| *core)
        })
        .or_else(|| {
            let focused = session.nodes.focused()?;
            open.iter()
                .find(|(_, core)| *core == focused)
                .map(|(_, core)| *core)
        })
        .or_else(|| open.first().map(|(_, core)| *core));
    let now = crate::frame_clock::monotonic_now();
    let changed = open.iter().fold(false, |changed, (output, _)| {
        session.clusters.close_bloom(output, now) || changed
    });
    if changed {
        crate::window::clear_focus(&mut session.wayland);
        session
            .nodes
            .focus(focus_core, session.start_time.elapsed().as_millis() as u64);
        super::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
        session.request_redraw();
    }
    changed
}

fn close_blooms_for_typing_away<D: SessionDriver>(session: &mut Session<D>) -> bool {
    let window_drag_active = matches!(
        &session.interactions.grab,
        crate::input::grab::Grab::PendingWindowMove(_)
            | crate::input::grab::Grab::MoveWindow { .. }
    );
    let focused = session
        .seat
        .get_keyboard()
        .and_then(|keyboard| keyboard.current_focus())
        .and_then(|focus| focus.wl_surface().map(|surface| surface.into_owned()))
        .and_then(|surface| session.nodes.id_for_surface(&surface));
    let open = session
        .wayland
        .space
        .outputs()
        .filter_map(|output| {
            let output = output.name();
            let cluster = session.clusters.bloom_open_on_output(&output)?;
            Some((output, cluster))
        })
        .collect::<Vec<_>>();
    let now = crate::frame_clock::monotonic_now();
    let changed = open.into_iter().fold(false, |changed, (output, cluster)| {
        let focused_owns_bloom = focused.is_some_and(|node| {
            session.clusters.cluster_for_member(node) == Some(cluster)
                || session.clusters.cluster_for_core(node) == Some(cluster)
        });
        if typing_abandons_bloom(window_drag_active, focused_owns_bloom) {
            session.clusters.close_bloom(&output, now) || changed
        } else {
            changed
        }
    });
    if changed {
        session.request_redraw();
    }
    changed
}

pub(crate) fn begin_cluster_commit<D: SessionDriver>(session: &mut Session<D>) -> bool {
    if !session.shell.cluster_composer.accepts_input() {
        return false;
    }
    let focused = session.nodes.focused();
    let output = session
        .clusters
        .creation()
        .map(|creation| creation.output.clone());
    let fallback_core_position = output
        .as_deref()
        .and_then(|output_name| {
            let output = session
                .wayland
                .space
                .outputs()
                .find(|candidate| candidate.name() == output_name)?;
            let geometry = session.wayland.space.output_geometry(output)?;
            let view = session.cameras.view(output_name)?;
            Some(halley_core::field::Vec2 {
                x: geometry.loc.x as f32 + view.center.x,
                y: geometry.loc.y as f32 + view.center.y,
            })
        })
        .unwrap_or(halley_core::field::Vec2 { x: 0.0, y: 0.0 });
    let now = crate::frame_clock::monotonic_now();
    match session
        .clusters
        .prepare_creation(&session.nodes.field, focused, fallback_core_position)
    {
        Ok(prepared) => {
            if session
                .shell
                .cluster_composer
                .begin_commit(prepared, session.settings.apogee, now)
            {
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Modal, None);
                session.request_redraw();
                true
            } else {
                session.clusters.abort_prepared_creation();
                false
            }
        }
        Err(message) => {
            eventline::warn!("clusters: {message}");
            if let Some(output) = output {
                session
                    .shell
                    .overlays
                    .show_error(output, &message, 3_000, now);
            }
            session.request_redraw();
            false
        }
    }
}

pub(crate) fn finish_cluster_creation<D: SessionDriver>(session: &mut Session<D>) -> bool {
    if session.clusters.renaming_target().is_some() {
        let output = session
            .clusters
            .creation()
            .map(|creation| creation.output.clone());
        return match session.clusters.finish_rename() {
            Ok(_) => {
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Modal, None);
                session.request_redraw();
                true
            }
            Err(message) => {
                eventline::warn!("clusters: {message}");
                if let Some(output) = output {
                    session.shell.overlays.show_error(
                        output,
                        &message,
                        3_000,
                        crate::frame_clock::monotonic_now(),
                    );
                }
                session.request_redraw();
                false
            }
        };
    }
    let draft_id = session.clusters.creation_draft_id();
    if let Some(confirmation) = session
        .clusters
        .confirm_draft(crate::frame_clock::monotonic_now())
    {
        let Some(wayland_display) = session.wayland_display.clone() else {
            eventline::warn!("clusters: cannot launch draft apps before Wayland is ready");
            return false;
        };
        let x11_display = session.xwayland.display_name();
        for launch in confirmation.launches {
            if !launch.command.trim().is_empty() {
                super::spawn::spawn_detached(
                    &launch.command,
                    &wayland_display,
                    x11_display.as_deref(),
                    session.cursor.size(),
                    &session.launch_environment,
                );
            }
        }
        session
            .cursor
            .set_override(crate::cursor::OverrideSource::Modal, None);
        crate::ipc::publish_cluster_draft(
            session,
            confirmation.id,
            halley_ipc::ClusterDraftState::Launching,
            None,
        );
        session.request_redraw();
        return true;
    }
    let prepared = session.clusters.prepared_creation().cloned();
    let focused_before = session.nodes.focused();
    match session.clusters.finish_creation(&mut session.nodes.field) {
        Ok(id) => {
            crate::nodes::resolve_new_cluster_core(session, id);
            let focus_core = prepared.map_or_else(
                || {
                    focused_before.is_some_and(|focused| {
                        session.clusters.cluster_for_member(focused) == Some(id)
                    })
                },
                |prepared| prepared.focus_core,
            );
            if focus_core {
                crate::window::clear_focus(&mut session.wayland);
                session.nodes.focus(
                    session.clusters.core_node(id),
                    session.start_time.elapsed().as_millis() as u64,
                );
            }
            super::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
            session
                .cursor
                .set_override(crate::cursor::OverrideSource::Modal, None);
            if let Some(draft_id) = draft_id {
                crate::ipc::publish_cluster_draft(
                    session,
                    draft_id,
                    halley_ipc::ClusterDraftState::Completed,
                    None,
                );
            }
            true
        }
        Err(message) => {
            eventline::warn!("clusters: {message}");
            false
        }
    }
}

fn bloom_drag_handoff(
    window_location: Point<i32, Logical>,
    window_size: smithay::utils::Size<i32, Logical>,
    pointer_world: halley_core::field::Vec2,
) -> (halley_core::field::Vec2, halley_core::field::Vec2) {
    let source_offset = halley_core::field::Vec2 {
        x: window_location.x as f32 - pointer_world.x,
        y: window_location.y as f32 - pointer_world.y,
    };
    let center = halley_core::field::Vec2 {
        x: window_location.x as f32 + window_size.w as f32 * 0.5,
        y: window_location.y as f32 + window_size.h as f32 * 0.5,
    };
    (source_offset, center)
}

pub(crate) fn wakeup_cluster_interactions<D: SessionDriver>(
    session: &mut Session<D>,
    now: std::time::Duration,
) -> bool {
    let mut changed = session.clusters.repeat_name_input_if_due(now);
    changed |= super::expire_cluster_draft(session, now);
    changed |= session.clusters.overflow_wakeup(now);
    changed |= session.clusters.tick_join_candidate_ready(now);

    let Some((cluster_id, member_id, output_name, tether_started)) = session.clusters.bloom_pull()
    else {
        return changed;
    };
    let Some(tether_started) = tether_started else {
        return changed;
    };
    if now.saturating_sub(tether_started) < crate::clusters::DETACH_HOLD_DURATION {
        return changed;
    }
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)
        .cloned()
    else {
        session.clusters.clear_bloom_pull();
        return true;
    };
    let Some(output_geometry) = session.wayland.space.output_geometry(&output) else {
        session.clusters.clear_bloom_pull();
        return true;
    };
    let Some(camera) = session.cameras.get(&output_name) else {
        session.clusters.clear_bloom_pull();
        return true;
    };
    let position = session.pointer.position();
    let world = crate::input::grab::screen_to_world_on_output(position, camera, output_geometry);
    if !session
        .clusters
        .detach_member(&mut session.nodes.field, cluster_id, member_id, world, now)
    {
        session.clusters.clear_bloom_pull();
        return true;
    }
    let _ = session.clusters.begin_field_drag(member_id);
    session.clusters.force_close_bloom(&output_name);
    session.clusters.set_overlay_hovered(None);
    crate::nodes::set_collapsed_output(session, member_id, &output);
    session.nodes.clear_direct_motion(member_id);
    let _ = crate::nodes::move_grabbed_body_rigid(session, member_id, world);
    if let Some(record) = session.nodes.record(member_id).cloned() {
        wayland::set_window_output(&record.window, &output);
        let window_location = session
            .wayland
            .space
            .element_location(&record.window)
            .unwrap_or(record.geometry.loc);
        let window_size = session
            .wayland
            .space
            .element_geometry(&record.window)
            .map(|geometry| geometry.size)
            .unwrap_or(record.geometry.size);
        let (source_offset, center) = bloom_drag_handoff(window_location, window_size, world);
        session.interactions.grab = crate::input::grab::Grab::MoveWindow {
            id: Some(member_id),
            window: record.window.clone(),
            cluster_drag: None,
            drag_size: None,
            button: BTN_LEFT,
            client_owned: false,
            anchor: crate::input::grab::WindowGrabAnchor::Source(source_offset),
            edge_pan: None,
            last_world: center,
            last_update: now,
            velocity: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
        };
        // The bloom press was compositor-only. Retire its suppression at the
        // handoff so the physical release reaches the MoveWindow cleanup
        // below; that cleanup still intercepts it, so no orphan release is
        // forwarded to the newly focused client.
        session
            .interactions
            .suppressed_buttons
            .release_is_suppressed(BTN_LEFT);
        super::focus_window(session, &record.window, SERIAL_COUNTER.next_serial());
    }
    session.cursor.set_override(
        crate::cursor::OverrideSource::Grab,
        Some(smithay::input::pointer::CursorIcon::Grabbing),
    );
    true
}

fn apply_active_resize<D: SessionDriver>(session: &mut Session<D>) -> bool {
    let crate::input::grab::Grab::ResizeWindow(state) = &session.interactions.grab else {
        return false;
    };
    let state = state.clone();
    let size = crate::input::grab::resize_preview_size(&state);
    let primary = session.driver.primary_output();
    let output = session
        .wayland
        .space
        .outputs()
        .find(|output| wayland::window_is_on_output(&state.window, output, primary))
        .cloned()
        .unwrap_or_else(|| primary.clone());
    let Some(output_geometry) = session.wayland.space.output_geometry(&output) else {
        return false;
    };
    let member = state
        .window
        .wl_surface()
        .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
        .filter(|member| session.clusters.is_member_floating(*member));
    let floating_target = member.and_then(|member| {
        let location = crate::input::grab::resize_location_after_commit(
            state.handle,
            state.start_rect.loc,
            state.start_rect.size,
            size,
        );
        let work_area = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
        let local = Rectangle::new(location - output_geometry.loc, size);
        session
            .clusters
            .update_member_floating_rect(&output.name(), member, local, work_area);
        let local = session.clusters.member_floating_rect(member)?;
        let global = Rectangle::new(output_geometry.loc + local.loc, local.size);
        let current = session
            .wayland
            .space
            .element_geometry(&state.window)
            .unwrap_or(state.start_rect);
        let _ = session
            .clusters
            .prepare_surface_target(member, current, global);
        Some(global)
    });
    if size != state.start_rect.size
        && let Some(surface) = state.window.wl_surface()
    {
        session
            .interactions
            .field_arrange
            .invalidate_surface(surface.as_ref());
    }
    if let Some(toplevel) = state.window.toplevel() {
        let size = floating_target.map_or(size, |target| target.size);
        toplevel.with_pending_state(|pending| pending.size = Some(size));
        let serial = toplevel.send_pending_configure();
        crate::input::grab::note_resize_configure(&mut session.interactions.resize_anchor, serial);
    } else {
        let target = floating_target.unwrap_or_else(|| {
            let location = crate::input::grab::resize_location_after_commit(
                state.handle,
                state.start_rect.loc,
                state.start_rect.size,
                size,
            );
            Rectangle::new(location, size)
        });
        crate::xwayland::configure_window(session, &state.window, target);
    }
    true
}

pub(crate) fn wakeup_smooth_resize<D: SessionDriver>(
    session: &mut Session<D>,
    now: std::time::Duration,
) -> bool {
    let animations = &session.settings.animations;
    let changed = match &mut session.interactions.grab {
        crate::input::grab::Grab::ResizeWindow(state) => {
            crate::input::grab::advance_resize_preview(
                state,
                now,
                animations.enabled && animations.smooth_resize.enabled,
                animations.smooth_resize.duration_ms,
            )
        }
        _ => false,
    };
    changed && apply_active_resize(session)
}

fn navigate_cluster<D: SessionDriver>(
    session: &mut Session<D>,
    output_name: &str,
    direction: halley_config::Direction,
    swap: bool,
) {
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)
        .cloned()
    else {
        return;
    };
    let work_area = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
    // Active cluster members are hidden from the Field scene, so logical
    // `NodesState` focus deliberately rejects them.  The focused client
    // surface remains authoritative inside a workspace, including for X11
    // windows whose keyboard target is an XWayland surface.
    let client_focused = session
        .wayland
        .focused_window
        .as_ref()
        .and_then(|surface| session.nodes.id_for_surface(surface));
    let focused = preferred_cluster_navigation_focus(client_focused, session.nodes.focused());
    let target = if swap {
        session.clusters.swap_directional_tile(
            output_name,
            focused,
            direction,
            work_area,
            crate::frame_clock::monotonic_now(),
        )
    } else {
        let layout = session
            .clusters
            .active_on(output_name)
            .and_then(|id| session.clusters.metadata(id))
            .map(|metadata| metadata.layout);
        match (layout, stacking_cycle_direction(direction)) {
            (
                Some(halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Stacking),
                Some(cycle_direction),
            ) => match session.clusters.cycle_stack(
                output_name,
                cycle_direction,
                work_area,
                crate::frame_clock::monotonic_now(),
            ) {
                crate::clusters::StackCycleOutcome::Cycled(member) => Some(member),
                crate::clusters::StackCycleOutcome::NotActive
                | crate::clusters::StackCycleOutcome::Unchanged => None,
            },
            (Some(halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Stacking), _) => None,
            _ => {
                session
                    .clusters
                    .directional_tile_target(output_name, focused, direction, work_area)
            }
        }
    };
    if let Some(window) = target
        .and_then(|id| session.nodes.record(id))
        .map(|record| record.window.clone())
    {
        super::focus_window(session, &window, SERIAL_COUNTER.next_serial());
        session.request_redraw();
    }
}

fn preferred_cluster_navigation_focus(
    client_focused: Option<halley_core::field::NodeId>,
    logical_focused: Option<halley_core::field::NodeId>,
) -> Option<halley_core::field::NodeId> {
    client_focused.or(logical_focused)
}

fn stacking_cycle_direction(
    direction: halley_config::Direction,
) -> Option<halley_config::FocusCycleDirection> {
    match direction {
        halley_config::Direction::Left => Some(halley_config::FocusCycleDirection::Forward),
        halley_config::Direction::Right => Some(halley_config::FocusCycleDirection::Backward),
        halley_config::Direction::Up | halley_config::Direction::Down => None,
    }
}

fn focus_output_target<D: SessionDriver>(
    session: &mut Session<D>,
    target: halley_config::MonitorTarget,
) {
    let target = match target {
        halley_config::MonitorTarget::Direction(direction) => {
            let Some(current) = crate::wayland::focus::selected_output(&session.wayland).cloned()
            else {
                return;
            };
            let Some(target) =
                crate::wayland::focus::adjacent_output(&session.wayland.space, &current, direction)
            else {
                return;
            };
            target
        }
        halley_config::MonitorTarget::Output(name) => {
            let Some(target) = session
                .wayland
                .space
                .outputs()
                .find(|output| output.name() == name)
                .cloned()
            else {
                eventline::warn!("monitor focus: unknown output {name:?}");
                return;
            };
            target
        }
    };
    crate::wayland::focus::select_output(&mut session.wayland, &target);

    let target_id = session.nodes.focused_on_output(&target.name());
    let target_record = target_id.and_then(|id| session.nodes.record(id)).cloned();
    match (target_id, target_record) {
        (Some(id), Some(record)) if record.collapsed => {
            crate::window::clear_focus(&mut session.wayland);
            session
                .nodes
                .focus(Some(id), session.start_time.elapsed().as_millis() as u64);
            super::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
        }
        (_, Some(record)) => {
            super::focus_window(session, &record.window, SERIAL_COUNTER.next_serial());
        }
        _ => {
            crate::window::clear_focus(&mut session.wayland);
            session
                .nodes
                .focus(None, session.start_time.elapsed().as_millis() as u64);
            super::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
        }
    }
    session.request_redraw();
}

fn toggle_cluster_or_focused_node<D: SessionDriver>(
    session: &mut Session<D>,
    output_name: Option<&str>,
    serial: smithay::utils::Serial,
) {
    let focused_core = session.nodes.focused().and_then(|core| {
        session
            .clusters
            .cluster_for_core(core)
            .map(|cluster| (core, cluster))
    });
    let active = output_name.and_then(|output| {
        session
            .clusters
            .active_on(output)
            .map(|cluster| (output.to_string(), cluster))
    });
    let target = active.or_else(|| {
        let (_, cluster) = focused_core?;
        let metadata = session.clusters.metadata(cluster)?;
        output_name
            .is_none_or(|output| output == metadata.output)
            .then(|| (metadata.output.clone(), cluster))
    });
    if let Some((output_name, cluster)) = target {
        let owned_focus = cluster_owns_focus(session, cluster);
        if session
            .clusters
            .activate(&output_name, cluster, crate::frame_clock::monotonic_now())
        {
            let output = {
                session
                    .wayland
                    .space
                    .outputs()
                    .find(|output| output.name() == output_name)
                    .cloned()
            };
            if let Some(output) = output {
                sync_cluster_activation_focus(session, &output, cluster, owned_focus, serial);
            }
            session.request_redraw();
            return;
        }
    }
    crate::nodes::toggle_focused_on_output(session, output_name, serial);
}

fn bearing_at_pointer(
    pointer: &crate::input::pointer::Pointer,
    space: &Space<Window>,
    bearings: &crate::shell::bearings::BearingsState,
) -> Option<(crate::shell::bearings::BearingTarget, Output)> {
    let position = pointer.position();
    let (output, geometry) = output_at_pointer(space, position)?;
    let local = Point::<f64, Logical>::from((
        position.0 - f64::from(geometry.loc.x),
        position.1 - f64::from(geometry.loc.y),
    ));
    let target = bearings.hit_test(&output.name(), local)?;
    Some((target, output))
}

pub fn handle<D, B>(session: &mut Session<D>, event: &InputEvent<B>, socket_name: &OsStr)
where
    D: SessionDriver,
    B: InputBackend,
{
    let steam_close_press = match event {
        InputEvent::PointerButton { event } => take_steam_close_press(
            &mut session.interactions.steam_close_pressed,
            event.button_code(),
        ),
        InputEvent::DeviceRemoved { .. } => {
            session.interactions.steam_close_pressed = None;
            session.interactions.basics_buttons.clear();
            session.interactions.basics_touches.clear();
            None
        }
        _ => None,
    };
    if is_user_activity(event) {
        let seat = session.seat.clone();
        session.idle_notifier_state.notify_activity(&seat);
    }
    if session.session_lock.active() {
        session.interactions.steam_close_pressed = None;
        crate::wayland::session_lock::handle_input(session, event);
        return;
    }
    if let InputEvent::PointerButton { event } = event
        && event.state() == ButtonState::Released
        && session
            .interactions
            .screenshot_buttons
            .release_is_suppressed(event.button_code())
    {
        return;
    }
    if modal::intercept_early(session, event) {
        return;
    }
    if super::touch::handle(session, event) || super::gesture::handle(session, event) {
        return;
    }
    let position_before = session.pointer.position();
    if matches!(
        event,
        InputEvent::PointerMotion { .. }
            | InputEvent::PointerMotionAbsolute { .. }
            | InputEvent::PointerButton { .. }
            | InputEvent::PointerAxis { .. }
    ) && session.cursor_policy.pointer_activity()
    {
        session.request_redraw();
    }
    let pointer_handle = session
        .seat
        .get_pointer()
        .expect("pointer capability added at seat setup");
    super::pointer::finish_frame(session, &pointer_handle);
    session
        .pointer
        .process_input_event(event, &session.wayland.space);
    let proposed_position = session.pointer.position();
    // Modal owners claim pointer events in this order before screenshot and grabs.
    if modal::capture(session, event, proposed_position)
        || modal::cluster_composer(session, event, proposed_position)
        || modal::apogee(session, event, proposed_position)
    {
        return;
    }
    if super::screenshot::handle_pointer(session, event) {
        return;
    }
    let Some(position_after) = motion::handle(session, event, position_before, proposed_position)
    else {
        return;
    };

    if matches!(event, InputEvent::PointerButton { .. }) {
        button::handle(
            session,
            event,
            steam_close_press,
            socket_name,
            position_after,
        );
        return;
    }

    if matches!(event, InputEvent::PointerAxis { .. }) {
        axis::handle(session, event, socket_name);
        return;
    }

    if let InputEvent::Keyboard { event: key_event } = event {
        keyboard::handle::<D, B>(session, key_event, socket_name);
    }
}

fn is_user_activity<B: InputBackend>(event: &InputEvent<B>) -> bool {
    !matches!(
        event,
        InputEvent::DeviceAdded { .. }
            | InputEvent::DeviceRemoved { .. }
            | InputEvent::SwitchToggle { .. }
            | InputEvent::Special(_)
    )
}

fn update_capture_pointer<D: SessionDriver>(session: &mut Session<D>, position: (f64, f64)) {
    match session.capture.kind() {
        Some(crate::capture::CaptureKind::Menu | crate::capture::CaptureKind::Area) => {
            session.capture.motion(position);
        }
        Some(crate::capture::CaptureKind::Screen) => {
            if let Some((_, geometry)) = output_at_pointer(&session.wayland.space, position) {
                session.capture.hover_screen(geometry);
            }
        }
        Some(crate::capture::CaptureKind::Source) if session.capture.menu_is_active() => {
            session.capture.motion(position);
        }
        Some(crate::capture::CaptureKind::Source) => {
            let Some(route) = super::pointer::route_client(session) else {
                return;
            };
            let Some(output_geometry) = session.wayland.space.output_geometry(&route.output) else {
                return;
            };
            let monitor = halley_ipc::CaptureSource::Monitor {
                name: route.output.name(),
                x: output_geometry.loc.x,
                y: output_geometry.loc.y,
                width: output_geometry.size.w,
                height: output_geometry.size.h,
            };
            let window = match route.target {
                crate::input::pointer::PointerTarget::Window(window)
                | crate::input::pointer::PointerTarget::Decoration { window, .. } => {
                    window.wl_surface().and_then(|surface| {
                        let geometry = crate::capture::window_capture_visual_geometry(
                            session,
                            &window,
                            route.visual_geometry?,
                        );
                        let size = crate::capture::window_capture_size(session, &window);
                        Some((
                            halley_ipc::CaptureSource::Window {
                                window_id: session.nodes.id_for_surface(surface.as_ref())?.as_u64(),
                                width: size.w,
                                height: size.h,
                            },
                            geometry,
                        ))
                    })
                }
                _ => None,
            };
            session
                .capture
                .hover_source(monitor, window, output_geometry);
        }
        Some(crate::capture::CaptureKind::Window) => {
            let hovered = super::pointer::route_client(session).and_then(|route| {
                let window = match route.target {
                    crate::input::pointer::PointerTarget::Window(window)
                    | crate::input::pointer::PointerTarget::Decoration { window, .. } => window,
                    _ => return None,
                };
                let surface = window.wl_surface()?.into_owned();
                let geometry = crate::capture::window_capture_visual_geometry(
                    session,
                    &window,
                    route.visual_geometry?,
                );
                Some((surface, geometry))
            });
            let (surface, geometry) = hovered
                .map(|(surface, geometry)| (Some(surface), Some(geometry)))
                .unwrap_or((None, None));
            session.capture.hover_window(surface, geometry);
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use halley_core::field::Vec2;
    use smithay::backend::input::{ButtonState, KeyState, TouchSlot};
    use smithay::input::keyboard::Keysym;
    use smithay::utils::{Logical, Point, Rectangle, Size};

    use super::actions::{cluster_blocks_zoom, window_action_output};
    use super::keyboard::{
        BasicsKeyRouting, ModalKeyRouting, basics_key_routing, modal_key_routing,
    };
    use super::{
        BTN_LEFT, BTN_RIGHT, BasicsInputRouting, BasicsTouchPhase, PendingWindowMoveMotion,
    };
    use super::{
        activation_shows_cluster_indicator, begin_cluster_core_direct_motion, bloom_drag_handoff,
        collapsed_node_drop_origin, drag_threshold_reached, forward_pointer_button,
        layer_allows_field_pan_always, outside_lift_press_dismisses, pending_window_move_motion,
        plain_background_press_dismisses_bloom, pointer_move_falls_back_to_field_pan,
        preferred_cluster_navigation_focus, releases_field_pan, releases_pending_window_move,
        sampled_drag_velocity, shortcut_policy_allows_bindings, stacking_cycle_direction,
        typing_abandons_bloom,
    };
    // Model the real two-phase dispatch: consume pending state before interception,
    // then call the forwarding hook only for events delivered to the client.
    fn steam_button(
        pending: &mut Option<u32>,
        button: u32,
        state: ButtonState,
        target: Option<u32>,
        forwarded: bool,
    ) -> Option<u32> {
        let previous = super::take_steam_close_press(pending, button);
        if forwarded {
            super::forwarded_steam_close_button(pending, previous, button, state, target)
        } else {
            None
        }
    }

    #[test]
    fn steam_close_scrollbar_press_does_not_arm_close_release() {
        let mut pending = None;
        steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, None, true);
        assert_eq!(
            steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
            None
        );
    }

    #[test]
    fn steam_close_matching_click_fires_once() {
        let mut pending = None;
        assert_eq!(
            steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, Some(1), true),
            None
        );
        assert_eq!(
            steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
            Some(1)
        );
        assert_eq!(
            steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
            None
        );
    }

    #[test]
    fn steam_close_release_elsewhere_cancels() {
        for target in [None, Some(2)] {
            let mut pending = None;
            steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, Some(1), true);
            assert_eq!(
                steam_button(&mut pending, BTN_LEFT, ButtonState::Released, target, true),
                None
            );
            assert_eq!(pending, None);
        }
    }

    #[test]
    fn steam_close_intercepted_buttons_cannot_arm_or_retain_press() {
        let mut pending = None;
        steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, Some(1), false);
        assert_eq!(
            steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
            None
        );
        steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, Some(1), true);
        steam_button(
            &mut pending,
            BTN_LEFT,
            ButtonState::Released,
            Some(1),
            false,
        );
        assert_eq!(
            steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
            None
        );
    }

    #[test]
    fn steam_close_new_press_replaces_stale_target() {
        let mut pending = Some(1);
        steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, None, true);
        assert_eq!(
            steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
            None
        );
    }

    #[test]
    fn steam_close_other_buttons_neither_arm_nor_complete_left_click() {
        let mut pending = None;
        steam_button(&mut pending, BTN_RIGHT, ButtonState::Pressed, Some(1), true);
        assert_eq!(pending, None);
        steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, Some(1), true);
        assert_eq!(
            steam_button(
                &mut pending,
                BTN_RIGHT,
                ButtonState::Released,
                Some(1),
                true
            ),
            None
        );
        assert_eq!(
            steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
            Some(1)
        );
    }

    fn sample_constant_motion(report_hz: u32) -> Vec2 {
        let step = Duration::from_secs_f64(1.0 / f64::from(report_hz));
        let mut previous = Vec2 { x: 0.0, y: 0.0 };
        let mut velocity = Vec2 { x: 0.0, y: 0.0 };
        let mut last = Duration::ZERO;
        for index in 1..=report_hz {
            let now = step * index;
            let current = Vec2 {
                x: 400.0 * now.as_secs_f32(),
                y: -180.0 * now.as_secs_f32(),
            };
            velocity = sampled_drag_velocity(previous, current, velocity, last, now);
            previous = current;
            last = now;
        }
        velocity
    }

    #[test]
    fn collapsed_node_drop_origin_is_local_to_the_destination_output() {
        let output = Rectangle::<i32, Logical>::new((1_920, 0).into(), (2_560, 1_440).into());

        assert_eq!(
            collapsed_node_drop_origin(
                Vec2 {
                    x: 2_400.0,
                    y: 500.0
                },
                Size::from((800, 600)),
                output,
            ),
            Rectangle::new((80, 200).into(), (800, 600).into())
        );
    }

    #[test]
    fn drag_velocity_is_stable_across_common_mouse_report_rates() {
        for report_hz in [125, 500, 1_000] {
            let sampled = sample_constant_motion(report_hz);
            assert!((sampled.x - 400.0).abs() < 0.1, "{report_hz} Hz");
            assert!((sampled.y + 180.0).abs() < 0.1, "{report_hz} Hz");
        }
    }

    #[test]
    fn drag_velocity_ignores_duplicate_timestamps() {
        let previous_velocity = Vec2 { x: 25.0, y: -15.0 };
        let sampled = sampled_drag_velocity(
            Vec2 { x: 10.0, y: 20.0 },
            Vec2 { x: 50.0, y: 80.0 },
            previous_velocity,
            Duration::from_millis(5),
            Duration::from_millis(5),
        );
        assert_eq!(sampled, previous_velocity);
    }

    #[test]
    fn maximized_titlebar_drag_ignores_double_click_jitter() {
        let press = Point::<f64, Logical>::from((400.0, 250.0));

        assert!(!drag_threshold_reached(press, (403.0, 254.0)));
        assert!(drag_threshold_reached(press, (408.0, 250.0)));
    }

    #[test]
    fn cluster_core_drag_start_commits_its_zoom_displacement() {
        let mut nodes = crate::nodes::NodesState::new(&halley_config::RuntimeConfig::default());
        let core = nodes.field.spawn_surface(
            "cluster-core",
            Vec2 { x: 180.0, y: 120.0 },
            Vec2 { x: 48.0, y: 48.0 },
        );
        assert!(
            nodes
                .field
                .set_state(core, halley_core::field::NodeState::Core)
        );
        nodes.remember_zoom_home(core, Vec2 { x: 100.0, y: 120.0 });

        begin_cluster_core_direct_motion(&mut nodes, Some(core));

        assert_eq!(nodes.zoom_home(core), None);
    }

    #[test]
    fn move_window_grab_becomes_field_pan_on_background() {
        assert!(pointer_move_falls_back_to_field_pan(
            &crate::input::pointer::PointerTarget::Background
        ));
    }

    #[test]
    fn camera_pan_ends_only_with_the_button_that_began_it() {
        use crate::input::grab::Grab;
        const BTN_MIDDLE: u32 = 0x112;

        let pan = Grab::Pan {
            output: "DP-1".into(),
            button: BTN_MIDDLE,
        };
        assert!(releases_field_pan(&pan, BTN_MIDDLE, true));
        assert!(
            !releases_field_pan(&pan, BTN_MIDDLE, false),
            "the press that began the pan must not end it"
        );
        assert!(
            !releases_field_pan(&pan, BTN_LEFT, true),
            "releasing another button must not drop a middle-button pan"
        );
        assert!(!releases_field_pan(&pan, BTN_RIGHT, true));

        // A left-button pan (the historical `pan-field` binding) still ends on
        // the left release, and nothing but a pan is ever released here.
        let left = Grab::Pan {
            output: "DP-1".into(),
            button: BTN_LEFT,
        };
        assert!(releases_field_pan(&left, BTN_LEFT, true));
        assert!(!releases_field_pan(&left, BTN_MIDDLE, true));
        assert!(!releases_field_pan(&Grab::None, BTN_LEFT, true));
    }

    #[test]
    fn always_pan_covers_desktop_layers_but_not_panels_or_overlays() {
        use smithay::wayland::shell::wlr_layer::Layer;

        assert!(layer_allows_field_pan_always(Layer::Background));
        assert!(layer_allows_field_pan_always(Layer::Bottom));
        assert!(!layer_allows_field_pan_always(Layer::Top));
        assert!(!layer_allows_field_pan_always(Layer::Overlay));
    }

    #[test]
    fn client_titlebar_move_forwards_its_consumed_release() {
        assert!(forward_pointer_button(true, true));
        assert!(!forward_pointer_button(true, false));
        assert!(forward_pointer_button(false, false));
    }

    #[test]
    fn left_press_outside_lift_dismisses_without_claiming_inside_clicks() {
        assert!(outside_lift_press_dismisses(
            BTN_LEFT,
            ButtonState::Pressed,
            true,
            false
        ));
        assert!(!outside_lift_press_dismisses(
            BTN_LEFT,
            ButtonState::Pressed,
            true,
            true
        ));
        assert!(!outside_lift_press_dismisses(
            BTN_LEFT,
            ButtonState::Released,
            true,
            false
        ));
        assert!(!outside_lift_press_dismisses(
            BTN_LEFT,
            ButtonState::Pressed,
            false,
            false
        ));
    }

    #[test]
    fn client_titlebar_click_releases_without_activating_move() {
        assert!(releases_pending_window_move(BTN_LEFT, BTN_LEFT, true));
        assert!(!releases_pending_window_move(BTN_LEFT, BTN_LEFT, false));
        assert!(!releases_pending_window_move(BTN_LEFT, BTN_RIGHT, true));
    }

    #[test]
    fn bloom_stays_open_for_windows_and_compositor_targets_until_a_blank_click() {
        assert!(!plain_background_press_dismisses_bloom(false, false));
        assert!(!plain_background_press_dismisses_bloom(true, true));
        assert!(plain_background_press_dismisses_bloom(false, true));
    }

    #[test]
    fn typing_away_closes_a_bloom_after_the_window_drag_is_over() {
        assert!(!typing_abandons_bloom(true, false));
        assert!(!typing_abandons_bloom(false, true));
        assert!(typing_abandons_bloom(false, false));
    }

    #[test]
    fn client_titlebar_move_activates_only_after_valid_threshold_motion() {
        let press = Point::<f64, Logical>::from((400.0, 250.0));

        assert_eq!(
            pending_window_move_motion(true, press, (403.0, 254.0)),
            PendingWindowMoveMotion::Wait,
        );
        assert_eq!(
            pending_window_move_motion(true, press, (408.0, 250.0)),
            PendingWindowMoveMotion::Activate,
        );
        assert_eq!(
            pending_window_move_motion(false, press, (420.0, 250.0)),
            PendingWindowMoveMotion::Cancel,
        );
    }

    #[test]
    fn bloom_drag_handoff_preserves_a_centered_source_grip() {
        let location = Point::<i32, Logical>::from((300, 200));
        let size = Size::<i32, Logical>::from((800, 600));
        let pointer = Vec2 { x: 700.0, y: 500.0 };

        let (source_offset, center) = bloom_drag_handoff(location, size, pointer);

        assert_eq!(
            source_offset,
            Vec2 {
                x: -400.0,
                y: -300.0
            }
        );
        assert_eq!(center, pointer);
    }

    #[test]
    fn shortcut_policy_respects_session_lock_and_client_inhibition() {
        assert!(shortcut_policy_allows_bindings(false, false));
        assert!(!shortcut_policy_allows_bindings(true, false));
        assert!(!shortcut_policy_allows_bindings(false, true));
        assert!(!shortcut_policy_allows_bindings(true, true));
    }

    #[test]
    fn keyboard_actions_ignore_stationary_pointer_in_both_focus_modes() {
        for mode in [
            halley_config::FocusMode::Hover,
            halley_config::FocusMode::Click,
        ] {
            assert_eq!(
                window_action_output(
                    super::actions::DispatchOrigin::Keyboard,
                    mode,
                    Some("left"),
                    Some("right")
                ),
                Some("right".into())
            );
            assert_eq!(
                window_action_output(
                    super::actions::DispatchOrigin::Keyboard,
                    mode,
                    Some("left"),
                    None
                ),
                None
            );
        }
    }

    #[test]
    fn window_actions_follow_pointer_only_in_hover_mode() {
        assert_eq!(
            window_action_output(
                super::actions::DispatchOrigin::Other,
                halley_config::FocusMode::Hover,
                Some("right"),
                Some("left"),
            ),
            Some("right".to_string())
        );
        assert_eq!(
            window_action_output(
                super::actions::DispatchOrigin::Other,
                halley_config::FocusMode::Click,
                Some("right"),
                Some("left"),
            ),
            Some("left".to_string())
        );
        assert_eq!(
            window_action_output(
                super::actions::DispatchOrigin::Other,
                halley_config::FocusMode::Hover,
                None,
                Some("left")
            ),
            Some("left".to_string())
        );
    }

    #[test]
    fn only_empty_cluster_activation_shows_the_centered_indicator() {
        assert!(activation_shows_cluster_indicator(None));
        assert!(!activation_shows_cluster_indicator(Some(
            halley_core::field::NodeId::new(1)
        )));
    }

    #[test]
    fn active_cluster_blocks_every_compositor_zoom_action() {
        for action in [
            halley_config::Action::ZoomIn,
            halley_config::Action::ZoomOut,
            halley_config::Action::ZoomReset,
        ] {
            assert!(cluster_blocks_zoom(&action, true));
            assert!(!cluster_blocks_zoom(&action, false));
        }
        assert!(!cluster_blocks_zoom(
            &halley_config::Action::ToggleState,
            true
        ));
    }

    #[test]
    fn horizontal_cluster_focus_maps_to_old_stack_cycle_directions() {
        assert_eq!(
            stacking_cycle_direction(halley_config::Direction::Left),
            Some(halley_config::FocusCycleDirection::Forward)
        );
        assert_eq!(
            stacking_cycle_direction(halley_config::Direction::Right),
            Some(halley_config::FocusCycleDirection::Backward)
        );
        assert_eq!(stacking_cycle_direction(halley_config::Direction::Up), None);
        assert_eq!(
            stacking_cycle_direction(halley_config::Direction::Down),
            None
        );
    }

    #[test]
    fn live_client_focus_wins_for_hidden_cluster_members() {
        let active_member = halley_core::field::NodeId::new(1);
        let stale_logical = halley_core::field::NodeId::new(2);

        assert_eq!(
            preferred_cluster_navigation_focus(Some(active_member), Some(stale_logical)),
            Some(active_member)
        );
        assert_eq!(
            preferred_cluster_navigation_focus(None, Some(stale_logical)),
            Some(stale_logical)
        );
    }

    #[test]
    fn basics_pointer_dismissal_owns_only_its_press_and_release() {
        let mut owned = crate::input::SuppressedButtons::default();

        assert_eq!(
            super::basics_pointer_button_routing(true, BTN_LEFT, ButtonState::Pressed, &mut owned),
            BasicsInputRouting::Dismiss
        );
        assert_eq!(
            super::basics_pointer_button_routing(
                false,
                BTN_RIGHT,
                ButtonState::Pressed,
                &mut owned
            ),
            BasicsInputRouting::Forward,
            "a new press during the dismissal fade reaches its target"
        );
        assert_eq!(
            super::basics_pointer_button_routing(
                false,
                BTN_LEFT,
                ButtonState::Released,
                &mut owned
            ),
            BasicsInputRouting::SuppressOwned,
            "the release paired with the dismissing press stays suppressed"
        );
        assert_eq!(
            super::basics_pointer_button_routing(
                false,
                BTN_RIGHT,
                ButtonState::Released,
                &mut owned
            ),
            BasicsInputRouting::Forward
        );
    }

    #[test]
    fn basics_touch_dismissal_owns_only_its_touch_stream() {
        let dismissing = TouchSlot::from(Some(1));
        let next = TouchSlot::from(Some(2));
        let mut owned = crate::input::SuppressedReleases::default();

        assert_eq!(
            super::basics_touch_routing(true, dismissing, BasicsTouchPhase::Down, &mut owned),
            BasicsInputRouting::Dismiss
        );
        assert_eq!(
            super::basics_touch_routing(false, next, BasicsTouchPhase::Down, &mut owned),
            BasicsInputRouting::Forward,
            "a new touch during the dismissal fade reaches its target"
        );
        assert_eq!(
            super::basics_touch_routing(false, dismissing, BasicsTouchPhase::Motion, &mut owned),
            BasicsInputRouting::SuppressOwned
        );
        assert_eq!(
            super::basics_touch_routing(false, dismissing, BasicsTouchPhase::Up, &mut owned),
            BasicsInputRouting::SuppressOwned
        );
        assert_eq!(
            super::basics_touch_routing(false, next, BasicsTouchPhase::Up, &mut owned),
            BasicsInputRouting::Forward
        );
    }

    #[test]
    fn basics_card_only_intercepts_its_dismissal_keys() {
        assert_eq!(
            basics_key_routing(true, KeyState::Pressed, Some(Keysym::Return)),
            BasicsKeyRouting::Dismiss
        );
        assert_eq!(
            basics_key_routing(true, KeyState::Pressed, Some(Keysym::Escape)),
            BasicsKeyRouting::Dismiss
        );
        assert_eq!(
            basics_key_routing(true, KeyState::Pressed, Some(Keysym::d)),
            BasicsKeyRouting::EvaluateNormally,
            "non-dismissal presses must reach compositor keybind matching"
        );
        assert_eq!(
            basics_key_routing(true, KeyState::Released, Some(Keysym::Return)),
            BasicsKeyRouting::EvaluateNormally
        );
    }

    #[test]
    fn modal_releases_retire_preexisting_forwarded_keys() {
        assert_eq!(
            modal_key_routing(true, KeyState::Released, false),
            ModalKeyRouting::RetireUnfocusedRelease
        );
        assert_eq!(
            modal_key_routing(false, KeyState::Released, true),
            ModalKeyRouting::SuppressRelease
        );
        assert_eq!(
            modal_key_routing(true, KeyState::Pressed, false),
            ModalKeyRouting::Evaluate
        );
    }
}
