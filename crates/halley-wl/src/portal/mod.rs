use std::collections::HashMap;
use std::fs::File;
use std::fs::OpenOptions;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eventline::{debug, info, warn};
use halley_core::field::NodeId;
use memmap2::MmapMut;
use smithay::backend::allocator::{
    Fourcc, Modifier,
    dmabuf::{Dmabuf, DmabufFlags},
};

use halley_api::protocol::{
    PortalDmabufPlane, SHM_CURSOR_FIELDS, SHM_CURSOR_MAX_H, SHM_CURSOR_MAX_W,
    SHM_CURSOR_OFF_HEIGHT, SHM_CURSOR_OFF_HOTSPOT_X, SHM_CURSOR_OFF_HOTSPOT_Y,
    SHM_CURSOR_OFF_POS_X, SHM_CURSOR_OFF_POS_Y, SHM_CURSOR_OFF_SERIAL, SHM_CURSOR_OFF_STRIDE,
    SHM_CURSOR_OFF_VISIBLE, SHM_CURSOR_OFF_WIDTH, SHM_CURSOR_OFFSET, SHM_PIXELS_OFFSET,
};

use crate::bootstrap::halley_runtime_dir;

const SHM_MAGIC: [u8; 4] = *b"HALS";

/// How the cursor should appear in a screencast stream, mirroring the portal
/// cursor modes. `Metadata` ships the cursor as PipeWire `SPA_META_Cursor`
/// (drawn/toggled client-side); `Embedded` bakes it into the frame pixels;
/// `Hidden` shows no cursor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ScreencastCursorMode {
    Hidden,
    Embedded,
    Metadata,
}

impl ScreencastCursorMode {
    pub(crate) fn from_portal_mode(mode: u32) -> Self {
        if mode == halley_api::PORTAL_CURSOR_MODE_METADATA {
            Self::Metadata
        } else if mode == halley_api::PORTAL_CURSOR_MODE_EMBEDDED {
            Self::Embedded
        } else {
            Self::Hidden
        }
    }

    /// Whether the cursor pixels should be composited into the captured frame.
    fn embeds_in_frame(self) -> bool {
        matches!(self, Self::Embedded)
    }
}

/// Resolved cursor state for a single captured frame, in stream-pixel coords.
pub(crate) struct CursorMetaFrame {
    pub(crate) pos_x: i32,
    pub(crate) pos_y: i32,
    pub(crate) hotspot_x: i32,
    pub(crate) hotspot_y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// BGRA bitmap, `width * height * 4` bytes.
    pub(crate) bgra: Vec<u8>,
}

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
    cursor_mode: ScreencastCursorMode,
    width: i32,
    height: i32,
    stride: i32,
    shm_path: PathBuf,
    _file: File,
    mmap: MmapMut,
    sequence: u64,
    /// Whether the PipeWire consumer is actively pulling frames. When false,
    /// the compositor skips fresh capture to avoid wasted GPU/CPU work.
    active: bool,
    dmabuf_buffers: HashMap<u64, Dmabuf>,
    /// Current cursor-bitmap version written into the shm cursor block. Bumped
    /// whenever the bitmap content changes so the producer re-ships it.
    cursor_serial: u64,
    /// Signature of the last bitmap written, to detect bitmap changes.
    last_cursor_sig: Option<u64>,
}

impl ScreencastSession {
    fn create(
        session_id: &str,
        label: &str,
        width: i32,
        height: i32,
        cursor_mode: ScreencastCursorMode,
    ) -> std::io::Result<Self> {
        let width = width.max(1);
        let height = height.max(1);
        let stride = width.saturating_mul(4);
        let data_size = (stride as usize) * (height as usize);
        let total_size = SHM_PIXELS_OFFSET + data_size;

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
        // Cursor block starts zero-initialised (visible = 0, serial = 0).

        mmap.flush()?;

        Ok(Self {
            target: ScreencastTarget::Output {
                name: String::new(),
            },
            cursor_mode,
            width,
            height,
            stride,
            shm_path,
            _file: file,
            mmap,
            sequence: 0,
            active: true,
            dmabuf_buffers: HashMap::new(),
            cursor_serial: 0,
            last_cursor_sig: None,
        })
    }

