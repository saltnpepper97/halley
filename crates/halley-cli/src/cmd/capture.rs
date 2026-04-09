use halley_ipc::{CaptureMode, CaptureRequest, Request};

use crate::help::HelpTopic;
use crate::parse::{ParseOutcome, UsageError};

pub(crate) fn parse_capture_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if args.is_empty()
        || args
            .iter()
            .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        return Ok(ParseOutcome::Help(HelpTopic::Capture));
    }
    let mode = match args[0].as_str() {
        "menu" => CaptureMode::Menu,
        "region" => CaptureMode::Region,
        "screen" => CaptureMode::Screen,
        "window" => CaptureMode::Window,
        other => {
            return Err(UsageError::new(
                format!("unknown capture mode: {other}"),
                HelpTopic::Capture,
            ));
        }
    };
    let mut output = None;
    let mut idx = 1usize;
    while idx < args.len() {
        match args[idx].as_str() {
            "-o" | "--output" => {
                idx += 1;
                let Some(name) = args.get(idx) else {
                    return Err(UsageError::new(
                        "missing output name after -o/--output",
                        HelpTopic::Capture,
                    ));
                };
                output = Some(name.clone());
            }
            other => {
                return Err(UsageError::new(
                    format!("unexpected argument: {other}"),
                    HelpTopic::Capture,
                ));
            }
        }
        idx += 1;
    }
    Ok(ParseOutcome::Request(Request::Capture(
        CaptureRequest::Start { mode, output },
    )))
}
