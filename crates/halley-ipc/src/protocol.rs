use serde::{Deserialize, Serialize};

use crate::error::IpcError;
use crate::types::OutputsResponse;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum NodeMoveDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TrailDirection {
    Prev,
    Next,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DpmsCommand {
    Off,
    On,
    Toggle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Quit,
    Reload,
    Outputs,
    NodeMove(NodeMoveDirection),
    Trail(TrailDirection),
    Dpms(DpmsCommand),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Reloaded,
    Outputs(OutputsResponse),
    Error(IpcError),
}
