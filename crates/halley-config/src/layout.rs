use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use crate::keybinds::{key_name_to_evdev, WHEEL_DOWN_CODE, WHEEL_UP_CODE};
use halley_core::decay::FocusRingDecayPolicy;
use halley_core::field::Vec2;
use halley_core::viewport::{FocusRing, Viewport};

use super::{
    BearingsBindingAction, CompositorBinding, CompositorBindingAction, DirectionalAction,
    KeyModifiers, Keybinds, LaunchBinding, NodeBindingAction, PointerBinding, PointerBindingAction,
    TrailBindingAction,
};

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

use regex::Regex;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClusterBloomDirection {
    Clockwise,
    CounterClockwise,
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

#[derive(Clone, Debug)]
pub struct RuntimeTuning {
    pub debug_tick_dump: bool,
    pub debug_dump_every_ms: u64,

    pub viewport_center: Vec2,
    pub viewport_size: Vec2,

    pub focus_ring_rx: f32,
    pub focus_ring_ry: f32,
    pub focus_ring_offset_x: f32,
    pub focus_ring_offset_y: f32,

    pub primary_hot_inner_frac: f32,
    pub primary_to_node_ms: u64,
    pub node_show_labels: NodeDisplayPolicy,
    pub node_show_app_icons: NodeDisplayPolicy,
    pub node_icon_size: f32,
    pub node_background_color: NodeBackgroundColorMode,
    pub node_border_color_hover: NodeBorderColorMode,
    pub node_border_color_inactive: NodeBorderColorMode,
    pub border_size_px: i32,
    pub border_radius_px: i32,
    pub border_color_focused: DecorationBorderColor,
    pub border_color_unfocused: DecorationBorderColor,
    pub resize_using_border: bool,
    pub click_collapsed_outside_focus: ClickCollapsedOutsideFocusMode,
    pub click_collapsed_pan: ClickCollapsedPanMode,
    pub bearings: BearingsConfig,

    pub dev_enabled: bool,
    pub dev_show_geometry_overlay: bool,
    pub dev_zoom_decay_enabled: bool,
    pub dev_zoom_decay_min_frac: f32,
    pub dev_anim_enabled: bool,
    pub dev_anim_state_change_ms: u64,
    pub dev_anim_bounce: f32,

    pub cluster_distance_px: f32,
    pub cluster_dwell_ms: u64,
    pub cluster_show_icons: bool,
    pub cluster_bloom_direction: ClusterBloomDirection,
    pub tile_gaps_inner_px: f32,
    pub tile_gaps_outer_px: f32,
    pub tile_queue_show_icons: bool,
    pub active_windows_allowed: usize,
    pub trail_history_length: usize,
    pub trail_wrap: bool,

    pub active_outside_ring_delay_ms: u64,
    pub inactive_outside_ring_delay_ms: u64,
    pub docked_offscreen_delay_ms: u64,

    pub non_overlap_gap_px: f32,
    pub pan_to_new: PanToNewMode,
    pub close_restore_focus: bool,
    pub close_restore_pan: CloseRestorePanMode,
    pub non_overlap_active_gap_scale: f32,
    pub non_overlap_bump_newer: bool,
    pub non_overlap_bump_damping: f32,
    pub drag_smoothing_boost: f32,
    pub center_window_to_mouse: bool,
    pub restore_last_active_on_pan_return: bool,
    pub physics_enabled: bool,
    pub no_csd: bool,
    pub window_rules: Vec<WindowRule>,

    pub keybinds: Keybinds,
    pub compositor_bindings: Vec<CompositorBinding>,
    pub launch_bindings: Vec<LaunchBinding>,
    pub pointer_bindings: Vec<PointerBinding>,

    pub tty_viewports: Vec<ViewportOutputConfig>,
    pub autostart_once: Vec<String>,
    pub autostart_on_reload: Vec<String>,
    pub env: HashMap<String, String>,
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

impl Default for RuntimeTuning {
    fn default() -> Self {
        Self {
            debug_tick_dump: false,
            debug_dump_every_ms: 1000,
            viewport_center: Vec2 { x: 0.0, y: 0.0 },
            viewport_size: Vec2 {
                x: 1920.0,
                y: 1080.0,
            },

            focus_ring_rx: 820.0,
            focus_ring_ry: 420.0,
            focus_ring_offset_x: 0.0,
            focus_ring_offset_y: 0.0,

            primary_hot_inner_frac: 0.88,
            primary_to_node_ms: 1_260_000,
            node_show_labels: NodeDisplayPolicy::Hover,
            node_show_app_icons: NodeDisplayPolicy::Always,
            node_icon_size: 0.72,
            node_background_color: NodeBackgroundColorMode::Auto,
            node_border_color_hover: NodeBorderColorMode::UseWindowActive,
            node_border_color_inactive: NodeBorderColorMode::UseWindowInactive,
            border_size_px: 3,
            border_radius_px: 0,
            border_color_focused: DecorationBorderColor {
                r: 0.22,
                g: 0.82,
                b: 0.92,
            },
            border_color_unfocused: DecorationBorderColor {
                r: 0.28,
                g: 0.30,
                b: 0.35,
            },
            resize_using_border: false,
            click_collapsed_outside_focus: ClickCollapsedOutsideFocusMode::Activate,
            click_collapsed_pan: ClickCollapsedPanMode::IfOffscreen,
            bearings: BearingsConfig {
                show_distance: true,
                show_icons: true,
                fade_distance: 1200.0,
            },

            dev_enabled: false,
            dev_show_geometry_overlay: false,
            dev_zoom_decay_enabled: true,
            dev_zoom_decay_min_frac: 0.05,
            dev_anim_enabled: true,
            dev_anim_state_change_ms: 360,
            dev_anim_bounce: 1.45,

            cluster_distance_px: 280.0,
            cluster_dwell_ms: 900,
            cluster_show_icons: true,
            cluster_bloom_direction: ClusterBloomDirection::Clockwise,
            tile_gaps_inner_px: 20.0,
            tile_gaps_outer_px: 20.0,
            tile_queue_show_icons: true,
            active_windows_allowed: 3,
            trail_history_length: 32,
            trail_wrap: true,

            active_outside_ring_delay_ms: 120_000,
            inactive_outside_ring_delay_ms: 30_000,
            docked_offscreen_delay_ms: 300_000,

            non_overlap_gap_px: 20.0,
            pan_to_new: PanToNewMode::IfNeeded,
            close_restore_focus: true,
            close_restore_pan: CloseRestorePanMode::IfOffscreen,
            non_overlap_active_gap_scale: 0.22,
            non_overlap_bump_newer: false,
            non_overlap_bump_damping: 0.65,
            drag_smoothing_boost: 6.0,
            center_window_to_mouse: false,
            restore_last_active_on_pan_return: true,
            physics_enabled: true,
            no_csd: false,
            window_rules: Vec::new(),

            keybinds: Keybinds::default(),
            compositor_bindings: default_compositor_bindings(Keybinds::default().modifier),
            launch_bindings: Vec::new(),
            pointer_bindings: default_pointer_bindings(Keybinds::default().modifier),

            tty_viewports: Vec::new(),
            autostart_once: Vec::new(),
            autostart_on_reload: Vec::new(),
            env: HashMap::from([
                ("XCURSOR_THEME".to_string(), "Adwaita".to_string()),
                ("XCURSOR_SIZE".to_string(), "24".to_string()),
            ]),
        }
    }
}

impl RuntimeTuning {
    pub fn effective_no_csd(&self) -> bool {
        self.no_csd || self.border_radius_px > 0
    }

    pub fn config_path() -> String {
        match env::var("HALLEY_WL_CONFIG") {
            Ok(path) => absolutize_path(&path).to_string_lossy().to_string(),
            Err(_) => default_config_path().to_string_lossy().to_string(),
        }
    }

    pub fn load() -> Self {
        Self::load_from_path(&Self::config_path())
    }

    pub fn load_from_path(path: &str) -> Self {
        let mut out = Self::try_load_from_path(path).unwrap_or_default();
        out.clamp_values();
        out
    }

    pub fn try_load_from_path(path: &str) -> Option<Self> {
        let mut out = Self::from_rune_file(path)?;
        out.clamp_values();
        Some(out)
    }

    pub fn apply_process_env(&self) {
        for (key, value) in &self.env {
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            unsafe { env::set_var(key, value) };
        }
    }

    fn clamp_values(&mut self) {
        self.debug_dump_every_ms = self.debug_dump_every_ms.clamp(100, 60_000);

        self.viewport_center.x = self.viewport_center.x.clamp(-100_000.0, 100_000.0);
        self.viewport_center.y = self.viewport_center.y.clamp(-100_000.0, 100_000.0);
        self.viewport_size.x = self.viewport_size.x.clamp(320.0, 16_000.0);
        self.viewport_size.y = self.viewport_size.y.clamp(240.0, 16_000.0);

        self.focus_ring_rx = self.focus_ring_rx.clamp(8.0, 16_000.0);
        self.focus_ring_ry = self.focus_ring_ry.clamp(8.0, 16_000.0);
        self.focus_ring_offset_x = self.focus_ring_offset_x.clamp(-16_000.0, 16_000.0);
        self.focus_ring_offset_y = self.focus_ring_offset_y.clamp(-16_000.0, 16_000.0);

        self.primary_hot_inner_frac = self.primary_hot_inner_frac.clamp(0.1, 1.0);
        self.primary_to_node_ms = self.primary_to_node_ms.clamp(250, 7_200_000);
        self.node_icon_size = self.node_icon_size.clamp(0.35, 0.95);
        self.border_size_px = self.border_size_px.clamp(0, 64);
        self.border_radius_px = self.border_radius_px.clamp(0, 256);
        self.bearings.fade_distance = self.bearings.fade_distance.clamp(120.0, 100_000.0);

        self.dev_zoom_decay_min_frac = self.dev_zoom_decay_min_frac.clamp(0.005, 0.5);
        self.dev_anim_state_change_ms = self.dev_anim_state_change_ms.clamp(30, 3_000);
        self.dev_anim_bounce = self.dev_anim_bounce.clamp(0.0, 3.0);

        self.cluster_distance_px = self.cluster_distance_px.clamp(24.0, 4_000.0);
        self.cluster_dwell_ms = self.cluster_dwell_ms.clamp(0, 30_000);
        self.tile_gaps_inner_px = self.tile_gaps_inner_px.clamp(0.0, 256.0);
        self.tile_gaps_outer_px = self.tile_gaps_outer_px.clamp(0.0, 512.0);
        self.active_windows_allowed = self.active_windows_allowed.clamp(1, 64);
        self.trail_history_length = self.trail_history_length.clamp(1, 512);

        self.active_outside_ring_delay_ms = self.active_outside_ring_delay_ms.clamp(0, 7_200_000);
        self.inactive_outside_ring_delay_ms =
            self.inactive_outside_ring_delay_ms.clamp(0, 7_200_000);
        self.docked_offscreen_delay_ms = self.docked_offscreen_delay_ms.clamp(0, 7_200_000);

        self.non_overlap_gap_px = self.non_overlap_gap_px.clamp(0.0, 256.0);
        self.non_overlap_active_gap_scale = self.non_overlap_active_gap_scale.clamp(0.0, 1.2);
        self.non_overlap_bump_damping = self.non_overlap_bump_damping.clamp(0.05, 1.0);
        self.drag_smoothing_boost = self.drag_smoothing_boost.clamp(0.1, 20.0);
    }

    pub fn enforce_guards(&mut self) {
        self.clamp_values();
    }

    pub fn viewport(&self) -> Viewport {
        Viewport::new(self.viewport_center, self.viewport_size)
    }

    pub fn focus_ring(&self) -> FocusRing {
        FocusRingConfig {
            rx: self.focus_ring_rx,
            ry: self.focus_ring_ry,
            offset_x: self.focus_ring_offset_x,
            offset_y: self.focus_ring_offset_y,
        }
        .to_focus_ring()
    }

    pub fn focus_ring_for_output(&self, output_name: &str) -> FocusRing {
        self.tty_viewports
            .iter()
            .find(|viewport| viewport.connector == output_name)
            .and_then(|viewport| viewport.focus_ring)
            .unwrap_or(FocusRingConfig {
                rx: self.focus_ring_rx,
                ry: self.focus_ring_ry,
                offset_x: self.focus_ring_offset_x,
                offset_y: self.focus_ring_offset_y,
            })
            .to_focus_ring()
    }

    pub fn focus_ring_decay_policy(&self) -> FocusRingDecayPolicy {
        let mut p = FocusRingDecayPolicy::new();
        p.inside_to_node_ms = self.primary_to_node_ms;
        p
    }

    pub fn keybinds_resolved_summary(&self) -> String {
        format!(
            "mod={} compositor_actions={} custom_launches={} pointer_actions={}",
            self.keybinds.modifier_name(),
            self.compositor_bindings.len(),
            self.launch_bindings.len(),
            self.pointer_bindings.len(),
        )
    }
}

pub(crate) fn default_pointer_bindings(modifier: KeyModifiers) -> Vec<PointerBinding> {
    let mut transfer_modifier = modifier;
    transfer_modifier.shift = true;
    vec![
        PointerBinding {
            modifiers: modifier,
            button: 272,
            action: PointerBindingAction::MoveWindow,
        },
        PointerBinding {
            modifiers: transfer_modifier,
            button: 272,
            action: PointerBindingAction::FieldJump,
        },
        PointerBinding {
            modifiers: modifier,
            button: 273,
            action: PointerBindingAction::ResizeWindow,
        },
    ]
}

pub(crate) fn default_compositor_bindings(modifier: KeyModifiers) -> Vec<CompositorBinding> {
    let key = |name: &str| key_name_to_evdev(name).expect("default compositor key should exist");

    vec![
        CompositorBinding {
            modifiers: modifier,
            key: key("r"),
            action: CompositorBindingAction::Reload,
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("n"),
            action: CompositorBindingAction::ToggleState,
        },
        CompositorBinding {
            modifiers: KeyModifiers {
                shift: true,
                ..modifier
            },
            key: key("c"),
            action: CompositorBindingAction::ClusterMode,
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("z"),
            action: CompositorBindingAction::Bearings(BearingsBindingAction::Show),
        },
        CompositorBinding {
            modifiers: KeyModifiers {
                shift: true,
                ..modifier
            },
            key: key("z"),
            action: CompositorBindingAction::Bearings(BearingsBindingAction::Toggle),
        },
        CompositorBinding {
            modifiers: modifier,
            key: WHEEL_UP_CODE,
            action: CompositorBindingAction::ZoomIn,
        },
        CompositorBinding {
            modifiers: modifier,
            key: WHEEL_DOWN_CODE,
            action: CompositorBindingAction::ZoomOut,
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("mousemiddle"),
            action: CompositorBindingAction::ZoomReset,
        },
        CompositorBinding {
            modifiers: KeyModifiers {
                shift: true,
                ..modifier
            },
            key: key("q"),
            action: CompositorBindingAction::Quit {
                requires_shift: true,
            },
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("h"),
            action: CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Left)),
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("k"),
            action: CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Up)),
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("l"),
            action: CompositorBindingAction::Node(NodeBindingAction::Move(
                DirectionalAction::Right,
            )),
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("j"),
            action: CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Down)),
        },
        CompositorBinding {
            modifiers: KeyModifiers {
                shift: true,
                ..modifier
            },
            key: key("comma"),
            action: CompositorBindingAction::Trail(TrailBindingAction::Prev),
        },
        CompositorBinding {
            modifiers: KeyModifiers {
                shift: true,
                ..modifier
            },
            key: key("dot"),
            action: CompositorBindingAction::Trail(TrailBindingAction::Next),
        },
    ]
}

fn default_config_path() -> PathBuf {
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

fn absolutize_path(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(p))
            .unwrap_or_else(|_| PathBuf::from(path))
    }
}
