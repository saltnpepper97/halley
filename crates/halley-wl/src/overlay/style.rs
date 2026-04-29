use halley_config::{
    OverlayBorderSource, OverlayColorMode, OverlayShape, RuntimeTuning, ShadowLayerConfig,
};
use smithay::backend::renderer::Color32F;

#[derive(Clone, Copy)]
pub(crate) struct OverlayRgb {
    pub(crate) r: f32,
    pub(crate) g: f32,
    pub(crate) b: f32,
}

impl OverlayRgb {
    pub(crate) fn alpha(self, alpha: f32) -> Color32F {
        Color32F::new(self.r, self.g, self.b, alpha)
    }

    pub(crate) fn mix(self, other: Self, amount: f32) -> Self {
        let t = amount.clamp(0.0, 1.0);
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
        }
    }

    pub(crate) fn luminance(self) -> f32 {
        self.r * 0.2126 + self.g * 0.7152 + self.b * 0.0722
    }
}

#[derive(Clone, Copy)]
pub(crate) struct OverlayPalette {
    pub(crate) fill: OverlayRgb,
    pub(crate) text: OverlayRgb,
    pub(crate) subtext: OverlayRgb,
    pub(crate) key_fill: OverlayRgb,
    pub(crate) key_text: OverlayRgb,
    pub(crate) border: OverlayRgb,
}

#[derive(Clone, Copy)]
pub(crate) struct OverlayVisuals {
    pub(crate) rounded: bool,
    pub(crate) border_px: f32,
    pub(crate) shadow: ShadowLayerConfig,
    pub(crate) palette: OverlayPalette,
}

const LIGHT_OVERLAY_FILL: OverlayRgb = OverlayRgb {
    r: 0.92,
    g: 0.95,
    b: 0.98,
};
const DARK_OVERLAY_FILL: OverlayRgb = OverlayRgb {
    r: 0.15,
    g: 0.18,
    b: 0.22,
};
const LIGHT_OVERLAY_TEXT: OverlayRgb = OverlayRgb {
    r: 0.08,
    g: 0.10,
    b: 0.12,
};
const DARK_OVERLAY_TEXT: OverlayRgb = OverlayRgb {
    r: 0.94,
    g: 0.96,
    b: 0.98,
};

fn resolve_overlay_base_background(mode: OverlayColorMode) -> OverlayRgb {
    match mode {
        OverlayColorMode::Auto | OverlayColorMode::Light => LIGHT_OVERLAY_FILL,
        OverlayColorMode::Dark => DARK_OVERLAY_FILL,
        OverlayColorMode::Fixed { r, g, b } => OverlayRgb { r, g, b },
    }
}

fn resolve_overlay_base_text(mode: OverlayColorMode, background: OverlayRgb) -> OverlayRgb {
    match mode {
        OverlayColorMode::Auto => {
            if background.luminance() < 0.45 {
                DARK_OVERLAY_TEXT
            } else {
                LIGHT_OVERLAY_TEXT
            }
        }
        OverlayColorMode::Light => LIGHT_OVERLAY_TEXT,
        OverlayColorMode::Dark => DARK_OVERLAY_TEXT,
        OverlayColorMode::Fixed { r, g, b } => OverlayRgb { r, g, b },
    }
}

fn resolve_overlay_border_color(tuning: &RuntimeTuning) -> OverlayRgb {
    let color = match tuning.overlay_style.border_source {
        OverlayBorderSource::Primary => tuning.decorations.border.color_focused,
        OverlayBorderSource::Secondary => {
            if tuning.window_secondary_border_enabled() {
                tuning.decorations.secondary_border.color_focused
            } else {
                tuning.decorations.border.color_focused
            }
        }
    };
    OverlayRgb {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

fn resolve_overlay_border_width(tuning: &RuntimeTuning) -> f32 {
    if !tuning.overlay_style.borders {
        return 0.0;
    }
    match tuning.overlay_style.border_source {
        OverlayBorderSource::Primary => tuning.window_primary_border_size_px() as f32,
        OverlayBorderSource::Secondary => {
            if tuning.window_secondary_border_enabled() {
                tuning.window_secondary_border_size_px() as f32
            } else {
                tuning.window_primary_border_size_px() as f32
            }
        }
    }
}

pub(crate) fn resolve_overlay_visuals(tuning: &RuntimeTuning) -> OverlayVisuals {
    let fill = resolve_overlay_base_background(tuning.overlay_style.background_color);
    let text = resolve_overlay_base_text(tuning.overlay_style.text_color, fill);
    let border = resolve_overlay_border_color(tuning);
    OverlayVisuals {
        rounded: matches!(tuning.overlay_style.shape, OverlayShape::Rounded),
        border_px: resolve_overlay_border_width(tuning),
        shadow: tuning.decorations.shadows.overlay,
        palette: OverlayPalette {
            fill,
            text,
            subtext: text.mix(fill, 0.20),
            key_fill: fill.mix(text, 0.10),
            key_text: text,
            border,
        },
    }
}

pub(crate) fn overlay_fill_and_text_colors(tuning: &RuntimeTuning) -> (Color32F, Color32F) {
    let visuals = resolve_overlay_visuals(tuning);
    (
        visuals.palette.fill.alpha(1.0),
        visuals.palette.text.alpha(1.0),
    )
}

pub(crate) fn color_luminance(color: Color32F) -> f32 {
    color.r() * 0.2126 + color.g() * 0.7152 + color.b() * 0.0722
}

pub(crate) fn overlay_text_color_for_fill(fill: Color32F, alpha: f32) -> Color32F {
    if color_luminance(fill) < 0.45 {
        DARK_OVERLAY_TEXT.alpha(alpha)
    } else {
        LIGHT_OVERLAY_TEXT.alpha(alpha)
    }
}

pub(crate) fn overlay_accent_fill(
    visuals: &OverlayVisuals,
    border_mix: f32,
    alpha: f32,
) -> Color32F {
    visuals
        .palette
        .fill
        .mix(visuals.palette.border, border_mix)
        .alpha(alpha)
}

pub(crate) fn overlay_text_mix(mix: f32) -> f32 {
    let t = ((mix - 0.10) / 0.90).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
