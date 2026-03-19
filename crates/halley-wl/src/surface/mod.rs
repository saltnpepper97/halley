mod surface_ops;

pub(crate) use surface_ops::{
    current_surface_size_for_node, request_close_focused_toplevel, request_toplevel_resize_mode,
    window_geometry_for_node,
};
