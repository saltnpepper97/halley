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
    AnimationToggleConfig, AnimationsConfig, ClickCollapsedOutsideFocusMode, ClickCollapsedPanMode,
    CloseRestorePanMode, ClusterBloomDirection, ClusterDefaultLayout, ConfigPathSource,
    CursorConfig, DebugConfig, DecorationBorderColor, DecorationsConfig, ExpandedPlacementConfig,
    ExpandedPlacementStrategy, FindEmptyMode, FontConfig, GamescopeConfig, GamescopeGameProfile,
    InitialWindowClusterParticipation, InitialWindowOverlapPolicy, InitialWindowSpawnPlacement,
    InputConfig, InputFocusMode, KeyboardConfig, LandmarkPlacementConfig,
    LandmarkPlacementStrategy, NodeBackgroundColorMode, NodeBorderColorMode, NodeDisplayPolicy,
    NormalBlockerPolicy, OverlayBorderSource, OverlayColorMode, OverlayShape, OverlayStyleConfig,
    PanToNewMode, PinBadgeCorner, PinnedBlockerPolicy, PinsConfig, PlacementConfig,
    PlacementRevealConfig, PrimaryBorderConfig, RaiseAnimationConfig, ResolvedConfigPath,
    RuntimeTuning, ScreenshotConfig, SecondaryBorderConfig, ShadowColor, ShadowLayerConfig,
    ShadowsConfig, ShapeStyle, TimedAnimationConfig, ViewportOutputConfig, ViewportVrrMode,
    WindowCloseAnimationConfig, WindowCloseAnimationStyle, WindowRule, WindowRulePattern,
};
pub use parse::{ConfigLoadDiagnostic, gather_dependencies_for_file};
