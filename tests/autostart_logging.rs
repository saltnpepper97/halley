//! Drive the production detached launcher and real logger entry point.
#![allow(dead_code)]
#[path = "../src/session/autostart.rs"]
mod autostart;
#[path = "../src/autostart_log.rs"]
mod autostart_log;
#[path = "../src/session/environment.rs"]
mod environment;
#[path = "../src/session/spawn.rs"]
mod spawn;

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "halley-logged-launch-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn halley_binary() -> PathBuf {
    std::env::var_os("HALLEY_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_halley")))
}

fn wait_for(path: &Path, needle: &str) -> String {
    for _ in 0..300 {
        if let Ok(text) = fs::read_to_string(path)
            && text.contains(needle)
        {
            return text;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("missing {needle:?} in {}", path.display());
}

#[test]
fn logger_preserves_output_and_environment_until_the_command_exits() {
    let scratch = Scratch::new();
    let path = scratch.0.join("service.log");
    let environment = environment::LaunchEnvironment::new(&BTreeMap::from([
        ("CUSTOM".into(), "configured-value".into()),
        ("WAYLAND_DISPLAY".into(), "wrong-display".into()),
        ("DISPLAY".into(), ":999".into()),
    ]));
    let mut process = spawn::session_autostart_process(
        &halley_binary(),
        &path,
        "printf started; sleep 0.25; printf '\n%s %s %s %s\n' \"$CUSTOM\" \"$WAYLAND_DISPLAY\" \"$DISPLAY\" \"$XCURSOR_SIZE\"; printf finished >&2; exit 19",
        OsStr::new("wayland-9"),
        Some(OsStr::new(":12")),
        32,
        &environment,
    );
    let mut intermediate = process.spawn().unwrap();
    drop(process);
    let started = wait_for(&path, "logger_pid=");
    let logger_pid: i32 = started
        .split("logger_pid=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::getsid(logger_pid) }, logger_pid);
    assert_eq!(
        fs::read_to_string(format!("/proc/{logger_pid}/comm"))
            .unwrap()
            .trim(),
        "halley-autolog"
    );
    let text = wait_for(&path, "exit status: 19");
    assert!(text.contains("started"));
    assert!(text.contains("finished"));
    assert!(text.contains("configured-value wayland-9 :12 32"));
    assert!(text.contains("logger_pid="));
    assert_eq!(intermediate.wait().unwrap().code(), Some(19));
}

#[test]
fn missing_autostart_command_reports_shell_error_and_exit_127() {
    let scratch = Scratch::new();
    let path = scratch.0.join("missing.log");
    let mut process = spawn::session_autostart_process(
        &halley_binary(),
        &path,
        "halley-nonexistent-command-for-logging-test",
        OsStr::new("wayland-9"),
        None,
        24,
        &environment::LaunchEnvironment::default(),
    );
    let mut child = process.spawn().unwrap();
    drop(process);
    let text = wait_for(&path, "exit status: 127");
    assert_eq!(child.wait().unwrap().code(), Some(127));
    assert!(text.contains("not found"));
    assert!(text.contains("DISPLAY=None"));
}

#[test]
fn native_logout_stops_only_owned_startup_groups_and_drains_final_output() {
    let scratch = Scratch::new();
    let path = scratch.0.join("owned.log");
    let mut process = spawn::session_autostart_process(
        &halley_binary(),
        &path,
        "trap 'printf graceful-shutdown; exit 0' TERM; printf ready; while :; do sleep 10; done",
        OsStr::new("wayland-9"),
        None,
        24,
        &environment::LaunchEnvironment::default(),
    );
    let child = process.spawn().unwrap();
    let pid = child.id();
    let mut unrelated = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    wait_for(&path, "ready");
    let mut children = vec![child];
    autostart::stop_children(&mut children);
    assert!(children.is_empty());
    let text = wait_for(&path, "=== exit");
    assert!(text.contains("graceful-shutdown"), "{text}");
    assert!(text.contains("exit status: 0"), "{text}");
    assert!(unrelated.try_wait().unwrap().is_none());
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}

#[test]
fn native_logout_is_bounded_when_an_app_ignores_termination() {
    let scratch = Scratch::new();
    let path = scratch.0.join("stubborn.log");
    let mut process = spawn::session_autostart_process(
        &halley_binary(),
        &path,
        "trap '' TERM; printf ready; while :; do sleep 10; done",
        OsStr::new("wayland-9"),
        None,
        24,
        &environment::LaunchEnvironment::default(),
    );
    let child = process.spawn().unwrap();
    let pid = child.id();
    wait_for(&path, "ready");
    let now = std::time::Instant::now();
    autostart::stop_children(&mut vec![child]);
    assert!(now.elapsed() < Duration::from_secs(2));
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
}
