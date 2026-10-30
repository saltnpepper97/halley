//! Runtime ownership of the selected configuration and its reload handle.

use std::path::{Path, PathBuf};

pub(crate) struct ConfigSource {
    path: Option<PathBuf>,
    watcher: Option<crate::config::ConfigWatcher>,
    startup_diagnostic: Option<halley_config::ConfigDiagnostic>,
}

impl ConfigSource {
    pub(crate) fn new(
        path: Option<PathBuf>,
        startup_diagnostic: Option<halley_config::ConfigDiagnostic>,
    ) -> Self {
        Self {
            path,
            watcher: None,
            startup_diagnostic,
        }
    }

    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub(crate) fn set_watcher(&mut self, watcher: crate::config::ConfigWatcher) {
        self.watcher = Some(watcher);
    }

    pub(crate) fn request_reload(&self) -> bool {
        let Some(watcher) = &self.watcher else {
            return false;
        };
        watcher.request_reload();
        true
    }

    pub(crate) fn take_startup_error(&mut self) -> bool {
        self.startup_diagnostic.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigSource;
    use std::path::{Path, PathBuf};

    #[test]
    fn startup_error_is_reported_once_and_reload_requires_a_watcher() {
        let path = PathBuf::from("/example/config.rune");
        let diagnostic = halley_config::ConfigDiagnostic::message(Some(path.clone()), "invalid");
        let mut source = ConfigSource::new(Some(path), Some(diagnostic));

        assert_eq!(source.path(), Some(Path::new("/example/config.rune")));
        assert!(source.take_startup_error());
        assert!(!source.take_startup_error());
        assert!(!source.request_reload());
    }
}
