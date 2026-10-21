# `halley-api`

`halley-api` is the supported Rust SDK for building launchers, panels,
automation, and other tools against the Halley compositor. It provides typed
commands and queries, semantic IDs and state types, structured errors, and
sequenced subscriptions with an initial snapshot.

```rust
use halley_api::{Client, EventTopic, NodeSelector};

fn main() -> halley_api::Result<()> {
    let client = Client::connect()?;
    client.focus_node(Some(NodeSelector::Latest), None)?;

    let mut subscription = client.subscribe([EventTopic::Nodes])?;
    println!("{} nodes already exist", subscription.initial.nodes.len());
    for event in &mut subscription.events {
        println!("{:?}", event?);
    }
    Ok(())
}
```

See [`docs/api.md`](../docs/api.md) for the contract, operation list, event
semantics, and integration guidance.

For precise movement, `Client::move_node_by(direction, distance, selector, output)`
moves up to a positive number of Field units and stops at obstructions. It requires
`node-move-distance-v1`; the existing `move_node` method retains its behavior.

`Client::move_node_to(x, y, selector, output)` requests exact Field placement and
requires `node-move-to-v1`. Expanded-window movement follows the compositor's
`animations.window-move` settings, including when using existing move methods.
