use std::path::PathBuf;

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            pub const fn new(value: u64) -> Self {
                Self(value)
            }
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl From<u64> for $name {
            fn from(value: u64) -> Self {
                Self(value)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_type!(NodeId);
id_type!(ClusterId);
id_type!(ClusterDraftId);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub compositor_version: String,
    pub api_version: u32,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DpmsCommand {
    Off,
    On,
    Toggle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BearingsCommand {
    Show,
    Hide,
    Toggle,
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
    Id(NodeId),
    Title(String),
    App(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrailDirection {
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
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
pub enum MonitorTarget {
    Direction(Direction),
    Output(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureMode {
    Menu,
    Region,
    Screen,
    Window,
}

/// Rectangle relative to an output's top-left, measured in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Options for local screenshots. Saved-image notifications are enabled by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureOptions {
    /// Suppress the saved-image notification, including its preview and Copy/Open actions.
    /// Interactive selection is still required when the target is incomplete.
    pub silent: bool,
}

/// Fully specified targets capture without selection UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureTarget {
    Menu {
        output: Option<String>,
    },
    Screen {
        output: Option<String>,
    },
    Window {
        node_id: NodeId,
    },
    Region {
        output: Option<String>,
        rect: Option<CaptureRect>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureOutcome {
    Saved(PathBuf),
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrailTarget {
    Index(usize),
    Selector(NodeSelector),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrailEntry {
    pub index: usize,
    pub cursor: bool,
    pub node: NodeInfo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrailInfo {
    pub output: String,
    pub cursor_index: Option<usize>,
    pub entries: Vec<TrailEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Surface,
    Core,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeState {
    Active,
    Drifting,
    Node,
    Core,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeRole {
    NormalToplevel,
    Dialog,
    Popup,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeProtocolFamily {
    XdgToplevel,
    XdgPopup,
    Xwayland,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: NodeId,
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
    pub parent: Option<NodeId>,
    pub transient_for: Option<NodeId>,
    pub child_popup_count: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModeInfo {
    pub width: i32,
    pub height: i32,
    pub refresh_millihz: i32,
    pub preferred: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub modes: Vec<ModeInfo>,
    pub current_mode: Option<usize>,
    pub offset_x: i32,
    pub offset_y: i32,
    /// Physical pixels per logical desktop unit.
    #[serde(default = "default_output_scale")]
    pub scale: f64,
    pub vrr: String,
    pub vrr_supported: bool,
    pub vrr_active: bool,
}

fn default_output_scale() -> f64 {
    1.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClusterLayout {
    Tiling,
    Stacking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClusterTarget {
    Current,
    Id(ClusterId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterSummary {
    pub id: ClusterId,
    pub slot: Option<u8>,
    pub name: String,
    pub output: String,
    pub layout: ClusterLayout,
    pub member_count: usize,
    pub active: bool,
    pub focused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClusterInfo {
    pub summary: ClusterSummary,
    pub core_node_id: Option<NodeId>,
    pub members: Vec<NodeInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterDraftApp {
    pub app_id: String,
    pub command: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterDraft {
    pub name_hint: Option<String>,
    pub apps: Vec<ClusterDraftApp>,
    pub running_nodes: Vec<NodeId>,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub sequence: u64,
    pub outputs: Vec<OutputInfo>,
    pub nodes: Vec<NodeInfo>,
    pub clusters: Vec<ClusterSummary>,
    pub config_path: Option<PathBuf>,
}

impl From<halley_ipc::ModeInfo> for ModeInfo {
    fn from(v: halley_ipc::ModeInfo) -> Self {
        Self {
            width: v.width,
            height: v.height,
            refresh_millihz: v.refresh_millihz,
            preferred: v.preferred,
        }
    }
}

impl From<halley_ipc::OutputInfo> for OutputInfo {
    fn from(v: halley_ipc::OutputInfo) -> Self {
        Self {
            name: v.name,
            modes: v.modes.into_iter().map(Into::into).collect(),
            current_mode: v.current_mode,
            offset_x: v.offset_x,
            offset_y: v.offset_y,
            scale: v.scale,
            vrr: v.vrr,
            vrr_supported: v.vrr_supported,
            vrr_active: v.vrr_active,
        }
    }
}

impl From<halley_ipc::NodeInfo> for NodeInfo {
    fn from(v: halley_ipc::NodeInfo) -> Self {
        Self {
            id: NodeId(v.id),
            title: v.title,
            app_id: v.app_id,
            output: v.output,
            kind: match v.kind {
                halley_ipc::NodeKind::Surface => NodeKind::Surface,
                halley_ipc::NodeKind::Core => NodeKind::Core,
            },
            state: match v.state {
                halley_ipc::NodeState::Active => NodeState::Active,
                halley_ipc::NodeState::Drifting => NodeState::Drifting,
                halley_ipc::NodeState::Node => NodeState::Node,
                halley_ipc::NodeState::Core => NodeState::Core,
            },
            visible: v.visible,
            focused: v.focused,
            latest: v.latest,
            pinned: v.pinned,
            role: match v.role {
                halley_ipc::NodeRole::NormalToplevel => NodeRole::NormalToplevel,
                halley_ipc::NodeRole::Dialog => NodeRole::Dialog,
                halley_ipc::NodeRole::Popup => NodeRole::Popup,
                halley_ipc::NodeRole::Unknown => NodeRole::Unknown,
            },
            protocol_family: match v.protocol_family {
                halley_ipc::NodeProtocolFamily::XdgToplevel => NodeProtocolFamily::XdgToplevel,
                halley_ipc::NodeProtocolFamily::XdgPopup => NodeProtocolFamily::XdgPopup,
                halley_ipc::NodeProtocolFamily::Xwayland => NodeProtocolFamily::Xwayland,
                halley_ipc::NodeProtocolFamily::Unknown => NodeProtocolFamily::Unknown,
            },
            modal: v.modal,
            parent: v.parent.and_then(|r| r.node_id).map(NodeId),
            transient_for: v.transient_for.and_then(|r| r.node_id).map(NodeId),
            child_popup_count: v.child_popup_count,
            x: v.pos_x,
            y: v.pos_y,
            width: v.width,
            height: v.height,
        }
    }
}

impl From<halley_ipc::TrailListResponse> for TrailInfo {
    fn from(v: halley_ipc::TrailListResponse) -> Self {
        Self {
            output: v.output,
            cursor_index: v.cursor_index,
            entries: v
                .entries
                .into_iter()
                .map(|entry| TrailEntry {
                    index: entry.index,
                    cursor: entry.cursor,
                    node: entry.node.into(),
                })
                .collect(),
        }
    }
}

impl From<halley_ipc::ClusterSummary> for ClusterSummary {
    fn from(v: halley_ipc::ClusterSummary) -> Self {
        Self {
            id: ClusterId(v.id),
            slot: v.slot,
            name: v.name,
            output: v.output,
            layout: match v.layout {
                halley_ipc::ClusterLayoutKind::Tiling => ClusterLayout::Tiling,
                halley_ipc::ClusterLayoutKind::Stacking => ClusterLayout::Stacking,
            },
            member_count: v.member_count,
            active: v.active,
            focused: v.focused,
        }
    }
}

impl From<halley_ipc::ClusterInfo> for ClusterInfo {
    fn from(v: halley_ipc::ClusterInfo) -> Self {
        Self {
            summary: v.summary.into(),
            core_node_id: v.core_node_id.map(NodeId),
            members: v.members.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<halley_ipc::StateSnapshot> for Snapshot {
    fn from(v: halley_ipc::StateSnapshot) -> Self {
        Self {
            sequence: v.sequence,
            outputs: v.outputs.into_iter().map(Into::into).collect(),
            nodes: v.nodes.into_iter().map(Into::into).collect(),
            clusters: v.clusters.into_iter().map(Into::into).collect(),
            config_path: v.config_path.map(PathBuf::from),
        }
    }
}
