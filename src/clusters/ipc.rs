use halley_core::cluster::ClusterId;
use halley_core::cluster::layout::ClusterWorkspaceLayoutKind;
use halley_core::field::NodeId;

fn output_context<D: crate::session::SessionDriver>(
    session: &crate::session::Session<D>,
    requested: Option<&str>,
) -> Result<String, String> {
    if let Some(output) = requested {
        if session
            .driver
            .output_info()
            .iter()
            .any(|candidate| candidate.name == output)
        {
            return Ok(output.to_string());
        }
        return Err(format!("output {output:?} was not found"));
    }
    Ok(crate::wayland::focus::selected_output(&session.wayland)
        .unwrap_or_else(|| session.driver.primary_output())
        .name())
}

fn summary<D: crate::session::SessionDriver>(
    session: &crate::session::Session<D>,
    id: ClusterId,
) -> Option<halley_ipc::ClusterSummary> {
    let cluster = session.clusters.registry.cluster(id)?;
    let metadata = session.clusters.metadata(id)?;
    Some(halley_ipc::ClusterSummary {
        id: id.as_u64(),
        slot: session.clusters.slot_of(&metadata.output, id),
        name: metadata.name.clone(),
        output: metadata.output.clone(),
        layout: match metadata.layout {
            ClusterWorkspaceLayoutKind::Tiling => halley_ipc::ClusterLayoutKind::Tiling,
            ClusterWorkspaceLayoutKind::Stacking => halley_ipc::ClusterLayoutKind::Stacking,
        },
        member_count: cluster.members().len(),
        active: session.clusters.active_on(&metadata.output) == Some(id),
        focused: session.nodes.focused().is_some_and(|focused| {
            cluster.contains(focused) || cluster.core_node() == Some(focused)
        }),
    })
}

