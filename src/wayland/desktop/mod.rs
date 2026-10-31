//! Desktop windows, workspaces, and shell protocols.

pub mod decoration;
/// Staging ext-workspace-v1 enumeration and activation, backed by the cluster model.
pub mod ext_workspace;
pub mod foreign_toplevel;
pub mod fullscreen;
pub mod layer_shell;
pub mod popup;
pub mod xdg_shell;
