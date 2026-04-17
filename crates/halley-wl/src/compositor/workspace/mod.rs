#![allow(unused_imports)]

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_core::decay::DecayLevel;
use halley_core::field::{NodeId, Vec2};
use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use crate::compositor::activity::CommitActivity;
use crate::compositor::ctx::{SurfaceLifecycleCtx, WorkspaceCtx};
use crate::compositor::root::Halley;

pub mod lifecycle;
pub mod state;
