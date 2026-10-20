//! Small real protocol client shared by socket and nested-rendering tests.
#![allow(dead_code)]
use std::{
    collections::HashMap,
    fs::File,
    os::{
        fd::{AsFd, FromRawFd},
        unix::{fs::FileExt, net::UnixStream},
    },
};
use wayland_client::protocol::{
    wl_buffer, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
pub use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1 as source, ext_output_image_capture_source_manager_v1 as sources,
};
pub use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_cursor_session_v1 as cursor, ext_image_copy_capture_frame_v1 as frame,
    ext_image_copy_capture_manager_v1 as manager, ext_image_copy_capture_session_v1 as session,
};
#[derive(Default, Debug)]
pub struct SessionEvents {
    pub sizes: Vec<(u32, u32)>,
    pub formats: Vec<u32>,
    pub done: usize,
    pub stopped: usize,
    pub dma: bool,
}
#[derive(Default, Debug)]
pub struct FrameEvents {
    pub ready: usize,
    pub failures: Vec<u32>,
    pub transforms: Vec<u32>,
    pub damage: Vec<(i32, i32, i32, i32)>,
    pub time: Option<(u32, u32, u32)>,
    pub order: Vec<&'static str>,
}
#[derive(Default)]
pub struct ClientState {
    pub globals: HashMap<String, u32>,
    pub sessions: Vec<SessionEvents>,
    pub frames: Vec<FrameEvents>,
    pub legacy_buffer: Option<(u32, u32, u32)>,
    pub legacy_ready: usize,
}
impl Dispatch<wl_registry::WlRegistry, ()> for ClientState {
    fn event(
        s: &mut Self,
        _: &wl_registry::WlRegistry,
        e: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = e
        {
            s.globals.insert(interface, name);
        }
    }
}
impl Dispatch<session::ExtImageCopyCaptureSessionV1, usize> for ClientState {
    fn event(
        s: &mut Self,
        _: &session::ExtImageCopyCaptureSessionV1,
        e: session::Event,
        id: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let s = &mut s.sessions[*id];
        match e {
            session::Event::BufferSize { width, height } => s.sizes.push((width, height)),
            session::Event::ShmFormat { format } => s.formats.push(format.into()),
            session::Event::Done => s.done += 1,
            session::Event::Stopped => s.stopped += 1,
            session::Event::DmabufDevice { .. } | session::Event::DmabufFormat { .. } => {
                s.dma = true
            }
            _ => {}
        }
    }
}
impl Dispatch<frame::ExtImageCopyCaptureFrameV1, usize> for ClientState {
    fn event(
        s: &mut Self,
        _: &frame::ExtImageCopyCaptureFrameV1,
        e: frame::Event,
        id: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let f = &mut s.frames[*id];
        match e {
            frame::Event::Ready => {
                f.ready += 1;
                f.order.push("ready");
            }
            frame::Event::Failed { reason } => {
                f.failures.push(reason.into());
                f.order.push("failed");
            }
            frame::Event::Transform { transform } => {
                f.transforms.push(transform.into());
                f.order.push("transform");
            }
            frame::Event::Damage {
                x,
                y,
                width,
                height,
            } => {
                f.damage.push((x, y, width, height));
                f.order.push("damage");
            }
            frame::Event::PresentationTime {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                f.time = Some((tv_sec_hi, tv_sec_lo, tv_nsec));
                f.order.push("time");
            }
            _ => {}
        }
    }
}
delegate_noop!(ClientState: ignore manager::ExtImageCopyCaptureManagerV1);
delegate_noop!(ClientState: ignore sources::ExtOutputImageCaptureSourceManagerV1);
delegate_noop!(ClientState: ignore source::ExtImageCaptureSourceV1);
delegate_noop!(ClientState: ignore cursor::ExtImageCopyCaptureCursorSessionV1);
delegate_noop!(ClientState: ignore wl_output::WlOutput);
delegate_noop!(ClientState: ignore wl_shm::WlShm);
delegate_noop!(ClientState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(ClientState: ignore wl_buffer::WlBuffer);
delegate_noop!(ClientState: ignore wl_seat::WlSeat);
delegate_noop!(ClientState: ignore wl_pointer::WlPointer);

pub struct Client {
    pub connection: Connection,
    pub queue: EventQueue<ClientState>,
    pub state: ClientState,
    pub registry: wl_registry::WlRegistry,
}
impl Client {
    pub fn new(socket: UnixStream) -> Self {
        let connection = Connection::from_socket(socket).unwrap();
        let mut queue = connection.new_event_queue();
        let registry = connection.display().get_registry(&queue.handle(), ());
        let mut state = ClientState::default();
        queue.roundtrip(&mut state).unwrap();
        Self {
            connection,
            queue,
            state,
            registry,
        }
    }
    pub fn sync(&mut self) {
        self.queue.roundtrip(&mut self.state).unwrap();
    }
    pub fn manager(&self) -> manager::ExtImageCopyCaptureManagerV1 {
        self.registry.bind(
            self.state.globals["ext_image_copy_capture_manager_v1"],
            1,
            &self.queue.handle(),
            (),
        )
    }
    pub fn source(&self) -> source::ExtImageCaptureSourceV1 {
        let manager: sources::ExtOutputImageCaptureSourceManagerV1 = self.registry.bind(
            self.state.globals["ext_output_image_capture_source_manager_v1"],
            1,
            &self.queue.handle(),
            (),
        );
        let output: wl_output::WlOutput =
            self.registry
                .bind(self.state.globals["wl_output"], 4, &self.queue.handle(), ());
        let source = manager.create_source(&output, &self.queue.handle(), ());
        manager.destroy();
        output.release();
        source
    }
    pub fn session(
        &mut self,
        manager: &manager::ExtImageCopyCaptureManagerV1,
        source: &source::ExtImageCaptureSourceV1,
        cursor: bool,
    ) -> session::ExtImageCopyCaptureSessionV1 {
        let id = self.state.sessions.len();
        self.state.sessions.push(SessionEvents::default());
        manager.create_session(
            source,
            if cursor {
                manager::Options::PaintCursors
            } else {
                manager::Options::empty()
            },
            &self.queue.handle(),
            id,
        )
    }
    pub fn frame(
        &mut self,
        session: &session::ExtImageCopyCaptureSessionV1,
    ) -> frame::ExtImageCopyCaptureFrameV1 {
        let id = self.state.frames.len();
        self.state.frames.push(FrameEvents::default());
        session.create_frame(&self.queue.handle(), id)
    }
    pub fn buffer(
        &self,
        width: i32,
        height: i32,
        stride: i32,
        offset: i32,
        format: wl_shm::Format,
    ) -> (wl_buffer::WlBuffer, File) {
        let size = (offset as usize + height as usize * stride as usize).max(1);
        let name = c"halley-image-copy-test";
        let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
        assert!(fd >= 0);
        let file = unsafe { File::from_raw_fd(fd) };
        file.set_len(size as u64).unwrap();
        file.write_all_at(&vec![0xa5; size], 0).unwrap();
        let shm: wl_shm::WlShm =
            self.registry
                .bind(self.state.globals["wl_shm"], 1, &self.queue.handle(), ());
        let pool = shm.create_pool(file.as_fd(), size as i32, &self.queue.handle(), ());
        let buffer = pool.create_buffer(
            offset,
            width,
            height,
            stride,
            format,
            &self.queue.handle(),
            (),
        );
        pool.destroy();
        (buffer, file)
    }
}

use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1, ext_session_lock_v1,
};
delegate_noop!(ClientState: ignore ext_session_lock_manager_v1::ExtSessionLockManagerV1);
delegate_noop!(ClientState: ignore ext_session_lock_v1::ExtSessionLockV1);

use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1 as legacy_frame, zwlr_screencopy_manager_v1 as legacy_manager,
};
delegate_noop!(ClientState: ignore legacy_manager::ZwlrScreencopyManagerV1);
impl Dispatch<legacy_frame::ZwlrScreencopyFrameV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &legacy_frame::ZwlrScreencopyFrameV1,
        event: legacy_frame::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            legacy_frame::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                assert_eq!(u32::from(format), 1);
                state.legacy_buffer = Some((width, height, stride));
            }
            legacy_frame::Event::Ready { .. } => state.legacy_ready += 1,
            legacy_frame::Event::Failed => panic!("legacy screencopy failed"),
            _ => {}
        }
    }
}

delegate_noop!(ClientState: ignore wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1);
