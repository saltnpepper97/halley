use super::*;

fn selected<D: crate::session::SessionDriver>(
    session: &crate::session::Session<D>,
) -> Result<Output, String> {
    let output = crate::wayland::focus::selected_output(&session.wayland)
        .cloned()
        .ok_or("no selected monitor")?;
    if session.clusters.active_on(&output.name()).is_some() {
        return Err("this action requires the Field".into());
    }
    Ok(output)
}

fn transfer_center(camera: Vec2, origin: (i32, i32)) -> Vec2 {
    Vec2 {
        x: camera.x + origin.0 as f32,
        y: camera.y + origin.1 as f32,
    }
}

fn pan_delta(direction: halley_config::Direction, view: Vec2) -> Vec2 {
    match direction {
        halley_config::Direction::Left => Vec2 {
            x: -view.x * 0.1,
            y: 0.0,
        },
        halley_config::Direction::Right => Vec2 {
            x: view.x * 0.1,
            y: 0.0,
        },
        halley_config::Direction::Up => Vec2 {
            x: 0.0,
            y: -view.y * 0.1,
        },
        halley_config::Direction::Down => Vec2 {
            x: 0.0,
            y: view.y * 0.1,
        },
    }
}

pub(crate) fn pan_field<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    direction: halley_config::Direction,
) -> Result<(), String> {
    let output = selected(session)?;
    let camera = session
        .cameras
        .get(&output.name())
        .ok_or("monitor camera unavailable")?;
    let delta = pan_delta(direction, camera.view_size);
    let target = Vec2 {
        x: camera.target_center.x + delta.x,
        y: camera.target_center.y + delta.y,
    };
    if !session.cameras.center_field_on(&output.name(), target) {
        return Err("camera is locked by fullscreen or maximize".into());
    }
    session.request_redraw();
    Ok(())
}

pub(crate) fn transfer_window<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    direction: halley_config::Direction,
) -> Result<(), String> {
    if !crate::session::pointer::transfer_pointer_available(session) {
        return Err("finish the pointer drag or unlock the pointer before transferring".into());
    }
    let source = selected(session)?;
    session.nodes.sync_from_space(&session.wayland.space);
    let id = session
        .nodes
        .focused_on_output(&source.name())
        .ok_or("no selected Field window")?;
    let record = session
        .nodes
        .record(id)
        .cloned()
        .ok_or("selected object is not a window")?;
    if record.output != source.name() || session.clusters.cluster_for_member(id).is_some() {
        return Err("selected window is not in this Field".into());
    }
    let surface = record
        .window
        .wl_surface()
        .ok_or("window surface unavailable")?;
    if session
        .fullscreen
        .is_fullscreen_or_pending(surface.as_ref())
        || session.maximize.contains(surface.as_ref())
    {
        return Err("restore fullscreen or maximized window before transferring".into());
    }
    let target = crate::wayland::focus::adjacent_output(&session.wayland.space, &source, direction)
        .ok_or("no monitor in that direction")?;
    if session.clusters.active_on(&target.name()).is_some() {
        return Err("destination monitor must show the Field".into());
    }
    let center = session
        .cameras
        .get_mut(&target.name())
        .ok_or("destination camera unavailable")?
        .center;
    let geometry = session
        .wayland
        .space
        .output_geometry(&target)
        .ok_or("destination geometry unavailable")?;
    let center = transfer_center(center, (geometry.loc.x, geometry.loc.y));
    session
        .interactions
        .field_arrange
        .invalidate_surface(surface.as_ref());
    super::session_ops::set_collapsed_output(session, id, &target);
    super::session_physics::apply_dynamics_positions(
        session,
        [(id, center)].into_iter().collect(),
        Some(id),
    );
    crate::wayland::focus::select_output(&mut session.wayland, &target);
    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
    if record.collapsed {
        crate::nodes::reveal_collapsed_node(session, id, serial, true);
    } else {
        crate::nodes::focus_or_reveal_node(session, id, serial, true);
    }
    crate::session::pointer::warp_after_transfer(
        session,
        (
            geometry.loc.x as f64 + geometry.size.w as f64 * 0.5,
            geometry.loc.y as f64 + geometry.size.h as f64 * 0.5,
        ),
    );
    session.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transfer_uses_destination_monitor_origin_and_current_pan() {
        let camera = Vec2 {
            x: 300.0,
            y: -150.0,
        };
        assert_eq!(
            transfer_center(camera, (1920, 100)),
            Vec2 {
                x: 2220.0,
                y: -50.0
            }
        );
        assert_eq!(
            transfer_center(camera, (-2560, -1080)),
            Vec2 {
                x: -2260.0,
                y: -1230.0
            }
        );
    }

    #[test]
    fn pan_steps_are_directional_and_relative_to_view() {
        let view = Vec2 {
            x: 1000.0,
            y: 600.0,
        };
        assert_eq!(
            pan_delta(halley_config::Direction::Right, view),
            Vec2 { x: 100.0, y: 0.0 }
        );
        assert_eq!(
            pan_delta(halley_config::Direction::Up, view),
            Vec2 { x: 0.0, y: -60.0 }
        );
    }
}
