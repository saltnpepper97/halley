mod capture;
mod cluster;
mod control;
mod node;
pub mod portal;
mod trail;

use std::path::PathBuf;

use halley_api::DpmsCommand;
use halley_api::{CaptureMode, Direction, MonitorTarget, StackCycleDirection};

pub use cluster::ClusterCommand;
pub use node::NodeCommand;
pub use portal::PortalCommand;
pub use trail::TrailCommand;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BearingsAction {
    Show,
    Hide,
    Toggle,
    Status,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Outputs,
    Reload,
    Capture {
        mode: CaptureMode,
        output: Option<String>,
    },
    CaptureHelp,
    Dpms {
        command: DpmsCommand,
        output: Option<String>,
    },
    DpmsHelp,
    Node {
        request: NodeCommand,
        output: NodeOutput,
    },
    NodeHelp,
    Cluster {
        request: ClusterCommand,
        output: ClusterOutput,
    },
    ClusterHelp,
    Bearings(BearingsAction),
    BearingsHelp,
    Trail {
        request: TrailCommand,
        output: TrailOutput,
    },
    TrailHelp,
    MonitorFocus(MonitorTarget),
    WindowTransfer(halley_api::Direction),
    PanField(halley_api::Direction),
    PanHelp,
    MonitorHelp,
    StackCycle {
        direction: StackCycleDirection,
        output: Option<String>,
    },
    StackHelp,
    Tile {
        direction: Direction,
        output: Option<String>,
        swap: bool,
    },
    TileHelp,
    Portal {
        command: PortalCommand,
        json: bool,
    },
    PortalHelp,
    ConfigEdit(Option<PathBuf>),
    ConfigMigrate {
        path: Option<PathBuf>,
        dry_run: bool,
    },
    ConfigVerify(Option<PathBuf>),
    ConfigHelp,
    Basics,
    BasicsHelp,
    Quit,
    Version,
    Help,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeOutput {
    List { json: bool },
    Info { json: bool },
    Ack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClusterOutput {
    List { json: bool },
    Info { json: bool },
    Ack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrailOutput {
    List { json: bool },
    Ack,
}

pub fn parse(args: &[String]) -> Result<Action, String> {
    let action = match args.first().map(String::as_str) {
        None | Some("--help") | Some("-h") => Action::Help,
        Some("--version") | Some("-V") => Action::Version,
        Some("outputs") => {
            if let Some(unexpected) = args.get(1) {
                return Err(format!("unexpected argument {unexpected:?}"));
            }
            Action::Outputs
        }
        Some("reload") => Action::Reload,
        Some("capture") => return capture::parse(&args[1..]),
        Some("dpms") => return parse_dpms(&args[1..]),
        Some("node") => return node::parse(&args[1..]),
        Some("cluster") => return cluster::parse(&args[1..]),
        Some("bearings") => return parse_bearings(&args[1..]),
        Some("trail") => return trail::parse(&args[1..]),
        Some("pan") => return control::parse_pan(&args[1..]),
        Some("monitor") => return control::parse_monitor(&args[1..]),
        Some("stack") => return control::parse_stack(&args[1..]),
        Some("tile") => return control::parse_tile(&args[1..]),
        Some("portal") => return portal::parse(&args[1..]),
        Some("config") => return parse_config(&args[1..]),
        Some("basics") => {
            if args
                .iter()
                .skip(1)
                .any(|arg| arg == "-h" || arg == "--help")
            {
                return Ok(Action::BasicsHelp);
            }
            if let Some(unexpected) = args.get(1) {
                return Err(format!("unexpected argument {unexpected:?}"));
            }
            Action::Basics
        }
        Some("quit") => Action::Quit,
        Some(other) => return Err(format!("unknown command {other:?}")),
    };

    if let Some(unexpected) = args.get(1) {
        return Err(format!("unexpected argument {unexpected:?}"));
    }
    Ok(action)
}

fn parse_output_option(args: &[String], command: &str) -> Result<Option<String>, String> {
    let mut output = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let name = args
                    .get(index)
                    .ok_or_else(|| format!("{command} output option requires a connector name"))?;
                if output.replace(name.clone()).is_some() {
                    return Err(format!(
                        "{command} output option was specified more than once"
                    ));
                }
            }
            other => return Err(format!("unexpected {command} argument {other:?}")),
        }
        index += 1;
    }
    Ok(output)
}

fn parse_dpms(args: &[String]) -> Result<Action, String> {
    if args.is_empty() || args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(Action::DpmsHelp);
    }

    let mut command = None;
    let mut output = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "dpms output option requires a connector name".to_string())?;
                if output.replace(value.clone()).is_some() {
                    return Err("dpms output option was specified more than once".to_string());
                }
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown dpms option {value:?}"));
            }
            value if command.is_none() => command = Some(value),
            value => return Err(format!("unexpected dpms argument {value:?}")),
        }
        index += 1;
    }

    let command = match command {
        Some("off") => DpmsCommand::Off,
        Some("on") => DpmsCommand::On,
        Some("toggle") => DpmsCommand::Toggle,
        Some(other) => return Err(format!("unknown dpms command {other:?}")),
        None => return Ok(Action::DpmsHelp),
    };
    Ok(Action::Dpms { command, output })
}

