use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::process::Command;

use super::{LaunchEnvironment, SESSION_VARIABLES, cursor_assignments};

#[test]
fn activation_exports_portal_and_wayland_identity() {
    for required in ["WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE"] {
        assert!(SESSION_VARIABLES.contains(&required));
    }
}

#[test]
fn cursor_assignments_use_the_validated_runtime_config() {
    assert_eq!(
        cursor_assignments("Breeze5", 32),
        [
            std::ffi::OsString::from("XCURSOR_THEME=Breeze5"),
            std::ffi::OsString::from("XCURSOR_SIZE=32")
        ]
    );
}

#[test]
fn launch_environment_keeps_live_reload_values_until_restart() {
    let mut environment =
        LaunchEnvironment::new(&BTreeMap::from([("FIRST".to_string(), "one".to_string())]));
    environment.reload(&BTreeMap::from([("SECOND".to_string(), "two".to_string())]));

    let mut command = Command::new("true");
    environment.apply_to(&mut command);
    let env = command.get_envs().collect::<BTreeMap<_, _>>();
    assert_eq!(env.get(OsStr::new("FIRST")), Some(&Some(OsStr::new("one"))));
    assert_eq!(
        env.get(OsStr::new("SECOND")),
        Some(&Some(OsStr::new("two")))
    );
    assert_eq!(
        environment.iter().collect::<BTreeMap<_, _>>(),
        BTreeMap::from([
            (OsStr::new("FIRST"), OsStr::new("one")),
            (OsStr::new("SECOND"), OsStr::new("two")),
        ])
    );
}

#[test]
fn cursor_theme_is_authoritative_and_refreshes_after_reload() {
    let mut environment = LaunchEnvironment::new(&BTreeMap::from([(
        "XCURSOR_THEME".to_string(),
        "ambient-override".to_string(),
    )]));
    environment.set_cursor_theme("Breeze5");
    assert_eq!(
        environment
            .iter()
            .find(|(key, _)| *key == "XCURSOR_THEME")
            .unwrap()
            .1,
        OsStr::new("Breeze5")
    );
    environment.reload(&BTreeMap::from([(
        "XCURSOR_THEME".to_string(),
        "stale".to_string(),
    )]));
    environment.set_cursor_theme("Adwaita");
    let mut command = Command::new("true");
    environment.apply_to(&mut command);
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == "XCURSOR_THEME")
            .unwrap()
            .1,
        Some(OsStr::new("Adwaita"))
    );
}

#[test]
fn configured_path_overrides_the_ambient_path() {
    let environment = LaunchEnvironment::new(&BTreeMap::from([(
        "PATH".to_string(),
        "/configured/bin".to_string(),
    )]));

    assert_eq!(
        environment.path().as_deref(),
        Some(OsStr::new("/configured/bin"))
    );
}

#[test]
fn failed_helpers_report_bounded_stderr_with_optional_failures_kept_at_debug() {
    let unique = format!(
        "helper-diagnostic-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    for optional in [false, true] {
        let label = format!("{unique}-{optional}");
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'failed-marker\nline two\n' >&2; head -c 131072 /dev/zero | tr '\\000' x >&2; exit 9"]);
        if optional {
            super::run_optional(&label, &mut command);
        } else {
            assert!(!super::run(&label, &mut command));
        }
        let records: Vec<_> = eventline::records()
            .into_iter()
            .filter_map(|record| match record.kind {
                eventline::core::RecordKind::Event { kind, name, .. }
                    if name.contains(&label) =>
                {
                    Some((kind, name))
                }
                _ => None,
            })
            .collect();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(
            records[0].0,
            if optional {
                eventline::EventKind::Debug
            } else {
                eventline::EventKind::Warning
            }
        );
        assert!(
            records[0]
                .1
                .contains("exit status: 9: failed-marker line two")
        );
        assert!(records[0].1.ends_with("[truncated]"));
        assert!(records[0].1.len() < 5000);
    }
}
