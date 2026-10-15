# Halley compositor API

`halley-api` is Halley's supported integration boundary for launchers, panels,
automation, testing tools, and desktop components. It is the layer external
programs should build against. `halley-ipc` is the private wire codec used by
the SDK and compositor; its postcard representation is not a public contract.

## Add the SDK

From a sibling project beside the active Halley checkout, use a path dependency:

```toml
[dependencies]
halley-api = { path = "../halley/halley-api" }
```

An application normally opens one `Client` and retains it. The client performs
an API-version handshake when it connects and keeps its command socket open:

```rust
use halley_api::{Client, NodeSelector};

fn main() -> halley_api::Result<()> {
    let client = Client::connect()?;
    println!("Halley {}", client.server_info().compositor_version);
    client.focus_node(Some(NodeSelector::Latest), None)
}
```

`Client::connect()` discovers the compositor socket beneath
`$XDG_RUNTIME_DIR/halley`. `Client::connect_to()` supports tests and custom
session wiring. `Client::connect_with()` additionally accepts read and write
timeouts.

`Client` is safe to share between threads. Commands on one client are
serialized over its persistent connection; a subscription owns a separate
connection so a blocking event reader never blocks commands.

## Supported operations

The version-1 semantic API includes:

| Area | Operations |
| --- | --- |
| Server | handshake, version and capability discovery, quit request |
| Outputs | list output modes, logical placement, and display scale; set DPMS state |
| Nodes | list, inspect, focus, move, close, collapse, restore, and toggle |
| Clusters | list, inspect, open, activate a slot, cycle layout, finalize a draft |
| Bearings | query visibility; show, hide, or toggle |
| Capture | interactive menu/selection; immediate output, node-ID, or rectangle screenshots |
| Configuration | discover the selected path and request a live reload |
| Events | output, node, node-geometry, cluster, draft, and config changes |

Selectors and IDs are semantic types such as `NodeId`, `NodeSelector`,
`ClusterId`, and `ClusterTarget`; callers do not construct wire requests.
Public state and event values implement Serde serialization and deserialization
for application persistence, logging, and JSON output.

## Screenshots and automation

`Client::capture_target(CaptureTarget)` requires the `direct-capture-v1`
capability. It returns `CaptureOutcome::Saved(path)` only after the PNG has
been written, or `Cancelled` after interactive cancellation. Explicit targets
capture immediately without confirmation or keyboard/pointer grabs. They show
the normal saved-image notification with a preview and Copy/Open actions:

```rust
use halley_api::{CaptureRect, CaptureTarget, Client, NodeId};

fn capture(client: &Client, node_id: NodeId) -> halley_api::Result<()> {
    client.capture_target(CaptureTarget::Screen { output: Some("DP-1".into()) })?;
    client.capture_target(CaptureTarget::Window { node_id })?;
    client.capture_target(CaptureTarget::Region {
        output: Some("DP-1".into()),
        rect: Some(CaptureRect { x: 100, y: 50, width: 800, height: 600 }),
    })?;
    Ok(())
}
```

Obtain window IDs from `Client::nodes` or `halleyctl node list --json`. Capture
uses the node's window directly without focusing or revealing it. Titles are
not capture selectors. Closed nodes fail with `NotFound`; unavailable window
content or encoding failures return an error rather than opening a picker.

The matching CLI commands are:

```sh
halleyctl capture screen -o DP-1
halleyctl capture window 123
halleyctl capture region -o DP-1 --rect 100 50 800 600
```

Notifications are enabled by default for screen, window, and region captures.
For automation, opt out with `--silent`, for example:

```sh
halleyctl capture screen -o DP-1 --silent
halleyctl capture window 123 --silent
halleyctl capture region -o DP-1 --rect 100 50 800 600 --silent
```

