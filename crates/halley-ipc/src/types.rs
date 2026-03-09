use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputsResponse {
    pub outputs: Vec<OutputInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub status: OutputStatus,
    pub enabled: bool,
    pub current_mode: Option<ModeInfo>,
    pub modes: Vec<ModeInfo>,
    pub logical: Option<LogicalOutputInfo>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutputStatus {
    Connected,
    Disconnected,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeInfo {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: Option<f64>,
    pub preferred: bool,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogicalOutputInfo {
    pub scale: f64,
    pub focused: bool,
    pub offset_x: i32,
    pub offset_y: i32,
}

impl ModeInfo {
    pub fn display_string(&self) -> String {
        match self.refresh_hz {
            Some(hz) => format!("{}x{} @ {:.2}Hz", self.width, self.height, hz),
            None => format!("{}x{}", self.width, self.height),
        }
    }
}
