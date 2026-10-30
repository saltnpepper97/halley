//! Wire tests against the production dispatcher, with deterministic CPU pixels.
#[path = "../src/wayland/capture/ext_image_copy_capture.rs"]
#[allow(dead_code)]
mod capture;
#[path = "support/image_copy_client.rs"]
mod client;
#[path = "../src/wayland/dispatch.rs"]
mod upstream_protocols;
use client::{manager, session};
use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::{
    ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1 as Cursor,
    ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1 as Frame,
    ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1 as Manager,
    ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1 as Session,
};
use smithay::reexports::wayland_server::{Client, Display, backend::ClientData};
use smithay::{
    output::{Output, PhysicalProperties, Subpixel},
    utils::Transform,
    wayland::{
        buffer::BufferHandler,
        image_capture_source::{
            ImageCaptureSource, ImageCaptureSourceHandler, OutputCaptureSourceHandler,
            OutputCaptureSourceState,
        },
        output::OutputHandler,
        shm::{ShmHandler, ShmState},
    },
};
use std::{
    os::unix::{fs::FileExt, net::UnixStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use wayland_client::protocol::wl_shm::Format;
#[derive(Clone)]
struct Control {
    descriptor: Option<capture::Descriptor>,
    complete: bool,
}
struct Server {
    compositor: smithay::wayland::compositor::CompositorState,
    capture: capture::State,
    shm: ShmState,
    output: Output,
    control: Arc<Mutex<Control>>,
}
struct Data(bool, smithay::wayland::compositor::CompositorClientState);
impl ClientData for Data {}
impl BufferHandler for Server {
    fn buffer_destroyed(
        &mut self,
        _: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    ) {
    }
}
impl ShmHandler for Server {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}
impl OutputHandler for Server {}
impl smithay::wayland::compositor::CompositorHandler for Server {
    fn compositor_state(&mut self) -> &mut smithay::wayland::compositor::CompositorState {
        &mut self.compositor
    }
    fn client_compositor_state<'a>(
        &self,
        client: &'a Client,
    ) -> &'a smithay::wayland::compositor::CompositorClientState {
        &client.get_data::<Data>().unwrap().1
    }
    fn commit(&mut self, _: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface) {}
}
impl ImageCaptureSourceHandler for Server {
    fn source_destroyed(&mut self, source: ImageCaptureSource) {
        self.capture.stop_source(&source);
    }
}
impl OutputCaptureSourceHandler for Server {
    fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
        &mut self.capture.output_sources
    }
    fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
        source.user_data().insert_if_missing(|| output.downgrade());
    }
}
impl capture::Handler for Server {
    fn image_capture_state(&mut self) -> &mut capture::State {
        &mut self.capture
    }
    fn refresh_image_capture(&mut self) {
        let d = self.control.lock().unwrap().descriptor;
        self.capture.sync(
            d.map(|d| vec![(self.output.clone(), d)])
                .unwrap_or_default(),
        );
    }
    fn request_image_capture(&mut self, _: &Output) {}
}
upstream_protocols::delegate_upstream_protocols!(Server);
smithay::reexports::wayland_server::delegate_global_dispatch!(Server:[Manager:capture::GlobalData]=>capture::State);
smithay::reexports::wayland_server::delegate_dispatch!(Server:[Manager:()]=>capture::State);
smithay::reexports::wayland_server::delegate_dispatch!(Server:[Session:Arc<capture::SessionData>]=>capture::State);
smithay::reexports::wayland_server::delegate_dispatch!(Server:[Frame:Arc<capture::FrameData>]=>capture::State);
smithay::reexports::wayland_server::delegate_dispatch!(Server:[Cursor:capture::CursorData]=>capture::State);

