#[cfg(test)]
pub(crate) mod model;

use std::time::Instant;

use crate::compositor::root::Halley;

pub(crate) fn activate_rail_item(st: &mut Halley, node_id: halley_core::field::NodeId) -> bool {
    crate::compositor::actions::window::focus_or_reveal_surface_node(st, node_id, Instant::now())
}
