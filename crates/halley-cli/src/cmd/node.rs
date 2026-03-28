use halley_ipc::{NodeRequest, Request};

use crate::help::HelpTopic;
use crate::parse::{
    ParseOutcome, UsageError, contains_help_flag, parse_move_direction, parse_output_option,
    parse_selector_flags,
};

pub(crate) fn parse_node_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
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
