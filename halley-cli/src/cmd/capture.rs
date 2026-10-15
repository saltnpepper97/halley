use halley_api::{CaptureRect, CaptureTarget, NodeId};

use super::Action;

pub(super) fn parse(args: &[String]) -> Result<Action, String> {
    let Some(mode) = args.first().map(String::as_str) else {
        return Ok(Action::CaptureHelp);
    };
    if matches!(mode, "-h" | "--help") {
        return Ok(Action::CaptureHelp);
    }
    if mode == "window" {
        if args.len() != 2 {
            return Err(
                "capture window requires one node ID from `halleyctl node list`; no output option"
                    .into(),
            );
        }
        let id = args[1].strip_prefix("id:").unwrap_or(&args[1]);
        let node_id = id
            .parse::<u64>()
            .map(NodeId::new)
            .map_err(|_| "capture window requires a numeric node ID".to_string())?;
        return Ok(Action::Capture {
            target: CaptureTarget::Window { node_id },
        });
    }
    if !matches!(mode, "menu" | "screen" | "region") {
        return Err(format!("unknown capture mode {mode:?}"));
    }
    let mut output = None;
    let mut rect = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" if output.is_none() => {
                index += 1;
                let name = args
                    .get(index)
                    .filter(|name| !name.is_empty() && !name.starts_with('-'))
                    .ok_or_else(|| "capture requires a name after --output".to_string())?;
                output = Some(name.clone());
                index += 1;
            }
            "--rect" if mode == "region" && rect.is_none() => {
                let values = args
                    .get(index + 1..index + 5)
                    .ok_or_else(|| {
                        "--rect requires X Y WIDTH HEIGHT in logical pixels".to_string()
                    })?
                    .iter()
                    .map(|v| v.parse::<i32>())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| {
                        "--rect requires four integer coordinates/dimensions".to_string()
                    })?;
                let area = CaptureRect {
                    x: values[0],
                    y: values[1],
                    width: values[2],
                    height: values[3],
                };
                if area.x < 0 || area.y < 0 || area.width <= 0 || area.height <= 0 {
                    return Err("--rect requires nonnegative X/Y and positive WIDTH/HEIGHT".into());
                }
                rect = Some(area);
                index += 5;
            }
            other => return Err(format!("unexpected capture argument {other:?}")),
        }
    }
    if rect.is_some() && output.is_none() {
        return Err("capture region --rect requires --output".into());
    }
    let target = match mode {
        "menu" => CaptureTarget::Menu { output },
        "screen" => CaptureTarget::Screen { output },
        "region" => CaptureTarget::Region { output, rect },
        _ => unreachable!(),
    };
    Ok(Action::Capture { target })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(values: &[&str]) -> Result<Action, String> {
        parse(&values.iter().map(|v| v.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_direct_and_interactive_targets() {
        assert_eq!(
            parse_args(&["screen", "-o", "DP-1"]),
            Ok(Action::Capture {
                target: CaptureTarget::Screen {
                    output: Some("DP-1".into())
                },
            })
        );
        assert_eq!(
            parse_args(&["screen"]),
            Ok(Action::Capture {
                target: CaptureTarget::Screen { output: None },
            })
        );
        assert_eq!(
            parse_args(&["window", "id:42"]),
            Ok(Action::Capture {
                target: CaptureTarget::Window {
                    node_id: NodeId::new(42)
                },
            })
        );
        assert_eq!(
            parse_args(&["window", "42"]),
            parse_args(&["window", "id:42"])
        );
        assert_eq!(
            parse_args(&[
                "region", "--rect", "10", "20", "300", "400", "--output", "DP-2"
            ]),
            Ok(Action::Capture {
                target: CaptureTarget::Region {
                    output: Some("DP-2".into()),
                    rect: Some(CaptureRect {
                        x: 10,
                        y: 20,
                        width: 300,
                        height: 400
                    }),
                },
            })
        );
        assert_eq!(
            parse_args(&["region", "-o", "DP-2"]),
            Ok(Action::Capture {
                target: CaptureTarget::Region {
                    output: Some("DP-2".into()),
                    rect: None
                },
            })
        );
        assert_eq!(
            parse_args(&["menu"]),
            Ok(Action::Capture {
                target: CaptureTarget::Menu { output: None },
            })
        );
    }

    #[test]
    fn rejects_ambiguous_or_invalid_targets() {
        for args in [
            vec!["window"],
            vec!["window", "title:foo"],
            vec!["window", "42", "-o", "DP-1"],
            vec!["screen", "--rect", "0", "0", "10", "10"],
            vec!["region", "--rect", "0", "0", "10", "10"],
            vec!["region", "-o", "DP-1", "--rect", "0", "0", "0", "10"],
            vec!["region", "-o", "DP-1", "--rect", "-1", "0", "10", "10"],
            vec![
                "region",
                "-o",
                "DP-1",
                "--rect",
                "0",
                "0",
                "2147483648",
                "10",
            ],
            vec!["region", "-o", "DP-1", "--rect", "0", "0", "10"],
            vec!["screen", "-o"],
            vec!["screen", "-o", "--rect"],
            vec!["screen", "-o", "DP-1", "-o", "DP-2"],
            vec!["pixels"],
        ] {
            assert!(parse_args(&args).is_err(), "accepted {args:?}");
        }
    }
}
