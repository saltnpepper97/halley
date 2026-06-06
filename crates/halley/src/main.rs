use std::ffi::OsString;

fn main() {
    let args = match CliArgs::parse(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("halley: {err}");
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

impl CliArgs {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut out = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
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

        Ok(out)
    }
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
            CliArgs {
                session: false,
                config: Some(OsString::from("/tmp/halley.rune")),
            }
        );
    }

    #[test]
    fn parses_long_config_with_equals() {
        assert_eq!(
            CliArgs::parse(os(&["--config=/tmp/halley.rune"]))
                .unwrap()
                .config,
            Some(OsString::from("/tmp/halley.rune"))
        );
    }

    #[test]
    fn parses_short_config_and_session() {
        assert_eq!(
            CliArgs::parse(os(&["--session", "-c", "/tmp/halley.rune"])).unwrap(),
            CliArgs {
                session: true,
                config: Some(OsString::from("/tmp/halley.rune")),
            }
        );
    }
}
