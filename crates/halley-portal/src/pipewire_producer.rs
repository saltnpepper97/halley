use std::collections::HashMap;
use std::fs::File;
use std::io::Cursor;
use std::mem::MaybeUninit;
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
use pipewire::spa::sys as spa_sys;
use pipewire::stream::{StreamFlags, StreamListener, StreamRc};

const SHM_HEADER_SIZE: usize = 32;

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

    if mmap.len() < SHM_HEADER_SIZE + 4 {
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
        .process(move |stream, frame_count| {
            let data_offset = SHM_HEADER_SIZE;
            let available = mmap_ref.len().saturating_sub(data_offset);
            let copy_len = available.min(frame_size);

            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }

            let data = &mut datas[0];
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

    let buffers_data = build_buffers_pod_bytes(width, height);
    let buffers_pod = spa::pod::Pod::from_bytes(&buffers_data)
        .ok_or_else(|| "failed to build PipeWire buffers pod".to_string())?;
    let mut params = [format_pod, buffers_pod];

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

    // format_data/buffers_data must outlive the connect call - they do since
    // we're still in scope.
    drop(format_data);
    drop(buffers_data);

    Ok((stream, listener, mmap, node_id, serial))
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
    // OBS accepts BGRx and may negotiate a smaller size than the compositor
    // output. The copy path clamps to the destination buffer length, so advertise
    // a size/framerate range instead of a fixed mode to keep OBS linking.
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
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle { width, height },
            spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            spa::utils::Rectangle {
                width: 8192,
                height: 8192
            }
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

fn build_buffers_pod_bytes(width: u32, height: u32) -> Vec<u8> {
    use spa::pod::builder::Builder;

    let stride = (width * 4) as i32;
    let size = stride * height as i32;
    let mut data = Vec::with_capacity(512);
    data.resize(512, 0u8);
    let mut builder = Builder::new(&mut data);

    let mut frame: MaybeUninit<spa_sys::spa_pod_frame> = MaybeUninit::uninit();
    unsafe {
        builder
            .push_object(
                &mut frame,
                spa_sys::SPA_TYPE_OBJECT_ParamBuffers,
                spa_sys::SPA_PARAM_Buffers,
            )
            .expect("push_object failed");
    }

    builder
        .add_prop(spa_sys::SPA_PARAM_BUFFERS_blocks, 0)
        .expect("add blocks failed");
    builder.add_int(1).expect("add blocks value failed");

    builder
        .add_prop(spa_sys::SPA_PARAM_BUFFERS_size, 0)
        .expect("add size failed");
    builder.add_int(size).expect("add size value failed");

    builder
        .add_prop(spa_sys::SPA_PARAM_BUFFERS_stride, 0)
        .expect("add stride failed");
    builder.add_int(stride).expect("add stride value failed");

    builder
        .add_prop(spa_sys::SPA_PARAM_BUFFERS_align, 0)
        .expect("add align failed");
    builder.add_int(16).expect("add align value failed");

    // Intentionally omit SPA_PARAM_BUFFERS_dataType. A previous mask intended
    // to force MemFd/MemPtr caused PipeWire to spam "invalid memory type".
    // Size/stride/align are enough to prevent OBS from giving us zero-length
    // mapped buffers while leaving memory-type negotiation to PipeWire.

    unsafe {
        builder.pop(&mut frame.assume_init());
    }

    data
}
