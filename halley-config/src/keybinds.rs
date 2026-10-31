/// The base `mod` key a keybind chord is built on. Generic and physical-side
/// variants are retained so `mod "lsuper"` remains distinct from `super`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierKey {
    Super,
    LeftSuper,
    RightSuper,
    Alt,
    LeftAlt,
    RightAlt,
    Ctrl,
    LeftCtrl,
    RightCtrl,
    Shift,
    LeftShift,
    RightShift,
}

/// A resolved modifier combination for a single keybind (the base `mod` plus
/// any extra modifiers in the chord, e.g. `mod+shift+e`).
///
/// Generic flags match either physical side. Side flags require that exact
/// key, using the compositor's raw evdev/XKB keycode tracking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub left_shift: bool,
    pub right_shift: bool,
    pub ctrl: bool,
    pub left_ctrl: bool,
    pub right_ctrl: bool,
    pub alt: bool,
    pub left_alt: bool,
    pub right_alt: bool,
    pub super_key: bool,
    pub left_super: bool,
    pub right_super: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BindingScope {
    #[default]
    Global,
    Field,
    Cluster,
    Tile,
    Stack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusCycleDirection {
    Forward,
    Backward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrailDirection {
    Previous,
    Next,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonitorTarget {
    Direction(Direction),
    Output(String),
}

/// Compatibility name retained for callers written before directional
/// navigation expanded beyond cluster tiles.
pub type ClusterDirection = Direction;

/// What a keybind does. Grows alongside whatever `halley-wl` actually wires
/// up next. `ZoomIn` exists to walk back a `ZoomOut` one step at a time, but
/// it's not a general "magnify past 1.0x" action - the 1.0x ceiling is
/// enforced independently of this enum (`Camera::clamp_view_size`, called
/// with a hardcoded `zoom_max` of `1.0` at every call site in
/// `src/input/zoom.rs`), so `ZoomIn` can never push past it no matter how
/// many times it's pressed. At 1.0x it's simply a no-op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Quit,
    CloseFocusedWindow,
    ToggleFullscreen,
    ToggleFieldMaximize,
    ToggleState,
    ToggleFocusedPin,
    Apogee,
    BearingsShow,
    BearingsToggle,
    FocusCycle(FocusCycleDirection),
    Trail(TrailDirection),
    FocusDirection(Direction),
    /// Move the focused or most-recent Field node by one placement step.
    MoveNode(Direction),
    /// Move along the requested axis by up to a positive number of Field units.
    MoveNodeBy {
        direction: Direction,
        distance: u32,
    },
    TransferWindow(Direction),
    PanField(Direction),
    /// Resize the focused Field window by one placement step. Left and up
    /// shrink; right and down grow.
    ResizeWindow(Direction),
    /// Arrange eligible windows visible on the active Field output into a
    /// balanced, non-persistent mosaic.
    ArrangeVisible,
    /// Restore the one-shot geometry snapshot captured by the last arrange.
    UndoArrange,
    /// Begin an interactive compositor move for the window under the pointer,
    /// falling back to Field panning when the grab starts on empty background.
    PointerMoveWindow,
    /// Begin an interactive compositor resize for the window under the pointer.
    PointerResizeWindow,
    /// Pan the Field from an empty-background pointer drag.
    PointerPanField,
    /// Pan the Field from a pointer drag that may begin anywhere on an output:
    /// over windows, titlebars, and collapsed nodes as well as empty
    /// background. It declines (and the press reaches the client) only while
    /// fullscreen, Field maximize, or an open cluster workspace owns the
    /// camera, over shell layers such as panels and overlays, and while a
    /// client holds a pointer lock or confinement.
    PointerPanFieldAlways,
    /// Keep a grabbed window on its output and pan that Field after dwelling
    /// against an edge.
    PointerDragPan,
    CenterLastFocused,
    ClusterMode,
    ClusterLayoutCycle,
    ClusterToggleFloat,
    ClusterSlot(u8),
    ClusterTileFocus(Direction),
    ClusterTileSwap(Direction),
    MonitorFocus(MonitorTarget),
    Reload,
    OpenTerminal,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Screenshot,
    /// A user-provided command line. This is the fallback for every action
    /// string that is not one of Halley's compositor actions.
    Spawn(String),
}

impl Action {
    /// Whether a compact keybind repeats when it does not provide an explicit
    /// `repeat` override. Continuous navigation and geometry actions repeat;
    /// destructive, modal, toggle, and process-launching actions do not.
    pub fn repeats_by_default(&self) -> bool {
        matches!(
            self,
            Self::FocusCycle(_)
                | Self::Trail(_)
                | Self::FocusDirection(_)
                | Self::MoveNode(_)
                | Self::MoveNodeBy { .. }
                | Self::PanField(_)
                | Self::ResizeWindow(_)
                | Self::ClusterTileFocus(_)
                | Self::ClusterTileSwap(_)
                | Self::MonitorFocus(_)
                | Self::ZoomIn
                | Self::ZoomOut
        )
    }

    pub fn default_scope(&self) -> BindingScope {
        match self {
            Self::MoveNode(_)
            | Self::MoveNodeBy { .. }
            | Self::TransferWindow(_)
            | Self::PanField(_)
            | Self::ResizeWindow(_)
            | Self::ToggleFocusedPin
            | Self::ArrangeVisible
            | Self::UndoArrange => BindingScope::Field,
            Self::PointerPanField | Self::PointerPanFieldAlways | Self::PointerDragPan => {
                BindingScope::Field
            }
            Self::ClusterLayoutCycle | Self::ClusterToggleFloat => BindingScope::Cluster,
            Self::ClusterTileFocus(_) | Self::ClusterTileSwap(_) => BindingScope::Tile,
            _ => BindingScope::Global,
        }
    }
}

/// A single parsed keybind, including the presentation context in which it is
/// active. Duplicate chords are valid when their scopes do not overlap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keybind {
    pub scope: BindingScope,
    pub modifiers: Modifiers,
    pub key: String,
    pub action: Action,
    /// Whether holding this keyboard trigger repeats the action using the
    /// compositor's configured input repeat delay and rate.
    pub repeat: bool,
}

/// The whole (currently keybinds-only) parsed config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keybinds {
    pub modifier: ModifierKey,
    pub binds: Vec<Keybind>,
}

impl Default for Keybinds {
    /// Parse the same template written by the bootstrap path so the runtime
    /// fallback and fresh-install config cannot drift apart.
    fn default() -> Self {
        let config = rune_cfg::RuneConfig::from_str(crate::bootstrap::DEFAULT_CONFIG)
            .expect("embedded default config must remain valid");
        crate::parse::parse_keybinds(&config).expect("embedded default keybinds must remain valid")
    }
}
