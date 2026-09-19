use serde::{Deserialize, Serialize};

/// Bumped whenever `Request`/`Response` change shape. `postcard` encodes
/// enum variants positionally (by index, not by name), so unlike the tidy
/// config parsers elsewhere in this project, adding a variant anywhere but
/// the end of `Request`/`Response` silently breaks wire-compatibility with
/// a differently-versioned build - worth remembering as this grows, not
/// solved here (this first pass has nothing to negotiate against yet).
pub const HALLEY_IPC_VERSION: u32 = 22;
pub const HALLEY_API_VERSION: u32 = 1;

/// A request from `halleyctl`, the portal backend, or another local client.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    Outputs,
    Version,
    Screenshot(ScreenshotRequest),
    CancelScreenshot {
        request_handle: String,
    },
    ChooseSource(SourceChooserRequest),
    CancelSourceChooser {
        request_handle: String,
    },
    RegisterDmabuf(RegisterDmabufRequest),
    RemoveDmabuf {
        stream_handle: String,
        buffer_id: u64,
    },
    CaptureFrame(CaptureFrameRequest),
    Node(NodeRequest),
    Bearings(BearingsRequest),
    Quit,
    ConfigPath,
    Dpms {
        command: DpmsCommand,
        output: Option<String>,
    },
    Cluster(ClusterRequest),
    CaptureCapabilities,
    /// Negotiate the external SDK contract before issuing API requests.
    Hello(HelloRequest),
    /// Turn this connection into a dedicated event stream.
    Subscribe(SubscribeRequest),
    ConfigReload,
    Trail(TrailRequest),
    LocalCapture(LocalCaptureRequest),
    Control(ControlRequest),
}

