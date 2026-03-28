use halley_ipc::{
    CompositorRequest, DpmsCommand, MonitorFocusDirection, MonitorFocusTarget, NodeMoveDirection,
    NodeSelector, Request, TrailTarget,
};

use crate::cmd::{
    bearings::parse_bearings_request, monitor::parse_monitor_request, node::parse_node_request,
    trail::parse_trail_request,
};
use crate::help::HelpTopic;

pub(crate) enum ParseOutcome {
    Request(Request),
    Help(HelpTopic),
}

pub(crate) struct UsageError {
    pub(crate) message: String,
    pub(crate) help: HelpTopic,
}

impl UsageError {
    pub(crate) fn new(message: impl Into<String>, help: HelpTopic) -> Self {
        Self {
            message: message.into(),
            help,
        }
    }
}

pub(crate) fn parse_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if args.is_empty() {
        return Ok(ParseOutcome::Help(HelpTopic::Top));
    }

    match args[0].as_str() {
        "help" | "--help" | "-h" => Ok(ParseOutcome::Help(HelpTopic::Top)),
        "quit" => parse_leaf_command(
            &args[1..],
            HelpTopic::Quit,
            Request::Compositor(CompositorRequest::Quit),
        ),
        "reload" => parse_leaf_command(
            &args[1..],
            HelpTopic::Reload,
            Request::Compositor(CompositorRequest::Reload),
        ),
        "outputs" => parse_leaf_command(
            &args[1..],
            HelpTopic::Outputs,
            Request::Compositor(CompositorRequest::Outputs),
        ),
        "dpms" => parse_dpms(&args[1..]),
        "node" => parse_node_request(&args[1..]),
        "trail" => parse_trail_request(&args[1..]),
        "monitor" => parse_monitor_request(&args[1..]),
        "bearings" => parse_bearings_request(&args[1..]),
        other => Err(UsageError::new(
            format!("unknown command: {other}"),
            HelpTopic::Top,
        )),
    }
}

pub(crate) fn parse_leaf_command(
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

fn parse_dpms(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if args.is_empty() || contains_help_flag(args) {
        return Ok(ParseOutcome::Help(HelpTopic::Dpms));
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
                        HelpTopic::Dpms,
                    ));
                };
                output = Some(value.clone());
            }
            other if other.starts_with('-') => {
                return Err(UsageError::new(
                    format!("unknown option for dpms: {other}"),
                    HelpTopic::Dpms,
                ));
            }
            _ => positionals.push(args[index].clone()),
        }
        index += 1;
    }

    let Some(command) = positionals.first() else {
        return Ok(ParseOutcome::Help(HelpTopic::Dpms));
    };
    if positionals.len() > 1 {
        return Err(UsageError::new(
            format!("unexpected argument: {}", positionals[1]),
            HelpTopic::Dpms,
        ));
    }
    let command = match command.as_str() {
        "off" => DpmsCommand::Off,
        "on" => DpmsCommand::On,
        "toggle" => DpmsCommand::Toggle,
        other => {
            return Err(UsageError::new(
                format!("unknown dpms command: {other}"),
                HelpTopic::Dpms,
            ));
        }
    };

    Ok(ParseOutcome::Request(Request::Compositor(
        CompositorRequest::Dpms { command, output },
    )))
}

pub(crate) fn parse_output_option(
    args: &[String],
    help: HelpTopic,
) -> Result<Option<String>, UsageError> {
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

pub(crate) fn parse_selector_flags(
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

pub(crate) fn parse_node_selector(text: &str) -> Result<NodeSelector, UsageError> {
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

pub(crate) fn parse_trail_target(text: &str) -> Result<TrailTarget, UsageError> {
    if let Ok(index) = text.parse::<usize>() {
        return Ok(TrailTarget::Index(index));
    }
    Ok(TrailTarget::Selector(parse_node_selector(text)?))
}

pub(crate) fn parse_move_direction(text: &str) -> Result<NodeMoveDirection, UsageError> {
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

pub(crate) fn parse_monitor_focus_target(text: &str) -> MonitorFocusTarget {
    match text {
        "left" => MonitorFocusTarget::Direction(MonitorFocusDirection::Left),
        "right" => MonitorFocusTarget::Direction(MonitorFocusDirection::Right),
        "up" => MonitorFocusTarget::Direction(MonitorFocusDirection::Up),
        "down" => MonitorFocusTarget::Direction(MonitorFocusDirection::Down),
        other => MonitorFocusTarget::Output(other.to_string()),
    }
}

pub(crate) fn contains_help_flag(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "-h" || arg == "--help")
}
