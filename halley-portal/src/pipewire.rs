use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Cursor;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::ptr;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use pipewire::context::ContextRc;
use pipewire::core::CoreRc;
use pipewire::loop_::Timeout;
use pipewire::main_loop::MainLoopRc;
use pipewire::properties::PropertiesBox;
use pipewire::spa;
use pipewire::spa::buffer::DataType;
use pipewire::spa::sys as spa_sys;
use pipewire::stream::{StreamFlags, StreamListener, StreamRc, StreamState};

const CURSOR_META_SIZE: usize = std::mem::size_of::<spa_sys::spa_meta_cursor>()
    + std::mem::size_of::<spa_sys::spa_meta_bitmap>()
    + 256 * 256 * 4;
// DMA-BUF frames are acknowledged by the compositor only after their renderer
// fence signals. The PipeWire process callback therefore cannot return (and
// queue the buffer) before its pixels are complete.
const ADVERTISE_DMABUF: bool = true;
const MAX_FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);
const IDLE_LOOP_INTERVAL: Duration = Duration::from_millis(100);
const DMABUF_CHUNK_MARKER: u32 = 9;
const MAX_DMABUF_FIXATION_ATTEMPTS: u8 = 2;

enum Command {
    Create {
        handle: String,
        source: halley_ipc::CaptureSource,
        cursor_mode: halley_ipc::CursorMode,
        reply: Sender<Result<(u32, Option<u64>), String>>,
    },
    Destroy(String),
    Quit,
}

struct ActiveStream {
    stream: StreamRc,
    _listener: StreamListener<u64>,
}

struct AllocatedDmabuf {
    _bo: gbm::BufferObject<()>,
    fds: Vec<OwnedFd>,
    buffer_id: u64,
    modifier: u64,
    planes: Vec<halley_ipc::DmabufPlane>,
}

struct AllocatedMemfd {
    fd: OwnedFd,
}

enum AllocatedBuffer {
    Dmabuf(AllocatedDmabuf),
    Memfd(AllocatedMemfd),
}

#[derive(Clone)]
struct DmabufAllocator {
    device: Rc<gbm::Device<File>>,
    formats: Vec<DmabufAllocationFormat>,
}

#[derive(Clone, Copy)]
struct DmabufAllocationFormat {
    modifier: gbm::Modifier,
    plane_count: u32,
}

#[derive(Debug, PartialEq, Eq)]
struct NegotiatedDmabufModifier {
    values: Vec<u64>,
    needs_fixation: bool,
}

#[derive(Default)]
struct DmabufNegotiation {
    fixation_attempts: u8,
    memfd_only: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum ModifierDecision {
    Fixate(u64),
    Ready(u64),
    Fallback,
    MemFd,
}

impl DmabufNegotiation {
    fn decide(
        &mut self,
        negotiated: Option<&NegotiatedDmabufModifier>,
        available: &[u64],
    ) -> Result<ModifierDecision, &'static str> {
        let Some(negotiated) = negotiated else {
            return Ok(ModifierDecision::MemFd);
        };
        if self.memfd_only {
            return Err("consumer still requested DMA-BUF after shared-memory fallback");
        }
        let selected = negotiated
            .values
            .iter()
            .copied()
            .find(|value| available.contains(value));
        if selected.is_none()
            || (negotiated.needs_fixation && self.fixation_attempts >= MAX_DMABUF_FIXATION_ATTEMPTS)
        {
            self.memfd_only = true;
            return Ok(ModifierDecision::Fallback);
        }
        let selected = selected.expect("supported modifier was checked above");
        if negotiated.needs_fixation {
            self.fixation_attempts += 1;
            Ok(ModifierDecision::Fixate(selected))
        } else {
            // Keep the retry budget for the whole stream. A consumer may first
            // fixate successfully, fail its import, and restart negotiation.
            Ok(ModifierDecision::Ready(selected))
        }
    }
}

pub struct Producer {
    commands: Sender<Command>,
    quit: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Producer {
    pub fn new() -> Self {
        let (commands, receiver) = mpsc::channel();
        let quit = Arc::new(AtomicBool::new(false));
        let thread_quit = quit.clone();
        let thread = std::thread::Builder::new()
            .name("halley-pipewire".to_string())
            .spawn(move || run_thread(receiver, thread_quit))
            .expect("failed to start PipeWire thread");
        Self {
            commands,
            quit,
            thread: Some(thread),
        }
    }

    pub fn create_stream(
        &self,
        handle: &str,
        source: halley_ipc::CaptureSource,
        cursor_mode: halley_ipc::CursorMode,
    ) -> Result<(u32, Option<u64>), String> {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .send(Command::Create {
                handle: handle.to_string(),
                source,
                cursor_mode,
                reply,
            })
            .map_err(|_| "PipeWire thread stopped".to_string())?;
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|err| format!("timed out creating PipeWire stream: {err}"))?
    }

    pub fn destroy_stream(&self, handle: &str) {
        let _ = self.commands.send(Command::Destroy(handle.to_string()));
    }
}

impl Drop for Producer {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        let _ = self.commands.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_thread(commands: Receiver<Command>, quit: Arc<AtomicBool>) {
    pipewire::init();
    let mainloop = match MainLoopRc::new(None) {
        Ok(mainloop) => mainloop,
        Err(err) => {
            eventline::error!("PipeWire main loop: {err}");
            return;
        }
    };
    let context = match ContextRc::new(&mainloop, None) {
        Ok(context) => context,
        Err(err) => {
            eventline::error!("PipeWire context: {err}");
            return;
        }
    };
    let core = match context.connect_rc(None) {
        Ok(core) => core,
        Err(err) => {
            eventline::error!("PipeWire connection: {err}");
            return;
        }
    };
    let mut streams = HashMap::new();
    let mut next_process = Instant::now();
    while !quit.load(Ordering::Relaxed) {
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Create {
                    handle,
                    source,
                    cursor_mode,
                    reply,
                } => {
                    let result = create_stream(&mainloop, &core, &handle, source, cursor_mode).map(
                        |(stream, listener, node, serial)| {
                            streams.insert(
                                handle,
                                ActiveStream {
                                    stream,
                                    _listener: listener,
                                },
                            );
                            (node, serial)
                        },
                    );
                    let _ = reply.send(result);
                }
                Command::Destroy(handle) => {
                    if let Some(active) = streams.remove(&handle) {
                        let _ = active.stream.disconnect();
                    }
                }
                Command::Quit => return,
            }
        }
        let now = Instant::now();
        if now >= next_process {
            for active in streams.values() {
                // Allocator/driver streams are fed by an external producer.
                // Triggering at the advertised maximum rate gives PipeWire a
                // chance to request a buffer; paused/unlinked streams simply
                // do not run their process callback.
                let _ = active.stream.trigger_process();
            }
            next_process = now + MAX_FRAME_INTERVAL;
        }
        let timeout = if streams.is_empty() {
            // Commands are delivered over a standard channel rather than a
            // PipeWire event source. A modest idle poll keeps stream creation
            // responsive without waking an unused portal sixty times a second.
            IDLE_LOOP_INTERVAL
        } else {
            next_process
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(16))
        };
        let _ = mainloop.loop_().iterate(Timeout::Finite(timeout));
    }
}

