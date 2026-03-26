use std::fmt;
use std::io;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcError {
    InvalidRequest(String),
    NotFound(String),
    Ambiguous(String),
    Unsupported(String),
    Internal(String),
}

#[derive(Debug)]
pub enum CodecError {
    Io(io::Error),
    Encode(postcard::Error),
    Decode(postcard::Error),
    FrameTooLarge(u32),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "i/o error: {e}"),
            Self::Encode(e) => write!(f, "encode error: {e}"),
            Self::Decode(e) => write!(f, "decode error: {e}"),
            Self::FrameTooLarge(len) => write!(f, "frame too large: {len} bytes"),
        }
    }
}

impl std::error::Error for CodecError {}

impl From<io::Error> for CodecError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
