#![allow(
    clippy::result_large_err,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

pub mod gamescope;
pub mod keybinds;
pub mod layout;
pub mod parse;

pub use keybinds::{
    BearingsBindingAction, ClusterBindingAction, CompositorBinding, CompositorBindingAction,
    CompositorBindingScope, DirectionalAction, FocusCycleBindingAction, KeyModifiers, Keybinds,
    LaunchBinding, MonitorBindingAction, MonitorBindingTarget, NodeBindingAction, PointerBinding,
    PointerBindingAction, StackBindingAction, StackCycleDirection, TileBindingAction,
    TrailBindingAction, WHEEL_DOWN_CODE, WHEEL_UP_CODE,
};
pub use layout::{
    AccelProfile, AnimationToggleConfig, AnimationsConfig, BackgroundColor, BackgroundConfig,
    BackgroundFit, BackgroundMode, BlurConfig, BlurMethod, ClickCollapsedOutsideFocusMode,
    ClickCollapsedPanMode, ClickMethod, ClientBlurMode, CloseRestorePanMode,
    ClusterAnimationConfig, ClusterBloomDirection, ClusterDefaultLayout, ClusterLayoutAnimConfig,
    CompositorGestureScope, ConfigPathSource, CursorConfig, DebugConfig, DecorationBorderColor,
    DecorationsConfig, DeviceOverride, DeviceSettings, EffectsConfig, ExpandedPlacementConfig,
    ExpandedPlacementStrategy, FindEmptyMode, FontConfig, GamescopeConfig, GamescopeGameProfile,
    GamingConfig,
    GestureBinding, GestureBindingAction, GestureHoldBinding, GestureScrollPanMode,
    GestureSwipeDirection, InitialWindowClusterParticipation, InitialWindowOverlapPolicy,
    InitialWindowSpawnPlacement, InputConfig, InputFocusMode, KeyboardConfig,
    LandmarkPlacementConfig, LandmarkPlacementStrategy, NodeBackgroundColorMode,
    NodeBorderColorMode, NodeDisplayPolicy, NormalBlockerPolicy, OverlayBorderSource,
    OverlayColorMode, OverlayShape, OverlayStyleConfig, PanToNewMode, PinBadgeCorner,
    PinnedBlockerPolicy, PinsConfig, PlacementConfig, PlacementRevealConfig, PrimaryBorderConfig,
    RaiseAnimationConfig, RaiseAnimationTrigger, ResolvedConfigPath, RuntimeTuning,
    ScreenshotConfig, ScrollMethod, SecondaryBorderConfig, ShadowColor, ShadowLayerConfig,
    ShadowsConfig, ShapeStyle, TapButtonMap, TimedAnimationConfig, ViewportOutputConfig,
    ViewportVrrMode, WindowCloseAnimationConfig, WindowCloseAnimationStyle, WindowRule,
    WindowRulePattern, ZoomFilter, overlay_blur_enabled, window_blur_enabled,
};
pub use parse::{ConfigLoadDiagnostic, gather_dependencies_for_file};
