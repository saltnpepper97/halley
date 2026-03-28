use halley_ipc::{MonitorRequest, Request};

use crate::help::HelpTopic;
use crate::parse::{ParseOutcome, UsageError, contains_help_flag, parse_monitor_focus_target};

pub(crate) fn parse_monitor_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
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
