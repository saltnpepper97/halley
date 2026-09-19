mod cmd;
mod config;
mod help;
mod print;

use std::process::ExitCode;

use cmd::{Action, BearingsAction, ClusterCommand, NodeCommand, TrailCommand};
use halley_api::{BearingsCommand, CaptureOutcome, Client};

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match cmd::parse(&args) {
        Ok(Action::Outputs) => with_client(|client| Ok(print::outputs(client.outputs()?))),
        Ok(Action::Reload) => with_client(|client| {
            client.reload_config()?;
            Ok(ExitCode::SUCCESS)
        }),
        Ok(Action::Basics) => with_client(|client| {
            client.show_basics()?;
            Ok(ExitCode::SUCCESS)
        }),
        Ok(Action::BasicsHelp) => show(help::BASICS_HELP),
        Ok(Action::Capture { mode, output }) => with_client(|client| {
            match client.capture(mode, output.as_deref())? {
                CaptureOutcome::Saved(path) => println!("saved: {}", path.display()),
                CaptureOutcome::Cancelled => println!("cancelled"),
            }
            Ok(ExitCode::SUCCESS)
        }),
        Ok(Action::CaptureHelp) => show(help::CAPTURE_HELP),
        Ok(Action::Dpms { command, output }) => with_client(|client| {
            client.set_dpms(command, output.as_deref())?;
            Ok(ExitCode::SUCCESS)
        }),
        Ok(Action::DpmsHelp) => show(help::DPMS_HELP),
        Ok(Action::Node {
            request,
            output: presentation,
        }) => with_client(|client| {
            match request {
                NodeCommand::List { output } => {
                    return Ok(print::node_list(
                        client.nodes(output.as_deref())?,
                        presentation,
                    ));
                }
                NodeCommand::Info { selector, output } => {
                    return Ok(print::node_info(
                        client.node_info(selector, output.as_deref())?,
                        presentation,
                    ));
                }
                NodeCommand::Focus { selector, output } => {
                    client.focus_node(selector, output.as_deref())?;
                }
                NodeCommand::Collapse { selector, output } => {
                    client.collapse_node(selector, output.as_deref())?;
                }
                NodeCommand::Restore { selector, output } => {
                    client.restore_node(selector, output.as_deref())?;
                }
                NodeCommand::Toggle { selector, output } => {
                    client.toggle_node(selector, output.as_deref())?;
                }
                NodeCommand::Close { selector, output } => {
                    client.close_node(selector, output.as_deref())?;
                }
                NodeCommand::Move {
                    direction,
                    selector,
                    output,
                } => client.move_node(direction, selector, output.as_deref())?,
            }
            Ok(ExitCode::SUCCESS)
        }),
        Ok(Action::NodeHelp) => show(help::NODE_HELP),
        Ok(Action::Cluster { request, output }) => with_client(|client| match request {
            ClusterCommand::List { output: connector } => Ok(print::cluster_list(
                client.clusters(connector.as_deref())?,
                output,
            )),
            ClusterCommand::Inspect {
                target,
                output: connector,
            } => Ok(print::cluster_info(
                client.cluster_info(target, connector.as_deref())?,
                output,
            )),
            ClusterCommand::LayoutCycle { output } => {
                client.cycle_cluster_layout(output.as_deref())?;
                Ok(ExitCode::SUCCESS)
            }
            ClusterCommand::Slot { slot, output } => {
                client.activate_cluster_slot(slot, output.as_deref())?;
                Ok(ExitCode::SUCCESS)
            }
        }),
        Ok(Action::ClusterHelp) => show(help::CLUSTER_HELP),
        Ok(Action::Bearings(action)) => with_client(|client| match action {
            BearingsAction::Status => Ok(print::bearings(client.bearings_visible()?)),
            BearingsAction::Show => {
                client.set_bearings(BearingsCommand::Show)?;
                Ok(ExitCode::SUCCESS)
            }
            BearingsAction::Hide => {
                client.set_bearings(BearingsCommand::Hide)?;
                Ok(ExitCode::SUCCESS)
            }
            BearingsAction::Toggle => {
                client.set_bearings(BearingsCommand::Toggle)?;
                Ok(ExitCode::SUCCESS)
            }
        }),
        Ok(Action::BearingsHelp) => show(help::BEARINGS_HELP),
        Ok(Action::Trail { request, output }) => with_client(|client| match request {
            TrailCommand::Navigate { direction, output } => {
                client.navigate_trail(direction, output.as_deref())?;
                Ok(ExitCode::SUCCESS)
            }
            TrailCommand::List { output: connector } => {
                Ok(print::trail(client.trail(connector.as_deref())?, output))
            }
            TrailCommand::Goto { target, output } => {
                client.goto_trail(target, output.as_deref())?;
                Ok(ExitCode::SUCCESS)
            }
        }),
        Ok(Action::TrailHelp) => show(help::TRAIL_HELP),
        Ok(Action::WindowTransfer(direction)) => with_client(|client| {
            client.transfer_window(direction)?;
            Ok(ExitCode::SUCCESS)
        }),
        Ok(Action::PanHelp) => show("Usage: halleyctl pan left|right|up|down\n"),
        Ok(Action::PanField(direction)) => with_client(|client| {
            client.pan_field(direction)?;
            Ok(ExitCode::SUCCESS)
        }),
        Ok(Action::MonitorFocus(target)) => with_client(|client| {
            client.focus_monitor(target)?;
            Ok(ExitCode::SUCCESS)
        }),
        Ok(Action::MonitorHelp) => show(help::MONITOR_HELP),
        Ok(Action::StackCycle { direction, output }) => with_client(|client| {
            client.cycle_stack(direction, output.as_deref())?;
            Ok(ExitCode::SUCCESS)
        }),
        Ok(Action::StackHelp) => show(help::STACK_HELP),
        Ok(Action::Tile {
            direction,
            output,
            swap,
        }) => with_client(|client| {
            if swap {
                client.swap_tile(direction, output.as_deref())?;
            } else {
                client.focus_tile(direction, output.as_deref())?;
            }
            Ok(ExitCode::SUCCESS)
        }),
        Ok(Action::TileHelp) => show(help::TILE_HELP),
        Ok(Action::Portal { command, json }) => cmd::portal::run(command, json),
        Ok(Action::PortalHelp) => show(help::PORTAL_HELP),
        Ok(Action::ConfigEdit(path)) => config::edit(path),
        Ok(Action::ConfigMigrate { path, dry_run }) => config::migrate(path, dry_run),
        Ok(Action::ConfigVerify(path)) => config::verify(path),
        Ok(Action::ConfigHelp) => show(help::CONFIG_HELP),
        Ok(Action::Quit) => with_client(|client| {
            client.request_quit()?;
            Ok(ExitCode::SUCCESS)
        }),
        Ok(Action::Version) => with_client(|client| Ok(print::version(client.server_info()))),
        Ok(Action::Help) => show(help::HELP),
        Err(err) => {
            eprintln!("halleyctl: {err}\n\n{}", help::HELP);
            ExitCode::from(2)
        }
    }
}

