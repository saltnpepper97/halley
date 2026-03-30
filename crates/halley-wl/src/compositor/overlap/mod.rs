#![allow(unused_imports)]

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_config::RuntimeTuning;
use halley_core::decay::DecayLevel;
use halley_core::field::{Field, NodeId, Vec2, Visibility};
use halley_core::viewport::{FocusRing, FocusZone};
use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};

use crate::activity::{CommitActivity, VisualState};
use crate::animation::{AnimSpec, AnimStyle};
use crate::compositor::ctx::InteractionCtx;
use crate::compositor::root::Halley;
use crate::render::{DebugScene, build_debug_scene};

pub mod physics;
pub mod read;
pub mod system;