fn create_stream(
    mainloop: &MainLoopRc,
    core: &CoreRc,
    handle: &str,
    source: halley_ipc::CaptureSource,
    cursor_mode: halley_ipc::CursorMode,
) -> Result<(StreamRc, StreamListener<u64>, u32, Option<u64>), String> {
    let width = source.width() as u32;
    let height = source.height() as u32;
    let wants_dmabuf = ADVERTISE_DMABUF
        && !matches!(
            (&source, cursor_mode),
            (
                halley_ipc::CaptureSource::Window { .. },
                halley_ipc::CursorMode::Embedded
            )
        );
    // Query the compositor before advertising DMA-BUF. Allocating from the
    // wrong render node can be accepted lazily by EGL and still fault on the
    // first draw, so filesystem guessing is not a safe compatibility test.
    let mut compositor_connection = crate::compositor::connect()?;
    let dmabuf_allocator = if wants_dmabuf {
        match crate::compositor::capture_capabilities(&mut compositor_connection) {
            Ok(capabilities) => match open_gbm_device(width, height, &capabilities) {
                Ok(allocator) => Some(allocator),
                Err(err) => {
                    eventline::warn!("DMA-BUF allocation unavailable, using MemFd: {err}");
                    None
                }
            },
            Err(err) => {
                eventline::warn!("DMA-BUF capabilities unavailable, using MemFd: {err}");
                None
            }
        }
    } else {
        None
    };
    let dmabuf_modifiers = dmabuf_allocator
        .as_ref()
        .map_or_else(Vec::new, |allocator| {
            allocator
                .formats
                .iter()
                .map(|format| u64::from(format.modifier))
                .collect()
        });
    let mut properties = PropertiesBox::new();
    properties.insert("media.class", "Video/Source");
    properties.insert("media.name", format!("halley-screencast-{handle}"));
    properties.insert("media.role", "Screen");
    properties.insert("node.name", "xdg-desktop-portal-halley");
    properties.insert("node.pause-on-idle", "false");
    properties.insert("stream.is-live", "true");
    let stream = StreamRc::new(
        core.clone(),
        &format!("halley-screencast-{handle}"),
        properties,
    )
    .map_err(|err| format!("create PipeWire stream: {err}"))?;

    // One compositor connection lives for the stream's entire lifetime. The
    // old per-frame connection created a compositor worker thread and a Unix
    // socket round trip sixty times per second.
    let compositor = Rc::new(RefCell::new(compositor_connection));
    let transport_logged = Rc::new(Cell::new(false));
    let frame_failure_logged = Rc::new(Cell::new(false));
    let buffer_ids = Rc::new(RefCell::new(HashMap::<RawFd, u64>::new()));
    let next_buffer_id = Rc::new(Cell::new(1u64));
    let selected_dmabuf_format = Rc::new(Cell::new(None::<DmabufAllocationFormat>));
    let process_handle = handle.to_string();
    let process_source = source.clone();
    let listener = stream
        .add_local_listener_with_user_data(0u64)
        .state_changed({
            let handle = handle.to_string();
            move |_stream, _frame_count, old, new| match &new {
                StreamState::Error(message) => eventline::error!(
                    "screencast {handle}: PipeWire state {old:?} -> {new:?}: {message}"
                ),
                _ => eventline::debug!("screencast {handle}: PipeWire state {old:?} -> {new:?}"),
            }
        })
        .param_changed({
            let allocator = dmabuf_allocator.clone();
            let selected_dmabuf_format = selected_dmabuf_format.clone();
            let modifiers = dmabuf_modifiers.clone();
            let mut negotiation = DmabufNegotiation::default();
            move |stream, _frame_count, id, param| {
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Some(param) = param else {
                    return;
                };
                let negotiated = match negotiated_dmabuf_modifier(param) {
                    Ok(negotiated) => negotiated,
                    Err(err) => {
                        eventline::error!("inspect negotiated PipeWire format: {err}");
                        return;
                    }
                };
                let decision = match negotiation.decide(negotiated.as_ref(), &modifiers) {
                    Ok(decision) => decision,
                    Err(err) => {
                        eventline::error!("negotiate PipeWire DMA-BUF: {err}");
                        return;
                    }
                };
                let selected_modifier = match decision {
                    ModifierDecision::Fixate(_) | ModifierDecision::Fallback => {
                        // Announce a final EnumFormat before configuring buffers.
                        // Chromium can return an unfixated modifier choice even
                        // when the producer originally offered scalar modifiers.
                        selected_dmabuf_format.set(None);
                        let offered = match decision {
                            ModifierDecision::Fixate(modifier) => {
                                eventline::info!(
                                    "screencast: finalizing DMA-BUF modifier {modifier:#x}"
                                );
                                std::iter::once(modifier)
                                    .chain(
                                        modifiers
                                            .iter()
                                            .copied()
                                            .filter(|value| *value != modifier),
                                    )
                                    .collect()
                            }
                            ModifierDecision::Fallback => {
                                eventline::warn!(
                                    "screencast: DMA-BUF negotiation did not settle, using MemFd"
                                );
                                Vec::new()
                            }
                            _ => unreachable!(),
                        };
                        let formats = match format_pods(width, height, &offered) {
                            Ok(formats) => formats,
                            Err(err) => {
                                eventline::error!("build fixated PipeWire format: {err}");
                                return;
                            }
                        };
                        let mut params = formats
                            .iter()
                            .filter_map(|data| spa::pod::Pod::from_bytes(data))
                            .collect::<Vec<_>>();
                        if let Err(err) = stream.update_params(&mut params) {
                            eventline::error!("finalize PipeWire format: {err}");
                        }
                        return;
                    }
                    ModifierDecision::Ready(modifier) => Some(modifier),
                    ModifierDecision::MemFd => None,
                };
                let selected = selected_modifier.and_then(|modifier| {
                    allocator
                        .as_ref()?
                        .formats
                        .iter()
                        .copied()
                        .find(|format| u64::from(format.modifier) == modifier)
                });
                selected_dmabuf_format.set(selected);
                let selected_dmabuf = selected.is_some();
                let selected_blocks = selected.map_or(1, |format| format.plane_count);
                let buffers_data =
                    match buffers_pod(width, height, selected_dmabuf, selected_blocks) {
                        Ok(data) => data,
                        Err(err) => {
                            eventline::error!("build negotiated PipeWire buffers: {err}");
                            return;
                        }
                    };
                let cursor_meta_data = meta_pod(spa_sys::SPA_META_Cursor, CURSOR_META_SIZE);
                let header_meta_data = meta_pod(
                    spa_sys::SPA_META_Header,
                    std::mem::size_of::<spa_sys::spa_meta_header>(),
                );
                let damage_meta_data = meta_pod(
                    spa_sys::SPA_META_VideoDamage,
                    std::mem::size_of::<spa_sys::spa_meta_region>(),
                );
                let Some(buffers) = spa::pod::Pod::from_bytes(&buffers_data) else {
                    eventline::error!("invalid negotiated PipeWire buffers POD");
                    return;
                };
                let Some(cursor_meta) = spa::pod::Pod::from_bytes(&cursor_meta_data) else {
                    eventline::error!("invalid negotiated PipeWire cursor POD");
                    return;
                };
                let Some(header_meta) = spa::pod::Pod::from_bytes(&header_meta_data) else {
                    eventline::error!("invalid negotiated PipeWire header POD");
                    return;
                };
                let Some(damage_meta) = spa::pod::Pod::from_bytes(&damage_meta_data) else {
                    eventline::error!("invalid negotiated PipeWire damage POD");
                    return;
                };
                let mut params = [buffers, cursor_meta, header_meta, damage_meta];
                if let Err(err) = stream.update_params(&mut params) {
                    eventline::error!("update negotiated PipeWire parameters: {err}");
                }
            }
        })
        .add_buffer({
            let handle = handle.to_string();
            let compositor = compositor.clone();
            let allocator = dmabuf_allocator.clone();
            let selected_dmabuf_format = selected_dmabuf_format.clone();
            let buffer_ids = buffer_ids.clone();
            let next_buffer_id = next_buffer_id.clone();
            move |_stream, _frame_count, buffer| {
                let Some(data_type) = allowed_buffer_type(buffer) else {
                    eventline::error!("PipeWire did not permit a supported buffer type");
                    return;
                };
                let allocated = match data_type {
                    DataType::DmaBuf => {
                        let Some(allocator) = allocator.as_ref() else {
                            eventline::error!(
                                "PipeWire selected DMA-BUF without an available allocator"
                            );
                            return;
                        };
                        let Some(format) = selected_dmabuf_format.get() else {
                            eventline::error!(
                                "PipeWire requested a DMA-BUF before selecting its modifier"
                            );
                            return;
                        };
                        let buffer_id = next_buffer_id.get();
                        next_buffer_id.set(buffer_id.wrapping_add(1).max(1));
                        let allocated = match allocate_dmabuf(
                            buffer,
                            &allocator.device,
                            width,
                            height,
                            format.modifier,
                            buffer_id,
                        ) {
                            Ok(allocated) => allocated,
                            Err(err) => {
                                eventline::error!("could not allocate PipeWire DMA-BUF: {err}");
                                return;
                            }
                        };
                        let Some(primary_fd) = allocated.fds.first().map(AsRawFd::as_raw_fd) else {
                            eventline::error!("allocated DMA-BUF has no planes");
                            return;
                        };
                        let request = halley_ipc::RegisterDmabufRequest {
                            stream_handle: handle.clone(),
                            buffer_id: allocated.buffer_id,
                            width: width as i32,
                            height: height as i32,
                            format: u32::from_le_bytes(*b"XR24"),
                            modifier: allocated.modifier,
                            flags: 0,
                            planes: allocated.planes.clone(),
                        };
                        let fds = allocated
                            .fds
                            .iter()
                            .map(AsRawFd::as_raw_fd)
                            .collect::<Vec<_>>();
                        if let Err(err) = crate::compositor::register_dmabuf(
                            &mut compositor.borrow_mut(),
                            request,
                            &fds,
                        ) {
                            eventline::error!("DMA-BUF registration rejected: {err}");
                        }
                        buffer_ids.borrow_mut().insert(primary_fd, buffer_id);
                        AllocatedBuffer::Dmabuf(allocated)
                    }
                    DataType::MemFd => match allocate_memfd(buffer, width, height) {
                        Ok(allocated) => AllocatedBuffer::Memfd(allocated),
                        Err(err) => {
                            eventline::error!("could not allocate PipeWire MemFd: {err}");
                            return;
                        }
                    },
                    _ => {
                        eventline::error!("unsupported PipeWire buffer type {data_type:?}");
                        return;
                    }
                };
                unsafe {
                    (*buffer).user_data = Box::into_raw(Box::new(allocated)).cast();
                }
            }
        })
        .remove_buffer({
            let handle = handle.to_string();
            let compositor = compositor.clone();
            let buffer_ids = buffer_ids.clone();
            move |_stream, _frame_count, buffer| {
                if let Some(allocated) = take_allocated_buffer(buffer) {
                    match allocated.as_ref() {
                        AllocatedBuffer::Dmabuf(allocated) => {
                            let _ = crate::compositor::remove_dmabuf(
                                &mut compositor.borrow_mut(),
                                handle.clone(),
                                allocated.buffer_id,
                            );
                            if let Some(fd) = allocated.fds.first().map(AsRawFd::as_raw_fd) {
                                buffer_ids.borrow_mut().remove(&fd);
                            }
                        }
                        AllocatedBuffer::Memfd(allocated) => {
                            // Keep the owned descriptor alive until PipeWire
                            // has completely removed this buffer.
                            let _ = allocated.fd.as_raw_fd();
                        }
                    }
                    clear_buffer_fds(buffer);
                }
            }
        })
        .process({
            let compositor = compositor.clone();
            let transport_logged = transport_logged.clone();
            let frame_failure_logged = frame_failure_logged.clone();
            let buffer_ids = buffer_ids.clone();
            move |stream, frame_count| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let (response, sequence) = {
                    let datas = buffer.datas_mut();
                    let Some(data) = datas.first_mut() else {
                        return;
                    };
                    if !transport_logged.replace(true) {
                        eventline::info!(
                            "screencast {}: PipeWire selected {:?}",
                            process_handle,
                            data.type_()
                        );
                    }
                    let mapoffset = data.as_raw().mapoffset;
                    let maxsize = data.as_raw().maxsize;
                    let data_type = data.type_();
                    let buffer = match data_type {
                        DataType::MemFd if data.fd() >= 0 => halley_ipc::CaptureBuffer::MemFd {
                            fd_index: 0,
                            offset: u64::from(mapoffset),
                            size: u64::from(maxsize),
                            stride: width * 4,
                        },
                        DataType::DmaBuf => {
                            let Some(buffer_id) = buffer_ids.borrow().get(&data.fd()).copied()
                            else {
                                eventline::warn!("PipeWire returned an unregistered DMA-BUF");
                                return;
                            };
                            halley_ipc::CaptureBuffer::Dmabuf { buffer_id }
                        }
                        _ => return,
                    };
                    let request = halley_ipc::CaptureFrameRequest {
                        stream_handle: process_handle.clone(),
                        source: process_source.clone(),
                        cursor_mode,
                        buffer,
                    };
                    let fd = (data.type_() == DataType::MemFd).then_some(data.fd());
                    match crate::compositor::capture_frame(
                        &mut compositor.borrow_mut(),
                        request,
                        fd,
                    ) {
                        Ok(response) => {
                            frame_failure_logged.set(false);
                            let chunk = data.chunk_mut();
                            *chunk.offset_mut() = 0;
                            // DMA-BUF allocation size is modifier-dependent and
                            // cannot be described as stride * height. Hyprland
                            // and xdpw use a small nonzero marker because some
                            // clients still treat a zero chunk as an empty
                            // frame even though the pixels live in the fd.
                            *chunk.size_mut() = if data_type == DataType::DmaBuf {
                                DMABUF_CHUNK_MARKER
                            } else {
                                maxsize
                            };
                            *chunk.stride_mut() = (width * 4) as i32;
                            let sequence = *frame_count;
                            *frame_count = frame_count.wrapping_add(1);
                            (response, sequence)
                        }
                        Err(err) => {
                            if !frame_failure_logged.replace(true) {
                                eventline::warn!("screencast frame failed: {err}");
                            }
                            let chunk = data.chunk_mut();
                            *chunk.offset_mut() = 0;
                            *chunk.size_mut() = 0;
                            *chunk.stride_mut() = (width * 4) as i32;
                            return;
                        }
                    }
                };
                fill_frame_meta(&buffer, sequence, width, height);
                fill_cursor_meta(&buffer, response.cursor.as_ref());
            }
        })
        .register()
        .map_err(|err| format!("register PipeWire listener: {err}"))?;

    let format_data = format_pods(width, height, &dmabuf_modifiers)?;
    let mut params = format_data
        .iter()
        .map(|data| {
            spa::pod::Pod::from_bytes(data).ok_or_else(|| "invalid PipeWire format POD".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    // Keep DMA-BUF first so OBS retains its zero-copy path. The second,
    // modifier-free format lets consumers whose GPU cannot import the
    // compositor's modifier negotiate a producer-owned MemFd instead.
    let flags = StreamFlags::ALLOC_BUFFERS | StreamFlags::DRIVER;
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            flags,
            params.as_mut_slice(),
        )
        .map_err(|err| format!("connect PipeWire stream: {err}"))?;
    stream
        .set_active(true)
        .map_err(|err| format!("activate PipeWire stream: {err}"))?;
    let node = wait_for_node(mainloop, &stream)?;
    let serial = stream
        .properties()
        .get("object.serial")
        .and_then(|value| value.parse().ok());
    Ok((stream, listener, node, serial))
}

fn wait_for_node(mainloop: &MainLoopRc, stream: &StreamRc) -> Result<u32, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let node = stream.node_id();
        if node != pipewire::constants::ID_ANY {
            return Ok(node);
        }
        if std::time::Instant::now() >= deadline {
            return Err("PipeWire did not assign a node id".to_string());
        }
        let _ = mainloop
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(16)));
    }
}

