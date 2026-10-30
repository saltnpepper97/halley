use halley_core::field::Vec2;
use halley_core::world::PortalDir;
use smithay::utils::{IsAlive, SERIAL_COUNTER};

use super::{Session, SessionDriver};

fn portal_direction(direction: halley_config::Direction) -> PortalDir {
    match direction {
        halley_config::Direction::Left => PortalDir::W,
        halley_config::Direction::Right => PortalDir::E,
        halley_config::Direction::Up => PortalDir::N,
        halley_config::Direction::Down => PortalDir::S,
    }
}

fn output_local_center(world: Vec2, output_origin: (i32, i32)) -> Vec2 {
    Vec2 {
        x: world.x - output_origin.0 as f32,
        y: world.y - output_origin.1 as f32,
    }
}

fn output_geometry_center(
    geometry: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
) -> (f64, f64) {
    (
        f64::from(geometry.loc.x) + f64::from(geometry.size.w) * 0.5,
        f64::from(geometry.loc.y) + f64::from(geometry.size.h) * 0.5,
    )
}

/// Moves the compositor pointer to the center of a named output. Presentation
/// navigation uses this after activation so a cross-output jump never leaves
/// the pointer behind on a different monitor.
pub(crate) fn center_pointer_on_output<D: SessionDriver>(
    session: &mut Session<D>,
    output_name: &str,
) -> bool {
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)
        .cloned()
    else {
        return false;
    };
    let Some(geometry) = session.wayland.space.output_geometry(&output) else {
        return false;
    };
    super::pointer::release_for_compositor_warp(session);
    session
        .pointer
        .set_position(output_geometry_center(geometry));
    session.cursor_policy.pointer_activity();
    super::pointer::update_client_state(session, session.start_time.elapsed().as_millis() as u32);
    session.request_output_redraw(&output);
    true
}

/// Select the owning cluster (or Field), bring a stack/overflow member into
/// view, and then use normal restore/focus. Explicit search retrieval centers
/// the target; taskbar activation retains minimal reveal behavior.
pub(crate) fn activate_node<D: SessionDriver>(
    session: &mut Session<D>,
    id: halley_core::field::NodeId,
    center: bool,
) -> bool {
    let Some(record) = session
        .nodes
        .record(id)
        .filter(|record| record.attached && record.window.alive())
        .cloned()
    else {
        return false;
    };
    let output_name =
        crate::wayland::window_output_name(&record.window).unwrap_or_else(|| record.output.clone());
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|o| o.name() == output_name)
        .cloned()
    else {
        return false;
    };
    crate::wayland::focus::select_output(&mut session.wayland, &output);
    let now = crate::frame_clock::monotonic_now();
    if let Some(cluster) = session.clusters.cluster_for_member(id) {
        session.clusters.activate_only(&output_name, cluster, now);
        let work_area = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
        if !session.clusters.is_member_floating(id) {
            if session.clusters.member_layout(id)
                == Some(halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Stacking)
            {
                let members = session.clusters.member_ids(cluster);
                for _ in 0..members.len() {
                    let front = session
                        .clusters
                        .member_ids(cluster)
                        .into_iter()
                        .find(|member| !session.clusters.is_member_floating(*member));
                    if front == Some(id) {
                        break;
                    }
                    if !matches!(
                        session.clusters.cycle_stack(
                            &output_name,
                            halley_config::FocusCycleDirection::Forward,
                            work_area,
                            now,
                        ),
                        crate::clusters::StackCycleOutcome::Cycled(_)
                    ) {
                        break;
                    }
                }
            } else {
                session
                    .clusters
                    .promote_overflow_member(&output_name, id, work_area, now);
            }
        }
        super::sync_cluster_camera(session, &output_name, now);
        super::reconcile_cluster_surfaces(session, &output_name);
    } else {
        session.clusters.deactivate_output(&output_name, now);
        super::sync_cluster_camera(session, &output_name, now);
    }
    if center {
        crate::nodes::focus_and_center_node(session, id, SERIAL_COUNTER.next_serial())
    } else {
        crate::nodes::focus_or_reveal_node(session, id, SERIAL_COUNTER.next_serial(), true)
    }
}

fn directional_field_node_is_eligible(
    output_name: &str,
    field_visible: bool,
    cluster_member: bool,
    record: Option<(&str, bool)>,
    core_output: Option<&str>,
) -> bool {
    field_visible
        && !cluster_member
        && (record.is_some_and(|(output, attached)| attached && output == output_name)
            || core_output == Some(output_name))
}

fn field_node_is_eligible_on_output<D: SessionDriver>(
    session: &Session<D>,
    id: halley_core::field::NodeId,
    output_name: &str,
) -> bool {
    let record = session
        .nodes
        .record(id)
        .map(|record| (record.output.as_str(), record.attached));
    let core_output = session
        .clusters
        .cluster_for_core(id)
        .and_then(|cluster| session.clusters.metadata(cluster))
        .map(|metadata| metadata.output.as_str());
    directional_field_node_is_eligible(
        output_name,
        session.nodes.field.is_visible(id),
        session.clusters.is_member(id),
        record,
        core_output,
    )
}

fn focus_directional_field_target<D: SessionDriver>(
    session: &mut Session<D>,
    id: halley_core::field::NodeId,
    output_name: &str,
    pan: bool,
) -> bool {
    if session.clusters.cluster_for_core(id).is_some() {
        return crate::nodes::reveal_cluster_core(session, id, SERIAL_COUNTER.next_serial(), pan);
    }
    let collapsed = session
        .nodes
        .record(id)
        .is_some_and(|record| record.collapsed);
    if !collapsed {
        return crate::nodes::focus_or_reveal_node(session, id, SERIAL_COUNTER.next_serial(), pan);
    }
    debug_assert_eq!(
        session
            .nodes
            .record(id)
            .map(|record| record.output.as_str()),
        Some(output_name)
    );
    crate::nodes::reveal_collapsed_node(session, id, SERIAL_COUNTER.next_serial(), pan)
}

