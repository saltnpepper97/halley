use halley_ipc::{
    ApertureStatusResponse, CaptureStatusResponse, ClusterInfo, ClusterLayoutKind,
    ClusterListResponse, ClusterSummary, IpcError, LogicalOutputInfo, NodeInfo, NodeListResponse,
    NodeProtocolFamily, NodeRelationInfo, NodeRole, OutputInfo, OutputStatus, OutputsResponse,
    Response, TrailEntryInfo, TrailListResponse,
};

pub(crate) fn print_response(response: Response) -> Result<(), String> {
    match response {
        Response::Ok => {
            println!("ok");
            Ok(())
        }
        Response::Reloaded => {
            println!("reloaded");
            Ok(())
        }
        Response::Outputs(outputs) => {
            print_outputs(outputs);
            Ok(())
        }
        Response::ApertureStatus(status) => {
            if wants_json() {
                print_json(&status)
            } else {
                print_aperture_status(&status);
                Ok(())
            }
        }
        Response::CaptureStatus(status) => {
            print_capture_status(&status);
            Ok(())
        }
        Response::NodeList(list) => {
            if wants_json() {
                print_json(&list)
            } else {
                print_node_list(&list);
                Ok(())
            }
        }
        Response::NodeInfo(node) => {
            if wants_json() {
                print_json(&node)
            } else {
                print_node_info(&node);
                Ok(())
            }
        }
        Response::ClusterList(list) => {
            if wants_json() {
                print_json(&list)
            } else {
                print_cluster_list(&list);
                Ok(())
            }
        }
        Response::ClusterInfo(cluster) => {
            if wants_json() {
                print_json(&cluster)
            } else {
                print_cluster_info(&cluster);
                Ok(())
            }
        }
        Response::TrailList(list) => {
            if wants_json() {
                print_json(&list)
            } else {
                print_trail_list(&list);
                Ok(())
            }
        }
        Response::BearingsStatus(status) => {
            println!("{}", if status.visible { "visible" } else { "hidden" });
            Ok(())
        }
        Response::Error(err) => Err(format_ipc_error(&err)),
    }
}

fn print_aperture_status(status: &ApertureStatusResponse) {
    let output = status.output.as_deref().unwrap_or("(default)");
    println!("output: {output}");
    println!("mode: {:?}", status.mode);
}

fn wants_json() -> bool {
    std::env::args().any(|arg| arg == "--json")
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|err| err.to_string())?;
    println!("{text}");
    Ok(())
}

fn format_ipc_error(err: &IpcError) -> String {
    match err {
        IpcError::InvalidRequest(message)
        | IpcError::NotFound(message)
        | IpcError::Ambiguous(message)
        | IpcError::Unsupported(message)
        | IpcError::Internal(message) => message.clone(),
    }
}

fn print_capture_status(status: &CaptureStatusResponse) {
    if let Some(path) = &status.saved_path {
        println!("saved: {path}");
    } else if let Some(error) = &status.error {
        println!("capture: {error}");
    } else if status.active {
        println!("capture active");
    } else {
        println!("capture idle");
    }
}

fn print_outputs(outputs: OutputsResponse) {
    if outputs.outputs.is_empty() {
        println!("No outputs.");
        return;
    }
    for output in outputs.outputs {
        print_output(&output);
    }
}

fn print_output(output: &OutputInfo) {
    println!("{}", output.name);
    println!("  status: {}", format_status(output.status));
    println!(
        "  enabled: {}",
        if output.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );

    if let Some(current_mode) = &output.current_mode {
        println!("  current_mode: {}", current_mode.display_string());
    }
    if let Some(vrr_mode) = &output.vrr_mode {
        println!("  vrr: {}", vrr_mode);
    }
    if let Some(vrr_support) = &output.vrr_support {
        println!("  vrr_support: {}", vrr_support);
    }

    if output.modes.is_empty() {
        println!("  modes: (none)");
    } else {
        let has_refresh = output.modes.iter().any(|mode| mode.refresh_hz.is_some());
        if has_refresh {
            println!("  modes:");
        } else {
            println!("  modes: (resolution-only; refresh unavailable)");
        }
        for mode in &output.modes {
            let marker = if mode.current {
                "*"
            } else if mode.preferred {
                "+"
            } else {
                "-"
            };
            println!("    {marker} {}", mode.display_string());
        }
    }

    if let Some(logical) = &output.logical {
        print_logical(logical);
    }
}

