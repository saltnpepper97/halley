//! Pointer-driven window movement and resize transactions.

use super::{
    Session, SessionDriver, configure_field_geometry, focus, node_user_pinned, pointer,
    reconcile_cluster_surfaces,
};
use smithay::utils::Rectangle;
use smithay::wayland::seat::WaylandFocus;

/// Hand an interaction the position actually on screen, instead of the already
/// committed movement endpoint. The interaction now owns the window's geometry.
pub(crate) fn settle_window_movement<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
) {
    let Some(surface) = window.wl_surface() else {
        return;
    };
    let Some(current) = session.wayland.space.element_geometry(window) else {
        return;
    };
    let now = crate::frame_clock::monotonic_now();
    let visible = session
        .window_animations
        .movement_visual(surface.as_ref(), now, current);
    session.window_animations.cancel_move(surface.as_ref());
    if let Some(visible) = visible {
        session.wayland.space.relocate_element(window, visible.loc);
        if crate::xwayland::is_x11(window) {
            crate::xwayland::configure_window(session, window, visible);
        }
    }
}

pub(crate) fn begin_window_resize<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    handle: crate::input::grab::ResizeHandle,
    button: u32,
    cursor: halley_core::field::Vec2,
    visual_geometry: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    serial: smithay::utils::Serial,
) -> bool {
    let surface = window.wl_surface();
    if surface
        .as_ref()
        .is_some_and(|surface| session.maximize.contains(surface.as_ref()))
        || surface
            .as_ref()
            .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
            .is_some_and(|id| session.clusters.active_layout_for_member(id).is_some())
        || surface
            .as_ref()
            .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
            .is_some_and(|id| node_user_pinned(session, id))
    {
        return false;
    }
    settle_window_movement(session, window);
    let Some(start_rect) = session.wayland.space.element_geometry(window) else {
        return false;
    };
    focus::focus_window_from_pointer(session, window, serial);
    session.interactions.grab =
        crate::input::grab::Grab::ResizeWindow(crate::input::grab::ResizeState {
            window: window.clone(),
            handle,
            button,
            start_rect,
            start_cursor: cursor,
            start_screen: session.pointer.position(),
            screen_to_source_scale: crate::input::grab::resize_screen_to_source_scale(
                start_rect,
                visual_geometry,
            ),
            target_size: start_rect.size,
            preview_size: halley_core::field::Vec2 {
                x: start_rect.size.w as f32,
                y: start_rect.size.h as f32,
            },
            last_smooth_tick: crate::frame_clock::monotonic_now(),
        });
    session.interactions.resize_anchor =
        window.toplevel().map(|_| crate::input::grab::ResizeAnchor {
            window: window.clone(),
            handle,
            phase: crate::input::grab::ResizePhase::Ongoing,
            last_configure: None,
            last_size: start_rect.size,
        });
    session.cursor.set_override(
        crate::cursor::OverrideSource::Grab,
        Some(handle.cursor_icon()),
    );
    true
}

pub(crate) fn begin_pointer_resize<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    handle: crate::input::grab::ResizeHandle,
    button: u32,
) -> bool {
    let Some(route) = pointer::route_client(session) else {
        return false;
    };
    let routed_window = match &route.target {
        crate::input::pointer::PointerTarget::Window(routed)
        | crate::input::pointer::PointerTarget::Decoration { window: routed, .. } => routed,
        _ => return false,
    };
    if routed_window != window {
        return false;
    }
    let cursor = halley_core::field::Vec2 {
        x: route.location.x as f32,
        y: route.location.y as f32,
    };
    let visual_geometry = route.visual_geometry.unwrap_or_else(|| {
        session
            .wayland
            .space
            .element_geometry(window)
            .unwrap_or_else(|| Rectangle::from_size((1, 1).into()))
    });
    begin_window_resize(
        session,
        window,
        handle,
        button,
        cursor,
        visual_geometry,
        smithay::utils::SERIAL_COUNTER.next_serial(),
    )
}