SDK callers use `Client::capture_target_with_options(target, CaptureOptions {
silent: true })`. Silent capture requires the `silent-capture-v1` capability;
unsupported servers return `Unsupported` without sending the new wire request.
Silent mode suppresses the saved notification, not any required selection UI,
and still returns the completed PNG path. Starting a direct capture clears the
previous screenshot card so repeated captures do not include that card.

Rectangles use **logical pixels relative to the output's top-left**. X/Y must
be nonnegative, dimensions positive, and the complete rectangle must fit within
the output. Invalid rectangles fail with `InvalidRequest`, without clipping.
Display scale determines the saved physical pixel dimensions; for example,
a 100 by 80 logical rectangle at scale 1.5 produces a 150 by 120 PNG.

`Screen { output: None }` opens a screen picker. `Region { rect: None, .. }`
opens region selection, restricted to the specified output when provided;
removing that output cancels selection. `Menu` opens the existing native menu,
including interactive window selection. These interactive requests retain the
saved preview card and can be cancelled. The Print shortcut and portal consent
flows retain their existing behavior. Locked sessions reject capture, and an
active picker causes competing requests to fail with `Busy`.

The existing `Client::capture(mode, output)` remains supported. Its explicit
screen output also captures immediately; window mode retains the legacy picker.
The new CLI requires the new compositor capability and reports `Unsupported`
until an upgraded compositor is running. The public API version remains 1;
the private IPC version is 25, with new requests appended so existing request
encodings remain valid.

## Subscriptions

A subscription returns a race-free initial snapshot and then ordered deltas:

```rust
use halley_api::{Client, Event, EventTopic};

fn watch() -> halley_api::Result<()> {
    let client = Client::connect()?;
    let mut subscription = client.subscribe([EventTopic::Nodes])?;
    render_full_state(&subscription.initial.nodes);

    for event in &mut subscription.events {
        match event? {
            Event::NodeAdded { node, .. } | Event::NodeChanged { node, .. } => {
                update_node(node)
            }
            Event::NodeRemoved { id, .. } => remove_node(id),
            _ => {}
        }
    }
    Ok(())
}

# fn render_full_state(_: &[halley_api::NodeInfo]) {}
# fn update_node(_: halley_api::NodeInfo) {}
# fn remove_node(_: halley_api::NodeId) {}
```

Every delta carries a monotonically increasing sequence number. The SDK checks
continuity and returns `ErrorKind::Protocol` on a gap; reconnect and rebuild
from the next initial snapshot in that case. The compositor uses a bounded
queue per subscriber and disconnects a consumer that cannot keep up, so a
stalled panel cannot consume unbounded compositor memory.

`EventTopic::NodeGeometry` is separate from `EventTopic::Nodes` because motion
can be high-volume. Subscribe only when live geometry is actually required.

## Cluster drafts

`Client::finalize_cluster_draft` is the integration point for launchers. A
draft contains selected running `NodeId`s plus application IDs and commands.
The compositor opens its naming UI, launches applications only after the user
confirms, stages matching Wayland and XWayland windows without exposing partial
state, and publishes draft lifecycle events. Unmatched applications time out
after 30 seconds and the completed membership is committed atomically.

## Errors and compatibility

All SDK operations return `halley_api::Result<T>`. Branch on `Error::kind()`;
messages are diagnostics for humans and are not stable parsing targets. Public
categories include `Connection`, `Protocol`, `InvalidRequest`, `NotFound`,
`Ambiguous`, `Unsupported`, `VersionMismatch`, `Busy`, and `Internal`.

`HALLEY_API_VERSION` versions the semantic contract. The handshake fails with
`VersionMismatch` when a compositor cannot provide that contract. Capability
strings allow optional extensions to be detected without guessing from the
compositor version. Changes to the private IPC codec do not by themselves
change the API version.

The workspace's `halleyctl` and the standalone [Halley Lift](https://github.com/saltnpepper97/halley-lift) are reference consumers: neither
constructs low-level node or cluster wire messages.