    pub fn shm_path(&self) -> &std::path::Path {
        &self.shm_path
    }

    pub fn dimensions(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    pub fn cursor_mode(&self) -> ScreencastCursorMode {
        self.cursor_mode
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

        let data_offset = SHM_PIXELS_OFFSET;
        let data_end = data_offset + expected;

        self.mmap[data_offset..data_end].copy_from_slice(&frame_data[..expected]);

        self.sequence = self.sequence.wrapping_add(1);
        let seq = self.sequence;
        self.mmap[16..24].copy_from_slice(&seq.to_le_bytes());
    }

    /// Update the shm cursor block for METADATA cursor mode. `cursor = None`
    /// marks the cursor absent this frame (`visible = 0`); the producer then
    /// emits no cursor metadata.
    pub fn write_cursor_meta(&mut self, cursor: Option<&CursorMetaFrame>) {
        let base = SHM_CURSOR_OFFSET;
        let Some(cursor) = cursor else {
            // visible = 0; leave the rest as-is.
            self.mmap[base + SHM_CURSOR_OFF_VISIBLE..base + SHM_CURSOR_OFF_VISIBLE + 4]
                .copy_from_slice(&0u32.to_le_bytes());
            return;
        };

        // Clamp bitmap to the reserved region; drop it if it somehow exceeds it.
        let w = cursor.width as usize;
        let h = cursor.height as usize;
        let bytes = w.saturating_mul(h).saturating_mul(4);
        let fits = w <= SHM_CURSOR_MAX_W
            && h <= SHM_CURSOR_MAX_H
            && bytes > 0
            && cursor.bgra.len() >= bytes;

        // Bump the serial only when the bitmap content changes, so the producer
        // can skip re-shipping an unchanged bitmap.
        if fits {
            let sig = cursor_bitmap_signature(cursor.width, cursor.height, &cursor.bgra[..bytes]);
            if self.last_cursor_sig != Some(sig) {
                self.last_cursor_sig = Some(sig);
                self.cursor_serial = self.cursor_serial.wrapping_add(1).max(1);
            }
        }

        let serial = self.cursor_serial.max(1);
        let write_u32 = |mmap: &mut MmapMut, off: usize, v: u32| {
            mmap[base + off..base + off + 4].copy_from_slice(&v.to_le_bytes());
        };
        let write_i32 = |mmap: &mut MmapMut, off: usize, v: i32| {
            mmap[base + off..base + off + 4].copy_from_slice(&v.to_le_bytes());
        };

        self.mmap[base + SHM_CURSOR_OFF_SERIAL..base + SHM_CURSOR_OFF_SERIAL + 8]
            .copy_from_slice(&serial.to_le_bytes());
        write_u32(&mut self.mmap, SHM_CURSOR_OFF_VISIBLE, 1);
        write_i32(&mut self.mmap, SHM_CURSOR_OFF_POS_X, cursor.pos_x);
        write_i32(&mut self.mmap, SHM_CURSOR_OFF_POS_Y, cursor.pos_y);
        write_i32(&mut self.mmap, SHM_CURSOR_OFF_HOTSPOT_X, cursor.hotspot_x);
        write_i32(&mut self.mmap, SHM_CURSOR_OFF_HOTSPOT_Y, cursor.hotspot_y);

        if fits {
            write_u32(&mut self.mmap, SHM_CURSOR_OFF_WIDTH, cursor.width);
            write_u32(&mut self.mmap, SHM_CURSOR_OFF_HEIGHT, cursor.height);
            write_u32(&mut self.mmap, SHM_CURSOR_OFF_STRIDE, cursor.width * 4);
            let bmp = base + SHM_CURSOR_FIELDS;
            self.mmap[bmp..bmp + bytes].copy_from_slice(&cursor.bgra[..bytes]);
        } else {
            // No usable bitmap: keep position updates but advertise no bitmap.
            write_u32(&mut self.mmap, SHM_CURSOR_OFF_WIDTH, 0);
            write_u32(&mut self.mmap, SHM_CURSOR_OFF_HEIGHT, 0);
            write_u32(&mut self.mmap, SHM_CURSOR_OFF_STRIDE, 0);
        }
    }
}

fn cursor_bitmap_signature(width: u32, height: u32, bgra: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    bgra.hash(&mut hasher);
    hasher.finish()
}

impl Drop for ScreencastSession {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.shm_path);
    }
}

