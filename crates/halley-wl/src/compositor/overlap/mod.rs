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

use crate::animation::{AnimSpec, AnimStyle};
use crate::compositor::activity::{CommitActivity, VisualState};
use crate::compositor::ctx::InteractionCtx;
use crate::compositor::debug_scene::{DebugScene, build_debug_scene};
use crate::compositor::root::Halley;

pub mod physics;
pub mod read;
pub mod system;