/// Arms a client-requested move without committing any move side effects.
/// Toolkits commonly send this request from their title-bar press handler,
/// before they know whether the gesture will become a click or a drag.
pub(crate) fn begin_client_pointer_move<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    serial: smithay::utils::Serial,
    button: u32,
) -> bool {
    begin_pending_pointer_move(session, window, serial, button, true)
}

pub(crate) fn begin_titlebar_pointer_move<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    serial: smithay::utils::Serial,
    button: u32,
) -> bool {
    begin_pending_pointer_move(session, window, serial, button, false)
}

fn begin_pending_pointer_move<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    serial: smithay::utils::Serial,
    button: u32,
    client_owned: bool,
) -> bool {
    if !matches!(session.interactions.grab, crate::input::grab::Grab::None)
        || !crate::window::accepts_compositor_grab(window)
        || window
            .wl_surface()
            .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
            .is_some_and(|id| node_user_pinned(session, id))
        || window.wl_surface().is_some_and(|surface| {
            session
                .fullscreen
                .is_fullscreen_or_pending(surface.as_ref())
        })
    {
        return false;
    }
    let Some(route) = pointer::route_client(session) else {
        return false;
    };
    let routed_window = match &route.target {
        crate::input::pointer::PointerTarget::Window(routed)
        | crate::input::pointer::PointerTarget::Decoration { window: routed, .. } => routed,
        _ => return false,
    };
    if routed_window != window {
        return false;
    }
    let visual_geometry = route.visual_geometry.unwrap_or_else(|| {
        session
            .wayland
            .space
            .element_geometry(window)
            .unwrap_or_else(|| window.geometry())
    });
    let maximized = window
        .wl_surface()
        .is_some_and(|surface| session.maximize.contains(surface.as_ref()));
    session.interactions.grab =
        crate::input::grab::Grab::PendingWindowMove(crate::input::grab::PendingWindowMove {
            window: window.clone(),
            serial,
            button,
            press_screen: smithay::utils::Point::from(session.pointer.position()),
            output: route.output.name(),
            visual_geometry,
            maximized,
            client_owned,
        });
    true
}

/// Starts a compositor-owned move immediately. Client title bars use
/// `begin_client_pointer_move` and promote only after real pointer motion.
pub(crate) fn begin_pointer_move<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    serial: smithay::utils::Serial,
    button: u32,
) -> bool {
    begin_pointer_move_active(session, window, serial, button, false, None, false)
}

pub(crate) fn begin_pointer_edge_pan<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    serial: smithay::utils::Serial,
    button: u32,
) -> bool {
    begin_pointer_move_active(session, window, serial, button, false, None, true)
}

pub(crate) fn activate_client_pointer_move<D: SessionDriver>(
    session: &mut Session<D>,
    pending: crate::input::grab::PendingWindowMove,
) -> bool {
    let window = pending.window.clone();
    begin_pointer_move_active(
        session,
        &window,
        pending.serial,
        pending.button,
        pending.client_owned,
        Some(pending),
        false,
    )
}

