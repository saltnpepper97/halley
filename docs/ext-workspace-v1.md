# ext-workspace-v1

Halley advertises the staging `ext-workspace-v1` protocol, so taskbars, docks,
and scripts can enumerate Halley's clusters and switch between them. A client
binds `ext_workspace_manager_v1` and receives one workspace group per mapped
output, each containing one workspace per cluster of that output.

Halley's cluster model stays authoritative. The protocol layer holds no second
mutable workspace model: it publishes a read-only snapshot of `ClusterSystem`
at the event-loop boundary and turns committed requests back into the same
activation path the keyboard and IPC already use.

## Mapping

| Protocol concept | Halley |
|---|---|
| Workspace group | One currently mapped output |
| Workspace | One cluster, including empty clusters |
| Name | `ClusterMetadata.name` |
| 1-D coordinate | Published slot, 0-based |
| `active` state | `ClusterSystem::active_on(output) == Some(id)` |
| No active workspace | The output is in Field mode |

There is deliberately no synthetic "Field" workspace. Field mode is the absence
of an active cluster, which the protocol already expresses by clearing the
`active` bit on every workspace of that group.

Coordinates are sent as a single dense dimension: the cluster's published slot
(1-based internally), minus one, as a native-endian `u32`. Clients should use
them only to order the workspaces of a group.

## Supported requests

| Request | Behaviour |
|---|---|
| `ext_workspace_manager_v1.commit` | Applies the queued transaction atomically |
| `ext_workspace_manager_v1.stop` | Answered with `finished`; the binding is dropped |
| `ext_workspace_handle_v1.activate` | Activates that cluster on its output |
| `ext_workspace_handle_v1.deactivate` | Clears the output only if that cluster is the active one |
| `ext_workspace_handle_v1.destroy` | Drops the client's handle; the cluster is untouched |
| `ext_workspace_group_handle_v1.destroy` | Drops the client's group handle; workspace handles keep working |

Advertised capabilities are deliberately narrow:

* workspace capabilities: `activate | deactivate` only
* group capabilities: empty, so `create_workspace` is never advertised

`remove`, `assign`, and `create_workspace` are therefore ignored. They never
mutate Halley, exactly as the protocol requires for unsupported requests.

## Transactions

Requests queue until `commit`. The transaction is then reduced against current
state before anything is applied:

* the last request for an output wins, so activating two workspaces on one
  output leaves only the second active;
* `deactivate` clears an output only when its target is the workspace that would
  be active there, so deactivating an unrelated workspace leaves the active
  selection alone;
* targets are revalidated at commit, so a request made on a handle that was
  since destroyed, or naming a cluster that no longer exists, is discarded;
* only the final state per output is applied, so a client never sees an
  intermediate animation or a transient focus change.

Repeating a request is idempotent. Halley's own keyboard and IPC shortcuts keep
toggling (`activate` closes an already-active workspace); the protocol uses an
explicit idempotent activation so a taskbar that re-sends "activate" cannot
close the workspace it is highlighting.

A transaction spanning several outputs switches all of them, but the keyboard
goes to at most one: the final applicable activation, or the single output of a
one-output transaction.

## Resource lifetimes

Protocol state is tracked per manager *binding*, not per client, so a client may
bind `ext_workspace_manager_v1` more than once and sees independent handles for
the same workspaces. Handles are owned per binding and never shared.

* A client destroying a handle never deletes a cluster.
* A retired handle (after `removed`) stays inert. A later re-advertisement uses
  a fresh object instead of reviving the old one.
* A handle a client destroyed outright is not re-advertised to that binding, so
  a client that prefers to manage its own objects cannot cause a
  create/destroy loop.
* Stopping the manager discards pending work and finishes the binding. Client
  disconnect drops the binding through the manager object's destruction.
* Every update batch ends with `done`; unchanged snapshots send nothing at all.

Event ordering follows the protocol's lifecycle rules: a workspace is a member
of at most one group, `workspace_leave` precedes any removal, and a workspace
handle is removed only while it belongs to no group.

## Outputs

One group is advertised per currently mapped output, and a cluster is advertised
only while its owning output is mapped.

When an output disappears, its group and the handles of its workspaces are
retired; Halley's cluster records are untouched, and no cluster is silently
migrated to another output. Membership is removed before the group is removed.

Output identity, not just the output name, is tracked. A monitor that
disconnects and reconnects under the same name is a new output, so its
re-advertised groups and workspaces always use fresh objects rather than
reviving stale handles.

`wl_output` association is handled for both orders: a `wl_output` bound after the
manager still receives `output_enter`, and binding `wl_output` more than once
associates every object exactly once.

## Policy boundaries

Switching is declined without error while the session is locked or a compositor
grab owns the pointer. The protocol promises no guarantee that a workspace will
actually be activated, so the request is dropped rather than cancelling a grab
or bypassing the lock. Halley reports the reduced transaction to its own layers
and simply does not apply it.

## Limitations

* No persistent workspace identifiers are sent. A cluster's runtime
  `ClusterId` is not evidence of stable identity across compositor restarts, and
  the protocol reserves `id` for workspaces that clients may store preferences
  against.
* `urgent` and `hidden` are never set. Collapsed clusters stay visible in panels.
* Client-driven creation, removal, and reassignment are not supported.
* Floating-window policy, cluster persistence, and output migration are
  unchanged by this protocol.
* Advertised behaviour is driven from the event-loop boundary in both the nested
  (`winit`) and hardware (`tty`) sessions, immediately before clients are
  flushed.

## Tests

`src/wayland/desktop/ext_workspace.rs` carries the pure logic — snapshot types, the
transaction reducer, and capability constants — with unit tests for reduction
and idempotence.

`tests/ext_workspace_protocol.rs` compiles that module directly with `#[path]`
and drives the production dispatch code over a real Wayland socket with a stub
cluster model. It covers initial enumeration, commit atomicity, per-binding and
per-client independence, unsupported requests, external model changes, output
removal and reconnection ordering, late `wl_output` binds, and manager `stop`.
The session-level lock and grab policy is not covered there because it needs a
graphics-backed session; it lives in `src/session/workspace/workspace.rs`.
