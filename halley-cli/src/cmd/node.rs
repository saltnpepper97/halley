use halley_api::{NodeId, NodeMoveDirection, NodeSelector};

use super::{Action, NodeOutput};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeCommand {
    List {
        output: Option<String>,
    },
    Info {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    Focus {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    Collapse {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    Restore {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    Toggle {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    Close {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    MoveTo {
        x: i32,
        y: i32,
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    Move {
        direction: NodeMoveDirection,
        distance: Option<u32>,
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
}

pub(super) fn parse(args: &[String]) -> Result<Action, String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(Action::NodeHelp);
    };
    if command == "-h"
        || command == "--help"
        || args[1..].iter().any(|arg| arg == "-h" || arg == "--help")
    {
        return Ok(Action::NodeHelp);
    }
    match command {
        "list" => {
            let (selector, output, json) = parse_flags(&args[1..], false)?;
            debug_assert!(selector.is_none());
            Ok(Action::Node {
                request: NodeCommand::List { output },
                output: NodeOutput::List { json },
            })
        }
        "info" | "focus" | "collapse" | "restore" | "toggle" | "close" => {
            let (selector, output, json) = parse_flags(&args[1..], true)?;
            if json && command != "info" {
                return Err("--json is supported only by node list and node info".to_string());
            }
            let request = match command {
                "info" => NodeCommand::Info { selector, output },
                "focus" => NodeCommand::Focus { selector, output },
                "collapse" => NodeCommand::Collapse { selector, output },
                "restore" => NodeCommand::Restore { selector, output },
                "toggle" => NodeCommand::Toggle { selector, output },
                "close" => NodeCommand::Close { selector, output },
                _ => unreachable!(),
            };
            Ok(Action::Node {
                request,
                output: if command == "info" {
                    NodeOutput::Info { json }
                } else {
                    NodeOutput::Ack
                },
            })
        }
        "move" => {
            if args.get(1).map(String::as_str) == Some("to") {
                let coordinate = |index: usize| -> Result<i32, String> {
                    args.get(index)
                        .and_then(|value| value.parse().ok())
                        .ok_or_else(|| {
                            "node move to requires X Y as signed 32-bit whole Field coordinates"
                                .into()
                        })
                };
                let x = coordinate(2)?;
                let y = coordinate(3)?;
                let (selector, output, json) = parse_flags(&args[4..], true)?;
                if json {
                    return Err("--json is supported only by node list and node info".into());
                }
                return Ok(Action::Node {
                    request: NodeCommand::MoveTo {
                        x,
                        y,
                        selector,
                        output,
                    },
                    output: NodeOutput::Ack,
                });
            }
            let direction = match args.get(1).map(String::as_str) {
                Some("left") => NodeMoveDirection::Left,
                Some("right") => NodeMoveDirection::Right,
                Some("up") => NodeMoveDirection::Up,
                Some("down") => NodeMoveDirection::Down,
                Some(other) => return Err(format!("unknown node move direction {other:?}")),
                None => {
                    return Err("node move requires left, right, up, down, or to X Y".to_string());
                }
            };
            let mut distance = None;
            let mut remaining = Vec::new();
            let mut index = 2;
            while index < args.len() {
                let arg = &args[index];
                if arg == "--distance" || arg.starts_with("--distance=") {
                    let value = if let Some(value) = arg.strip_prefix("--distance=") {
                        value
                    } else {
                        index += 1;
                        args.get(index)
                            .ok_or("--distance requires a positive whole number")?
                    };
                    let amount = value.parse::<i32>().ok().filter(|n| *n > 0).ok_or(
                        "movement distance must be a whole number in 1..=2147483647 Field units",
                    )?;
                    if distance.replace(amount as u32).is_some() {
                        return Err("--distance was specified more than once".into());
                    }
                } else {
                    // Preserve option values literally, even when an output
                    // name happens to look like a movement option.
                    remaining.push(arg.clone());
                    if matches!(arg.as_str(), "-o" | "--output") {
                        index += 1;
                        remaining.push(
                            args.get(index)
                                .ok_or("node output option requires a connector name")?
                                .clone(),
                        );
                    }
                }
                index += 1;
            }
            let (selector, output, json) = parse_flags(&remaining, true)?;
            if json {
                return Err("--json is supported only by node list and node info".to_string());
            }
            Ok(Action::Node {
                request: NodeCommand::Move {
                    direction,
                    distance,
                    selector,
                    output,
                },
                output: NodeOutput::Ack,
            })
        }
        other => Err(format!("unknown node command {other:?}")),
    }
}

fn parse_flags(
    args: &[String],
    allow_selector: bool,
) -> Result<(Option<NodeSelector>, Option<String>, bool), String> {
    let mut selector = None;
    let mut output = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => json = true,
            "-o" | "--output" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "node output option requires a connector name".to_string())?;
                if output.replace(value.clone()).is_some() {
                    return Err("node output option was specified more than once".to_string());
                }
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown node option {value:?}"));
            }
            value if allow_selector && selector.is_none() => {
                selector = Some(parse_selector(value)?);
            }
            value if !allow_selector => return Err(format!("unexpected argument {value:?}")),
            value => return Err(format!("unexpected extra selector {value:?}")),
        }
        index += 1;
    }
    Ok((selector, output, json))
}

pub(super) fn parse_selector(value: &str) -> Result<NodeSelector, String> {
    match value {
        _ if value.eq_ignore_ascii_case("focused") => Ok(NodeSelector::Focused),
        _ if value.eq_ignore_ascii_case("latest") => Ok(NodeSelector::Latest),
        _ if value.parse::<u64>().is_ok() => Ok(NodeSelector::Id(NodeId::new(
            value.parse().expect("checked numeric selector"),
        ))),
        _ if value.starts_with("id:") => value[3..]
            .parse::<u64>()
            .map(NodeId::new)
            .map(NodeSelector::Id)
            .map_err(|_| format!("invalid node id selector {value:?}")),
        _ if value.starts_with("title:") => Ok(NodeSelector::Title(value[6..].to_string())),
        _ if value.starts_with("app:") => Ok(NodeSelector::App(value[4..].to_string())),
        _ => Err(format!("invalid node selector {value:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(input: &str) -> Vec<String> {
        input.split_whitespace().map(str::to_owned).collect()
    }
    #[test]
    fn exact_movement_accepts_negative_coordinates_and_existing_selectors() {
        assert_eq!(
            parse(&args("move to -300 200 app:firefox -o DP-2")).unwrap(),
            Action::Node {
                request: NodeCommand::MoveTo {
                    x: -300,
                    y: 200,
                    selector: Some(NodeSelector::App("firefox".into())),
                    output: Some("DP-2".into())
                },
                output: NodeOutput::Ack
            }
        );
        assert!(matches!(
            parse(&args("move to 0 0")).unwrap(),
            Action::Node {
                request: NodeCommand::MoveTo { selector: None, .. },
                ..
            }
        ));
        assert!(matches!(
            parse(&args("move right 17 --distance 200")).unwrap(),
            Action::Node {
                request: NodeCommand::Move {
                    distance: Some(200),
                    selector: Some(NodeSelector::Id(_)),
                    ..
                },
                ..
            }
        ));
    }
    #[test]
    fn exact_movement_rejects_incomplete_or_ambiguous_arguments() {
        for input in [
            "move to",
            "move to 1",
            "move to 1.5 2",
            "move to 2147483648 0",
            "move to 0 0 --distance 10",
            "move to 0 0 17 18",
            "move to 0 0 --json",
        ] {
            assert!(parse(&args(input)).is_err(), "{input}");
        }
    }
}
