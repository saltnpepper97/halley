//! Authorized output capture with SHM buffers. Full-frame damage is deliberate:
//! every successful copy initializes the whole buffer, including after reuse.
use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::{
    ext_image_copy_capture_cursor_session_v1::{
        self as cursor, ExtImageCopyCaptureCursorSessionV1 as CursorSession,
    },
    ext_image_copy_capture_frame_v1::{self as frame, ExtImageCopyCaptureFrameV1 as Frame},
    ext_image_copy_capture_manager_v1::{self as manager, ExtImageCopyCaptureManagerV1 as Manager},
    ext_image_copy_capture_session_v1::{
        self as session, ExtImageCopyCaptureSessionV1 as CaptureSession,
    },
};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum, Weak,
    backend::{ClientId, GlobalId},
    protocol::{wl_buffer::WlBuffer, wl_output, wl_shm::Format},
};
use smithay::{
    output::{Output, WeakOutput},
    utils::{Physical, Size, Transform},
    wayland::{
        image_capture_source::{
            ImageCaptureSource, OutputCaptureSourceHandler, OutputCaptureSourceState,
        },
        shm,
    },
};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

pub struct GlobalData {
    allowed: fn(&Client) -> bool,
}
pub trait Handler: OutputCaptureSourceHandler {
    fn image_capture_state(&mut self) -> &mut State;
    fn refresh_image_capture(&mut self);
    fn request_image_capture(&mut self, output: &Output);
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Descriptor {
    pub size: Size<i32, Physical>,
    // Track changes even when they leave the buffer dimensions unchanged.
    pub scale: f64,
    pub transform: Transform,
}

pub struct State {
    _global: GlobalId,
    pub output_sources: OutputCaptureSourceState,
    outputs: Vec<(Output, Descriptor)>,
    sessions: Vec<CaptureSession>,
    frames: Vec<Frame>,
}

pub struct SessionData {
    source: Option<ImageCaptureSource>,
    output: Option<WeakOutput>,
    paint_cursor: bool,
    descriptor: Mutex<Option<Descriptor>>,
    stopped: AtomicBool,
    resource: OnceLock<Weak<CaptureSession>>,
    frame: Mutex<Option<Weak<Frame>>>,
}
pub struct FrameData {
    session: Arc<SessionData>,
    buffer: Mutex<Option<WlBuffer>>,
    captured: AtomicBool,
    finished: AtomicBool,
}
pub struct CursorData(AtomicBool);

impl State {
    pub fn new<D>(display: &DisplayHandle, allowed: fn(&Client) -> bool) -> Self
    where
        D: Handler + GlobalDispatch<Manager, GlobalData>,
    {
        Self {
            _global: display.create_global::<D, Manager, _>(1, GlobalData { allowed }),
            output_sources: OutputCaptureSourceState::new_with_filter::<D, _>(display, allowed),
            outputs: Vec::new(),
            sessions: Vec::new(),
            frames: Vec::new(),
        }
    }

    /// Includes sessions whose protocol object was destroyed while a frame lives.
    fn session_data(&self) -> Vec<Arc<SessionData>> {
        let mut data: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|s| s.data::<Arc<SessionData>>().cloned())
            .collect();
        for frame in &self.frames {
            if let Some(frame) = frame.data::<Arc<FrameData>>()
                && !data.iter().any(|s| Arc::ptr_eq(s, &frame.session))
            {
                data.push(frame.session.clone());
            }
        }
        data
    }

    pub fn sync(&mut self, outputs: Vec<(Output, Descriptor)>) {
        for data in self.session_data() {
            if data.stopped.load(Ordering::Acquire) {
                continue;
            }
            let current = data
                .output
                .as_ref()
                .and_then(WeakOutput::upgrade)
                .and_then(|output| {
                    outputs
                        .iter()
                        .find(|(known, _)| *known == output)
                        .map(|(_, d)| *d)
                })
                .filter(|_| data.source.as_ref().is_some_and(ImageCaptureSource::alive));
            if let Some(current) = current {
                let mut previous = data.descriptor.lock().unwrap();
                if *previous != Some(current) {
                    *previous = Some(current);
                    if let Some(resource) = data.resource.get().and_then(|r| r.upgrade().ok()) {
                        send_constraints(&resource, current);
                    }
                }
            } else {
                data.stop();
            }
        }
        self.outputs = outputs;
    }

