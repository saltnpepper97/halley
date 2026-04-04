use halley_ipc::{ClusterRequest, Request};

use crate::help::HelpTopic;
use crate::parse::{ParseOutcome, UsageError, contains_help_flag, parse_output_option};

pub(crate) fn parse_cluster_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => Ok(ParseOutcome::Help(HelpTopic::Cluster)),
        Some("layout") => parse_cluster_layout_request(&args[1..]),
        Some(other) => Err(UsageError::new(
            format!("unknown cluster command: {other}"),
            HelpTopic::Cluster,
        )),
    }
}

fn parse_cluster_layout_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => Ok(ParseOutcome::Help(HelpTopic::ClusterLayout)),
        Some("cycle") => parse_cluster_layout_cycle(&args[1..]),
        Some(other) => Err(UsageError::new(
            format!("unknown cluster layout command: {other}"),
            HelpTopic::ClusterLayout,
        )),
    }
}

fn parse_cluster_layout_cycle(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if contains_help_flag(args) {
        return Ok(ParseOutcome::Help(HelpTopic::ClusterLayoutCycle));
    }
    let output = parse_output_option(args, HelpTopic::ClusterLayoutCycle)?;
    Ok(ParseOutcome::Request(Request::Cluster(
        ClusterRequest::LayoutCycle { output },
    )))
}
