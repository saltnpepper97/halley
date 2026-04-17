use std::cmp::Ordering;
use std::time::Instant;

use halley_core::field::{NodeId, NodeKind as FieldNodeKind, NodeState as FieldNodeState};
use halley_ipc::{
    IpcError, NodeInfo, NodeKind, NodeListResponse, NodeMoveDirection, NodeOutputGroup,
    NodeProtocolFamily, NodeRelationInfo, NodeRequest, NodeRole, NodeSelector, NodeState, Response,
};
use smithay::desktop::PopupManager;
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::{XdgPopupSurfaceData, XdgToplevelSurfaceData};

use crate::compositor::actions::window::promote_node_level;
use crate::compositor::root::Halley;
use crate::compositor::surface_ops::{current_surface_size_for_node, request_close_node_toplevel};

use super::{focus_output_if_needed, sorted_outputs, validate_output};

pub(super) fn handle_node_request(st: &mut Halley, request: NodeRequest) -> Response {
    match request {
        NodeRequest::List { output } => match list_nodes(st, output.as_deref()) {
            Ok(outputs) => Response::NodeList(outputs),
            Err(err) => Response::Error(err),
        },
        NodeRequest::Info { selector, output } => {
            match resolve_node_selector(st, selector.as_ref(), output.as_deref()) {
                Ok(id) => Response::NodeInfo(node_info(st, id)),
                Err(err) => Response::Error(err),
            }
        }
        NodeRequest::Focus { selector, output } => {
            match resolve_node_selector(st, selector.as_ref(), output.as_deref()) {
                Ok(id) => match focus_node(st, id, Instant::now()) {
                    Ok(()) => Response::Ok,
                    Err(err) => Response::Error(err),
                },
                Err(err) => Response::Error(err),
            }
        }
        NodeRequest::Move {
            direction,
            selector,
            output,
        } => match resolve_node_selector(st, selector.as_ref(), output.as_deref()) {
            Ok(id) => match move_node_direction(st, id, direction) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            },
            Err(err) => Response::Error(err),
        },
        NodeRequest::Close { selector, output } => {
            match resolve_node_selector(st, selector.as_ref(), output.as_deref()) {
                Ok(id) => {
                    if request_close_node_toplevel(st, id) {
                        Response::Ok
                    } else {
                        Response::Error(IpcError::NotFound(format!(
                            "node {} does not have a closeable toplevel surface",
                            id.as_u64()
                        )))
                    }
                }
                Err(err) => Response::Error(err),
            }
        }
    }
}

fn list_nodes(st: &Halley, output: Option<&str>) -> Result<NodeListResponse, IpcError> {
    let outputs: Vec<String> = match output {
        Some(name) => vec![validate_output(st, name)?.to_string()],
        None => sorted_outputs(st),
    };
    let groups = outputs
        .into_iter()
        .map(|output| NodeOutputGroup {
            nodes: surface_nodes_on_output(st, output.as_str())
                .into_iter()
                .map(|id| node_info(st, id))
                .collect(),
            output,
        })
        .collect();
    Ok(NodeListResponse { outputs: groups })
}

pub(super) fn focus_node(st: &mut Halley, id: NodeId, now: Instant) -> Result<(), IpcError> {
    if !node_is_queryable_surface(st, id) {
        return Err(IpcError::NotFound(format!(
            "node {} is not available",
            id.as_u64()
        )));
    }
    let node = st
        .model
        .field
        .node(id)
        .cloned()
        .ok_or_else(|| IpcError::NotFound(format!("node {} is not available", id.as_u64())))?;
    let output = st
        .model
        .monitor_state
        .node_monitor
        .get(&id)
        .cloned()
        .ok_or_else(|| IpcError::NotFound(format!("node {} has no output", id.as_u64())))?;
    focus_output_if_needed(st, output.as_str(), now);
    match node.state {
        FieldNodeState::Active | FieldNodeState::Drifting => {
            st.set_interaction_focus(Some(id), 30_000, now);
            let _ = st.animate_viewport_center_to(node.pos, now);
            Ok(())
        }
        FieldNodeState::Node => {
            if promote_node_level(st, id, now) {
                Ok(())
            } else {
                Err(IpcError::Internal(format!(
                    "failed to focus node {}",
                    id.as_u64()
                )))
            }
        }
        FieldNodeState::Core => Err(IpcError::Unsupported(format!(
            "node {} is a core node and cannot be focused via node commands",
            id.as_u64()
        ))),
    }
}