fn negotiated_dmabuf_modifier(
    format: &spa::pod::Pod,
) -> Result<Option<NegotiatedDmabufModifier>, String> {
    let Ok((_, spa::pod::Value::Object(object))) =
        spa::pod::deserialize::PodDeserializer::deserialize_from::<spa::pod::Value>(
            format.as_bytes(),
        )
    else {
        return Err("format is not a SPA object".to_string());
    };
    let Some(property) = object.properties.iter().find(|property| {
        property.key == spa::param::format::FormatProperties::VideoModifier.as_raw()
    }) else {
        return Ok(None);
    };
    match &property.value {
        spa::pod::Value::Long(modifier) => Ok(Some(NegotiatedDmabufModifier {
            values: vec![*modifier as u64],
            needs_fixation: property
                .flags
                .contains(spa::pod::PropertyFlags::DONT_FIXATE),
        })),
        spa::pod::Value::Choice(spa::pod::ChoiceValue::Long(spa::utils::Choice(_, choice))) => {
            let mut values = match choice {
                spa::utils::ChoiceEnum::None(value) => vec![*value as u64],
                spa::utils::ChoiceEnum::Enum {
                    default,
                    alternatives,
                } => std::iter::once(*default)
                    .chain(alternatives.iter().copied())
                    .map(|value| value as u64)
                    .collect(),
                _ => return Err("DMA-BUF modifier choice was not an enumeration".to_string()),
            };
            let mut seen = Vec::new();
            values.retain(|value| {
                if seen.contains(value) {
                    false
                } else {
                    seen.push(*value);
                    true
                }
            });
            Ok(Some(NegotiatedDmabufModifier {
                needs_fixation: property
                    .flags
                    .contains(spa::pod::PropertyFlags::DONT_FIXATE)
                    || values.len() > 1,
                values,
            }))
        }
        _ => Err("DMA-BUF modifier was neither a Long nor a Long choice".to_string()),
    }
}

