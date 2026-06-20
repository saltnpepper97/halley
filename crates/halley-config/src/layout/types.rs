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
    Fixed { r: f32, g: f32, b: f32, a: f32 },
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinBadgeCorner {
    TopLeft,
    TopRight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinsConfig {
    pub corner: PinBadgeCorner,
    pub color: OverlayColorMode,
    pub background_color: OverlayColorMode,
    pub size: f32,
}

impl Default for PinsConfig {
    fn default() -> Self {
        Self {
            corner: PinBadgeCorner::TopRight,
            color: OverlayColorMode::Auto,
            background_color: OverlayColorMode::Auto,
            size: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayStyleConfig {
    pub background_color: OverlayColorMode,
    pub text_color: OverlayColorMode,
    pub error_color: OverlayColorMode,
    pub shape: OverlayShape,
    pub borders: bool,
    pub border_source: OverlayBorderSource,
    pub blur: bool,
}

impl Default for OverlayStyleConfig {
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
            shape: OverlayShape::Square,
            borders: true,
            border_source: OverlayBorderSource::Primary,
            blur: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugConfig {
    pub overlay_fps: bool,
    pub show_ring_when_resizing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ApogeeConfig {
    pub enabled: bool,
    pub live_previews: bool,
    pub transition_ms: u64,
    pub gap: f32,
    pub max_rows: u32,
    pub background_dim: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParallaxConfig {
    pub enabled: bool,
    pub strength: f32,
    pub tau_ms: u64,
}

impl Default for ParallaxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: 0.035,
            tau_ms: 90,
        }
    }
}

impl Default for ApogeeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            live_previews: false,
            transition_ms: 320,
            gap: 24.0,
            max_rows: 3,
            background_dim: 0.85,
        }
    }
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            overlay_fps: false,
            show_ring_when_resizing: true,
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
    Fade,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowCloseAnimationConfig {
    pub enabled: bool,
    pub duration_ms: u64,
    pub style: WindowCloseAnimationStyle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RaiseAnimationTrigger {
    Always,
    Overlap,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RaiseAnimationConfig {
    pub enabled: bool,
    pub duration_ms: u64,
    pub scale: f32,
    pub shadow_boost: f32,
    pub trigger: RaiseAnimationTrigger,
}

impl RaiseAnimationConfig {
    pub const fn new(enabled: bool, duration_ms: u64, scale: f32, shadow_boost: f32) -> Self {
        Self {
            enabled,
            duration_ms,
            scale,
            shadow_boost,
            trigger: RaiseAnimationTrigger::Always,
        }
    }
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
    pub fullscreen: TimedAnimationConfig,
    pub window_close: WindowCloseAnimationConfig,
    pub window_open: TimedAnimationConfig,
    pub tile: TimedAnimationConfig,
    pub stack: TimedAnimationConfig,
    pub raise: RaiseAnimationConfig,
}

impl Default for AnimationsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            smooth_resize: TimedAnimationConfig::new(true, 90),
            maximize: TimedAnimationConfig::new(true, 240),
            fullscreen: TimedAnimationConfig::new(true, 240),
            window_close: WindowCloseAnimationConfig::new(
                true,
                250,
                WindowCloseAnimationStyle::Shrink,
            ),
            window_open: TimedAnimationConfig::new(true, 620),
            tile: TimedAnimationConfig::new(true, 240),
            stack: TimedAnimationConfig::new(true, 220),
            raise: RaiseAnimationConfig::new(true, 140, 1.025, 0.18),
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
pub struct ShadowColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowLayerConfig {
    pub enabled: bool,
    pub blur_radius: f32,
    pub spread: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub color: ShadowColor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowsConfig {
    pub window: ShadowLayerConfig,
    pub node: ShadowLayerConfig,
    pub overlay: ShadowLayerConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientBlurMode {
    Off,
    Auto,
    Always,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlurMethod {
    DualKawase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurConfig {
    pub enabled: bool,
    pub overlays: bool,
    pub windows: ClientBlurMode,
    pub layer_shell: ClientBlurMode,
    pub method: BlurMethod,
    pub radius: f32,
    pub passes: u32,
    pub saturation: f32,
    pub noise: f32,
}

impl Default for BlurConfig {
    fn default() -> Self {
        Self {
            // Conservative: blur is opt-in so upgrading users do not pay a new
            // GPU cost unless they ask for it. The shipped template/config turns
            // it on explicitly.
            enabled: false,
            overlays: true,
            windows: ClientBlurMode::Auto,
            layer_shell: ClientBlurMode::Off,
            method: BlurMethod::DualKawase,
            radius: 24.0,
            passes: 3,
            saturation: 1.10,
            noise: 0.012,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectsConfig {
    pub blur: BlurConfig,
    pub shadows: ShadowsConfig,
}

impl Default for EffectsConfig {
    fn default() -> Self {
        Self {
            blur: BlurConfig::default(),
            shadows: ShadowsConfig::default(),
        }
    }
}

/// Whether a compositor-owned overlay should be drawn with backdrop blur.
///
/// Requires global blur on, the overlay channel allowed globally, and the
/// overlay style opting in.
pub fn overlay_blur_enabled(blur: &BlurConfig, overlay_style: &OverlayStyleConfig) -> bool {
    blur.enabled && blur.overlays && overlay_style.blur
}

/// Whether a client window should be drawn with backdrop blur.
///
/// Policy (rule-level `blur false` always wins; rule-level `blur true` can opt a
/// window in even when the global window mode is `off`/`auto`):
/// - global blur off, or surface excluded (fullscreen/presentation/video/
///   gamescope) -> never blur.
/// - rule `blur false` -> never blur.
/// - rule `blur true` -> always blur.
/// - otherwise by window mode: `off` -> no; `always` -> yes; `auto` -> only when
///   the surface is actually translucent (blur would matter).
pub fn window_blur_enabled(
    blur: &BlurConfig,
    rule_blur: Option<bool>,
    opacity: f32,
    is_excluded: bool,
) -> bool {
    if !blur.enabled || is_excluded {
        return false;
    }
    match rule_blur {
        Some(false) => false,
        Some(true) => true,
        None => match blur.windows {
            ClientBlurMode::Off => false,
            ClientBlurMode::Always => true,
            ClientBlurMode::Auto => opacity < 0.999,
        },
    }
}

#[cfg(test)]
mod blur_policy_tests {
    use super::*;

    fn blur(enabled: bool, windows: ClientBlurMode) -> BlurConfig {
        BlurConfig {
            enabled,
            windows,
            layer_shell: ClientBlurMode::Off,
            ..BlurConfig::default()
        }
    }

    #[test]
    fn overlay_blur_requires_all_three_gates() {
        let mut style = OverlayStyleConfig::default();
        let mut b = BlurConfig::default();
        b.enabled = true;
        b.overlays = true;
        style.blur = true;
        assert!(overlay_blur_enabled(&b, &style));

        style.blur = false;
        assert!(!overlay_blur_enabled(&b, &style));

        style.blur = true;
        b.overlays = false;
        assert!(!overlay_blur_enabled(&b, &style));

        b.overlays = true;
        b.enabled = false;
        assert!(!overlay_blur_enabled(&b, &style));
    }

    #[test]
    fn window_blur_global_off_disables_everything() {
        let b = blur(false, ClientBlurMode::Always);
        assert!(!window_blur_enabled(&b, Some(true), 0.5, false));
        assert!(!window_blur_enabled(&b, None, 0.5, false));
    }

    #[test]
    fn window_blur_excluded_surface_never_blurs() {
        let b = blur(true, ClientBlurMode::Always);
        assert!(!window_blur_enabled(&b, Some(true), 0.5, true));
    }

    #[test]
    fn window_blur_rule_false_always_wins() {
        for mode in [
            ClientBlurMode::Off,
            ClientBlurMode::Auto,
            ClientBlurMode::Always,
        ] {
            let b = blur(true, mode);
            assert!(!window_blur_enabled(&b, Some(false), 0.5, false));
        }
    }

    #[test]
    fn window_blur_rule_true_opts_in_under_off_and_auto() {
        for mode in [
            ClientBlurMode::Off,
            ClientBlurMode::Auto,
            ClientBlurMode::Always,
        ] {
            let b = blur(true, mode);
            assert!(window_blur_enabled(&b, Some(true), 1.0, false));
        }
    }

    #[test]
    fn window_blur_mode_off_without_rule_is_no() {
        let b = blur(true, ClientBlurMode::Off);
        assert!(!window_blur_enabled(&b, None, 0.5, false));
    }

    #[test]
    fn window_blur_mode_always_blurs_eligible() {
        let b = blur(true, ClientBlurMode::Always);
        assert!(window_blur_enabled(&b, None, 1.0, false));
    }

    #[test]
    fn window_blur_mode_auto_only_when_translucent() {
        let b = blur(true, ClientBlurMode::Auto);
        assert!(window_blur_enabled(&b, None, 0.85, false));
        assert!(!window_blur_enabled(&b, None, 1.0, false));
    }
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

impl Default for ShadowsConfig {
    fn default() -> Self {
        Self {
            window: ShadowLayerConfig {
                enabled: true,
                blur_radius: 8.0,
                spread: 0.0,
                offset_x: 0.0,
                offset_y: 5.0,
                color: ShadowColor {
                    r: 0x05 as f32 / 255.0,
                    g: 0x03 as f32 / 255.0,
                    b: 0x05 as f32 / 255.0,
                    a: 0x30 as f32 / 255.0,
                },
            },
            node: ShadowLayerConfig {
                enabled: true,
                blur_radius: 14.0,
                spread: 0.0,
                offset_x: 0.0,
                offset_y: 3.0,
                color: ShadowColor {
                    r: 0x05 as f32 / 255.0,
                    g: 0x03 as f32 / 255.0,
                    b: 0x05 as f32 / 255.0,
                    a: 0x24 as f32 / 255.0,
                },
            },
            overlay: ShadowLayerConfig {
                enabled: true,
                blur_radius: 24.0,
                spread: 1.0,
                offset_x: 0.0,
                offset_y: 7.0,
                color: ShadowColor {
                    r: 0x05 as f32 / 255.0,
                    g: 0x03 as f32 / 255.0,
                    b: 0x05 as f32 / 255.0,
                    a: 0x38 as f32 / 255.0,
                },
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
pub enum ExpandedPlacementStrategy {
    Center,
    FindEmpty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FindEmptyMode {
    BestEffort,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LandmarkPlacementStrategy {
    NearestFree,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NormalBlockerPolicy {
    Relocate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinnedBlockerPolicy {
    Preserve,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpandedPlacementConfig {
    pub strategy: ExpandedPlacementStrategy,
    pub fallback: ExpandedPlacementStrategy,
    pub find_empty_mode: FindEmptyMode,
}

impl Default for ExpandedPlacementConfig {
    fn default() -> Self {
        Self {
            strategy: ExpandedPlacementStrategy::FindEmpty,
            fallback: ExpandedPlacementStrategy::Center,
            find_empty_mode: FindEmptyMode::BestEffort,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LandmarkPlacementConfig {
    pub strategy: LandmarkPlacementStrategy,
    pub normal_blocker: NormalBlockerPolicy,
    pub pinned_blocker: PinnedBlockerPolicy,
}

impl Default for LandmarkPlacementConfig {
    fn default() -> Self {
        Self {
            strategy: LandmarkPlacementStrategy::NearestFree,
            normal_blocker: NormalBlockerPolicy::Relocate,
            pinned_blocker: PinnedBlockerPolicy::Preserve,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacementRevealConfig {
    pub enabled: bool,
    pub max_pan_px: f32,
    pub animation_ms: u64,
}

impl Default for PlacementRevealConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_pan_px: 360.0,
            animation_ms: 180,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacementConfig {
    pub expanded: ExpandedPlacementConfig,
    pub landmarks: LandmarkPlacementConfig,
    pub reveal: PlacementRevealConfig,
}

impl Default for PlacementConfig {
    fn default() -> Self {
        Self {
            expanded: ExpandedPlacementConfig::default(),
            landmarks: LandmarkPlacementConfig::default(),
            reveal: PlacementRevealConfig::default(),
        }
    }
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyboardConfig {
    pub layout: String,
    pub variant: String,
    pub options: String,
    pub model: String,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            layout: "us".to_string(),
            variant: String::new(),
            options: String::new(),
            model: String::new(),
        }
    }
}

/// libinput pointer acceleration profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccelProfile {
    Adaptive,
    Flat,
}

/// libinput scroll method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScrollMethod {
    NoScroll,
    TwoFinger,
    Edge,
    OnButtonDown,
}

/// libinput touchpad click method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClickMethod {
    ButtonAreas,
    Clickfinger,
}

/// libinput tap-to-click button mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TapButtonMap {
    /// 1/2/3 fingers → left/right/middle.
    LeftRightMiddle,
    /// 1/2/3 fingers → left/middle/right.
    LeftMiddleRight,
}

/// libinput device settings. Every field is optional: `None` means "leave libinput's own
/// default untouched". The same struct backs the `touchpad:`/`mouse:` type sections and the
/// per-device override blocks, so a device's effective settings are simply the type section
/// overlaid with any matching `devices.<name>` block (see [`DeviceSettings::overlay`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceSettings {
    // Shared pointer settings.
    pub natural_scroll: Option<bool>,
    pub accel_speed: Option<f64>,
    pub accel_profile: Option<AccelProfile>,
    pub scroll_method: Option<ScrollMethod>,
    pub scroll_button: Option<u32>,
    pub left_handed: Option<bool>,
    pub middle_emulation: Option<bool>,
    pub enabled: Option<bool>,
    // Touchpad-only settings.
    pub tap: Option<bool>,
    pub tap_button_map: Option<TapButtonMap>,
    pub dwt: Option<bool>,
    pub click_method: Option<ClickMethod>,
    pub drag: Option<bool>,
    pub drag_lock: Option<bool>,
    pub disabled_on_external_mouse: Option<bool>,
}

impl DeviceSettings {
    pub fn has_any(&self) -> bool {
        self.natural_scroll.is_some()
            || self.accel_speed.is_some()
            || self.accel_profile.is_some()
            || self.scroll_method.is_some()
            || self.scroll_button.is_some()
            || self.left_handed.is_some()
            || self.middle_emulation.is_some()
            || self.enabled.is_some()
            || self.tap.is_some()
            || self.tap_button_map.is_some()
            || self.dwt.is_some()
            || self.click_method.is_some()
            || self.drag.is_some()
            || self.drag_lock.is_some()
            || self.disabled_on_external_mouse.is_some()
    }

    /// Layer `other` on top of `self`, taking each field from `other` only where it is set.
    pub fn overlay(&self, other: &DeviceSettings) -> DeviceSettings {
        DeviceSettings {
            natural_scroll: other.natural_scroll.or(self.natural_scroll),
            accel_speed: other.accel_speed.or(self.accel_speed),
            accel_profile: other.accel_profile.or(self.accel_profile),
            scroll_method: other.scroll_method.or(self.scroll_method),
            scroll_button: other.scroll_button.or(self.scroll_button),
            left_handed: other.left_handed.or(self.left_handed),
            middle_emulation: other.middle_emulation.or(self.middle_emulation),
            enabled: other.enabled.or(self.enabled),
            tap: other.tap.or(self.tap),
            tap_button_map: other.tap_button_map.or(self.tap_button_map),
            dwt: other.dwt.or(self.dwt),
            click_method: other.click_method.or(self.click_method),
            drag: other.drag.or(self.drag),
            drag_lock: other.drag_lock.or(self.drag_lock),
            disabled_on_external_mouse: other
                .disabled_on_external_mouse
                .or(self.disabled_on_external_mouse),
        }
    }
}

/// A per-device override block (`input.devices.<name>:`), matched against the libinput
/// device name reported by `libinput list-devices`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceOverride {
    pub name: String,
    pub settings: DeviceSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositorGestureScope {
    EmptyField,
    Global,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GestureScrollPanMode {
    Off,
    EmptyField,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GestureSwipeDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GestureBindingAction {
    ApogeeOpen,
    ApogeeClose,
    Compositor(crate::keybinds::CompositorBindingAction),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GestureBinding {
    pub direction: GestureSwipeDirection,
    pub fingers: u32,
    pub action: GestureBindingAction,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GestureInputConfig {
    pub enabled: bool,
    pub client_passthrough: bool,
    pub touch_passthrough: bool,
    pub pinch_to_zoom: bool,
    pub pinch_scope: CompositorGestureScope,
    pub compositor_scope: CompositorGestureScope,
    pub modifier: crate::keybinds::KeyModifiers,
    pub scroll_pan: GestureScrollPanMode,
    pub swipe_threshold_px: f32,
    pub swipe_bindings: Vec<GestureBinding>,
    pub apogee_swipe_bindings: Vec<GestureBinding>,
}

impl Default for GestureInputConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            client_passthrough: true,
            touch_passthrough: true,
            pinch_to_zoom: true,
            pinch_scope: CompositorGestureScope::EmptyField,
            compositor_scope: CompositorGestureScope::Global,
            modifier: crate::keybinds::Keybinds::default().modifier,
            scroll_pan: GestureScrollPanMode::EmptyField,
            swipe_threshold_px: 120.0,
            swipe_bindings: vec![GestureBinding {
                direction: GestureSwipeDirection::Up,
                fingers: 3,
                action: GestureBindingAction::ApogeeOpen,
            }],
            apogee_swipe_bindings: vec![GestureBinding {
                direction: GestureSwipeDirection::Up,
                fingers: 3,
                action: GestureBindingAction::ApogeeClose,
            }],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct InputConfig {
    pub repeat_rate: i32,
    pub repeat_delay: i32,
    pub focus_mode: InputFocusMode,
    pub raise_on_click: bool,
    pub keyboard: KeyboardConfig,
    pub gestures: GestureInputConfig,
    pub touchpad: DeviceSettings,
    pub mouse: DeviceSettings,
    pub devices: Vec<DeviceOverride>,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            repeat_rate: 30,
            repeat_delay: 500,
            focus_mode: InputFocusMode::Click,
            raise_on_click: true,
            keyboard: KeyboardConfig::default(),
            gestures: GestureInputConfig::default(),
            touchpad: DeviceSettings::default(),
            mouse: DeviceSettings::default(),
            devices: Vec::new(),
        }
    }
}

impl InputConfig {
    pub fn has_physical_device_settings(&self) -> bool {
        self.touchpad.has_any()
            || self.mouse.has_any()
            || self.devices.iter().any(|device| device.settings.has_any())
    }
}

#[cfg(test)]
mod input_config_tests {
    use super::{AccelProfile, DeviceOverride, DeviceSettings, InputConfig};

    #[test]
    fn device_settings_has_any_detects_configured_fields() {
        assert!(!DeviceSettings::default().has_any());

        let settings = DeviceSettings {
            accel_profile: Some(AccelProfile::Flat),
            ..DeviceSettings::default()
        };
        assert!(settings.has_any());
    }

    #[test]
    fn input_config_detects_physical_device_settings() {
        assert!(!InputConfig::default().has_physical_device_settings());

        let touchpad = InputConfig {
            touchpad: DeviceSettings {
                natural_scroll: Some(true),
                ..DeviceSettings::default()
            },
            ..InputConfig::default()
        };
        assert!(touchpad.has_physical_device_settings());

        let device_override = InputConfig {
            devices: vec![DeviceOverride {
                name: "Mouse".to_string(),
                settings: DeviceSettings {
                    accel_speed: Some(-0.25),
                    ..DeviceSettings::default()
                },
            }],
            ..InputConfig::default()
        };
        assert!(device_override.has_physical_device_settings());
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

/// Top-level `gamescope:` configuration: global defaults plus repeated per-game
/// `game:` profiles. Games are wrapped at launch by `halleyctl gamescope run`.
#[derive(Clone, Debug, PartialEq)]
pub struct GamescopeConfig {
    pub enabled: bool,
    /// Monitor selector: `focused`, `cursor`, `primary`, or a connector name.
    pub monitor: String,
    /// Dimension/refresh values are `"auto"` (resolve from the selected monitor)
    /// or a numeric string.
    pub output_width: String,
    pub output_height: String,
    pub game_width: String,
    pub game_height: String,
    pub refresh: String,
    pub fullscreen: bool,
    pub borderless: bool,
    pub suppress_overlays: bool,
    pub passthrough_pointer_lock: bool,
    pub bypass_spatial_camera: bool,
    pub games: Vec<GamescopeGameProfile>,
}

/// A per-game `game:` profile. Every field except the match keys is optional and
/// inherits the global [`GamescopeConfig`] default when unset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GamescopeGameProfile {
    pub name: Option<String>,
    pub app_id: Option<String>,
    pub enabled: Option<bool>,
    pub monitor: Option<String>,
    pub output_width: Option<String>,
    pub output_height: Option<String>,
    pub game_width: Option<String>,
    pub game_height: Option<String>,
    pub refresh: Option<String>,
    pub fullscreen: Option<bool>,
    pub borderless: Option<bool>,
    pub suppress_overlays: Option<bool>,
    pub passthrough_pointer_lock: Option<bool>,
    pub bypass_spatial_camera: Option<bool>,
}

impl Default for GamescopeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            monitor: "focused".to_string(),
            output_width: "auto".to_string(),
            output_height: "auto".to_string(),
            game_width: "auto".to_string(),
            game_height: "auto".to_string(),
            refresh: "auto".to_string(),
            fullscreen: true,
            borderless: false,
            suppress_overlays: true,
            passthrough_pointer_lock: true,
            bypass_spatial_camera: true,
            games: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BearingsConfig {
    pub show_distance: bool,
    pub show_icons: bool,
    pub show_pinned: bool,
    pub fade_distance: f32,
    /// Frosted-glass backdrop blur behind each bearing chip. Also requires the
    /// global overlay blur (`effects.blur.overlays`) to be enabled.
    pub blur: bool,
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
    Default,
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

#[derive(Clone, Debug, PartialEq)]
pub struct WindowRule {
    pub app_ids: Vec<WindowRulePattern>,
    pub titles: Vec<WindowRulePattern>,
    pub initial_size: Option<(u32, u32)>,
    pub opacity: Option<f32>,
    pub blur: Option<bool>,
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
        matches!(self, Self::On)
    }
}
