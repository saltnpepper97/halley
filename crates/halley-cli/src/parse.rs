use halley_ipc::{
    CompositorRequest, DpmsCommand, MonitorFocusDirection, MonitorFocusTarget, NodeMoveDirection,
    NodeSelector, Request, TrailTarget,
};

use crate::cmd::{
    bearings::parse_bearings_request, capture::parse_capture_request,
    cluster::parse_cluster_request, monitor::parse_monitor_request, node::parse_node_request,
    stack::parse_stack_request, tile::parse_tile_request, trail::parse_trail_request,
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
        "capture" => parse_capture_request(&args[1..]),
        "dpms" => parse_dpms(&args[1..]),
        "node" => parse_node_request(&args[1..]),
        "trail" => parse_trail_request(&args[1..]),
        "monitor" => parse_monitor_request(&args[1..]),
        "bearings" => parse_bearings_request(&args[1..]),
        "cluster" => parse_cluster_request(&args[1..]),
        "stack" => parse_stack_request(&args[1..]),
        "tile" => parse_tile_request(&args[1..]),
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

#[cfg(test)]
mod tests {
    use super::{ParseOutcome, parse_request};

    #[test]
    fn stack_cycle_request_parses() {
        let args = vec![
            "stack".to_string(),
            "cycle".to_string(),
            "forward".to_string(),
        ];
        let outcome = match parse_request(&args) {
            Ok(outcome) => outcome,
            Err(err) => panic!("stack request should parse: {}", err.message),
        };

        match outcome {
            ParseOutcome::Request(halley_ipc::Request::Stack(
                halley_ipc::StackRequest::Cycle { direction, output },
            )) => {
                assert_eq!(direction, halley_ipc::StackCycleDirection::Forward);
                assert_eq!(output, None);
            }
            _ => panic!("unexpected parse outcome"),
        }
    }

    #[test]
    fn tile_focus_request_parses() {
        let args = vec!["tile".to_string(), "focus".to_string(), "left".to_string()];
        let outcome = match parse_request(&args) {
            Ok(outcome) => outcome,
            Err(err) => panic!("tile request should parse: {}", err.message),
        };

        match outcome {
            ParseOutcome::Request(halley_ipc::Request::Tile(halley_ipc::TileRequest::Focus {
                direction,
                output,
            })) => {
                assert!(matches!(direction, halley_ipc::NodeMoveDirection::Left));
                assert_eq!(output, None);
            }
            _ => panic!("unexpected parse outcome"),
        }
    }

    #[test]
    fn cluster_layout_cycle_request_parses() {
        let args = vec![
            "cluster".to_string(),
            "layout".to_string(),
            "cycle".to_string(),
        ];
        let outcome = match parse_request(&args) {
            Ok(outcome) => outcome,
            Err(err) => panic!("cluster request should parse: {}", err.message),
        };

        match outcome {
            ParseOutcome::Request(halley_ipc::Request::Cluster(
                halley_ipc::ClusterRequest::LayoutCycle { output },
            )) => {
                assert_eq!(output, None);
            }
            _ => panic!("unexpected parse outcome"),
        }
    }

    #[test]
    fn cluster_slot_request_parses() {
        let args = vec![
            "cluster".to_string(),
            "slot".to_string(),
            "10".to_string(),
            "-o".to_string(),
            "DP-1".to_string(),
        ];
        let outcome = match parse_request(&args) {
            Ok(outcome) => outcome,
            Err(err) => panic!("cluster slot request should parse: {}", err.message),
        };

        match outcome {
            ParseOutcome::Request(halley_ipc::Request::Cluster(
                halley_ipc::ClusterRequest::Slot { slot, output },
            )) => {
                assert_eq!(slot, 10);
                assert_eq!(output.as_deref(), Some("DP-1"));
            }
            _ => panic!("unexpected parse outcome"),
        }
    }

    #[test]
    fn cluster_list_request_parses() {
        let args = vec!["cluster".to_string(), "list".to_string()];
        let outcome = match parse_request(&args) {
            Ok(outcome) => outcome,
            Err(err) => panic!("cluster list request should parse: {}", err.message),
        };

        match outcome {
            ParseOutcome::Request(halley_ipc::Request::Cluster(
                halley_ipc::ClusterRequest::List { output },
            )) => {
                assert_eq!(output, None);
            }
            _ => panic!("unexpected parse outcome"),
        }
    }

    #[test]
    fn cluster_inspect_request_parses_default_current() {
        let args = vec!["cluster".to_string(), "inspect".to_string()];
        let outcome = match parse_request(&args) {
            Ok(outcome) => outcome,
            Err(err) => panic!("cluster inspect request should parse: {}", err.message),
        };

        match outcome {
            ParseOutcome::Request(halley_ipc::Request::Cluster(
                halley_ipc::ClusterRequest::Inspect { target, output },
            )) => {
                assert!(target.is_none());
                assert_eq!(output, None);
            }
            _ => panic!("unexpected parse outcome"),
        }
    }

    #[test]
    fn cluster_inspect_request_parses_id_target() {
        let args = vec![
            "cluster".to_string(),
            "inspect".to_string(),
            "2".to_string(),
        ];
        let outcome = match parse_request(&args) {
            Ok(outcome) => outcome,
            Err(err) => panic!("cluster inspect id request should parse: {}", err.message),
        };

        match outcome {
            ParseOutcome::Request(halley_ipc::Request::Cluster(
                halley_ipc::ClusterRequest::Inspect { target, output },
            )) => {
                assert!(matches!(target, Some(halley_ipc::ClusterTarget::Id(2))));
                assert_eq!(output, None);
            }
            _ => panic!("unexpected parse outcome"),
        }
    }
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
