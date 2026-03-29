use std::cell::RefCell;
use std::rc::Rc;

use crate::backend::interface::BackendView;
use crate::interaction::types::{ModState, PointerState};

/// Shared context threaded through every input handler.
///
/// Bundles the per-call parameters that were previously repeated on every
/// function signature (`mod_state`, `pointer_state`, `backend`, `config_path`,
/// `wayland_display`), giving each handler a single, cohesive argument instead
/// of a long, unordered parameter list.
///
/// Ownership note: the two `Rc<RefCell<_>>` fields are cheap to clone and are
/// intentionally kept as references-to-shared-state so that handlers can read
/// and write them without threading lifetimes through every sub-call.
pub(crate) struct InputCtx<'a, B: BackendView> {
    pub(crate) mod_state: &'a Rc<RefCell<ModState>>,
    pub(crate) pointer_state: &'a Rc<RefCell<PointerState>>,
    pub(crate) backend: &'a B,
    pub(crate) config_path: &'a str,
    pub(crate) wayland_display: &'a str,
}
