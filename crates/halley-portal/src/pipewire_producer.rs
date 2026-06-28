use std::collections::HashMap;
use std::fs::File;
use std::io::Cursor;
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use eventline::{debug, error, info, warn};
use memmap2::Mmap;
use pipewire::context::ContextRc;
use pipewire::core::CoreRc;
use pipewire::loop_::Timeout;
use pipewire::main_loop::MainLoopRc;
use pipewire::properties::PropertiesBox;
use pipewire::spa;
use pipewire::spa::buffer::DataType;
use pipewire::spa::sys as spa_sys;
use pipewire::stream::{StreamFlags, StreamListener, StreamRc, StreamState};

use halley_api::protocol::{
    PortalDmabufPlane, SHM_CURSOR_BITMAP_BYTES, SHM_CURSOR_FIELDS, SHM_CURSOR_OFF_HEIGHT,
    SHM_CURSOR_OFF_HOTSPOT_X, SHM_CURSOR_OFF_HOTSPOT_Y, SHM_CURSOR_OFF_POS_X, SHM_CURSOR_OFF_POS_Y,
    SHM_CURSOR_OFF_SERIAL, SHM_CURSOR_OFF_STRIDE, SHM_CURSOR_OFF_VISIBLE, SHM_CURSOR_OFF_WIDTH,
    SHM_CURSOR_OFFSET, SHM_PIXELS_OFFSET,
};

/// Size of the per-buffer `SPA_META_Cursor` region we request: the cursor struct,
/// an inline bitmap struct, and room for a max-size BGRA cursor bitmap.
const CURSOR_META_SIZE: usize = std::mem::size_of::<spa_sys::spa_meta_cursor>()
    + std::mem::size_of::<spa_sys::spa_meta_bitmap>()
    + SHM_CURSOR_BITMAP_BYTES;
const DRM_FORMAT_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
const DRM_FORMAT_MOD_INVALID: u64 = u64::MAX;

pub enum PwCommand {
    CreateStream {
        session_id: String,
        width: u32,
        height: u32,
        shm_path: String,
        reply_tx: Sender<PwReply>,
    },
    DestroyStream {
        session_id: String,
    },
    Quit,
}

#[allow(dead_code)]
pub enum PwReply {
    StreamCreated {
        session_id: String,
        node_id: u32,
        pipewire_serial: Option<u64>,
    },
    Error {
        session_id: String,
        message: String,
    },
}

struct ActiveStream {
    stream: StreamRc,
    _listener: StreamListener<u64>,
    _mmap: Arc<Mmap>,
}

pub struct PipewireProducer {
    command_tx: Sender<PwCommand>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
    quit_flag: Arc<AtomicBool>,
}

impl PipewireProducer {
    pub fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel::<PwCommand>();
        let quit_flag = Arc::new(AtomicBool::new(false));
        let quit_for_thread = quit_flag.clone();

        let handle = std::thread::Builder::new()
            .name("halley-pipewire".to_string())
            .spawn(move || {
                pipewire_thread(command_rx, quit_for_thread);
            })
            .expect("failed to spawn pipewire thread");

        Self {
            command_tx,
            thread_handle: Some(handle),
            quit_flag,
        }
    }

    pub fn create_stream(
        &self,
        session_id: &str,
        width: u32,
        height: u32,
        shm_path: &str,
    ) -> Result<(u32, Option<u64>), String> {
        let (reply_tx, reply_rx) = mpsc::channel();

        self.command_tx
            .send(PwCommand::CreateStream {
                session_id: session_id.to_string(),
                width,
                height,
                shm_path: shm_path.to_string(),
                reply_tx,
            })
            .map_err(|_| "pipewire thread command channel closed".to_string())?;

        match reply_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(PwReply::StreamCreated {
                node_id,
                pipewire_serial,
                ..
            }) => Ok((node_id, pipewire_serial)),
            Ok(PwReply::Error { message, .. }) => Err(message),
            Err(e) => Err(format!("timeout waiting for pipewire stream: {e}")),
        }
    }

    pub fn destroy_stream(&self, session_id: &str) {
        let _ = self.command_tx.send(PwCommand::DestroyStream {
            session_id: session_id.to_string(),
        });
    }
}