/// Selects a directional neighbour in the output's Field. Expanded windows,
/// collapsed nodes, and collapsed cluster cores participate using their visible
/// footprints. Cluster members remain excluded because workspace navigation
/// owns them while open and their Field geometry is only storage while closed.
/// An active Field arrangement on that output still moves focus, but does not
/// pan the camera.
pub(super) fn focus_directional_field<D: SessionDriver>(
    session: &mut Session<D>,
    output_name: &str,
    direction: halley_config::Direction,
) -> bool {
    // Directional Field focus must not punch through another presentation's
    // camera ownership (fullscreen, maximize, or a cluster workspace).
    if session.cameras.get_mut(output_name).is_none() {
        return false;
    }
    let current = session
        .nodes
        .focused()
        .filter(|id| field_node_is_eligible_on_output(session, *id, output_name))
        .or_else(|| {
            session
                .nodes
                .focused_on_output(output_name)
                .filter(|id| field_node_is_eligible_on_output(session, *id, output_name))
        });
    let Some(current) = current else {
        return false;
    };
    let Some(current_rect) = halley_core::focus::node_field_rect(&session.nodes.field, current)
    else {
        return false;
    };
    let direction = portal_direction(direction);
    let target = session
        .nodes
        .field
        .nodes()
        .keys()
        .copied()
        .filter(|id| *id != current && field_node_is_eligible_on_output(session, *id, output_name))
        .filter_map(|id| {
            let rect = halley_core::focus::node_field_rect(&session.nodes.field, id)?;
            let score =
                halley_core::focus::directional_candidate_score(current_rect, rect, direction)?;
            Some((score, id.as_u64(), id))
        })
        .min_by(|a, b| {
            a.0.0
                .total_cmp(&b.0.0)
                .then_with(|| a.0.1.total_cmp(&b.0.1))
                .then_with(|| a.0.2.total_cmp(&b.0.2))
                .then_with(|| a.0.3.total_cmp(&b.0.3))
                .then_with(|| a.1.cmp(&b.1))
        })
        .map(|(_, _, id)| id);

    let pan = !session.interactions.field_arrange.active_on(output_name);
    target.is_some_and(|id| focus_directional_field_target(session, id, output_name, pan))
}

/// Focuses the output's most recent Field window and pans that output's camera
/// to its stored center. It never changes window geometry or restores a
/// collapsed node, and camera ownership makes this a no-op in cluster,
/// fullscreen, and maximized presentations.
pub(super) fn center_last_focused<D: SessionDriver>(
    session: &mut Session<D>,
    output_name: &str,
) -> bool {
    let Some(id) = session.nodes.focused_on_output(output_name) else {
        return false;
    };
    if session.clusters.is_member(id) {
        return false;
    }
    let Some(record) = session.nodes.record(id).cloned() else {
        return false;
    };
    let Some(node) = session.nodes.field.node(id) else {
        return false;
    };
    if !record.attached || record.output != output_name {
        return false;
    }
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
    let target = output_local_center(node.pos, (output_geometry.loc.x, output_geometry.loc.y));
    let Some(camera) = session.cameras.get_mut(output_name) else {
        return false;
    };
    camera.pan_vel = Vec2 { x: 0.0, y: 0.0 };
    camera.target_center = target;

    if record.collapsed {
        crate::window::clear_focus(&mut session.wayland);
        session
            .nodes
            .focus(Some(id), session.start_time.elapsed().as_millis() as u64);
        super::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
    } else {
        super::focus_window(session, &record.window, SERIAL_COUNTER.next_serial());
    }
    session.request_redraw();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directional_field_eligibility_includes_collapsed_nodes_and_cluster_cores() {
        assert!(directional_field_node_is_eligible(
            "DP-2",
            true,
            false,
            Some(("DP-2", true)),
            None,
        ));
        assert!(directional_field_node_is_eligible(
            "DP-2",
            true,
            false,
            None,
            Some("DP-2"),
        ));
        assert!(!directional_field_node_is_eligible(
            "DP-2",
            true,
            false,
            Some(("DP-1", true)),
            None,
        ));
        assert!(!directional_field_node_is_eligible(
            "DP-2",
            true,
            true,
            Some(("DP-2", true)),
            None,
        ));
        assert!(!directional_field_node_is_eligible(
            "DP-2",
            false,
            false,
            None,
            Some("DP-2"),
        ));
    }

    #[test]
    fn maps_config_directions_to_field_directions() {
        assert_eq!(
            portal_direction(halley_config::Direction::Left),
            PortalDir::W
        );
        assert_eq!(
            portal_direction(halley_config::Direction::Right),
            PortalDir::E
        );
        assert_eq!(portal_direction(halley_config::Direction::Up), PortalDir::N);
        assert_eq!(
            portal_direction(halley_config::Direction::Down),
            PortalDir::S
        );
    }

    #[test]
    fn output_pointer_center_uses_global_layout_coordinates() {
        let secondary = smithay::utils::Rectangle::new((2560, -120).into(), (1920, 1200).into());

        assert_eq!(output_geometry_center(secondary), (3520.0, 480.0));
    }

    #[test]
    fn centering_converts_global_field_position_to_output_local_camera_position() {
        assert_eq!(
            output_local_center(
                Vec2 {
                    x: 2080.0,
                    y: 500.0
                },
                (1280, 0)
            ),
            Vec2 { x: 800.0, y: 500.0 }
        );
    }
}
