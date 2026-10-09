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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_actions_have_distinct_repeat_and_default_policies() {
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            let pan = Action::PanField(direction);
            let transfer = Action::TransferWindow(direction);
            assert!(pan.repeats_by_default());
            assert!(!transfer.repeats_by_default());
            assert_eq!(pan.default_scope(), BindingScope::Field);
            assert!(
                Keybinds::default()
                    .binds
                    .iter()
                    .any(|bind| bind.action == transfer)
            );
            assert!(
                !Keybinds::default()
                    .binds
                    .iter()
                    .any(|bind| bind.action == pan)
            );
        }
    }

    #[test]
    fn default_matches_the_shipped_keybinds() {
        let kb = Keybinds::default();
        assert_eq!(kb.modifier, ModifierKey::Super);
        assert_eq!(kb.binds.len(), 66);

        let previous = kb
            .binds
            .iter()
            .find(|bind| bind.action == Action::Trail(TrailDirection::Previous))
            .expect("previous Trail bind present");
        assert_eq!(previous.key, "comma");
        assert!(previous.repeat);

        let next = kb
            .binds
            .iter()
            .find(|bind| bind.action == Action::Trail(TrailDirection::Next))
            .expect("next Trail bind present");
        assert_eq!(next.key, "period");
        assert!(next.repeat);

        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            let resize = kb
                .binds
                .iter()
                .find(|bind| bind.action == Action::ResizeWindow(direction))
                .expect("Field resize bind present");
            assert_eq!(resize.scope, BindingScope::Field);
            assert!(resize.modifiers.ctrl);
            assert!(resize.repeat);

            let swap = kb
                .binds
                .iter()
                .find(|bind| bind.action == Action::ClusterTileSwap(direction))
                .expect("tile swap bind present");
            assert_eq!(swap.scope, BindingScope::Tile);
        }

        let reload = kb
            .binds
            .iter()
            .find(|bind| bind.action == Action::Reload)
            .expect("manual reload bind present");
        assert_eq!(reload.scope, BindingScope::Global);
        assert!(!reload.repeat);

        let arrange = kb
            .binds
            .iter()
            .find(|bind| bind.action == Action::ArrangeVisible)
            .expect("arrange-visible bind present");
        assert_eq!(arrange.scope, BindingScope::Field);
        assert_eq!(arrange.key, "a");
        assert!(arrange.modifiers.super_key);
        assert!(!arrange.modifiers.shift);
        assert!(!arrange.repeat);

        let move_or_pan = kb
            .binds
            .iter()
            .find(|bind| bind.action == Action::PointerMoveWindow)
            .expect("contextual pointer move bind present");
        assert!(move_or_pan.modifiers.super_key);
        assert_eq!(move_or_pan.key, "click-left");

        let drag_pan = kb
            .binds
            .iter()
            .find(|bind| bind.action == Action::PointerDragPan)
            .expect("grabbed-window Field pan bind present");
        assert!(drag_pan.modifiers.super_key);
        assert!(drag_pan.modifiers.shift);
        assert_eq!(drag_pan.key, "click-left");

        let bare_pan = kb
            .binds
            .iter()
            .find(|bind| bind.action == Action::PointerPanField)
            .expect("bare Field pan bind present");
        assert_eq!(bare_pan.modifiers, Modifiers::default());
        assert_eq!(bare_pan.key, "click-left");

        let launcher = kb
            .binds
            .iter()
            .find(|bind| bind.action == Action::Spawn("halley-lift".into()))
            .expect("Halley Lift launcher bind present");
        assert!(launcher.modifiers.super_key);
        assert_eq!(launcher.key, "d");

        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            let movement = kb
                .binds
                .iter()
                .find(|bind| bind.action == Action::MoveNode(direction))
                .expect("Field movement bind present");
            assert!(movement.modifiers.super_key);
            assert!(movement.modifiers.alt);
            assert!(movement.repeat);
        }

        assert!(!launcher.repeat);

        let quit = kb.binds.iter().find(|b| b.action == Action::Quit).unwrap();
        assert!(quit.modifiers.super_key);
        assert!(quit.modifiers.shift);
        assert_eq!(quit.key, "e");

        let close = kb
            .binds
            .iter()
            .find(|b| b.action == Action::CloseFocusedWindow)
            .unwrap();
        assert!(close.modifiers.super_key);
        assert!(!close.modifiers.shift);
        assert_eq!(close.key, "q");

        let fullscreen = kb
            .binds
            .iter()
            .find(|b| b.action == Action::ToggleFullscreen)
            .unwrap();
        assert!(fullscreen.modifiers.super_key);
        assert_eq!(fullscreen.key, "f");

        let maximize = kb
            .binds
            .iter()
            .find(|b| b.action == Action::ToggleFieldMaximize)
            .unwrap();
        assert!(maximize.modifiers.super_key);
        assert_eq!(maximize.key, "m");

        let toggle_state = kb
            .binds
            .iter()
            .find(|b| b.action == Action::ToggleState)
            .unwrap();
        assert!(toggle_state.modifiers.super_key);
        assert_eq!(toggle_state.key, "n");

        let toggle_pin = kb
            .binds
            .iter()
            .find(|b| b.action == Action::ToggleFocusedPin)
            .unwrap();
        assert!(toggle_pin.modifiers.super_key);
        assert_eq!(toggle_pin.key, "p");
        assert!(!toggle_pin.repeat);

        let cluster_float = kb
            .binds
            .iter()
            .find(|b| b.action == Action::ClusterToggleFloat)
            .unwrap();
        assert!(cluster_float.modifiers.super_key);
        assert_eq!(cluster_float.key, "v");

        let apogee = kb
            .binds
            .iter()
            .find(|b| b.action == Action::Apogee)
            .unwrap();
        assert!(apogee.modifiers.super_key);
        assert_eq!(apogee.key, "o");

        let bearings_show = kb
            .binds
            .iter()
            .find(|b| b.action == Action::BearingsShow)
            .unwrap();
        assert!(bearings_show.modifiers.super_key);
        assert!(!bearings_show.modifiers.shift);
        assert_eq!(bearings_show.key, "z");

        let bearings_toggle = kb
            .binds
            .iter()
            .find(|b| b.action == Action::BearingsToggle)
            .unwrap();
        assert!(bearings_toggle.modifiers.super_key);
        assert!(bearings_toggle.modifiers.shift);
        assert_eq!(bearings_toggle.key, "z");

        let forward = kb
            .binds
            .iter()
            .find(|b| b.action == Action::FocusCycle(FocusCycleDirection::Forward))
            .unwrap();
        assert!(forward.modifiers.alt);
        assert!(!forward.modifiers.shift);
        assert_eq!(forward.key, "tab");

        let backward = kb
            .binds
            .iter()
            .find(|b| b.action == Action::FocusCycle(FocusCycleDirection::Backward))
            .unwrap();
        assert!(backward.modifiers.alt);
        assert!(backward.modifiers.shift);
        assert_eq!(backward.key, "tab");

        for (direction, key) in [
            (Direction::Left, "left"),
            (Direction::Right, "right"),
            (Direction::Up, "up"),
            (Direction::Down, "down"),
        ] {
            let focus = kb
                .binds
                .iter()
                .find(|bind| bind.action == Action::FocusDirection(direction))
                .unwrap();
            assert!(focus.modifiers.super_key);
            assert!(!focus.modifiers.ctrl);
            assert!(!focus.modifiers.shift);
            assert_eq!(focus.key, key);

            let swap = kb
                .binds
                .iter()
                .find(|bind| bind.action == Action::ClusterTileSwap(direction))
                .unwrap();
            assert!(swap.modifiers.super_key);
            assert!(swap.modifiers.ctrl);
            assert!(!swap.modifiers.shift);
            assert_eq!(swap.key, key);

            let monitor = kb
                .binds
                .iter()
                .find(|bind| {
                    bind.action == Action::MonitorFocus(MonitorTarget::Direction(direction))
                })
                .unwrap();
            assert!(monitor.modifiers.super_key);
            assert!(!monitor.modifiers.ctrl);
            assert!(monitor.modifiers.shift);
            assert_eq!(monitor.key, key);
        }

        let center = kb
            .binds
            .iter()
            .find(|bind| bind.action == Action::CenterLastFocused)
            .unwrap();
        assert!(center.modifiers.super_key);
        assert_eq!(center.key, "h");

        let term = kb
            .binds
            .iter()
            .find(|b| b.action == Action::OpenTerminal)
            .unwrap();
        assert!(term.modifiers.super_key);
        assert_eq!(term.key, "t");

        let zoom_out = kb
            .binds
            .iter()
            .find(|b| b.action == Action::ZoomOut)
            .unwrap();
        assert!(zoom_out.modifiers.super_key);
        assert_eq!(zoom_out.key, "minus");

        let zoom_in = kb
            .binds
            .iter()
            .find(|b| b.action == Action::ZoomIn)
            .unwrap();
        assert!(zoom_in.modifiers.super_key);
        assert_eq!(zoom_in.key, "equal");

        let zoom_reset = kb
            .binds
            .iter()
            .find(|b| b.action == Action::ZoomReset)
            .unwrap();
        assert!(zoom_reset.modifiers.super_key);
        assert_eq!(zoom_reset.key, "0");

        let screenshot = kb
            .binds
            .iter()
            .find(|b| b.action == Action::Screenshot)
            .unwrap();
        assert_eq!(screenshot.modifiers, Modifiers::default());
        assert_eq!(screenshot.key, "Print");
    }

    #[test]
    fn repeat_defaults_are_limited_to_continuous_actions() {
        for action in [
            Action::FocusCycle(FocusCycleDirection::Forward),
            Action::FocusDirection(Direction::Left),
            Action::MoveNode(Direction::Right),
            Action::ClusterTileFocus(Direction::Up),
            Action::ClusterTileSwap(Direction::Down),
            Action::MonitorFocus(MonitorTarget::Direction(Direction::Left)),
            Action::ZoomIn,
            Action::ZoomOut,
        ] {
            assert!(action.repeats_by_default(), "{action:?}");
        }
        for action in [
            Action::Quit,
            Action::CloseFocusedWindow,
            Action::ToggleFullscreen,
            Action::ToggleFieldMaximize,
            Action::ToggleState,
            Action::ArrangeVisible,
            Action::UndoArrange,
            Action::Apogee,
            Action::BearingsShow,
            Action::BearingsToggle,
            Action::CenterLastFocused,
            Action::ClusterMode,
            Action::ClusterLayoutCycle,
            Action::ClusterToggleFloat,
            Action::ClusterSlot(1),
            Action::OpenTerminal,
            Action::ZoomReset,
            Action::Screenshot,
            Action::Spawn("command".to_string()),
        ] {
            assert!(!action.repeats_by_default(), "{action:?}");
        }
    }
}
