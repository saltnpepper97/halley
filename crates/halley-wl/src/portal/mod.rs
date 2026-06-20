use std::collections::HashMap;
use std::fs::File;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::time::Instant;

use eventline::{debug, info, warn};
use halley_core::field::NodeId;
use memmap2::MmapMut;

use crate::bootstrap::halley_runtime_dir;

const SHM_MAGIC: [u8; 4] = *b"HALS";
const SHM_HEADER_SIZE: usize = 32;

/// What a screencast session streams.
pub(crate) enum ScreencastTarget {
    /// Whole-output monitor capture.
    Output { name: String },
    /// A single window (node). Each frame the window's live screen rect is
    /// recomputed and cropped out of its host output's captured frame, so the
    /// stream follows the window as it moves or resizes.
    Window { node_id: NodeId, monitor: String },
}

pub(crate) struct ScreencastSession {
    pub(crate) target: ScreencastTarget,
    width: i32,
    height: i32,
    stride: i32,
    shm_path: PathBuf,
    _file: File,
    mmap: MmapMut,
    sequence: u64,
}

impl ScreencastSession {
    fn create(session_id: &str, label: &str, width: i32, height: i32) -> std::io::Result<Self> {
        let width = width.max(1);
        let height = height.max(1);
        let stride = width.saturating_mul(4);
        let data_size = (stride as usize) * (height as usize);
        let total_size = SHM_HEADER_SIZE + data_size;

        let runtime_dir = halley_runtime_dir()?;
        let shm_path = runtime_dir.join(format!("screencast_{session_id}_{label}.shm"));

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&shm_path)?;
        file.set_len(total_size as u64)?;
        let mut mmap = unsafe { MmapMut::map_mut(&file) }?;

        // Write header
        mmap[0..4].copy_from_slice(&SHM_MAGIC);
        mmap[4..8].copy_from_slice(&width.to_le_bytes());
        mmap[8..12].copy_from_slice(&height.to_le_bytes());
        mmap[12..16].copy_from_slice(&stride.to_le_bytes());
        mmap[16..24].copy_from_slice(&0u64.to_le_bytes()); // sequence = 0
        // bytes 24-31 reserved

        mmap.flush()?;

        Ok(Self {
            target: ScreencastTarget::Output {
                name: String::new(),
            },
            width,
            height,
            stride,
            shm_path,
            _file: file,
            mmap,
            sequence: 0,
        })
    }

    pub fn shm_path(&self) -> &std::path::Path {
        &self.shm_path
    }

    pub fn dimensions(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// Write a captured frame into shared memory. The caller is responsible for
    /// ensuring `frame_data` has at least `stride * height` bytes.
    pub fn write_frame(&mut self, frame_data: &[u8]) {
        let expected = (self.stride as usize) * (self.height as usize);
        if frame_data.len() < expected {
            warn!(
                "screencast frame too small: {} bytes, expected {}",
                frame_data.len(),
                expected
            );
            return;
        }

        let data_offset = SHM_HEADER_SIZE;
        let data_end = data_offset + expected;

        self.mmap[data_offset..data_end].copy_from_slice(&frame_data[..expected]);

        self.sequence = self.sequence.wrapping_add(1);
        let seq = self.sequence;
        self.mmap[16..24].copy_from_slice(&seq.to_le_bytes());

        let _ = self.mmap.flush();
    }
}

impl Drop for ScreencastSession {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.shm_path);
    }
}

#[derive(Default)]
pub(crate) struct ScreencastState {
    sessions: HashMap<String, ScreencastSession>,
}

impl ScreencastState {
    pub fn start_output(
        &mut self,
        session_handle: &str,
        output_name: &str,
        width: i32,
        height: i32,
    ) -> std::io::Result<PathBuf> {
        let mut session =
            ScreencastSession::create(&short_id(session_handle), "output", width, height)?;
        session.target = ScreencastTarget::Output {
            name: output_name.to_string(),
        };
        let path = session.shm_path().to_path_buf();
        info!(
            "screencast: started output session for {} ({}x{}) shm={}",
            output_name,
            width,
            height,
            path.display()
        );
        self.sessions.insert(session_handle.to_string(), session);
        Ok(path)
    }

    pub fn start_window(
        &mut self,
        session_handle: &str,
        node_id: NodeId,
        monitor: &str,
        width: i32,
        height: i32,
    ) -> std::io::Result<PathBuf> {
        let mut session =
            ScreencastSession::create(&short_id(session_handle), "window", width, height)?;
        session.target = ScreencastTarget::Window {
            node_id,
            monitor: monitor.to_string(),
        };
        let path = session.shm_path().to_path_buf();
        info!(
            "screencast: started window session for node {} on {} ({}x{}) shm={}",
            node_id.as_u64(),
            monitor,
            width,
            height,
            path.display()
        );
        self.sessions.insert(session_handle.to_string(), session);
        Ok(path)
    }

    pub fn stop(&mut self, session_handle: &str) {
        if let Some(session) = self.sessions.remove(session_handle) {
            match &session.target {
                ScreencastTarget::Output { name } => {
                    debug!("screencast: stopped output session for {}", name);
                }
                ScreencastTarget::Window { node_id, monitor } => {
                    debug!(
                        "screencast: stopped window session for node {} on {}",
                        node_id.as_u64(),
                        monitor
                    );
                }
            }
        }
    }

