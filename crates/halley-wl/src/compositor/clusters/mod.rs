#![allow(unused_imports)]

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_config::RuntimeTuning;
use halley_core::cluster::ClusterId;
use halley_core::field::{Field, NodeId, Vec2, Visibility};
use halley_core::tiling::Rect;
use halley_core::viewport::Viewport;
use smithay::reexports::wayland_server::{Resource, backend::ObjectId, protocol::wl_surface::WlSurface};

use crate::activity::{CommitActivity, VisualState};
use crate::animation::{AnimSpec, AnimStyle};
use crate::compositor::ctx::ClusterCtx;
use crate::render::{DebugScene, build_debug_scene};
use crate::compositor::root::Halley;

pub mod read;
pub mod state;
pub mod system;
