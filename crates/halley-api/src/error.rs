use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApiError {
    InvalidRequest(String),
    NotFound(String),
    Ambiguous(String),
    Unsupported(String),
    Internal(String),
}
