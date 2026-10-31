use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The default config file contents are identical to the shipped top-level
/// example. Keeping one canonical template for the whole workspace prevents
/// fresh-install behavior and user-facing documentation from drifting apart.
pub const DEFAULT_CONFIG: &str = include_str!("../halley.default.rune");

/// Resolve the config file path: `$XDG_CONFIG_HOME/halley/halley.rune`,
/// falling back to `$HOME/.config/halley/halley.rune` when
/// `XDG_CONFIG_HOME` is unset. Returns `None` if neither env var is set -
/// no sensible home to put a config in.
pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("halley").join("halley.rune"))
}

/// If no config file exists yet at `config_path()`, write `DEFAULT_CONFIG`
/// there (creating parent directories as needed). Returns `Ok(true)` if a
/// file was actually written, `Ok(false)` if one already existed or the
/// path couldn't be resolved. Never overwrites an existing config - a
/// user's edits are never at risk from this.
pub fn bootstrap_default_config() -> io::Result<bool> {
    let Some(path) = config_path() else {
        return Ok(false);
    };
    bootstrap_default_config_at(&path)
}

/// Same as `bootstrap_default_config`, but against an explicit path - the
/// actual logic, factored out so it's testable against a temp directory
/// instead of the real `$HOME`.
pub fn bootstrap_default_config_at(path: &Path) -> io::Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Atomic create avoids truncating a config created concurrently, and
    // treats existing symlinks (including dangling ones) as user-owned paths.
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(error),
    };
    file.write_all(DEFAULT_CONFIG.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, uniquely-named scratch directory under the OS temp dir,
    /// scoped to one test so parallel test runs never collide.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(test_name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "halley-config-test-{}-{}-{}",
                std::process::id(),
                test_name,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).expect("create scratch dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn default_config_matches_shipped_example() {
        let example = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../examples/halley.rune"
        ))
        .expect("shipped example exists in the workspace");
        assert_eq!(DEFAULT_CONFIG, example);
    }

    #[test]
    fn writes_default_config_when_absent() {
        let scratch = ScratchDir::new("writes_default_config_when_absent");
        let config_file = scratch.path().join("halley").join("halley.rune");

        let wrote = bootstrap_default_config_at(&config_file).unwrap();

        assert!(wrote);
        assert!(config_file.exists());
        assert_eq!(fs::read_to_string(&config_file).unwrap(), DEFAULT_CONFIG);
    }

    #[test]
    fn does_not_overwrite_existing_config() {
        let scratch = ScratchDir::new("does_not_overwrite_existing_config");
        let config_file = scratch.path().join("halley").join("halley.rune");
        fs::create_dir_all(config_file.parent().unwrap()).unwrap();
        fs::write(&config_file, "keybinds:\n  mod \"alt\"\nend\n").unwrap();

        let wrote = bootstrap_default_config_at(&config_file).unwrap();

        assert!(!wrote);
        assert_eq!(
            fs::read_to_string(&config_file).unwrap(),
            "keybinds:\n  mod \"alt\"\nend\n"
        );
    }

    #[test]
    fn concurrent_bootstrap_has_only_one_writer() {
        let scratch = ScratchDir::new("concurrent_bootstrap_has_only_one_writer");
        let config_file = scratch.path().join("halley.rune");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let workers = (0..8)
            .map(|_| {
                let path = config_file.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    bootstrap_default_config_at(&path).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let writes = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(|wrote| *wrote)
            .count();
        assert_eq!(writes, 1);
        assert_eq!(fs::read_to_string(config_file).unwrap(), DEFAULT_CONFIG);
    }

    /// Milestone 4 boundary: the longer delays are not a migration. An existing
    /// config that states its own decay values is never rewritten, and a config
    /// that omits the section keeps Halley's built-in delays.
    #[test]
    fn existing_configs_keep_their_own_decay_values() {
        const EXISTING: &str = concat!(
            "decay:\n",
            "  enabled true\n",
            "  outside-delay-seconds 45\n",
            "  inside-delay-seconds 90\n",
            "end\n",
            "\n",
            "keybinds:\n",
            "  mod \"super\"\n",
            "end\n",
        );

        let scratch = ScratchDir::new("existing_configs_keep_their_own_decay_values");
        let config_file = scratch.path().join("halley").join("halley.rune");
        fs::create_dir_all(config_file.parent().unwrap()).unwrap();
        fs::write(&config_file, EXISTING).unwrap();

        let wrote = bootstrap_default_config_at(&config_file).unwrap();

        assert!(!wrote, "bootstrap must not write when a config exists");
        assert_eq!(
            fs::read_to_string(&config_file).unwrap(),
            EXISTING,
            "an existing decay section must stay byte-for-byte unchanged"
        );

        let runtime = crate::load_runtime_config_at(&config_file).expect("existing config loads");
        assert_eq!(runtime.decay.outside_delay_seconds, 45);
        assert_eq!(runtime.decay.inside_delay_seconds, 90);
    }
}

#[cfg(all(test, unix))]
mod symlink_tests {
    use super::*;
    #[test]
    fn bootstrap_preserves_a_dangling_user_symlink() {
        let root = std::env::temp_dir().join(format!(
            "halley-bootstrap-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let target = root.join("owned.rune");
        let config = root.join("halley.rune");
        std::os::unix::fs::symlink(&target, &config).unwrap();
        assert!(!bootstrap_default_config_at(&config).unwrap());
        assert_eq!(fs::read_link(&config).unwrap(), target);
        assert!(!target.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
