use crate::input::grab::{Grab, ResizeAnchor};
use crate::input::pointer::WheelAccumulator;
use crate::input::{SuppressedButtons, SuppressedKeys};

/// Mutable state shared by compositor-owned input interactions.
///
/// These values advance and retire together across grabs, modal interception,
/// focus changes, and pointer constraints. Grouping them makes that lifecycle
/// explicit and keeps the session coordinator from exposing six independent
/// mutation points.
pub struct InteractionState {
    pub(crate) grab: Grab,
    pub(crate) resize_anchor: Option<ResizeAnchor>,
    pub(crate) suppressed_buttons: SuppressedButtons,
    pub(crate) suppressed_keys: SuppressedKeys,
    pub(crate) wheel_accumulator: WheelAccumulator,
    pub(crate) field_arrange: super::arrange::ArrangeTransactions,
    pub(crate) pointer_constraints: super::pointer::PointerConstraintLifecycle,
    pub(crate) client_pointer_route: Option<super::pointer::ClientPointerRoute>,
    pub(crate) titlebar_hovered: Option<crate::titlebar::ButtonTarget>,
    pub(crate) steam_close_pressed: Option<smithay::desktop::Window>,
    pub(crate) titlebar_pressed: Option<crate::titlebar::ButtonTarget>,
    pub(crate) titlebar_last_click: Option<crate::titlebar::LastClick>,
}

impl Default for InteractionState {
    fn default() -> Self {
        Self {
            grab: Grab::None,
            resize_anchor: None,
            suppressed_buttons: SuppressedButtons::default(),
            suppressed_keys: SuppressedKeys::default(),
            wheel_accumulator: WheelAccumulator::default(),
            field_arrange: super::arrange::ArrangeTransactions::default(),
            pointer_constraints: super::pointer::PointerConstraintLifecycle::default(),
            client_pointer_route: None,
            titlebar_hovered: None,
            titlebar_pressed: None,
            steam_close_pressed: None,
            titlebar_last_click: None,
        }
    }
}
