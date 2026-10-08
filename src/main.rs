#![cfg_attr(not(feature = "xwayland"), allow(dead_code))]

mod accessibility;
mod animation;
mod appearance;
mod autostart_log;
mod backend;
mod capture;
mod clusters;
mod config;
mod cursor;
mod frame_clock;
mod idle_service;
mod input;
mod ipc;
mod logging;
mod nodes;
mod presentation;
mod render;
mod session;
mod shell;
mod titlebar;
mod trail;
#[path = "wayland/dispatch.rs"]
mod upstream_protocols;
mod wayland;
mod window;
#[cfg(feature = "xwayland")]
mod xwayland;
#[cfg(not(feature = "xwayland"))]
#[path = "xwayland/disabled.rs"]
mod xwayland;

fn main() {
    if let Some(status) = autostart_log::run_worker_if_requested() {
        std::process::exit(status);
    }
    logging::init();
    let args = match StartupArgs::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("halley: {err}");
            eprintln!("Try `halley --help` for usage.");
            std::process::exit(2);
        }
    };
    if args.help {
        print_help();
        return;
    }
    if args.version {
        println!("halley {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    if args.session {
        let _session = session::environment::NativeSession;
        session::environment::prepare_session();
        session::tty::run(args.config_path);
    } else if args.force_winit || detect_nested_session() {
        #[cfg(feature = "winit")]
        session::winit::run(args.config_path);
        #[cfg(not(feature = "winit"))]
        {
            eprintln!("halley: nested session detected, but the winit backend is not compiled in");
            eprintln!("Rebuild with `--features winit`, or pass `--session` for a TTY session.");
            std::process::exit(1);
        }
    } else {
        // Reaching the DRM/KMS backend means this process is the desktop
        // session even when it was launched directly from a tty instead of
        // through the display-manager `--session` entry point.
        let _session = session::environment::NativeSession;
        session::environment::prepare_session();
        session::tty::run(args.config_path);
    }
    logging::flush();
}

#[derive(Default)]
struct StartupArgs {
    force_winit: bool,
    session: bool,
    help: bool,
    version: bool,
    config_path: Option<std::path::PathBuf>,
}

impl StartupArgs {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--winit" => parsed.force_winit = true,
                "--session" => parsed.session = true,
                "-h" | "--help" => parsed.help = true,
                "-V" | "--version" => parsed.version = true,
                "-c" | "--config" => {
                    let path = args
                        .next()
                        .ok_or_else(|| format!("`{arg}` requires a path"))?;
                    set_config_path(&mut parsed, path)?;
                }
                _ if arg.starts_with("--config=") => {
                    set_config_path(&mut parsed, arg["--config=".len()..].to_string())?;
                }
                _ => return Err(format!("unknown argument {arg:?}")),
            }
        }
        if parsed.force_winit && parsed.session {
            return Err("`--winit` and `--session` cannot be used together".to_string());
        }
        Ok(parsed)
    }
}

fn print_help() {
    println!("halley {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Usage: halley [OPTIONS]");
    println!();
    println!("Options:");
    println!("  -c, --config PATH  Load configuration from PATH");
    println!("      --session  Start a full TTY desktop session");
    println!("      --winit    Run nested inside the current desktop");
    println!("  -h, --help     Show this help");
    println!("  -V, --version  Show the version");
}

fn set_config_path(parsed: &mut StartupArgs, path: String) -> Result<(), String> {
    if path.is_empty() {
        return Err("configuration path cannot be empty".to_string());
    }
    if parsed
        .config_path
        .replace(std::path::PathBuf::from(path))
        .is_some()
    {
        return Err("configuration path was specified more than once".to_string());
    }
    Ok(())
}

/// Whether we're already running inside another Wayland/X compositor - if
/// so, taking over real hardware (the tty/DRM session) would be wrong even
/// without `--winit`. A `WAYLAND_DISPLAY` or `DISPLAY` already set in the
/// environment means a host compositor/X server is available to nest under.
fn detect_nested_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}

#[cfg(test)]
mod tests {
    use super::StartupArgs;

    #[test]
    fn session_and_winit_are_mutually_exclusive() {
        assert!(StartupArgs::parse(["--session".to_string(), "--winit".to_string()]).is_err());
    }

    #[test]
    fn unknown_arguments_are_rejected() {
        assert!(StartupArgs::parse(["--mystery".to_string()]).is_err());
    }

    #[test]
    fn config_path_accepts_short_long_and_equals_forms() {
        assert_eq!(
            StartupArgs::parse(["-c".to_string(), "one.rune".to_string()])
                .unwrap()
                .config_path,
            Some("one.rune".into())
        );
        assert_eq!(
            StartupArgs::parse(["--config".to_string(), "two.rune".to_string()])
                .unwrap()
                .config_path,
            Some("two.rune".into())
        );
        assert_eq!(
            StartupArgs::parse(["--config=three.rune".to_string()])
                .unwrap()
                .config_path,
            Some("three.rune".into())
        );
    }

    #[test]
    fn config_path_rejects_missing_empty_and_duplicate_values() {
        assert!(StartupArgs::parse(["-c".to_string()]).is_err());
        assert!(StartupArgs::parse(["--config=".to_string()]).is_err());
        assert!(
            StartupArgs::parse([
                "-c".to_string(),
                "one".to_string(),
                "--config=two".to_string()
            ])
            .is_err()
        );
    }
}
