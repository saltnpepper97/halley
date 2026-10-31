use super::*;

#[test]
fn command_lines_use_a_shell_and_halley_display() {
    let process = detached_process(
        "grim -g \"$(slurp)\" ~/shot.png",
        OsStr::new("wayland-9"),
        Some(OsStr::new(":12")),
        32,
        &LaunchEnvironment::default(),
    );
    assert_eq!(process.get_program(), "sh");
    assert_eq!(
        process.get_args().collect::<Vec<_>>(),
        ["-c", "grim -g \"$(slurp)\" ~/shot.png"]
    );
    assert!(process.get_envs().any(|(name, value)| {
        name == "WAYLAND_DISPLAY" && value == Some(OsStr::new("wayland-9"))
    }));
    assert!(
        process
            .get_envs()
            .any(|(name, value)| name == "DISPLAY" && value == Some(OsStr::new(":12")))
    );
    assert!(
        process
            .get_envs()
            .any(|(name, value)| { name == "XCURSOR_SIZE" && value == Some(OsStr::new("32")) })
    );
}

#[test]
fn cursor_theme_is_inherited_and_an_explicit_launch_override_wins() {
    let mut environment = LaunchEnvironment::default();
    environment.set_cursor_theme("Breeze5");
    let process = configured_process(
        "true",
        OsStr::new("wayland-4"),
        Some(OsStr::new(":8")),
        24,
        &environment,
        &[],
    );
    assert_eq!(
        process
            .get_envs()
            .find(|(key, _)| *key == "XCURSOR_THEME")
            .unwrap()
            .1,
        Some(OsStr::new("Breeze5"))
    );
    let process = configured_process(
        "true",
        OsStr::new("wayland-4"),
        Some(OsStr::new(":8")),
        24,
        &environment,
        &[("XCURSOR_THEME", "Adwaita")],
    );
    assert_eq!(
        process
            .get_envs()
            .find(|(key, _)| *key == "XCURSOR_THEME")
            .unwrap()
            .1,
        Some(OsStr::new("Adwaita"))
    );
}

#[test]
fn unavailable_xwayland_removes_ambient_display() {
    let process = detached_process(
        "foot",
        OsStr::new("wayland-2"),
        None,
        24,
        &LaunchEnvironment::default(),
    );
    assert!(
        process
            .get_envs()
            .any(|(name, value)| name == "DISPLAY" && value.is_none())
    );
}

#[test]
fn configured_environment_is_inherited_but_session_values_win() {
    let environment = LaunchEnvironment::new(&std::collections::BTreeMap::from([
        ("CUSTOM".to_string(), "value".to_string()),
        ("DISPLAY".to_string(), ":99".to_string()),
        ("WAYLAND_DISPLAY".to_string(), "wrong".to_string()),
        ("XCURSOR_SIZE".to_string(), "99".to_string()),
    ]));
    let process = detached_process(
        "true",
        OsStr::new("wayland-4"),
        Some(OsStr::new(":8")),
        24,
        &environment,
    );
    let env = process
        .get_envs()
        .collect::<std::collections::BTreeMap<_, _>>();

    assert_eq!(
        env.get(OsStr::new("CUSTOM")),
        Some(&Some(OsStr::new("value")))
    );
    assert_eq!(
        env.get(OsStr::new("WAYLAND_DISPLAY")),
        Some(&Some(OsStr::new("wayland-4")))
    );
    assert_eq!(
        env.get(OsStr::new("DISPLAY")),
        Some(&Some(OsStr::new(":8")))
    );
    assert_eq!(
        env.get(OsStr::new("XCURSOR_SIZE")),
        Some(&Some(OsStr::new("24")))
    );
}