    pub fn stop_source(&mut self, source: &ImageCaptureSource) {
        for data in self.session_data() {
            if data.source.as_ref().is_some_and(|s| s.id() == source.id()) {
                data.stop();
            }
        }
    }
    pub fn stop_output(&mut self, output: &Output) {
        for data in self.session_data() {
            if data.output.as_ref().and_then(WeakOutput::upgrade).as_ref() == Some(output) {
                data.stop();
            }
        }
        self.outputs.retain(|(known, _)| known != output);
    }
    pub fn stop_all(&mut self) {
        self.sync(Vec::new());
    }

    pub fn has_pending_frames(&self) -> bool {
        self.frames.iter().any(|frame| {
            let data = frame.data::<Arc<FrameData>>().unwrap();
            data.captured.load(Ordering::Acquire) && !data.finished.load(Ordering::Acquire)
        })
    }

    pub fn take_frames(&mut self, output: &Output) -> Vec<FrameJob> {
        self.frames
            .iter()
            .filter_map(|resource| {
                let data = resource.data::<Arc<FrameData>>()?;
                if !data.captured.load(Ordering::Acquire)
                    || data.finished.load(Ordering::Acquire)
                    || data.session.stopped.load(Ordering::Acquire)
                    || data.session.output.as_ref()?.upgrade().as_ref() != Some(output)
                {
                    return None;
                }
                let descriptor = (*data.session.descriptor.lock().unwrap())?;
                Some(FrameJob {
                    resource: resource.clone(),
                    descriptor,
                    paint_cursor: data.session.paint_cursor,
                })
            })
            .collect()
    }
}

impl SessionData {
    fn stop(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(resource) = self.resource.get().and_then(|r| r.upgrade().ok()) {
            resource.stopped();
        }
        if let Some(resource) = self
            .frame
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|r| r.upgrade().ok())
            && resource
                .data::<Arc<FrameData>>()
                .unwrap()
                .captured
                .load(Ordering::Acquire)
        {
            fail(&resource, frame::FailureReason::Stopped);
        }
    }
}
fn send_constraints(resource: &CaptureSession, descriptor: Descriptor) {
    resource.buffer_size(descriptor.size.w as u32, descriptor.size.h as u32);
    resource.shm_format(Format::Xrgb8888);
    resource.done();
}
fn create_session<D>(
    state: &mut D,
    init: &mut DataInit<'_, D>,
    id: New<CaptureSession>,
    source: Option<ImageCaptureSource>,
    paint_cursor: bool,
) where
    D: Handler + Dispatch<CaptureSession, Arc<SessionData>>,
{
    state.refresh_image_capture();
    let output = source
        .as_ref()
        .filter(|s| s.alive())
        .and_then(|s| s.user_data().get::<WeakOutput>())
        .and_then(WeakOutput::upgrade);
    let descriptor = output.as_ref().and_then(|output| {
        state
            .image_capture_state()
            .outputs
            .iter()
            .find(|(known, _)| known == output)
            .map(|(_, d)| *d)
    });
    let data = Arc::new(SessionData {
        source,
        output: output.as_ref().map(Output::downgrade),
        paint_cursor,
        descriptor: Mutex::new(descriptor),
        stopped: AtomicBool::new(descriptor.is_none()),
        resource: OnceLock::new(),
        frame: Mutex::new(None),
    });
    let resource = init.init(id, data.clone());
    data.resource.set(resource.downgrade()).unwrap();
    if let Some(descriptor) = descriptor {
        send_constraints(&resource, descriptor);
    } else {
        resource.stopped();
    }
    state.image_capture_state().sessions.push(resource);
}