impl Drop for PipewireProducer {
    fn drop(&mut self) {
        self.quit_flag.store(true, Ordering::Relaxed);
        let _ = self.command_tx.send(PwCommand::Quit);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

fn pipewire_thread(command_rx: Receiver<PwCommand>, quit_flag: Arc<AtomicBool>) {
    pipewire::init();
    info!("pipewire thread started");

    let mainloop = match MainLoopRc::new(None) {
        Ok(ml) => ml,
        Err(e) => {
            error!("failed to create pipewire main loop: {e}");
            return;
        }
    };

    let context = match ContextRc::new(&mainloop, None) {
        Ok(ctx) => ctx,
        Err(e) => {
            error!("failed to create pipewire context: {e}");
            return;
        }
    };

    let core = match context.connect_rc(None) {
        Ok(core) => core,
        Err(e) => {
            error!("failed to connect to pipewire: {e}");
            return;
        }
    };

    let mut streams: HashMap<String, ActiveStream> = HashMap::new();

    loop {
        // Process commands
        loop {
            match command_rx.try_recv() {
                Ok(PwCommand::CreateStream {
                    session_id,
                    width,
                    height,
                    shm_path,
                    reply_tx,
                }) => {
                    match create_pw_stream(&mainloop, &core, &session_id, width, height, &shm_path)
                    {
                        Ok((stream, listener, mmap, node_id, serial)) => {
                            info!(
                                "pipewire stream created for {}: node_id={}",
                                session_id, node_id
                            );
                            streams.insert(
                                session_id.clone(),
                                ActiveStream {
                                    stream,
                                    _listener: listener,
                                    _mmap: mmap,
                                },
                            );
                            let _ = reply_tx.send(PwReply::StreamCreated {
                                session_id,
                                node_id,
                                pipewire_serial: serial,
                            });
                        }
                        Err(e) => {
                            warn!("failed to create pipewire stream for {}: {e}", session_id);
                            let _ = reply_tx.send(PwReply::Error {
                                session_id,
                                message: e,
                            });
                        }
                    }
                }
                Ok(PwCommand::DestroyStream { session_id }) => {
                    if let Some(stream_info) = streams.remove(&session_id) {
                        let _ = stream_info.stream.disconnect();
                        debug!("pipewire stream destroyed for {}", session_id);
                    }
                }
                Ok(PwCommand::Quit) => {
                    info!("pipewire thread quitting");
                    for (_, s) in streams.drain() {
                        let _ = s.stream.disconnect();
                    }
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    info!("pipewire thread: command channel closed, quitting");
                    return;
                }
            }
        }

        if quit_flag.load(Ordering::Relaxed) {
            for (_, s) in streams.drain() {
                let _ = s.stream.disconnect();
            }
            return;
        }

        // Run the pipewire loop for a short time
        let _ = mainloop
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(16)));
    }
}

