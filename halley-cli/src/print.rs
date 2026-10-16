use std::collections::BTreeMap;
use std::fmt::Write;
use std::process::ExitCode;

use halley_api::{
    ClusterInfo, ClusterLayout, ClusterSummary, ModeInfo, NodeInfo, NodeProtocolFamily, NodeRole,
    NodeState, OutputInfo, TrailInfo,
};
use serde::Serialize;

use crate::cmd::{ClusterOutput, NodeOutput, TrailOutput};

pub fn node_list(nodes: Vec<NodeInfo>, output: NodeOutput) -> ExitCode {
    let NodeOutput::List { json } = output else {
        return invalid_output("node list");
    };
    if json {
        return print_json(&group_nodes(&nodes), "node list");
    }
    print!("{}", format_node_list(&nodes));
    ExitCode::SUCCESS
}

pub fn node_info(node: NodeInfo, output: NodeOutput) -> ExitCode {
    let NodeOutput::Info { json } = output else {
        return invalid_output("node info");
    };
    if json {
        return print_json(&node, "node info");
    }
    print!("{}", format_node_info(&node));
    ExitCode::SUCCESS
}

pub fn cluster_list(clusters: Vec<ClusterSummary>, output: ClusterOutput) -> ExitCode {
    let ClusterOutput::List { json } = output else {
        return invalid_output("cluster list");
    };
    if json {
        return print_json(&group_clusters(&clusters), "cluster list");
    }
    print!("{}", format_cluster_list(&clusters));
    ExitCode::SUCCESS
}

pub fn cluster_info(info: ClusterInfo, output: ClusterOutput) -> ExitCode {
    let ClusterOutput::Info { json } = output else {
        return invalid_output("cluster info");
    };
    if json {
        return print_json(&info, "cluster info");
    }
    print!("{}", format_cluster_info(&info));
    ExitCode::SUCCESS
}

pub fn bearings(visible: bool) -> ExitCode {
    println!("{}", if visible { "visible" } else { "hidden" });
    ExitCode::SUCCESS
}

pub fn trail(info: TrailInfo, output: TrailOutput) -> ExitCode {
    let TrailOutput::List { json } = output else {
        return invalid_output("trail list");
    };
    if json {
        return print_json(&info, "trail list");
    }
    print!("{}", format_trail(&info));
    ExitCode::SUCCESS
}

pub fn outputs(outputs: Vec<OutputInfo>) -> ExitCode {
    if outputs.is_empty() {
        println!("(no outputs)");
        return ExitCode::SUCCESS;
    }
    for (index, output) in outputs.iter().enumerate() {
        match format_output(output) {
            Ok(formatted) => {
                if index > 0 {
                    println!();
                }
                print!("{formatted}");
            }
            Err(error) => {
                eprintln!("halleyctl: invalid output response: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

pub fn version() -> ExitCode {
    println!("halleyctl {}", env!("CARGO_PKG_VERSION"));
    ExitCode::SUCCESS
}

#[derive(Serialize)]
struct NodeGroup<'a> {
    output: &'a str,
    nodes: Vec<&'a NodeInfo>,
}

fn group_nodes(nodes: &[NodeInfo]) -> Vec<NodeGroup<'_>> {
    let mut groups: BTreeMap<&str, Vec<&NodeInfo>> = BTreeMap::new();
    for node in nodes {
        groups
            .entry(node.output.as_deref().unwrap_or("(unknown)"))
            .or_default()
            .push(node);
    }
    groups
        .into_iter()
        .map(|(output, nodes)| NodeGroup { output, nodes })
        .collect()
}

#[derive(Serialize)]
struct ClusterGroup<'a> {
    output: &'a str,
    clusters: Vec<&'a ClusterSummary>,
}

fn group_clusters(clusters: &[ClusterSummary]) -> Vec<ClusterGroup<'_>> {
    let mut groups: BTreeMap<&str, Vec<&ClusterSummary>> = BTreeMap::new();
    for cluster in clusters {
        groups.entry(&cluster.output).or_default().push(cluster);
    }
    groups
        .into_iter()
        .map(|(output, clusters)| ClusterGroup { output, clusters })
        .collect()
}

fn print_json(value: &impl Serialize, label: &str) -> ExitCode {
    match serde_json::to_string_pretty(value) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("halleyctl: failed to encode {label}: {error}");
            ExitCode::FAILURE
        }
    }
}

fn invalid_output(label: &str) -> ExitCode {
    eprintln!("halleyctl: internal output mismatch for {label}");
    ExitCode::FAILURE
}

fn format_node_list(nodes: &[NodeInfo]) -> String {
    if nodes.is_empty() {
        return "No nodes.\n".to_string();
    }
    let mut formatted = String::new();
    for group in group_nodes(nodes) {
        writeln!(formatted, "{}", group.output).unwrap();
        writeln!(formatted, "  nodes: {}", group.nodes.len()).unwrap();
        writeln!(formatted, "  entries:").unwrap();
        for node in group.nodes {
            let marker = if node.focused {
                "*"
            } else if node.latest {
                "+"
            } else {
                "-"
            };
            writeln!(formatted, "    {marker} {}  {}", node.id, node.title).unwrap();
            format_node_fields(&mut formatted, node, 6);
        }
    }
    formatted
}