    /// True if any session streams the given output (either directly, or as the
    /// host of a window target).
    pub fn has_session_for_output(&self, output_name: &str) -> bool {
        self.sessions.values().any(|session| match &session.target {
            ScreencastTarget::Output { name } => name == output_name,
            ScreencastTarget::Window { monitor, .. } => monitor == output_name,
        })
    }
}

/// Called from the compositor thread on vblank/present for a given output.
/// Captures frames for any screencast sessions targeting that output (either
/// whole-output sessions or window sessions hosted on it) and writes them into
/// the corresponding shared-memory files.
pub(crate) fn capture_screencast_for_output(
    st: &mut crate::compositor::root::Halley,
    output_name: &str,
) {
    if !st.screencast.has_session_for_output(output_name) {
        return;
    }

    let Some(output) = st.model.monitor_state.outputs.get(output_name).cloned() else {
        return;
    };

    // Window sessions on this output: resolve their live screen rects first.
    // Each entry is (session_handle, local x, y, w, h) in output pixel coords.
    let mut window_crops: Vec<(String, i32, i32, i32, i32)> = Vec::new();
    let now = Instant::now();
    for handle in st.screencast.sessions.keys().cloned().collect::<Vec<_>>() {
        let Some(session) = st.screencast.sessions.get(&handle) else {
            continue;
        };
        let ScreencastTarget::Window { node_id, monitor } = &session.target else {
            continue;
        };
        if monitor != output_name {
            continue;
        }
        let (sw, sh) = session.dimensions();
        let Some(rect) = window_local_screen_rect(st, *node_id, output_name, sw, sh, now) else {
            continue;
        };
        window_crops.push((handle, rect.0, rect.1, rect.2, rect.3));
    }

    match crate::protocol::wayland::portal::capture_output_shm(st, &output, false, None) {
        Ok(frame) => {
            let frame_w = frame.spec.width.max(1);
            let frame_stride = frame.spec.stride.max(1);
            // Whole-output sessions get the full frame.
            for session in st.screencast.sessions.values_mut() {
                if let ScreencastTarget::Output { name } = &session.target {
                    if name == output_name {
                        session.write_frame(&frame.bytes);
                    }
                }
            }
            // Window sessions get a cropped frame computed from the captured
            // output bytes. Crop rects were resolved above before re-borrowing.
            for (handle, x, y, w, h) in window_crops {
                let Some(session) = st.screencast.sessions.get_mut(&handle) else {
                    continue;
                };
                let cropped = crop_frame(&frame.bytes, frame_stride, frame_w, x, y, w, h);
                session.write_frame(&cropped);
            }
        }
        Err(e) => {
            debug!("screencast: capture failed for {}: {}", output_name, e);
        }
    }
}

/// Crop a captured output frame (XRGB8888, row-major, stride = width*4) to the
/// given local pixel rect, producing a buffer sized to (w*4) rows of h. Output
/// is clamped to the source bounds; missing areas are left as the source bytes
/// (the session shm is zero-initialised, so out-of-bounds rows stay black).
fn crop_frame(src: &[u8], src_stride: i32, src_w: i32, x: i32, y: i32, w: i32, h: i32) -> Vec<u8> {
    let w = w.max(1);
    let h = h.max(1);
    let dst_stride = w.saturating_mul(4);
    let mut out = vec![0u8; (dst_stride as usize) * (h as usize)];
    let src_h = (src.len() as i32) / src_stride.max(1);
    for row in 0..h {
        let sy = y + row;
        if sy < 0 || sy >= src_h {
            continue;
        }
        let copy_w = w.min(src_w - x.max(0));
        if copy_w <= 0 {
            continue;
        }
        let src_start = (sy as usize) * (src_stride as usize) + (x.max(0) as usize) * 4;
        let copy_bytes = (copy_w as usize) * 4;
        let dst_start = (row as usize) * (dst_stride as usize);
        let src_end = src_start + copy_bytes;
        if src_end <= src.len() && dst_start + copy_bytes <= out.len() {
            out[dst_start..dst_start + copy_bytes].copy_from_slice(&src[src_start..src_end]);
        }
    }
    out
}

/// Resolve a window's current screen rect in output-local pixel coordinates,
/// clamped to the session's shm size. Returns (x, y, w, h).
fn window_local_screen_rect(
    st: &mut crate::compositor::root::Halley,
    node_id: NodeId,
    monitor: &str,
    session_w: i32,
    session_h: i32,
    now: Instant,
) -> Option<(i32, i32, i32, i32)> {
    use crate::input::active_node_screen_rect;

    let space = st.model.monitor_state.monitors.get(monitor)?;
    let (mut left, mut top, mut right, mut bottom) =
        active_node_screen_rect(st, space.width, space.height, node_id, now, None)?;
    // Inflate by the active frame pad like the screenshot path so the border is
    // included in the stream.
    let pad = crate::window::active_window_frame_pad_px(&st.runtime.tuning) as f32;
    left -= pad;
    top -= pad;
    right += pad;
    bottom += pad;
    let x = left.round() as i32;
    let y = top.round() as i32;
    let w = (right - left).round().max(1.0) as i32;
    let h = (bottom - top).round().max(1.0) as i32;
    // The shm is fixed at start time; clamp the captured rect to it so the
    // stream keeps a stable size. Resizes larger than the initial capture are
    // cropped; the portal can restart the stream to renegotiate.
    let cw = w.min(session_w);
    let ch = h.min(session_h);
    Some((x, y, cw, ch))
}

fn short_id(session_handle: &str) -> String {
    session_handle
        .rsplit('/')
        .next()
        .unwrap_or("session")
        .to_string()
}