fn format_pods(width: u32, height: u32, dmabuf_modifiers: &[u64]) -> Result<Vec<Vec<u8>>, String> {
    let mut formats = Vec::with_capacity(dmabuf_modifiers.len() + 1);
    for modifier in dmabuf_modifiers {
        formats.push(format_pod(width, height, Some(*modifier))?);
    }
    formats.push(format_pod(width, height, None)?);
    Ok(formats)
}

fn format_pod(width: u32, height: u32, modifier: Option<u64>) -> Result<Vec<u8>, String> {
    let object = match modifier {
        Some(modifier) => spa::pod::object!(
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
            {
                let mut property = spa::pod::property!(
                    spa::param::format::FormatProperties::VideoModifier,
                    Long,
                    modifier as i64
                );
                property.flags = spa::pod::PropertyFlags::MANDATORY;
                property
            },
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoSize,
                Rectangle,
                spa::utils::Rectangle { width, height }
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoFramerate,
                Fraction,
                spa::utils::Fraction { num: 0, denom: 1 }
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoMaxFramerate,
                Choice,
                Range,
                Fraction,
                spa::utils::Fraction { num: 60, denom: 1 },
                spa::utils::Fraction { num: 1, denom: 1 },
                spa::utils::Fraction { num: 60, denom: 1 }
            ),
        ),
        None => spa::pod::object!(
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
                Fraction,
                spa::utils::Fraction { num: 0, denom: 1 }
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoMaxFramerate,
                Choice,
                Range,
                Fraction,
                spa::utils::Fraction { num: 60, denom: 1 },
                spa::utils::Fraction { num: 1, denom: 1 },
                spa::utils::Fraction { num: 60, denom: 1 }
            ),
        ),
    };
    spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map(|result| result.0.into_inner())
    .map_err(|err| format!("serialize PipeWire format: {err}"))
}

fn buffer_data_types(allow_dmabuf: bool) -> i32 {
    if allow_dmabuf {
        1 << spa_sys::SPA_DATA_DmaBuf
    } else {
        1 << spa_sys::SPA_DATA_MemFd
    }
}