fn begin_pointer_move_active<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    serial: smithay::utils::Serial,
    button: u32,
    client_owned: bool,
    pending: Option<crate::input::grab::PendingWindowMove>,
    edge_pan_requested: bool,
) -> bool {
    if !crate::window::accepts_compositor_grab(window)
        || window
            .wl_surface()
            .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
            .is_some_and(|id| node_user_pinned(session, id))
        || window.wl_surface().is_some_and(|surface| {
            session
                .fullscreen
                .is_fullscreen_or_pending(surface.as_ref())
        })
    {
        return false;
    }
    let (output_name, visual_geometry, press_screen, was_maximized) =
        if let Some(pending) = pending.as_ref() {
            (
                pending.output.clone(),
                pending.visual_geometry,
                pending.press_screen,
                pending.maximized,
            )
        } else {
            let Some(route) = pointer::route_client(session) else {
                return false;
            };
            let routed_window = match &route.target {
                crate::input::pointer::PointerTarget::Window(routed)
                | crate::input::pointer::PointerTarget::Decoration { window: routed, .. } => routed,
                _ => return false,
            };
            if routed_window != window {
                return false;
            }
            let visual = route.visual_geometry.unwrap_or_else(|| {
                session
                    .wayland
                    .space
                    .element_geometry(window)
                    .unwrap_or_else(|| window.geometry())
            });
            let maximized = window
                .wl_surface()
                .is_some_and(|surface| session.maximize.contains(surface.as_ref()));
            (
                route.output.name(),
                visual,
                smithay::utils::Point::from(session.pointer.position()),
                maximized,
            )
        };
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)
        .cloned()
    else {
        return false;
    };
    let Some(output_geometry) = session.wayland.space.output_geometry(&output) else {
        return false;
    };
    let Some(camera) = session.cameras.get(&output_name) else {
        return false;
    };
    if edge_pan_requested && was_maximized {
        return false;
    }
    let pointer_position = session.pointer.position();
    let pointer_world =
        crate::input::grab::screen_to_world_on_output(pointer_position, camera, output_geometry);
    let press_world = crate::input::grab::screen_to_world_on_output(
        (press_screen.x, press_screen.y),
        camera,
        output_geometry,
    );
    let world = halley_core::field::Vec2 {
        x: pointer_world.x,
        y: pointer_world.y,
    };
    settle_window_movement(session, window);
    let Some(window_location) = session.wayland.space.element_location(window) else {
        return false;
    };
    let restore = was_maximized
        .then(|| {
            window
                .wl_surface()
                .and_then(|surface| session.maximize.restore(surface.as_ref()))
        })
        .flatten();

    let id = window
        .wl_surface()
        .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()));
    let mut cluster_drag = id.and_then(|id| {
        let cluster_id = session.clusters.cluster_for_member(id)?;
        let metadata = session.clusters.metadata(cluster_id)?;
        let floating = session.clusters.is_member_floating(id);
        (session.clusters.active_on(&metadata.output) == Some(cluster_id)
            && (floating || metadata.output == output_name))
            .then(|| {
                let kind = if floating {
                    crate::input::grab::ClusterWindowDragKind::Floating
                } else {
                    crate::input::grab::ClusterWindowDragKind::Layout(metadata.layout)
                };
                crate::input::grab::ClusterWindowDrag {
                    cluster_id,
                    output: metadata.output.clone(),
                    kind,
                    on_origin_output: metadata.output == output_name,
                }
            })
    });
    if cluster_drag.as_ref().is_some_and(|drag| {
        matches!(
            drag.kind,
            crate::input::grab::ClusterWindowDragKind::Layout(
                halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Stacking
            )
        ) && id != session.clusters.first_member(drag.cluster_id)
    }) {
        return false;
    }
    if edge_pan_requested {
        let output_right = output_geometry.loc.x + output_geometry.size.w;
        let output_bottom = output_geometry.loc.y + output_geometry.size.h;
        let visual_right = visual_geometry.loc.x + visual_geometry.size.w;
        let visual_bottom = visual_geometry.loc.y + visual_geometry.size.h;
        let fully_visible = visual_geometry.loc.x >= output_geometry.loc.x
            && visual_geometry.loc.y >= output_geometry.loc.y
            && visual_right <= output_right
            && visual_bottom <= output_bottom;
        if id.is_none()
            || cluster_drag.is_some()
            || !fully_visible
            || session.clusters.active_on(&output_name).is_some()
        {
            return false;
        }
    }
    focus::focus_window_from_pointer(session, window, serial);

    let mut cluster_grab_location = None;
    let mut cluster_drag_rect = None;
    let anchor = if cluster_drag.is_some() {
        let offset = crate::input::grab::screen_grip_offset(
            (press_screen.x, press_screen.y),
            visual_geometry.loc,
        );
        cluster_drag_rect = Some(Rectangle::new(
            visual_geometry.loc - output_geometry.loc,
            visual_geometry.size,
        ));
        let camera = session
            .cameras
            .get(&output_name)
            .expect("the pointer output camera was validated above");
        cluster_grab_location = Some(crate::input::grab::world_location_from_screen_grip(
            pointer_position,
            offset,
            camera,
            output_geometry,
        ));
        crate::input::grab::WindowGrabAnchor::Screen(offset)
    } else if was_maximized {
        let source = restore
            .as_ref()
            .map(|restore| restore.geometry)
            .or_else(|| session.wayland.space.element_geometry(window))
            .unwrap_or_else(|| window.geometry());
        let ratio_x = ((press_screen.x - f64::from(visual_geometry.loc.x))
            / f64::from(visual_geometry.size.w.max(1)))
        .clamp(0.0, 1.0);
        let ratio_y = ((press_screen.y - f64::from(visual_geometry.loc.y))
            / f64::from(visual_geometry.size.h.max(1)))
        .clamp(0.0, 1.0);
        crate::input::grab::WindowGrabAnchor::Source(halley_core::field::Vec2 {
            x: -(source.size.w as f32 * ratio_x as f32),
            y: -(source.size.h as f32 * ratio_y as f32),
        })
    } else {
        crate::input::grab::WindowGrabAnchor::Source(halley_core::field::Vec2 {
            x: window_location.x as f32 - press_world.x,
            y: window_location.y as f32 - press_world.y,
        })
    };

    if let Some(expected) = restore.as_ref() {
        let maximize_output = session
            .maximize
            .output_for_surface(&expected.surface)
            .map(str::to_owned);
        if let Some(restore) = session.maximize.take_restore(&expected.surface) {
            session.render.fullscreen_textures.remove(&restore.surface);
            configure_field_geometry(session, &restore);
            if let Some(output) = maximize_output {
                let _ = session.cameras.apply_field_maximize(&output, None);
            }
        }
    }

    if let (Some(id), Some(location), Some(drag)) =
        (id, cluster_grab_location, cluster_drag.as_mut())
    {
        let Some(rect) = cluster_drag_rect else {
            return false;
        };
        let began = match drag.kind {
            crate::input::grab::ClusterWindowDragKind::Layout(_) => session
                .clusters
                .begin_workspace_drag(&drag.output, id, rect),
            crate::input::grab::ClusterWindowDragKind::Floating => session
                .clusters
                .begin_floating_member_drag(&drag.output, &output_name, id, rect),
        };
        if !began {
            return false;
        }
        reconcile_cluster_surfaces(session, &output_name);
        session.wayland.space.relocate_element(window, location);
        crate::window::raise_managed(&mut session.wayland, window);
        session.xwayland.raise_window(window);
    } else if let Some(id) = id {
        let _ = session.clusters.begin_field_drag(id);
        crate::window::raise_managed(&mut session.wayland, window);
        session.xwayland.raise_window(window);
    }

    if let Some(id) = id {
        session.nodes.clear_direct_motion(id);
    }
    let drag_size = restore.as_ref().map(|restore| restore.geometry.size);
    let center = drag_size
        .and_then(|size| {
            let camera = session.cameras.get(&output_name)?;
            let location = anchor.world_location(pointer_position, camera, output_geometry);
            Some(halley_core::field::Vec2 {
                x: location.x as f32 + size.w as f32 * 0.5,
                y: location.y as f32 + size.h as f32 * 0.5,
            })
        })
        .or_else(|| {
            session
                .wayland
                .space
                .element_geometry(window)
                .map(|geometry| halley_core::field::Vec2 {
                    x: geometry.loc.x as f32 + geometry.size.w as f32 * 0.5,
                    y: geometry.loc.y as f32 + geometry.size.h as f32 * 0.5,
                })
        })
        .unwrap_or(world);
    session.interactions.grab = crate::input::grab::Grab::MoveWindow {
        id,
        window: window.clone(),
        cluster_drag,
        drag_size,
        button,
        client_owned,
        anchor,
        edge_pan: edge_pan_requested.then(|| {
            crate::input::grab::WindowEdgePan::new(
                output_name.clone(),
                crate::frame_clock::monotonic_now(),
            )
        }),
        last_world: center,
        last_update: crate::frame_clock::monotonic_now(),
        velocity: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
    };
    session.cursor.set_override(
        crate::cursor::OverrideSource::Grab,
        Some(smithay::input::pointer::CursorIcon::Grabbing),
    );
    true
}
