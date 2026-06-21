use std::env;
use std::path::PathBuf;
use std::process::Command;

use halley_api::{CompositorRequest, Request, Response};
use halley_ipc::send_request;

use crate::help::HelpTopic;
use crate::parse::{ParseOutcome, UsageError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PortalCommand {
    Status,
    Version,
}

pub(crate) fn parse_portal_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    if args.is_empty()
        || args
            .iter()
            .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        return Ok(ParseOutcome::Help(HelpTopic::Portal));
    }
    let command = match args[0].as_str() {
        "status" => PortalCommand::Status,
        "version" => PortalCommand::Version,
        other => {
            return Err(UsageError::new(
                format!("unknown portal command: {other}"),
                HelpTopic::Portal,
            ));
        }
    };
    for arg in &args[1..] {
        if arg != "--json" {
            return Err(UsageError::new(
                format!("unexpected portal argument: {arg}"),
                HelpTopic::Portal,
            ));
        }
    }
    Ok(ParseOutcome::Portal(command))
}

pub(crate) fn run(command: PortalCommand) {
    let json = env::args().any(|arg| arg == "--json");
    let portal_path = find_in_path("xdg-desktop-portal-halley");
    let portal_version = portal_path
        .as_ref()
        .and_then(|path| run_version(path).ok())
        .unwrap_or_else(|| "(not found)".to_string());
    let compositor_version = compositor_version();

    match command {
        PortalCommand::Version => print_version(json, portal_version, compositor_version),
        PortalCommand::Status => {
            print_status(json, portal_path, portal_version, compositor_version)
        }
    }
}

fn print_version(json: bool, portal_version: String, compositor_version: Result<String, String>) {
    let compositor = compositor_version.unwrap_or_else(|err| format!("unreachable: {err}"));
    if json {
        println!(
            "{{\n  \"portal\": {:?},\n  \"halleyctl\": {:?},\n  \"compositor\": {:?}\n}}",
            portal_version,
            env!("CARGO_PKG_VERSION"),
            compositor
        );
    } else {
        println!("portal: {portal_version}");
        println!("halleyctl: {}", env!("CARGO_PKG_VERSION"));
        println!("halley: {compositor}");
    }
}

fn print_status(
    json: bool,
    portal_path: Option<PathBuf>,
    portal_version: String,
    compositor_version: Result<String, String>,
) {
    let backend = portal_path
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "(not found in PATH)".to_string());
    let compositor = compositor_version
        .map(|version| format!("ok ({version})"))
        .unwrap_or_else(|err| format!("unreachable ({err})"));
    if json {
        println!(
            "{{\n  \"backend\": {:?},\n  \"portal_version\": {:?},\n  \"compositor\": {:?},\n  \"sources\": [\"screen\", \"window\"],\n  \"cursor_modes\": [\"hidden\", \"embedded\", \"metadata\"]\n}}",
            backend, portal_version, compositor
        );
    } else {
        println!("backend: {backend}");
        println!("portal: {portal_version}");
        println!("compositor-ipc: {compositor}");
        println!("sources: screen, window");
        println!("cursor-modes: hidden, embedded, metadata");
    }
}

fn compositor_version() -> Result<String, String> {
    match send_request(&Request::Compositor(CompositorRequest::Version)) {
        Ok(Response::Version(info)) => Ok(format!(
            "{} (ipc protocol {})",
            info.version, info.ipc_protocol
        )),
        Ok(Response::Error(err)) => Err(format!("{err:?}")),
        Ok(other) => Err(format!("unexpected response: {other:?}")),
        Err(err) => Err(err.to_string()),
    }
}

fn run_version(path: &PathBuf) -> Result<String, String> {
    let output = Command::new(path)
        .arg("--version")
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(format!("exited with {}", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|dir| dir.join(binary))
            .find(|candidate| candidate.is_file())
    })
}
