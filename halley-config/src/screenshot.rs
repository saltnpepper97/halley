use rune_cfg::RuneConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screenshot {
    pub directory: String,
}

impl Default for Screenshot {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Self {
            directory: format!("{home}/Pictures/Screenshots"),
        }
    }
}

pub fn parse_screenshot(config: &RuneConfig) -> Screenshot {
    let defaults = Screenshot::default();
    Screenshot {
        directory: config.get_or("screenshot.directory", defaults.directory),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_directory() {
        let config = RuneConfig::from_str(
            r#"
screenshot:
  directory "/tmp/shots"
end
"#,
        )
        .unwrap();

        assert_eq!(
            parse_screenshot(&config),
            Screenshot {
                directory: "/tmp/shots".to_string()
            }
        );
    }
}
