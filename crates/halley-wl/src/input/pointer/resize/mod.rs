use smithay::input::pointer::CursorIcon;

use crate::compositor::interaction::ResizeHandle;

mod anim;
mod geometry;
mod handles;
mod interaction;

pub(crate) use geometry::{
    active_node_screen_rect, active_node_surface_transform_screen_details,
    active_resize_geometry_screen,
};
pub(super) use interaction::{begin_resize, finalize_resize, handle_resize_motion};
pub(crate) use anim::advance_resize_anim;

#[inline]
pub(crate) fn resize_rect_nearly_eq(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.5
}

#[allow(unused_imports)]
pub(crate) fn _cursor_icon_typecheck(_: CursorIcon, _: ResizeHandle) {}

#[cfg(test)]
mod tests;
