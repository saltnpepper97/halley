use halley_ipc::{
    BearingsRequest, CompositorRequest, DpmsCommand, IpcError, LogicalOutputInfo,
    MonitorFocusDirection, MonitorFocusTarget, MonitorRequest, NodeInfo, NodeListResponse,
    NodeMoveDirection, NodeProtocolFamily, NodeRelationInfo, NodeRequest, NodeRole, NodeSelector,
    OutputInfo, OutputStatus, OutputsResponse, Request, Response, TrailEntryInfo,
    TrailListResponse, TrailRequest, TrailTarget, send_request,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_request(&args) {
        Ok(ParseOutcome::Request(request)) => match send_request(&request) {
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
        },
        Ok(ParseOutcome::Help(topic)) => print_help(topic),
        Err(err) => exit_usage(err),
    }
}

enum ParseOutcome {
    Request(Request),
    Help(HelpTopic),
}

#[derive(Clone, Copy)]
enum HelpTopic {
    Top,
    Comp,
    CompDpms,
    Node,
    NodeList,
    NodeInfo,
    NodeFocus,
    NodeMove,
    NodeClose,
    Trail,
    TrailPrev,
    TrailNext,
    TrailList,
    TrailGoto,
    Monitor,
    MonitorFocus,
    Bearings,
    BearingsShow,
    BearingsHide,
    BearingsToggle,
    BearingsStatus,
}

struct UsageError {
    message: String,
    help: HelpTopic,
}

impl UsageError {
    fn new(message: impl Into<String>, help: HelpTopic) -> Self {
        Self {
            message: message.into(),
            help,
        }
    }
}

fn parse_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if args.is_empty() {
        return Ok(ParseOutcome::Help(HelpTopic::Top));
    }

    match args[0].as_str() {
        "help" | "--help" | "-h" => Ok(ParseOutcome::Help(HelpTopic::Top)),
        "comp" => parse_comp_request(&args[1..]),
        "node" => parse_node_request(&args[1..]),
        "trail" => parse_trail_request(&args[1..]),
        "monitor" => parse_monitor_request(&args[1..]),
        "bearings" => parse_bearings_request(&args[1..]),
        "quit" => parse_leaf_alias(
            &args[1..],
            HelpTopic::Comp,
            Request::Compositor(CompositorRequest::Quit),
        ),
        "reload" => parse_leaf_alias(
            &args[1..],
            HelpTopic::Comp,
            Request::Compositor(CompositorRequest::Reload),
        ),
        "outputs" => parse_leaf_alias(
            &args[1..],
            HelpTopic::Comp,
            Request::Compositor(CompositorRequest::Outputs),
        ),
        "dpms" => parse_comp_dpms(&args[1..]),
        other => Err(UsageError::new(
            format!("unknown command: {other}"),
            HelpTopic::Top,
        )),
    }
}

fn parse_leaf_alias(
    args: &[String],
    help: HelpTopic,
    request: Request,
) -> Result<ParseOutcome, UsageError> {
    if contains_help_flag(args) {
        return Ok(ParseOutcome::Help(help));
    }
    if let Some(arg) = args.first() {
        return Err(UsageError::new(format!("unexpected argument: {arg}"), help));
    }
    Ok(ParseOutcome::Request(request))
}

fn parse_comp_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => Ok(ParseOutcome::Help(HelpTopic::Comp)),
        Some("quit") => parse_leaf_command(
            &args[1..],
            HelpTopic::Comp,
            Request::Compositor(CompositorRequest::Quit),
        ),
        Some("reload") => parse_leaf_command(
            &args[1..],
            HelpTopic::Comp,
            Request::Compositor(CompositorRequest::Reload),
        ),
        Some("outputs") => parse_leaf_command(
            &args[1..],
            HelpTopic::Comp,
            Request::Compositor(CompositorRequest::Outputs),
        ),
        Some("dpms") => parse_comp_dpms(&args[1..]),
        Some(other) => Err(UsageError::new(
            format!("unknown comp command: {other}"),
            HelpTopic::Comp,
        )),
    }
}

fn parse_leaf_command(
    args: &[String],
    help: HelpTopic,
    request: Request,
) -> Result<ParseOutcome, UsageError> {
    if args.is_empty() {
        return Ok(ParseOutcome::Request(request));
    }
    if contains_help_flag(args) {
        return Ok(ParseOutcome::Help(help));
    }
    Err(UsageError::new(
        format!("unexpected argument: {}", args[0]),
        help,
    ))
}

