use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};

pub(crate) fn default_aperture_config_path() -> PathBuf {
    if let Ok(home) = env::var("XDG_CONFIG_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Path::new(trimmed).join("halley/aperture.rune");
        }
    }

    if let Ok(home) = env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Path::new(trimmed).join(".config/halley/aperture.rune");
        }
    }

    PathBuf::from("aperture.rune")
}

pub(crate) fn config_matches_event_path(event_path: &Path, targets: &[PathBuf]) -> bool {
    targets
        .iter()
        .any(|target| matches_path(event_path, target))
}

pub(crate) fn config_watch_roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for root in paths.iter().map(|path| {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    }) {
        if seen.insert(root.clone()) {
            out.push(root);
        }
    }
    out
}

fn matches_path(event_path: &Path, target_path: &Path) -> bool {
    event_path == target_path
        || target_path
            .file_name()
            .is_some_and(|name| event_path.file_name() == Some(name))
}