fn create_pw_stream(
    mainloop: &MainLoopRc,
    core: &CoreRc,
    session_id: &str,
    width: u32,
    height: u32,
    shm_path: &str,
) -> Result<(StreamRc, StreamListener<u64>, Arc<Mmap>, u32, Option<u64>), String> {
    let file = File::open(Path::new(shm_path))
        .map_err(|e| format!("failed to open shm file {shm_path}: {e}"))?;
    let mmap = unsafe { Mmap::map(&file) }.map_err(|e| format!("failed to mmap shm file: {e}"))?;

    if mmap.len() < SHM_PIXELS_OFFSET + 4 {
        return Err("shm file too small".into());
    }
    if &mmap[0..4] != b"HALS" {
        return Err("shm file has wrong magic".into());
    }

    let frame_size = (width as usize) * (height as usize) * 4;

    // Create stream properties
    let mut props = PropertiesBox::new();
    props.insert("media.class", "Video/Source");
    props.insert("media.name", format!("halley-screencast-{session_id}"));
    props.insert("media.role", "Screen");
    props.insert("node.name", "xdg-desktop-portal-halley");
    props.insert("node.pause-on-idle", "false");
    props.insert("stream.is-live", "true");

    // Create the stream
    let stream = StreamRc::new(
        core.clone(),
        &format!("halley-screencast-{session_id}"),
        props,
    )
    .map_err(|e| format!("failed to create stream: {e}"))?;

    // Add process listener - uses shared memory for frame data
    let mmap = Arc::new(mmap);
    let mmap_ref = mmap.clone();
    let session_id_for_process = session_id.to_string();

    let listener = stream
        .add_local_listener_with_user_data(0u64)
        .state_changed({
            let session_id_for_state = session_id.to_string();
            move |_stream, _user_data, _old, new| {
                info!(
                    "pipewire state changed: session={} state={:?}",
                    session_id_for_state, new
                );
                if matches!(new, StreamState::Streaming) {
                    let handle = session_id_for_state.clone();
                    std::thread::spawn(move || {
                        let _ =
                            crate::compositor_client::CompositorClient::set_active(&handle, true);
                    });
                }
            }
        })
        .add_buffer({
            let session_id_for_add = session_id.to_string();
            move |_stream, _frame_count, buffer| {
                let Some((buffer_id, planes, fds)) = dmabuf_buffer_info(buffer, width, height) else {
                    return;
                };
                if let Err(err) = crate::compositor_client::CompositorClient::add_dmabuf_buffer(
                    &session_id_for_add,
                    buffer_id,
                    width as i32,
                    height as i32,
                    DRM_FORMAT_XRGB8888,
                    DRM_FORMAT_MOD_INVALID,
                    0,
                    planes,
                    &fds,
                ) {
                    warn!(
                        "pipewire add_buffer: failed to register dmabuf buffer session={} buffer={} err={}",
                        session_id_for_add, buffer_id, err
                    );
                }
            }
        })
        .remove_buffer({
            let session_id_for_remove = session_id.to_string();
            move |_stream, _frame_count, buffer| {
                if let Some((buffer_id, _, _)) = dmabuf_buffer_info(buffer, width, height) {
                    let _ = crate::compositor_client::CompositorClient::remove_dmabuf_buffer(
                        &session_id_for_remove,
                        buffer_id,
                    );
                }
            }
        })
        .process(move |stream, frame_count| {
            let data_offset = SHM_PIXELS_OFFSET;
            let available = mmap_ref.len().saturating_sub(data_offset);
            let copy_len = available.min(frame_size);

            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };

            // Mirror the compositor's shm cursor block into the buffer's
            // SPA_META_Cursor (METADATA cursor mode). Consumers like OBS draw and
            // toggle this cursor client-side, so the toggle is live.
            fill_cursor_meta(&buffer, &mmap_ref);

            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }

            let data = &mut datas[0];
            if data.type_() == DataType::DmaBuf && data.fd() >= 0 {
                let buffer_id = data.fd() as u64;
                match crate::compositor_client::CompositorClient::render_dmabuf_buffer(
                    &session_id_for_process,
                    buffer_id,
                ) {
                    Ok(()) => {
                        let chunk = data.chunk_mut();
                        *chunk.offset_mut() = 0;
                        *chunk.size_mut() = frame_size as u32;
                        *chunk.stride_mut() = (width * 4) as i32;
                        *frame_count = frame_count.wrapping_add(1);
                    }
                    Err(err) => {
                        warn!(
                            "pipewire process: dmabuf render failed session={} buffer={} err={}",
                            session_id_for_process, buffer_id, err
                        );
                    }
                }
                return;
            }

            let copied = if let Some(dst) = data.data() {
                let len = copy_len.min(dst.len());
                dst[..len].copy_from_slice(&mmap_ref[data_offset..data_offset + len]);
                len
            } else {
                // No CPU pointer: a non-mappable (e.g. DMABUF) buffer slipped through
                // negotiation. We can't fill it via memcpy; warn instead of silently
                // shipping a black frame. The buffers POD restricts dataType to
                // MemFd/MemPtr so this should not happen.
                warn!(
                    "pipewire process: session={} dequeued buffer has no CPU pointer \
                     (non-mappable buffer type) - dropping frame",
                    session_id_for_process
                );
                0
            };

            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.size_mut() = copied as u32;
            *chunk.stride_mut() = (width * 4) as i32;

            *frame_count = frame_count.wrapping_add(1);
            if *frame_count == 1 || *frame_count % 300 == 0 {
                let seq = mmap_ref
                    .get(16..24)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(u64::from_le_bytes)
                    .unwrap_or(0);
                debug!(
                    "pipewire process: session={} frames={} copied={} shm_seq={}",
                    session_id_for_process, *frame_count, copied, seq
                );
            }
        })
        .register()
        .map_err(|e| format!("failed to register stream listener: {e}"))?;

    // Keep this format POD built through pipewire-rs' typed serializer. The
    // hand-built low-level POD looked valid at a glance, but PipeWire adapted
    // the source as an audio-ish port and OBS/Discord could not link it.
    let format_data = build_format_pod_bytes(width, height)?;
    let format_pod = spa::pod::Pod::from_bytes(&format_data)
        .ok_or_else(|| "failed to build PipeWire format pod".to_string())?;

    let buffers_data = build_buffers_pod_bytes(width, height)?;
    let buffers_pod = spa::pod::Pod::from_bytes(&buffers_data)
        .ok_or_else(|| "failed to build PipeWire buffers pod".to_string())?;

    // Request a per-buffer SPA_META_Cursor region so we can ship the cursor as
    // metadata (drawn/toggled client-side) instead of baking it into pixels.
    let meta_data = build_meta_pod_bytes();
    let meta_pod = spa::pod::Pod::from_bytes(&meta_data)
        .ok_or_else(|| "failed to build PipeWire meta pod".to_string())?;
    let mut params = [format_pod, buffers_pod, meta_pod];

    // Do not add ALLOC_BUFFERS or DRIVER here. ALLOC_BUFFERS made PipeWire hand
    // us invalid/zero-sized buffers, and DRIVER made WirePlumber link activation
    // unreliable. AUTOCONNECT + MAP_BUFFERS matches the working producer model:
    // PipeWire allocates mapped buffers and we memcpy frames into them.
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| format!("failed to connect stream: {e}"))?;

    stream
        .set_active(true)
        .map_err(|e| format!("failed to activate stream: {e}"))?;

    let node_id = wait_for_node_id(mainloop, &stream)?;

    let serial = stream
        .properties()
        .get("object.serial")
        .and_then(|s| s.parse::<u64>().ok());

    // format_data/buffers_data/meta_data must outlive the connect call - they do
    // since we're still in scope.
    drop(format_data);
    drop(buffers_data);
    drop(meta_data);

    Ok((stream, listener, mmap, node_id, serial))
}

