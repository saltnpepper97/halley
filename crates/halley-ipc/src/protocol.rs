use serde::{Deserialize, Serialize};

use crate::error::IpcError;
use crate::types::{
    BearingsStatusResponse, ClusterInfo, ClusterListResponse, NodeInfo, NodeListResponse,
    OutputsResponse, TrailListResponse,
};

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum StackCycleDirection {
    Forward,
    Backward,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DpmsCommand {
    Off,
    On,
    Toggle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeSelector {
    Focused,
    Latest,
    Id(u64),
    Title(String),
    App(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrailTarget {
    Index(usize),
    Selector(NodeSelector),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MonitorFocusDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MonitorFocusTarget {
    Direction(MonitorFocusDirection),
    Output(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BearingsRequest {
    Show,
    Hide,
    Toggle,
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StackRequest {
    Cycle {
        direction: StackCycleDirection,
        output: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TileRequest {
    Focus {
        direction: NodeMoveDirection,
        output: Option<String>,
    },
    Swap {
        direction: NodeMoveDirection,
        output: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusterTarget {
    Current,
    Id(u64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusterRequest {
    List {
        output: Option<String>,
    },
    Inspect {
        target: Option<ClusterTarget>,
        output: Option<String>,
    },
    LayoutCycle {
        output: Option<String>,
    },
    Slot {
        slot: u8,
        output: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CaptureMode {
    Menu,
    Region,
    Screen,
    Window,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CaptureRequest {
    Start {
        mode: CaptureMode,
        output: Option<String>,
    },
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompositorRequest {
    Quit,
    Reload,
    Outputs,
    ApertureStatus,
    Dpms {
        command: DpmsCommand,
        output: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrailRequest {
    Prev {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MonitorRequest {
    Focus(MonitorFocusTarget),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Compositor(CompositorRequest),
    Capture(CaptureRequest),
    Node(NodeRequest),
    Trail(TrailRequest),
    Monitor(MonitorRequest),
    Bearings(BearingsRequest),
    Stack(StackRequest),
    Tile(TileRequest),
    Cluster(ClusterRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Reloaded,
    Outputs(OutputsResponse),
    ApertureStatus(crate::types::ApertureStatusResponse),
    CaptureStatus(crate::types::CaptureStatusResponse),
    NodeList(NodeListResponse),
    NodeInfo(NodeInfo),
    ClusterList(ClusterListResponse),
    ClusterInfo(ClusterInfo),
    TrailList(TrailListResponse),
    BearingsStatus(BearingsStatusResponse),
    Error(IpcError),
}
