//! Stable, Rust-first client API for controlling and observing Halley.
//!
//! [`Client`] keeps one direct Unix-socket connection open for commands and
//! queries. [`Client::subscribe`] creates a second dedicated connection and
//! returns an initial snapshot followed by sequenced, typed changes.

mod client;
mod error;
mod events;
mod types;

pub use client::{Client, ConnectOptions};
pub use error::{Error, ErrorKind, Result};
pub use events::{Event, EventStream, EventTopic, Subscription};
pub use types::*;

/// Version of the public semantic contract exposed by this crate.
pub const HALLEY_API_VERSION: u32 = 1;

#[cfg(any(feature = "wayland-client", feature = "wayland-server"))]
pub mod wayland;

#[cfg(feature = "test-support")]
pub mod test_support;