fn dmabuf_buffer_info(
    buffer: *mut pipewire::sys::pw_buffer,
    width: u32,
    _height: u32,
) -> Option<(u64, Vec<PortalDmabufPlane>, Vec<RawFd>)> {
    if buffer.is_null() {
        return None;
    }
    let spa_buffer = unsafe { (*buffer).buffer };
    if spa_buffer.is_null() {
        return None;
    }

    let n_datas = unsafe { (*spa_buffer).n_datas as usize };
    let datas = unsafe { (*spa_buffer).datas };
    if n_datas == 0 || datas.is_null() {
        return None;
    }

    let mut planes = Vec::new();
    let mut fds = Vec::new();
    for idx in 0..n_datas {
        let data = unsafe { &*datas.add(idx) };
        if data.type_ != spa_sys::SPA_DATA_DmaBuf || data.fd < 0 {
            return None;
        }
        let stride = if !data.chunk.is_null() {
            let chunk_stride = unsafe { (*data.chunk).stride };
            if chunk_stride > 0 {
                chunk_stride as u32
            } else {
                width.saturating_mul(4)
            }
        } else {
            width.saturating_mul(4)
        };
        planes.push(PortalDmabufPlane {
            fd_index: fds.len() as u32,
            plane_index: idx as u32,
            offset: data.mapoffset,
            stride,
        });
        fds.push(data.fd as RawFd);
    }

    let buffer_id = fds.first().copied()? as u64;
    Some((buffer_id, planes, fds))
}

