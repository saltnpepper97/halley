use halley_api::{
    ApiError, ClusterDraftAppLaunch, ClusterDraftRequest, ClusterInfo, ClusterLayoutKind,
    ClusterListResponse, ClusterOutputGroup, ClusterRequest, ClusterSummary, ClusterTarget,
    Response,
};
use halley_core::cluster::ClusterId;
use halley_core::cluster_layout::ClusterWorkspaceLayoutKind as CoreClusterLayoutKind;
use halley_core::field::NodeId;
use std::time::Instant;

use crate::compositor::clusters::state::ClusterNameRecord;
use crate::compositor::root::Halley;

use super::focus_output_if_needed;
use super::node::node_info;
use super::{resolve_output_context, sorted_outputs, validate_output};

pub(super) fn handle_cluster_request(st: &mut Halley, request: ClusterRequest) -> Response {
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
        ClusterRequest::Open { target, output } => {
            match open_cluster(st, &target, output.as_deref()) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            }
        }
        ClusterRequest::OpenFinalizeDraft { draft, output } => {
            match open_finalize_draft(st, draft, output.as_deref()) {
                Ok(()) => Response::Ok,
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
                    Err(ApiError::Unsupported(format!(
                        "no active cluster workspace on output {}",
                        monitor
                    )))
                }
            }) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            }
        }
        ClusterRequest::Slot { slot, output } => {
            match activate_cluster_slot(st, slot, output.as_deref()) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            }
        }
    }
}

fn open_cluster(
    st: &mut Halley,
    target: &ClusterTarget,
    output: Option<&str>,
) -> Result<(), ApiError> {
    let cid = resolve_cluster_target(st, Some(target), output)?;
    let monitor = output
        .map(|name| validate_output(st, name).map(str::to_string))
        .transpose()?
        .or_else(|| preferred_monitor_for_cluster(st, cid, None))
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    let now = Instant::now();
    focus_output_if_needed(st, monitor.as_str(), now);
    let core = st
        .model
        .field
        .cluster(cid)
        .and_then(|cluster| cluster.core)
        .or_else(|| st.collapse_cluster(cid));
    let Some(core) = core else {
        return Err(ApiError::Unsupported(format!(
            "cluster {} cannot be opened because it has no core node",
            cid.as_u64()
        )));
    };
    if crate::compositor::clusters::system::enter_cluster_workspace_by_core(
        st,
        core,
        monitor.as_str(),
        now,
    ) {
        Ok(())
    } else {
        Err(ApiError::Unsupported(format!(
            "failed to open cluster {} on output {}",
            cid.as_u64(),
            monitor
        )))
    }
}

fn open_finalize_draft(
    st: &mut Halley,
    draft: ClusterDraftRequest,
    output: Option<&str>,
) -> Result<(), ApiError> {
    let monitor = resolve_output_context(st, output)?;
    let running_node_ids = draft
        .running_node_ids
        .into_iter()
        .map(NodeId::new)
        .collect::<Vec<_>>();
    let now = Instant::now();
    focus_output_if_needed(st, monitor.as_str(), now);
    if crate::compositor::clusters::system::open_lift_cluster_finalize_draft(
        st,
        monitor.as_str(),
        draft.name_hint,
        draft.app_ids,
        draft_app_launches(draft.app_launches),
        running_node_ids,
        now,
    ) {
        Ok(())
    } else {
        Err(ApiError::Unsupported(format!(
            "failed to open cluster finalize draft on output {monitor}"
        )))
    }
}

fn draft_app_launches(
    app_launches: Vec<ClusterDraftAppLaunch>,
) -> Vec<crate::compositor::clusters::state::ClusterFinalizeAppLaunch> {
    app_launches
        .into_iter()
        .map(
            |launch| crate::compositor::clusters::state::ClusterFinalizeAppLaunch {
                app_id: launch.app_id,
                command: launch.command,
            },
        )
        .collect()
}

fn activate_cluster_slot(st: &mut Halley, slot: u8, output: Option<&str>) -> Result<(), ApiError> {
    if !(1..=10).contains(&slot) {
        return Err(ApiError::InvalidRequest(format!(
            "cluster slot must be between 1 and 10, got {slot}"
        )));
    }

    let monitor = resolve_output_context(st, output)?;
    let exists = crate::compositor::clusters::system::cluster_slot_cluster_for_monitor(
        &*st,
        monitor.as_str(),
        slot,
    )
    .is_some();
    if !exists {
        return Err(ApiError::NotFound(format!(
            "no cluster in slot {slot} on output {monitor}"
        )));
    }

    let now = Instant::now();
    focus_output_if_needed(st, monitor.as_str(), now);
    if st.activate_cluster_slot_on_current_monitor(slot, now) {
        Ok(())
    } else {
        Err(ApiError::Unsupported(format!(
            "failed to activate cluster slot {slot} on output {monitor}"
        )))
    }
}

pub(super) fn list_clusters(
    st: &Halley,
    output: Option<&str>,
) -> Result<ClusterListResponse, ApiError> {
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

pub(super) fn inspect_cluster(
    st: &Halley,
    target: Option<&ClusterTarget>,
    output: Option<&str>,
) -> Result<ClusterInfo, ApiError> {
    let cid = resolve_cluster_target(st, target, output)?;
    cluster_info(st, cid)
}

fn resolve_cluster_target(
    st: &Halley,
    target: Option<&ClusterTarget>,
    output: Option<&str>,
) -> Result<ClusterId, ApiError> {
    match target {
        Some(ClusterTarget::Id(raw)) => {
            let cid = ClusterId::new(*raw);
            st.model
                .field
                .cluster(cid)
                .map(|_| cid)
                .ok_or_else(|| ApiError::NotFound(format!("cluster {} not found", cid.as_u64())))
        }
        Some(ClusterTarget::Current) | None => {
            let monitor = resolve_output_context(st, output)?;
            st.active_cluster_workspace_for_monitor(monitor.as_str())
                .ok_or_else(|| {
                    ApiError::NotFound(format!("no active cluster workspace on output {}", monitor))
                })
        }
    }
}

fn cluster_summary(st: &Halley, cid: ClusterId) -> Option<ClusterSummary> {
    let cluster = st.model.field.cluster(cid)?;
    Some(ClusterSummary {
        id: cid.as_u64(),
        slot: cluster_slot(st, cid),
        name: cluster_display_name(st, cid),
        output: cluster_output(st, cid),
        layout: ipc_cluster_layout_kind(st.runtime.tuning.cluster_layout_kind()),
        member_count: cluster.members().len(),
        active: cluster.is_active(),
        focused: cluster_has_focus(st, cid),
    })
}

fn cluster_info(st: &Halley, cid: ClusterId) -> Result<ClusterInfo, ApiError> {
    let cluster = st
        .model
        .field
        .cluster(cid)
        .ok_or_else(|| ApiError::NotFound(format!("cluster {} not found", cid.as_u64())))?;
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
        slot: cluster_slot(st, cid),
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

fn cluster_slot(st: &Halley, cid: ClusterId) -> Option<u8> {
    let output = cluster_output(st, cid)?;
    crate::compositor::clusters::system::cluster_slot_order_for_monitor(st, output.as_str())
        .iter()
        .position(|existing| *existing == cid)
        .and_then(|index| u8::try_from(index + 1).ok())
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