fn parse_comp_dpms(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if args.is_empty() || contains_help_flag(args) {
        return Ok(ParseOutcome::Help(HelpTopic::CompDpms));
    }

    let mut positionals = Vec::new();
    let mut output = None;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(UsageError::new(
                        "missing value for -o/--output",
                        HelpTopic::CompDpms,
                    ));
                };
                output = Some(value.clone());
            }
            other if other.starts_with('-') => {
                return Err(UsageError::new(
                    format!("unknown option for comp dpms: {other}"),
                    HelpTopic::CompDpms,
                ));
            }
            _ => positionals.push(args[index].clone()),
        }
        index += 1;
    }

    let Some(command) = positionals.first() else {
        return Ok(ParseOutcome::Help(HelpTopic::CompDpms));
    };
    if positionals.len() > 1 {
        return Err(UsageError::new(
            format!("unexpected argument: {}", positionals[1]),
            HelpTopic::CompDpms,
        ));
    }
    let command = match command.as_str() {
        "off" => DpmsCommand::Off,
        "on" => DpmsCommand::On,
        "toggle" => DpmsCommand::Toggle,
        other => {
            return Err(UsageError::new(
                format!("unknown dpms command: {other}"),
                HelpTopic::CompDpms,
            ));
        }
    };

    Ok(ParseOutcome::Request(Request::Compositor(
        CompositorRequest::Dpms { command, output },
    )))
}

fn parse_node_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => Ok(ParseOutcome::Help(HelpTopic::Node)),
        Some("list") => parse_node_list(&args[1..]),
        Some("info") => parse_node_selector_command(&args[1..], NodeSelectorCommand::Info),
        Some("focus") => parse_node_selector_command(&args[1..], NodeSelectorCommand::Focus),
        Some("close") => parse_node_selector_command(&args[1..], NodeSelectorCommand::Close),
        Some("move") => parse_node_move(&args[1..]),
        Some(other) => Err(UsageError::new(
            format!("unknown node command: {other}"),
            HelpTopic::Node,
        )),
    }
}

fn parse_node_list(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if contains_help_flag(args) {
        return Ok(ParseOutcome::Help(HelpTopic::NodeList));
    }
    let output = parse_output_option(args, HelpTopic::NodeList)?;
    Ok(ParseOutcome::Request(Request::Node(NodeRequest::List {
        output,
    })))
}

fn parse_node_move(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if args.is_empty() || contains_help_flag(args) {
        return Ok(ParseOutcome::Help(HelpTopic::NodeMove));
    }
    let direction = parse_move_direction(&args[0])?;
    let (selector, output, _json) = parse_selector_flags(&args[1..], HelpTopic::NodeMove)?;
    Ok(ParseOutcome::Request(Request::Node(NodeRequest::Move {
        direction,
        selector,
        output,
    })))
}

#[derive(Clone, Copy)]
enum NodeSelectorCommand {
    Info,
    Focus,
    Close,
}

impl NodeSelectorCommand {
    fn help_topic(self) -> HelpTopic {
        match self {
            Self::Info => HelpTopic::NodeInfo,
            Self::Focus => HelpTopic::NodeFocus,
            Self::Close => HelpTopic::NodeClose,
        }
    }
}

fn parse_node_selector_command(
    args: &[String],
    command: NodeSelectorCommand,
) -> Result<ParseOutcome, UsageError> {
    if contains_help_flag(args) {
        return Ok(ParseOutcome::Help(command.help_topic()));
    }
    let (selector, output, _json) = parse_selector_flags(args, command.help_topic())?;
    Ok(ParseOutcome::Request(Request::Node(match command {
        NodeSelectorCommand::Info => NodeRequest::Info { selector, output },
        NodeSelectorCommand::Focus => NodeRequest::Focus { selector, output },
        NodeSelectorCommand::Close => NodeRequest::Close { selector, output },
    })))
}

fn parse_trail_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => Ok(ParseOutcome::Help(HelpTopic::Trail)),
        Some("prev") => parse_trail_prev_next(&args[1..], HelpTopic::TrailPrev, true),
        Some("next") => parse_trail_prev_next(&args[1..], HelpTopic::TrailNext, false),
        Some("list") => parse_trail_list(&args[1..]),
        Some("goto") => parse_trail_goto(&args[1..]),
        Some(other) => Err(UsageError::new(
            format!("unknown trail command: {other}"),
            HelpTopic::Trail,
        )),
    }
}

