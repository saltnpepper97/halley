use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::error::Error;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use calloop::LoopHandle;
use calloop::channel::{Event, sync_channel};

const POLLING_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, PartialEq, Eq)]
enum FileProps {
    Present {
        canonical: PathBuf,
        modified: SystemTime,
        len: u64,
        device: u64,
        inode: u64,
    },
    Unavailable(String),
}

impl FileProps {
    fn read(path: &Path) -> Self {
        let result = (|| {
            let canonical = path.canonicalize()?;
            let metadata = canonical.metadata()?;
            Ok::<_, std::io::Error>(Self::Present {
                canonical,
                modified: metadata.modified()?,
                len: metadata.len(),
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        })();
        result.unwrap_or_else(|error| {
            Self::Unavailable(format!(
                "{:?}:{}",
                error.kind(),
                error.raw_os_error().unwrap_or(0)
            ))
        })
    }
}

struct ConfigFileState {
    path: PathBuf,
    last_props: BTreeMap<PathBuf, FileProps>,
}

impl ConfigFileState {
    fn new(path: PathBuf) -> Self {
        let last_props = config_tree_props(&path);
        Self { path, last_props }
    }

    fn changed(&mut self) -> bool {
        let props = config_tree_props(&self.path);
        if self.last_props == props {
            return false;
        }
        self.last_props = props;
        true
    }
}

fn gather_path(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("gather")?.trim();
    let quote = rest.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let after_quote = &rest[quote.len_utf8()..];
    let end = after_quote.find(quote)?;
    Some(&after_quote[..end])
}

fn resolve_gather_path(raw: &str, base: &Path) -> Option<PathBuf> {
    let path = if let Some(rest) = raw.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME")?).join(rest)
    } else {
        PathBuf::from(raw)
    };
    Some(if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}

fn discover_config_tree(root: &Path) -> BTreeSet<PathBuf> {
    fn visit(path: PathBuf, files: &mut BTreeSet<PathBuf>, visited: &mut HashSet<PathBuf>) {
        let identity = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !visited.insert(identity) {
            return;
        }
        files.insert(path.clone());
        let Ok(source) = fs::read_to_string(&path) else {
            return;
        };
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        for dependency in source
            .lines()
            .filter_map(gather_path)
            .filter_map(|raw| resolve_gather_path(raw, base))
        {
            visit(dependency, files, visited);
        }
    }

    let mut files = BTreeSet::new();
    visit(root.to_path_buf(), &mut files, &mut HashSet::new());
    files
}

fn config_tree_props(root: &Path) -> BTreeMap<PathBuf, FileProps> {
    discover_config_tree(root)
        .into_iter()
        .map(|path| {
            let props = FileProps::read(&path);
            (path, props)
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct InitialConfig {
    pub path: Option<PathBuf>,
    pub config: halley_config::RuntimeConfig,
    pub diagnostic: Option<halley_config::ConfigDiagnostic>,
    /// True only when this startup created the configuration file from the
    /// built-in template. The one-time basics card is owed to exactly those
    /// configurations; an existing or explicitly selected file never sets it.
    pub fresh: bool,
}

#[derive(Clone, Debug)]
pub enum ConfigReload {
    Loaded(Box<halley_config::RuntimeConfig>),
    Rejected(halley_config::ConfigDiagnostic),
}

#[derive(Clone)]
pub struct ConfigWatcher {
    reload_requested: Arc<AtomicBool>,
}

impl ConfigWatcher {
    pub fn request_reload(&self) {
        self.reload_requested.store(true, Ordering::Release);
    }
}

/// Resolve a user-supplied path without requiring the file to exist.
///
/// Keeping the lexical path, rather than canonicalizing it, lets Halley
/// report and watch an explicitly selected file that will be created later.
pub fn absolute_path(path: PathBuf) -> std::io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn bootstrap_implicit_config(path: &Path) -> std::io::Result<bool> {
    halley_config::bootstrap_default_config_at(path)
}

/// Loads the initial validated snapshot and retains the exact path that later
/// reloads and `halleyctl config verify` must use.
pub fn load_initial(explicit_path: Option<PathBuf>) -> InitialConfig {
    let explicit = explicit_path.is_some();
    let selected = match explicit_path {
        Some(path) => absolute_path(path).map(Some),
        None => match halley_config::config_path() {
            Some(path) => absolute_path(path).map(Some),
            None => Ok(None),
        },
    };

    let path = match selected {
        Ok(path) => path,
        Err(error) => {
            let diagnostic = halley_config::ConfigDiagnostic::message(
                None,
                format!("could not resolve the configuration path: {error}"),
            );
            eventline::warn!("config: {}, using defaults", diagnostic.message);
            return InitialConfig {
                path: None,
                config: halley_config::RuntimeConfig::default(),
                diagnostic: Some(diagnostic),
                fresh: false,
            };
        }
    };

    let Some(path) = path else {
        let diagnostic = halley_config::ConfigDiagnostic::message(
            None,
            "no configuration path is available because HOME and XDG_CONFIG_HOME are unset",
        );
        eventline::warn!("config: {}, using defaults", diagnostic.message);
        return InitialConfig {
            path: None,
            config: halley_config::RuntimeConfig::default(),
            diagnostic: Some(diagnostic),
            fresh: false,
        };
    };

    load_resolved(path, !explicit)
}

/// Loads an already-resolved path, optionally bootstrapping the default config
/// first. Split out from [`load_initial`] so the fresh-config signal can be
/// tested without mutating process environment variables.
fn load_resolved(path: PathBuf, bootstrap: bool) -> InitialConfig {
    let mut fresh = false;
    if bootstrap {
        match bootstrap_implicit_config(&path) {
            Ok(wrote) => fresh = wrote,
            Err(error) => eventline::warn!("config: failed to bootstrap default config: {error}"),
        }
    }

    match halley_config::load_runtime_config_diagnostic_at(&path) {
        Ok(config) => InitialConfig {
            path: Some(path),
            config,
            diagnostic: None,
            fresh,
        },
        Err(diagnostic) => {
            eventline::warn!(
                "config: failed to load {:?}, using defaults: {}",
                diagnostic.path,
                diagnostic.message
            );
            InitialConfig {
                path: Some(path),
                config: halley_config::RuntimeConfig::default(),
                diagnostic: Some(diagnostic),
                fresh,
            }
        }
    }
}

/// Polls the selected file identity and mtime on a background thread, parses
/// there, and delivers both valid snapshots and rejected attempts onto the
/// compositor event loop.
pub fn watch<App: 'static>(
    loop_handle: &LoopHandle<'_, App>,
    path: PathBuf,
    mut notify: impl FnMut(&mut App, ConfigReload) + 'static,
) -> Result<ConfigWatcher, Box<dyn Error>> {
    let (sender, receiver) = sync_channel(1);
    let reload_requested = Arc::new(AtomicBool::new(false));
    let watcher_reload_requested = reload_requested.clone();
    thread::Builder::new()
        .name(format!("halley config watcher for {path:?}"))
        .spawn(move || {
            let mut state = ConfigFileState::new(path);
            loop {
                thread::sleep(POLLING_INTERVAL);
                let forced = watcher_reload_requested.swap(false, Ordering::AcqRel);
                let changed = state.changed();
                if !forced && !changed {
                    continue;
                }
                let loaded = halley_config::load_runtime_config_diagnostic_at(&state.path);
                if sender.send(loaded).is_err() {
                    break;
                }
            }
        })?;

    loop_handle.insert_source(receiver, move |event, _, app| {
        if let Event::Msg(loaded) = event {
            notify(app, classify_reload(loaded));
        }
    })?;
    Ok(ConfigWatcher { reload_requested })
}

fn classify_reload(
    loaded: Result<halley_config::RuntimeConfig, halley_config::ConfigDiagnostic>,
) -> ConfigReload {
    match loaded {
        Ok(config) => ConfigReload::Loaded(Box::new(config)),
        Err(diagnostic) => {
            eventline::warn!(
                "config: reload rejected, keeping last valid config: {}",
                diagnostic.message
            );
            ConfigReload::Rejected(diagnostic)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File, FileTimes};
    use std::time::UNIX_EPOCH;

    struct ScratchFile {
        path: PathBuf,
    }

    impl ScratchFile {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("halley-config-watch-{}-{name}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Self {
                path: dir.join("halley.rune"),
            }
        }
    }

    impl Drop for ScratchFile {
        fn drop(&mut self) {
            if let Some(parent) = self.path.parent() {
                let _ = fs::remove_dir_all(parent);
            }
        }
    }

    #[test]
    fn file_state_detects_create_delete_and_replacement_but_not_unchanged_file() {
        let scratch = ScratchFile::new("props");
        let mut state = ConfigFileState::new(scratch.path.clone());
        assert!(!state.changed());

        fs::write(&scratch.path, "first").unwrap();
        File::options()
            .write(true)
            .open(&scratch.path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1)))
            .unwrap();
        assert!(state.changed());
        assert!(!state.changed());

        fs::remove_file(&scratch.path).unwrap();
        assert!(state.changed());
        assert!(!state.changed());

        let replacement = scratch.path.with_extension("new");
        fs::write(&replacement, "second").unwrap();
        File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(2)))
            .unwrap();
        fs::rename(&replacement, &scratch.path).unwrap();
        assert!(state.changed());
    }