/// Mirror the compositor's shm cursor block into the buffer's `SPA_META_Cursor`.
/// When the cursor is not visible this frame, `id` is set to 0 (consumer draws
/// nothing); otherwise position/hotspot are set and the BGRA bitmap is written
/// inline so each buffer is self-contained.
fn fill_cursor_meta(buffer: &pipewire::buffer::Buffer<'_>, mmap: &Mmap) {
    let Some(meta) = buffer.find_meta::<spa::buffer::meta::MetaCursor>() else {
        return;
    };
    let cur = meta as *const spa::buffer::meta::MetaCursor as *mut spa_sys::spa_meta_cursor;
    let base = SHM_CURSOR_OFFSET;
    let read_u32 = |off: usize| -> u32 {
        mmap.get(base + off..base + off + 4)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(u32::from_le_bytes)
            .unwrap_or(0)
    };
    let read_i32 = |off: usize| -> i32 {
        mmap.get(base + off..base + off + 4)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(i32::from_le_bytes)
            .unwrap_or(0)
    };

    unsafe {
        if read_u32(SHM_CURSOR_OFF_VISIBLE) == 0 {
            (*cur).id = 0;
            (*cur).flags = 0;
            (*cur).bitmap_offset = 0;
            return;
        }

        let serial = mmap
            .get(base + SHM_CURSOR_OFF_SERIAL..base + SHM_CURSOR_OFF_SERIAL + 8)
            .and_then(|b| <[u8; 8]>::try_from(b).ok())
            .map(u64::from_le_bytes)
            .unwrap_or(1);
        (*cur).id = (serial as u32).max(1);
        (*cur).flags = 0;
        (*cur).position.x = read_i32(SHM_CURSOR_OFF_POS_X);
        (*cur).position.y = read_i32(SHM_CURSOR_OFF_POS_Y);
        (*cur).hotspot.x = read_i32(SHM_CURSOR_OFF_HOTSPOT_X);
        (*cur).hotspot.y = read_i32(SHM_CURSOR_OFF_HOTSPOT_Y);

        let w = read_u32(SHM_CURSOR_OFF_WIDTH);
        let h = read_u32(SHM_CURSOR_OFF_HEIGHT);
        let stride = read_u32(SHM_CURSOR_OFF_STRIDE);
        let bmp_bytes = (stride as usize).saturating_mul(h as usize);
        let cursor_sz = std::mem::size_of::<spa_sys::spa_meta_cursor>();
        let bitmap_sz = std::mem::size_of::<spa_sys::spa_meta_bitmap>();

        // Always ship the bitmap when visible so each rotated PipeWire buffer is
        // self-contained (avoids a missing cursor on buffers that didn't see the
        // last change).
        if w > 0 && h > 0 && bmp_bytes > 0 && cursor_sz + bitmap_sz + bmp_bytes <= CURSOR_META_SIZE
        {
            (*cur).bitmap_offset = cursor_sz as u32;
            let bmp = (cur as *mut u8).add(cursor_sz) as *mut spa_sys::spa_meta_bitmap;
            (*bmp).format = spa_sys::SPA_VIDEO_FORMAT_BGRA;
            (*bmp).size.width = w;
            (*bmp).size.height = h;
            (*bmp).stride = stride as i32;
            (*bmp).offset = bitmap_sz as u32;
            let dst = (bmp as *mut u8).add(bitmap_sz);
            let src_off = base + SHM_CURSOR_FIELDS;
            if let Some(src) = mmap.get(src_off..src_off + bmp_bytes) {
                std::ptr::copy_nonoverlapping(src.as_ptr(), dst, bmp_bytes);
            }
        } else {
            (*cur).bitmap_offset = 0;
        }
    }
}

