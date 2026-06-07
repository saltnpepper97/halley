use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputsResponse {
    pub outputs: Vec<OutputInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub ipc_protocol: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ApertureMode {
    Normal,
    Collapsed,
    Minimal,
    Hidden,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApertureStatusResponse {
    pub output: Option<String>,
    pub mode: ApertureMode,
    pub outputs: Vec<ApertureOutputStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApertureOutputStatus {
    pub output: String,
    pub mode: ApertureMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RailVisibility {
    Visible,
    HiddenEmpty,
    HiddenFullscreen,
    HiddenMaximized,
    HiddenObstructed,
    HiddenTiledCluster,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RailItemInfo {
    pub node_id: u64,
    pub title: String,
    pub app_id: Option<String>,
    pub pinned: bool,
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RailOutputSnapshot {
    pub output: String,
    pub visibility: RailVisibility,
    pub items: Vec<RailItemInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RailStatusResponse {
    pub output: Option<String>,
    pub outputs: Vec<RailOutputSnapshot>,
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
    pub direct_scanout_candidate_node: Option<u64>,
    pub direct_scanout_active_node: Option<u64>,
    pub direct_scanout_reason: Option<String>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NodeRole {
    NormalToplevel,
    Dialog,
    Popup,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NodeProtocolFamily {
    XdgToplevel,
    XdgPopup,
    Xwayland,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeRelationInfo {
    pub node_id: Option<u64>,
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
    pub role: NodeRole,
    pub protocol_family: NodeProtocolFamily,
    pub modal: bool,
    pub parent: Option<NodeRelationInfo>,
    pub transient_for: Option<NodeRelationInfo>,
    pub child_popup_count: usize,
    pub pos_x: f32,
    pub pos_y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ClusterLayoutKind {
    Tiling,
    Stacking,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterSummary {
    pub id: u64,
    pub slot: Option<u8>,
    pub name: Option<String>,
    pub output: Option<String>,
    pub layout: ClusterLayoutKind,
    pub member_count: usize,
    pub active: bool,
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterOutputGroup {
    pub output: String,
    pub clusters: Vec<ClusterSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterListResponse {
    pub outputs: Vec<ClusterOutputGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterInfo {
    pub id: u64,
    pub slot: Option<u8>,
    pub name: Option<String>,
    pub output: Option<String>,
    pub layout: ClusterLayoutKind,
    pub member_count: usize,
    pub active: bool,
    pub focused: bool,
    pub focused_member_index: Option<usize>,
    pub focused_member_id: Option<u64>,
    pub members: Vec<NodeInfo>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BearingsStatusResponse {
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureStatusResponse {
    pub active: bool,
    pub session_serial: Option<u64>,
    pub last_finished_serial: Option<u64>,
    pub saved_path: Option<String>,
    pub error: Option<String>,
}

impl ModeInfo {
    pub fn display_string(&self) -> String {
        match self.refresh_hz {
            Some(hz) => format!("{}x{} @ {:.2}Hz", self.width, self.height, hz),
            None => format!("{}x{}", self.width, self.height),
        }
    }
}
