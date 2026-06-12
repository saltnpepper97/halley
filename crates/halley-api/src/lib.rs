pub mod error;
pub mod protocol;
pub mod types;

pub use error::ApiError;
pub use protocol::{
    BearingsRequest, CaptureMode, CaptureRequest, ClusterRequest, ClusterTarget, CompositorRequest,
    DpmsCommand, MonitorFocusDirection, MonitorFocusTarget, MonitorRequest, NodeMoveDirection,
    NodeRequest, NodeSelector, Request, Response, StackCycleDirection, StackRequest, TileRequest,
    TrailDirection, TrailRequest, TrailTarget,
};
pub use types::{
    ApertureMode, ApertureOutputStatus, ApertureStatusResponse, BearingsStatusResponse,
    CaptureStatusResponse, ClusterDraftAppLaunch, ClusterDraftRequest, ClusterDraftSource,
    ClusterInfo, ClusterLayoutKind, ClusterListResponse, ClusterOutputGroup, ClusterSummary,
    GamescopeTargetResponse, LensResultKind, LensSearchResponse, LensSearchResult,
    LogicalOutputInfo, ModeInfo, NodeInfo, NodeKind, NodeListResponse, NodeOutputGroup,
    NodeProtocolFamily, NodeRelationInfo, NodeRole, NodeState, OutputInfo, OutputStatus,
    OutputsResponse, TrailEntryInfo, TrailListResponse, VersionInfo,
};

pub const HALLEY_API_VERSION: u32 = 1;