pub fn handle_request<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    request: halley_ipc::ClusterRequest,
) -> halley_ipc::Response {
    match request {
        halley_ipc::ClusterRequest::List { output } => {
            let outputs = match output {
                Some(output) => match output_context(session, Some(&output)) {
                    Ok(output) => vec![output],
                    Err(message) => return halley_ipc::Response::Error(message),
                },
                None => session
                    .driver
                    .output_info()
                    .into_iter()
                    .map(|output| output.name)
                    .collect(),
            };
            halley_ipc::Response::ClusterList(halley_ipc::ClusterListResponse {
                outputs: outputs
                    .into_iter()
                    .map(|output| {
                        let clusters = session
                            .clusters
                            .clusters_for_output(&output)
                            .filter_map(|(_, id, _)| summary(session, id))
                            .collect();
                        halley_ipc::ClusterOutputGroup { output, clusters }
                    })
                    .collect(),
            })
        }
        halley_ipc::ClusterRequest::Inspect { target, output } => {
            let id = match target {
                halley_ipc::ClusterTarget::Id(raw) => ClusterId::new(raw),
                halley_ipc::ClusterTarget::Current => {
                    let output = match output_context(session, output.as_deref()) {
                        Ok(output) => output,
                        Err(message) => return halley_ipc::Response::Error(message),
                    };
                    let Some(id) = session.clusters.active_on(&output) else {
                        return halley_ipc::Response::Error(format!(
                            "no active cluster on output {output}"
                        ));
                    };
                    id
                }
            };
            let Some(cluster) = session.clusters.registry.cluster(id) else {
                return halley_ipc::Response::Error(format!(
                    "cluster {} was not found",
                    id.as_u64()
                ));
            };
            let Some(summary) = summary(session, id) else {
                return halley_ipc::Response::Error("cluster metadata is incomplete".into());
            };
            let members = cluster
                .members()
                .iter()
                .filter_map(|id| crate::nodes::ipc::node_info(session, *id))
                .collect();
            halley_ipc::Response::ClusterInfo(halley_ipc::ClusterInfo {
                summary,
                core_node_id: cluster.core_node().map(NodeId::as_u64),
                members,
            })
        }
        halley_ipc::ClusterRequest::LayoutCycle { output } => {
            let output = match output_context(session, output.as_deref()) {
                Ok(output) => output,
                Err(message) => return halley_ipc::Response::Error(message),
            };
            let work_area = session
                .wayland
                .space
                .outputs()
                .find(|candidate| candidate.name() == output)
                .map(smithay::desktop::layer_map_for_output)
                .map(|map| map.non_exclusive_zone());
            let now = crate::frame_clock::monotonic_now();
            if work_area.is_some_and(|work_area| {
                session
                    .clusters
                    .cycle_active_layout(&output, work_area, now)
            }) {
                if let Some(id) = session.clusters.active_on(&output) {
                    crate::session::show_cluster_indicator(session, id, now);
                }
                session.request_redraw();
                halley_ipc::Response::Ack
            } else {
                halley_ipc::Response::Error(format!("no active cluster on output {output}"))
            }
        }
        halley_ipc::ClusterRequest::Slot { slot, output } => {
            if !(1..=10).contains(&slot) {
                return halley_ipc::Response::Error(format!(
                    "cluster slot must be between 1 and 10, got {slot}"
                ));
            }
            let output = match output_context(session, output.as_deref()) {
                Ok(output) => output,
                Err(message) => return halley_ipc::Response::Error(message),
            };
            let target = session
                .clusters
                .clusters_for_output(&output)
                .find_map(|(candidate_slot, id, _)| (candidate_slot == slot).then_some(id));
            let owned_focus =
                target.is_some_and(|id| crate::session::cluster_owns_focus(session, id));
            if session
                .clusters
                .activate_slot(&output, slot, crate::frame_clock::monotonic_now())
            {
                let output_handle = session
                    .wayland
                    .space
                    .outputs()
                    .find(|candidate| candidate.name() == output)
                    .cloned();
                if let Some((id, output_handle)) = target.zip(output_handle) {
                    crate::session::sync_cluster_activation_focus(
                        session,
                        &output_handle,
                        id,
                        owned_focus,
                        smithay::utils::SERIAL_COUNTER.next_serial(),
                    );
                }
                session.request_redraw();
                halley_ipc::Response::Ack
            } else {
                halley_ipc::Response::Error(format!(
                    "no cluster exists in slot {slot} on output {output}"
                ))
            }
        }
        halley_ipc::ClusterRequest::Open { target, output } => {
            let id = match target {
                halley_ipc::ClusterTarget::Id(raw) => ClusterId::new(raw),
                halley_ipc::ClusterTarget::Current => {
                    let output = match output_context(session, output.as_deref()) {
                        Ok(output) => output,
                        Err(message) => return halley_ipc::Response::Error(message),
                    };
                    let Some(id) = session.clusters.active_on(&output) else {
                        return halley_ipc::Response::Error(format!(
                            "no active cluster on output {output}"
                        ));
                    };
                    return if session.clusters.active_on(&output) == Some(id) {
                        halley_ipc::Response::Ack
                    } else {
                        halley_ipc::Response::Error("cluster disappeared".into())
                    };
                }
            };
            let Some(metadata) = session.clusters.metadata(id) else {
                return halley_ipc::Response::Error(format!(
                    "cluster {} was not found",
                    id.as_u64()
                ));
            };
            let owned_output = metadata.output.clone();
            if let Some(requested) = output.as_deref()
                && requested != owned_output
            {
                return halley_ipc::Response::Error(format!(
                    "cluster {} belongs to output {owned_output}, not {requested}",
                    id.as_u64()
                ));
            }
            if session.clusters.active_on(&owned_output) == Some(id) {
                return halley_ipc::Response::Ack;
            }
            let owned_focus = crate::session::cluster_owns_focus(session, id);
            let output_handle = session
                .wayland
                .space
                .outputs()
                .find(|candidate| candidate.name() == owned_output)
                .cloned();
            // `activate_only` is the idempotent form; the already-active case
            // returned above, so `Open` never toggled a workspace closed and
            // this preserves the existing behaviour exactly.
            if session.clusters.activate_only(
                &owned_output,
                id,
                crate::frame_clock::monotonic_now(),
            ) {
                if let Some(output_handle) = output_handle {
                    crate::session::sync_cluster_activation_focus(
                        session,
                        &output_handle,
                        id,
                        owned_focus,
                        smithay::utils::SERIAL_COUNTER.next_serial(),
                    );
                }
                session.request_redraw();
                halley_ipc::Response::Ack
            } else {
                halley_ipc::Response::Error(format!("failed to open cluster {}", id.as_u64()))
            }
        }
        halley_ipc::ClusterRequest::OpenFinalizeDraft { draft, output } => {
            let output = match output_context(session, output.as_deref()) {
                Ok(output) => output,
                Err(message) => {
                    return halley_ipc::Response::ApiError(halley_ipc::ServerError::new(
                        halley_ipc::ServerErrorKind::NotFound,
                        message,
                    ));
                }
            };
            let running_nodes = draft
                .running_node_ids
                .into_iter()
                .map(NodeId::new)
                .collect();
            match session.clusters.begin_draft(
                session.nodes.field(),
                output,
                draft.name_hint,
                running_nodes,
                draft.app_launches,
            ) {
                Ok(id) => {
                    session
                        .cursor
                        .set_override(crate::cursor::OverrideSource::Modal, None);
                    session.request_redraw();
                    crate::ipc::publish_cluster_draft(
                        session,
                        id,
                        halley_ipc::ClusterDraftState::Started,
                        None,
                    );
                    crate::ipc::publish_cluster_draft(
                        session,
                        id,
                        halley_ipc::ClusterDraftState::AwaitingName,
                        None,
                    );
                    halley_ipc::Response::ClusterDraftStarted { id }
                }
                Err(message) => halley_ipc::Response::ApiError(halley_ipc::ServerError::new(
                    halley_ipc::ServerErrorKind::InvalidRequest,
                    message,
                )),
            }
        }
    }
}
