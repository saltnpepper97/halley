//! Reusable Aperture widget core.
//!
//! The compositor decides where config lives and how it is reloaded.
//! This crate only owns config values, decoding, clock state, animation,
//! layout math, and render snapshots.

mod clock;
mod config;
mod geometry;

pub use clock::{ApertureRuntime, ClockSnapshot, PresentationState};
pub use config::{ApertureConfig, ApertureConfigError, ApertureMode, ClockColor, ClockConfig};
pub use geometry::{Point, Rect, Size};