fn print_logical(logical: &LogicalOutputInfo) {
    println!("  logical:");
    println!("    scale: {}", logical.scale);
    println!("    focused: {}", logical.focused);
    println!("    offset: {}, {}", logical.offset_x, logical.offset_y);
}

fn print_node_list(list: &NodeListResponse) {
    if list.outputs.iter().all(|group| group.nodes.is_empty()) {
        println!("No nodes.");
        return;
    }
    for group in &list.outputs {
        println!("{}", group.output);
        println!("  nodes: {}", group.nodes.len());
        if group.nodes.is_empty() {
            println!("  entries: (none)");
            continue;
        }
        println!("  entries:");
        for node in &group.nodes {
            print_node_brief(node);
        }
    }
}

fn print_node_info(node: &NodeInfo) {
    println!("{}  {}", node.id, node.title);
    print_node_fields(node, 2);
}

fn print_node_brief(node: &NodeInfo) {
    let marker = if node.focused {
        "*"
    } else if node.latest {
        "+"
    } else {
        "-"
    };
    println!("    {marker} {}  {}", node.id, node.title);
    print_node_fields(node, 6);
}

fn print_node_fields(node: &NodeInfo, indent: usize) {
    let pad = " ".repeat(indent);
    if let Some(output) = &node.output {
        println!("{pad}output: {output}");
    }
    println!("{pad}state: {}", format_node_state(node));
    if let Some(app_id) = &node.app_id {
        println!("{pad}app: {app_id}");
    }
    println!("{pad}role: {}", format_node_role(node.role));
    println!(
        "{pad}protocol: {}",
        format_node_protocol(node.protocol_family)
    );
    println!("{pad}modal: {}", node.modal);
    print_node_relation("parent-node", node.parent.as_ref(), indent);
    print_node_relation("transient-for", node.transient_for.as_ref(), indent);
    if node.child_popup_count > 0 {
        println!("{pad}child-popups: {}", node.child_popup_count);
    }
    println!("{pad}focused: {}", node.focused);
    println!("{pad}latest: {}", node.latest);
    println!("{pad}pos: {:.0}, {:.0}", node.pos_x, node.pos_y);
    println!("{pad}size: {:.0} x {:.0}", node.width, node.height);
}

fn print_node_relation(label: &str, relation: Option<&NodeRelationInfo>, indent: usize) {
    let pad = " ".repeat(indent);
    match relation {
        Some(NodeRelationInfo { node_id: Some(id) }) => println!("{pad}{label}: {id}"),
        Some(NodeRelationInfo { node_id: None }) => println!("{pad}{label}: (unresolved)"),
        None => println!("{pad}{label}: (none)"),
    }
}

