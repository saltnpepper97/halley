use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PointerConstraintState {
    /// Constraint owner and global Smithay origin, held for the complete
    /// constraint lifetime. Cursor and camera coordinates must not replace it.
    pub(crate) active: Option<(WlSurface, Point<f64, Logical>)>,
}

impl PointerConstraintState {
    pub(crate) fn activate(&mut self, surface: WlSurface, origin: Point<f64, Logical>) {
        self.active = Some((surface, origin));
    }

    pub(crate) fn clear(&mut self) {
        self.active = None;
    }
}