struct Fixture {
    client: client::Client,
    control: Arc<Mutex<Control>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
fn descriptor() -> capture::Descriptor {
    capture::Descriptor {
        size: (3, 2).into(),
        scale: 1.0,
        transform: Transform::Normal,
    }
}
impl Fixture {
    fn new(authorized: bool) -> Self {
        let (client_socket, server_socket) = UnixStream::pair().unwrap();
        let mut display = Display::<Server>::new().unwrap();
        let mut dh = display.handle();
        let output = Output::new(
            "test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
                serial_number: "test".into(),
            },
        );
        output.create_global::<Server>(&dh);
        let _pixels =
            smithay::wayland::single_pixel_buffer::SinglePixelBufferState::new::<Server>(&dh);
        let compositor = smithay::wayland::compositor::CompositorState::new_v6::<Server>(&dh);
        let capture =
            capture::State::new::<Server>(&dh, |c: &Client| c.get_data::<Data>().unwrap().0);
        let shm = ShmState::new::<Server>(&dh, vec![]);
        dh.insert_client(
            server_socket,
            Arc::new(Data(authorized, Default::default())),
        )
        .unwrap();
        let control = Arc::new(Mutex::new(Control {
            descriptor: Some(descriptor()),
            complete: true,
        }));
        let mut server = Server {
            compositor,
            capture,
            shm,
            output,
            control: control.clone(),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                display.dispatch_clients(&mut server).unwrap();
                capture::Handler::refresh_image_capture(&mut server);
                if server.control.lock().unwrap().complete {
                    for frame in server.capture.take_frames(&server.output) {
                        let value = if frame.paint_cursor { 0x55 } else { 0x33 };
                        let size = frame.descriptor.size;
                        frame.complete(
                            &vec![value; size.w as usize * size.h as usize * 4],
                            Duration::new(10, 123),
                        );
                    }
                }
                display.flush_clients().unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            client: client::Client::new(client_socket),
            control,
            stop,
            worker: Some(worker),
        }
    }
    fn session(
        &mut self,
    ) -> (
        manager::ExtImageCopyCaptureManagerV1,
        client::source::ExtImageCaptureSourceV1,
        session::ExtImageCopyCaptureSessionV1,
    ) {
        let manager = self.client.manager();
        let source = self.client.source();
        let session = self.client.session(&manager, &source, false);
        self.client.sync();
        (manager, source, session)
    }
    fn finish(&mut self, index: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            self.client.sync();
            let f = &self.client.state.frames[index];
            if f.ready + f.failures.len() > 0 {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn error(&mut self, code: u32, interface: &str) {
        assert!((0..4).any(|_| self.client.queue.roundtrip(&mut self.client.state).is_err()));
        let error = self.client.connection.protocol_error().unwrap();
        assert_eq!(error.code, code);
        assert_eq!(error.object_interface, interface);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
fn unauthorized_clients_cannot_see_either_global() {
    let f = Fixture::new(false);
    assert!(
        !f.client
            .state
            .globals
            .contains_key("ext_image_copy_capture_manager_v1")
    );
    assert!(
        !f.client
            .state
            .globals
            .contains_key("ext_output_image_capture_source_manager_v1")
    );
}
#[test]
fn constraints_and_full_frame_copy_preserve_pool_offset_and_padding() {
    let mut f = Fixture::new(true);
    let (_, _, session) = f.session();
    let events = &f.client.state.sessions[0];
    assert_eq!(events.sizes, [(3, 2)]);
    assert_eq!(events.formats, [1]);
    assert_eq!(events.done, 1);
    assert!(!events.dma);
    let (buffer, file) = f.client.buffer(3, 2, 20, 8, Format::Xrgb8888);
    let frame = f.client.frame(&session);
    frame.attach_buffer(&buffer);
    frame.damage_buffer(0, 0, 1, 1);
    frame.capture();
    f.finish(0);
    let mut bytes = vec![0; 48];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes[..8], &[0xa5; 8]);
    assert_eq!(&bytes[8..20], &[0x33; 12]);
    assert_eq!(&bytes[20..28], &[0xa5; 8]);
    assert_eq!(&bytes[28..40], &[0x33; 12]);
    assert_eq!(&bytes[40..], &[0xa5; 8]);
    let events = &f.client.state.frames[0];
    assert_eq!(events.order, ["transform", "damage", "time", "ready"]);
    assert_eq!(events.transforms, [0]);
    assert_eq!(events.damage, [(0, 0, 3, 2)]);
    assert_eq!(events.time, Some((0, 10, 123)));
    assert_eq!(events.ready, 1);
    frame.destroy();
    f.client.sync();
    let next = f.client.frame(&session);
    next.attach_buffer(&buffer);
    next.capture();
    f.finish(1);
    assert_eq!(f.client.state.frames[1].ready, 1);
}
#[test]
fn cursor_option_reaches_the_copy_job() {
    let mut f = Fixture::new(true);
    let manager = f.client.manager();
    let source = f.client.source();
    let session = f.client.session(&manager, &source, true);
    let (buffer, file) = f.client.buffer(3, 2, 12, 0, Format::Xrgb8888);
    let frame = f.client.frame(&session);
    frame.attach_buffer(&buffer);
    frame.capture();
    f.finish(0);
    let mut bytes = [0; 24];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(bytes, [0x55; 24]);
}
#[test]
fn completed_frame_still_blocks_a_second_frame_until_destroyed() {
    let mut f = Fixture::new(true);
    let (_, _, session) = f.session();
    let (buffer, _) = f.client.buffer(3, 2, 12, 0, Format::Xrgb8888);
    let frame = f.client.frame(&session);
    frame.attach_buffer(&buffer);
    frame.capture();
    f.finish(0);
    f.client.frame(&session);
    f.error(1, "ext_image_copy_capture_session_v1");
}
#[test]
fn capture_without_buffer_is_a_protocol_error() {
    let mut f = Fixture::new(true);
    let (_, _, session) = f.session();
    f.client.frame(&session).capture();
    f.error(1, "ext_image_copy_capture_frame_v1");
}
#[test]
fn invalid_buffer_damage_is_a_protocol_error() {
    for rect in [(-1, 0, 1, 1), (0, -1, 1, 1), (0, 0, 0, 1), (0, 0, 1, -1)] {
        let mut f = Fixture::new(true);
        let (_, _, session) = f.session();
        f.client
            .frame(&session)
            .damage_buffer(rect.0, rect.1, rect.2, rect.3);
        f.error(2, "ext_image_copy_capture_frame_v1");
    }
}
#[test]
fn requests_after_capture_are_protocol_errors() {
    for request in 0..3 {
        let mut f = Fixture::new(true);
        let (_, _, session) = f.session();
        let (buffer, _) = f.client.buffer(3, 2, 12, 0, Format::Xrgb8888);
        let frame = f.client.frame(&session);
        frame.attach_buffer(&buffer);
        frame.capture();
        f.finish(0);
        match request {
            0 => frame.capture(),
            1 => frame.attach_buffer(&buffer),
            _ => frame.damage_buffer(0, 0, 1, 1),
        }
        f.error(3, "ext_image_copy_capture_frame_v1");
    }
}
#[test]
fn exact_dimensions_and_advertised_format_are_required() {
    for (w, h, format) in [
        (4, 2, Format::Xrgb8888),
        (3, 3, Format::Xrgb8888),
        (3, 2, Format::Argb8888),
    ] {
        let mut f = Fixture::new(true);
        let (_, _, session) = f.session();
        let (buffer, _) = f.client.buffer(w, h, w * 4, 0, format);
        let frame = f.client.frame(&session);
        frame.attach_buffer(&buffer);
        frame.capture();
        f.finish(0);
        assert_eq!(f.client.state.frames[0].failures, [1]);
        assert_eq!(f.client.state.frames[0].ready, 0);
    }
}
#[test]
fn destroying_manager_and_session_preserves_an_existing_frame() {
    let mut f = Fixture::new(true);
    let (manager, _, session) = f.session();
    let frame = f.client.frame(&session);
    manager.destroy();
    session.destroy();
    let (buffer, _) = f.client.buffer(3, 2, 12, 0, Format::Xrgb8888);
    frame.attach_buffer(&buffer);
    frame.capture();
    f.finish(0);
    assert_eq!(f.client.state.frames[0].ready, 1);
}
#[test]
fn source_destruction_stops_even_an_orphan_frame() {
    let mut f = Fixture::new(true);
    f.control.lock().unwrap().complete = false;
    let (_, source, session) = f.session();
    let frame = f.client.frame(&session);
    let (buffer, file) = f.client.buffer(3, 2, 12, 0, Format::Xrgb8888);
    frame.attach_buffer(&buffer);
    frame.capture();
    session.destroy();
    source.destroy();
    f.finish(0);
    assert_eq!(f.client.state.frames[0].failures, [2]);
    let mut bytes = [0; 24];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(bytes, [0xa5; 24]);
}
#[test]
fn output_loss_stops_pending_copy_and_does_not_revive_session() {
    let mut f = Fixture::new(true);
    f.control.lock().unwrap().complete = false;
    let (_, _, session) = f.session();
    let frame = f.client.frame(&session);
    let (buffer, _) = f.client.buffer(3, 2, 12, 0, Format::Xrgb8888);
    frame.attach_buffer(&buffer);
    frame.capture();
    f.client.sync();
    f.control.lock().unwrap().descriptor = None;
    f.finish(0);
    assert_eq!(f.client.state.frames[0].failures, [2]);
    assert_eq!(f.client.state.sessions[0].stopped, 1);
    f.control.lock().unwrap().descriptor = Some(descriptor());
    frame.destroy();
    f.client.sync();
    let frame = f.client.frame(&session);
    frame.attach_buffer(&buffer);
    frame.capture();
    f.finish(1);
    assert_eq!(f.client.state.frames[1].failures, [2]);
}
#[test]
fn constraint_changes_reject_stale_buffers_and_allow_new_size() {
    let mut f = Fixture::new(true);
    f.control.lock().unwrap().complete = false;
    let (_, _, session) = f.session();
    let (buffer, _) = f.client.buffer(3, 2, 12, 0, Format::Xrgb8888);
    let frame = f.client.frame(&session);
    frame.attach_buffer(&buffer);
    frame.capture();
    f.client.sync();
    let mut d = descriptor();
    d.size = (2, 3).into();
    d.scale = 1.5;
    d.transform = Transform::_90;
    f.control.lock().unwrap().descriptor = Some(d);
    f.control.lock().unwrap().complete = true;
    f.finish(0);
    assert_eq!(f.client.state.frames[0].failures, [1]);
    assert_eq!(f.client.state.sessions[0].sizes.last(), Some(&(2, 3)));
    frame.destroy();
    f.client.sync();
    let (buffer, _) = f.client.buffer(2, 3, 8, 0, Format::Xrgb8888);
    let frame = f.client.frame(&session);
    frame.attach_buffer(&buffer);
    frame.capture();
    f.finish(1);
    assert_eq!(f.client.state.frames[1].ready, 1);
}
#[test]
fn destroyed_buffer_fails_without_accessing_its_pixels() {
    let mut f = Fixture::new(true);
    f.control.lock().unwrap().complete = false;
    let (_, _, session) = f.session();
    let (buffer, _) = f.client.buffer(3, 2, 12, 0, Format::Xrgb8888);
    let frame = f.client.frame(&session);
    frame.attach_buffer(&buffer);
    frame.capture();
    buffer.destroy();
    f.client.sync();
    f.control.lock().unwrap().complete = true;
    f.finish(0);
    assert_eq!(f.client.state.frames[0].failures, [1]);
}
#[test]
fn destroyed_frame_cancels_its_pending_copy() {
    let mut f = Fixture::new(true);
    f.control.lock().unwrap().complete = false;
    let (_, _, session) = f.session();
    let (buffer, file) = f.client.buffer(3, 2, 12, 0, Format::Xrgb8888);
    let frame = f.client.frame(&session);
    frame.attach_buffer(&buffer);
    frame.capture();
    frame.destroy();
    f.client.sync();
    f.control.lock().unwrap().complete = true;
    let frame = f.client.frame(&session);
    frame.attach_buffer(&buffer);
    frame.capture();
    f.finish(1);
    assert_eq!(f.client.state.frames[0].ready, 0);
    let mut bytes = [0; 24];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(bytes, [0x33; 24]);
}

#[test]
fn invalid_options_are_a_protocol_error() {
    let mut f = Fixture::new(true);
    let manager = f.client.manager();
    let source = f.client.source();
    f.client.state.sessions.push(Default::default());
    manager.create_session(
        &source,
        manager::Options::from_bits_retain(2),
        &f.client.queue.handle(),
        0usize,
    );
    f.error(1, "ext_image_copy_capture_manager_v1");
}

#[test]
fn non_shm_buffers_fail_with_buffer_constraints() {
    use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1;
    let mut f = Fixture::new(true);
    let (_, _, session) = f.session();
    let pixels: wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1 =
        f.client.registry.bind(
            f.client.state.globals["wp_single_pixel_buffer_manager_v1"],
            1,
            &f.client.queue.handle(),
            (),
        );
    let buffer = pixels.create_u32_rgba_buffer(0, 0, 0, u32::MAX, &f.client.queue.handle(), ());
    let frame = f.client.frame(&session);
    frame.attach_buffer(&buffer);
    frame.capture();
    f.finish(0);
    assert_eq!(f.client.state.frames[0].failures, [1]);
}
#[test]
fn stopped_session_leaves_uncaptured_frame_quiet_until_capture() {
    let mut f = Fixture::new(true);
    let (_, source, session) = f.session();
    let frame = f.client.frame(&session);
    source.destroy();
    f.client.sync();
    assert!(f.client.state.frames[0].failures.is_empty());
    let (buffer, _) = f.client.buffer(3, 2, 12, 0, Format::Xrgb8888);
    frame.attach_buffer(&buffer);
    frame.capture();
    f.finish(0);
    assert_eq!(f.client.state.frames[0].failures, [2]);
    assert_eq!(f.client.state.sessions[0].stopped, 1);
}
