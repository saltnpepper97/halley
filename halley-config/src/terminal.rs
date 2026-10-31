use std::ffi::OsStr;

/// Terminal candidates for Halley's built-in `open-terminal` action, in
/// priority order.
///
/// The first three are deliberately `alacritty`, `kitty`, then `ghostty`.
/// The remaining entries provide sensible coverage across common Wayland and
/// desktop environments.
pub const TERMINAL_PRIORITY: &[&str] = &[
    "alacritty",
    "kitty",
    "ghostty",
    "wezterm",
    "foot",
    "footclient",
    "rio",
    "contour",
    "kgx",
    "gnome-terminal",
    "konsole",
    "xfce4-terminal",
    "tilix",
    "terminator",
    "mate-terminal",
    "qterminal",
    "lxterminal",
    "xterm",
];

/// Resolve the first available terminal from `TERMINAL_PRIORITY`.
///
/// Takes the availability check as an injectable closure so the
/// priority-order logic is unit-testable without touching the real
/// filesystem/PATH - see `resolve_default_terminal_from_path` for the real
/// version.
pub fn resolve_default_terminal(is_available: impl Fn(&str) -> bool) -> Option<String> {
    TERMINAL_PRIORITY
        .iter()
        .find(|name| is_available(name))
        .map(|name| name.to_string())
}

/// Real-PATH-scanning version of `resolve_default_terminal`.
pub fn resolve_default_terminal_from_path() -> Option<String> {
    let path = std::env::var_os("PATH");
    resolve_default_terminal_in_path(path.as_deref())
}

/// PATH-injectable terminal resolution for compositor-managed launch
/// environments. `None` means no search path is available.
pub fn resolve_default_terminal_in_path(path: Option<&OsStr>) -> Option<String> {
    resolve_default_terminal(|command| command_exists_in_path(command, path))
}

fn command_exists_in_path(command: &str, path: Option<&OsStr>) -> bool {
    path.is_some_and(|path| std::env::split_paths(path).any(|dir| dir.join(command).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_selection_follows_priority_and_handles_missing_candidates() {
        assert_eq!(&TERMINAL_PRIORITY[..3], &["alacritty", "kitty", "ghostty"]);
        for (available, expected) in [
            (&["kitty", "wezterm"][..], Some("kitty")),
            (&["alacritty", "kitty", "ghostty"][..], Some("alacritty")),
            (&["contour"][..], Some("contour")),
            (&[][..], None),
        ] {
            let resolved = resolve_default_terminal(|name| available.contains(&name));
            assert_eq!(resolved.as_deref(), expected, "available: {available:?}");
        }
    }
}
