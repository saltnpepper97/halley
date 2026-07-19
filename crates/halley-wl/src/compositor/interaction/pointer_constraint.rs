use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};
use smithay::wayland::compositor::RegionAttributes;

#[derive(Clone, Debug)]
pub(crate) struct ActivePointerConstraint {
    pub(crate) surface: WlSurface,
    pub(crate) origin: Point<f64, Logical>,
    pub(crate) locked: bool,
    pub(crate) region: Option<RegionAttributes>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PositionHintAction {
    Reject,
    SyncLocked,
    IgnoreUnlocked,
}

pub(crate) fn position_hint_action(
    constraint_active: bool,
    constraint_locked: bool,
    owns_pointer_focus: bool,
) -> PositionHintAction {
    if !constraint_active || !owns_pointer_focus {
        PositionHintAction::Reject
    } else if constraint_locked {
        PositionHintAction::SyncLocked
    } else {
        PositionHintAction::IgnoreUnlocked
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PointerConstraintController {
    /// Constraint owner and global Smithay origin, held for the complete
    /// constraint lifetime. Cursor and camera coordinates must not replace it.
    pub(crate) active: Option<(WlSurface, Point<f64, Logical>)>,
}

impl PointerConstraintController {
    pub(crate) fn activate(&mut self, surface: WlSurface, origin: Point<f64, Logical>) {
        self.active = Some((surface, origin));
    }

    pub(crate) fn clear(&mut self) {
        self.active = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{PositionHintAction, position_hint_action};

    #[test]
    fn only_owned_locked_constraint_syncs_position_hint() {
        assert_eq!(
            position_hint_action(true, true, true),
            PositionHintAction::SyncLocked
        );
        assert_eq!(
            position_hint_action(true, false, true),
            PositionHintAction::IgnoreUnlocked
        );
        assert_eq!(
            position_hint_action(true, true, false),
            PositionHintAction::Reject
        );
        assert_eq!(
            position_hint_action(false, true, true),
            PositionHintAction::Reject
        );
    }
}
