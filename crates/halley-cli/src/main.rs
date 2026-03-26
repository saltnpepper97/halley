use halley_ipc::{
    CompositorRequest, DpmsCommand, IpcError, LogicalOutputInfo, MonitorFocusDirection,
    MonitorFocusTarget, MonitorRequest, NodeInfo, NodeListResponse, NodeMoveDirection,
    NodeRequest, NodeSelector, OutputInfo, OutputStatus, OutputsResponse, Request, Response,
    TrailEntryInfo, TrailListResponse, TrailRequest, TrailTarget, send_request,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let request = match parse_request(&args) {
        Ok(Some(request)) => request,
        Ok(None) => {
            print_help();
            return;
        }
        Err(err) => exit_usage(err.as_str()),
    };

    match send_request(&request) {
        Ok(response) => {
            if let Err(err) = print_response(response) {
                eprintln!("halleyctl failed: {err}");
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("halleyctl failed to talk to halley: {err}");
            std::process::exit(1);
        }
    }
}

fn parse_request(args: &[String]) -> Result<Option<Request>, String> {
    if args.is_empty() {
        return Ok(None);
    }

    match args[0].as_str() {
        "help" | "--help" | "-h" => Ok(None),
        "comp" => parse_comp_request(&args[1..]).map(Some),
        "node" => parse_node_request(&args[1..]).map(Some),
        "trail" => parse_trail_request(&args[1..]).map(Some),
        "monitor" => parse_monitor_request(&args[1..]).map(Some),
        "quit" => Ok(Some(Request::Compositor(CompositorRequest::Quit))),
        "reload" => Ok(Some(Request::Compositor(CompositorRequest::Reload))),
        "outputs" => Ok(Some(Request::Compositor(CompositorRequest::Outputs))),
        "dpms" => parse_comp_dpms(&args[1..]).map(Some),
        other => Err(format!("unknown command: {other}")),
    }
}

fn parse_comp_request(args: &[String]) -> Result<Request, String> {
    match args.first().map(|value| value.as_str()) {
        Some("quit") => Ok(Request::Compositor(CompositorRequest::Quit)),
        Some("reload") => Ok(Request::Compositor(CompositorRequest::Reload)),
        Some("outputs") => Ok(Request::Compositor(CompositorRequest::Outputs)),
        Some("dpms") => parse_comp_dpms(&args[1..]),
        Some(other) => Err(format!("unknown comp command: {other}")),
        None => Err("missing comp command".into()),
    }
}

fn parse_comp_dpms(args: &[String]) -> Result<Request, String> {
    let mut positionals = Vec::new();
    let mut output = None;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("missing value for -o/--output".into());
                };
                output = Some(value.clone());
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option for comp dpms: {other}"));
            }
            _ => positionals.push(args[index].clone()),
        }
        index += 1;
    }

    let Some(command) = positionals.first() else {
        return Err("missing dpms command".into());
    };
    let command = match command.as_str() {
        "off" => DpmsCommand::Off,
        "on" => DpmsCommand::On,
        "toggle" => DpmsCommand::Toggle,
        other => return Err(format!("unknown dpms command: {other}")),
    };

    Ok(Request::Compositor(CompositorRequest::Dpms { command, output }))
}

fn parse_node_request(args: &[String]) -> Result<Request, String> {
    match args.first().map(|value| value.as_str()) {
        Some("list") => {
            let output = parse_output_option(&args[1..])?;
            Ok(Request::Node(NodeRequest::List { output }))
        }
        Some("info") => parse_node_selector_command(&args[1..], NodeSelectorCommand::Info),
        Some("focus") => parse_node_selector_command(&args[1..], NodeSelectorCommand::Focus),
        Some("close") => parse_node_selector_command(&args[1..], NodeSelectorCommand::Close),
        Some("move") => parse_node_move(&args[1..]),
        Some(other) => Err(format!("unknown node command: {other}")),
        None => Err("missing node command".into()),
    }
}

fn parse_node_move(args: &[String]) -> Result<Request, String> {
    let Some(direction) = args.first() else {
        return Err("missing node move direction".into());
    };
    let direction = parse_move_direction(direction)?;
    let (selector, output, _json) = parse_selector_flags(&args[1..])?;
    Ok(Request::Node(NodeRequest::Move {
        direction,
        selector,
        output,
    }))
}

#[derive(Clone, Copy)]
enum NodeSelectorCommand {
    Info,
    Focus,
    Close,
}

fn parse_node_selector_command(
    args: &[String],
    command: NodeSelectorCommand,
) -> Result<Request, String> {
    let (selector, output, _json) = parse_selector_flags(args)?;
    Ok(Request::Node(match command {
        NodeSelectorCommand::Info => NodeRequest::Info { selector, output },
        NodeSelectorCommand::Focus => NodeRequest::Focus { selector, output },
        NodeSelectorCommand::Close => NodeRequest::Close { selector, output },
    }))
}

fn parse_trail_request(args: &[String]) -> Result<Request, String> {
    match args.first().map(|value| value.as_str()) {
        Some("prev") => Ok(Request::Trail(TrailRequest::Prev {
            output: parse_output_option(&args[1..])?,
        })),
        Some("next") => Ok(Request::Trail(TrailRequest::Next {
            output: parse_output_option(&args[1..])?,
        })),
        Some("list") => Ok(Request::Trail(TrailRequest::List {
            output: parse_output_option(&args[1..])?,
        })),
        Some("goto") => {
            let Some(target) = args.get(1) else {
                return Err("missing trail goto target".into());
            };
            let output = parse_output_option(&args[2..])?;
            Ok(Request::Trail(TrailRequest::Goto {
                target: parse_trail_target(target.as_str())?,
                output,
            }))
        }
        Some(other) => Err(format!("unknown trail command: {other}")),
        None => Err("missing trail command".into()),
    }
}

