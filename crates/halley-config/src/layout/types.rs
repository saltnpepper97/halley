use halley_core::cluster_layout::ClusterWorkspaceLayoutKind;
use halley_core::viewport::FocusRing;
use regex::Regex;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeBorderColorMode {
    UseWindowActive,
    UseWindowInactive,
    UseWindowSecondaryActive,
    UseWindowSecondaryInactive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeDisplayPolicy {
    Off,
    Hover,
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NodeBackgroundColorMode {
    Auto,
    Theme,
    Light,
    Dark,
    Fixed { r: f32, g: f32, b: f32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShapeStyle {
    Square,
    Squircle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayColorMode {
    Auto,
    Light,
    Dark,
    Fixed { r: f32, g: f32, b: f32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayShape {
    Square,
    Rounded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayBorderSource {
    Primary,
    Secondary,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayStyleConfig {
    pub background_color: OverlayColorMode,
    pub text_color: OverlayColorMode,
    pub shape: OverlayShape,
    pub borders: bool,
    pub border_source: OverlayBorderSource,
}

impl Default for OverlayStyleConfig {
    fn default() -> Self {
        Self {
            background_color: OverlayColorMode::Auto,
            text_color: OverlayColorMode::Auto,
            shape: OverlayShape::Square,
            borders: true,
            border_source: OverlayBorderSource::Primary,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimationToggleConfig {
    pub enabled: bool,
}

impl Default for AnimationToggleConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimedAnimationConfig {
    pub enabled: bool,
    pub duration_ms: u64,
}

impl TimedAnimationConfig {
    pub const fn new(enabled: bool, duration_ms: u64) -> Self {
        Self {
            enabled,
            duration_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowCloseAnimationStyle {
    Shrink,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowCloseAnimationConfig {
    pub enabled: bool,
    pub duration_ms: u64,
    pub style: WindowCloseAnimationStyle,
}

impl WindowCloseAnimationConfig {
    pub const fn new(enabled: bool, duration_ms: u64, style: WindowCloseAnimationStyle) -> Self {
        Self {
            enabled,
            duration_ms,
            style,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimationsConfig {
    pub enabled: bool,
    pub smooth_resize: TimedAnimationConfig,
    pub maximize: TimedAnimationConfig,
    pub window_close: WindowCloseAnimationConfig,
    pub window_open: TimedAnimationConfig,
    pub tile: TimedAnimationConfig,
    pub stack: TimedAnimationConfig,
}

impl Default for AnimationsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            smooth_resize: TimedAnimationConfig::new(true, 90),
            maximize: TimedAnimationConfig::new(true, 240),
            window_close: WindowCloseAnimationConfig::new(
                true,
                250,
                WindowCloseAnimationStyle::Shrink,
            ),
            window_open: TimedAnimationConfig::new(true, 620),
            tile: TimedAnimationConfig::new(true, 240),
            stack: TimedAnimationConfig::new(true, 220),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenshotConfig {
    pub directory: String,
    pub highlight_color: OverlayColorMode,
    pub background_color: OverlayColorMode,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Self {
            directory: format!("{home}/Pictures/Screenshots"),
            highlight_color: OverlayColorMode::Auto,
            background_color: OverlayColorMode::Auto,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecorationBorderColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrimaryBorderConfig {
    pub size_px: i32,
    pub radius_px: i32,
    pub color_focused: DecorationBorderColor,
    pub color_unfocused: DecorationBorderColor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SecondaryBorderConfig {
    pub enabled: bool,
    pub size_px: i32,
    pub gap_px: i32,
    pub color_focused: DecorationBorderColor,
    pub color_unfocused: DecorationBorderColor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecorationsConfig {
    pub border: PrimaryBorderConfig,
    pub secondary_border: SecondaryBorderConfig,
    pub resize_using_border: bool,
}

impl Default for PrimaryBorderConfig {
    fn default() -> Self {
        Self {
            size_px: 3,
            radius_px: 0,
            color_focused: DecorationBorderColor {
                r: 0.22,
                g: 0.82,
                b: 0.92,
            },
            color_unfocused: DecorationBorderColor {
                r: 0.28,
                g: 0.30,
                b: 0.35,
            },
        }
    }
}

impl Default for SecondaryBorderConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            size_px: 1,
            gap_px: 2,
            color_focused: DecorationBorderColor {
                r: 0.98,
                g: 0.74,
                b: 0.15,
            },
            color_unfocused: DecorationBorderColor {
                r: 0.12,
                g: 0.12,
                b: 0.12,
            },
        }
    }
}

impl Default for DecorationsConfig {
    fn default() -> Self {
        Self {
            border: PrimaryBorderConfig::default(),
            secondary_border: SecondaryBorderConfig::default(),
            resize_using_border: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanToNewMode {
    Never,
    IfNeeded,
    Always,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseRestorePanMode {
    Never,
    IfOffscreen,
    Always,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClickCollapsedOutsideFocusMode {
    Ignore,
    Activate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClickCollapsedPanMode {
    Never,
    IfOffscreen,
    Always,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputFocusMode {
    Click,
    Hover,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputConfig {
    pub repeat_rate: i32,
    pub repeat_delay: i32,
    pub focus_mode: InputFocusMode,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            repeat_rate: 30,
            repeat_delay: 500,
            focus_mode: InputFocusMode::Click,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusRingConfig {
    pub rx: f32,
    pub ry: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

impl FocusRingConfig {
    pub fn to_focus_ring(self) -> FocusRing {
        FocusRing::new(self.rx, self.ry, self.offset_x, self.offset_y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BearingsConfig {
    pub show_distance: bool,
    pub show_icons: bool,
    pub fade_distance: f32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorConfig {
    pub theme: String,
    pub size: u32,
    pub hide_while_typing: bool,
    pub hide_after_ms: u64,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            theme: "Adwaita".to_string(),
            size: 24,
            hide_while_typing: false,
            hide_after_ms: 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontConfig {
    pub family: String,
    pub size: u32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "monospace".to_string(),
            size: 11,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClusterBloomDirection {
    Clockwise,
    CounterClockwise,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClusterDefaultLayout {
    Tiling,
    Stacking,
}

impl ClusterDefaultLayout {
    pub fn to_workspace_layout_kind(self) -> ClusterWorkspaceLayoutKind {
        match self {
            Self::Tiling => ClusterWorkspaceLayoutKind::Tiling,
            Self::Stacking => ClusterWorkspaceLayoutKind::Stacking,
        }
    }
}

#[derive(Clone, Debug)]
pub enum WindowRulePattern {
    Exact(String),
    Regex(Regex),
}

impl PartialEq for WindowRulePattern {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Regex(a), Self::Regex(b)) => a.as_str() == b.as_str(),
            _ => false,
        }
    }
}

impl Eq for WindowRulePattern {}

impl WindowRulePattern {
    pub fn matches(&self, value: &str) -> bool {
        match self {
            Self::Exact(exact) => exact == value,
            Self::Regex(regex) => regex.is_match(value),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Exact(exact) => exact.as_str(),
            Self::Regex(regex) => regex.as_str(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialWindowOverlapPolicy {
    None,
    ParentOnly,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialWindowSpawnPlacement {
    Center,
    Adjacent,
    ViewportCenter,
    Cursor,
    App,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialWindowClusterParticipation {
    Layout,
    Float,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowRule {
    pub app_ids: Vec<WindowRulePattern>,
    pub titles: Vec<WindowRulePattern>,
    pub overlap_policy: InitialWindowOverlapPolicy,
    pub spawn_placement: InitialWindowSpawnPlacement,
    pub cluster_participation: InitialWindowClusterParticipation,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewportOutputConfig {
    pub connector: String,
    pub enabled: bool,
    pub offset_x: i32,
    pub offset_y: i32,
    pub width: u32,
    pub height: u32,
    pub refresh_rate: Option<f64>,
    pub transform_degrees: u16,
    pub vrr: ViewportVrrMode,
    pub focus_ring: Option<FocusRingConfig>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewportVrrMode {
    Off,
    On,
    OnDemand,
}

impl ViewportVrrMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::OnDemand => "on-demand",
        }
    }

    pub fn drm_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}
