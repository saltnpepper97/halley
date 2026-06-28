use std::env;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use halley_api::{CompositorRequest, Request, Response};
use halley_config::RuntimeTuning;
use halley_config::gamescope::{
    GamescopeDecision, TargetDims, build_gamescope_argv, resolve_profile,
};
use halley_ipc::send_request;

use crate::help::HelpTopic;
use crate::parse::{ParseOutcome, UsageError};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GamescopeMode {
    /// Resolve and exec the (possibly wrapped) command, replacing this process.
    Run,
    /// Resolve and print the command that would run, without executing.
    Print,
}

pub(crate) struct GamescopeInvocation {
    pub(crate) mode: GamescopeMode,
    pub(crate) app_id: Option<String>,
    pub(crate) command: Vec<String>,
}

/// Parse `gamescope <run|print> [--app-id <id>] -- <command…>`.
pub(crate) fn parse_gamescope_request(args: &[String]) -> Result<ParseOutcome, UsageError> {
    let Some((mode_arg, rest)) = args.split_first() else {
        return Err(UsageError::new(
            "expected `run` or `print` after `gamescope`",
            HelpTopic::Gamescope,
        ));
    };
    let mode = match mode_arg.as_str() {
        "run" => GamescopeMode::Run,
        "print" => GamescopeMode::Print,
        "help" | "--help" | "-h" => return Ok(ParseOutcome::Help(HelpTopic::Gamescope)),
        other => {
            return Err(UsageError::new(
                format!("unknown gamescope subcommand `{other}`; expected `run` or `print`"),
                HelpTopic::Gamescope,
            ));
        }
    };

    let mut app_id = None;
    let mut idx = 0;
    while idx < rest.len() {
        match rest[idx].as_str() {
            "--" => {
                idx += 1;
                break;
            }
            "--app-id" => {
                let value = rest.get(idx + 1).ok_or_else(|| {
                    UsageError::new("`--app-id` requires a value", HelpTopic::Gamescope)
                })?;
                app_id = Some(value.clone());
                idx += 2;
            }
            other => {
                return Err(UsageError::new(
                    format!("unexpected argument `{other}` before `--`"),
                    HelpTopic::Gamescope,
                ));
            }
        }
    }

    let command: Vec<String> = rest[idx..].to_vec();
    if command.is_empty() {
        return Err(UsageError::new(
            "no game command given; usage: `gamescope run -- <command…>`",
            HelpTopic::Gamescope,
        ));
    }

    Ok(ParseOutcome::Gamescope(GamescopeInvocation {
        mode,
        app_id,
        command,
    }))
}

/// Execute a parsed gamescope invocation. On `Run` this replaces the current
/// process (via `exec`) and only returns on failure.
pub(crate) fn run(invocation: GamescopeInvocation) -> ! {
    let config = load_runtime_tuning();
    let gamescope = &config.gamescope;
    let app_id = invocation.app_id.clone().or_else(steam_app_id_from_env);

    match resolve_profile(gamescope, app_id.as_deref()) {
        GamescopeDecision::Disabled => finish(invocation.mode, invocation.command.clone(), None),
        GamescopeDecision::Skip => {
            eprintln!(
                "halleyctl gamescope: profile for `{}` is opted out (enabled false); running unwrapped",
                app_id.as_deref().unwrap_or("<unknown app-id>")
            );
            finish(invocation.mode, invocation.command.clone(), None)
        }
        GamescopeDecision::Wrap(profile) => {
            if !command_exists("gamescope") {
                eprintln!(
                    "halleyctl gamescope: `gamescope` binary not found in PATH; running unwrapped.\n  Install gamescope (e.g. your distro's `gamescope` package) to enable wrapping, or set `gamescope.enabled false`."
                );
                finish(invocation.mode, invocation.command.clone(), None);
            }
            let target = resolve_target_dims(&profile.monitor);
            let (argv, diagnostics) = build_gamescope_argv(&profile, &target, &invocation.command);
            for diagnostic in diagnostics {
                eprintln!("halleyctl {diagnostic}");
            }
            finish(invocation.mode, argv, Some("gamescope"))
        }
    }
}

/// Print (Print mode) or exec (Run mode) the final argv.
fn finish(mode: GamescopeMode, argv: Vec<String>, _label: Option<&str>) -> ! {
    if argv.is_empty() {
        eprintln!("halleyctl gamescope: empty command");
        std::process::exit(2);
    }
    match mode {
        GamescopeMode::Print => {
            println!("{}", shell_join(&argv));
            std::process::exit(0);
        }
        GamescopeMode::Run => {
            let err = Command::new(&argv[0]).args(&argv[1..]).exec();
            // exec only returns on failure.
            eprintln!("halleyctl gamescope: failed to exec `{}`: {err}", argv[0]);
            std::process::exit(127);
        }
    }
}

fn resolve_target_dims(selector: &str) -> TargetDims {
    match send_request(&Request::Compositor(CompositorRequest::GamescopeTarget {
        selector: selector.to_string(),
    })) {
        Ok(Response::GamescopeTarget(target)) => TargetDims {
            width: (target.width > 0).then_some(target.width),
            height: (target.height > 0).then_some(target.height),
            refresh_hz: target.refresh_hz,
        },
        Ok(Response::Error(err)) => {
            eprintln!(
                "halleyctl gamescope: could not resolve monitor `{selector}` ({err:?}); using gamescope auto-detection"
            );
            TargetDims::default()
        }
        Ok(_) => TargetDims::default(),
        Err(err) => {
            eprintln!(
                "halleyctl gamescope: compositor unreachable ({err}); using gamescope auto-detection"
            );
            TargetDims::default()
        }
    }
}

fn steam_app_id_from_env() -> Option<String> {
    for key in ["SteamAppId", "SteamGameId"] {
        if let Ok(value) = env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
                return Some(format!("steam_app_{trimmed}"));
            }
        }
    }
    None
}

fn load_runtime_tuning() -> RuntimeTuning {
    resolve_config_path()
        .and_then(|path| RuntimeTuning::from_rune_file(path.as_str()))
        .unwrap_or_default()
}

fn resolve_config_path() -> Option<String> {
    if let Ok(path) = env::var("HALLEY_WL_CONFIG")
        && !path.trim().is_empty()
    {
        return Some(path);
    }
    let user = RuntimeTuning::default_home_config_path();
    if Path::new(&user).is_file() {
        return Some(user);
    }
    let system = "/etc/halley/halley.rune";
    if Path::new(system).is_file() {
        return Some(system.to_string());
    }
    None
}

fn command_exists(command: &str) -> bool {
    env::var_os("PATH").is_some_and(|path| {
        env::split_paths(&path).any(|dir| {
            let candidate = dir.join(command);
            candidate.is_file() && is_executable(&candidate)
        })
    })
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Render an argv as a copy-pasteable shell command (best-effort quoting).
fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.' | ':' | '='))
    {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}