fn format_trail(info: &TrailInfo) -> String {
    if info.entries.is_empty() {
        return format!("{}\n  No trail entries.\n", info.output);
    }
    let mut formatted = String::new();
    writeln!(formatted, "{}", info.output).unwrap();
    for entry in &info.entries {
        let marker = if entry.cursor { "*" } else { "-" };
        writeln!(
            formatted,
            "  {marker} {}: {}  {}",
            entry.index, entry.node.id, entry.node.title
        )
        .unwrap();
    }
    formatted
}

fn format_node_info(node: &NodeInfo) -> String {
    let mut formatted = String::new();
    writeln!(formatted, "{}  {}", node.id, node.title).unwrap();
    format_node_fields(&mut formatted, node, 2);
    formatted
}

fn format_cluster_list(clusters: &[ClusterSummary]) -> String {
    if clusters.is_empty() {
        return "No clusters.\n".to_string();
    }
    let mut formatted = String::new();
    for group in group_clusters(clusters) {
        writeln!(formatted, "{}", group.output).unwrap();
        for cluster in group.clusters {
            let marker = if cluster.active {
                "*"
            } else if cluster.focused {
                "+"
            } else {
                "-"
            };
            let slot = cluster
                .slot
                .map(|slot| slot.to_string())
                .unwrap_or_else(|| "?".into());
            writeln!(
                formatted,
                "  {marker} slot {slot}: {}  {} [{}; {} window{}]",
                cluster.id,
                cluster.name,
                cluster_layout(cluster.layout),
                cluster.member_count,
                if cluster.member_count == 1 { "" } else { "s" },
            )
            .unwrap();
        }
    }
    formatted
}

fn format_cluster_info(info: &ClusterInfo) -> String {
    let mut formatted = String::new();
    writeln!(formatted, "{}  {}", info.summary.id, info.summary.name).unwrap();
    writeln!(formatted, "  output: {}", info.summary.output).unwrap();
    writeln!(
        formatted,
        "  slot: {}",
        info.summary
            .slot
            .map(|slot| slot.to_string())
            .unwrap_or_else(|| "(none)".into())
    )
    .unwrap();
    writeln!(
        formatted,
        "  layout: {}",
        cluster_layout(info.summary.layout)
    )
    .unwrap();
    writeln!(formatted, "  active: {}", info.summary.active).unwrap();
    writeln!(formatted, "  focused: {}", info.summary.focused).unwrap();
    writeln!(formatted, "  members: {}", info.members.len()).unwrap();
    for member in &info.members {
        writeln!(formatted, "    - {}  {}", member.id, member.title).unwrap();
    }
    formatted
}

fn cluster_layout(layout: ClusterLayout) -> &'static str {
    match layout {
        ClusterLayout::Tiling => "tiling",
        ClusterLayout::Stacking => "stacking",
    }
}

fn format_node_fields(formatted: &mut String, node: &NodeInfo, indent: usize) {
    let pad = " ".repeat(indent);
    if let Some(output) = &node.output {
        writeln!(formatted, "{pad}output: {output}").unwrap();
    }
    writeln!(formatted, "{pad}state: {}", node_state(node.state)).unwrap();
    if let Some(app_id) = &node.app_id {
        writeln!(formatted, "{pad}app: {app_id}").unwrap();
    }
    writeln!(formatted, "{pad}role: {}", node_role(node.role)).unwrap();
    writeln!(
        formatted,
        "{pad}protocol: {}",
        node_protocol(node.protocol_family)
    )
    .unwrap();
    writeln!(formatted, "{pad}modal: {}", node.modal).unwrap();
    format_relation(formatted, "parent-node", node.parent, indent);
    format_relation(formatted, "transient-for", node.transient_for, indent);
    if node.child_popup_count > 0 {
        writeln!(formatted, "{pad}child-popups: {}", node.child_popup_count).unwrap();
    }
    writeln!(formatted, "{pad}focused: {}", node.focused).unwrap();
    writeln!(formatted, "{pad}latest: {}", node.latest).unwrap();
    writeln!(formatted, "{pad}pos: {:.0}, {:.0}", node.x, node.y).unwrap();
    writeln!(
        formatted,
        "{pad}size: {:.0} x {:.0}",
        node.width, node.height
    )
    .unwrap();
}

fn format_relation(
    formatted: &mut String,
    label: &str,
    relation: Option<halley_api::NodeId>,
    indent: usize,
) {
    let pad = " ".repeat(indent);
    match relation {
        Some(id) => writeln!(formatted, "{pad}{label}: {id}").unwrap(),
        None => writeln!(formatted, "{pad}{label}: (none)").unwrap(),
    }
}

fn node_state(state: NodeState) -> &'static str {
    match state {
        NodeState::Active => "active",
        NodeState::Drifting => "drifting",
        NodeState::Node => "node",
        NodeState::Core => "core",
    }
}