    #[test]
    fn file_state_follows_nested_gathers_and_refreshes_the_dependency_graph() {
        let scratch = ScratchFile::new("gathers");
        let directory = scratch.path.parent().unwrap();
        let child = directory.join("input.rune");
        let nested = directory.join("keys.rune");
        let replacement = directory.join("replacement.rune");
        fs::write(&scratch.path, "gather \"input.rune\"\n").unwrap();
        fs::write(&child, "gather \"keys.rune\"\n").unwrap();
        fs::write(&nested, "keybinds:\nend\n").unwrap();

        let mut state = ConfigFileState::new(scratch.path.clone());
        assert_eq!(state.last_props.len(), 3);
        assert!(!state.changed());

        fs::write(&nested, "keybinds:\n  mod \"super\"\nend\n").unwrap();
        assert!(state.changed(), "nested gather changes must trigger reload");
        assert!(!state.changed());

        fs::write(&child, "gather \"replacement.rune\"\n").unwrap();
        assert!(state.changed());
        assert!(
            state.last_props.contains_key(&replacement),
            "missing dependencies remain watched for creation"
        );
        assert!(!state.last_props.contains_key(&nested));

        fs::write(&replacement, "input:\nend\n").unwrap();
        assert!(state.changed());
        fs::write(&nested, "this dependency was removed").unwrap();
        assert!(
            !state.changed(),
            "removed dependencies stop triggering reloads"
        );
    }

