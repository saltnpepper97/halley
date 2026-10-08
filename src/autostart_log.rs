//! Persistent, bounded output capture for detached autostart commands.
//!
//! A separate invocation of Halley drains the child's output. A compositor
//! shutdown must not close that pipe and give an otherwise independent service
//! SIGPIPE. The worker exits without initializing any compositor or protocol.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const WORKER_ARG: &str = "--internal-autostart-log";
const MAX_BYTES: u64 = 1024 * 1024;

pub(crate) fn prepare(command: &str) -> io::Result<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no state directory available"))?;
    let directory = base.join("halley/autostart");
    fs::create_dir_all(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    let path = directory.join(log_name(command));
    // Detect an unavailable log destination before launching the worker. The
    // caller can fall back to the usual detached launch rather than lose an app.
    RotatingLog::new(&path, MAX_BYTES)?;
    Ok(path)
}

fn log_name(command: &str) -> String {
    let label: String = command
        .split_whitespace()
        .next()
        .and_then(|word| Path::new(word).file_name())
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .take(32)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    // Stable across processes and Rust releases; distinguish command lines
    // that use the same executable with different arguments.
    let hash = command
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    format!("{label}-{hash:016x}.log")
}

pub(crate) fn run_worker_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(WORKER_ARG)) {
        return None;
    }
    // Keep process tools able to distinguish this small logger from the real
    // compositor. The worker never initializes Halley's graphics or logging.
    unsafe { libc::prctl(libc::PR_SET_NAME, c"halley-autolog".as_ptr(), 0, 0, 0) };
    // Session shutdown signals the command's process group. Keep its logger
    // alive to drain final output; capture resets this disposition for apps.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
    let result = match (args.next(), args.next(), args.next()) {
        (Some(path), Some(command), None) => run_worker(Path::new(&path), command),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid autostart logger arguments",
        )),
    };
    Some(match result {
        Ok(status) => status,
        Err(err) => {
            eprintln!("halley autostart logger: {err}");
            1
        }
    })
}

fn run_worker(path: &Path, command: OsString) -> io::Result<i32> {
    let mut log = match RotatingLog::new(path, MAX_BYTES) {
        Ok(log) => log,
        Err(err) => {
            // Storage may disappear after the compositor's initial check.
            // Still launch the command when logging cannot be established.
            eprintln!("halley autostart logger: output logging unavailable: {err}");
            let mut process = Command::new("sh");
            process
                .arg("-c")
                .arg(command)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            reset_child_termination(&mut process);
            let status = process.status()?;
            return Ok(exit_code(status));
        }
    };
    let start = format!(
        "\n=== start unix={} logger_pid={} command={command:?} WAYLAND_DISPLAY={:?} DISPLAY={:?} ===\n",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        std::process::id(),
        std::env::var_os("WAYLAND_DISPLAY"),
        std::env::var_os("DISPLAY"),
    );
    let _ = log.append(start.as_bytes());
    let mut process = Command::new("sh");
    process.arg("-c").arg(command);
    capture(&mut process, &mut log)
}

fn capture(process: &mut Command, log: &mut RotatingLog) -> io::Result<i32> {
    reset_child_termination(process);
    let (mut reader, writer) = UnixStream::pair()?;
    process.stdin(Stdio::null());
    process.stdout(Stdio::from(std::os::fd::OwnedFd::from(writer.try_clone()?)));
    process.stderr(Stdio::from(std::os::fd::OwnedFd::from(writer)));
    let child = process.spawn();
    // Command retains its copies of the writer descriptors after spawn. Drop
    // them before reading, or EOF would never arrive even after the app exits.
    process.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = match child {
        Ok(child) => child,
        Err(err) => {
            let _ = log.append(format!("\n=== spawn failed: {err} ===\n").as_bytes());
            return Err(err);
        }
    };
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => {
                // Keep draining if the disk becomes unavailable. A logging
                // failure must not block or kill the launched service.
                let _ = log.append(&buffer[..length]);
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    let status = child.wait()?;
    let _ = log.append(
        format!(
            "\n=== exit unix={}: {status} ===\n",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        )
        .as_bytes(),
    );
    Ok(exit_code(status))
}

fn reset_child_termination(process: &mut Command) {
    unsafe {
        process.pre_exec(|| {
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            Ok(())
        });
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}

struct RotatingLog {
    path: PathBuf,
    lock: File,
    max_bytes: u64,
}

impl RotatingLog {
    fn new(path: &Path, max_bytes: u64) -> io::Result<Self> {
        let lock = private_file(&path.with_extension("lock"))?;
        private_file(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            lock,
            max_bytes,
        })
    }

    fn append(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        // Reload commands can overlap with an earlier launch. Serialize each
        // append and rotation between workers, rather than rename an open log
        // out from under another writer and leave it growing without a bound.
        if unsafe { libc::flock(self.lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            while !bytes.is_empty() {
                let mut file = private_file(&self.path)?;
                let length = file.metadata()?.len();
                if length >= self.max_bytes {
                    drop(file);
                    let previous = self.path.with_extension("log.1");
                    let oldest = self.path.with_extension("log.2");
                    if previous.exists() {
                        fs::rename(&previous, &oldest)?;
                    }
                    fs::rename(&self.path, &previous)?;
                    continue;
                }
                let count = bytes.len().min((self.max_bytes - length) as usize);
                file.write_all(&bytes[..count])?;
                bytes = &bytes[count..];
            }
            Ok(())
        })();
        unsafe { libc::flock(self.lock.as_raw_fd(), libc::LOCK_UN) };
        result
    }
}

fn private_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
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
}
