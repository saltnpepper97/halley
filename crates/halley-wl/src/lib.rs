pub mod activity;
pub mod animation;
pub(crate) mod aperture;
pub(crate) mod backend;
pub mod bootstrap;
pub(crate) mod compositor;
pub(crate) mod input;
pub(crate) mod ipc;
pub(crate) mod overlay;
pub(crate) mod protocol;
pub mod render;
pub(crate) mod spatial;

pub use bootstrap::{run, run_winit};
