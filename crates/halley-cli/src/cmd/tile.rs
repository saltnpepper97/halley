use halley_ipc::{Request, TileRequest};

use crate::help::HelpTopic;
use crate::parse::{
    ParseOutcome, UsageError, contains_help_flag, parse_move_direction, parse_output_option,
};

pub(crate) fn parse_tile_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => Ok(ParseOutcome::Help(HelpTopic::Tile)),
        Some("focus") => parse_tile_directional(&args[1..], HelpTopic::TileFocus, false),
        Some("swap") => parse_tile_directional(&args[1..], HelpTopic::TileSwap, true),
        Some(other) => Err(UsageError::new(
            format!("unknown tile command: {other}"),
            HelpTopic::Tile,
        )),
    }
}

fn parse_tile_directional(
    args: &[String],
    help: HelpTopic,
    swap: bool,
) -> Result<ParseOutcome, UsageError> {
    if args.is_empty() || contains_help_flag(args) {
        return Ok(ParseOutcome::Help(help));
    }
    let direction = parse_move_direction(&args[0])?;
    let output = parse_output_option(&args[1..], help)?;
    let request = if swap {
        TileRequest::Swap { direction, output }
    } else {
        TileRequest::Focus { direction, output }
    };
    Ok(ParseOutcome::Request(Request::Tile(request)))
}