fn build_meta_pod_bytes() -> Vec<u8> {
    use spa::pod::builder::Builder;

    let mut data = vec![0u8; 256];
    let mut builder = Builder::new(&mut data);
    let mut frame: MaybeUninit<spa_sys::spa_pod_frame> = MaybeUninit::uninit();
    unsafe {
        builder
            .push_object(
                &mut frame,
                spa_sys::SPA_TYPE_OBJECT_ParamMeta,
                spa_sys::SPA_PARAM_Meta,
            )
            .expect("push meta object failed");
    }

    builder
        .add_prop(spa_sys::SPA_PARAM_META_type, 0)
        .expect("add meta type failed");
    builder
        .add_id(spa::utils::Id(spa_sys::SPA_META_Cursor))
        .expect("add meta type value failed");

    builder
        .add_prop(spa_sys::SPA_PARAM_META_size, 0)
        .expect("add meta size failed");
    builder
        .add_int(CURSOR_META_SIZE as i32)
        .expect("add meta size value failed");

    unsafe {
        builder.pop(&mut frame.assume_init());
    }

    data
}

fn wait_for_node_id(mainloop: &MainLoopRc, stream: &StreamRc) -> Result<u32, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let node_id = stream.node_id();
        if node_id != pipewire::constants::ID_ANY {
            return Ok(node_id);
        }
        if std::time::Instant::now() >= deadline {
            return Err("PipeWire did not assign a node id".into());
        }
        let _ = mainloop
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(16)));
    }
}

fn build_format_pod_bytes(width: u32, height: u32) -> Result<Vec<u8>, String> {
    // The compositor-side DMA-BUF render path currently renders whole-output
    // buffers, so keep the PipeWire source size fixed to the compositor stream
    // size. Negotiated downscale needs a scaled render target before it is safe.
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Id,
            spa::param::video::VideoFormat::BGRx
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Rectangle,
            spa::utils::Rectangle { width, height }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 60, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 360, denom: 1 }
        ),
    );

    spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map(|success| success.0.into_inner())
    .map_err(|e| format!("failed to serialize PipeWire format pod: {e}"))
}

fn build_buffers_pod_bytes(width: u32, height: u32) -> Result<Vec<u8>, String> {
    use spa::pod::Property;
    use spa::utils::SpaTypes;

    let stride = (width * 4) as i32;
    let size = stride * height as i32;

    let obj = spa::pod::object!(
        SpaTypes::ObjectParamBuffers,
        spa::param::ParamType::Buffers,
        Property::new(spa_sys::SPA_PARAM_BUFFERS_blocks, spa::pod::Value::Int(1)),
        Property::new(spa_sys::SPA_PARAM_BUFFERS_size, spa::pod::Value::Int(size)),
        Property::new(
            spa_sys::SPA_PARAM_BUFFERS_stride,
            spa::pod::Value::Int(stride)
        ),
        Property::new(spa_sys::SPA_PARAM_BUFFERS_align, spa::pod::Value::Int(16)),
    );

    spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map(|success| success.0.into_inner())
    .map_err(|e| format!("failed to serialize PipeWire buffers pod: {e}"))
}