fn parse_trail_prev_next(
    args: &[String],
    help: HelpTopic,
    prev: bool,
) -> Result<ParseOutcome, UsageError> {
    if contains_help_flag(args) {
        return Ok(ParseOutcome::Help(help));
    }
    let output = parse_output_option(args, help)?;
    let request = if prev {
        TrailRequest::Prev { output }
    } else {
        TrailRequest::Next { output }
    };
    Ok(ParseOutcome::Request(Request::Trail(request)))
}

fn parse_trail_list(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if contains_help_flag(args) {
        return Ok(ParseOutcome::Help(HelpTopic::TrailList));
    }
    Ok(ParseOutcome::Request(Request::Trail(TrailRequest::List {
        output: parse_output_option(args, HelpTopic::TrailList)?,
    })))
}

fn parse_trail_goto(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if args.is_empty() || contains_help_flag(args) {
        return Ok(ParseOutcome::Help(HelpTopic::TrailGoto));
    }
    let target = parse_trail_target(&args[0])?;
    let output = parse_output_option(&args[1..], HelpTopic::TrailGoto)?;
    Ok(ParseOutcome::Request(Request::Trail(TrailRequest::Goto {
        target,
        output,
    })))
}

fn parse_monitor_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => Ok(ParseOutcome::Help(HelpTopic::Monitor)),
        Some("focus") => parse_monitor_focus(&args[1..]),
        Some(other) => Err(UsageError::new(
            format!("unknown monitor command: {other}"),
            HelpTopic::Monitor,
        )),
    }
}

fn parse_monitor_focus(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if args.is_empty() || contains_help_flag(args) {
        return Ok(ParseOutcome::Help(HelpTopic::MonitorFocus));
    }
    if args.len() > 1 {
        return Err(UsageError::new(
            format!("unexpected argument: {}", args[1]),
            HelpTopic::MonitorFocus,
        ));
    }
    Ok(ParseOutcome::Request(Request::Monitor(
        MonitorRequest::Focus(parse_monitor_focus_target(&args[0])),
    )))
}

fn parse_bearings_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => Ok(ParseOutcome::Help(HelpTopic::Bearings)),
        Some("show") => parse_leaf_command(
            &args[1..],
            HelpTopic::BearingsShow,
            Request::Bearings(BearingsRequest::Show),
        ),
        Some("hide") => parse_leaf_command(
            &args[1..],
            HelpTopic::BearingsHide,
            Request::Bearings(BearingsRequest::Hide),
        ),
        Some("toggle") => parse_leaf_command(
            &args[1..],
            HelpTopic::BearingsToggle,
            Request::Bearings(BearingsRequest::Toggle),
        ),
        Some("status") => parse_leaf_command(
            &args[1..],
            HelpTopic::BearingsStatus,
            Request::Bearings(BearingsRequest::Status),
        ),
        Some(other) => Err(UsageError::new(
            format!("unknown bearings command: {other}"),
            HelpTopic::Bearings,
        )),
    }
}

fn parse_output_option(args: &[String], help: HelpTopic) -> Result<Option<String>, UsageError> {
    let mut output = None;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(UsageError::new("missing value for -o/--output", help));
                };
                output = Some(value.clone());
            }
            "--json" => {}
            other => {
                return Err(UsageError::new(
                    format!("unexpected argument: {other}"),
                    help,
                ));
            }
        }
        index += 1;
    }
    Ok(output)
}

fn parse_selector_flags(
    args: &[String],
    help: HelpTopic,
) -> Result<(Option<NodeSelector>, Option<String>, bool), UsageError> {
    let mut selector = None;
    let mut output = None;
    let mut json = false;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(UsageError::new("missing value for -o/--output", help));
                };
                output = Some(value.clone());
            }
            "--json" => json = true,
            other if other.starts_with('-') => {
                return Err(UsageError::new(format!("unknown option: {other}"), help));
            }
            other => {
                if selector.is_some() {
                    return Err(UsageError::new(
                        format!("unexpected extra selector argument: {other}"),
                        help,
                    ));
                }
                selector = Some(parse_node_selector(other)?);
            }
        }
        index += 1;
    }
    Ok((selector, output, json))
}

