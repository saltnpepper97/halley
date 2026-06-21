use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::types::{
    BearingsStatusResponse, ClusterDraftRequest, ClusterInfo, ClusterListResponse, NodeInfo,
    NodeListResponse, OutputsResponse, TrailListResponse,
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
    Open {
        target: ClusterTarget,
        output: Option<String>,
    },
    OpenFinalizeDraft {
        draft: ClusterDraftRequest,
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

// ---------------------------------------------------------------------------
// Portal ScreenCast IPC
//
// These types are used by the standalone xdg-desktop-portal-halley backend to
// communicate with the Halley compositor over the existing halley.sock IPC.
// The compositor owns output listing, source selection, and (later) frame
// capture. The portal backend owns D-Bus, session state, and (later) PipeWire.
// ---------------------------------------------------------------------------

/// Bitmask values matching org.freedesktop.impl.portal.ScreenCast source types.
pub const PORTAL_SOURCE_TYPE_MONITOR: u32 = 1;
pub const PORTAL_SOURCE_TYPE_WINDOW: u32 = 2;
pub const PORTAL_SOURCE_TYPE_VIRTUAL: u32 = 4;

/// Bitmask values matching org.freedesktop.impl.portal.ScreenCast cursor modes.
pub const PORTAL_CURSOR_MODE_HIDDEN: u32 = 1;
pub const PORTAL_CURSOR_MODE_EMBEDDED: u32 = 2;
pub const PORTAL_CURSOR_MODE_METADATA: u32 = 4;

// ---------------------------------------------------------------------------
// Screencast shared-memory layout
//
// Single source of truth for the shm file the compositor writes and the portal's
// PipeWire producer reads. Layout (all little-endian):
//
//   [SHM_FRAME_HEADER bytes]  magic("HALS"), width, height, stride, sequence
//   [SHM_CURSOR_BLOCK bytes]  cursor metadata for METADATA cursor mode
//   [pixels]                  XRGB8888 frame, `stride * height` bytes
//
// The cursor block lets the compositor ship the pointer as PipeWire
// `SPA_META_Cursor` metadata (so consumers like OBS can toggle/draw it
// client-side) instead of baking it into the frame pixels.
// ---------------------------------------------------------------------------

/// Frame header size (magic + dims + stride + sequence).
pub const SHM_FRAME_HEADER: usize = 32;

/// Max cursor bitmap dimensions carried in the cursor block (BGRA, 4 bytes/px).
pub const SHM_CURSOR_MAX_W: usize = 256;
pub const SHM_CURSOR_MAX_H: usize = 256;

/// Size of the fixed scalar fields at the start of the cursor block. Field byte
/// offsets *within the cursor block*:
///   serial:    u64 @ 0
///   visible:   u32 @ 8   (1 = cursor present in the captured region this frame)
///   pos_x:     i32 @ 12  (stream-pixel coords, top-left origin)
///   pos_y:     i32 @ 16
///   hotspot_x: i32 @ 20
///   hotspot_y: i32 @ 24
///   width:     u32 @ 28  (bitmap dimensions / stride)
///   height:    u32 @ 32
///   stride:    u32 @ 36
/// Bitmap BGRA bytes follow at `SHM_CURSOR_FIELDS`.
pub const SHM_CURSOR_FIELDS: usize = 40;
pub const SHM_CURSOR_OFF_SERIAL: usize = 0;
pub const SHM_CURSOR_OFF_VISIBLE: usize = 8;
pub const SHM_CURSOR_OFF_POS_X: usize = 12;
pub const SHM_CURSOR_OFF_POS_Y: usize = 16;
pub const SHM_CURSOR_OFF_HOTSPOT_X: usize = 20;
pub const SHM_CURSOR_OFF_HOTSPOT_Y: usize = 24;
pub const SHM_CURSOR_OFF_WIDTH: usize = 28;
pub const SHM_CURSOR_OFF_HEIGHT: usize = 32;
pub const SHM_CURSOR_OFF_STRIDE: usize = 36;

/// Bytes reserved for the cursor bitmap (BGRA).
pub const SHM_CURSOR_BITMAP_BYTES: usize = SHM_CURSOR_MAX_W * SHM_CURSOR_MAX_H * 4;
/// Total cursor block size.
pub const SHM_CURSOR_BLOCK: usize = SHM_CURSOR_FIELDS + SHM_CURSOR_BITMAP_BYTES;
/// Absolute byte offset of frame pixels within the shm file.
pub const SHM_PIXELS_OFFSET: usize = SHM_FRAME_HEADER + SHM_CURSOR_BLOCK;
/// Absolute byte offset of the cursor block within the shm file.
pub const SHM_CURSOR_OFFSET: usize = SHM_FRAME_HEADER;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PortalScreenCastRequest {
    /// List all outputs suitable for monitor capture, with focus flag.
    ListOutputs,
    /// Auto-select the best output for screencast (focused → primary → first).
    SelectOutput { session_handle: String },
    /// Begin streaming the given output. Returns stream metadata.
    /// For now this is metadata-only; PipeWire node creation comes later.
    Start {
        session_handle: String,
        output: String,
        cursor_mode: u32,
    },
    /// Begin streaming a specific window (node). The compositor captures the
    /// window's live screen rect each frame, cropped from its host output.
    StartWindow {
        session_handle: String,
        node_id: u64,
        cursor_mode: u32,
    },
    /// Stop a previously started stream.
    Stop { session_handle: String },
    /// Open the Halley-native source chooser overlay. `source_types` is a mask
    /// of the portal source types the calling app is willing to accept. The
    /// compositor shows the picker and resolves the result asynchronously via
    /// `PollSourceChooser`. Returns immediately with `SourceChooserStarted`.
    StartSourceChooser {
        session_handle: String,
        source_types: u32,
    },
    /// Poll the active source chooser for this session. The portal backend
    /// calls this in a loop until it gets a terminal result.
    PollSourceChooser { session_handle: String },
    /// Cancel an active source chooser (e.g. the D-Bus request was closed).
    CancelSourceChooser { session_handle: String },
    /// Notify the compositor that the PipeWire stream changed active state.
    /// When `active` is false, the compositor should stop fresh captures to
    /// avoid wasting GPU/CPU work when no consumer is pulling frames.
    SetActive {
        session_handle: String,
        active: bool,
    },
    /// Register a PipeWire DMA-BUF buffer with the compositor. The buffer fds
    /// are sent out-of-band via SCM_RIGHTS on the same IPC frame.
    AddDmabufBuffer {
        session_handle: String,
        buffer_id: u64,
        width: i32,
        height: i32,
        format: u32,
        modifier: u64,
        flags: u32,
        planes: Vec<PortalDmabufPlane>,
    },
    /// Remove a previously registered PipeWire DMA-BUF buffer.
    RemoveDmabufBuffer {
        session_handle: String,
        buffer_id: u64,
    },
    /// Render one frame into a registered PipeWire DMA-BUF buffer.
    RenderDmabufBuffer {
        session_handle: String,
        buffer_id: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortalDmabufPlane {
    pub fd_index: u32,
    pub plane_index: u32,
    pub offset: u32,
    pub stride: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PortalScreenCastResponse {
    /// Available outputs for monitor capture.
    Outputs(Vec<PortalOutput>),
    /// Auto-selected output, if any.
    SelectedOutput(Option<PortalOutput>),
    /// Stream metadata from Start. node_id is filled by the portal's PipeWire
    /// stream; the compositor returns 0 as a placeholder. shm_path is the
    /// shared-memory file the compositor writes frames into.
    Started {
        node_id: u32,
        width: i32,
        height: i32,
        offset_x: i32,
        offset_y: i32,
        source_type: u32,
        mapping_id: String,
        shm_path: String,
    },
    /// Stream stopped cleanly.
    Stopped,
    /// Error from the compositor.
    Error(String),
    /// The chooser overlay was opened successfully. The portal should now poll.
    SourceChooserStarted,
    /// The chooser is still open; the user has not confirmed or cancelled yet.
    SourceChooserPending,
    /// The user confirmed a source. Carries the resolved target the portal
    /// should stream.
    SourceChooserSelected(PortalSourceSelection),
    /// The user cancelled, or the chooser was dismissed/timed out.
    SourceChooserCancelled,
    /// Acknowledgement of a SetActive request.
    ActiveSet,
    /// Acknowledgement that a DMA-BUF buffer was registered.
    DmabufBufferAdded,
    /// Acknowledgement that a DMA-BUF buffer was removed.
    DmabufBufferRemoved,
    /// Acknowledgement that a frame was rendered into a DMA-BUF buffer.
    DmabufFrameRendered,
}

/// A source picked from the chooser overlay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PortalSourceSelection {
    Monitor(PortalOutput),
    Window(PortalWindowSource),
}

/// A window (node) target for portal screencast.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortalWindowSource {
    pub node_id: u64,
    pub title: String,
    pub app_id: Option<String>,
    pub output: String,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortalOutput {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub focused: bool,
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
    Version,
    /// Resolve a gamescope monitor selector (`focused`, `cursor`, `primary`, or a
    /// connector name) to that monitor's current dimensions, computed live.
    GamescopeTarget {
        selector: String,
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
    PortalScreenCast(PortalScreenCastRequest),
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
    Error(ApiError),
    Version(crate::types::VersionInfo),
    GamescopeTarget(crate::types::GamescopeTargetResponse),
    PortalScreenCast(PortalScreenCastResponse),
}