fn move_node_direction(
    st: &mut Halley,
    id: NodeId,
    direction: NodeMoveDirection,
) -> Result<(), IpcError> {
    let node = st
        .model
        .field
        .node(id)
        .cloned()
        .ok_or_else(|| IpcError::NotFound(format!("node {} not found", id.as_u64())))?;
    let step = 80.0;
    let (dx, dy) = match direction {
        NodeMoveDirection::Left => (-step, 0.0),
        NodeMoveDirection::Right => (step, 0.0),
        NodeMoveDirection::Up => (0.0, step),
        NodeMoveDirection::Down => (0.0, -step),
    };
    let to = halley_core::field::Vec2 {
        x: node.pos.x + dx,
        y: node.pos.y + dy,
    };
    let _ = st.model.field.set_pinned(id, false);
    crate::compositor::carry::system::begin_carry_state_tracking(st, id);
    let moved = st.carry_surface_non_overlap(id, to, false);
    if moved {
        crate::compositor::carry::system::update_carry_state_preview(st, id, Instant::now());
        st.set_interaction_focus(Some(id), 30_000, Instant::now());
    }
    crate::compositor::carry::system::end_carry_state_tracking(st, id);
    if moved {
        Ok(())
    } else {
        Err(IpcError::Internal(format!(
            "failed to move node {}",
            id.as_u64()
        )))
    }
}

pub(super) fn resolve_node_selector(
    st: &Halley,
    selector: Option<&NodeSelector>,
    output: Option<&str>,
) -> Result<NodeId, IpcError> {
    let requested_output = output.map(|name| validate_output(st, name)).transpose()?;
    match selector {
        None => resolve_default_node(st, requested_output.as_deref()),
        Some(NodeSelector::Focused) => resolve_focused_node(st, requested_output.as_deref()),
        Some(NodeSelector::Latest) => resolve_latest_node(st, requested_output.as_deref()),
        Some(NodeSelector::Id(id)) => resolve_node_match(
            st,
            requested_output.as_deref(),
            &format!("id:{id}"),
            |node_id| node_id.as_u64() == *id,
        ),
        Some(NodeSelector::Title(text)) => resolve_node_match(
            st,
            requested_output.as_deref(),
            &format!("title:{text}"),
            |node_id| {
                st.model
                    .field
                    .node(node_id)
                    .is_some_and(|node| contains_case_insensitive(node.label.as_str(), text))
            },
        ),
        Some(NodeSelector::App(text)) => resolve_node_match(
            st,
            requested_output.as_deref(),
            &format!("app:{text}"),
            |node_id| {
                st.model
                    .node_app_ids
                    .get(&node_id)
                    .is_some_and(|app_id| contains_case_insensitive(app_id.as_str(), text))
            },
        ),
    }
}

fn resolve_default_node(st: &Halley, output: Option<&str>) -> Result<NodeId, IpcError> {
    let output = output.unwrap_or_else(|| st.focused_monitor());
    resolve_focused_node(st, Some(output))
        .or_else(|_| {
            st.last_focused_surface_node_for_monitor(output)
                .filter(|&id| node_is_queryable_surface(st, id))
                .ok_or_else(|| {
                    IpcError::NotFound(format!("no last-focused node on output {output}"))
                })
        })
        .or_else(|_| resolve_latest_node(st, Some(output)))
}

