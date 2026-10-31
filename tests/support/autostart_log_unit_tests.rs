use super::*;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "halley-autostart-log-{}-{}",
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

#[test]
fn shell_output_and_nonzero_exit_are_captured() {
    let scratch = Scratch::new();
    let path = scratch.0.join("command.log");
    let mut log = RotatingLog::new(&path, MAX_BYTES).unwrap();
    let mut process = Command::new("sh");
    process.args([
        "-c",
        "printf stdout-marker; printf stderr-marker >&2; exit 23",
    ]);
    assert_eq!(capture(&mut process, &mut log).unwrap(), 23);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("stdout-marker"));
    assert!(text.contains("stderr-marker"));
    assert!(text.contains("exit status: 23"));
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn noisy_output_is_drained_and_all_generations_are_bounded() {
    let scratch = Scratch::new();
    let path = scratch.0.join("noisy.log");
    let mut log = RotatingLog::new(&path, 1024).unwrap();
    let mut process = Command::new("sh");
    process.args([
        "-c",
        "head -c 32768 /dev/zero; printf last-error >&2; exit 7",
    ]);
    assert_eq!(capture(&mut process, &mut log).unwrap(), 7);
    for file in [
        &path,
        &path.with_extension("log.1"),
        &path.with_extension("log.2"),
    ] {
        assert!(fs::metadata(file).unwrap().len() <= 1024);
    }
    let final_log = fs::read_to_string(path).unwrap();
    assert!(final_log.contains("last-error"));
    assert!(final_log.contains("exit status: 7"));
}

#[test]
fn concurrent_workers_do_not_write_into_renamed_files() {
    let scratch = Scratch::new();
    let path = scratch.0.join("shared.log");
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut log = RotatingLog::new(&path, 1024).unwrap();
                for _ in 0..50 {
                    log.append(&[b'x'; 256]).unwrap();
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    for file in [
        &path,
        &path.with_extension("log.1"),
        &path.with_extension("log.2"),
    ] {
        assert!(fs::metadata(file).unwrap().len() <= 1024);
    }
}

#[test]
fn same_program_with_different_arguments_has_distinct_safe_names() {
    assert_ne!(log_name("waybar"), log_name("waybar --config /somewhere"));
    assert!(log_name("/usr/bin/waybar").starts_with("waybar-"));
    assert!(!log_name("../../service; echo output").contains('/'));
}

#[test]
fn unavailable_log_storage_does_not_prevent_command_launch() {
    let scratch = Scratch::new();
    let path = scratch.0.join("missing-directory/command.log");
    assert_eq!(run_worker(&path, OsString::from("exit 23")).unwrap(), 23);
}