    #[test]
    fn implicit_startup_never_rewrites_an_existing_config() {
        let scratch = ScratchFile::new("startup-existing");
        let original = format!(
            "{}\n# user-owned trailing comment\n",
            halley_config::DEFAULT_CONFIG
        );
        fs::write(&scratch.path, &original).unwrap();

        assert!(!bootstrap_implicit_config(&scratch.path).unwrap());
        assert_eq!(fs::read_to_string(&scratch.path).unwrap(), original);
        assert_eq!(
            fs::read_dir(scratch.path.parent().unwrap())
                .unwrap()
                .count(),
            1,
            "startup must not create a migration backup or sidecar"
        );
    }

    #[test]
    fn explicit_missing_path_is_not_bootstrapped() {
        let scratch = ScratchFile::new("explicit");
        let initial = load_initial(Some(scratch.path.clone()));

        assert_eq!(initial.path.as_deref(), Some(scratch.path.as_path()));
        assert!(initial.diagnostic.is_some());
        assert!(
            !initial.fresh,
            "an explicitly selected path never owes a first-run card"
        );
        assert!(!scratch.path.exists());
    }

    /// The one-time basics card is owed to configurations Halley generated
    /// itself, so only a real bootstrap may report `fresh`.
    #[test]
    fn only_a_written_default_config_reports_fresh() {
        let scratch = ScratchFile::new("fresh-signal");

        let first = load_resolved(scratch.path.clone(), true);
        assert!(first.fresh);
        assert_eq!(first.config.keybinds, halley_config::Keybinds::default());
        assert!(scratch.path.exists());

        let second = load_resolved(scratch.path.clone(), true);
        assert!(
            !second.fresh,
            "an existing config is never reported as freshly generated"
        );

        let existing = load_resolved(scratch.path.clone(), false);
        assert!(!existing.fresh);
    }

    #[test]
    fn relative_paths_are_resolved_against_the_launch_directory() {
        let relative = PathBuf::from("relative/halley.rune");
        assert_eq!(
            absolute_path(relative.clone()).unwrap(),
            std::env::current_dir().unwrap().join(relative)
        );
    }

    #[test]
    fn rejected_live_reload_keeps_structured_diagnostics() {
        let diagnostic =
            halley_config::ConfigDiagnostic::message(None, "invalid keybind".to_string());
        assert!(matches!(
            classify_reload(Err(diagnostic)),
            ConfigReload::Rejected(_)
        ));
        assert!(matches!(
            classify_reload(Ok(halley_config::RuntimeConfig::default())),
            ConfigReload::Loaded(_)
        ));
    }
}
