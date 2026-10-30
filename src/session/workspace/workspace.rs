//! Session policy for the staging `ext-workspace-v1` global.
//!
//! [`crate::wayland::ext_workspace`] owns protocol objects, snapshots, and
//! transactions. This module owns the two decisions only the session can make:
//! what the current cluster model looks like, and how a reduced transaction is
//! applied through Halley's existing activation path.
//!
//! Mapping, in one place:
//!
//! | Protocol concept | Halley |
//! |---|---|
//! | Workspace group | One currently mapped output |
//! | Workspace | One cluster, including empty clusters |
//! | Name | `ClusterMetadata::name` |
//! | 1-D coordinate | Published slot, 0-based |
//! | Active | `ClusterSystem::active_on(output) == Some(id)` |
//! | No active workspace | The output is in Field mode |
//!
//! There is deliberately no synthetic "Field" workspace: Field mode is the
//! absence of an active cluster, which the protocol already expresses by
//! clearing the `active` state bit on every workspace of that group.

use smithay::utils::SERIAL_COUNTER;

use super::{Session, SessionDriver};
use crate::wayland::ext_workspace::{GroupSnapshot, Snapshot, TransactionPlan, WorkspaceSnapshot};

/// Builds the protocol-visible view of the cluster model.
///
/// Groups exist only for currently mapped outputs, and a cluster is advertised
/// only while its owning output is mapped. Nothing is created or moved here;
/// this is a pure read of `ClusterSystem` plus the output list.
pub(crate) fn snapshot<D: SessionDriver>(session: &Session<D>) -> Snapshot {
    let mut groups = Vec::new();
    for output in session.wayland.space.outputs() {
        let name = output.name();
        let workspaces = session
            .clusters
            .clusters_for_output(&name)
            .map(|(slot, id, metadata)| WorkspaceSnapshot {
                id,
                name: metadata.name.clone(),
                order: slot,
                active: session.clusters.active_on(&name) == Some(id),
            })
            .collect();
        groups.push(GroupSnapshot {
            output: output.clone(),
            workspaces,
        });
    }
    Snapshot { groups }
}

/// Pushes the current snapshot to every manager binding.
///
/// Called at the event-loop boundary before clients are flushed, so every path
/// that mutates clusters - keyboard and pointer activation, IPC, cluster
/// creation and dissolution, startup clusters, core movement between outputs,
/// slot compaction, and output reconfiguration - is reflected without adding a
/// notification call to each of them.
pub(crate) fn sync_ext_workspace<D: SessionDriver>(session: &mut Session<D>) {
    if session.wayland.ext_workspace_state.is_idle() {
        return;
    }
    let snapshot = snapshot(session);
    let display = session.wayland.display_handle.clone();
    session
        .wayland
        .ext_workspace_state
        .sync::<Session<D>>(&display, &snapshot);
}

/// Applies a committed transaction through the existing activation path.
///
/// Returns whether the model changed.
pub(crate) fn apply_transaction<D: SessionDriver>(
    session: &mut Session<D>,
    plan: TransactionPlan,
) -> bool {
    if plan.is_empty() || !switching_allowed(session) {
        return false;
    }
    let now = crate::frame_clock::monotonic_now();
    let mut applied = Vec::new();
    for (output, selection) in &plan.selections {
        // Focus ownership is captured before any state changes, exactly as the
        // keyboard and IPC activation paths do.
        let target = match selection {
            Some(id) => *id,
            None => match session.clusters.active_on(output) {
                Some(id) => id,
                None => continue,
            },
        };
        let owned_focus = crate::session::cluster_owns_focus(session, target);
        let changed = match selection {
            Some(id) => session.clusters.activate_only(output, *id, now),
            None => session.clusters.deactivate_output(output, now),
        };
        if changed {
            applied.push(AppliedSwitch {
                output: output.clone(),
                target,
                owned_focus,
            });
        }
    }
    if applied.is_empty() {
        return false;
    }

    // Every changed output has its camera synchronised, but the keyboard is
    // handed over at most once: to the transaction's final activation when that
    // output really changed, or to the single output of a one-output
    // transaction, matching what the equivalent keyboard shortcut does.
    let focus_output = plan
        .focus
        .as_ref()
        .map(|(output, _)| output.clone())
        .filter(|output| applied.iter().any(|switch| switch.output == *output))
        .or_else(|| (applied.len() == 1).then(|| applied[0].output.clone()));

    for switch in &applied {
        let Some(handle) = session
            .wayland
            .space
            .outputs()
            .find(|candidate| candidate.name() == switch.output)
            .cloned()
        else {
            continue;
        };
        if focus_output.as_deref() == Some(switch.output.as_str()) {
            crate::session::sync_cluster_activation_focus(
                session,
                &handle,
                switch.target,
                switch.owned_focus,
                SERIAL_COUNTER.next_serial(),
            );
        } else {
            crate::session::sync_cluster_camera(session, &switch.output, now);
        }
    }

    session.request_redraw();
    true
}

struct AppliedSwitch {
    output: String,
    target: halley_core::cluster::ClusterId,
    owned_focus: bool,
}

/// External activation is ignored while switching would be unsafe.
///
/// The session lock owns the screen and a compositor grab owns the pointer.
/// Neither is cancelled or bypassed from here: the request is simply dropped,
/// which is allowed because the protocol promises no guarantee that a workspace
/// will actually be activated.
fn switching_allowed<D: SessionDriver>(session: &Session<D>) -> bool {
    !session.session_lock.active()
        && matches!(session.interactions.grab, crate::input::grab::Grab::None)
}
