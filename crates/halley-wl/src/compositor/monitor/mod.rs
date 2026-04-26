#![allow(unused_imports)]

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_core::field::{NodeId, Vec2};
use halley_core::viewport::Viewport;
use smithay::{
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::wayland_server::{Resource, backend::ObjectId, protocol::wl_surface::WlSurface},
    utils::Transform,
};

use crate::compositor::ctx::LayerShellCtx;
use crate::compositor::root::Halley;

pub mod camera;
pub mod layer_shell;
pub mod state;