impl<D> GlobalDispatch<Manager, GlobalData, D> for State
where
    D: Handler + GlobalDispatch<Manager, GlobalData> + Dispatch<Manager, ()>,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        id: New<Manager>,
        _: &GlobalData,
        init: &mut DataInit<'_, D>,
    ) {
        init.init(id, ());
    }
    fn can_view(client: Client, data: &GlobalData) -> bool {
        (data.allowed)(&client)
    }
}
impl<D> Dispatch<Manager, (), D> for State
where
    D: Handler
        + Dispatch<Manager, ()>
        + Dispatch<CaptureSession, Arc<SessionData>>
        + Dispatch<CursorSession, CursorData>,
{
    fn request(
        state: &mut D,
        _: &Client,
        resource: &Manager,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, D>,
    ) {
        match request {
            manager::Request::CreateSession {
                session,
                source,
                options,
            } => {
                let WEnum::Value(options) = options else {
                    resource.post_error(manager::Error::InvalidOption, "unknown capture option");
                    return;
                };
                if options.bits() & !manager::Options::PaintCursors.bits() != 0 {
                    resource.post_error(manager::Error::InvalidOption, "unknown capture option");
                    return;
                }
                create_session(
                    state,
                    init,
                    session,
                    ImageCaptureSource::from_resource(&source),
                    options.contains(manager::Options::PaintCursors),
                );
            }
            // Cursor-only capture is unsupported. Its derived session is stopped.
            manager::Request::CreatePointerCursorSession { session, .. } => {
                init.init(session, CursorData(AtomicBool::new(false)));
            }
            _ => {}
        }
    }
}
impl<D> Dispatch<CursorSession, CursorData, D> for State
where
    D: Handler + Dispatch<CursorSession, CursorData> + Dispatch<CaptureSession, Arc<SessionData>>,
{
    fn request(
        state: &mut D,
        _: &Client,
        resource: &CursorSession,
        request: cursor::Request,
        data: &CursorData,
        _: &DisplayHandle,
        init: &mut DataInit<'_, D>,
    ) {
        if let cursor::Request::GetCaptureSession { session } = request {
            if data.0.swap(true, Ordering::AcqRel) {
                resource.post_error(
                    cursor::Error::DuplicateSession,
                    "capture session already created",
                );
                return;
            }
            create_session(state, init, session, None, false);
        }
    }
}
impl<D> Dispatch<CaptureSession, Arc<SessionData>, D> for State
where
    D: Handler + Dispatch<CaptureSession, Arc<SessionData>> + Dispatch<Frame, Arc<FrameData>>,
{
    fn request(
        state: &mut D,
        _: &Client,
        resource: &CaptureSession,
        request: session::Request,
        data: &Arc<SessionData>,
        _: &DisplayHandle,
        init: &mut DataInit<'_, D>,
    ) {
        if let session::Request::CreateFrame { frame } = request {
            let mut current = data.frame.lock().unwrap();
            if current.as_ref().and_then(|r| r.upgrade().ok()).is_some() {
                resource.post_error(
                    session::Error::DuplicateFrame,
                    "destroy the previous frame first",
                );
                return;
            }
            let frame = init.init(
                frame,
                Arc::new(FrameData {
                    session: data.clone(),
                    buffer: Mutex::new(None),
                    captured: AtomicBool::new(false),
                    finished: AtomicBool::new(false),
                }),
            );
            *current = Some(frame.downgrade());
            state.image_capture_state().frames.push(frame);
        }
    }
    fn destroyed(state: &mut D, _: ClientId, resource: &CaptureSession, _: &Arc<SessionData>) {
        // Existing frame objects remain valid when the session object is destroyed.
        state
            .image_capture_state()
            .sessions
            .retain(|s| s != resource);
    }
}
impl<D> Dispatch<Frame, Arc<FrameData>, D> for State
where
    D: Handler + Dispatch<Frame, Arc<FrameData>>,
{
    fn request(
        state: &mut D,
        _: &Client,
        resource: &Frame,
        request: frame::Request,
        data: &Arc<FrameData>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        if matches!(request, frame::Request::Destroy) {
            return;
        }
        if data.captured.load(Ordering::Acquire) {
            resource.post_error(
                frame::Error::AlreadyCaptured,
                "frame has already been captured",
            );
            return;
        }
        match request {
            frame::Request::AttachBuffer { buffer } => {
                *data.buffer.lock().unwrap() = Some(buffer);
            }
            frame::Request::DamageBuffer {
                x,
                y,
                width,
                height,
            } => {
                if x < 0 || y < 0 || width <= 0 || height <= 0 {
                    resource.post_error(
                        frame::Error::InvalidBufferDamage,
                        "invalid buffer damage rectangle",
                    );
                }
            }
            frame::Request::Capture => {
                data.captured.store(true, Ordering::Release);
                if data.buffer.lock().unwrap().is_none() {
                    resource.post_error(frame::Error::NoBuffer, "attach a buffer before capture");
                    return;
                }
                state.refresh_image_capture();
                if data.session.stopped.load(Ordering::Acquire) {
                    fail(resource, frame::FailureReason::Stopped);
                    return;
                }
                let descriptor = data.session.descriptor.lock().unwrap().unwrap();
                if !data
                    .buffer
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|b| valid_buffer(b, descriptor.size))
                {
                    fail(resource, frame::FailureReason::BufferConstraints);
                    return;
                }
                if let Some(output) = data.session.output.as_ref().and_then(WeakOutput::upgrade) {
                    state.request_image_capture(&output);
                } else {
                    fail(resource, frame::FailureReason::Stopped);
                }
            }
            _ => {}
        }
    }
    fn destroyed(state: &mut D, _: ClientId, resource: &Frame, data: &Arc<FrameData>) {
        state.image_capture_state().frames.retain(|f| f != resource);
        data.buffer.lock().unwrap().take();
        *data.session.frame.lock().unwrap() = None;
    }
}

