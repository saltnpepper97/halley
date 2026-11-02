//! Exercise the production reference collection with real upstream surface
//! transactions. Single-pixel buffers isolate reference lifetime from hardware;
//! the sibling fence tests cover delayed GPU completion separately.
use super::*;
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::reexports::wayland_server::{Client, Display};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{CompositorClientState, CompositorHandler, CompositorState};
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_subcompositor, wl_subsurface, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1 as pixels;

struct Server {
    compositor: CompositorState,
    retained: Arc<Mutex<Vec<Buffer>>>,
    explicit: bool,
}
#[derive(Default)]
struct ClientData(CompositorClientState);
impl smithay::reexports::wayland_server::backend::ClientData for ClientData {}
impl CompositorHandler for Server {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientData>().unwrap().0
    }
    fn new_surface(&mut self, surface: &WlSurface) {
        if self.explicit {
            with_states(surface, |states| {
                states
                    .data_map
                    .insert_if_missing(|| UsedExplicitSync(Cell::new(true)));
            });
        }
        add_destruction_hook::<Self, _>(surface, |server, surface| {
            if let Some(buffer) = current_explicit_buffer(surface) {
                server.retained.lock().unwrap().push(buffer);
            }
        });
    }
    fn commit(&mut self, surface: &WlSurface) {
        if !is_sync_subsurface(surface) {
            self.retained
                .lock()
                .unwrap()
                .extend(replaced_buffers(surface));
        }
        on_commit_buffer_handler::<Self>(surface);
    }
}
impl BufferHandler for Server {
    fn buffer_destroyed(
        &mut self,
        _: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    ) {
    }
}
smithay::delegate_dispatch2!(Server);

#[derive(Default)]
struct TestClient {
    globals: HashMap<String, u32>,
    releases: Vec<u32>,
}
impl Dispatch<wl_registry::WlRegistry, ()> for TestClient {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            state.globals.insert(interface, name);
        }
    }
}
impl Dispatch<wl_buffer::WlBuffer, ()> for TestClient {
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            state.releases.push(buffer.id().protocol_id());
        }
    }
}
delegate_noop!(TestClient: ignore wl_compositor::WlCompositor);
delegate_noop!(TestClient: ignore wl_surface::WlSurface);
delegate_noop!(TestClient: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(TestClient: ignore wl_subsurface::WlSubsurface);
delegate_noop!(TestClient: ignore pixels::WpSinglePixelBufferManagerV1);

struct Fixture {
    state: TestClient,
    queue: EventQueue<TestClient>,
    compositor: wl_compositor::WlCompositor,
    subs: wl_subcompositor::WlSubcompositor,
    pixels: pixels::WpSinglePixelBufferManagerV1,
    retained: Arc<Mutex<Vec<Buffer>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(explicit: bool) -> Self {
        let (client, server) = UnixStream::pair().unwrap();
        let mut display = Display::<Server>::new().unwrap();
        let mut handle = display.handle();
        let compositor = CompositorState::new_v6::<Server>(&handle);
        let _pixels = SinglePixelBufferState::new::<Server>(&handle);
        handle
            .insert_client(server, Arc::new(ClientData::default()))
            .unwrap();
        let retained = Arc::new(Mutex::new(Vec::new()));
        let mut server = Server {
            compositor,
            retained: retained.clone(),
            explicit,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                display.dispatch_clients(&mut server).unwrap();
                display.flush_clients().unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        });
        let connection = Connection::from_socket(client).unwrap();
        let mut queue = connection.new_event_queue();
        let registry = connection.display().get_registry(&queue.handle(), ());
        let mut state = TestClient::default();
        queue.roundtrip(&mut state).unwrap();
        let compositor = registry.bind(state.globals["wl_compositor"], 6, &queue.handle(), ());
        let subs = registry.bind(state.globals["wl_subcompositor"], 1, &queue.handle(), ());
        let pixels = registry.bind(
            state.globals["wp_single_pixel_buffer_manager_v1"],
            1,
            &queue.handle(),
            (),
        );
        Self {
            state,
            queue,
            compositor,
            subs,
            pixels,
            retained,
            stop,
            worker: Some(worker),
        }
    }
    fn sync(&mut self) {
        self.queue.roundtrip(&mut self.state).unwrap();
    }
    fn surface(&self) -> wl_surface::WlSurface {
        self.compositor.create_surface(&self.queue.handle(), ())
    }
    fn buffer(&self) -> wl_buffer::WlBuffer {
        self.pixels
            .create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &self.queue.handle(), ())
    }
    fn complete_gpu(&mut self) {
        self.retained.lock().unwrap().clear();
        self.sync();
    }
    fn released(&self, buffer: &wl_buffer::WlBuffer) -> bool {
        self.state.releases.contains(&buffer.id().protocol_id())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
fn replacement_detach_and_destruction_preserve_outgoing_buffer_references() {
    let mut f = Fixture::new(true);
    let surface = f.surface();
    let first = f.buffer();
    surface.attach(Some(&first), 0, 0);
    surface.commit();
    f.sync();
    surface.commit();
    f.sync();
    assert!(
        f.retained.lock().unwrap().is_empty(),
        "metadata-only commit"
    );
    let second = f.buffer();
    surface.attach(Some(&second), 0, 0);
    surface.commit();
    f.sync();
    assert!(!f.released(&first));
    assert_eq!(f.retained.lock().unwrap().len(), 1);
    f.complete_gpu();
    assert!(f.released(&first));
    surface.attach(None, 0, 0);
    surface.commit();
    f.sync();
    assert!(!f.released(&second));
    f.complete_gpu();
    assert!(f.released(&second));
    let third = f.buffer();
    surface.attach(Some(&third), 0, 0);
    surface.commit();
    f.sync();
    surface.destroy();
    f.sync();
    assert!(!f.released(&third));
    f.complete_gpu();
    assert!(f.released(&third));
}

#[test]
fn synchronized_child_is_retained_only_when_the_parent_applies_its_buffer() {
    let mut f = Fixture::new(true);
    let parent = f.surface();
    let child = f.surface();
    let _role = f
        .subs
        .get_subsurface(&child, &parent, &f.queue.handle(), ());
    let first = f.buffer();
    let second = f.buffer();
    child.attach(Some(&first), 0, 0);
    child.commit();
    parent.commit();
    f.sync();
    child.attach(Some(&second), 0, 0);
    child.commit();
    f.sync();
    assert!(f.retained.lock().unwrap().is_empty());
    parent.commit();
    f.sync();
    assert_eq!(f.retained.lock().unwrap().len(), 1);
    assert!(!f.released(&first));
    f.complete_gpu();
    assert!(f.released(&first));
}

#[test]
fn implicit_buffers_do_not_add_release_fences() {
    let mut f = Fixture::new(false);
    let surface = f.surface();
    let first = f.buffer();
    let second = f.buffer();
    surface.attach(Some(&first), 0, 0);
    surface.commit();
    f.sync();
    surface.attach(Some(&second), 0, 0);
    surface.commit();
    f.sync();
    assert!(f.released(&first));
    assert!(f.retained.lock().unwrap().is_empty());
}
