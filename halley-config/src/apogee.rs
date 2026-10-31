use rune_cfg::RuneConfig;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Apogee {
    pub enabled: bool,
    pub live_previews: bool,
    pub preview_max_fps: u32,
    pub transition_ms: u64,
    pub gap: f32,
    pub max_rows: u32,
    pub background_dim: f32,
}

impl Default for Apogee {
    fn default() -> Self {
        Self {
            enabled: true,
            live_previews: true,
            preview_max_fps: 30,
            transition_ms: 320,
            gap: 24.0,
            max_rows: 3,
            background_dim: 0.85,
        }
    }
}

pub fn parse_apogee(config: &RuneConfig) -> Apogee {
    let defaults = Apogee::default();
    Apogee {
        enabled: config.get_or("apogee.enabled", defaults.enabled),
        live_previews: config.get_or("apogee.live-previews", defaults.live_previews),
        preview_max_fps: config
            .get_or("apogee.preview-max-fps", defaults.preview_max_fps)
            .clamp(1, 240),
        transition_ms: config
            .get_or("apogee.transition-ms", defaults.transition_ms)
            .clamp(0, 10_000),
        gap: config.get_or("apogee.gap", defaults.gap).clamp(0.0, 256.0),
        max_rows: config
            .get_or("apogee.max-rows", defaults.max_rows)
            .clamp(1, 5),
        background_dim: config
            .get_or("apogee.background-dim", defaults.background_dim)
            .clamp(0.0, 1.0),
    }
}