fn fail(resource: &Frame, reason: frame::FailureReason) {
    let data = resource.data::<Arc<FrameData>>().unwrap();
    if !data.finished.swap(true, Ordering::AcqRel) {
        resource.failed(reason);
        data.buffer.lock().unwrap().take();
    }
}

pub struct FrameJob {
    resource: Frame,
    pub descriptor: Descriptor,
    pub paint_cursor: bool,
}
impl FrameJob {
    pub fn fail(self) {
        fail(&self.resource, frame::FailureReason::Unknown);
    }
    pub fn complete(self, pixels: &[u8], timestamp: Duration) {
        let data = self.resource.data::<Arc<FrameData>>().unwrap();
        if data.finished.load(Ordering::Acquire) {
            return;
        }
        if data.session.stopped.load(Ordering::Acquire) {
            fail(&self.resource, frame::FailureReason::Stopped);
            return;
        }
        let buffer = data.buffer.lock().unwrap().take();
        let copied = buffer.as_ref().is_some_and(|buffer| {
            buffer.is_alive()
                && shm::with_buffer_contents_mut(buffer, |ptr, len, layout| {
                    if !layout_valid(layout, len, self.descriptor.size)
                        || pixels.len()
                            != self.descriptor.size.w as usize * self.descriptor.size.h as usize * 4
                    {
                        return false;
                    }
                    let row_bytes = self.descriptor.size.w as usize * 4;
                    for row in 0..self.descriptor.size.h as usize {
                        // Bounds checked above; use raw byte copies because clients
                        // may write to their SHM mapping concurrently.
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                pixels.as_ptr().add(row * row_bytes),
                                ptr.add(layout.offset as usize + row * layout.stride as usize),
                                row_bytes,
                            );
                        }
                    }
                    true
                })
                .unwrap_or(false)
        });
        if !copied {
            fail(&self.resource, frame::FailureReason::BufferConstraints);
            return;
        }
        data.finished.store(true, Ordering::Release);
        // The existing renderer produces an upright, already transformed image.
        self.resource.transform(wl_output::Transform::Normal);
        self.resource
            .damage(0, 0, self.descriptor.size.w, self.descriptor.size.h);
        let seconds = timestamp.as_secs();
        self.resource.presentation_time(
            (seconds >> 32) as u32,
            seconds as u32,
            timestamp.subsec_nanos(),
        );
        self.resource.ready();
    }
}
fn valid_buffer(buffer: &WlBuffer, size: Size<i32, Physical>) -> bool {
    buffer.is_alive()
        && shm::with_buffer_contents(buffer, |_, len, layout| layout_valid(layout, len, size))
            .unwrap_or(false)
}
fn layout_valid(data: shm::BufferData, pool_len: usize, size: Size<i32, Physical>) -> bool {
    if size.w <= 0
        || size.h <= 0
        || data.offset < 0
        || data.stride < 0
        || data.width != size.w
        || data.height != size.h
        || data.format != Format::Xrgb8888
    {
        return false;
    }
    let Some(row_bytes) = (size.w as usize).checked_mul(4) else {
        return false;
    };
    if (data.stride as usize) < row_bytes {
        return false;
    }
    (size.h as usize - 1)
        .checked_mul(data.stride as usize)
        .and_then(|n| n.checked_add(data.offset as usize))
        .and_then(|n| n.checked_add(row_bytes))
        .is_some_and(|end| end <= pool_len)
}
