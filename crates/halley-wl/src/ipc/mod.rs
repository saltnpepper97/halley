use std::cmp::Ordering;
use std::time::Instant;

use halley_core::cluster::ClusterId;
use halley_core::cluster_layout::ClusterWorkspaceLayoutKind as CoreClusterLayoutKind;
use halley_core::field::{NodeId, NodeKind as FieldNodeKind, NodeState as FieldNodeState};
use halley_ipc::{
    BearingsRequest, BearingsStatusResponse, CaptureRequest, CaptureStatusResponse, ClusterInfo,
    ClusterLayoutKind, ClusterListResponse, ClusterOutputGroup, ClusterRequest, ClusterSummary,
    ClusterTarget, CompositorRequest, IpcError, MonitorFocusDirection, MonitorFocusTarget,
    MonitorRequest, NodeInfo, NodeKind, NodeListResponse, NodeMoveDirection, NodeOutputGroup,
    NodeProtocolFamily, NodeRelationInfo, NodeRequest, NodeRole, NodeSelector, NodeState, Request,
    Response, StackRequest, TileRequest, TrailEntryInfo, TrailListResponse, TrailRequest,
    TrailTarget,
};
use smithay::desktop::PopupManager;
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::{XdgPopupSurfaceData, XdgToplevelSurfaceData};

use crate::compositor::actions::window::promote_node_level;
use crate::compositor::clusters::state::ClusterNameRecord;
use crate::compositor::root::Halley;
use crate::compositor::surface_ops::{current_surface_size_for_node, request_close_node_toplevel};

pub(crate) fn handle_request(st: &mut Halley, request: Request) -> Response {
    match request {
        Request::Capture(request) => handle_capture_request(st, request),
        Request::Node(request) => handle_node_request(st, request),
        Request::Trail(request) => handle_trail_request(st, request),
        Request::Monitor(request) => handle_monitor_request(st, request),
        Request::Bearings(request) => handle_bearings_request(st, request),
        Request::Stack(request) => handle_stack_request(st, request),
        Request::Tile(request) => handle_tile_request(st, request),
        Request::Cluster(request) => handle_cluster_request(st, request),
        Request::Compositor(CompositorRequest::Outputs) => Response::Error(IpcError::Unsupported(
            "outputs are handled by the ipc listener".into(),
        )),
        Request::Compositor(CompositorRequest::ApertureStatus) => {
            Response::ApertureStatus(crate::aperture::aperture_status(st))
        }
        Request::Compositor(CompositorRequest::Quit)
        | Request::Compositor(CompositorRequest::Reload)
        | Request::Compositor(CompositorRequest::Dpms { .. }) => Response::Error(
            IpcError::Unsupported("backend request not handled here".into()),
        ),
    }
}

fn handle_capture_request(st: &mut Halley, request: CaptureRequest) -> Response {
    match request {
        CaptureRequest::Start { mode, output } => {
            if st.start_screenshot_session(mode, output.as_deref(), Instant::now()) {
                Response::CaptureStatus(capture_status_response(st))
            } else {
                Response::Error(IpcError::Unsupported(
                    "screenshot session is already active".into(),
                ))
            }
        }
        CaptureRequest::Status => Response::CaptureStatus(capture_status_response(st)),
    }
}

fn capture_status_response(st: &Halley) -> CaptureStatusResponse {
    let last = st.input.interaction_state.last_screenshot_result.as_ref();
    CaptureStatusResponse {
        active: st.screenshot_session_active()
            || st
                .input
                .interaction_state
                .pending_screenshot_capture
                .is_some()
            || st
                .input
                .interaction_state
                .inflight_screenshot_capture
                .is_some(),
        session_serial: st
            .input
            .interaction_state
            .screenshot_session
            .as_ref()
            .map(|_| {
                st.input
                    .interaction_state
                    .screenshot_next_serial
                    .saturating_sub(1)
            }),
        last_finished_serial: last.map(|result| result.serial),
        saved_path: last
            .and_then(|result| result.saved_path.as_ref())
            .map(|path| path.display().to_string()),
        error: last.and_then(|result| result.error.clone()),
    }
}

