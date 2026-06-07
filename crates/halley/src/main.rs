use std::ffi::OsString;

fn main() {
    let args = match CliArgs::parse(std::env::args_os().skip(1)) {
        Ok(ParseResult::Run(args)) => args,
        Ok(ParseResult::Help) => {
            print_help();
            return;
        }
        Err(err) => {
            eprintln!("halley: {err}");
            eprintln!("Try 'halley --help' for usage.");
            std::process::exit(2);
        }
    };
    if let Some(config) = args.config {
        unsafe {
            std::env::set_var("HALLEY_WL_CONFIG", config);
        }
    }
    let result = if args.session {
        halley_wl::run_session()
    } else {
        halley_wl::run()
    };

    if let Err(err) = result {
        eprintln!("halley-wl exited with error: {err}");
        std::process::exit(1);
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
struct CliArgs {
    session: bool,
    config: Option<OsString>,
}

#[derive(Debug, Eq, PartialEq)]
enum ParseResult {
    Run(CliArgs),
    Help,
}

impl CliArgs {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<ParseResult, String> {
        let mut out = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--help" || arg == "-h" {
                return Ok(ParseResult::Help);
            }
            if arg == "--session" {
                out.session = true;
                continue;
            }
            if arg == "--config" || arg == "-c" {
                let Some(path) = args.next() else {
                    return Err(format!("{} requires a path", arg.to_string_lossy()));
                };
                if path.to_string_lossy().trim().is_empty() {
                    return Err(format!(
                        "{} requires a non-empty path",
                        arg.to_string_lossy()
                    ));
                }
                out.config = Some(path);
                continue;
            }
            if let Some(path) = arg.to_string_lossy().strip_prefix("--config=") {
                if path.trim().is_empty() {
                    return Err("--config requires a non-empty path".to_string());
                }
                out.config = Some(OsString::from(path));
                continue;
            }

            return Err(format!("unknown argument {}", arg.to_string_lossy()));
        }

        Ok(ParseResult::Run(out))
    }
}

fn print_help() {
    println!(
        "Halley spatial Wayland compositor\n\
\n\
Usage: halley [OPTIONS]\n\
\n\
Options:\n\
  -c, --config <PATH>  Use a specific config file\n\
  -h, --help           Show this help\n\
      --session        Run as a full desktop session; kept for session wrappers\n\
                       and services. Normal users should launch halley-session."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_long_config_with_space() {
        assert_eq!(
            CliArgs::parse(os(&["--config", "/tmp/halley.rune"])).unwrap(),
            ParseResult::Run(CliArgs {
                session: false,
                config: Some(OsString::from("/tmp/halley.rune")),
            })
        );
    }

    #[test]
    fn parses_long_config_with_equals() {
        assert_eq!(
            CliArgs::parse(os(&["--config=/tmp/halley.rune"])).unwrap(),
            ParseResult::Run(CliArgs {
                session: false,
                config: Some(OsString::from("/tmp/halley.rune")),
            })
        );
    }

    #[test]
    fn parses_short_config_and_session() {
        assert_eq!(
            CliArgs::parse(os(&["--session", "-c", "/tmp/halley.rune"])).unwrap(),
            ParseResult::Run(CliArgs {
                session: true,
                config: Some(OsString::from("/tmp/halley.rune")),
            })
        );
    }

    #[test]
    fn parses_help_flags() {
        assert_eq!(CliArgs::parse(os(&["--help"])).unwrap(), ParseResult::Help);
        assert_eq!(CliArgs::parse(os(&["-h"])).unwrap(), ParseResult::Help);
    }

    #[test]
    fn unknown_arg_errors() {
        assert!(CliArgs::parse(os(&["session"])).is_err());
    }
}
