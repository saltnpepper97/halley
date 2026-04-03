use halley_core::cluster_layout::ClusterWorkspaceLayoutKind;
use halley_core::viewport::FocusRing;
use regex::Regex;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeBorderColorMode {
    UseWindowActive,
    UseWindowInactive,
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
    Fixed { r: f32, g: f32, b: f32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShapeStyle {
    Square,
    Squircle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecorationBorderColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
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
            hide_after_ms: 2000,
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