fn buffers_pod(
    width: u32,
    height: u32,
    allow_dmabuf: bool,
    data_blocks: u32,
) -> Result<Vec<u8>, String> {
    use spa::pod::Property;
    let data_type = buffer_data_types(allow_dmabuf);
    let buffer_count = spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
        spa::utils::ChoiceFlags::empty(),
        spa::utils::ChoiceEnum::Range {
            default: 2,
            min: 2,
            max: 32,
        },
    )));
    let data_types = spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
        spa::utils::ChoiceFlags::empty(),
        spa::utils::ChoiceEnum::Flags {
            default: data_type,
            flags: vec![data_type],
        },
    )));
    let mut properties = vec![
        Property::new(spa_sys::SPA_PARAM_BUFFERS_buffers, buffer_count),
        Property::new(
            spa_sys::SPA_PARAM_BUFFERS_blocks,
            spa::pod::Value::Int(data_blocks as i32),
        ),
        Property::new(spa_sys::SPA_PARAM_BUFFERS_align, spa::pod::Value::Int(16)),
        Property::new(spa_sys::SPA_PARAM_BUFFERS_dataType, data_types),
    ];
    if !allow_dmabuf {
        properties.push(Property::new(
            spa_sys::SPA_PARAM_BUFFERS_size,
            spa::pod::Value::Int((width * height * 4) as i32),
        ));
        properties.push(Property::new(
            spa_sys::SPA_PARAM_BUFFERS_stride,
            spa::pod::Value::Int((width * 4) as i32),
        ));
    }
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties,
    };
    spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map(|result| result.0.into_inner())
    .map_err(|err| format!("serialize PipeWire buffers: {err}"))
}

fn open_gbm_device(
    width: u32,
    height: u32,
    capabilities: &halley_ipc::CaptureCapabilities,
) -> Result<DmabufAllocator, String> {
    let main_device = capabilities
        .main_device
        .ok_or_else(|| "compositor has no hardware render node".to_string())?;
    let modifiers = capabilities
        .dmabuf_formats
        .iter()
        .filter(|format| format.fourcc == u32::from_le_bytes(*b"XR24"))
        .map(|format| gbm::Modifier::from(format.modifier))
        .collect::<Vec<_>>();
    if modifiers.is_empty() {
        return Err("compositor cannot import XR24 DMA-BUFs".to_string());
    }
    let mut nodes = std::fs::read_dir("/dev/dri")
        .map_err(|err| format!("enumerate /dev/dri: {err}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let path = entry.path();
            (name.to_string_lossy().starts_with("renderD")
                && entry
                    .metadata()
                    .is_ok_and(|metadata| metadata.rdev() == main_device))
            .then_some(path)
        })
        .collect::<Vec<PathBuf>>();
    nodes.sort();
    let mut errors = Vec::new();
    for node in nodes {
        let result = (|| {
            let file = OpenOptions::new().read(true).write(true).open(&node)?;
            let device = gbm::Device::new(file)?;
            let mut allocation_errors = Vec::new();
            let mut formats = Vec::new();
            for modifier in &modifiers {
                match create_gbm_buffer(&device, width, height, *modifier) {
                    Ok(probe) if probe.modifier() == *modifier => {
                        formats.push(DmabufAllocationFormat {
                            modifier: *modifier,
                            plane_count: probe.plane_count(),
                        });
                        drop(probe);
                    }
                    Ok(probe) => allocation_errors.push(format!(
                        "requested {modifier:?}, GBM returned {:?}",
                        probe.modifier()
                    )),
                    Err(err) => allocation_errors.push(format!("{modifier:?}: {err}")),
                }
            }
            if formats.is_empty() {
                Err(std::io::Error::other(allocation_errors.join(", ")))
            } else {
                Ok(DmabufAllocator {
                    device: Rc::new(device),
                    formats,
                })
            }
        })();
        match result {
            Ok(result) => {
                let modifiers = result
                    .formats
                    .iter()
                    .map(|format| format!("{:#x}", u64::from(format.modifier)))
                    .collect::<Vec<_>>()
                    .join(", ");
                eventline::info!(
                    "screencast DMA-BUF allocator: {} (modifiers: {modifiers})",
                    node.display(),
                );
                return Ok(result);
            }
            Err(err) => errors.push(format!("{}: {err}", node.display())),
        }
    }
    if errors.is_empty() {
        Err(format!(
            "no DRM render node matches compositor device {main_device}"
        ))
    } else {
        Err(errors.join("; "))
    }
}

fn create_gbm_buffer(
    device: &gbm::Device<File>,
    width: u32,
    height: u32,
    modifier: gbm::Modifier,
) -> std::io::Result<gbm::BufferObject<()>> {
    if modifier == gbm::Modifier::Invalid {
        device.create_buffer_object(
            width,
            height,
            gbm::Format::Xrgb8888,
            gbm::BufferObjectFlags::RENDERING,
        )
    } else {
        device.create_buffer_object_with_modifiers2(
            width,
            height,
            gbm::Format::Xrgb8888,
            std::iter::once(modifier),
            gbm::BufferObjectFlags::RENDERING,
        )
    }
}

fn allowed_buffer_type(buffer: *mut pipewire::sys::pw_buffer) -> Option<DataType> {
    if buffer.is_null() {
        return None;
    }
    let spa_buffer = unsafe { (*buffer).buffer };
    if spa_buffer.is_null() || unsafe { (*spa_buffer).n_datas } == 0 {
        return None;
    }
    let datas = unsafe { (*spa_buffer).datas };
    if datas.is_null() {
        return None;
    }
    let allowed = unsafe { (*datas).type_ };
    if allowed == u32::MAX || allowed & (1 << spa_sys::SPA_DATA_MemFd) != 0 {
        Some(DataType::MemFd)
    } else if allowed & (1 << spa_sys::SPA_DATA_DmaBuf) != 0 {
        Some(DataType::DmaBuf)
    } else {
        None
    }
}

