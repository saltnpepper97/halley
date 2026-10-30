//! Focused-node lookup and pinning controls.

use super::*;

pub(super) fn focused_window_record<D: SessionDriver>(
    session: &Session<D>,
    output: Option<&str>,
) -> Option<crate::nodes::NodeRecord> {
    let belongs_to_output = |record: &&crate::nodes::NodeRecord| {
        output.is_none_or(|output| {
            crate::wayland::window_output_name(&record.window)
                .as_deref()
                .unwrap_or(&record.output)
                == output
        })
    };
    session
        .wayland
        .focused_window
        .as_ref()
        .and_then(|surface| session.nodes.id_for_surface(surface))
        .and_then(|id| session.nodes.record(id))
        .filter(|record| !record.collapsed)
        .filter(belongs_to_output)
        .cloned()
        .or_else(|| {
            let id = match output {
                Some(output) => session.nodes.focused_on_output(output),
                None => session.nodes.focused(),
            }?;
            session
                .nodes
                .record(id)
                .filter(|record| !record.collapsed)
                .filter(belongs_to_output)
                .cloned()
        })
}

pub(super) fn node_belongs_to_output<D: SessionDriver>(
    session: &Session<D>,
    id: halley_core::field::NodeId,
    output: Option<&str>,
) -> bool {
    output.is_none_or(|output| {
        session
            .nodes
            .record(id)
            .is_some_and(|record| record.output == output)
            || session
                .clusters
                .cluster_for_core(id)
                .and_then(|cluster| session.clusters.metadata(cluster))
                .is_some_and(|metadata| metadata.output == output)
    })
}

pub(crate) fn node_user_pinned<D: SessionDriver>(
    session: &Session<D>,
    id: halley_core::field::NodeId,
) -> bool {
    if session.clusters.cluster_for_member(id).is_some() {
        return false;
    }
    session
        .clusters
        .cluster_for_core(id)
        .and_then(|cluster| session.clusters.registry().cluster(cluster))
        .map_or_else(
            || session.nodes.field.node(id).is_some_and(|node| node.pinned),
            |cluster| cluster.pinned,
        )
}

pub(crate) fn set_node_user_pinned<D: SessionDriver>(
    session: &mut Session<D>,
    id: halley_core::field::NodeId,
    pinned: bool,
) -> bool {
    if session.clusters.cluster_for_member(id).is_some() {
        return false;
    }
    if session.clusters.cluster_for_core(id).is_some() {
        session
            .clusters
            .set_core_pinned(&mut session.nodes.field, id, pinned)
    } else {
        session.nodes.field.set_pinned(id, pinned)
    }
}

pub(super) fn toggle_focused_pin<D: SessionDriver>(
    session: &mut Session<D>,
    output: Option<&str>,
) -> bool {
    let live_focus = session
        .wayland
        .focused_window
        .as_ref()
        .and_then(|surface| session.nodes.id_for_surface(surface))
        .filter(|id| node_belongs_to_output(session, *id, output));
    let logical_focus = session
        .nodes
        .focused()
        .filter(|id| node_belongs_to_output(session, *id, output));
    let output_focus = output.and_then(|name| session.nodes.focused_on_output(name));
    let Some(id) = live_focus.or(logical_focus).or(output_focus) else {
        return false;
    };
    if !session.nodes.field.is_visible(id) {
        return false;
    }
    let pinned = !node_user_pinned(session, id);
    if !set_node_user_pinned(session, id, pinned) {
        return false;
    }
    session.nodes.clear_direct_motion(id);
    session.request_redraw();
    true
}
