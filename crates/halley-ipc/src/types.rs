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
    pub vrr_mode: Option<String>,
    pub vrr_support: Option<String>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeKind {
    Surface,
    Core,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeState {
    Active,
    Drifting,
    Node,
    Core,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: u64,
    pub title: String,
    pub app_id: Option<String>,
    pub output: Option<String>,
    pub kind: NodeKind,
    pub state: NodeState,
    pub visible: bool,
    pub focused: bool,
    pub latest: bool,
    pub pos_x: f32,
    pub pos_y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeOutputGroup {
    pub output: String,
    pub nodes: Vec<NodeInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeListResponse {
    pub outputs: Vec<NodeOutputGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailEntryInfo {
    pub index: usize,
    pub cursor: bool,
    pub node: NodeInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailListResponse {
    pub output: String,
    pub entries: Vec<TrailEntryInfo>,
    pub cursor_index: Option<usize>,
}

impl ModeInfo {
    pub fn display_string(&self) -> String {
        match self.refresh_hz {
            Some(hz) => format!("{}x{} @ {:.2}Hz", self.width, self.height, hz),
            None => format!("{}x{}", self.width, self.height),
        }
    }
}