fn parse_config(args: &[String]) -> Result<Action, String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(Action::ConfigHelp);
    };
    if command == "-h" || command == "--help" {
        return Ok(Action::ConfigHelp);
    }
    if command != "edit" && command != "verify" && command != "migrate" {
        return Err(format!("unknown config command {command:?}"));
    }
    let mut path = None;
    let mut dry_run = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "-h" | "--help" => return Ok(Action::ConfigHelp),
            "--dry-run" if command == "migrate" => {
                if dry_run {
                    return Err("config migrate --dry-run was specified more than once".to_string());
                }
                dry_run = true;
            }
            "-c" | "--config" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("config {command} path option requires a path"))?;
                set_config_path(&mut path, value, command)?;
            }
            value if value.starts_with("--config=") => {
                set_config_path(&mut path, &value["--config=".len()..], command)?;
            }
            value => return Err(format!("unexpected config {command} argument {value:?}")),
        }
        index += 1;
    }
    Ok(match command {
        "edit" => Action::ConfigEdit(path),
        "migrate" => Action::ConfigMigrate { path, dry_run },
        "verify" => Action::ConfigVerify(path),
        _ => unreachable!("config command checked above"),
    })
}

fn set_config_path(path: &mut Option<PathBuf>, value: &str, command: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("config {command} path cannot be empty"));
    }
    if path.replace(PathBuf::from(value)).is_some() {
        return Err(format!(
            "config {command} path was specified more than once"
        ));
    }
    Ok(())
}

fn parse_bearings(args: &[String]) -> Result<Action, String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(Action::BearingsHelp);
    };
    if command == "-h" || command == "--help" {
        return Ok(Action::BearingsHelp);
    }
    if let Some(unexpected) = args.get(1) {
        return Err(format!("unexpected argument {unexpected:?}"));
    }
    let request = match command {
        "show" => BearingsAction::Show,
        "hide" => BearingsAction::Hide,
        "toggle" => BearingsAction::Toggle,
        "status" => BearingsAction::Status,
        other => return Err(format!("unknown bearings command {other:?}")),
    };
    Ok(Action::Bearings(request))
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_api::{NodeMoveDirection, NodeSelector};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parser_accepts_top_level_commands() {
        assert_eq!(parse(&[]), Ok(Action::Help));
        assert_eq!(parse(&args(&["outputs"])), Ok(Action::Outputs));
        assert_eq!(parse(&args(&["--help"])), Ok(Action::Help));
        assert_eq!(parse(&args(&["--version"])), Ok(Action::Version));
        assert_eq!(parse(&args(&["quit"])), Ok(Action::Quit));
        assert!(parse(&args(&["output"])).is_err());
        assert!(parse(&args(&["outputs", "extra"])).is_err());
    }

    #[test]
    fn parser_covers_bearings_dpms_and_config() {
        assert_eq!(
            parse(&args(&["bearings", "show"])),
            Ok(Action::Bearings(BearingsAction::Show))
        );
        assert_eq!(
            parse(&args(&["dpms", "-o", "DP-1", "on"])),
            Ok(Action::Dpms {
                command: DpmsCommand::On,
                output: Some("DP-1".to_string()),
            })
        );
        assert_eq!(
            parse(&args(&["config", "verify", "--config=/tmp/two.rune"])),
            Ok(Action::ConfigVerify(Some("/tmp/two.rune".into())))
        );
        assert_eq!(
            parse(&args(&["config", "edit"])),
            Ok(Action::ConfigEdit(None))
        );
        assert_eq!(
            parse(&args(&["config", "edit", "-c", "/tmp/two.rune"])),
            Ok(Action::ConfigEdit(Some("/tmp/two.rune".into())))
        );
        assert_eq!(
            parse(&args(&[
                "config",
                "migrate",
                "--dry-run",
                "-c",
                "/tmp/two.rune",
            ])),
            Ok(Action::ConfigMigrate {
                path: Some("/tmp/two.rune".into()),
                dry_run: true,
            })
        );
        assert!(parse(&args(&["dpms", "sleep"])).is_err());
        assert!(parse(&args(&["config", "verify", "-c"])).is_err());
        assert!(parse(&args(&["config", "edit", "--config="])).is_err());
        assert!(parse(&args(&["config", "verify", "--dry-run"])).is_err());
    }

    #[test]
    fn parser_covers_node_state_controls() {
        assert_eq!(
            parse(&args(&["node", "list", "-o", "DP-1", "--json"])),
            Ok(Action::Node {
                request: NodeCommand::List {
                    output: Some("DP-1".into())
                },
                output: NodeOutput::List { json: true },
            })
        );
        assert_eq!(
            parse(&args(&["node", "collapse", "focused"])),
            Ok(Action::Node {
                request: NodeCommand::Collapse {
                    selector: Some(NodeSelector::Focused),
                    output: None,
                },
                output: NodeOutput::Ack,
            })
        );
        assert_eq!(
            parse(&args(&["node", "move", "left", "app:firefox"])),
            Ok(Action::Node {
                request: NodeCommand::Move {
                    direction: NodeMoveDirection::Left,
                    selector: Some(NodeSelector::App("firefox".into())),
                    output: None,
                },
                output: NodeOutput::Ack,
            })
        );
        assert!(parse(&args(&["node", "collapse", "--json"])).is_err());
    }
}
