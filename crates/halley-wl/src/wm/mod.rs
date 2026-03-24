#![allow(unused_imports)]

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_config::RuntimeTuning;
use halley_core::cluster::ActiveLayoutMode;
use halley_core::decay::DecayLevel;
use halley_core::field::{Field, NodeId, Vec2, Visibility};
use halley_core::viewport::{FocusRing, FocusZone};
use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};

use crate::activity::{CommitActivity, VisualState};
use crate::animation::{AnimSpec, AnimStyle};
use crate::render::{DebugScene, build_debug_scene};
use crate::state::Halley;

mod carry;
mod focus;
mod fullscreen;
mod maintenance;
pub(crate) mod overlap;
mod trail;
mod workspace;
mod zoom;
