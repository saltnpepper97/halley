#![allow(
    clippy::result_large_err,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

pub mod animation;
#[cfg(feature = "aperture")]
pub(crate) mod aperture;
pub(crate) mod diagnostics;
#[cfg(not(feature = "aperture"))]
pub(crate) mod aperture {
    use std::collections::HashSet;
    use std::env;
    use std::path::{Path, PathBuf};

    use crate::compositor::root::Halley;

    pub(crate) mod core {
        #[allow(dead_code)]
        #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
        pub(crate) enum ApertureMode {
            #[default]
            Hidden,
        }

        #[derive(Clone, Debug, Default, PartialEq)]
        pub(crate) struct ApertureConfig;

        #[allow(dead_code)]
        #[derive(Clone, Copy, Debug, Default, PartialEq)]
        pub(crate) struct Rect {
            pub(crate) x: f32,
            pub(crate) y: f32,
            pub(crate) w: f32,
            pub(crate) h: f32,
        }

        #[allow(dead_code)]
        #[derive(Clone, Copy, Debug, Default, PartialEq)]
        pub(crate) struct Size {
            pub(crate) w: f32,
            pub(crate) h: f32,
        }

        #[allow(dead_code)]
        #[derive(Clone, Copy, Debug, Default, PartialEq)]
        pub(crate) struct ClockSnapshot;
    }

    pub(crate) struct ApertureState {
        config: core::ApertureConfig,
    }

    impl ApertureState {
        pub(crate) fn new(config: core::ApertureConfig, _now: std::time::Instant) -> Self {
            Self { config }
        }

        pub(crate) fn apply_config(&mut self, config: core::ApertureConfig) {
            self.config = config;
        }

        #[allow(dead_code)]
        pub(crate) fn config(&self) -> &core::ApertureConfig {
            &self.config
        }

        #[allow(dead_code)]
        pub(crate) fn snapshot_for_mode<F>(
            &self,
            _mode: core::ApertureMode,
            _output_rect: core::Rect,
            _work_area_rect: core::Rect,
            _scale: f64,
            _measure_text: F,
        ) -> Option<core::ClockSnapshot>
        where
            F: FnMut(u32, &str) -> core::Size,
        {
            None
        }

        pub(crate) fn invalidate_mode_cache(&mut self) {}
    }

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

    pub(crate) fn load_aperture_config_from_path(_path: &Path) -> core::ApertureConfig {
        core::ApertureConfig
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

    pub(crate) fn reload_aperture_config(_st: &mut Halley, _path: &Path, _reason: &str) -> bool {
        false
    }

    pub(crate) fn aperture_status(_st: &Halley) -> halley_api::ApertureStatusResponse {
        halley_api::ApertureStatusResponse {
            output: None,
            mode: halley_api::ApertureMode::Hidden,
            outputs: Vec::new(),
        }
    }

    pub(crate) fn small_reservation_px_for_monitor(_st: &Halley, _monitor: &str) -> i32 {
        0
    }

    pub(crate) fn monitor_minimal_aperture_intended(_st: &Halley, _monitor: &str) -> bool {
        false
    }

    pub(crate) fn accepted_minimal_aperture_tab_height_px(
        _st: &Halley,
        _height_px: i32,
    ) -> Option<i32> {
        None
    }

    pub(crate) fn log_aperture_config_startup(_path: &PathBuf) {}
}
pub(crate) mod app_env;
pub(crate) mod backend;
pub mod bootstrap;
pub(crate) mod compositor;
pub(crate) mod frame_loop;
pub(crate) mod input;
pub(crate) mod ipc;
pub(crate) mod overlay;
pub(crate) mod perf;
pub(crate) mod portal;
pub(crate) mod presentation;
pub(crate) mod protocol;
pub mod render;
pub(crate) mod spatial;
pub(crate) mod text;
pub(crate) mod window;

pub use bootstrap::{run, run_nested, run_session, run_winit};
