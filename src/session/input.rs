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
mod cluster_interaction;
mod dispatch;
pub(crate) use cluster_interaction::*;
mod hit_test;
use hit_test::*;
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
                        let geometry = crate::capture::window_capture_context(session)
                            .visual_geometry(&window, route.visual_geometry?);
                        let size = crate::capture::window_capture_context(session).size(&window);
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
                let geometry = crate::capture::window_capture_context(session)
                    .visual_geometry(&window, route.visual_geometry?);
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
mod tests;
