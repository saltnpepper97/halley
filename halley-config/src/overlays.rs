use std::fmt;

use rune_cfg::RuneConfig;

pub const DEFAULT_SUCCESS_DURATION_MS: u64 = 4_000;
pub const DEFAULT_ERROR_DURATION_MS: u64 = 9_000;
pub const DEFAULT_ZOOM_INDICATOR_HOLD_DURATION_MS: u64 = 750;
pub const DEFAULT_ZOOM_INDICATOR_FADE_DURATION_MS: u64 = 180;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayColorMode {
    /// Halley's deterministic built-in palette and contrast behavior.
    Auto,
    /// Follow the desktop preference exposed by org.freedesktop.appearance.
    System,
    Light,
    Dark,
    Fixed {
        r: f32,
        g: f32,
        b: f32,
        a: f32,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SystemColorScheme {
    #[default]
    NoPreference,
    PreferDark,
    PreferLight,
}

impl OverlayColorMode {
    fn resolved_for_system(self, scheme: SystemColorScheme) -> Self {
        if self != Self::System {
            return self;
        }
        match scheme {
            SystemColorScheme::PreferDark => Self::Dark,
            SystemColorScheme::PreferLight => Self::Light,
            SystemColorScheme::NoPreference => Self::Auto,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationPosition {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Notifications {
    pub position: NotificationPosition,
    pub offset_x: i32,
    pub offset_y: i32,
    pub success_duration_ms: u64,
    pub error_duration_ms: u64,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            position: NotificationPosition::TopCenter,
            offset_x: 0,
            offset_y: 0,
            success_duration_ms: DEFAULT_SUCCESS_DURATION_MS,
            error_duration_ms: DEFAULT_ERROR_DURATION_MS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomIndicator {
    pub enabled: bool,
    pub position: NotificationPosition,
    pub hold_duration_ms: u64,
    pub fade_duration_ms: u64,
    pub background: bool,
    pub text_size: Option<u16>,
    pub text_color: Option<OverlayColorMode>,
    pub background_color: Option<OverlayColorMode>,
    pub border_color: Option<OverlayColorMode>,
    pub opacity: f32,
    pub borders: Option<bool>,
    pub radius_px: Option<i32>,
}

impl Default for ZoomIndicator {
    fn default() -> Self {
        Self {
            enabled: true,
            position: NotificationPosition::BottomCenter,
            hold_duration_ms: DEFAULT_ZOOM_INDICATOR_HOLD_DURATION_MS,
            fade_duration_ms: DEFAULT_ZOOM_INDICATOR_FADE_DURATION_MS,
            background: true,
            text_size: None,
            text_color: None,
            background_color: None,
            border_color: None,
            opacity: 1.0,
            borders: None,
            radius_px: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overlays {
    pub background_color: OverlayColorMode,
    pub text_color: OverlayColorMode,
    pub error_color: OverlayColorMode,
    pub border_color: OverlayColorMode,
    pub radius_px: i32,
    pub borders: bool,
    pub border_size_px: i32,
    pub notifications: Notifications,
    pub zoom_indicator: ZoomIndicator,
}

impl Default for Overlays {
    fn default() -> Self {
        Self {
            background_color: OverlayColorMode::Auto,
            text_color: OverlayColorMode::Auto,
            error_color: OverlayColorMode::Fixed {
                r: 0xfb as f32 / 255.0,
                g: 0x49 as f32 / 255.0,
                b: 0x34 as f32 / 255.0,
                a: 1.0,
            },
            border_color: OverlayColorMode::Fixed {
                r: 0xd6 as f32 / 255.0,
                g: 0x5d as f32 / 255.0,
                b: 0x26 as f32 / 255.0,
                a: 1.0,
            },
            radius_px: 8,
            borders: true,
            border_size_px: 3,
            notifications: Notifications::default(),
            zoom_indicator: ZoomIndicator::default(),
        }
    }
}

impl Overlays {
    /// Resolve the explicit system mode once at the session boundary.
    ///
    /// Renderers then receive a deterministic palette and never perform D-Bus
    /// work or depend on desktop-global state.
    pub fn resolved_for_system(mut self, scheme: SystemColorScheme) -> Self {
        self.background_color = self.background_color.resolved_for_system(scheme);
        self.text_color = self.text_color.resolved_for_system(scheme);
        self.error_color = self.error_color.resolved_for_system(scheme);
        self.border_color = self.border_color.resolved_for_system(scheme);
        self.zoom_indicator.text_color = self
            .zoom_indicator
            .text_color
            .map(|mode| mode.resolved_for_system(scheme));
        self.zoom_indicator.background_color = self
            .zoom_indicator
            .background_color
            .map(|mode| mode.resolved_for_system(scheme));
        self.zoom_indicator.border_color = self
            .zoom_indicator
            .border_color
            .map(|mode| mode.resolved_for_system(scheme));
        self
    }
}

#[derive(Debug)]
pub enum OverlayParseError {
    Rune(rune_cfg::RuneError),
    InvalidValue { path: &'static str, value: String },
    InvalidDuration { path: &'static str, value: u64 },
}

impl fmt::Display for OverlayParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rune(error) => write!(f, "{error}"),
            Self::InvalidValue { path, value } => {
                write!(f, "invalid value {value:?} for {path}")
            }
            Self::InvalidDuration { path, value } => {
                write!(f, "{path} must be greater than zero, got {value}")
            }
        }
    }
}

impl std::error::Error for OverlayParseError {}

impl From<rune_cfg::RuneError> for OverlayParseError {
    fn from(value: rune_cfg::RuneError) -> Self {
        Self::Rune(value)
    }
}

pub fn parse_overlays_checked(config: &RuneConfig) -> Result<Overlays, OverlayParseError> {
    let defaults = Overlays::default();
    let background_color = parse_color(
        optional_string(
            config,
            &[
                "overlays.background-colour",
                "overlays.background-color",
                "overlay.background-colour",
                "overlay.background-color",
            ],
        )?,
        "overlays.background-colour",
        defaults.background_color,
    )?;
    let text_color = parse_color(
        optional_string(
            config,
            &[
                "overlays.text-colour",
                "overlays.text-color",
                "overlay.text-colour",
                "overlay.text-color",
            ],
        )?,
        "overlays.text-colour",
        defaults.text_color,
    )?;
    let error_color = parse_color(
        optional_string(
            config,
            &[
                "overlays.error-colour",
                "overlays.error-color",
                "overlay.error-colour",
                "overlay.error-color",
            ],
        )?,
        "overlays.error-colour",
        defaults.error_color,
    )?;
    let border_color = parse_color(
        optional_string(
            config,
            &[
                "overlays.border-colour",
                "overlays.border-color",
                "overlay.border-colour",
                "overlay.border-color",
            ],
        )?,
        "overlays.border-colour",
        defaults.border_color,
    )?;
    let radius_px = optional_i32(config, &["overlays.radius", "overlay.radius"])?
        .unwrap_or(defaults.radius_px)
        .clamp(0, 256);
    let notification_position = parse_position(
        optional_string(
            config,
            &[
                "overlays.notifications.position",
                "overlay.notifications.position",
            ],
        )?,
        "overlays.notifications.position",
        defaults.notifications.position,
    )?;
    let zoom_indicator_position = parse_position(
        optional_string(config, &["overlays.zoom-indicator.position"])?,
        "overlays.zoom-indicator.position",
        defaults.zoom_indicator.position,
    )?;
    let zoom_indicator_hold_duration_ms =
        optional_u64(config, &["overlays.zoom-indicator.hold-duration-ms"])?
            .unwrap_or(defaults.zoom_indicator.hold_duration_ms);
    let zoom_indicator_fade_duration_ms =
        optional_u64(config, &["overlays.zoom-indicator.fade-duration-ms"])?
            .unwrap_or(defaults.zoom_indicator.fade_duration_ms);
    let zoom_indicator_text_color = parse_optional_color(
        optional_string(
            config,
            &[
                "overlays.zoom-indicator.text-colour",
                "overlays.zoom-indicator.text-color",
            ],
        )?,
        "overlays.zoom-indicator.text-colour",
    )?;
    let zoom_indicator_background_color = parse_optional_color(
        optional_string(
            config,
            &[
                "overlays.zoom-indicator.background-colour",
                "overlays.zoom-indicator.background-color",
            ],
        )?,
        "overlays.zoom-indicator.background-colour",
    )?;
    let zoom_indicator_border_color = parse_optional_color(
        optional_string(
            config,
            &[
                "overlays.zoom-indicator.border-colour",
                "overlays.zoom-indicator.border-color",
            ],
        )?,
        "overlays.zoom-indicator.border-colour",
    )?;
    let success_duration_ms = optional_u64(
        config,
        &[
            "overlays.notifications.success-duration-ms",
            "overlay.notifications.success-duration-ms",
        ],
    )?
    .unwrap_or(defaults.notifications.success_duration_ms);
    let error_duration_ms = optional_u64(
        config,
        &[
            "overlays.notifications.error-duration-ms",
            "overlay.notifications.error-duration-ms",
        ],
    )?
    .unwrap_or(defaults.notifications.error_duration_ms);
    validate_duration(
        "overlays.notifications.success-duration-ms",
        success_duration_ms,
    )?;
    validate_duration(
        "overlays.notifications.error-duration-ms",
        error_duration_ms,
    )?;
    validate_duration(
        "overlays.zoom-indicator.hold-duration-ms",
        zoom_indicator_hold_duration_ms,
    )?;
    validate_duration(
        "overlays.zoom-indicator.fade-duration-ms",
        zoom_indicator_fade_duration_ms,
    )?;

    Ok(Overlays {
        background_color,
        text_color,
        error_color,
        border_color,
        radius_px,
        borders: optional_bool(config, &["overlays.borders", "overlay.borders"])?
            .unwrap_or(defaults.borders),
        border_size_px: optional_i32(config, &["overlays.border-size", "overlay.border-size"])?
            .unwrap_or(defaults.border_size_px)
            .clamp(0, 64),
        notifications: Notifications {
            position: notification_position,
            offset_x: optional_i32(
                config,
                &[
                    "overlays.notifications.offset-x",
                    "overlay.notifications.offset-x",
                ],
            )?
            .unwrap_or(defaults.notifications.offset_x),
            offset_y: optional_i32(
                config,
                &[
                    "overlays.notifications.offset-y",
                    "overlay.notifications.offset-y",
                ],
            )?
            .unwrap_or(defaults.notifications.offset_y),
            success_duration_ms,
            error_duration_ms,
        },
        zoom_indicator: ZoomIndicator {
            enabled: optional_bool(config, &["overlays.zoom-indicator.enabled"])?
                .unwrap_or(defaults.zoom_indicator.enabled),
            position: zoom_indicator_position,
            hold_duration_ms: zoom_indicator_hold_duration_ms,
            fade_duration_ms: zoom_indicator_fade_duration_ms,
            background: optional_bool(config, &["overlays.zoom-indicator.background"])?
                .unwrap_or(defaults.zoom_indicator.background),
            text_size: optional_u64(config, &["overlays.zoom-indicator.text-size"])?
                .map(|size| size.clamp(6, 96) as u16),
            text_color: zoom_indicator_text_color,
            background_color: zoom_indicator_background_color,
            border_color: zoom_indicator_border_color,
            opacity: optional_f32(config, &["overlays.zoom-indicator.opacity"])?
                .filter(|value| value.is_finite())
                .unwrap_or(defaults.zoom_indicator.opacity)
                .clamp(0.0, 1.0),
            borders: optional_bool(config, &["overlays.zoom-indicator.borders"])?,
            radius_px: optional_i32(config, &["overlays.zoom-indicator.radius"])?
                .map(|radius| radius.clamp(0, 256)),
        },
    })
}

fn parse_position(
    raw: Option<String>,
    path: &'static str,
    default: NotificationPosition,
) -> Result<NotificationPosition, OverlayParseError> {
    match raw.as_deref().map(str::to_ascii_lowercase).as_deref() {
        None => Ok(default),
        Some("top-left") => Ok(NotificationPosition::TopLeft),
        Some("top-center") => Ok(NotificationPosition::TopCenter),
        Some("top-right") => Ok(NotificationPosition::TopRight),
        Some("bottom-left") => Ok(NotificationPosition::BottomLeft),
        Some("bottom-center") => Ok(NotificationPosition::BottomCenter),
        Some("bottom-right") => Ok(NotificationPosition::BottomRight),
        Some(value) => Err(OverlayParseError::InvalidValue {
            path,
            value: value.to_string(),
        }),
    }
}

fn optional_string(
    config: &RuneConfig,
    paths: &[&str],
) -> Result<Option<String>, rune_cfg::RuneError> {
    for path in paths {
        if let Some(value) = config.get_optional::<String>(path)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn optional_bool(config: &RuneConfig, paths: &[&str]) -> Result<Option<bool>, rune_cfg::RuneError> {
    for path in paths {
        if let Some(value) = config.get_optional::<bool>(path)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn optional_u64(config: &RuneConfig, paths: &[&str]) -> Result<Option<u64>, rune_cfg::RuneError> {
    for path in paths {
        if let Some(value) = config.get_optional::<u64>(path)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn optional_f32(config: &RuneConfig, paths: &[&str]) -> Result<Option<f32>, rune_cfg::RuneError> {
    for path in paths {
        if let Some(value) = config.get_optional::<f32>(path)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn optional_i32(config: &RuneConfig, paths: &[&str]) -> Result<Option<i32>, rune_cfg::RuneError> {
    for path in paths {
        if let Some(value) = config.get_optional::<i32>(path)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn parse_color(
    raw: Option<String>,
    path: &'static str,
    default: OverlayColorMode,
) -> Result<OverlayColorMode, OverlayParseError> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(OverlayColorMode::Auto),
        "system" => Ok(OverlayColorMode::System),
        "light" => Ok(OverlayColorMode::Light),
        "dark" => Ok(OverlayColorMode::Dark),
        value => parse_hex_color(value).ok_or(OverlayParseError::InvalidValue { path, value: raw }),
    }
}

fn parse_optional_color(
    raw: Option<String>,
    path: &'static str,
) -> Result<Option<OverlayColorMode>, OverlayParseError> {
    raw.map(|raw| parse_color(Some(raw), path, OverlayColorMode::Auto))
        .transpose()
}

fn parse_hex_color(value: &str) -> Option<OverlayColorMode> {
    let hex = value.strip_prefix('#')?;
    let (r, g, b, a) = match hex.len() {
        3 => (
            expand_nibble(&hex[0..1])?,
            expand_nibble(&hex[1..2])?,
            expand_nibble(&hex[2..3])?,
            255,
        ),
        4 => (
            expand_nibble(&hex[0..1])?,
            expand_nibble(&hex[1..2])?,
            expand_nibble(&hex[2..3])?,
            expand_nibble(&hex[3..4])?,
        ),
        6 => (
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
            255,
        ),
        8 => (
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
            u8::from_str_radix(&hex[6..8], 16).ok()?,
        ),
        _ => return None,
    };
    Some(OverlayColorMode::Fixed {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    })
}

fn expand_nibble(value: &str) -> Option<u8> {
    u8::from_str_radix(value, 16).ok().map(|value| value * 17)
}

fn validate_duration(path: &'static str, value: u64) -> Result<(), OverlayParseError> {
    if value == 0 {
        Err(OverlayParseError::InvalidDuration { path, value })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shared_style_and_notification_settings() {
        let config = RuneConfig::from_str(
            r##"
overlays:
  background-colour "#1238"
  text-color "dark"
  error-colour "#fb4934"
  border-color "#d65d26"
  radius 12
  borders false
  border-size 5
  notifications:
    position "bottom-right"
    offset-x -40
    offset-y 20
    success-duration-ms 1500
    error-duration-ms 12000
  end
  zoom-indicator:
    enabled false
    position "top-left"
    hold-duration-ms 600
    fade-duration-ms 240
    background false
    text-size 18
    text-color "#f0c8"
    background-colour "#ffffff80"
    border-colour "system"
    opacity 0.8
    borders true
    radius 20
  end
end
"##,
        )
        .unwrap();
        let overlays = parse_overlays_checked(&config).unwrap();

        assert_eq!(overlays.radius_px, 12);
        assert!(!overlays.borders);
        assert_eq!(overlays.border_size_px, 5);
        assert_eq!(
            overlays.notifications.position,
            NotificationPosition::BottomRight
        );
        assert_eq!(overlays.notifications.offset_x, -40);
        assert_eq!(overlays.notifications.offset_y, 20);
        assert_eq!(overlays.notifications.success_duration_ms, 1_500);
        assert_eq!(overlays.notifications.error_duration_ms, 12_000);
        assert!(!overlays.zoom_indicator.enabled);
        assert_eq!(
            overlays.zoom_indicator.position,
            NotificationPosition::TopLeft
        );
        assert_eq!(overlays.zoom_indicator.hold_duration_ms, 600);
        assert_eq!(overlays.zoom_indicator.fade_duration_ms, 240);
        assert!(!overlays.zoom_indicator.background);
        assert_eq!(overlays.zoom_indicator.text_size, Some(18));
        assert!(matches!(
            overlays.zoom_indicator.text_color,
            Some(OverlayColorMode::Fixed { .. })
        ));
        assert!(matches!(
            overlays.zoom_indicator.background_color,
            Some(OverlayColorMode::Fixed { r, g, b, a })
                if r == 1.0
                    && g == 1.0
                    && b == 1.0
                    && (a - 128.0 / 255.0).abs() < f32::EPSILON
        ));
        assert_eq!(
            overlays.zoom_indicator.border_color,
            Some(OverlayColorMode::System)
        );
        assert_eq!(overlays.zoom_indicator.opacity, 0.8);
        assert_eq!(overlays.zoom_indicator.borders, Some(true));
        assert_eq!(overlays.zoom_indicator.radius_px, Some(20));
        assert!(matches!(
            overlays.background_color,
            OverlayColorMode::Fixed { .. }
        ));
        assert!(matches!(
            overlays.border_color,
            OverlayColorMode::Fixed { r, g, b, a }
                if (r - 0xd6 as f32 / 255.0).abs() < f32::EPSILON
                    && (g - 0x5d as f32 / 255.0).abs() < f32::EPSILON
                    && (b - 0x26 as f32 / 255.0).abs() < f32::EPSILON
                    && a == 1.0
        ));
    }
}
