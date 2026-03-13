use serde::{Deserialize, Serialize};

use crate::error::IpcError;
use crate::types::OutputsResponse;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Quit,
    Reload,
    Outputs,
    DockingBegin,
    DockingEnd,
    NodeMove(NodeMoveDirection),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Reloaded,
    Outputs(OutputsResponse),
    Error(IpcError),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum NodeMoveDirection {
    Left,
    Right,
    Up,
    Down,
}
