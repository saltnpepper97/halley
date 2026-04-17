pub(crate) mod policy;
mod surface;

pub(crate) use policy::apply_hover_focus_mode;
pub(crate) use surface::{
    grabbed_layer_surface_focus, layer_surface_focus_for_screen, pointer_focus_for_screen,
};

#[cfg(test)]
pub(crate) use policy::hover_focus_enabled;
