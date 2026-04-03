use std::env;
use std::path::{Path, PathBuf};

pub(crate) fn default_config_path() -> PathBuf {
    if let Ok(home) = env::var("XDG_CONFIG_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Path::new(trimmed).join("halley/halley.rune");
        }
    }

    if let Ok(home) = env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Path::new(trimmed).join(".config/halley/halley.rune");
        }
    }

    PathBuf::from("halley.rune")
}

pub(crate) fn absolutize_path(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(p))
            .unwrap_or_else(|_| PathBuf::from(path))
    }
}