fn handle_node_request(st: &mut Halley, request: NodeRequest) -> Response {
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

fn handle_trail_request(st: &mut Halley, request: TrailRequest) -> Response {
    match request {
        TrailRequest::Prev { output } => {
            match resolve_output_context(st, output.as_deref()).and_then(|monitor| {
                focus_output_if_needed(st, monitor.as_str(), Instant::now());
                if st.navigate_window_trail(halley_ipc::TrailDirection::Prev, Instant::now()) {
                    Ok(())
                } else {
                    Err(IpcError::NotFound(format!(
                        "no previous trail entry on output {}",
                        monitor
                    )))
                }
            }) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            }
        }
        TrailRequest::Next { output } => {
            match resolve_output_context(st, output.as_deref()).and_then(|monitor| {
                focus_output_if_needed(st, monitor.as_str(), Instant::now());
                if st.navigate_window_trail(halley_ipc::TrailDirection::Next, Instant::now()) {
                    Ok(())
                } else {
                    Err(IpcError::NotFound(format!(
                        "no next trail entry on output {}",
                        monitor
                    )))
                }
            }) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            }
        }
        TrailRequest::List { output } => match list_trail(st, output.as_deref()) {
            Ok(trail) => Response::TrailList(trail),
            Err(err) => Response::Error(err),
        },
        TrailRequest::Goto { target, output } => {
            match goto_trail_target(st, target, output.as_deref(), Instant::now()) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            }
        }
    }
}

fn handle_monitor_request(st: &mut Halley, request: MonitorRequest) -> Response {
    match request {
        MonitorRequest::Focus(target) => match resolve_monitor_focus_target(st, &target) {
            Ok(monitor) => {
                st.focus_monitor_view(monitor.as_str(), Instant::now());
                Response::Ok
            }
            Err(err) => Response::Error(err),
        },
    }
}

fn handle_bearings_request(st: &mut Halley, request: BearingsRequest) -> Response {
    match request {
        BearingsRequest::Show => {
            st.ui.render_state.set_bearings_visible(true);
            Response::Ok
        }
        BearingsRequest::Hide => {
            st.ui.render_state.set_bearings_visible(false);
            Response::Ok
        }
        BearingsRequest::Toggle => {
            st.ui.render_state.toggle_bearings_visible();
            Response::Ok
        }
        BearingsRequest::Status => Response::BearingsStatus(BearingsStatusResponse {
            visible: st.ui.render_state.bearings_visible(),
        }),
    }
}

fn handle_stack_request(st: &mut Halley, request: StackRequest) -> Response {
    match request {
        StackRequest::Cycle { direction, output } => {
            match resolve_output_context(st, output.as_deref()).and_then(|monitor| {
                let now = Instant::now();
                focus_output_if_needed(st, monitor.as_str(), now);
                let direction = match direction {
                    halley_ipc::StackCycleDirection::Forward => {
                        halley_core::cluster_layout::ClusterCycleDirection::Next
                    }
                    halley_ipc::StackCycleDirection::Backward => {
                        halley_core::cluster_layout::ClusterCycleDirection::Prev
                    }
                };
                if st.cycle_active_stack_for_monitor(monitor.as_str(), direction, now) {
                    Ok(())
                } else {
                    Err(IpcError::Unsupported(format!(
                        "stack layout is not active on output {}",
                        monitor
                    )))
                }
            }) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            }
        }
    }
}

fn handle_tile_request(st: &mut Halley, request: TileRequest) -> Response {
    let (direction, output, swap) = match request {
        TileRequest::Focus { direction, output } => (direction, output, false),
        TileRequest::Swap { direction, output } => (direction, output, true),
    };
    match resolve_output_context(st, output.as_deref()).and_then(|monitor| {
        let now = Instant::now();
        focus_output_if_needed(st, monitor.as_str(), now);
        let direction = match direction {
            NodeMoveDirection::Left => halley_config::DirectionalAction::Left,
            NodeMoveDirection::Right => halley_config::DirectionalAction::Right,
            NodeMoveDirection::Up => halley_config::DirectionalAction::Up,
            NodeMoveDirection::Down => halley_config::DirectionalAction::Down,
        };
        let ok = if swap {
            st.tile_swap_active_cluster_member_for_monitor(monitor.as_str(), direction, now)
        } else {
            st.tile_focus_active_cluster_member_for_monitor(monitor.as_str(), direction, now)
        };
        if ok {
            Ok(())
        } else {
            Err(IpcError::Unsupported(format!(
                "tiled layout is not active or no tile exists {} on output {}",
                if swap { "to swap" } else { "to focus" },
                monitor
            )))
        }
    }) {
        Ok(()) => Response::Ok,
        Err(err) => Response::Error(err),
    }
}