fn allocate_memfd(
    buffer: *mut pipewire::sys::pw_buffer,
    width: u32,
    height: u32,
) -> Result<AllocatedMemfd, String> {
    if buffer.is_null() {
        return Err("null PipeWire buffer".to_string());
    }
    let spa_buffer = unsafe { (*buffer).buffer };
    if spa_buffer.is_null() {
        return Err("null SPA buffer".to_string());
    }
    let count = unsafe { (*spa_buffer).n_datas as usize };
    let datas = unsafe { (*spa_buffer).datas };
    if count != 1 || datas.is_null() {
        return Err(format!("MemFd buffer requires one data block, got {count}"));
    }
    let size = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| format!("MemFd size overflow for {width}x{height}"))?;
    let raw_fd = unsafe {
        libc::memfd_create(
            c"halley-screencast".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if raw_fd < 0 {
        return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    if unsafe { libc::ftruncate(fd.as_raw_fd(), i64::from(size)) } != 0 {
        return Err(format!("ftruncate: {}", std::io::Error::last_os_error()));
    }
    let data = unsafe { &mut *datas };
    if data.chunk.is_null() {
        return Err("MemFd data block has no SPA chunk".to_string());
    }
    data.type_ = spa_sys::SPA_DATA_MemFd;
    data.flags = spa_sys::SPA_DATA_FLAG_READABLE | spa_sys::SPA_DATA_FLAG_MAPPABLE;
    data.fd = i64::from(fd.as_raw_fd());
    data.mapoffset = 0;
    data.maxsize = size;
    data.data = ptr::null_mut();
    unsafe {
        (*data.chunk).offset = 0;
        (*data.chunk).size = size;
        (*data.chunk).stride = (width * 4) as i32;
        (*data.chunk).flags = 0;
    }
    Ok(AllocatedMemfd { fd })
}

fn allocate_dmabuf(
    buffer: *mut pipewire::sys::pw_buffer,
    device: &gbm::Device<File>,
    width: u32,
    height: u32,
    expected_modifier: gbm::Modifier,
    buffer_id: u64,
) -> Result<AllocatedDmabuf, String> {
    if buffer.is_null() {
        return Err("null PipeWire buffer".to_string());
    }
    let spa_buffer = unsafe { (*buffer).buffer };
    if spa_buffer.is_null() {
        return Err("null SPA buffer".to_string());
    }
    let count = unsafe { (*spa_buffer).n_datas as usize };
    let datas = unsafe { (*spa_buffer).datas };
    if count == 0 || datas.is_null() {
        return Err("SPA buffer has no data planes".to_string());
    }
    let allowed = unsafe { (*datas).type_ };
    if allowed != u32::MAX && allowed & (1 << spa_sys::SPA_DATA_DmaBuf) == 0 {
        return Err(format!(
            "PipeWire did not permit DMA-BUF data (mask {allowed:#x})"
        ));
    }
    let bo = create_gbm_buffer(device, width, height, expected_modifier)
        .map_err(|err| format!("GBM buffer allocation: {err}"))?;
    let modifier = bo.modifier();
    if modifier != expected_modifier {
        return Err(format!(
            "GBM changed modifier from {expected_modifier:?} to {modifier:?}"
        ));
    }
    let plane_count = bo.plane_count() as usize;
    if plane_count != count {
        return Err(format!(
            "PipeWire provided {count} data blocks but GBM allocated {plane_count} planes"
        ));
    }
    let mut planes = Vec::with_capacity(count);
    let mut fds = Vec::with_capacity(count);
    for index in 0..count {
        let plane = index as i32;
        let stride = bo.stride_for_plane(plane);
        let offset = bo.offset(plane);
        let fd = bo
            .fd_for_plane(plane)
            .map_err(|_| format!("export GBM plane {index}"))?;
        let data = unsafe { &mut *datas.add(index) };
        if data.chunk.is_null() {
            return Err(format!("DMA-BUF plane {index} has no SPA chunk"));
        }
        data.type_ = spa_sys::SPA_DATA_DmaBuf;
        data.flags = spa_sys::SPA_DATA_FLAG_READABLE;
        data.fd = fd.as_raw_fd() as i64;
        data.mapoffset = 0;
        // The byte size of a tiled/compressed DMA-BUF plane is not derivable
        // from its visible stride and height. Leave it unspecified and use a
        // nonzero chunk marker, as the established portal implementations do.
        data.maxsize = 0;
        data.data = ptr::null_mut();
        unsafe {
            (*data.chunk).offset = offset;
            (*data.chunk).size = DMABUF_CHUNK_MARKER;
            (*data.chunk).stride = stride as i32;
            (*data.chunk).flags = 0;
        }
        planes.push(halley_ipc::DmabufPlane {
            fd_index: index as u32,
            plane_index: index as u32,
            offset,
            stride,
        });
        fds.push(fd);
    }
    Ok(AllocatedDmabuf {
        _bo: bo,
        fds,
        buffer_id,
        modifier: modifier.into(),
        planes,
    })
}

fn take_allocated_buffer(buffer: *mut pipewire::sys::pw_buffer) -> Option<Box<AllocatedBuffer>> {
    if buffer.is_null() {
        return None;
    }
    let allocation = unsafe { (*buffer).user_data.cast::<AllocatedBuffer>() };
    if allocation.is_null() {
        return None;
    }
    unsafe {
        (*buffer).user_data = ptr::null_mut();
        Some(Box::from_raw(allocation))
    }
}

fn clear_buffer_fds(buffer: *mut pipewire::sys::pw_buffer) {
    if buffer.is_null() {
        return;
    }
    let spa_buffer = unsafe { (*buffer).buffer };
    if spa_buffer.is_null() {
        return;
    }
    let count = unsafe { (*spa_buffer).n_datas as usize };
    let datas = unsafe { (*spa_buffer).datas };
    if datas.is_null() {
        return;
    }
    for index in 0..count {
        unsafe {
            (*datas.add(index)).fd = -1;
        }
    }
}

fn meta_pod(meta_type: u32, meta_size: usize) -> Vec<u8> {
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
            .expect("cursor metadata object");
    }
    builder
        .add_prop(spa_sys::SPA_PARAM_META_type, 0)
        .expect("cursor metadata type");
    builder
        .add_id(spa::utils::Id(meta_type))
        .expect("metadata id");
    builder
        .add_prop(spa_sys::SPA_PARAM_META_size, 0)
        .expect("cursor metadata size");
    builder
        .add_int(meta_size as i32)
        .expect("metadata size value");
    unsafe {
        builder.pop(&mut frame.assume_init());
    }
    data
}

fn fill_frame_meta(buffer: &pipewire::buffer::Buffer<'_>, sequence: u64, width: u32, height: u32) {
    if let Some(meta) = buffer.find_meta::<spa::buffer::meta::MetaHeader>() {
        let header = meta as *const _ as *mut spa_sys::spa_meta_header;
        unsafe {
            (*header).flags = 0;
            (*header).offset = 0;
            (*header).pts = monotonic_timestamp_ns();
            (*header).dts_offset = 0;
            (*header).seq = sequence;
        }
    }
    if let Some(meta) = buffer.find_meta::<spa::buffer::meta::MetaVideoDamage>() {
        let raw = meta.as_raw() as *const _ as *mut spa_sys::spa_meta;
        let region = unsafe { spa_sys::spa_meta_first(raw).cast::<spa_sys::spa_meta_region>() };
        if !region.is_null() && unsafe { spa_sys::spa_meta_check(region.cast(), raw) } {
            unsafe {
                (*region).region.position.x = 0;
                (*region).region.position.y = 0;
                (*region).region.size.width = width;
                (*region).region.size.height = height;
            }
        }
    }
}

fn monotonic_timestamp_ns() -> i64 {
    let mut timestamp = MaybeUninit::<libc::timespec>::uninit();
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, timestamp.as_mut_ptr()) } != 0 {
        return -1;
    }
    let timestamp = unsafe { timestamp.assume_init() };
    timestamp
        .tv_sec
        .saturating_mul(1_000_000_000)
        .saturating_add(timestamp.tv_nsec)
}