fn parse_monitor_request(args: &[String]) -> Result<Request, String> {
    match args.first().map(|value| value.as_str()) {
        Some("focus") => {
            let Some(target) = args.get(1) else {
                return Err("missing monitor focus target".into());
            };
            Ok(Request::Monitor(MonitorRequest::Focus(
                parse_monitor_focus_target(target.as_str()),
            )))
        }
        Some(other) => Err(format!("unknown monitor command: {other}")),
        None => Err("missing monitor command".into()),
    }
}

fn parse_output_option(args: &[String]) -> Result<Option<String>, String> {
    let mut output = None;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("missing value for -o/--output".into());
                };
                output = Some(value.clone());
            }
            "--json" => {}
            other => return Err(format!("unexpected argument: {other}")),
        }
        index += 1;
    }
    Ok(output)
}

fn parse_selector_flags(
    args: &[String],
) -> Result<(Option<NodeSelector>, Option<String>, bool), String> {
    let mut selector = None;
    let mut output = None;
    let mut json = false;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("missing value for -o/--output".into());
                };
                output = Some(value.clone());
            }
            "--json" => json = true,
            other if other.starts_with('-') => return Err(format!("unknown option: {other}")),
            other => {
                if selector.is_some() {
                    return Err(format!("unexpected extra selector argument: {other}"));
                }
                selector = Some(parse_node_selector(other)?);
            }
        }
        index += 1;
    }
    Ok((selector, output, json))
}

fn parse_node_selector(text: &str) -> Result<NodeSelector, String> {
    if text.eq_ignore_ascii_case("focused") {
        return Ok(NodeSelector::Focused);
    }
    if text.eq_ignore_ascii_case("latest") {
        return Ok(NodeSelector::Latest);
    }
    if let Ok(id) = text.parse::<u64>() {
        return Ok(NodeSelector::Id(id));
    }
    if let Some(value) = text.strip_prefix("id:") {
        return value
            .parse::<u64>()
            .map(NodeSelector::Id)
            .map_err(|_| format!("invalid node id selector: {text}"));
    }
    if let Some(value) = text.strip_prefix("title:") {
        return Ok(NodeSelector::Title(value.to_string()));
    }
    if let Some(value) = text.strip_prefix("app:") {
        return Ok(NodeSelector::App(value.to_string()));
    }
    Err(format!("unknown node selector: {text}"))
}

fn parse_trail_target(text: &str) -> Result<TrailTarget, String> {
    if let Ok(index) = text.parse::<usize>() {
        return Ok(TrailTarget::Index(index));
    }
    Ok(TrailTarget::Selector(parse_node_selector(text)?))
}

fn parse_move_direction(text: &str) -> Result<NodeMoveDirection, String> {
    match text {
        "left" => Ok(NodeMoveDirection::Left),
        "right" => Ok(NodeMoveDirection::Right),
        "up" => Ok(NodeMoveDirection::Up),
        "down" => Ok(NodeMoveDirection::Down),
        other => Err(format!("unknown move direction: {other}")),
    }
}

fn parse_monitor_focus_target(text: &str) -> MonitorFocusTarget {
    match text {
        "left" => MonitorFocusTarget::Direction(MonitorFocusDirection::Left),
        "right" => MonitorFocusTarget::Direction(MonitorFocusDirection::Right),
        "up" => MonitorFocusTarget::Direction(MonitorFocusDirection::Up),
        "down" => MonitorFocusTarget::Direction(MonitorFocusDirection::Down),
        other => MonitorFocusTarget::Output(other.to_string()),
    }
}

fn exit_usage(message: &str) -> ! {
    eprintln!("{message}");
    print_help();
    std::process::exit(2);
}

fn print_help() {
    println!("halleyctl");
    println!();
    println!("Usage:");
    println!("  halleyctl comp quit");
    println!("  halleyctl comp reload");
    println!("  halleyctl comp outputs");
    println!("  halleyctl comp dpms off|on|toggle [-o OUTPUT]");
    println!("  halleyctl node list [-o OUTPUT] [--json]");
    println!("  halleyctl node info [SELECTOR] [-o OUTPUT] [--json]");
    println!("  halleyctl node focus [SELECTOR] [-o OUTPUT]");
    println!("  halleyctl node move left|right|up|down [SELECTOR] [-o OUTPUT]");
    println!("  halleyctl node close [SELECTOR] [-o OUTPUT]");
    println!("  halleyctl trail prev [-o OUTPUT]");
    println!("  halleyctl trail next [-o OUTPUT]");
    println!("  halleyctl trail list [-o OUTPUT] [--json]");
    println!("  halleyctl trail goto <TARGET> [-o OUTPUT]");
    println!("  halleyctl monitor focus <left|right|up|down|OUTPUT>");
}

fn print_response(response: Response) -> Result<(), String> {
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
        Response::TrailList(list) => {
            if wants_json() {
                print_json(&list)
            } else {
                print_trail_list(&list);
                Ok(())
            }
        }
        Response::Error(err) => Err(format_ipc_error(&err)),
    }
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
        if output.enabled { "enabled" } else { "disabled" }
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
    println!("{pad}focused: {}", node.focused);
    println!("{pad}latest: {}", node.latest);
    println!("{pad}pos: {:.0}, {:.0}", node.pos_x, node.pos_y);
    println!("{pad}size: {:.0} x {:.0}", node.width, node.height);
}

fn print_trail_list(list: &TrailListResponse) {
    println!("{}", list.output);
    println!(
        "  cursor: {}",
        list.cursor_index
            .map(|index| index.to_string())
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
    println!("      pos: {:.0}, {:.0}", entry.node.pos_x, entry.node.pos_y);
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
