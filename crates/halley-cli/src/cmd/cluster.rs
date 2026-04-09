use halley_ipc::{ClusterRequest, ClusterTarget, Request};

use crate::help::HelpTopic;
use crate::parse::{ParseOutcome, UsageError, contains_help_flag, parse_output_option};

pub(crate) fn parse_cluster_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => Ok(ParseOutcome::Help(HelpTopic::Cluster)),
        Some("list") => parse_cluster_list(&args[1..]),
        Some("inspect") => parse_cluster_inspect(&args[1..]),
        Some("layout") => parse_cluster_layout_request(&args[1..]),
        Some(other) => Err(UsageError::new(
            format!("unknown cluster command: {other}"),
            HelpTopic::Cluster,
        )),
    }
}

fn parse_cluster_list(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if contains_help_flag(args) {
        return Ok(ParseOutcome::Help(HelpTopic::ClusterList));
    }
    let output = parse_output_option(args, HelpTopic::ClusterList)?;
    Ok(ParseOutcome::Request(Request::Cluster(
        ClusterRequest::List { output },
    )))
}

fn parse_cluster_inspect(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if contains_help_flag(args) {
        return Ok(ParseOutcome::Help(HelpTopic::ClusterInspect));
    }

    let mut target = None;
    let mut output = None;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(UsageError::new(
                        "missing value for -o/--output",
                        HelpTopic::ClusterInspect,
                    ));
                };
                output = Some(value.clone());
            }
            "--json" => {}
            other if other.starts_with('-') => {
                return Err(UsageError::new(
                    format!("unknown option for cluster inspect: {other}"),
                    HelpTopic::ClusterInspect,
                ));
            }
            other => {
                if target.is_some() {
                    return Err(UsageError::new(
                        format!("unexpected extra cluster target: {other}"),
                        HelpTopic::ClusterInspect,
                    ));
                }
                target = Some(parse_cluster_target(other)?);
            }
        }
        index += 1;
    }

    Ok(ParseOutcome::Request(Request::Cluster(
        ClusterRequest::Inspect { target, output },
    )))
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

fn parse_cluster_target(text: &str) -> Result<ClusterTarget, UsageError> {
    if text.eq_ignore_ascii_case("current") {
        return Ok(ClusterTarget::Current);
    }
    text.parse::<u64>().map(ClusterTarget::Id).map_err(|_| {
        UsageError::new(
            format!("unknown cluster target: {text}"),
            HelpTopic::ClusterInspect,
        )
    })
}
