use rune_cfg::RuneConfig;

pub const MIN_FADE_DISTANCE: f32 = 120.0;
pub const MAX_FADE_DISTANCE: f32 = 100_000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bearings {
    pub show_distance: bool,
    pub show_icons: bool,
    pub show_pinned: bool,
    pub fade_distance: f32,
    pub blur: bool,
}

impl Default for Bearings {
    fn default() -> Self {
        Self {
            show_distance: true,
            show_icons: true,
            show_pinned: true,
            fade_distance: 1_200.0,
            blur: true,
        }
    }
}

pub fn parse_bearings(config: &RuneConfig) -> Bearings {
    let defaults = Bearings::default();
    Bearings {
        show_distance: config.get_or("bearings.show-distance", defaults.show_distance),
        show_icons: config.get_or("bearings.show-icons", defaults.show_icons),
        show_pinned: config.get_or("bearings.show-pinned", defaults.show_pinned),
        fade_distance: config
            .get_or("bearings.fade-distance", defaults.fade_distance)
            .clamp(MIN_FADE_DISTANCE, MAX_FADE_DISTANCE),
        blur: config.get_or("bearings.blur", defaults.blur),
    }
}