/// The compositor's reply to a `Request`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    Outputs(OutputsResponse),
    Version(VersionInfo),
    Screenshot(ScreenshotResponse),
    Source(SourceChooserResponse),
    Frame(CaptureFrameResponse),
    Ack,
    Error(String),
    NodeList(NodeListResponse),
    NodeInfo(NodeInfo),
    BearingsStatus(BearingsStatusResponse),
    ConfigPath(Option<String>),
    ClusterList(ClusterListResponse),
    ClusterInfo(ClusterInfo),
    CaptureCapabilities(CaptureCapabilities),
    Hello(ServerInfo),
    Subscribed(StateSnapshot),
    Event(ApiEvent),
    ApiError(ServerError),
    ClusterDraftStarted { id: u64 },
    TrailList(TrailListResponse),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerErrorKind {
    InvalidRequest,
    NotFound,
    Ambiguous,
    Unsupported,
    VersionMismatch,
    Busy,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerError {
    pub kind: ServerErrorKind,
    pub message: String,
}

impl ServerError {
    pub fn new(kind: ServerErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloRequest {
    pub api_version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub compositor_version: String,
    pub api_version: u32,
    pub ipc_protocol: u32,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventTopic {
    Outputs,
    Nodes,
    NodeGeometry,
    Clusters,
    Config,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeRequest {
    pub api_version: u32,
    pub topics: Vec<EventTopic>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub sequence: u64,
    pub outputs: Vec<OutputInfo>,
    pub nodes: Vec<NodeInfo>,
    pub clusters: Vec<ClusterSummary>,
    pub config_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ApiEvent {
    OutputAdded {
        sequence: u64,
        output: OutputInfo,
    },
    OutputChanged {
        sequence: u64,
        output: OutputInfo,
    },
    OutputRemoved {
        sequence: u64,
        name: String,
    },
    NodeAdded {
        sequence: u64,
        node: NodeInfo,
    },
    NodeChanged {
        sequence: u64,
        node: NodeInfo,
    },
    NodeGeometryChanged {
        sequence: u64,
        id: u64,
        pos_x: f32,
        pos_y: f32,
        width: f32,
        height: f32,
    },
    NodeRemoved {
        sequence: u64,
        id: u64,
    },
    ClusterAdded {
        sequence: u64,
        cluster: ClusterSummary,
    },
    ClusterChanged {
        sequence: u64,
        cluster: ClusterSummary,
    },
    ClusterRemoved {
        sequence: u64,
        id: u64,
    },
    ConfigReloaded {
        sequence: u64,
        accepted: bool,
    },
    ClusterDraftChanged {
        sequence: u64,
        id: u64,
        state: ClusterDraftState,
        message: Option<String>,
    },
}

impl ApiEvent {
    pub fn sequence(&self) -> u64 {
        match self {
            Self::OutputAdded { sequence, .. }
            | Self::OutputChanged { sequence, .. }
            | Self::OutputRemoved { sequence, .. }
            | Self::NodeAdded { sequence, .. }
            | Self::NodeChanged { sequence, .. }
            | Self::NodeGeometryChanged { sequence, .. }
            | Self::NodeRemoved { sequence, .. }
            | Self::ClusterAdded { sequence, .. }
            | Self::ClusterChanged { sequence, .. }
            | Self::ClusterRemoved { sequence, .. }
            | Self::ConfigReloaded { sequence, .. }
            | Self::ClusterDraftChanged { sequence, .. } => *sequence,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmabufFormat {
    pub fourcc: u32,
    pub modifier: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureCapabilities {
    pub main_device: Option<u64>,
    pub dmabuf_formats: Vec<DmabufFormat>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClusterLayoutKind {
    Tiling,
    Stacking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClusterTarget {
    Current,
    Id(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClusterRequest {
    List {
        output: Option<String>,
    },
    Inspect {
        target: ClusterTarget,
        output: Option<String>,
    },
    LayoutCycle {
        output: Option<String>,
    },
    Slot {
        slot: u8,
        output: Option<String>,
    },
    Open {
        target: ClusterTarget,
        output: Option<String>,
    },
    OpenFinalizeDraft {
        draft: ClusterDraftRequest,
        output: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClusterDraftSource {
    HalleyLift,
    External,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClusterDraftState {
    Started,
    AwaitingName,
    Launching,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterDraftAppLaunch {
    pub app_id: String,
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterDraftRequest {
    pub name_hint: Option<String>,
    pub app_launches: Vec<ClusterDraftAppLaunch>,
    pub running_node_ids: Vec<u64>,
    pub source: ClusterDraftSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterSummary {
    pub id: u64,
    pub slot: Option<u8>,
    pub name: String,
    pub output: String,
    pub layout: ClusterLayoutKind,
    pub member_count: usize,
    pub active: bool,
    pub focused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterOutputGroup {
    pub output: String,
    pub clusters: Vec<ClusterSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterListResponse {
    pub outputs: Vec<ClusterOutputGroup>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClusterInfo {
    pub summary: ClusterSummary,
    pub core_node_id: Option<u64>,
    pub members: Vec<NodeInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BearingsRequest {
    Show,
    Hide,
    Toggle,
    Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DpmsCommand {
    Off,
    On,
    Toggle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BearingsStatusResponse {
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeMoveDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeSelector {
    Focused,
    Latest,
    Id(u64),
    Title(String),
    App(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeRequest {
    List {
        output: Option<String>,
    },
    Info {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    Focus {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    Move {
        direction: NodeMoveDirection,
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    Close {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    Collapse {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    Restore {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    Toggle {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeKind {
    Surface,
    Core,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeState {
    Active,
    Drifting,
    Node,
    Core,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeRole {
    NormalToplevel,
    Dialog,
    Popup,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeProtocolFamily {
    XdgToplevel,
    XdgPopup,
    Xwayland,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRelationInfo {
    pub node_id: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    pub pinned: bool,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeOutputGroup {
    pub output: String,
    pub nodes: Vec<NodeInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeListResponse {
    pub outputs: Vec<NodeOutputGroup>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrailRequest {
    Previous {
        output: Option<String>,
    },
    Next {
        output: Option<String>,
    },
    List {
        output: Option<String>,
    },
    Goto {
        target: TrailTarget,
        output: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrailTarget {
    Index(usize),
    Selector(NodeSelector),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrailEntryInfo {
    pub index: usize,
    pub cursor: bool,
    pub node: NodeInfo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrailListResponse {
    pub output: String,
    pub cursor_index: Option<usize>,
    pub entries: Vec<TrailEntryInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalCaptureMode {
    Menu,
    Region,
    Screen,
    Window,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalCaptureRequest {
    pub mode: LocalCaptureMode,
    pub output: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StackCycleDirection {
    Forward,
    Backward,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MonitorFocusTarget {
    Direction(ControlDirection),
    Output(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlRequest {
    MonitorFocus(MonitorFocusTarget),
    StackCycle {
        direction: StackCycleDirection,
        output: Option<String>,
    },
    TileFocus {
        direction: ControlDirection,
        output: Option<String>,
    },
    TileSwap {
        direction: ControlDirection,
        output: Option<String>,
    },
    WindowTransfer(ControlDirection),
    PanField(ControlDirection),
    /// Show the one-time Halley basics card again. Manual reopening is always
    /// allowed, independent of whether the first-run card was already
    /// dismissed.
    ShowBasics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenshotTarget {
    Screen,
    Window,
    Area,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotRequest {
    pub request_handle: String,
    pub target: ScreenshotTarget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenshotResponse {
    Saved { path: String },
    Cancelled,
    Failed { message: String },
}

pub const SOURCE_MONITOR: u32 = 1;
pub const SOURCE_WINDOW: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceChooserRequest {
    pub request_handle: String,
    pub source_types: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceChooserResponse {
    Selected(CaptureSource),
    Cancelled,
    Failed { message: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureSource {
    Monitor {
        name: String,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    Window {
        surface_id: u32,
        width: i32,
        height: i32,
    },
}

impl CaptureSource {
    pub fn width(&self) -> i32 {
        match self {
            Self::Monitor { width, .. } | Self::Window { width, .. } => *width,
        }
    }

    pub fn height(&self) -> i32 {
        match self {
            Self::Monitor { height, .. } | Self::Window { height, .. } => *height,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorMode {
    Hidden,
    Embedded,
    Metadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmabufPlane {
    pub fd_index: u32,
    pub plane_index: u32,
    pub offset: u32,
    pub stride: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterDmabufRequest {
    pub stream_handle: String,
    pub buffer_id: u64,
    pub width: i32,
    pub height: i32,
    pub format: u32,
    pub modifier: u64,
    pub flags: u32,
    pub planes: Vec<DmabufPlane>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureBuffer {
    MemFd {
        fd_index: u32,
        offset: u64,
        size: u64,
        stride: u32,
    },
    Dmabuf {
        buffer_id: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureFrameRequest {
    pub stream_handle: String,
    pub source: CaptureSource,
    pub cursor_mode: CursorMode,
    pub buffer: CaptureBuffer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorMetadata {
    pub x: i32,
    pub y: i32,
    pub hotspot_x: i32,
    pub hotspot_y: i32,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureFrameResponse {
    pub cursor: Option<CursorMetadata>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputsResponse {
    pub outputs: Vec<OutputInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    /// Every mode advertised by the connector, in connector order.
    pub modes: Vec<ModeInfo>,
    /// Index of the active mode in `modes`, or `None` when this connected
    /// output could not be initialized.
    pub current_mode: Option<usize>,
    pub offset_x: i32,
    pub offset_y: i32,
    /// Configured VRR policy as "off", "on", or "auto".
    pub vrr: String,
    /// Whether the connector advertises usable variable-refresh support.
    pub vrr_supported: bool,
    /// Effective hardware state for the next submitted frame.
    pub vrr_active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModeInfo {
    pub width: i32,
    pub height: i32,
    /// Refresh rate in Smithay's lossless millihertz representation rather
    /// than an approximate floating value.
    pub refresh_millihz: i32,
    pub preferred: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub ipc_protocol: u32,
}