fn with_client(operation: impl FnOnce(&Client) -> halley_api::Result<ExitCode>) -> ExitCode {
    let client = match Client::connect() {
        Ok(client) => client,
        Err(error) => {
            eprintln!("halleyctl: failed to reach the running compositor: {error}");
            return ExitCode::FAILURE;
        }
    };
    match operation(&client) {
        Ok(code) => code,
        Err(error) => {
            eprintln!(
                "halleyctl: compositor request failed ({:?}): {}",
                error.kind(),
                error.message()
            );
            ExitCode::FAILURE
        }
    }
}

fn show(text: &str) -> ExitCode {
    print!("{text}");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    #[test]
    fn help_separates_commands_from_options() {
        let commands = crate::help::HELP
            .split_once("Commands:\n")
            .and_then(|(_, rest)| rest.split_once("\nOptions:"))
            .map(|(commands, _)| commands)
            .expect("help has Commands followed by Options");

        assert!(commands.contains("outputs"));
        assert!(commands.contains("reload"));
        assert!(commands.contains("capture"));
        assert!(commands.contains("dpms"));
        assert!(commands.contains("node"));
        assert!(commands.contains("cluster"));
        assert!(commands.contains("bearings"));
        assert!(commands.contains("trail"));
        assert!(commands.contains("monitor"));
        assert!(commands.contains("stack"));
        assert!(commands.contains("tile"));
        assert!(commands.contains("portal"));
        assert!(!commands.contains("gamescope"));
        assert!(commands.contains("config"));
        assert!(commands.contains("basics"));
        assert!(commands.contains("quit"));
        assert!(!commands.contains("--help"));
        assert!(!commands.contains("--version"));
    }
}