fn parse_node_selector(text: &str) -> Result<NodeSelector, UsageError> {
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
        return value.parse::<u64>().map(NodeSelector::Id).map_err(|_| {
            UsageError::new(format!("invalid node id selector: {text}"), HelpTopic::Node)
        });
    }
    if let Some(value) = text.strip_prefix("title:") {
        return Ok(NodeSelector::Title(value.to_string()));
    }
    if let Some(value) = text.strip_prefix("app:") {
        return Ok(NodeSelector::App(value.to_string()));
    }
    Err(UsageError::new(
        format!("unknown node selector: {text}"),
        HelpTopic::Node,
    ))
}

fn parse_trail_target(text: &str) -> Result<TrailTarget, UsageError> {
    if let Ok(index) = text.parse::<usize>() {
        return Ok(TrailTarget::Index(index));
    }
    Ok(TrailTarget::Selector(parse_node_selector(text)?))
}

fn parse_move_direction(text: &str) -> Result<NodeMoveDirection, UsageError> {
    match text {
        "left" => Ok(NodeMoveDirection::Left),
        "right" => Ok(NodeMoveDirection::Right),
        "up" => Ok(NodeMoveDirection::Up),
        "down" => Ok(NodeMoveDirection::Down),
        other => Err(UsageError::new(
            format!("unknown move direction: {other}"),
            HelpTopic::NodeMove,
        )),
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

fn contains_help_flag(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "-h" || arg == "--help")
}

fn exit_usage(error: UsageError) -> ! {
    eprintln!("{}", error.message);
    eprintln!();
    print_help(error.help);
    std::process::exit(2);
}

fn print_help(topic: HelpTopic) {
    match topic {
        HelpTopic::Top => print_help_page(
            "halleyctl",
            &["halleyctl <command> [args]"],
            &[
                ("comp", "Compositor/runtime controls"),
                ("node", "Node actions and inspection"),
                ("trail", "Trail navigation and inspection"),
                ("monitor", "Monitor-related actions"),
                ("bearings", "Bearings visibility controls"),
            ],
        ),
        HelpTopic::Comp => print_help_page(
            "halleyctl comp",
            &[
                "halleyctl comp quit",
                "halleyctl comp reload",
                "halleyctl comp outputs",
                "halleyctl comp dpms off|on|toggle [-o OUTPUT]",
            ],
            &[
                ("quit", "Ask the running Halley compositor to exit"),
                (
                    "reload",
                    "Ask the running Halley compositor to reload config",
                ),
                ("outputs", "Print current output information"),
                ("dpms", "Control output power state"),
            ],
        ),
        HelpTopic::CompDpms => print_help_page(
            "halleyctl comp dpms",
            &["halleyctl comp dpms off|on|toggle [-o OUTPUT]"],
            &[("off|on|toggle", "Set or toggle output power state")],
        ),
        HelpTopic::Node => print_help_page(
            "halleyctl node",
            &[
                "halleyctl node list [-o OUTPUT] [--json]",
                "halleyctl node info [SELECTOR] [-o OUTPUT] [--json]",
                "halleyctl node focus [SELECTOR] [-o OUTPUT]",
                "halleyctl node move left|right|up|down [SELECTOR] [-o OUTPUT]",
                "halleyctl node close [SELECTOR] [-o OUTPUT]",
            ],
            &[
                ("list", "List nodes"),
                ("info", "Show information about a node"),
                ("focus", "Focus a node by selector"),
                ("move", "Move a node in a direction"),
                ("close", "Close a node"),
            ],
        ),
        HelpTopic::NodeList => print_help_page(
            "halleyctl node list",
            &["halleyctl node list [-o OUTPUT] [--json]"],
            &[("list", "List nodes on the focused or selected output")],
        ),
        HelpTopic::NodeInfo => print_help_page(
            "halleyctl node info",
            &["halleyctl node info [SELECTOR] [-o OUTPUT] [--json]"],
            &[(
                "SELECTOR",
                "Use focused, latest, id:<id>, title:<text>, or app:<app-id>",
            )],
        ),
        HelpTopic::NodeFocus => print_help_page(
            "halleyctl node focus",
            &["halleyctl node focus [SELECTOR] [-o OUTPUT]"],
            &[(
                "SELECTOR",
                "Use focused, latest, id:<id>, title:<text>, or app:<app-id>",
            )],
        ),
        HelpTopic::NodeMove => print_help_page(
            "halleyctl node move",
            &["halleyctl node move left|right|up|down [SELECTOR] [-o OUTPUT]"],
            &[
                ("left|right|up|down", "Direction to move the node"),
                (
                    "SELECTOR",
                    "Use focused, latest, id:<id>, title:<text>, or app:<app-id>",
                ),
            ],
        ),
        HelpTopic::NodeClose => print_help_page(
            "halleyctl node close",
            &["halleyctl node close [SELECTOR] [-o OUTPUT]"],
            &[(
                "SELECTOR",
                "Use focused, latest, id:<id>, title:<text>, or app:<app-id>",
            )],
        ),
        HelpTopic::Trail => print_help_page(
            "halleyctl trail",
            &[
                "halleyctl trail prev [-o OUTPUT]",
                "halleyctl trail next [-o OUTPUT]",
                "halleyctl trail list [-o OUTPUT] [--json]",
                "halleyctl trail goto <TARGET> [-o OUTPUT]",
            ],
            &[
                ("prev", "Focus the previous trail entry"),
                ("next", "Focus the next trail entry"),
                ("list", "List trail entries"),
                ("goto", "Focus a specific trail entry"),
            ],
        ),
        HelpTopic::TrailPrev => print_help_page(
            "halleyctl trail prev",
            &["halleyctl trail prev [-o OUTPUT]"],
            &[("prev", "Focus the previous trail entry")],
        ),
        HelpTopic::TrailNext => print_help_page(
            "halleyctl trail next",
            &["halleyctl trail next [-o OUTPUT]"],
            &[("next", "Focus the next trail entry")],
        ),
        HelpTopic::TrailList => print_help_page(
            "halleyctl trail list",
            &["halleyctl trail list [-o OUTPUT] [--json]"],
            &[(
                "list",
                "List trail entries on the focused or selected output",
            )],
        ),
        HelpTopic::TrailGoto => print_help_page(
            "halleyctl trail goto",
            &["halleyctl trail goto <TARGET> [-o OUTPUT]"],
            &[(
                "TARGET",
                "Use a trail index or the same selectors accepted by node commands",
            )],
        ),
        HelpTopic::Monitor => print_help_page(
            "halleyctl monitor",
            &["halleyctl monitor focus <left|right|up|down|OUTPUT>"],
            &[("focus", "Focus an adjacent monitor or a named output")],
        ),
        HelpTopic::MonitorFocus => print_help_page(
            "halleyctl monitor focus",
            &["halleyctl monitor focus <left|right|up|down|OUTPUT>"],
            &[(
                "left|right|up|down|OUTPUT",
                "Direction or exact output name to focus",
            )],
        ),
        HelpTopic::Bearings => print_help_page(
            "halleyctl bearings",
            &[
                "halleyctl bearings show",
                "halleyctl bearings hide",
                "halleyctl bearings toggle",
                "halleyctl bearings status",
            ],
            &[
                ("show", "Enable bearings"),
                ("hide", "Hide bearings"),
                ("toggle", "Toggle bearings visibility"),
                ("status", "Print current bearings visibility"),
            ],
        ),
        HelpTopic::BearingsShow => print_help_page(
            "halleyctl bearings show",
            &["halleyctl bearings show"],
            &[("show", "Enable bearings")],
        ),
        HelpTopic::BearingsHide => print_help_page(
            "halleyctl bearings hide",
            &["halleyctl bearings hide"],
            &[("hide", "Hide bearings")],
        ),
        HelpTopic::BearingsToggle => print_help_page(
            "halleyctl bearings toggle",
            &["halleyctl bearings toggle"],
            &[("toggle", "Toggle bearings visibility")],
        ),
        HelpTopic::BearingsStatus => print_help_page(
            "halleyctl bearings status",
            &["halleyctl bearings status"],
            &[("status", "Print current bearings visibility")],
        ),
    }
}

fn print_help_page(title: &str, usage: &[&str], commands: &[(&str, &str)]) {
    println!("{title}");
    println!();
    println!("Usage:");
    for line in usage {
        println!("  {line}");
    }
    if !commands.is_empty() {
        println!();
        println!("Commands:");
        for (name, description) in commands {
            println!("  {name:<9} {description}");
        }
    }
    println!();
    println!("Options:");
    println!("  -h, --help  Show help");
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
        Response::BearingsStatus(status) => {
            println!("{}", if status.visible { "visible" } else { "hidden" });
            Ok(())
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