fn handle_cluster_request(st: &mut Halley, request: ClusterRequest) -> Response {
    match request {
        ClusterRequest::List { output } => match list_clusters(st, output.as_deref()) {
            Ok(list) => Response::ClusterList(list),
            Err(err) => Response::Error(err),
        },
        ClusterRequest::Inspect { target, output } => {
            match inspect_cluster(st, target.as_ref(), output.as_deref()) {
                Ok(cluster) => Response::ClusterInfo(cluster),
                Err(err) => Response::Error(err),
            }
        }
        ClusterRequest::LayoutCycle { output } => {
            match resolve_output_context(st, output.as_deref()).and_then(|monitor| {
                let now = Instant::now();
                focus_output_if_needed(st, monitor.as_str(), now);
                if st.cycle_active_cluster_layout_for_monitor(monitor.as_str(), now) {
                    Ok(())
                } else {
                    Err(IpcError::Unsupported(format!(
                        "no active cluster workspace on output {}",
                        monitor
                    )))
                }
            }) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            }
        }
    }
}

fn list_clusters(st: &Halley, output: Option<&str>) -> Result<ClusterListResponse, IpcError> {
    let outputs: Vec<String> = match output {
        Some(name) => vec![validate_output(st, name)?.to_string()],
        None => sorted_outputs(st),
    };
    let cluster_ids = st.model.field.cluster_ids();
    let groups = outputs
        .into_iter()
        .map(|output| {
            let mut clusters = cluster_ids
                .iter()
                .copied()
                .filter(|&cid| cluster_output(st, cid).as_deref() == Some(output.as_str()))
                .filter_map(|cid| cluster_summary(st, cid))
                .collect::<Vec<_>>();
            sort_cluster_summaries(&mut clusters);
            ClusterOutputGroup { output, clusters }
        })
        .collect();
    Ok(ClusterListResponse { outputs: groups })
}

fn inspect_cluster(
    st: &Halley,
    target: Option<&ClusterTarget>,
    output: Option<&str>,
) -> Result<ClusterInfo, IpcError> {
    let cid = resolve_cluster_target(st, target, output)?;
    cluster_info(st, cid)
}

fn resolve_cluster_target(
    st: &Halley,
    target: Option<&ClusterTarget>,
    output: Option<&str>,
) -> Result<ClusterId, IpcError> {
    match target {
        Some(ClusterTarget::Id(raw)) => {
            let cid = ClusterId::new(*raw);
            st.model
                .field
                .cluster(cid)
                .map(|_| cid)
                .ok_or_else(|| IpcError::NotFound(format!("cluster {} not found", cid.as_u64())))
        }
        Some(ClusterTarget::Current) | None => {
            let monitor = resolve_output_context(st, output)?;
            st.active_cluster_workspace_for_monitor(monitor.as_str())
                .ok_or_else(|| {
                    IpcError::NotFound(format!("no active cluster workspace on output {}", monitor))
                })
        }
    }
}

fn cluster_summary(st: &Halley, cid: ClusterId) -> Option<ClusterSummary> {
    let cluster = st.model.field.cluster(cid)?;
    Some(ClusterSummary {
        id: cid.as_u64(),
        name: cluster_display_name(st, cid),
        output: cluster_output(st, cid),
        layout: ipc_cluster_layout_kind(st.runtime.tuning.cluster_layout_kind()),
        member_count: cluster.members().len(),
        active: cluster.is_active(),
        focused: cluster_has_focus(st, cid),
    })
}

fn cluster_info(st: &Halley, cid: ClusterId) -> Result<ClusterInfo, IpcError> {
    let cluster = st
        .model
        .field
        .cluster(cid)
        .ok_or_else(|| IpcError::NotFound(format!("cluster {} not found", cid.as_u64())))?;
    let focused_member_index = st
        .model
        .focus_state
        .primary_interaction_focus
        .and_then(|id| cluster.members().iter().position(|member| *member == id));
    let focused_member_id = focused_member_index.map(|index| cluster.members()[index].as_u64());
    let members = cluster
        .members()
        .iter()
        .copied()
        .filter(|&id| st.model.field.node(id).is_some())
        .map(|id| node_info(st, id))
        .collect();
    Ok(ClusterInfo {
        id: cid.as_u64(),
        name: cluster_display_name(st, cid),
        output: cluster_output(st, cid),
        layout: ipc_cluster_layout_kind(st.runtime.tuning.cluster_layout_kind()),
        member_count: cluster.members().len(),
        active: cluster.is_active(),
        focused: cluster_has_focus(st, cid),
        focused_member_index,
        focused_member_id,
        members,
    })
}

