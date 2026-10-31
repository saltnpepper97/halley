use super::*;
use std::time::{Duration, SystemTime};

fn context() -> (LaunchEnvironment, OsString) {
    (LaunchEnvironment::default(), OsString::from("wayland-9"))
}

#[test]
fn once_can_only_be_armed_and_run_once() {
    let (environment, display) = context();
    let mut autostart = Autostart::enabled();
    autostart.arm_once(&display, Vec::new());
    autostart.arm_once(&display, vec!["sleep 30".to_string()]);
    autostart.run_once(None, 24, &environment);
    autostart.run_once(None, 24, &environment);

    assert!(matches!(autostart.once, OnceState::Finished));
}

#[test]
fn once_skips_duplicate_commands_but_preserves_arguments_and_order() {
    let (environment, display) = context();
    let mut autostart = Autostart::enabled();
    autostart.arm_once(
        &display,
        vec![
            "waybar".into(),
            " mako ".into(),
            "  waybar  ".into(),
            "waybar --config other".into(),
            "mako".into(),
            "  ".into(),
        ],
    );
    let OnceState::Pending(commands) = &autostart.once else {
        panic!("startup not pending");
    };
    assert_eq!(commands, &["waybar", " mako ", "waybar --config other"]);
    // Nested startup remains suppressed and the ordinary once policy is
    // unchanged; these commands are not launched by the test.
    let mut nested = Autostart::disabled();
    nested.arm_once(&display, commands.clone());
    nested.run_once(None, 24, &environment);
    assert!(nested.children.is_empty());
}

fn unique_label(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn events_for(label: &str) -> Vec<(eventline::EventKind, String)> {
    eventline::records()
        .into_iter()
        .filter_map(|record| match record.kind {
            eventline::core::RecordKind::Event { kind, name, .. } if name.contains(label) => {
                Some((kind, name))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn failed_autostart_is_reaped_and_reported_once_with_its_log_path() {
    let label = unique_label("failed-service");
    let path = std::env::temp_dir().join(format!("{label}.log"));
    let mut child = std::process::Command::new("sh")
        .args(["-c", "exit 23"])
        .spawn()
        .unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(23));
    let mut autostart = Autostart::enabled();
    autostart
        .children
        .push(AutostartChild::new(&label, child, Some(path.clone())));
    autostart.reap_finished();
    assert!(autostart.children.is_empty());
    autostart.next_reap = std::time::Instant::now();
    autostart.reap_finished();
    let events = events_for(&label);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, eventline::EventKind::Warning);
    assert!(events[0].1.contains("exit status: 23"));
    assert!(events[0].1.contains(path.to_str().unwrap()));
}

#[test]
fn successful_one_shot_is_reaped_without_a_failure_warning() {
    let label = unique_label("one-shot");
    let mut child = std::process::Command::new("true").spawn().unwrap();
    assert!(child.wait().unwrap().success());
    let mut autostart = Autostart::enabled();
    autostart
        .children
        .push(AutostartChild::new(&label, child, None));
    autostart.reap_finished();
    assert!(autostart.children.is_empty());
    let events = events_for(&label);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, eventline::EventKind::Debug);
}

#[test]
fn live_autostarts_keep_their_group_until_owned_shutdown_without_failure_warnings() {
    use std::os::unix::process::CommandExt;
    let label = unique_label("owned-service");
    let child = std::process::Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id();
    let mut unrelated = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let mut autostart = Autostart::enabled();
    autostart
        .children
        .push(AutostartChild::new(&label, child, None));
    autostart.reap_finished();
    let retained = autostart.children.len() == 1;
    autostart.shutdown();
    let unrelated_running = unrelated.try_wait().unwrap().is_none();
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
    assert!(retained);
    assert!(autostart.children.is_empty());
    assert!(unrelated_running);
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    assert!(events_for(&label).is_empty());
}

#[test]
fn nested_policy_suppresses_once_and_reload_commands() {
    let (environment, display) = context();
    let mut autostart = Autostart::disabled();
    autostart.arm_once(&display, vec!["sleep 30".to_string()]);
    autostart.run_once(None, 24, &environment);
    autostart.run_reload(&["sleep 30".to_string()], None, 24, &environment);

    assert!(matches!(autostart.once, OnceState::Finished));
}

#[test]
fn reload_does_not_consume_once() {
    let (environment, display) = context();
    let mut autostart = Autostart::enabled();
    autostart.arm_once(&display, Vec::new());
    autostart.run_reload(&["  ".to_string()], None, 24, &environment);

    assert!(matches!(autostart.once, OnceState::Pending(_)));
}

#[test]
fn dropping_launcher_does_not_stop_launched_commands() {
    let (environment, display) = context();
    let marker = std::env::temp_dir().join(format!(
        "halley-autostart-detached-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let command = format!(
        "printf started > '{}'; sleep 0.1; printf finished > '{}'",
        marker.display(),
        marker.display()
    );
    super::super::spawn::spawn_detached(&command, &display, None, 24, &environment);

    assert!(wait_for_marker(&marker, "started"));
    assert!(wait_for_marker(&marker, "finished"));
    let _ = std::fs::remove_file(marker);
}

fn wait_for_marker(path: &std::path::Path, expected: &str) -> bool {
    for _ in 0..100 {
        if std::fs::read_to_string(path).is_ok_and(|value| value == expected) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}
