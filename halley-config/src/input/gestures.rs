use crate::{Action, ModifierKey};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GestureScope {
    #[default]
    EmptyField,
    Global,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollPanMode {
    Off,
    #[default]
    EmptyField,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GestureModifier {
    #[default]
    Keybind,
    Disabled,
    Explicit(ModifierKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GestureSwipeDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GestureAction {
    ZoomFitOpen,
    ZoomFitClose,
    Compositor(Action),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GestureBinding {
    pub direction: GestureSwipeDirection,
    pub fingers: u32,
    pub action: GestureAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GestureHoldBinding {
    pub fingers: u32,
    pub action: GestureAction,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GestureSettings {
    pub enabled: bool,
    pub client_passthrough: bool,
    pub touch_passthrough: bool,
    pub pinch_to_zoom: bool,
    pub pinch_scope: GestureScope,
    pub compositor_scope: GestureScope,
    pub modifier: GestureModifier,
    pub scroll_pan: ScrollPanMode,
    pub pan_fingers: u32,
    pub pan_momentum: bool,
    pub pan_decay_rate: f32,
    pub flick_min_px_per_s: f32,
    pub swipe_threshold_px: f32,
    pub swipe_bindings: Vec<GestureBinding>,
    pub zoom_fit_swipe_bindings: Vec<GestureBinding>,
    pub hold_bindings: Vec<GestureHoldBinding>,
}

impl Default for GestureSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            client_passthrough: true,
            touch_passthrough: true,
            pinch_to_zoom: true,
            pinch_scope: GestureScope::EmptyField,
            compositor_scope: GestureScope::EmptyField,
            modifier: GestureModifier::Keybind,
            scroll_pan: ScrollPanMode::EmptyField,
            pan_fingers: 3,
            pan_momentum: true,
            pan_decay_rate: 6.0,
            flick_min_px_per_s: 200.0,
            swipe_threshold_px: 120.0,
            swipe_bindings: vec![GestureBinding {
                direction: GestureSwipeDirection::Up,
                fingers: 4,
                action: GestureAction::ZoomFitOpen,
            }],
            zoom_fit_swipe_bindings: vec![GestureBinding {
                direction: GestureSwipeDirection::Down,
                fingers: 4,
                action: GestureAction::ZoomFitClose,
            }],
            hold_bindings: Vec::new(),
        }
    }
}