fn cluster_output(st: &Halley, cid: ClusterId) -> Option<String> {
    preferred_monitor_for_cluster(st, cid, None)
}

fn preferred_monitor_for_cluster(
    st: &Halley,
    cid: ClusterId,
    preferred: Option<&str>,
) -> Option<String> {
    preferred
        .map(str::to_string)
        .or_else(|| {
            st.model
                .cluster_state
                .active_cluster_workspaces
                .iter()
                .find_map(|(monitor, active_cid)| (*active_cid == cid).then(|| monitor.clone()))
        })
        .or_else(|| {
            st.model
                .cluster_state
                .cluster_bloom_open
                .iter()
                .find_map(|(monitor, open_cid)| (*open_cid == cid).then(|| monitor.clone()))
        })
        .or_else(|| {
            st.model
                .field
                .cluster(cid)
                .and_then(|cluster| cluster.core)
                .and_then(|core_id| st.model.monitor_state.node_monitor.get(&core_id).cloned())
        })
        .or_else(|| {
            st.model.field.cluster(cid).and_then(|cluster| {
                cluster
                    .members()
                    .iter()
                    .find_map(|member| st.model.monitor_state.node_monitor.get(member).cloned())
            })
        })
        .or_else(|| Some(st.model.monitor_state.current_monitor.clone()))
}

fn cluster_display_name(st: &Halley, cid: ClusterId) -> Option<String> {
    match st.model.cluster_state.cluster_names.get(&cid)? {
        ClusterNameRecord::Generic { slot } => Some(format!("Cluster {slot}")),
        ClusterNameRecord::Custom { name } => Some(name.clone()),
    }
}

fn cluster_has_focus(st: &Halley, cid: ClusterId) -> bool {
    let Some(id) = st.model.focus_state.primary_interaction_focus else {
        return false;
    };
    st.model.field.cluster_id_for_member_public(id) == Some(cid)
        || st.model.field.cluster_id_for_core_public(id) == Some(cid)
}

fn ipc_cluster_layout_kind(kind: CoreClusterLayoutKind) -> ClusterLayoutKind {
    match kind {
        CoreClusterLayoutKind::Tiling => ClusterLayoutKind::Tiling,
        CoreClusterLayoutKind::Stacking => ClusterLayoutKind::Stacking,
    }
}

