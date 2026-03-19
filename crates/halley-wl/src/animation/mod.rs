mod curves;
mod movement;
mod nodes;
mod surface;

pub(crate) use curves::{ease_in_out_cubic, ease_out_back};
pub(crate) use movement::advance_node_move_anim;
pub use nodes::{AnimSpec, AnimStyle, Animator};
pub(crate) use surface::{active_surface_render_scale, proxy_anim_scale};
