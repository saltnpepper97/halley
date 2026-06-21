pub mod error;
pub mod protocol;
pub mod types;

pub use error::ApiError;
pub use protocol::{
    BearingsRequest, CaptureMode, CaptureRequest, ClusterRequest, ClusterTarget, CompositorRequest,
    DpmsCommand, MonitorFocusDirection, MonitorFocusTarget, MonitorRequest, NodeMoveDirection,
    NodeRequest, NodeSelector, PORTAL_CURSOR_MODE_EMBEDDED, PORTAL_CURSOR_MODE_HIDDEN,
    PORTAL_CURSOR_MODE_METADATA, PORTAL_SOURCE_TYPE_MONITOR, PORTAL_SOURCE_TYPE_VIRTUAL,
    PORTAL_SOURCE_TYPE_WINDOW, PortalOutput, PortalScreenCastRequest, PortalScreenCastResponse,
    PortalSourceSelection, PortalWindowSource, Request, Response, StackCycleDirection,
    StackRequest, TileRequest, TrailDirection, TrailRequest, TrailTarget,
};
pub use types::{
    ApertureMode, ApertureOutputStatus, ApertureStatusResponse, BearingsStatusResponse,
    CaptureStatusResponse, ClusterDraftAppLaunch, ClusterDraftRequest, ClusterDraftSource,
    ClusterInfo, ClusterLayoutKind, ClusterListResponse, ClusterOutputGroup, ClusterSummary,
    GamescopeTargetResponse, LiftResultKind, LiftSearchResponse, LiftSearchResult,
    LogicalOutputInfo, ModeInfo, NodeInfo, NodeKind, NodeListResponse, NodeOutputGroup,
    NodeProtocolFamily, NodeRelationInfo, NodeRole, NodeState, OutputInfo, OutputStatus,
    OutputsResponse, TrailEntryInfo, TrailListResponse, VersionInfo,
};

pub const HALLEY_API_VERSION: u32 = 3;