/// Screencast capture is throttled to this rate per output rather than running
/// once per vblank: each capture is a full off-screen re-render plus a blocking
/// GPU→CPU readback, so on a high-refresh monitor capturing every vblank stalls
/// the compositor. The PipeWire producer is pull-based and only ships the latest
/// frame on demand, so faster capture is wasted work.
const TARGET_CAPTURE_FPS: u32 = 60;

#[derive(Default)]
pub(crate) struct ScreencastState {
    sessions: HashMap<String, ScreencastSession>,
    last_capture: HashMap<String, Instant>,
}

impl ScreencastState {
    pub fn start_output(
        &mut self,
        session_handle: &str,
        output_name: &str,
        width: i32,
        height: i32,
        cursor_mode: ScreencastCursorMode,
    ) -> std::io::Result<PathBuf> {
        let mut session = ScreencastSession::create(
            &short_id(session_handle),
            "output",
            width,
            height,
            cursor_mode,
        )?;
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
        cursor_mode: ScreencastCursorMode,
    ) -> std::io::Result<PathBuf> {
        let mut session = ScreencastSession::create(
            &short_id(session_handle),
            "window",
            width,
            height,
            cursor_mode,
        )?;
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

    /// Set the active state for a session. When inactive, the compositor skips
    /// fresh captures because the PipeWire consumer is not pulling frames.
    pub fn set_active(&mut self, session_handle: &str, active: bool) {
        if let Some(session) = self.sessions.get_mut(session_handle) {
            session.active = active;
            debug!(
                "screencast: session {} {}",
                session_handle,
                if active { "activated" } else { "paused" }
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_dmabuf_buffer(
        &mut self,
        session_handle: &str,
        buffer_id: u64,
        fds: Vec<OwnedFd>,
        width: i32,
        height: i32,
        format: u32,
        modifier: u64,
        flags: u32,
        planes: Vec<PortalDmabufPlane>,
    ) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_handle)
            .ok_or_else(|| format!("unknown screencast session {session_handle}"))?;
        if width <= 0 || height <= 0 {
            return Err(format!("invalid dmabuf size {width}x{height}"));
        }
        if planes.is_empty() {
            return Err("dmabuf buffer has no planes".to_string());
        }

        let fourcc = Fourcc::try_from(format)
            .map_err(|_| format!("unsupported dmabuf fourcc 0x{format:08x}"))?;
        let mut fd_slots = fds.into_iter().map(Some).collect::<Vec<_>>();
        let mut builder = Dmabuf::builder(
            (width, height),
            fourcc,
            Modifier::from(modifier),
            DmabufFlags::from_bits_retain(flags),
        );
        for plane in planes {
            let fd = fd_slots
                .get_mut(plane.fd_index as usize)
                .and_then(Option::take)
                .ok_or_else(|| format!("missing dmabuf fd index {}", plane.fd_index))?;
            if !builder.add_plane(fd, plane.plane_index, plane.offset, plane.stride) {
                return Err("too many dmabuf planes".to_string());
            }
        }
        let dmabuf = builder
            .build()
            .ok_or_else(|| "failed to build dmabuf".to_string())?;
        session.dmabuf_buffers.insert(buffer_id, dmabuf);
        Ok(())
    }

    pub fn remove_dmabuf_buffer(&mut self, session_handle: &str, buffer_id: u64) {
        if let Some(session) = self.sessions.get_mut(session_handle) {
            session.dmabuf_buffers.remove(&buffer_id);
        }
    }

    /// True if any **active** session targets this output. Used to skip capture
    /// when all consumers are paused.
    pub fn has_active_session_for_output(&self, output_name: &str) -> bool {
        self.sessions.values().any(|session| {
            session.active
                && match &session.target {
                    ScreencastTarget::Output { name } => name == output_name,
                    ScreencastTarget::Window { monitor, .. } => monitor == output_name,
                }
        })
    }
}

pub(crate) fn render_screencast_dmabuf_buffer(
    st: &mut crate::compositor::root::Halley,
    session_handle: &str,
    buffer_id: u64,
) -> Result<(), String> {
    let (output_name, overlay_cursor, mut dmabuf) = {
        let session = st
            .screencast
            .sessions
            .get_mut(session_handle)
            .ok_or_else(|| format!("unknown screencast session {session_handle}"))?;
        let output_name = match &session.target {
            ScreencastTarget::Output { name } => name.clone(),
            ScreencastTarget::Window { .. } => {
                return Err("dma-buf screencast currently supports monitor sessions only".into());
            }
        };
        let overlay_cursor = session.cursor_mode().embeds_in_frame();
        let dmabuf = session
            .dmabuf_buffers
            .remove(&buffer_id)
            .ok_or_else(|| format!("unknown dmabuf buffer {buffer_id}"))?;
        (output_name, overlay_cursor, dmabuf)
    };

    let output = st
        .model
        .monitor_state
        .outputs
        .get(&output_name)
        .cloned()
        .ok_or_else(|| format!("unknown output {output_name}"))?;
    let result = crate::protocol::wayland::portal::capture_output_dmabuf(
        st,
        &output,
        overlay_cursor,
        None,
        &mut dmabuf,
    )
    .map(|_| ())
    .map_err(|err| err.to_string());

    if let Some(session) = st.screencast.sessions.get_mut(session_handle) {
        session.dmabuf_buffers.insert(buffer_id, dmabuf);
    }

    result
}

/// Called from the compositor thread on vblank/present for a given output.
/// Captures frames for any screencast sessions targeting that output (either
/// whole-output sessions or window sessions hosted on it) and writes them into
/// the corresponding shared-memory files.
pub(crate) fn capture_screencast_for_output(
    st: &mut crate::compositor::root::Halley,
    output_name: &str,
) {
    if !st.screencast.has_active_session_for_output(output_name) {
        return;
    }

    // Throttle to the target rate: skip this vblank if we captured too recently.
    let now = Instant::now();
    let min_interval = Duration::from_micros(1_000_000 / TARGET_CAPTURE_FPS as u64);
    if let Some(last) = st.screencast.last_capture.get(output_name)
        && now.duration_since(*last) < min_interval
    {
        return;
    }
    st.screencast
        .last_capture
        .insert(output_name.to_string(), now);

    let Some(output) = st.model.monitor_state.outputs.get(output_name).cloned() else {
        return;
    };

    // Window sessions on this output: resolve their live screen rects first.
    // Each entry is (session_handle, local x, y, w, h) in output pixel coords.
    let mut window_crops: Vec<(String, i32, i32, i32, i32)> = Vec::new();
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

    // Only `Embedded` sessions need the cursor baked into the pixels; `Metadata`
    // and `Hidden` use a plain (cursorless) frame. Capture at most one frame per
    // distinct preference — the common case is a single session, one capture.
    let session_on_output = |session: &ScreencastSession| match &session.target {
        ScreencastTarget::Output { name } => name == output_name,
        ScreencastTarget::Window { monitor, .. } => monitor == output_name,
    };
    let mut need_embedded = false;
    let mut need_plain = false;
    for session in st.screencast.sessions.values() {
        if !session_on_output(session) {
            continue;
        }
        if session.cursor_mode().embeds_in_frame() {
            need_embedded = true;
        } else {
            need_plain = true;
        }
    }

    let embedded_frame = if need_embedded {
        crate::protocol::wayland::portal::capture_output_shm(st, &output, true, None).ok()
    } else {
        None
    };
    let plain_frame = if need_plain {
        match crate::protocol::wayland::portal::capture_output_shm(st, &output, false, None) {
            Ok(frame) => Some(frame),
            Err(e) => {
                debug!("screencast: capture failed for {}: {}", output_name, e);
                None
            }
        }
    } else {
        None
    };

    // Whole-output sessions get the full frame matching their cursor mode, plus a
    // cursor-metadata update for `Metadata` sessions.
    for handle in st.screencast.sessions.keys().cloned().collect::<Vec<_>>() {
        let (mode, dims) = match st.screencast.sessions.get(&handle) {
            Some(session) => match &session.target {
                ScreencastTarget::Output { name } if name == output_name => {
                    (session.cursor_mode(), session.dimensions())
                }
                _ => continue,
            },
            None => continue,
        };
        let frame_ref = if mode.embeds_in_frame() {
            embedded_frame.as_ref()
        } else {
            plain_frame.as_ref()
        };
        let cursor_meta = (mode == ScreencastCursorMode::Metadata)
            .then(|| resolve_cursor_meta_for_region(st, output_name, 0, 0, dims.0, dims.1))
            .flatten();
        let Some(session) = st.screencast.sessions.get_mut(&handle) else {
            continue;
        };
        if let Some(frame) = frame_ref {
            session.write_frame(&frame.bytes);
        }
        if mode == ScreencastCursorMode::Metadata {
            session.write_cursor_meta(cursor_meta.as_ref());
        }
    }

    // Window sessions get a cropped frame computed from the captured output bytes.
    for (handle, x, y, w, h) in window_crops {
        let mode = match st.screencast.sessions.get(&handle) {
            Some(session) => session.cursor_mode(),
            None => continue,
        };
        let frame = if mode.embeds_in_frame() {
            embedded_frame.as_ref()
        } else {
            plain_frame.as_ref()
        };
        let Some(frame) = frame else {
            continue;
        };
        let frame_w = frame.spec.width.max(1);
        let frame_stride = frame.spec.stride.max(1);
        let cropped = crop_frame(&frame.bytes, frame_stride, frame_w, x, y, w, h);
        let cursor_meta = (mode == ScreencastCursorMode::Metadata)
            .then(|| resolve_cursor_meta_for_region(st, output_name, x, y, w, h))
            .flatten();
        if let Some(session) = st.screencast.sessions.get_mut(&handle) {
            session.write_frame(&cropped);
            if mode == ScreencastCursorMode::Metadata {
                session.write_cursor_meta(cursor_meta.as_ref());
            }
        }
    }
}

/// Resolve the cursor for METADATA mode within a captured region (output-local
/// pixel coords). Returns `None` when the cursor is hidden or outside the region,
/// which the caller records as "no cursor this frame".
fn resolve_cursor_meta_for_region(
    st: &crate::compositor::root::Halley,
    output_name: &str,
    region_x: i32,
    region_y: i32,
    region_w: i32,
    region_h: i32,
) -> Option<CursorMetaFrame> {
    use smithay::input::pointer::{CursorIcon, CursorImageStatus};

    let status = crate::compositor::platform::effective_cursor_image_status(st);
    if matches!(status, CursorImageStatus::Hidden) {
        return None;
    }
    let (sx, sy) = st.input.interaction_state.last_pointer_screen_global?;
    if st.monitor_for_screen(sx, sy).as_deref() != Some(output_name) {
        return None;
    }
    let (_, _, local_sx, local_sy) = st.local_screen_in_monitor(output_name, sx, sy);
    let pos_x = local_sx.round() as i32 - region_x;
    let pos_y = local_sy.round() as i32 - region_y;
    if pos_x < 0 || pos_y < 0 || pos_x >= region_w || pos_y >= region_h {
        return None;
    }

    // Named cursors (cursor-shape / themed) map to a themed sprite directly. For
    // client surface cursors we fall back to the themed default arrow so the
    // pointer stays visible at the correct position (its exact shape isn't
    // reproduced in metadata mode).
    let icon = match status {
        CursorImageStatus::Named(icon) => icon,
        _ => CursorIcon::Default,
    };
    let cursor_config = &st.runtime.tuning.cursor;
    let sprite =
        crate::render::themed_cursor_sprite_with_fallback(cursor_config, icon).or_else(|| {
            crate::render::themed_cursor_sprite_with_fallback(cursor_config, CursorIcon::Default)
        })?;

    Some(CursorMetaFrame {
        pos_x,
        pos_y,
        hotspot_x: sprite.hotspot_x,
        hotspot_y: sprite.hotspot_y,
        width: sprite.width as u32,
        height: sprite.height as u32,
        bgra: sprite.pixels_bgra.clone(),
    })
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
