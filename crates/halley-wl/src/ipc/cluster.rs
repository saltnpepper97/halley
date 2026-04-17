use halley_core::cluster::ClusterId;
use halley_core::cluster_layout::ClusterWorkspaceLayoutKind as CoreClusterLayoutKind;
use halley_ipc::{
    ClusterInfo, ClusterLayoutKind, ClusterListResponse, ClusterOutputGroup, ClusterRequest,
    ClusterSummary, ClusterTarget, IpcError, Response,
};
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

pub(super) fn list_clusters(
    st: &Halley,
    output: Option<&str>,
) -> Result<ClusterListResponse, IpcError> {
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
