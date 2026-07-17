use halley_core::field::NodeId;
use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};
use smithay::utils::{Logical, Point};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PointerContents {
    pub(crate) monitor: Option<String>,
    pub(crate) surface: Option<ObjectId>,
    pub(crate) root_surface: Option<ObjectId>,
    pub(crate) node_id: Option<NodeId>,
    pub(crate) is_layer_surface: bool,
    pub(crate) is_session_lock_surface: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PointerFocusState {
    pub(crate) contents: PointerContents,
    /// Exact ungrabbed focus produced by normal pointer routing. New pointer
    /// constraints may activate only from this live target.
    pub(crate) target: Option<(WlSurface, Point<f64, Logical>)>,
    pub(crate) surface_origin: Option<(ObjectId, f64, f64)>,
    pub(crate) seat_focus: Option<(WlSurface, Point<f64, Logical>)>,
}

impl PointerFocusState {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn set(
        &mut self,
        contents: PointerContents,
        focus: Option<&(WlSurface, Point<f64, Logical>)>,
    ) -> bool {
        let changed = self.contents != contents;
        self.contents = contents;
        self.target = focus.cloned();
        self.surface_origin = focus.map(|(surface, origin)| (surface.id(), origin.x, origin.y));
        self.seat_focus = focus.cloned();
        changed
    }
}