fn sort_cluster_summaries(clusters: &mut [ClusterSummary]) {
    clusters.sort_by(|a, b| {
        b.focused
            .cmp(&a.focused)
            .then(b.active.cmp(&a.active))
            .then(a.id.cmp(&b.id))
    });
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

fn list_trail(st: &mut Halley, output: Option<&str>) -> Result<TrailListResponse, IpcError> {
    let output = resolve_output_context(st, output)?;
    let snapshot = {
        let trail = st.model.focus_state.focus_trail.get(output.as_str());
        let entries = trail.map(|trail| trail.entries()).unwrap_or_default();
        let cursor_index = trail.and_then(|trail| trail.cursor_index());
        (entries, cursor_index)
    };

    let mut entries = Vec::new();
    for (index, id) in snapshot.0.into_iter().enumerate() {
        if !node_matches_output(st, id, output.as_str()) || !node_is_queryable_surface(st, id) {
            if let Some(trail) = st.model.focus_state.focus_trail.get_mut(output.as_str()) {
                trail.forget_node(id);
            }
            continue;
        }
        entries.push(TrailEntryInfo {
            index,
            cursor: snapshot.1 == Some(index),
            node: node_info(st, id),
        });
    }

    Ok(TrailListResponse {
        output,
        cursor_index: snapshot.1,
        entries,
    })
}

fn goto_trail_target(
    st: &mut Halley,
    target: TrailTarget,
    output: Option<&str>,
    now: Instant,
) -> Result<(), IpcError> {
    let output = resolve_output_context(st, output)?;
    focus_output_if_needed(st, output.as_str(), now);
    let node_id = match target {
        TrailTarget::Index(index) => st
            .model
            .focus_state
            .focus_trail
            .get_mut(output.as_str())
            .and_then(|trail| trail.seek_to_index(index))
            .ok_or_else(|| {
                IpcError::NotFound(format!(
                    "trail entry {} not found on output {}",
                    index, output
                ))
            })?,
        TrailTarget::Selector(selector) => {
            let node_id = resolve_node_selector(st, Some(&selector), Some(output.as_str()))?;
            let found = st
                .model
                .focus_state
                .focus_trail
                .get_mut(output.as_str())
                .is_some_and(|trail| trail.seek_to_node(node_id));
            if !found {
                return Err(IpcError::NotFound(format!(
                    "node {} is not present in the trail for output {}",
                    node_id.as_u64(),
                    output
                )));
            }
            node_id
        }
    };
    focus_node(st, node_id, now)
}

fn focus_node(st: &mut Halley, id: NodeId, now: Instant) -> Result<(), IpcError> {
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
    direction: halley_ipc::NodeMoveDirection,
) -> Result<(), IpcError> {
    let node = st
        .model
        .field
        .node(id)
        .cloned()
        .ok_or_else(|| IpcError::NotFound(format!("node {} not found", id.as_u64())))?;
    let step = 80.0;
    let (dx, dy) = match direction {
        halley_ipc::NodeMoveDirection::Left => (-step, 0.0),
        halley_ipc::NodeMoveDirection::Right => (step, 0.0),
        halley_ipc::NodeMoveDirection::Up => (0.0, step),
        halley_ipc::NodeMoveDirection::Down => (0.0, -step),
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

fn resolve_node_selector(
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

fn resolve_output_context(st: &Halley, output: Option<&str>) -> Result<String, IpcError> {
    match output {
        Some(name) => Ok(validate_output(st, name)?.to_string()),
        None => Ok(st.focused_monitor().to_string()),
    }
}

fn validate_output<'a>(st: &'a Halley, output: &'a str) -> Result<&'a str, IpcError> {
    st.model
        .monitor_state
        .monitors
        .contains_key(output)
        .then_some(output)
        .ok_or_else(|| IpcError::NotFound(format!("output {output} not found")))
}

fn focus_output_if_needed(st: &mut Halley, output: &str, now: Instant) {
    if st.focused_monitor() != output {
        st.focus_monitor_view(output, now);
    }
}

fn node_info(st: &Halley, id: NodeId) -> NodeInfo {
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

fn node_is_queryable_surface(st: &Halley, id: NodeId) -> bool {
    st.model
        .field
        .node(id)
        .is_some_and(|node| st.model.field.is_visible(id) && node.kind == FieldNodeKind::Surface)
}

fn node_matches_output(st: &Halley, id: NodeId, output: &str) -> bool {
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

fn sorted_outputs(st: &Halley) -> Vec<String> {
    let mut outputs: Vec<_> = st.model.monitor_state.monitors.keys().cloned().collect();
    outputs.sort_by(|a, b| {
        let am = st.model.monitor_state.monitors.get(a).expect("monitor");
        let bm = st.model.monitor_state.monitors.get(b).expect("monitor");
        am.offset_x
            .cmp(&bm.offset_x)
            .then(am.offset_y.cmp(&bm.offset_y))
            .then(a.cmp(b))
    });
    outputs
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(needle.to_ascii_lowercase().as_str())
}

fn resolve_monitor_focus_target(
    st: &Halley,
    target: &MonitorFocusTarget,
) -> Result<String, IpcError> {
    match target {
        MonitorFocusTarget::Output(output) => Ok(validate_output(st, output)?.to_string()),
        MonitorFocusTarget::Direction(direction) => {
            adjacent_monitor(st, *direction).ok_or_else(|| {
                IpcError::NotFound(format!(
                    "no {} output adjacent to {}",
                    monitor_direction_label(*direction),
                    st.focused_monitor()
                ))
            })
        }
    }
}

fn adjacent_monitor(st: &Halley, direction: MonitorFocusDirection) -> Option<String> {
    let current_name = st.focused_monitor();
    let current = st.model.monitor_state.monitors.get(current_name)?;
    let current_center = (
        current.offset_x as f32 + current.width as f32 * 0.5,
        current.offset_y as f32 + current.height as f32 * 0.5,
    );

    let mut candidates: Vec<(String, f32, f32)> = st
        .model
        .monitor_state
        .monitors
        .iter()
        .filter_map(|(name, monitor)| {
            if name == current_name {
                return None;
            }
            let center = (
                monitor.offset_x as f32 + monitor.width as f32 * 0.5,
                monitor.offset_y as f32 + monitor.height as f32 * 0.5,
            );
            let dx = center.0 - current_center.0;
            let dy = center.1 - current_center.1;
            let (primary, secondary, keep) = match direction {
                MonitorFocusDirection::Left => (-dx, dy.abs(), dx < 0.0),
                MonitorFocusDirection::Right => (dx, dy.abs(), dx > 0.0),
                MonitorFocusDirection::Up => (-dy, dx.abs(), dy < 0.0),
                MonitorFocusDirection::Down => (dy, dx.abs(), dy > 0.0),
            };
            keep.then_some((name.clone(), primary, secondary))
        })
        .collect();
    candidates.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(Ordering::Equal)
            .then(a.2.partial_cmp(&b.2).unwrap_or(Ordering::Equal))
            .then(a.0.cmp(&b.0))
    });
    candidates.into_iter().next().map(|(name, _, _)| name)
}

fn monitor_direction_label(direction: MonitorFocusDirection) -> &'static str {
    match direction {
        MonitorFocusDirection::Left => "left",
        MonitorFocusDirection::Right => "right",
        MonitorFocusDirection::Up => "up",
        MonitorFocusDirection::Down => "down",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_core::field::Vec2;

    fn cluster_test_state() -> (Halley, ClusterId, NodeId, NodeId) {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let first = state.model.field.spawn_surface(
            "firefox",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let second = state.model.field.spawn_surface(
            "foot",
            Vec2 { x: 200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.assign_node_to_current_monitor(first);
        state.assign_node_to_current_monitor(second);
        let cid = state
            .model
            .field
            .create_cluster(vec![first, second])
            .expect("cluster");
        state.model.cluster_state.cluster_names.insert(
            cid,
            ClusterNameRecord::Custom {
                name: "web".to_string(),
            },
        );
        let monitor = state.model.monitor_state.current_monitor.clone();
        state
            .model
            .cluster_state
            .active_cluster_workspaces
            .insert(monitor, cid);
        state
            .model
            .field
            .cluster_mut(cid)
            .expect("cluster mut")
            .enter_active();
        state.model.focus_state.primary_interaction_focus = Some(second);
        (state, cid, first, second)
    }

    #[test]
    fn default_selector_falls_back_to_latest_on_output() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let first = state.model.field.spawn_surface(
            "first",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let second = state.model.field.spawn_surface(
            "second",
            Vec2 { x: 200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.assign_node_to_current_monitor(first);
        state.assign_node_to_current_monitor(second);
        state.model.focus_state.primary_interaction_focus = None;

        assert_eq!(
            resolve_node_selector(&state, None, None).unwrap().as_u64(),
            2
        );
    }

    #[test]
    fn title_selector_reports_ambiguity() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let first = state.model.field.spawn_surface(
            "Kitty",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let second = state.model.field.spawn_surface(
            "Kitty scratch",
            Vec2 { x: 200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.assign_node_to_current_monitor(first);
        state.assign_node_to_current_monitor(second);

        let result =
            resolve_node_selector(&state, Some(&NodeSelector::Title("kitty".into())), None);
        assert!(matches!(result, Err(IpcError::Ambiguous(_))));
    }

    #[test]
    fn monitor_focus_direction_picks_adjacent_output() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 1920,
                height: 1080,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 1920,
                offset_y: 0,
                width: 1920,
                height: 1080,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.focus_monitor_view("left", Instant::now());

        assert_eq!(
            adjacent_monitor(&state, MonitorFocusDirection::Right).as_deref(),
            Some("right")
        );
    }

    #[test]
    fn cluster_list_reports_named_active_cluster() {
        let (state, cid, _, _) = cluster_test_state();

        let list = list_clusters(&state, None).expect("cluster list");
        let cluster = list
            .outputs
            .iter()
            .flat_map(|group| group.clusters.iter())
            .find(|cluster| cluster.id == cid.as_u64())
            .expect("cluster summary");

        assert_eq!(cluster.name.as_deref(), Some("web"));
        assert!(cluster.active);
        assert!(cluster.focused);
        assert_eq!(cluster.member_count, 2);
    }

    #[test]
    fn cluster_inspect_defaults_to_current_output_active_cluster() {
        let (state, cid, first, second) = cluster_test_state();

        let info = inspect_cluster(&state, None, None).expect("cluster inspect");

        assert_eq!(info.id, cid.as_u64());
        assert_eq!(info.name.as_deref(), Some("web"));
        assert!(info.active);
        assert!(info.focused);
        assert_eq!(info.focused_member_index, Some(1));
        assert_eq!(info.focused_member_id, Some(second.as_u64()));
        assert_eq!(info.members.len(), 2);
        assert_eq!(info.members[0].id, first.as_u64());
        assert_eq!(info.members[1].id, second.as_u64());
    }
}
