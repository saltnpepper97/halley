use serde::{Deserialize, Serialize};

use crate::error::IpcError;
use crate::types::OutputsResponse;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Quit,
    Reload,
    Outputs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Reloaded,
    Outputs(OutputsResponse),
    Error(IpcError),
}
