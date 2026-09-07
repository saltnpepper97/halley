use std::ffi::OsStr;

use smithay::backend::input::{
    Axis, ButtonState, Event, InputBackend, InputEvent, KeyState, KeyboardKeyEvent,
    PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::desktop::{Space, Window};
use smithay::input::keyboard::{FilterResult, Keysym};
use smithay::input::pointer::{ButtonEvent, RelativeMotionEvent};
use smithay::output::Output;
use smithay::reexports::wayland_server::Resource;
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
mod keyboard;
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

fn forward_pointer_button(intercepted: bool, finishing_client_move: bool) -> bool {
    !intercepted || finishing_client_move
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

fn shortcut_policy_allows_bindings(focus_bypasses_shortcuts: bool, inhibitor_active: bool) -> bool {
    !focus_bypasses_shortcuts && !inhibitor_active
}

pub(super) fn bindings_enabled<D: SessionDriver>(session: &Session<D>) -> bool {
    let focus = wayland::focus::current(
        &session.wayland,
        &session.fullscreen,
        &session.clusters,
        &session.nodes,
        crate::frame_clock::monotonic_now(),
    );
    let bypasses_shortcuts = focus
        .as_ref()
        .is_some_and(|focus| focus.bypasses_shortcuts());
    let inhibitor_active = focus
        .map(|focus| focus.surface())
        .and_then(|surface| {
            session
                .seat
                .keyboard_shortcuts_inhibitor_for_surface(&surface)
        })
        .is_some_and(|inhibitor| inhibitor.is_active());
    shortcut_policy_allows_bindings(bypasses_shortcuts, inhibitor_active)
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
    let pointer = session
        .wayland
        .space
        .output_under(session.pointer.position())
        .next()
        .map(|output| output.name());
    let output = match session.settings.input.focus_mode {
        halley_config::FocusMode::Click => selected,
        halley_config::FocusMode::Hover => pointer.or(selected),
    };
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
                return begin_field_pan(session, route, serial);
            }
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
            begin_field_pan(session, route, serial)
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
    serial: smithay::utils::Serial,
) -> bool {
    if !matches!(
        &route.target,
        crate::input::pointer::PointerTarget::Background
    ) || node_at_pointer(session).is_some()
    {
        return false;
    }
    wayland::focus::select_output(&mut session.wayland, &route.output);
    super::focus::focus_layer(session, None, serial);
    session.interactions.grab = crate::input::grab::Grab::Pan {
        output: route.output.name(),
    };
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
    if cluster_exclusive_on_output(
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
    if cluster_exclusive_on_output(
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
    if session.shell.overlays.confirmation_modal_active()
        && !matches!(event, InputEvent::Keyboard { .. })
    {
        session.interactions.steam_close_pressed = None;
        match event {
            InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. } => {
                session
                    .pointer
                    .process_input_event(event, &session.wayland.space);
                session.cursor_policy.pointer_activity();
                session.request_redraw();
            }
            InputEvent::PointerButton { event } => match event.state() {
                ButtonState::Pressed => session
                    .interactions
                    .suppressed_buttons
                    .suppress(event.button_code()),
                ButtonState::Released => {
                    session
                        .interactions
                        .suppressed_buttons
                        .release_is_suppressed(event.button_code());
                }
            },
            InputEvent::PointerAxis { .. } => session.interactions.wheel_accumulator.reset_all(),
            _ => {}
        }
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
    let motion = match event {
        InputEvent::PointerMotion { event } => Some((
            event.delta(),
            event.delta_unaccel(),
            event.time(),
            event.time_msec(),
        )),
        InputEvent::PointerMotionAbsolute { event } => {
            let delta = Point::<f64, Logical>::from((
                proposed_position.0 - position_before.0,
                proposed_position.1 - position_before.1,
            ));
            Some((delta, delta, event.time(), event.time_msec()))
        }
        _ => None,
    };
    if session.capture.is_active() {
        match event {
            InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. } => {
                update_capture_pointer(session, proposed_position);
                session.request_redraw();
                return;
            }
            InputEvent::PointerButton { event } => {
                if event.button_code() == BTN_LEFT {
                    update_capture_pointer(session, proposed_position);
                    match event.state() {
                        ButtonState::Pressed => {
                            session.interactions.suppressed_buttons.suppress(BTN_LEFT);
                            match session.capture.press(proposed_position) {
                                Some(crate::capture::CapturePress::ActivateScreenshot(mode)) => {
                                    if session.capture.activate_menu(mode, &session.wayland.space) {
                                        update_capture_pointer(session, proposed_position);
                                    }
                                }
                                Some(crate::capture::CapturePress::ActivateSource(mode)) => {
                                    if session.capture.activate_source(mode) {
                                        update_capture_pointer(session, proposed_position);
                                    }
                                }
                                Some(crate::capture::CapturePress::Accept) => {
                                    crate::capture::accept_selected(session);
                                }
                                Some(crate::capture::CapturePress::Consumed) | None => {}
                            }
                        }
                        ButtonState::Released => {
                            session
                                .interactions
                                .suppressed_buttons
                                .release_is_suppressed(BTN_LEFT);
                            session.capture.release();
                        }
                    }
                }
                session.request_redraw();
                return;
            }
            InputEvent::PointerAxis { .. } => return,
            _ => {}
        }
    }
    if session.shell.cluster_composer.accepts_input() {
        let naming = session
            .clusters
            .creation()
            .is_some_and(|creation| creation.naming);
        match event {
            InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. } => {
                if naming {
                    let output = session
                        .shell
                        .cluster_composer
                        .target_output()
                        .map(str::to_string);
                    let hit = output.as_deref().and_then(|output| {
                        session
                            .render
                            .cluster_creation_overlay
                            .hit_test(output, proposed_position)
                    });
                    if let Some(
                        crate::render::overlays::cluster_creation::CreationOverlayHit::InputCaret(
                            caret,
                        ),
                    ) = hit
                    {
                        session.clusters.drag_name_selection(caret);
                    }
                    session.cursor.set_override(
                        crate::cursor::OverrideSource::Modal,
                        match hit {
                            Some(
                                crate::render::overlays::cluster_creation::CreationOverlayHit::ConfirmButton,
                            ) => Some(smithay::input::pointer::CursorIcon::Pointer),
                            Some(
                                crate::render::overlays::cluster_creation::CreationOverlayHit::InputCaret(_),
                            ) => Some(smithay::input::pointer::CursorIcon::Text),
                            None => None,
                        },
                    );
                } else {
                    session
                        .shell
                        .cluster_composer
                        .hover(Point::<f64, Logical>::from(proposed_position));
                }
            }
            InputEvent::PointerButton { event } => {
                let button = event.button_code();
                match event.state() {
                    ButtonState::Pressed => {
                        session.interactions.suppressed_buttons.suppress(button);
                        if button == BTN_LEFT {
                            if naming {
                                let output = session
                                    .shell
                                    .cluster_composer
                                    .target_output()
                                    .map(str::to_string);
                                match output.as_deref().and_then(|output| {
                                    session
                                        .render
                                        .cluster_creation_overlay
                                        .hit_test(output, proposed_position)
                                }) {
                                    Some(
                                        crate::render::overlays::cluster_creation::CreationOverlayHit::ConfirmButton,
                                    ) => {
                                        begin_cluster_commit(session);
                                    }
                                    Some(
                                        crate::render::overlays::cluster_creation::CreationOverlayHit::InputCaret(caret),
                                    ) => {
                                        session.clusters.begin_name_selection(caret);
                                    }
                                    None => {}
                                }
                            } else {
                                let position = Point::<f64, Logical>::from(proposed_position);
                                let clicked = session
                                    .shell
                                    .cluster_composer
                                    .session()
                                    .and_then(|composer| composer.hit_test(position));
                                session.shell.cluster_composer.hover(position);
                                if let (Some(id), Some(output)) = (
                                    clicked,
                                    session
                                        .shell
                                        .cluster_composer
                                        .target_output()
                                        .map(str::to_string),
                                ) {
                                    session.clusters.toggle_creation_member(id, &output);
                                }
                            }
                        }
                    }
                    ButtonState::Released => {
                        session
                            .interactions
                            .suppressed_buttons
                            .release_is_suppressed(button);
                        if naming && button == BTN_LEFT {
                            session.clusters.end_name_selection();
                        }
                    }
                }
            }
            InputEvent::PointerAxis { .. } => {
                session.interactions.wheel_accumulator.reset_all();
            }
            _ => {}
        }
        if matches!(
            event,
            InputEvent::PointerMotion { .. }
                | InputEvent::PointerMotionAbsolute { .. }
                | InputEvent::PointerButton { .. }
                | InputEvent::PointerAxis { .. }
        ) {
            session.request_redraw();
            super::pointer::finish_frame(session, &pointer_handle);
            return;
        }
    }
    if session.shell.cluster_composer.is_active() {
        match event {
            InputEvent::PointerButton { event } => match event.state() {
                ButtonState::Pressed => session
                    .interactions
                    .suppressed_buttons
                    .suppress(event.button_code()),
                ButtonState::Released => {
                    session
                        .interactions
                        .suppressed_buttons
                        .release_is_suppressed(event.button_code());
                }
            },
            InputEvent::PointerAxis { .. } => {
                session.interactions.wheel_accumulator.reset_all();
            }
            _ => {}
        }
        if matches!(
            event,
            InputEvent::PointerMotion { .. }
                | InputEvent::PointerMotionAbsolute { .. }
                | InputEvent::PointerButton { .. }
                | InputEvent::PointerAxis { .. }
        ) {
            session.request_redraw();
            super::pointer::finish_frame(session, &pointer_handle);
            return;
        }
    }
    if session.shell.apogee.accepts_input() {
        match event {
            InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. } => {
                crate::shell::apogee::pointer_motion(session, proposed_position);
            }
            InputEvent::PointerButton { event } if event.button_code() == BTN_LEFT => {
                match event.state() {
                    ButtonState::Pressed => {
                        session.interactions.suppressed_buttons.suppress(BTN_LEFT);
                        crate::shell::apogee::pointer_press(session, proposed_position);
                    }
                    ButtonState::Released => {
                        session
                            .interactions
                            .suppressed_buttons
                            .release_is_suppressed(BTN_LEFT);
                    }
                }
            }
            InputEvent::PointerButton { .. } | InputEvent::PointerAxis { .. } => {}
            _ => {}
        }
        if matches!(
            event,
            InputEvent::PointerMotion { .. }
                | InputEvent::PointerMotionAbsolute { .. }
                | InputEvent::PointerButton { .. }
                | InputEvent::PointerAxis { .. }
        ) {
            session.request_redraw();
            return;
        }
    }
    let constrained_motion = super::pointer::constrain_motion(session, &pointer_handle);

    if let super::pointer::ConstrainedMotion::RelativeOnly { surface, origin } = &constrained_motion
        && let Some((delta, delta_unaccel, time, _)) = motion
    {
        session.pointer.set_position(position_before);
        crate::session::trace::surface_sampled_event(
            session,
            surface,
            "locked-relative-motion",
            format_args!(
                "delta={delta:?} delta_unaccel={delta_unaccel:?} anchor={position_before:?} origin={origin:?}"
            ),
        );
        pointer_handle.relative_motion(
            session,
            Some((surface.clone(), *origin)),
            &RelativeMotionEvent {
                delta,
                delta_unaccel,
                utime: time,
            },
        );
        super::pointer::finish_frame(session, &pointer_handle);
        return;
    }

    if let super::pointer::ConstrainedMotion::Clamp(position) = constrained_motion {
        session.pointer.set_position((position.x, position.y));
    }
    if let crate::input::grab::Grab::MoveWindow {
        edge_pan: Some(edge_pan),
        ..
    } = &session.interactions.grab
        && let Some(output) = session
            .wayland
            .space
            .outputs()
            .find(|output| output.name() == edge_pan.output)
        && let Some(geometry) = session.wayland.space.output_geometry(output)
    {
        let position = session.pointer.position();
        let right = f64::from(geometry.loc.x + geometry.size.w) - 0.001;
        let bottom = f64::from(geometry.loc.y + geometry.size.h) - 0.001;
        session.pointer.set_position((
            position.0.clamp(f64::from(geometry.loc.x), right),
            position.1.clamp(f64::from(geometry.loc.y), bottom),
        ));
    }
    let position_after = session.pointer.position();
    session.request_redraw();

    if motion.is_some()
        && let Some(output) = session
            .clusters
            .creation()
            .filter(|creation| creation.naming)
            .map(|creation| creation.output.clone())
    {
        let hit = session
            .render
            .cluster_creation_overlay
            .hit_test(&output, position_after);
        if let Some(crate::render::overlays::cluster_creation::CreationOverlayHit::InputCaret(
            caret,
        )) = hit
        {
            session.clusters.drag_name_selection(caret);
        }
        session.cursor.set_override(
            crate::cursor::OverrideSource::Modal,
            match hit {
                Some(
                    crate::render::overlays::cluster_creation::CreationOverlayHit::ConfirmButton,
                ) => Some(smithay::input::pointer::CursorIcon::Pointer),
                Some(
                    crate::render::overlays::cluster_creation::CreationOverlayHit::InputCaret(_),
                ) => Some(smithay::input::pointer::CursorIcon::Text),
                None => None,
            },
        );
        super::pointer::finish_frame(session, &pointer_handle);
        return;
    }

    if motion.is_some() && session.clusters.bloom_pull().is_some() {
        let now = crate::frame_clock::monotonic_now();
        session
            .clusters
            .update_bloom_pull(Point::<f64, Logical>::from(position_after), now);
        session.clusters.set_overlay_hovered(None);
        session.cursor.set_override(
            crate::cursor::OverrideSource::Grab,
            Some(smithay::input::pointer::CursorIcon::Grabbing),
        );
        session.request_redraw();
        super::pointer::finish_frame(session, &pointer_handle);
        return;
    }

    if motion.is_some()
        && let Some((drag_output, _)) = session.clusters.overflow_drag()
    {
        let now = crate::frame_clock::monotonic_now();
        if let Some((output, geometry)) = output_at_pointer(&session.wayland.space, position_after)
        {
            let local = Point::<f64, Logical>::from((
                position_after.0 - f64::from(geometry.loc.x),
                position_after.1 - f64::from(geometry.loc.y),
            ));
            let output_name = output.name();
            if output_name == drag_output {
                session
                    .clusters
                    .update_overflow_drag(&output_name, local, now);
            }
        }
        session.clusters.set_overlay_hovered(None);
        session.cursor.set_override(
            crate::cursor::OverrideSource::Grab,
            Some(smithay::input::pointer::CursorIcon::Grabbing),
        );
        session.request_redraw();
        super::pointer::finish_frame(session, &pointer_handle);
        return;
    }

    if motion.is_some()
        && let crate::input::grab::Grab::PendingNode {
            id,
            surface,
            press_screen,
            screen_offset,
            ..
        } = &session.interactions.grab
    {
        let dx = position_after.0 - press_screen.x;
        let dy = position_after.1 - press_screen.y;
        if dx.hypot(dy) >= NODE_DRAG_THRESHOLD_PX && !crate::session::node_user_pinned(session, *id)
        {
            let id = *id;
            let surface = surface.clone();
            let screen_offset = *screen_offset;
            if let Some((output, output_geometry)) =
                output_at_pointer(&session.wayland.space, position_after)
                && let Some(camera) = session.cameras.get(&output.name())
            {
                let pointer_world = crate::input::grab::screen_to_world_on_output(
                    position_after,
                    camera,
                    output_geometry,
                );
                let offset = crate::input::grab::screen_offset_to_world(screen_offset, camera);
                let desired = halley_core::field::Vec2 {
                    x: pointer_world.x + offset.x,
                    y: pointer_world.y + offset.y,
                };
                crate::nodes::set_collapsed_output(session, id, &output);
                session.nodes.clear_direct_motion(id);
                session.interactions.grab = crate::input::grab::Grab::MoveNode {
                    id,
                    surface,
                    screen_offset,
                    last_world: desired,
                    last_update: crate::frame_clock::monotonic_now(),
                    velocity: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
                };
                session.cursor.set_override(
                    crate::cursor::OverrideSource::Grab,
                    Some(smithay::input::pointer::CursorIcon::Grabbing),
                );
            }
        }
    }

    if motion.is_some()
        && let crate::input::grab::Grab::PendingClusterCore {
            id,
            press_screen,
            screen_offset,
            ..
        } = &session.interactions.grab
    {
        let dx = position_after.0 - press_screen.x;
        let dy = position_after.1 - press_screen.y;
        if dx.hypot(dy) >= NODE_DRAG_THRESHOLD_PX
            && session
                .clusters
                .core_node(*id)
                .is_none_or(|core| !crate::session::node_user_pinned(session, core))
        {
            session.interactions.grab = crate::input::grab::Grab::MoveClusterCore {
                id: *id,
                screen_offset: *screen_offset,
            };
            session.cursor.set_override(
                crate::cursor::OverrideSource::Grab,
                Some(smithay::input::pointer::CursorIcon::Grabbing),
            );
        }
    }

    if motion.is_some()
        && let crate::input::grab::Grab::PendingWindowMove(pending) = &session.interactions.grab
    {
        let pending = pending.clone();
        match pending_window_move_motion(
            !pending.client_owned || pointer_handle.has_grab(pending.serial),
            pending.press_screen,
            position_after,
        ) {
            PendingWindowMoveMotion::Wait => {}
            PendingWindowMoveMotion::Cancel => {
                session.interactions.grab = crate::input::grab::Grab::None;
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Grab, None);
            }
            PendingWindowMoveMotion::Activate => {
                session.interactions.grab = crate::input::grab::Grab::None;
                if !super::activate_client_pointer_move(session, pending) {
                    session
                        .cursor
                        .set_override(crate::cursor::OverrideSource::Grab, None);
                }
            }
        }
    }

    let mut resize_preview_changed = false;
    if motion.is_some()
        && let crate::input::grab::Grab::ResizeWindow(state) = &mut session.interactions.grab
    {
        let world = crate::input::grab::resize_cursor_from_screen(state, position_after);
        let requested_size = crate::input::grab::resize_target_size(
            state.handle,
            state.start_rect,
            state.start_cursor,
            world,
        );
        state.target_size = if crate::xwayland::is_x11(&state.window) {
            crate::xwayland::constrain_window_size(&state.window, requested_size)
        } else {
            requested_size
        };
        let animations = &session.settings.animations;
        resize_preview_changed = crate::input::grab::advance_resize_preview(
            state,
            crate::frame_clock::monotonic_now(),
            animations.enabled && animations.smooth_resize.enabled,
            animations.smooth_resize.duration_ms,
        );
    }
    match &session.interactions.grab {
        crate::input::grab::Grab::MoveWindow {
            id,
            window,
            cluster_drag,
            drag_size,
            anchor,
            edge_pan,
            last_world,
            last_update,
            velocity,
            ..
        } => {
            let id = *id;
            let window = window.clone();
            let mut cluster_drag = cluster_drag.clone();
            let drag_size = *drag_size;
            let anchor = *anchor;
            let edge_pan = edge_pan.clone();
            let previous = *last_world;
            let last_update = *last_update;
            let previous_velocity = *velocity;
            let drag_output = edge_pan
                .as_ref()
                .and_then(|edge_pan| {
                    let output = session
                        .wayland
                        .space
                        .outputs()
                        .find(|output| output.name() == edge_pan.output)?
                        .clone();
                    let geometry = session.wayland.space.output_geometry(&output)?;
                    Some((output, geometry))
                })
                .or_else(|| output_at_pointer(&session.wayland.space, position_after));
            if let Some((output, output_geometry)) = drag_output {
                let output_name = output.name();
                let Some(camera) = session.cameras.get(&output_name) else {
                    return;
                };
                let output_changed = edge_pan.is_none()
                    && wayland::window_output_name(&window).as_deref()
                        != Some(output_name.as_str());
                let size = drag_size.unwrap_or_else(|| {
                    session
                        .wayland
                        .space
                        .element_geometry(&window)
                        .map(|geometry| geometry.size)
                        .unwrap_or((1, 1).into())
                });
                let edge_placement = edge_pan.as_ref().and_then(|_| {
                    let frame_extents = crate::titlebar::frame_extents(
                        &window,
                        &session.settings.decorations,
                        &session.settings.font,
                    );
                    crate::input::grab::window_edge_pan_placement(
                        position_after,
                        anchor,
                        size,
                        frame_extents,
                        camera,
                        output_geometry,
                    )
                });
                let desired_location = edge_placement
                    .map(|placement| placement.location)
                    .unwrap_or_else(|| {
                        anchor.world_location(position_after, camera, output_geometry)
                    });
                let desired_center = edge_placement.map(|placement| placement.center).unwrap_or(
                    halley_core::field::Vec2 {
                        x: desired_location.x as f32 + size.w as f32 * 0.5,
                        y: desired_location.y as f32 + size.h as f32 * 0.5,
                    },
                );
                let camera_scale = crate::presentation::camera::scale(camera).max(0.05);
                let now = crate::frame_clock::monotonic_now();
                if let Some(placement) = edge_placement
                    && let crate::input::grab::Grab::MoveWindow {
                        edge_pan: Some(live),
                        ..
                    } = &mut session.interactions.grab
                {
                    live.update_contact(placement.contact, now);
                }
                let sampled = if edge_pan.is_some() {
                    halley_core::field::Vec2 { x: 0.0, y: 0.0 }
                } else {
                    sampled_drag_velocity(
                        previous,
                        desired_center,
                        previous_velocity,
                        last_update,
                        now,
                    )
                };
                if let Some(drag) = cluster_drag.as_mut() {
                    drag.on_origin_output = drag.output == output_name;
                    if let Some(id) = id {
                        let screen_offset = anchor.screen_offset(camera);
                        let screen_location = Point::<i32, Logical>::from((
                            (position_after.0 + f64::from(screen_offset.x)).round() as i32
                                - output_geometry.loc.x,
                            (position_after.1 + f64::from(screen_offset.y)).round() as i32
                                - output_geometry.loc.y,
                        ));
                        let _ = session.clusters.update_workspace_drag(
                            id,
                            &output_name,
                            screen_location,
                        );
                    }
                    if let crate::input::grab::Grab::MoveWindow {
                        cluster_drag: Some(live),
                        ..
                    } = &mut session.interactions.grab
                    {
                        live.on_origin_output = drag.on_origin_output;
                    }
                }
                let attached_tile = cluster_drag.as_ref().is_some_and(|drag| {
                    matches!(
                        drag.kind,
                        crate::input::grab::ClusterWindowDragKind::Layout(
                            halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Tiling
                        )
                    ) && drag.on_origin_output
                });
                let cluster_tracks_pointer = cluster_drag.is_some();
                if output_changed {
                    if let Some(id) = id {
                        crate::nodes::set_collapsed_output(session, id, &output);
                    } else {
                        wayland::set_window_output(&window, &output);
                    }
                }
                if let Some(id) = id {
                    if attached_tile || cluster_tracks_pointer {
                        session
                            .wayland
                            .space
                            .relocate_element(&window, desired_location);
                        session.request_redraw();
                    } else if !session.nodes.physics.enabled {
                        let _ = crate::nodes::move_grabbed_body_rigid(session, id, desired_center);
                    }
                    let cluster_reordered = attached_tile
                        && cluster_drag.as_ref().is_some_and(|drag| {
                            let output_local = Point::<f64, Logical>::from((
                                position_after.0 - f64::from(output_geometry.loc.x),
                                position_after.1 - f64::from(output_geometry.loc.y),
                            ));
                            let work_area = smithay::desktop::layer_map_for_output(&output)
                                .non_exclusive_zone();
                            session.clusters.move_tiled_drag_to_point(
                                &drag.output,
                                id,
                                work_area,
                                output_local,
                                now,
                            )
                        });
                    let desired_client = Rectangle::<i32, Logical>::new(desired_location, size);
                    let desired_outer = crate::titlebar::outer_rect_for_client(
                        &window,
                        desired_client,
                        &session.settings.decorations,
                        &session.settings.font,
                    );
                    let join_candidate_changed = cluster_drag.is_none()
                        && edge_pan.is_none()
                        && session.clusters.update_join_candidate(
                            &session.nodes.field,
                            &output_name,
                            id,
                            crate::clusters::JoinContact {
                                center: desired_center,
                                member_left: desired_center.x - desired_outer.loc.x as f32,
                                member_right: desired_outer.loc.x as f32
                                    + desired_outer.size.w as f32
                                    - desired_center.x,
                                member_top: desired_center.y - desired_outer.loc.y as f32,
                                member_bottom: desired_outer.loc.y as f32
                                    + desired_outer.size.h as f32
                                    - desired_center.y,
                                core_radius: crate::clusters::CORE_DIAMETER_PX * 0.5 / camera_scale,
                                gap: session.nodes.landmarks.gap_px / camera_scale,
                            },
                            now,
                        );
                    if cluster_reordered || join_candidate_changed {
                        session.request_redraw();
                    }
                } else {
                    session.clusters.cancel_join_candidate();
                    session
                        .wayland
                        .space
                        .map_element(window.clone(), desired_location, false);
                }
                if let crate::input::grab::Grab::MoveWindow {
                    last_world,
                    last_update,
                    velocity,
                    ..
                } = &mut session.interactions.grab
                {
                    *last_world = desired_center;
                    *last_update = now;
                    *velocity = sampled;
                }
                if id.is_some() && session.nodes.physics.enabled {
                    session.request_redraw();
                }
                if output_changed {
                    wayland::popup::update_reactive_for_window(
                        &session.wayland,
                        crate::session::popup_unconstrain_context!(session),
                        &window,
                    );
                }
            }
        }
        crate::input::grab::Grab::MoveNode {
            id,
            screen_offset,
            last_world,
            last_update,
            velocity,
            ..
        } => {
            let id = *id;
            let screen_offset = *screen_offset;
            let previous = *last_world;
            let last_update = *last_update;
            let previous_velocity = *velocity;
            if let Some((output, output_geometry)) =
                output_at_pointer(&session.wayland.space, position_after)
                && let Some(camera) = session.cameras.get(&output.name())
            {
                let pointer_world = crate::input::grab::screen_to_world_on_output(
                    position_after,
                    camera,
                    output_geometry,
                );
                let offset = crate::input::grab::screen_offset_to_world(screen_offset, camera);
                let desired = halley_core::field::Vec2 {
                    x: pointer_world.x + offset.x,
                    y: pointer_world.y + offset.y,
                };
                let now = crate::frame_clock::monotonic_now();
                let sampled =
                    sampled_drag_velocity(previous, desired, previous_velocity, last_update, now);
                crate::nodes::set_collapsed_output(session, id, &output);
                if !session.nodes.physics.enabled {
                    let _ = crate::nodes::move_grabbed_body_rigid(session, id, desired);
                }
                if let crate::input::grab::Grab::MoveNode {
                    last_world,
                    last_update,
                    velocity,
                    ..
                } = &mut session.interactions.grab
                {
                    *last_world = desired;
                    *last_update = now;
                    *velocity = sampled;
                }
                if session.nodes.physics.enabled {
                    session.request_redraw();
                }
            }
        }
        crate::input::grab::Grab::MoveClusterCore { id, screen_offset } => {
            let id = *id;
            let screen_offset = *screen_offset;
            if let Some((output, output_geometry)) =
                output_at_pointer(&session.wayland.space, position_after)
                && let Some(camera) = session.cameras.get(&output.name())
            {
                let pointer_world = crate::input::grab::screen_to_world_on_output(
                    position_after,
                    camera,
                    output_geometry,
                );
                let offset = crate::input::grab::screen_offset_to_world(screen_offset, camera);
                let desired = halley_core::field::Vec2 {
                    x: pointer_world.x + offset.x,
                    y: pointer_world.y + offset.y,
                };
                let output_name = output.name();
                let output_changed = session
                    .clusters
                    .metadata(id)
                    .is_some_and(|metadata| metadata.output != output_name);
                let members = if output_changed {
                    session.clusters.member_ids(id)
                } else {
                    Vec::new()
                };
                let output_ready = if output_changed {
                    let current = session
                        .clusters
                        .metadata(id)
                        .map(|metadata| metadata.core_position)
                        .unwrap_or(desired);
                    session.clusters.move_core(id, &output_name, current)
                } else {
                    true
                };
                if output_ready && output_changed {
                    for member in members {
                        crate::nodes::set_collapsed_output(session, member, &output);
                    }
                }
                if output_ready && crate::nodes::move_cluster_core_rigid(session, id, desired) {
                    session.request_redraw();
                }
            }
        }
        crate::input::grab::Grab::Pan { output } => {
            let dx = position_after.0 - position_before.0;
            let dy = position_after.1 - position_before.1;
            if let Some(camera) = session.cameras.get_mut(output) {
                let delta = crate::input::grab::screen_delta_to_world(dx, dy, camera);
                camera.pan_target(halley_core::field::Vec2 {
                    x: -delta.x,
                    y: -delta.y,
                });
            }
        }
        crate::input::grab::Grab::ResizeWindow(_) => {}
        crate::input::grab::Grab::None
        | crate::input::grab::Grab::PendingWindowMove(_)
        | crate::input::grab::Grab::PendingNode { .. }
        | crate::input::grab::Grab::PendingClusterCore { .. } => {}
    }

    if resize_preview_changed {
        apply_active_resize(session);
    }

    if let Some((delta, delta_unaccel, time, time_msec)) = motion
        && !super::pointer::constraint_suspended_for_grab(session)
    {
        let route = super::pointer::route_for_motion(session, time_msec);
        if super::pointer::relative_motion_allowed(session, route.as_ref()) {
            pointer_handle.relative_motion(
                session,
                route.as_ref().and_then(|route| route.focus.clone()),
                &RelativeMotionEvent {
                    delta,
                    delta_unaccel,
                    utime: time,
                },
            );
        }
        super::pointer::finish_frame(session, &pointer_handle);
        if let Some(route) = route.as_ref() {
            super::focus::update_hover(session, route, SERIAL_COUNTER.next_serial());
        }
        let titlebar_hovered = route.as_ref().and_then(|route| match &route.target {
            crate::input::pointer::PointerTarget::Decoration {
                window,
                hit: crate::titlebar::Hit::Control(control),
            } => Some(crate::titlebar::ButtonTarget {
                window: window.clone(),
                control: *control,
            }),
            _ => None,
        });
        let border_resize_hover = route.as_ref().and_then(|route| match &route.target {
            crate::input::pointer::PointerTarget::Decoration {
                hit: crate::titlebar::Hit::Resize(handle),
                ..
            } => Some(*handle),
            _ => None,
        });
        if session.interactions.titlebar_hovered != titlebar_hovered {
            session.interactions.titlebar_hovered = titlebar_hovered;
            session.request_redraw();
        }
        let node_grab_active = session.interactions.grab.landmark_active();
        let now = crate::frame_clock::monotonic_now();
        let hovered_action = (!node_grab_active)
            .then(|| cluster_action_at_pointer(session, now))
            .flatten();
        let hovered_bloom = (!node_grab_active && hovered_action.is_none())
            .then(|| cluster_bloom_at_pointer(session, now))
            .flatten();
        let overflow_hover =
            (!node_grab_active && hovered_action.is_none() && hovered_bloom.is_none())
                .then(|| update_overflow_hover(session, now))
                .flatten();
        let overflow_intercepts = overflow_hover
            .as_ref()
            .is_some_and(|hover| hover.intercepts_desktop);
        let hovered_node = (!node_grab_active && hovered_bloom.is_none() && !overflow_intercepts)
            .then(|| node_at_pointer(session))
            .flatten();
        let hovered_cluster = (!node_grab_active
            && hovered_bloom.is_none()
            && !overflow_intercepts
            && hovered_node.is_none())
        .then(|| cluster_at_pointer(session))
        .flatten();
        if let Some((id, output)) = hovered_node.as_ref() {
            super::focus::focus_node_from_hover(session, *id, output, SERIAL_COUNTER.next_serial());
        }
        if let Some((id, output)) = hovered_cluster.as_ref()
            && let Some(core) = session.clusters.core_node(*id)
        {
            super::focus::focus_cluster_core_from_hover(
                session,
                core,
                output,
                SERIAL_COUNTER.next_serial(),
            );
        }
        let hovered = hovered_node.map(|(id, _)| id);
        let node_changed = session.nodes.set_hovered(hovered, now);
        let cluster_changed = session
            .clusters
            .set_hovered_core(hovered_cluster.map(|(id, _)| id), now);
        let overlay_hovered = hovered_bloom
            .as_ref()
            .map(|(token, output)| (output.name(), token.member_id))
            .or_else(|| {
                overflow_hover
                    .as_ref()
                    .and_then(|hover| hover.member.map(|member| (hover.output.name(), member)))
            });
        let overlay_changed = session.clusters.set_overlay_hovered(overlay_hovered);
        if hovered_action.is_some()
            || hovered_bloom.is_some()
            || overflow_hover
                .as_ref()
                .is_some_and(|hover| hover.member.is_some())
        {
            session.cursor.set_override(
                crate::cursor::OverrideSource::Hover,
                Some(smithay::input::pointer::CursorIcon::Pointer),
            );
        } else if let Some(handle) = border_resize_hover {
            session.cursor.set_override(
                crate::cursor::OverrideSource::Hover,
                Some(handle.cursor_icon()),
            );
        } else if !node_grab_active {
            session
                .cursor
                .set_override(crate::cursor::OverrideSource::Hover, None);
        }
        if node_changed
            || cluster_changed
            || overlay_changed
            || overflow_hover.is_some_and(|hover| hover.changed)
        {
            session.request_redraw();
        }
    }
    if motion.is_some() && super::pointer::constraint_suspended_for_grab(session) {
        super::pointer::finish_frame(session, &pointer_handle);
    }

    if let InputEvent::PointerButton {
        event: button_event,
    } = event
    {
        let button = button_event.button_code();
        let state = button_event.state();
        let time = button_event.time_msec();
        let serial = SERIAL_COUNTER.next_serial();
        if matches!(
            &session.interactions.grab,
            crate::input::grab::Grab::PendingWindowMove(pending)
                if pending.client_owned && releases_pending_window_move(
                    pending.button,
                    button,
                    state == ButtonState::Released,
                )
        ) {
            // No compositor move began, so leave this release unconsumed. It
            // completes the client's click and lets its double-click handler
            // decide whether to toggle maximize.
            session.interactions.grab = crate::input::grab::Grab::None;
            session
                .cursor
                .set_override(crate::cursor::OverrideSource::Grab, None);
        }
        if button == BTN_LEFT && state == ButtonState::Released {
            if session.clusters.bloom_pull().is_some() {
                session.clusters.clear_bloom_pull();
                session.clusters.set_overlay_hovered(None);
                session
                    .interactions
                    .suppressed_buttons
                    .release_is_suppressed(button);
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Grab, None);
                session.request_redraw();
                super::pointer::finish_frame(session, &pointer_handle);
                return;
            }
            if let Some((drag_output, drag)) = session.clusters.take_overflow_drag() {
                let now = crate::frame_clock::monotonic_now();
                let output = session
                    .wayland
                    .space
                    .outputs()
                    .find(|output| output.name() == drag_output)
                    .cloned();
                let mut changed = false;
                if let Some(output) = output.as_ref()
                    && let Some(geometry) = session.wayland.space.output_geometry(output)
                {
                    let local = Point::<f64, Logical>::from((
                        position_after.0 - f64::from(geometry.loc.x),
                        position_after.1 - f64::from(geometry.loc.y),
                    ));
                    let work_area =
                        smithay::desktop::layer_map_for_output(output).non_exclusive_zone();
                    let moved = (local.x - drag.press_local.x).hypot(local.y - drag.press_local.y)
                        >= NODE_DRAG_THRESHOLD_PX;
                    let strip_slot = moved
                        .then(|| {
                            session.clusters.overflow_strip_slot(
                                &drag_output,
                                work_area,
                                local,
                                now,
                            )
                        })
                        .flatten();
                    let target = (moved && strip_slot.is_none())
                        .then(|| {
                            session
                                .clusters
                                .visible_tile_hit_test(&drag_output, work_area, local)
                        })
                        .flatten();
                    changed = if let Some(target_slot) = strip_slot {
                        session.clusters.reorder_overflow_member(
                            &drag_output,
                            drag.member_id,
                            target_slot,
                            now,
                        )
                    } else if let Some(target) = target {
                        session.clusters.swap_overflow_member(
                            &drag_output,
                            drag.member_id,
                            target,
                            work_area,
                            now,
                        )
                    } else {
                        session.clusters.promote_overflow_member(
                            &drag_output,
                            drag.member_id,
                            work_area,
                            now,
                        )
                    };
                    session.clusters.reveal_overflow(&drag_output, now);
                }
                if changed
                    && let Some(window) = session
                        .nodes
                        .record(drag.member_id)
                        .map(|record| record.window.clone())
                {
                    super::focus_window(session, &window, serial);
                }
                session.clusters.set_overlay_hovered(None);
                session
                    .interactions
                    .suppressed_buttons
                    .release_is_suppressed(button);
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Grab, None);
                session.request_redraw();
                super::pointer::finish_frame(session, &pointer_handle);
                return;
            }
            match &session.interactions.grab {
                crate::input::grab::Grab::PendingNode { id, .. } => {
                    let id = *id;
                    session.interactions.grab = crate::input::grab::Grab::None;
                    session
                        .cursor
                        .set_override(crate::cursor::OverrideSource::Grab, None);
                    let _ = crate::nodes::restore(session, id, serial);
                    super::pointer::finish_frame(session, &pointer_handle);
                    return;
                }
                crate::input::grab::Grab::MoveNode { id, .. } => {
                    let id = *id;
                    let now = crate::frame_clock::monotonic_now();
                    if session.nodes.physics.enabled {
                        let _ = crate::nodes::tick_physics(session, now);
                    }
                    let active_workspace_release = (|| {
                        let (output, output_geometry) =
                            output_at_pointer(&session.wayland.space, session.pointer.position())?;
                        session.clusters.active_on(&output.name())?;
                        let work_area =
                            smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
                        let global_work_area =
                            Rectangle::new(output_geometry.loc + work_area.loc, work_area.size);
                        if !global_work_area
                            .to_f64()
                            .contains(Point::<f64, Logical>::from(session.pointer.position()))
                        {
                            return None;
                        }
                        let center = session.nodes.field.node(id)?.pos;
                        let size = session.nodes.record(id)?.geometry.size;
                        let origin = collapsed_node_drop_origin(center, size, output_geometry);
                        Some((output, work_area, origin))
                    })();
                    session.interactions.grab = crate::input::grab::Grab::None;
                    session
                        .cursor
                        .set_override(crate::cursor::OverrideSource::Grab, None);
                    session.nodes.clear_direct_motion(id);
                    session.clusters.cancel_join_candidate();
                    if let Some((output, work_area, origin)) = active_workspace_release {
                        let output_name = output.name();
                        crate::nodes::set_collapsed_output(session, id, &output);
                        if crate::nodes::restore_for_cluster_join(session, id, serial)
                            && session.clusters.join_active_member_front(
                                &mut session.nodes.field,
                                &output_name,
                                id,
                                work_area,
                                origin,
                                now,
                            )
                        {
                            super::reconcile_cluster_surfaces(session, &output_name);
                        }
                    }
                    session.request_redraw();
                    super::pointer::finish_frame(session, &pointer_handle);
                    return;
                }
                crate::input::grab::Grab::PendingClusterCore { id, output, .. } => {
                    let id = *id;
                    let output_name = output.clone();
                    let now = crate::frame_clock::monotonic_now();
                    session
                        .interactions
                        .suppressed_buttons
                        .release_is_suppressed(BTN_LEFT);
                    session.interactions.grab = crate::input::grab::Grab::None;
                    session
                        .cursor
                        .set_override(crate::cursor::OverrideSource::Grab, None);
                    let output = {
                        session
                            .wayland
                            .space
                            .outputs()
                            .find(|candidate| candidate.name() == output_name)
                            .cloned()
                    };
                    let owned_focus = cluster_owns_focus(session, id);
                    if session.clusters.activate(&output_name, id, now)
                        && let Some(output) = output
                    {
                        sync_cluster_activation_focus(session, &output, id, owned_focus, serial);
                    }
                    session.request_redraw();
                    super::pointer::finish_frame(session, &pointer_handle);
                    return;
                }
                crate::input::grab::Grab::MoveClusterCore { .. } => {
                    session
                        .interactions
                        .suppressed_buttons
                        .release_is_suppressed(BTN_LEFT);
                    session.interactions.grab = crate::input::grab::Grab::None;
                    session
                        .cursor
                        .set_override(crate::cursor::OverrideSource::Grab, None);
                    session.request_redraw();
                    super::pointer::finish_frame(session, &pointer_handle);
                    return;
                }
                _ => {}
            }
        }
        let mut route = super::pointer::route_for_discrete_input(session, time);
        let native_popup_grab = pointer_handle
            .with_grab(|_, grab| {
                grab.downcast_ref::<smithay::desktop::PopupPointerGrab<Session<D>>>()
                    .is_some()
            })
            .unwrap_or(false);
        if native_popup_grab && let Some(mut grab) = session.popup_grab.take() {
            let popup_roots = grab
                .pointer_grab_start_data()
                .focus
                .as_ref()
                .map(|(root, _)| {
                    smithay::desktop::PopupManager::popups_for_surface(root)
                        .map(|(popup, _)| popup.wl_surface().clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let target_root = route
                .as_ref()
                .and_then(|route| route.focus.as_ref())
                .map(|(surface, _)| wayland::compositor::root_surface(surface));
            if !grab.has_ended()
                && wayland::popup::outside_popup_press(
                    state == ButtonState::Pressed,
                    target_root.as_ref(),
                    &popup_roots,
                )
            {
                grab.ungrab(smithay::desktop::PopupUngrabStrategy::All);
                // Motion retires Smithay's ended pointer/keyboard grabs and
                // restores the actual outside target before normal dispatch.
                // Do not deliver a synthetic button or consume the real click.
                route = super::pointer::route_for_motion(session, time);
                session.request_redraw();
            } else if !grab.has_ended() {
                session.popup_grab = Some(grab);
                // The popup owns this event, including releases. Compositor
                // bindings and desktop interactions must not steal it.
                pointer_handle.button(
                    session,
                    &ButtonEvent {
                        serial,
                        time,
                        button,
                        state,
                    },
                );
                super::pointer::finish_frame(session, &pointer_handle);
                return;
            }
        } else if !native_popup_grab {
            session.popup_grab = None;
        }
        dismiss_lift_on_outside_press(session, route.as_ref(), button, state);
        if session.clusters.accepts_modal_input() {
            let naming_output = session
                .clusters
                .creation()
                .filter(|creation| creation.naming)
                .map(|creation| creation.output.clone());
            if let Some(output) = naming_output {
                if button == BTN_LEFT {
                    match state {
                        ButtonState::Pressed => {
                            match session
                                .render
                                .cluster_creation_overlay
                                .hit_test(&output, session.pointer.position())
                            {
                                Some(
                                    crate::render::overlays::cluster_creation::CreationOverlayHit::ConfirmButton,
                                ) => {
                                    finish_cluster_creation(session);
                                }
                                Some(
                                    crate::render::overlays::cluster_creation::CreationOverlayHit::InputCaret(caret),
                                ) => {
                                    session.clusters.begin_name_selection(caret);
                                }
                                None => {}
                            }
                            session.interactions.suppressed_buttons.suppress(button);
                        }
                        ButtonState::Released => {
                            session
                                .interactions
                                .suppressed_buttons
                                .release_is_suppressed(button);
                            session.clusters.end_name_selection();
                        }
                    }
                    session.request_redraw();
                } else if state == ButtonState::Pressed {
                    session.interactions.suppressed_buttons.suppress(button);
                } else {
                    session
                        .interactions
                        .suppressed_buttons
                        .release_is_suppressed(button);
                }
                super::pointer::finish_frame(session, &pointer_handle);
                return;
            }
            if button == BTN_LEFT
                && state == ButtonState::Pressed
                && let Some(crate::input::pointer::PointerRoute {
                    output,
                    target: crate::input::pointer::PointerTarget::Window(window),
                    ..
                }) = route.as_ref()
                && let Some(surface) = window.wl_surface()
                && let Some(id) = session.nodes.id_for_surface(surface.as_ref())
                && session.clusters.toggle_creation_member(id, &output.name())
            {
                wayland::focus::select_output(&mut session.wayland, output);
                session.request_redraw();
            }
            if state == ButtonState::Pressed {
                session.interactions.suppressed_buttons.suppress(button);
            } else {
                session
                    .interactions
                    .suppressed_buttons
                    .release_is_suppressed(button);
            }
            super::pointer::finish_frame(session, &pointer_handle);
            return;
        }
        if state == ButtonState::Pressed
            && matches!(
                route.as_ref().map(|route| &route.target),
                Some(crate::input::pointer::PointerTarget::Decoration { .. })
            )
            && bindings_enabled(session)
        {
            let modifiers = session
                .seat
                .get_keyboard()
                .expect("keyboard capability added at seat setup")
                .modifier_state();
            let context = binding_context_for_output(
                session,
                route.as_ref().map(|route| route.output.name()).as_deref(),
            );
            let action = match_pointer_bind(
                &session.keyboard.binds,
                &modifiers,
                session.keyboard.side_modifiers,
                context,
                button,
            );
            if action.as_ref().is_some_and(|action| {
                matches!(
                    action,
                    halley_config::Action::PointerMoveWindow
                        | halley_config::Action::PointerResizeWindow
                )
            }) && dispatch_pointer_grab_action(
                session,
                action.as_ref().expect("checked above"),
                route.as_ref(),
                button,
                serial,
            ) {
                super::pointer::finish_frame(session, &pointer_handle);
                return;
            }
        }
        if button == BTN_LEFT {
            match state {
                ButtonState::Pressed => {
                    if let Some(crate::input::pointer::PointerRoute {
                        target: crate::input::pointer::PointerTarget::Decoration { window, hit },
                        ..
                    }) = route.as_ref()
                    {
                        if matches!(
                            hit,
                            crate::titlebar::Hit::Control(crate::titlebar::Control::Close)
                        ) && crate::titlebar::control_enabled(
                            window,
                            crate::titlebar::Control::Close,
                        ) {
                            super::closing::capture_close_control(session, window);
                        }
                        super::focus::focus_window_from_pointer(session, window, serial);
                        match hit {
                            crate::titlebar::Hit::Control(control) => {
                                session.interactions.titlebar_pressed =
                                    Some(crate::titlebar::ButtonTarget {
                                        window: window.clone(),
                                        control: *control,
                                    });
                            }
                            crate::titlebar::Hit::Drag => {
                                let _ = super::begin_titlebar_pointer_move(
                                    session, window, serial, button,
                                );
                            }
                            crate::titlebar::Hit::Resize(handle) => {
                                let location =
                                    route.as_ref().expect("matched decoration route").location;
                                let world = halley_core::field::Vec2 {
                                    x: location.x as f32,
                                    y: location.y as f32,
                                };
                                let visual_geometry = route
                                    .as_ref()
                                    .and_then(|route| route.visual_geometry)
                                    .or_else(|| session.wayland.space.element_geometry(window))
                                    .unwrap_or_else(|| window.geometry());
                                let _ = super::begin_window_resize(
                                    session,
                                    window,
                                    *handle,
                                    button,
                                    world,
                                    visual_geometry,
                                    serial,
                                );
                            }
                        }
                        session.request_redraw();
                        super::pointer::finish_frame(session, &pointer_handle);
                        return;
                    }
                }
                ButtonState::Released => {
                    if let Some(pressed) = session.interactions.titlebar_pressed.take() {
                        let activates = route.as_ref().is_some_and(|route| {
                            matches!(
                                &route.target,
                                crate::input::pointer::PointerTarget::Decoration {
                                    window,
                                    hit: crate::titlebar::Hit::Control(control),
                                } if window == &pressed.window && control == &pressed.control
                            )
                        });
                        if activates {
                            super::activate_titlebar_control(session, &pressed, serial);
                        } else if pressed.control == crate::titlebar::Control::Close {
                            super::closing::discard_close_control(session, &pressed.window);
                        }
                        session.request_redraw();
                        super::pointer::finish_frame(session, &pointer_handle);
                        return;
                    }
                    let pending_titlebar = match &session.interactions.grab {
                        crate::input::grab::Grab::PendingWindowMove(pending)
                            if !pending.client_owned && pending.button == button =>
                        {
                            Some(pending.clone())
                        }
                        _ => None,
                    };
                    if let Some(pending) = pending_titlebar {
                        session.interactions.grab = crate::input::grab::Grab::None;
                        session
                            .cursor
                            .set_override(crate::cursor::OverrideSource::Grab, None);
                        let clicked_same_drag_region = route.as_ref().is_some_and(|route| {
                            matches!(
                                &route.target,
                                crate::input::pointer::PointerTarget::Decoration {
                                    window,
                                    hit: crate::titlebar::Hit::Drag,
                                } if window == &pending.window
                            )
                        });
                        if clicked_same_drag_region
                            && let Some(surface) = pending.window.wl_surface()
                        {
                            let now = crate::frame_clock::monotonic_now();
                            let double_click = session
                                .interactions
                                .titlebar_last_click
                                .as_ref()
                                .is_some_and(|last| {
                                    &last.surface == surface.as_ref()
                                        && now.saturating_sub(last.at)
                                            <= std::time::Duration::from_millis(500)
                                });
                            if double_click {
                                session.interactions.titlebar_last_click = None;
                                let maximized = session.maximize.contains(surface.as_ref());
                                let _ = super::set_surface_field_maximized(
                                    session,
                                    surface.as_ref(),
                                    !maximized,
                                );
                            } else {
                                session.interactions.titlebar_last_click =
                                    Some(crate::titlebar::LastClick {
                                        surface: surface.into_owned(),
                                        at: now,
                                    });
                            }
                        }
                        session.request_redraw();
                        super::pointer::finish_frame(session, &pointer_handle);
                        return;
                    }
                }
            }
        }
        if button == BTN_LEFT
            && state == ButtonState::Pressed
            && let Some(route) = route.as_ref()
        {
            wayland::focus::select_output(&mut session.wayland, &route.output);
        }
        let mut intercepted = false;
        let mut finishing_client_move = false;
        let now = crate::frame_clock::monotonic_now();
        let action_target = (button == BTN_LEFT
            && state == ButtonState::Pressed
            && !session.shell.focus_cycle.is_open())
        .then(|| cluster_action_at_pointer(session, now))
        .flatten();
        if let Some((cluster, control, output)) = action_target {
            let output_name = output.name();
            wayland::focus::select_output(&mut session.wayland, &output);
            session.clusters.close_bloom(&output_name, now);
            session.clusters.set_hovered_core(None, now);
            match control {
                crate::clusters::ClusterActionControl::Close => {
                    session.request_cluster_dissolution(cluster);
                }
                crate::clusters::ClusterActionControl::Edit => {
                    if session.clusters.begin_rename(cluster) {
                        session.cursor.set_override(
                            crate::cursor::OverrideSource::Modal,
                            Some(smithay::input::pointer::CursorIcon::Default),
                        );
                    } else {
                        session.shell.overlays.show_error(
                            output_name,
                            "Cluster name could not be edited",
                            3_000,
                            now,
                        );
                    }
                }
            }
            session.interactions.suppressed_buttons.suppress(button);
            session.request_redraw();
            intercepted = true;
        }
        let bloom_token = (button == BTN_LEFT
            && state == ButtonState::Pressed
            && !session.shell.focus_cycle.is_open()
            && !intercepted)
            .then(|| cluster_bloom_at_pointer(session, now))
            .flatten();
        if button == BTN_LEFT
            && state == ButtonState::Pressed
            && !session.shell.focus_cycle.is_open()
            && let Some((token, output)) = bloom_token
            && session.clusters.begin_bloom_pull(token, output.name())
        {
            wayland::focus::select_output(&mut session.wayland, &output);
            session.interactions.suppressed_buttons.suppress(button);
            session.clusters.set_overlay_hovered(None);
            session.cursor.set_override(
                crate::cursor::OverrideSource::Grab,
                Some(smithay::input::pointer::CursorIcon::Grabbing),
            );
            session.request_redraw();
            intercepted = true;
        }
        if button == BTN_LEFT
            && state == ButtonState::Pressed
            && !session.shell.focus_cycle.is_open()
            && !intercepted
            && let Some((member, output, local)) =
                cluster_overflow_at_pointer(session, crate::frame_clock::monotonic_now())
            && session.clusters.begin_overflow_drag(
                &output.name(),
                member,
                local,
                crate::frame_clock::monotonic_now(),
            )
        {
            wayland::focus::select_output(&mut session.wayland, &output);
            session.interactions.suppressed_buttons.suppress(button);
            session.clusters.set_overlay_hovered(None);
            session.cursor.set_override(
                crate::cursor::OverrideSource::Grab,
                Some(smithay::input::pointer::CursorIcon::Grabbing),
            );
            session.request_redraw();
            intercepted = true;
        }
        if button == BTN_LEFT
            && state == ButtonState::Pressed
            && !session.shell.focus_cycle.is_open()
            && !intercepted
            && let Some((id, output)) = cluster_at_pointer(session)
            && let Some(metadata) = session.clusters.metadata(id)
            && let Some(output_geometry) = session.wayland.space.output_geometry(&output)
            && let Some(camera) = session.cameras.get(&output.name())
        {
            wayland::focus::select_output(&mut session.wayland, &output);
            if let Some(core) = session.clusters.core_node(id) {
                session
                    .nodes
                    .focus(Some(core), session.start_time.elapsed().as_millis() as u64);
            }
            let center =
                crate::nodes::screen_from_world(metadata.core_position, camera, output_geometry);
            let screen_offset = halley_core::field::Vec2 {
                x: center.x as f32 - position_after.0 as f32,
                y: center.y as f32 - position_after.1 as f32,
            };
            session
                .clusters
                .close_bloom(&output.name(), crate::frame_clock::monotonic_now());
            session
                .clusters
                .set_hovered_core(None, crate::frame_clock::monotonic_now());
            let modifiers = session
                .seat
                .get_keyboard()
                .expect("keyboard capability added at seat setup")
                .modifier_state();
            if crate::input::mod_key_held(
                &modifiers,
                session.keyboard.side_modifiers,
                session.keyboard.effective_mod,
            ) {
                if session
                    .clusters
                    .core_node(id)
                    .is_none_or(|core| !crate::session::node_user_pinned(session, core))
                {
                    session.interactions.grab =
                        crate::input::grab::Grab::MoveClusterCore { id, screen_offset };
                    session.cursor.set_override(
                        crate::cursor::OverrideSource::Grab,
                        Some(smithay::input::pointer::CursorIcon::Grabbing),
                    );
                }
            } else {
                session.interactions.grab = crate::input::grab::Grab::PendingClusterCore {
                    id,
                    output: output.name(),
                    press_screen: Point::<f64, Logical>::from(position_after),
                    screen_offset,
                };
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Grab, None);
            }
            session.interactions.suppressed_buttons.suppress(button);
            session.request_redraw();
            intercepted = true;
        }
        if button == BTN_LEFT
            && state == ButtonState::Pressed
            && !session.shell.focus_cycle.is_open()
            && let Some((target, output)) = bearing_at_pointer(
                &session.pointer,
                &session.wayland.space,
                &session.shell.bearings,
            )
        {
            wayland::focus::select_output(&mut session.wayland, &output);
            let revealed = match target {
                crate::shell::bearings::BearingTarget::Node(id) => {
                    crate::nodes::focus_or_reveal_node(session, id, serial, true)
                }
                crate::shell::bearings::BearingTarget::ClusterCore { core, .. } => {
                    crate::nodes::reveal_cluster_core(session, core, serial, true)
                }
            };
            if revealed {
                session.interactions.suppressed_buttons.suppress(button);
                intercepted = true;
            }
        }
        let modifiers = session
            .seat
            .get_keyboard()
            .expect("keyboard capability added at seat setup")
            .modifier_state();
        let bindings_enabled = bindings_enabled(session);
        let on_background = route.as_ref().is_some_and(|route| {
            matches!(
                &route.target,
                crate::input::pointer::PointerTarget::Background
            )
        });
        let binding_context = binding_context_for_output(
            session,
            route.as_ref().map(|route| route.output.name()).as_deref(),
        );
        if !intercepted {
            match process_pointer_binding(
                &session.keyboard.binds,
                &modifiers,
                session.keyboard.side_modifiers,
                binding_context,
                button,
                state,
                bindings_enabled,
                &mut session.interactions.suppressed_buttons,
            ) {
                PointerBindingResult::Action(action) => {
                    let pointer_grab = matches!(
                        action,
                        halley_config::Action::PointerMoveWindow
                            | halley_config::Action::PointerResizeWindow
                            | halley_config::Action::PointerPanField
                            | halley_config::Action::PointerDragPan
                    );
                    let handled = if pointer_grab {
                        dispatch_pointer_grab_action(
                            session,
                            &action,
                            route.as_ref(),
                            button,
                            serial,
                        )
                    } else {
                        let output_name =
                            route.as_ref().map(|route| route.output.name().to_string());
                        actions::dispatch(
                            session,
                            action,
                            socket_name,
                            output_name.as_deref(),
                            None,
                            actions::DispatchOrigin::Other,
                        );
                        true
                    };
                    if pointer_grab || !handled {
                        session
                            .interactions
                            .suppressed_buttons
                            .release_is_suppressed(button);
                    }
                    intercepted = handled;
                }
                PointerBindingResult::SuppressedRelease => intercepted = true,
                PointerBindingResult::Unhandled => {}
            }
        }

        if !intercepted
            && button == BTN_LEFT
            && state == ButtonState::Pressed
            && let Some((id, output)) = node_at_pointer(session)
            && let Some(record) = session.nodes.record(id).cloned()
            && let Some(node_position) = session.nodes.field.node(id).map(|node| node.pos)
            && let Some(output_geometry) = session.wayland.space.output_geometry(&output)
            && let Some(camera) = session.cameras.get(&output.name())
        {
            wayland::focus::select_output(&mut session.wayland, &output);
            let center = crate::nodes::screen_from_world(node_position, camera, output_geometry);
            let screen_offset = halley_core::field::Vec2 {
                x: center.x as f32 - position_after.0 as f32,
                y: center.y as f32 - position_after.1 as f32,
            };
            session
                .nodes
                .clear_hover(crate::frame_clock::monotonic_now());
            super::focus::focus_node_from_pointer(session, id, &output, serial);
            let mod_held = crate::input::mod_key_held(
                &modifiers,
                session.keyboard.side_modifiers,
                session.keyboard.effective_mod,
            );
            if mod_held {
                if !crate::session::node_user_pinned(session, id) {
                    session.nodes.clear_direct_motion(id);
                    session.interactions.grab = crate::input::grab::Grab::MoveNode {
                        id,
                        surface: record.surface,
                        screen_offset,
                        last_world: node_position,
                        last_update: crate::frame_clock::monotonic_now(),
                        velocity: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
                    };
                    session.cursor.set_override(
                        crate::cursor::OverrideSource::Grab,
                        Some(smithay::input::pointer::CursorIcon::Grabbing),
                    );
                }
            } else {
                session.interactions.grab = crate::input::grab::Grab::PendingNode {
                    id,
                    surface: record.surface,
                    press_screen: Point::<f64, Logical>::from(position_after),
                    screen_offset,
                };
            }
            intercepted = true;
        }

        // Old Halley kept a bloom open while another window or compositor
        // marker was clicked or grabbed. Only an otherwise-unhandled press on
        // the empty Field dismissed it; core presses close their own bloom in
        // the dedicated core path above.
        if button == BTN_LEFT
            && state == ButtonState::Pressed
            && plain_background_press_dismisses_bloom(intercepted, on_background)
            && let Some(route) = route.as_ref()
        {
            let now = crate::frame_clock::monotonic_now();
            if session.clusters.close_bloom(&route.output.name(), now) {
                session.clusters.set_overlay_hovered(None);
                session.clusters.set_hovered_core(None, now);
                session.request_redraw();
            }
        }

        if !intercepted && button == BTN_LEFT {
            match state {
                ButtonState::Pressed => match route.as_ref().map(|route| &route.target) {
                    Some(crate::input::pointer::PointerTarget::Window(window)) => {
                        super::focus::focus_window_from_pointer(session, window, serial);
                    }
                    Some(crate::input::pointer::PointerTarget::Layer(layer)) => {
                        super::focus::focus_layer(session, Some(layer.clone()), serial);
                    }
                    Some(crate::input::pointer::PointerTarget::Decoration { .. }) => {
                        intercepted = true;
                    }
                    Some(crate::input::pointer::PointerTarget::Background) => {
                        super::focus::focus_layer(session, None, serial);
                    }
                    None => {}
                },
                ButtonState::Released => {
                    let released_window = match &session.interactions.grab {
                        crate::input::grab::Grab::MoveWindow {
                            id,
                            window,
                            cluster_drag,
                            button: move_button,
                            client_owned,
                            last_world,
                            ..
                        } if *move_button == button => Some((
                            *id,
                            window.clone(),
                            cluster_drag.clone(),
                            *last_world,
                            *client_owned,
                        )),
                        _ => None,
                    };
                    if let Some((id, window, cluster_drag, last_world, client_owned)) =
                        released_window
                    {
                        finishing_client_move = client_owned;
                        let now = crate::frame_clock::monotonic_now();
                        if session.nodes.physics.enabled {
                            let _ = crate::nodes::tick_physics(session, now);
                        }
                        let cluster_release = cluster_drag.as_ref().and_then(|drag| {
                            let output = session
                                .wayland
                                .space
                                .outputs()
                                .find(|output| output.name() == drag.output)?
                                .clone();
                            let output_geometry = session.wayland.space.output_geometry(&output)?;
                            let work_area = smithay::desktop::layer_map_for_output(&output)
                                .non_exclusive_zone();
                            let origin =
                                super::presented_window_rect(session, &window, &output, now)
                                    .map(|geometry| geometry.to_logical(1))
                                    .or_else(|| {
                                        let geometry =
                                            session.wayland.space.element_geometry(&window)?;
                                        Some(Rectangle::new(
                                            geometry.loc - output_geometry.loc,
                                            geometry.size,
                                        ))
                                    })?;
                            let global_work_area =
                                Rectangle::new(output_geometry.loc + work_area.loc, work_area.size);
                            let pointer_inside = global_work_area.to_f64().contains(Point::<
                                f64,
                                Logical,
                            >::from(
                                session.pointer.position(),
                            ));
                            Some((
                                drag.clone(),
                                output.name(),
                                work_area,
                                origin,
                                pointer_inside,
                            ))
                        });
                        let active_workspace_release =
                            (cluster_drag.is_none())
                                .then(|| {
                                    let (output, output_geometry) = output_at_pointer(
                                        &session.wayland.space,
                                        session.pointer.position(),
                                    )?;
                                    session.clusters.active_on(&output.name())?;
                                    let work_area = smithay::desktop::layer_map_for_output(&output)
                                        .non_exclusive_zone();
                                    let global_work_area = Rectangle::new(
                                        output_geometry.loc + work_area.loc,
                                        work_area.size,
                                    );
                                    if !global_work_area.to_f64().contains(
                                        Point::<f64, Logical>::from(session.pointer.position()),
                                    ) {
                                        return None;
                                    }
                                    let origin = super::presented_window_rect(
                                        session, &window, &output, now,
                                    )
                                    .map(|geometry| geometry.to_logical(1))
                                    .or_else(|| {
                                        let geometry =
                                            session.wayland.space.element_geometry(&window)?;
                                        Some(Rectangle::new(
                                            geometry.loc - output_geometry.loc,
                                            geometry.size,
                                        ))
                                    })?;
                                    Some((output.name(), work_area, origin))
                                })
                                .flatten();
                        let joined = if cluster_drag.is_none() && active_workspace_release.is_none()
                        {
                            id.and_then(|member| {
                                session
                                    .clusters
                                    .commit_join_candidate(&mut session.nodes.field, member)
                            })
                        } else {
                            None
                        };
                        session.interactions.grab = crate::input::grab::Grab::None;
                        session
                            .cursor
                            .set_override(crate::cursor::OverrideSource::Grab, None);
                        let mut cluster_drop_handled = false;
                        if let (
                            Some(id),
                            Some((drag, member_output, work_area, origin, pointer_inside)),
                        ) = (id, cluster_release)
                        {
                            match drag.kind {
                                crate::input::grab::ClusterWindowDragKind::Floating
                                    if pointer_inside =>
                                {
                                    cluster_drop_handled =
                                        session.clusters.finish_floating_member_drag(
                                            &drag.output,
                                            &member_output,
                                            id,
                                            work_area,
                                            origin,
                                        );
                                }
                                crate::input::grab::ClusterWindowDragKind::Layout(_)
                                    if pointer_inside =>
                                {
                                    cluster_drop_handled = session.clusters.finish_workspace_drag(
                                        &drag.output,
                                        id,
                                        work_area,
                                        origin,
                                        now,
                                    );
                                }
                                crate::input::grab::ClusterWindowDragKind::Floating
                                | crate::input::grab::ClusterWindowDragKind::Layout(_) => {
                                    cluster_drop_handled =
                                        session.clusters.detach_active_member_for_drag(
                                            &mut session.nodes.field,
                                            &drag.output,
                                            crate::clusters::ClusterDragMember {
                                                cluster_id: drag.cluster_id,
                                                node_id: id,
                                            },
                                            work_area,
                                            last_world,
                                            now,
                                        );
                                }
                            }
                            if cluster_drop_handled {
                                session.nodes.clear_direct_motion(id);
                                super::reconcile_cluster_surfaces(session, &drag.output);
                                if member_output != drag.output {
                                    super::reconcile_cluster_surfaces(session, &member_output);
                                }
                                session.request_redraw();
                            }
                        }
                        if let (Some(id), Some((output, work_area, origin))) =
                            (id, active_workspace_release)
                            && session.clusters.join_active_member_front(
                                &mut session.nodes.field,
                                &output,
                                id,
                                work_area,
                                origin,
                                now,
                            )
                        {
                            cluster_drop_handled = true;
                            session.nodes.clear_direct_motion(id);
                            super::reconcile_cluster_surfaces(session, &output);
                            session.request_redraw();
                        }
                        let drag_presentation_released = session.clusters.cancel_window_drag();
                        if cluster_drag.is_some() && !cluster_drop_handled {
                            if let (Some(id), Some(drag)) = (id, cluster_drag.as_ref()) {
                                let output = session
                                    .wayland
                                    .space
                                    .outputs()
                                    .find(|output| output.name() == drag.output)
                                    .cloned();
                                if let Some(output) = output {
                                    crate::nodes::set_collapsed_output(session, id, &output);
                                    super::reconcile_cluster_surfaces(session, &drag.output);
                                }
                            }
                            session.request_redraw();
                        } else if joined.is_some() {
                            if let Some(id) = id {
                                session.nodes.clear_direct_motion(id);
                            }
                            session.request_redraw();
                        } else if !cluster_drop_handled
                            && session.nodes.physics.enabled
                            && let Some(id) = id
                        {
                            session.nodes.lock_released_window(id, now);
                            session.request_redraw();
                        } else if drag_presentation_released {
                            session.request_redraw();
                        }
                        intercepted = true;
                    } else if matches!(
                        session.interactions.grab,
                        crate::input::grab::Grab::Pan { .. }
                    ) {
                        session.interactions.grab = crate::input::grab::Grab::None;
                        session
                            .cursor
                            .set_override(crate::cursor::OverrideSource::Grab, None);
                        intercepted = true;
                    }
                }
            }
        }

        if !intercepted
            && state == ButtonState::Released
            && matches!(
                &session.interactions.grab,
                crate::input::grab::Grab::ResizeWindow(resize) if resize.button == button
            )
        {
            session.interactions.grab = crate::input::grab::Grab::None;
            crate::input::grab::release_resize_anchor(&mut session.interactions.resize_anchor);
            session
                .cursor
                .set_override(crate::cursor::OverrideSource::Grab, None);
            intercepted = true;
        }

        if forward_pointer_button(intercepted, finishing_client_move) {
            if let Some(window) = forwarded_steam_close_button(
                &mut session.interactions.steam_close_pressed,
                steam_close_press,
                button,
                state,
                steam_client_close_target(route.as_ref()),
            ) {
                super::closing::start_steam_client_close_control(session, &window);
            }
            pointer_handle.button(
                session,
                &ButtonEvent {
                    serial,
                    time,
                    button,
                    state,
                },
            );
        }
        super::pointer::finish_frame(session, &pointer_handle);
    }

    if let InputEvent::PointerAxis { event: axis_event } = event {
        if super::gesture::handle_axis_pan(session, axis_event) {
            super::pointer::finish_frame(session, &pointer_handle);
            return;
        }
        let route = super::pointer::route_for_discrete_input(session, axis_event.time_msec());
        let output_name = route.as_ref().map(|route| route.output.name().to_string());
        let bindings_enabled = bindings_enabled(session);
        let modifiers = session
            .seat
            .get_keyboard()
            .expect("keyboard capability added at seat setup")
            .modifier_state();
        let sides = session.keyboard.side_modifiers;
        let binding_context = binding_context_for_output(session, output_name.as_deref());
        let result = process_wheel_bindings(
            axis_event,
            &mut session.interactions.wheel_accumulator,
            bindings_enabled,
            |direction| {
                match_wheel_bind(
                    &session.keyboard.binds,
                    &modifiers,
                    sides,
                    binding_context,
                    direction,
                )
            },
        );
        let bound_action = !result.actions.is_empty();
        for (direction, action) in result.actions {
            eventline::debug!("keybinds: wheel {direction:?} + {modifiers:?} -> {action:?}");
            actions::dispatch(
                session,
                action,
                socket_name,
                output_name.as_deref(),
                None,
                actions::DispatchOrigin::Other,
            );
        }

        if !bound_action
            && scroll_cluster_overflow_at_pointer(
                session,
                axis_event,
                crate::frame_clock::monotonic_now(),
            )
        {
            session.request_redraw();
            super::pointer::finish_frame(session, &pointer_handle);
            return;
        }

        if result.forward_horizontal || result.forward_vertical {
            let frame = axis_frame_filtered(
                axis_event,
                result.forward_horizontal,
                result.forward_vertical,
            );
            pointer_handle.axis(session, frame);
        }
        super::pointer::finish_frame(session, &pointer_handle);
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
                                surface_id: surface.id().protocol_id(),
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
    use smithay::backend::input::{ButtonState, KeyState};
    use smithay::utils::{Logical, Point, Rectangle, Size};

    use super::actions::{cluster_blocks_zoom, window_action_output};
    use super::keyboard::{ModalKeyRouting, modal_key_routing};
    use super::{BTN_LEFT, BTN_RIGHT, PendingWindowMoveMotion};
    use super::{
        activation_shows_cluster_indicator, bloom_drag_handoff, collapsed_node_drop_origin,
        drag_threshold_reached, forward_pointer_button, outside_lift_press_dismisses,
        pending_window_move_motion, plain_background_press_dismisses_bloom,
        pointer_move_falls_back_to_field_pan, preferred_cluster_navigation_focus,
        releases_pending_window_move, sampled_drag_velocity, shortcut_policy_allows_bindings,
        stacking_cycle_direction, typing_abandons_bloom,
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
    fn move_window_grab_becomes_field_pan_on_background() {
        assert!(pointer_move_falls_back_to_field_pan(
            &crate::input::pointer::PointerTarget::Background
        ));
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
    fn shortcut_policy_respects_shell_and_client_inhibition() {
        assert!(shortcut_policy_allows_bindings(false, false));
        assert!(!shortcut_policy_allows_bindings(true, false));
        assert!(!shortcut_policy_allows_bindings(false, true));
        assert!(!shortcut_policy_allows_bindings(true, true));
    }

    #[test]
    fn window_actions_follow_pointer_only_in_hover_mode() {
        assert_eq!(
            window_action_output(halley_config::FocusMode::Hover, Some("right"), Some("left"),),
            Some("right".to_string())
        );
        assert_eq!(
            window_action_output(halley_config::FocusMode::Click, Some("right"), Some("left"),),
            Some("left".to_string())
        );
        assert_eq!(
            window_action_output(halley_config::FocusMode::Hover, None, Some("left")),
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