fn node_role(role: NodeRole) -> &'static str {
    match role {
        NodeRole::NormalToplevel => "normal",
        NodeRole::Dialog => "dialog",
        NodeRole::Popup => "popup",
        NodeRole::Unknown => "unknown",
    }
}

fn node_protocol(protocol: NodeProtocolFamily) -> &'static str {
    match protocol {
        NodeProtocolFamily::XdgToplevel => "xdg-toplevel",
        NodeProtocolFamily::XdgPopup => "xdg-popup",
        NodeProtocolFamily::Xwayland => "xwayland",
        NodeProtocolFamily::Unknown => "unknown",
    }
}

fn format_output(output: &OutputInfo) -> Result<String, String> {
    let mut formatted = String::new();
    writeln!(formatted, "{}", output.name).unwrap();
    if let Some(current_index) = output.current_mode {
        let current = output.modes.get(current_index).ok_or_else(|| {
            format!(
                "{} refers to missing current mode index {current_index}",
                output.name
            )
        })?;
        writeln!(
            formatted,
            "  Current mode: {}x{} @ {:.3} Hz{}",
            current.width,
            current.height,
            refresh_hz(current),
            mode_qualifier(current.preferred, false),
        )
        .unwrap();
        writeln!(
            formatted,
            "  Position: {}, {}",
            output.offset_x, output.offset_y
        )
        .unwrap();
        writeln!(
            formatted,
            "  Scale: {} ({}%)",
            output.scale,
            output.scale * 100.0
        )
        .unwrap();
        let vrr_state = if !output.vrr_supported {
            "unsupported"
        } else if output.vrr_active {
            "active"
        } else {
            "inactive"
        };
        writeln!(formatted, "  VRR: {} ({vrr_state})", output.vrr).unwrap();
    } else {
        writeln!(formatted, "  Disabled").unwrap();
    }
    if output.modes.is_empty() {
        writeln!(formatted, "  Available modes: (none)").unwrap();
        return Ok(formatted);
    }
    writeln!(formatted, "  Available modes:").unwrap();
    for (index, mode) in output.modes.iter().enumerate() {
        writeln!(
            formatted,
            "    {}x{}@{:.3}{}",
            mode.width,
            mode.height,
            refresh_hz(mode),
            mode_qualifier(mode.preferred, Some(index) == output.current_mode),
        )
        .unwrap();
    }
    Ok(formatted)
}

fn refresh_hz(mode: &ModeInfo) -> f64 {
    mode.refresh_millihz as f64 / 1000.0
}

fn mode_qualifier(preferred: bool, current: bool) -> &'static str {
    match (current, preferred) {
        (true, true) => " (current, preferred)",
        (true, false) => " (current)",
        (false, true) => " (preferred)",
        (false, false) => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_api::{NodeId, NodeKind};

    fn sample_node(id: u64, focused: bool, latest: bool) -> NodeInfo {
        NodeInfo {
            id: NodeId::new(id),
            title: "Firefox".into(),
            app_id: Some("firefox".into()),
            output: Some("DP-1".into()),
            kind: NodeKind::Surface,
            state: NodeState::Active,
            visible: true,
            focused,
            latest,
            pinned: false,
            role: NodeRole::NormalToplevel,
            protocol_family: NodeProtocolFamily::XdgToplevel,
            modal: false,
            parent: None,
            transient_for: Some(NodeId::new(4)),
            child_popup_count: 1,
            x: 12.0,
            y: 34.0,
            width: 1280.0,
            height: 720.0,
        }
    }

    #[test]
    fn rich_node_list_groups_outputs_and_marks_focus() {
        let formatted = format_node_list(&[sample_node(7, true, false)]);
        assert!(formatted.starts_with("DP-1\n  nodes: 1\n  entries:\n    * 7  Firefox\n"));
        assert!(formatted.contains("      transient-for: 4\n"));
    }

    #[test]
    fn rich_node_info_includes_protocol_relations_and_geometry() {
        let formatted = format_node_info(&sample_node(9, false, true));
        assert!(formatted.starts_with("9  Firefox\n"));
        assert!(formatted.contains("  protocol: xdg-toplevel\n"));
        assert!(formatted.contains("  pos: 12, 34\n"));
        assert!(formatted.contains("  size: 1280 x 720\n"));
    }

    #[test]
    fn empty_lists_are_explicit() {
        assert_eq!(format_node_list(&[]), "No nodes.\n");
        assert_eq!(format_cluster_list(&[]), "No clusters.\n");
    }

    #[test]
    fn rejects_invalid_current_output_mode() {
        let output = OutputInfo {
            name: "DP-1".into(),
            modes: Vec::new(),
            current_mode: Some(1),
            offset_x: 0,
            offset_y: 0,
            scale: 1.0,
            vrr: "off".into(),
            vrr_supported: false,
            vrr_active: false,
        };
        assert_eq!(
            format_output(&output),
            Err("DP-1 refers to missing current mode index 1".into())
        );
    }
}
