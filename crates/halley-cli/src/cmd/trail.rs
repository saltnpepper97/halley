use halley_ipc::{Request, TrailRequest};

use crate::help::HelpTopic;
use crate::parse::{
    ParseOutcome, UsageError, contains_help_flag, parse_output_option, parse_trail_target,
};

pub(crate) fn parse_trail_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
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
