use halley_ipc::{Request, StackCycleDirection, StackRequest};

use crate::help::HelpTopic;
use crate::parse::{ParseOutcome, UsageError, contains_help_flag, parse_output_option};

pub(crate) fn parse_stack_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => Ok(ParseOutcome::Help(HelpTopic::Stack)),
        Some("cycle") => parse_stack_cycle(&args[1..]),
        Some(other) => Err(UsageError::new(
            format!("unknown stack command: {other}"),
            HelpTopic::Stack,
        )),
    }
}

fn parse_stack_cycle(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if args.is_empty() || contains_help_flag(args) {
        return Ok(ParseOutcome::Help(HelpTopic::StackCycle));
    }

    let direction = match args[0].as_str() {
        "forward" => StackCycleDirection::Forward,
        "backward" => StackCycleDirection::Backward,
        other => {
            return Err(UsageError::new(
                format!("unknown stack cycle direction: {other}"),
                HelpTopic::StackCycle,
            ));
        }
    };
    let output = parse_output_option(&args[1..], HelpTopic::StackCycle)?;
    Ok(ParseOutcome::Request(Request::Stack(StackRequest::Cycle {
        direction,
        output,
    })))
}
