use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use halley_capit::CaptureCrop;
use halley_core::field::NodeId;
use halley_ipc::CaptureMode;

#[derive(Clone, Debug)]
pub(crate) struct ScreenshotSessionState {
    pub(crate) mode: CaptureMode,
    pub(crate) monitor: String,
    pub(crate) selected_window: Option<NodeId>,
    pub(crate) keyboard_captured: bool,
    pub(crate) menu_selected: usize,
    pub(crate) menu_hovered: Option<usize>,
    pub(crate) drag_anchor: Option<(i32, i32)>,
    pub(crate) drag_current: Option<(i32, i32)>,
    pub(crate) selection_rect: Option<CaptureCrop>,
    pub(crate) region_drag_mode: ScreenshotRegionDragMode,
    pub(crate) region_grab_cursor: (i32, i32),
    pub(crate) region_grab_rect: Option<CaptureCrop>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScreenshotRegionDragMode {
    None,
    Move,
    Resize(ScreenshotRegionResizeDir),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScreenshotRegionResizeDir {
    pub(crate) left: bool,
    pub(crate) right: bool,
    pub(crate) top: bool,
    pub(crate) bottom: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingScreenshotCapture {
    pub(crate) monitor: String,
    pub(crate) serial: u64,
    pub(crate) crop: CaptureCrop,
    pub(crate) output_path: PathBuf,
    pub(crate) execute_at_ms: u64,
}

pub(crate) struct InflightScreenshotCapture {
    pub(crate) monitor: String,
    pub(crate) serial: u64,
    pub(crate) rx: Receiver<Result<PathBuf, String>>,
}

#[derive(Clone, Debug)]
pub(crate) struct ScreenshotCaptureResult {
    pub(crate) serial: u64,
    pub(crate) saved_path: Option<PathBuf>,
    pub(crate) error: Option<String>,
}