fn fill_cursor_meta(
    buffer: &pipewire::buffer::Buffer<'_>,
    metadata: Option<&halley_ipc::CursorMetadata>,
) {
    let Some(meta) = buffer.find_meta::<spa::buffer::meta::MetaCursor>() else {
        return;
    };
    let cursor = meta as *const _ as *mut spa_sys::spa_meta_cursor;
    unsafe {
        let Some(metadata) = metadata else {
            (*cursor).id = 0;
            (*cursor).bitmap_offset = 0;
            return;
        };
        (*cursor).id = 1;
        (*cursor).position.x = metadata.x;
        (*cursor).position.y = metadata.y;
        (*cursor).hotspot.x = metadata.hotspot_x;
        (*cursor).hotspot.y = metadata.hotspot_y;
        let cursor_size = std::mem::size_of::<spa_sys::spa_meta_cursor>();
        let bitmap_size = std::mem::size_of::<spa_sys::spa_meta_bitmap>();
        let bytes = metadata.bgra.len();
        if cursor_size + bitmap_size + bytes > CURSOR_META_SIZE {
            (*cursor).bitmap_offset = 0;
            return;
        }
        (*cursor).bitmap_offset = cursor_size as u32;
        let bitmap = (cursor as *mut u8)
            .add(cursor_size)
            .cast::<spa_sys::spa_meta_bitmap>();
        (*bitmap).format = spa_sys::SPA_VIDEO_FORMAT_BGRA;
        (*bitmap).size.width = metadata.width;
        (*bitmap).size.height = metadata.height;
        (*bitmap).stride = (metadata.width * 4) as i32;
        (*bitmap).offset = bitmap_size as u32;
        std::ptr::copy_nonoverlapping(
            metadata.bgra.as_ptr(),
            (bitmap as *mut u8).add(bitmap_size),
            bytes,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_window_fallback_can_exclude_dmabuf() {
        let data_types = buffer_data_types(false);
        assert_ne!(data_types & (1 << spa_sys::SPA_DATA_MemFd), 0);
        assert_eq!(data_types & (1 << spa_sys::SPA_DATA_DmaBuf), 0);
    }

    #[test]
    fn dmabuf_support_stays_isolated_for_explicit_enablement() {
        let data_types = buffer_data_types(true);
        assert_eq!(data_types & (1 << spa_sys::SPA_DATA_MemFd), 0);
        assert_ne!(data_types & (1 << spa_sys::SPA_DATA_DmaBuf), 0);
    }

    #[test]
    fn production_negotiation_prefers_fence_backed_dmabufs() {
        let data_types = buffer_data_types(ADVERTISE_DMABUF);
        assert_eq!(data_types & (1 << spa_sys::SPA_DATA_MemFd), 0);
        assert_ne!(data_types & (1 << spa_sys::SPA_DATA_DmaBuf), 0);
    }

    #[test]
    fn dmabuf_format_carries_the_allocated_modifier() {
        let data = format_pod(1920, 1080, Some(0)).expect("format POD");
        let pod = spa::pod::Pod::from_bytes(&data).expect("valid POD");
        let value = spa::pod::deserialize::PodDeserializer::deserialize_from::<spa::pod::Value>(
            pod.as_bytes(),
        )
        .expect("deserialize format")
        .1;
        let spa::pod::Value::Object(object) = value else {
            panic!("format is not an object");
        };
        let modifier = object
            .properties
            .iter()
            .find(|property| {
                property.key == spa::param::format::FormatProperties::VideoModifier.as_raw()
            })
            .expect("modifier property");
        assert_eq!(modifier.value, spa::pod::Value::Long(0));
        assert!(modifier.flags.contains(spa::pod::PropertyFlags::MANDATORY));
    }

    #[test]
    fn hardware_stream_offers_dmabuf_then_memfd_fallback() {
        let formats = format_pods(1920, 1080, &[42, 0]).expect("format PODs");
        assert_eq!(formats.len(), 3);
        let tiled = spa::pod::Pod::from_bytes(&formats[0]).expect("tiled DMA-BUF POD");
        let linear = spa::pod::Pod::from_bytes(&formats[1]).expect("linear DMA-BUF POD");
        let memfd = spa::pod::Pod::from_bytes(&formats[2]).expect("MemFd POD");
        assert_eq!(
            negotiated_dmabuf_modifier(tiled),
            Ok(Some(NegotiatedDmabufModifier {
                values: vec![42],
                needs_fixation: false,
            }))
        );
        assert_eq!(
            negotiated_dmabuf_modifier(linear),
            Ok(Some(NegotiatedDmabufModifier {
                values: vec![0],
                needs_fixation: false,
            }))
        );
        assert_eq!(negotiated_dmabuf_modifier(memfd), Ok(None));
    }

    #[test]
    fn dmabuf_modifier_choice_requests_fixation_of_its_default() {
        let data = format_pod(1920, 1080, Some(42)).expect("format POD");
        let (_, mut value) =
            spa::pod::deserialize::PodDeserializer::deserialize_from::<spa::pod::Value>(&data)
                .expect("deserialize format");
        let spa::pod::Value::Object(object) = &mut value else {
            panic!("format is not an object");
        };
        let modifier = object
            .properties
            .iter_mut()
            .find(|property| {
                property.key == spa::param::format::FormatProperties::VideoModifier.as_raw()
            })
            .expect("modifier property");
        modifier.value = spa::pod::Value::Choice(spa::pod::ChoiceValue::Long(spa::utils::Choice(
            spa::utils::ChoiceFlags::empty(),
            spa::utils::ChoiceEnum::Enum {
                default: 42,
                alternatives: vec![0],
            },
        )));
        modifier.flags |= spa::pod::PropertyFlags::DONT_FIXATE;
        let data = spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), &value)
            .expect("serialize choice format")
            .0
            .into_inner();
        let pod = spa::pod::Pod::from_bytes(&data).expect("choice POD");
        assert_eq!(
            negotiated_dmabuf_modifier(pod),
            Ok(Some(NegotiatedDmabufModifier {
                values: vec![42, 0],
                needs_fixation: true,
            }))
        );
    }

    #[test]
    fn negotiation_selects_a_supported_alternative() {
        let mut negotiation = DmabufNegotiation::default();
        let offered = NegotiatedDmabufModifier {
            values: vec![42, 0],
            needs_fixation: true,
        };
        assert_eq!(
            negotiation.decide(Some(&offered), &[0]),
            Ok(ModifierDecision::Fixate(0))
        );
        let fixed = format_pods(1920, 1080, &[0]).expect("fixed formats");
        let fixed = negotiated_dmabuf_modifier(spa::pod::Pod::from_bytes(&fixed[0]).unwrap())
            .expect("fixed modifier");
        assert_eq!(
            negotiation.decide(fixed.as_ref(), &[0]),
            Ok(ModifierDecision::Ready(0))
        );
    }

    #[test]
    fn repeated_fixation_switches_permanently_to_memfd() {
        let mut negotiation = DmabufNegotiation::default();
        let offered = NegotiatedDmabufModifier {
            values: vec![42],
            needs_fixation: true,
        };
        let fixed = NegotiatedDmabufModifier {
            values: vec![42],
            needs_fixation: false,
        };
        for _ in 0..MAX_DMABUF_FIXATION_ATTEMPTS {
            assert_eq!(
                negotiation.decide(Some(&offered), &[42]),
                Ok(ModifierDecision::Fixate(42))
            );
            // A failed consumer import can restart negotiation after a format
            // was already fixed. That must not replenish the retry budget.
            assert_eq!(
                negotiation.decide(Some(&fixed), &[42]),
                Ok(ModifierDecision::Ready(42))
            );
        }
        assert_eq!(
            negotiation.decide(Some(&offered), &[42]),
            Ok(ModifierDecision::Fallback)
        );
        let formats = format_pods(1920, 1080, &[]).expect("MemFd formats");
        assert_eq!(formats.len(), 1);
        let fallback = negotiated_dmabuf_modifier(spa::pod::Pod::from_bytes(&formats[0]).unwrap())
            .expect("fallback format");
        assert_eq!(
            negotiation.decide(fallback.as_ref(), &[42]),
            Ok(ModifierDecision::MemFd)
        );
        assert!(negotiation.decide(Some(&offered), &[42]).is_err());
        assert!(negotiation.decide(Some(&fixed), &[42]).is_err());
    }

    #[test]
    fn unsupported_modifier_recovers_without_allocating_it() {
        let mut negotiation = DmabufNegotiation::default();
        let offered = NegotiatedDmabufModifier {
            values: vec![42],
            needs_fixation: true,
        };
        assert_eq!(
            negotiation.decide(Some(&offered), &[0]),
            Ok(ModifierDecision::Fallback)
        );
        assert_eq!(negotiation.decide(None, &[0]), Ok(ModifierDecision::MemFd));
    }

    #[test]
    fn already_fixed_formats_preserve_the_gpu_path() {
        let mut negotiation = DmabufNegotiation::default();
        let fixed = NegotiatedDmabufModifier {
            values: vec![42],
            needs_fixation: false,
        };
        assert_eq!(
            negotiation.decide(Some(&fixed), &[42]),
            Ok(ModifierDecision::Ready(42))
        );
        assert_eq!(negotiation.fixation_attempts, 0);
        assert!(!negotiation.memfd_only);
    }

    #[test]
    fn a_fixed_single_value_choice_does_not_restart_negotiation() {
        let data = format_pod(1920, 1080, Some(42)).unwrap();
        let (_, mut value) =
            spa::pod::deserialize::PodDeserializer::deserialize_from::<spa::pod::Value>(&data)
                .unwrap();
        let spa::pod::Value::Object(object) = &mut value else {
            panic!("format is not an object");
        };
        let modifier = object
            .properties
            .iter_mut()
            .find(|property| {
                property.key == spa::param::format::FormatProperties::VideoModifier.as_raw()
            })
            .unwrap();
        modifier.value = spa::pod::Value::Choice(spa::pod::ChoiceValue::Long(spa::utils::Choice(
            spa::utils::ChoiceFlags::empty(),
            spa::utils::ChoiceEnum::Enum {
                default: 42,
                alternatives: vec![42],
            },
        )));
        let data = spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), &value)
            .unwrap()
            .0
            .into_inner();
        assert_eq!(
            negotiated_dmabuf_modifier(spa::pod::Pod::from_bytes(&data).unwrap()),
            Ok(Some(NegotiatedDmabufModifier {
                values: vec![42],
                needs_fixation: false
            }))
        );
    }

    #[test]
    fn software_stream_only_offers_memfd() {
        let formats = format_pods(1920, 1080, &[]).expect("format PODs");
        assert_eq!(formats.len(), 1);
        let memfd = spa::pod::Pod::from_bytes(&formats[0]).expect("MemFd POD");
        assert_eq!(negotiated_dmabuf_modifier(memfd), Ok(None));
    }

    #[test]
    fn format_advertises_variable_rate_with_a_real_maximum() {
        let data = format_pod(1920, 1080, None).expect("format POD");
        let (_, value) =
            spa::pod::deserialize::PodDeserializer::deserialize_from::<spa::pod::Value>(&data)
                .expect("deserialize format");
        let spa::pod::Value::Object(object) = value else {
            panic!("format is not an object");
        };
        assert!(object.properties.iter().any(|property| {
            property.key == spa::param::format::FormatProperties::VideoFramerate.as_raw()
                && property.value
                    == spa::pod::Value::Fraction(spa::utils::Fraction { num: 0, denom: 1 })
        }));
        let maximum = object
            .properties
            .iter()
            .find(|property| {
                property.key == spa::param::format::FormatProperties::VideoMaxFramerate.as_raw()
            })
            .expect("maximum framerate");
        let spa::pod::Value::Choice(spa::pod::ChoiceValue::Fraction(spa::utils::Choice(
            _,
            spa::utils::ChoiceEnum::Range { default, min, max },
        ))) = &maximum.value
        else {
            panic!("maximum framerate is not a range");
        };
        assert_eq!(*default, spa::utils::Fraction { num: 60, denom: 1 });
        assert_eq!(*min, spa::utils::Fraction { num: 1, denom: 1 });
        assert_eq!(*max, spa::utils::Fraction { num: 60, denom: 1 });
    }

    #[test]
    fn dmabuf_frames_never_publish_an_empty_chunk() {
        assert_ne!(DMABUF_CHUNK_MARKER, 0);
    }

    #[test]
    fn allocator_buffer_requirements_are_choice_typed() {
        let data = buffers_pod(1920, 1080, true, 2).expect("buffers POD");
        let (_, value) =
            spa::pod::deserialize::PodDeserializer::deserialize_from::<spa::pod::Value>(&data)
                .expect("deserialize buffers");
        let spa::pod::Value::Object(object) = value else {
            panic!("buffers is not an object");
        };
        assert!(
            !object
                .properties
                .iter()
                .any(|property| property.key == spa_sys::SPA_PARAM_BUFFERS_size)
        );
        let data_type = object
            .properties
            .iter()
            .find(|property| property.key == spa_sys::SPA_PARAM_BUFFERS_dataType)
            .expect("data type property");
        let spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
            _,
            spa::utils::ChoiceEnum::Flags { default, flags },
        ))) = &data_type.value
        else {
            panic!("data type is not a flag choice");
        };
        assert_eq!(*default, 1 << spa_sys::SPA_DATA_DmaBuf);
        assert_eq!(flags, &[1 << spa_sys::SPA_DATA_DmaBuf]);
        assert!(object.properties.iter().any(|property| {
            property.key == spa_sys::SPA_PARAM_BUFFERS_blocks
                && property.value == spa::pod::Value::Int(2)
        }));
    }
}