fn resolve_focused_node(st: &Halley, output: Option<&str>) -> Result<NodeId, IpcError> {
    let output = output.unwrap_or_else(|| st.focused_monitor());
    st.model
        .focus_state
        .primary_interaction_focus
        .filter(|&id| node_matches_output(st, id, output) && node_is_queryable_surface(st, id))
        .ok_or_else(|| IpcError::NotFound(format!("no focused node on output {output}")))
}

fn resolve_latest_node(st: &Halley, output: Option<&str>) -> Result<NodeId, IpcError> {
    let output = output.unwrap_or_else(|| st.focused_monitor());
    surface_nodes_on_output(st, output)
        .into_iter()
        .max_by_key(|id| id.as_u64())
        .ok_or_else(|| IpcError::NotFound(format!("no nodes on output {output}")))
}

fn resolve_node_match<F>(
    st: &Halley,
    output: Option<&str>,
    label: &str,
    predicate: F,
) -> Result<NodeId, IpcError>
where
    F: Fn(NodeId) -> bool,
{
    let candidates: Vec<NodeId> = surface_nodes(st, output)
        .into_iter()
        .filter(|&id| predicate(id))
        .collect();
    match candidates.as_slice() {
        [id] => Ok(*id),
        [] => Err(IpcError::NotFound(format!(
            "no node matched selector {label}"
        ))),
        many => Err(IpcError::Ambiguous(format!(
            "selector {label} matched multiple nodes: {}",
            many.iter()
                .map(|id| format!(
                    "{} ({})",
                    id.as_u64(),
                    st.model
                        .field
                        .node(*id)
                        .map(|n| n.label.as_str())
                        .unwrap_or("unknown")
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

pub(super) fn node_info(st: &Halley, id: NodeId) -> NodeInfo {
    let node = st
        .model
        .field
        .node(id)
        .expect("node info requires live node");
    let size = current_surface_size_for_node(st, id).unwrap_or(node.intrinsic_size);
    let output = st.model.monitor_state.node_monitor.get(&id).cloned();
    let metadata = node_surface_metadata(st, id);
    let latest = output.as_deref().and_then(|output| {
        surface_nodes_on_output(st, output)
            .into_iter()
            .max_by_key(|candidate| candidate.as_u64())
    }) == Some(id);
    NodeInfo {
        id: id.as_u64(),
        title: node.label.clone(),
        app_id: st.model.node_app_ids.get(&id).cloned(),
        output,
        kind: match node.kind {
            FieldNodeKind::Surface => NodeKind::Surface,
            FieldNodeKind::Core => NodeKind::Core,
        },
        state: match node.state {
            FieldNodeState::Active => NodeState::Active,
            FieldNodeState::Drifting => NodeState::Drifting,
            FieldNodeState::Node => NodeState::Node,
            FieldNodeState::Core => NodeState::Core,
        },
        visible: st.model.field.is_visible(id),
        focused: st.model.focus_state.primary_interaction_focus == Some(id),
        latest,
        role: metadata.role,
        protocol_family: metadata.protocol_family,
        modal: metadata.modal,
        parent: metadata.parent,
        transient_for: metadata.transient_for,
        child_popup_count: metadata.child_popup_count,
        pos_x: node.pos.x,
        pos_y: node.pos.y,
        width: size.x,
        height: size.y,
    }
}

#[derive(Debug, Clone)]
struct NodeSurfaceMetadata {
    role: NodeRole,
    protocol_family: NodeProtocolFamily,
    modal: bool,
    parent: Option<NodeRelationInfo>,
    transient_for: Option<NodeRelationInfo>,
    child_popup_count: usize,
}

fn node_surface_metadata(st: &Halley, id: NodeId) -> NodeSurfaceMetadata {
    let Some(surface) = node_root_surface(st, id) else {
        return NodeSurfaceMetadata {
            role: NodeRole::Unknown,
            protocol_family: NodeProtocolFamily::Unknown,
            modal: false,
            parent: None,
            transient_for: None,
            child_popup_count: 0,
        };
    };

    let child_popup_count = PopupManager::popups_for_surface(&surface).count();
    let xdg_toplevel = with_states(&surface, |states| {
        states.data_map.get::<XdgToplevelSurfaceData>().map(|data| {
            let guard = data.lock().expect("xdg toplevel data");
            (guard.parent.clone(), guard.modal)
        })
    });
    if let Some((parent_surface, modal)) = xdg_toplevel {
        let relation = parent_surface
            .as_ref()
            .map(|surface| relation_for_surface(st, surface));
        let role = if modal || relation.is_some() {
            NodeRole::Dialog
        } else {
            NodeRole::NormalToplevel
        };
        return NodeSurfaceMetadata {
            role,
            protocol_family: NodeProtocolFamily::XdgToplevel,
            modal,
            parent: relation.clone(),
            transient_for: relation,
            child_popup_count,
        };
    }

    let xdg_popup = with_states(&surface, |states| {
        states
            .data_map
            .get::<XdgPopupSurfaceData>()
            .map(|data| data.lock().expect("xdg popup data").parent.clone())
    });
    if let Some(parent_surface) = xdg_popup {
        let relation = parent_surface
            .as_ref()
            .map(|surface| relation_for_surface(st, surface));
        return NodeSurfaceMetadata {
            role: NodeRole::Popup,
            protocol_family: NodeProtocolFamily::XdgPopup,
            modal: false,
            parent: relation.clone(),
            transient_for: relation,
            child_popup_count,
        };
    }

    NodeSurfaceMetadata {
        role: NodeRole::Unknown,
        protocol_family: NodeProtocolFamily::Unknown,
        modal: false,
        parent: None,
        transient_for: None,
        child_popup_count,
    }
}

fn node_root_surface(st: &Halley, id: NodeId) -> Option<WlSurface> {
    st.platform
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .find_map(|surface| {
            let surface_id = surface.wl_surface().id();
            (st.model.surface_to_node.get(&surface_id).copied() == Some(id))
                .then(|| surface.wl_surface().clone())
        })
        .or_else(|| {
            st.platform
                .xdg_shell_state
                .popup_surfaces()
                .iter()
                .find_map(|surface| {
                    let surface_id = surface.wl_surface().id();
                    (st.model.surface_to_node.get(&surface_id).copied() == Some(id))
                        .then(|| surface.wl_surface().clone())
                })
        })
}

fn relation_for_surface(st: &Halley, surface: &WlSurface) -> NodeRelationInfo {
    NodeRelationInfo {
        node_id: st
            .model
            .surface_to_node
            .get(&surface.id())
            .map(|id| id.as_u64()),
    }
}

pub(super) fn node_is_queryable_surface(st: &Halley, id: NodeId) -> bool {
    st.model
        .field
        .node(id)
        .is_some_and(|node| st.model.field.is_visible(id) && node.kind == FieldNodeKind::Surface)
}

pub(super) fn node_matches_output(st: &Halley, id: NodeId, output: &str) -> bool {
    st.model
        .monitor_state
        .node_monitor
        .get(&id)
        .map(|name| name.as_str())
        == Some(output)
}

fn surface_nodes(st: &Halley, output: Option<&str>) -> Vec<NodeId> {
    let mut nodes: Vec<NodeId> = st
        .model
        .field
        .node_ids_all()
        .into_iter()
        .filter_map(|id| {
            let node = st.model.field.node(id)?;
            (node.kind == FieldNodeKind::Surface
                && st.model.field.is_visible(id)
                && output
                    .map(|output| node_matches_output(st, id, output))
                    .unwrap_or(true))
            .then_some(id)
        })
        .collect();
    nodes.sort_by(|a, b| {
        let output_cmp = match (
            st.model.monitor_state.node_monitor.get(a),
            st.model.monitor_state.node_monitor.get(b),
        ) {
            (Some(a_output), Some(b_output)) => a_output.cmp(b_output),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        output_cmp.then(a.as_u64().cmp(&b.as_u64()))
    });
    nodes
}

fn surface_nodes_on_output(st: &Halley, output: &str) -> Vec<NodeId> {
    surface_nodes(st, Some(output))
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(needle.to_ascii_lowercase().as_str())
}
