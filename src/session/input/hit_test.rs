//! Resolve pointer targets for field and cluster interaction.

use super::*;

pub(super) fn output_at_pointer(
    space: &Space<Window>,
    position: (f64, f64),
) -> Option<(Output, Rectangle<i32, Logical>)> {
    let output = space.output_under(position).next()?.clone();
    let geometry = space.output_geometry(&output)?;
    Some((output, geometry))
}

pub(super) fn work_area_for_output(
    space: &Space<Window>,
    output_name: &str,
) -> Option<Rectangle<i32, Logical>> {
    let output = space
        .outputs()
        .find(|output| output.name() == output_name)?;
    Some(smithay::desktop::layer_map_for_output(output).non_exclusive_zone())
}

pub(super) fn cluster_exclusive_on_output<D: SessionDriver>(
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

pub(super) fn node_at_pointer<D: SessionDriver>(
    session: &Session<D>,
) -> Option<(halley_core::field::NodeId, Output)> {
    let position = session.pointer.position();
    let (output, geometry) = output_at_pointer(&session.wayland.space, position)?;
    if crate::session::pointer::presentation_covers_field_point(
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

pub(super) fn cluster_at_pointer<D: SessionDriver>(
    session: &Session<D>,
) -> Option<(halley_core::cluster::ClusterId, Output)> {
    let position = session.pointer.position();
    let (output, geometry) = output_at_pointer(&session.wayland.space, position)?;
    if crate::session::pointer::presentation_covers_field_point(
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

pub(super) fn cluster_overflow_at_pointer<D: SessionDriver>(
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

pub(super) fn cluster_action_at_pointer<D: SessionDriver>(
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

pub(super) fn cluster_bloom_at_pointer<D: SessionDriver>(
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

pub(super) struct OverflowHover {
    pub(super) output: Output,
    pub(super) member: Option<halley_core::field::NodeId>,
    pub(super) intercepts_desktop: bool,
    pub(super) changed: bool,
}

pub(super) fn update_overflow_hover<D: SessionDriver>(
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

pub(super) fn scroll_cluster_overflow_at_pointer<D, B, E>(
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
