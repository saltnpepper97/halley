use halley_ipc::{BearingsRequest, Request};

use crate::help::HelpTopic;
use crate::parse::{ParseOutcome, UsageError, parse_leaf_command};

pub(crate) fn parse_bearings_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
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
