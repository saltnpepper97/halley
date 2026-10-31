use rune_cfg::RuneConfig;

const DEFAULT_SIZE: u8 = 24;
const DEFAULT_HIDE_AFTER_MS: u32 = 2_000;

/// Cursor appearance and presentation-only visibility policy.
///
/// `hide_after_ms` is `None` when inactivity hiding is explicitly disabled
/// with `hide-after-ms 0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub theme: String,
    pub size: u8,
    pub disable_hardware_cursor: bool,
    pub hide_when_typing: bool,
    pub hide_on_keyboard_nav: bool,
    pub hide_on_touch: bool,
    pub hide_after_ms: Option<u32>,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            theme: "default".to_string(),
            size: DEFAULT_SIZE,
            disable_hardware_cursor: false,
            hide_when_typing: false,
            hide_on_keyboard_nav: true,
            hide_on_touch: true,
            hide_after_ms: Some(DEFAULT_HIDE_AFTER_MS),
        }
    }
}

pub fn parse_cursor(config: &RuneConfig) -> Cursor {
    let defaults = Cursor::default();
    let theme = config.get_or("cursor.theme", defaults.theme.clone());
    let theme = if theme.trim().is_empty() {
        defaults.theme
    } else {
        theme
    };
    let size = match config.get_or("cursor.size", defaults.size) {
        0 => defaults.size,
        size => size,
    };
    let hide_after_ms = match config.get_or(
        "cursor.hide-after-ms",
        defaults
            .hide_after_ms
            .expect("the built-in inactivity timeout is enabled"),
    ) {
        0 => None,
        timeout => Some(timeout),
    };

    Cursor {
        theme,
        size,
        disable_hardware_cursor: config.get_or(
            "cursor.disable-hardware-cursor",
            defaults.disable_hardware_cursor,
        ),
        hide_when_typing: config.get_or("cursor.hide-when-typing", defaults.hide_when_typing),
        hide_on_keyboard_nav: config
            .get_or("cursor.hide-on-keyboard-nav", defaults.hide_on_keyboard_nav),
        hide_on_touch: config.get_or("cursor.hide-on-touch", defaults.hide_on_touch),
        hide_after_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cursor_section() {
        let config = RuneConfig::from_str(
            r#"
cursor:
  theme "Breeze"
  size 32
  disable-hardware-cursor true
  hide-when-typing true
  hide-on-keyboard-nav false
  hide-on-touch false
  hide-after-ms 750
end
"#,
        )
        .expect("valid Rune config");

        assert_eq!(
            parse_cursor(&config),
            Cursor {
                theme: "Breeze".to_string(),
                size: 32,
                disable_hardware_cursor: true,
                hide_when_typing: true,
                hide_on_keyboard_nav: false,
                hide_on_touch: false,
                hide_after_ms: Some(750),
            }
        );
    }

    #[test]
    fn zero_timeout_disables_inactivity_hiding() {
        let config = RuneConfig::from_str(
            "cursor:\n  hide-after-ms 0\nend\nkeybinds:\n  mod \"super\"\nend\n",
        )
        .expect("valid Rune config");

        assert_eq!(parse_cursor(&config).hide_after_ms, None);
    }

    #[test]
    fn keyboard_navigation_hiding_defaults_on_and_is_configurable() {
        let missing = RuneConfig::from_str("keybinds:\n  mod \"super\"\nend\n").unwrap();
        assert!(parse_cursor(&missing).hide_on_keyboard_nav);

        let disabled = RuneConfig::from_str(
            "cursor:\n  hide-on-keyboard-nav false\nend\nkeybinds:\n  mod \"super\"\nend\n",
        )
        .unwrap();
        assert!(!parse_cursor(&disabled).hide_on_keyboard_nav);
    }

    #[test]
    fn empty_theme_and_zero_size_use_safe_defaults() {
        let config = RuneConfig::from_str(
            "cursor:\n  theme \"\"\n  size 0\nend\nkeybinds:\n  mod \"super\"\nend\n",
        )
        .expect("valid Rune config");

        let parsed = parse_cursor(&config);
        assert_eq!(parsed.theme, "default");
        assert_eq!(parsed.size, DEFAULT_SIZE);
    }
}