fn print_trail_list(list: &TrailListResponse) {
    println!("{}", list.output);
    println!(
        "  cursor: {}",
        list.cursor_index
            .map(|i| i.to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    if list.entries.is_empty() {
        println!("  entries: (none)");
        return;
    }
    println!("  entries:");
    for entry in &list.entries {
        print_trail_entry(entry);
    }
}

fn print_cluster_list(list: &ClusterListResponse) {
    if list.outputs.iter().all(|group| group.clusters.is_empty()) {
        println!("No clusters.");
        return;
    }
    for group in &list.outputs {
        println!("{}", group.output);
        println!("  clusters: {}", group.clusters.len());
        if group.clusters.is_empty() {
            println!("  entries: (none)");
            continue;
        }
        println!("  entries:");
        for cluster in &group.clusters {
            print_cluster_brief(cluster);
        }
    }
}

fn print_cluster_info(cluster: &ClusterInfo) {
    println!(
        "{}  {}",
        cluster.id,
        cluster_display_name(cluster.name.as_deref())
    );
    if let Some(output) = &cluster.output {
        println!("  output: {output}");
    }
    println!(
        "  slot: {}",
        cluster
            .slot
            .map(|slot| slot.to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    println!("  layout: {}", format_cluster_layout(cluster.layout));
    println!("  active: {}", cluster.active);
    println!("  focused: {}", cluster.focused);
    println!("  members: {}", cluster.member_count);
    println!(
        "  focused-member-index: {}",
        cluster
            .focused_member_index
            .map(|index| index.to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    println!(
        "  focused-member-id: {}",
        cluster
            .focused_member_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    if cluster.members.is_empty() {
        println!("  entries: (none)");
        return;
    }
    println!("  entries:");
    for node in &cluster.members {
        print_cluster_member(node);
    }
}

fn print_cluster_brief(cluster: &ClusterSummary) {
    let marker = if cluster.focused {
        "*"
    } else if cluster.active {
        "+"
    } else {
        "-"
    };
    println!(
        "    {marker} {}  {}",
        cluster.id,
        cluster_display_name(cluster.name.as_deref())
    );
    println!(
        "      slot: {}",
        cluster
            .slot
            .map(|slot| slot.to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    println!("      layout: {}", format_cluster_layout(cluster.layout));
    println!("      members: {}", cluster.member_count);
}

fn print_cluster_member(node: &NodeInfo) {
    let marker = if node.focused { "*" } else { "-" };
    println!("    {marker} {}  {}", node.id, node.title);
    if let Some(app_id) = &node.app_id {
        println!("      app: {app_id}");
    }
    println!("      state: {}", format_node_state(node));
}

fn cluster_display_name(name: Option<&str>) -> &str {
    name.unwrap_or("(unnamed)")
}

fn format_cluster_layout(layout: ClusterLayoutKind) -> &'static str {
    match layout {
        ClusterLayoutKind::Tiling => "tiling",
        ClusterLayoutKind::Stacking => "stacking",
    }
}

fn print_trail_entry(entry: &TrailEntryInfo) {
    let marker = if entry.cursor { "*" } else { "-" };
    println!(
        "    {marker} [{}] {}  {}",
        entry.index, entry.node.id, entry.node.title
    );
    if let Some(app_id) = &entry.node.app_id {
        println!("      app: {app_id}");
    }
    println!("      state: {}", format_node_state(&entry.node));
    println!(
        "      pos: {:.0}, {:.0}",
        entry.node.pos_x, entry.node.pos_y
    );
}

fn format_status(status: OutputStatus) -> &'static str {
    match status {
        OutputStatus::Connected => "connected",
        OutputStatus::Disconnected => "disconnected",
        OutputStatus::Unknown => "unknown",
    }
}

fn format_node_state(node: &NodeInfo) -> &'static str {
    match node.state {
        halley_ipc::NodeState::Active => "active",
        halley_ipc::NodeState::Drifting => "drifting",
        halley_ipc::NodeState::Node => "node",
        halley_ipc::NodeState::Core => "core",
    }
}

fn format_node_role(role: NodeRole) -> &'static str {
    match role {
        NodeRole::NormalToplevel => "normal",
        NodeRole::Dialog => "dialog",
        NodeRole::Popup => "popup",
        NodeRole::Unknown => "unknown",
    }
}

fn format_node_protocol(protocol: NodeProtocolFamily) -> &'static str {
    match protocol {
        NodeProtocolFamily::XdgToplevel => "xdg-toplevel",
        NodeProtocolFamily::XdgPopup => "xdg-popup",
        NodeProtocolFamily::Xwayland => "xwayland",
        NodeProtocolFamily::Unknown => "unknown",
    }
}
