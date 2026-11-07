//! Real DRM acquire points through upstream's protocol and Halley's production
//! readiness helper. These tests only open a render node; they never take KMS
//! ownership or alter the running desktop.
#[path = "../src/wayland/surface/buffer_readiness.rs"]
mod buffer_readiness;
#[path = "../src/wayland/surface/explicit_sync.rs"]
mod explicit_sync;

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Allocator, Buffer as _, Format, Fourcc, Modifier};
use smithay::backend::drm::DrmDeviceFd;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::reexports::drm::control::{Device as _, syncobj};
use smithay::reexports::wayland_server::{
    self as server, Client, Display, Resource, protocol::wl_surface::WlSurface,
};
use smithay::utils::DeviceFd;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, add_pre_commit_hook,
};
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::drm_syncobj::{DrmSyncobjHandler, DrmSyncobjState, supports_syncobj_eventfd};
use wayland_client::protocol::{wl_buffer, wl_compositor, wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1 as params, zwp_linux_dmabuf_v1 as dmabuf,
};
use wayland_protocols::wp::linux_drm_syncobj::v1::client::{
    wp_linux_drm_syncobj_manager_v1 as sync_manager,
    wp_linux_drm_syncobj_surface_v1 as sync_surface,
    wp_linux_drm_syncobj_timeline_v1 as sync_timeline,
};

#[derive(Default)]
struct ClientData(CompositorClientState);
impl server::backend::ClientData for ClientData {}

#[derive(Default)]
struct Observations {
    commits: HashMap<u32, usize>,
    explicit_waits: usize,
    implicit_fallbacks: usize,
    rejected: usize,
}

struct Server {
    compositor: CompositorState,
    sync: Option<DrmSyncobjState>,
    dma: DmabufState,
    handle: calloop::LoopHandle<'static, Self>,
    observations: Arc<Mutex<Observations>>,
    reject_registration: bool,
}

impl CompositorHandler for Server {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientData>().unwrap().0
    }
    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, |state, display, surface| {
            let result = explicit_sync::defer_until_acquired(surface, |client, source| {
                if state.reject_registration {
                    return false;
                }
                let display = display.clone();
                let registered = state
                    .handle
                    .insert_source(source, move |_, _, state| {
                        state
                            .client_compositor_state(&client)
                            .blocker_cleared(state, &display);
                        Ok(())
                    })
                    .is_ok();
                if registered {
                    state.observations.lock().unwrap().explicit_waits += 1;
                }
                registered
            });
            match result {
                Ok(true) => {}
                Ok(false) => state.observations.lock().unwrap().implicit_fallbacks += 1,
                Err(error) => {
                    state.observations.lock().unwrap().rejected += 1;
                    buffer_readiness::reject_commit(surface, error, |client, error| {
                        let display = display.clone();
                        state
                            .handle
                            .insert_idle(move |_| client.kill(&display, error));
                    });
                }
            }
        });
    }
    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        *self
            .observations
            .lock()
            .unwrap()
            .commits
            .entry(surface.id().protocol_id())
            .or_default() += 1;
    }
}
impl BufferHandler for Server {
    fn buffer_destroyed(&mut self, _: &server::protocol::wl_buffer::WlBuffer) {}
}
impl DrmSyncobjHandler for Server {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.sync.as_mut()
    }
}
impl DmabufHandler for Server {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dma
    }
    fn dmabuf_imported(&mut self, _: &DmabufGlobal, _: Dmabuf, notifier: ImportNotifier) {
        // The client exports a real GBM buffer; the regression exercises commit
        // readiness without involving a renderer or physical presentation.
        notifier.successful::<Self>().unwrap();
    }
}
smithay::delegate_dispatch2!(Server);

#[derive(Default)]
struct TestClient(HashMap<String, u32>);
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
            state.0.insert(interface, name);
        }
    }
}
delegate_noop!(TestClient: ignore wl_compositor::WlCompositor);
delegate_noop!(TestClient: ignore wl_surface::WlSurface);
delegate_noop!(TestClient: ignore wl_buffer::WlBuffer);
delegate_noop!(TestClient: ignore dmabuf::ZwpLinuxDmabufV1);
delegate_noop!(TestClient: ignore params::ZwpLinuxBufferParamsV1);
delegate_noop!(TestClient: ignore sync_manager::WpLinuxDrmSyncobjManagerV1);
delegate_noop!(TestClient: ignore sync_surface::WpLinuxDrmSyncobjSurfaceV1);
delegate_noop!(TestClient: ignore sync_timeline::WpLinuxDrmSyncobjTimelineV1);

struct Timeline {
    device: DrmDeviceFd,
    handle: syncobj::Handle,
}
impl Timeline {
    fn new(device: &DrmDeviceFd) -> Self {
        Self {
            device: device.clone(),
            handle: device.create_syncobj(false).unwrap(),
        }
    }
    fn export(&self) -> OwnedFd {
        self.device.syncobj_to_fd(self.handle, false).unwrap()
    }
    fn signaled(&self, point: u64) -> bool {
        self.device
            .syncobj_timeline_wait(&[self.handle], &[point], 0, false, false, false)
            .is_ok()
    }
    fn signal(&self, point: u64) {
        self.device
            .syncobj_timeline_signal(&[self.handle], &[point])
            .unwrap();
    }
}
impl Drop for Timeline {
    fn drop(&mut self) {
        self.device.destroy_syncobj(self.handle).unwrap();
    }
}

fn render_device() -> DrmDeviceFd {
    let path = std::env::var_os("HALLEY_TEST_DRM_RENDER_NODE")
        .expect("set HALLEY_TEST_DRM_RENDER_NODE to the GPU render node under test");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let device = DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(file)));
    assert!(supports_syncobj_eventfd(&device));
    device
}

struct Fixture {
    queue: EventQueue<TestClient>,
    client: TestClient,
    compositor: wl_compositor::WlCompositor,
    manager: sync_manager::WpLinuxDrmSyncobjManagerV1,
    dmabuf: dmabuf::ZwpLinuxDmabufV1,
    observations: Arc<Mutex<Observations>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(device: DrmDeviceFd, reject_registration: bool) -> Self {
        let (client_socket, server_socket) = UnixStream::pair().unwrap();
        let observations = Arc::new(Mutex::new(Observations::default()));
        let observed = observations.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            let mut display = Display::<Server>::new().unwrap();
            let mut dh = display.handle();
            let compositor = CompositorState::new_v6::<Server>(&dh);
            let sync = Some(DrmSyncobjState::new::<Server>(&dh, device));
            let mut dma = DmabufState::new();
            let _global = dma.create_global::<Server>(
                &dh,
                [Format {
                    code: Fourcc::Argb8888,
                    modifier: Modifier::Linear,
                }],
            );
            dh.insert_client(server_socket, Arc::new(ClientData::default()))
                .unwrap();
            let mut event_loop = calloop::EventLoop::<Server>::try_new().unwrap();
            let mut server = Server {
                compositor,
                sync,
                dma,
                handle: event_loop.handle(),
                observations: observed,
                reject_registration,
            };
            while !stopped.load(Ordering::SeqCst) {
                display.dispatch_clients(&mut server).unwrap();
                event_loop
                    .dispatch(Duration::from_millis(1), &mut server)
                    .unwrap();
                display.flush_clients().unwrap();
            }
        });
        let connection = Connection::from_socket(client_socket).unwrap();
        let mut queue = connection.new_event_queue();
        let registry = connection.display().get_registry(&queue.handle(), ());
        let mut client = TestClient::default();
        queue.roundtrip(&mut client).unwrap();
        let qh = queue.handle();
        let compositor = registry.bind(client.0["wl_compositor"], 6, &qh, ());
        let manager = registry.bind(client.0["wp_linux_drm_syncobj_manager_v1"], 1, &qh, ());
        let dmabuf = registry.bind(client.0["zwp_linux_dmabuf_v1"], 3, &qh, ());
        Self {
            queue,
            client,
            compositor,
            manager,
            dmabuf,
            observations,
            stop,
            worker: Some(worker),
        }
    }
    fn sync(&mut self) {
        self.queue.roundtrip(&mut self.client).unwrap();
    }
    fn buffer(&self, device: &DrmDeviceFd) -> wl_buffer::WlBuffer {
        let gbm = GbmDevice::new(device.clone()).unwrap();
        let mut allocator =
            GbmAllocator::new(gbm, GbmBufferFlags::RENDERING | GbmBufferFlags::LINEAR);
        let bo = allocator
            .create_buffer(32, 32, Fourcc::Argb8888, &[Modifier::Linear])
            .unwrap();
        let dma = bo.export().unwrap();
        let qh = self.queue.handle();
        let params = self.dmabuf.create_params(&qh, ());
        for (index, ((fd, offset), stride)) in dma
            .handles()
            .zip(dma.offsets())
            .zip(dma.strides())
            .enumerate()
        {
            let modifier = u64::from(dma.format().modifier);
            params.add(
                fd,
                index as u32,
                offset,
                stride,
                (modifier >> 32) as u32,
                modifier as u32,
            );
        }
        let buffer = params.create_immed(
            32,
            32,
            Fourcc::Argb8888 as u32,
            params::Flags::empty(),
            &qh,
            (),
        );
        params.destroy();
        buffer
    }
    fn commits(&self, surface: &wl_surface::WlSurface) -> usize {
        self.observations
            .lock()
            .unwrap()
            .commits
            .get(&surface.id().protocol_id())
            .copied()
            .unwrap_or_default()
    }
    fn wait_for_commit(&mut self, surface: &wl_surface::WlSurface, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.commits(surface) < expected && Instant::now() < deadline {
            self.sync();
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(self.commits(surface), expected);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
#[ignore = "requires HALLEY_TEST_DRM_RENDER_NODE with GBM and DRM syncobj eventfd"]
fn acquire_wait_survives_syncobj_recreation() {
    let device = render_device();
    for recreations in [0, 1, 3] {
        let acquire = Timeline::new(&device);
        let release = Timeline::new(&device);
        let mut f = Fixture::new(device.clone(), false);
        let qh = f.queue.handle();
        let surface = f.compositor.create_surface(&qh, ());
        let mut sync = f.manager.get_surface(&surface, &qh, ());
        for _ in 0..recreations {
            sync.destroy();
            sync = f.manager.get_surface(&surface, &qh, ());
        }
        let acquire_fd = acquire.export();
        let release_fd = release.export();
        let acquire_client = f.manager.import_timeline(acquire_fd.as_fd(), &qh, ());
        let release_client = f.manager.import_timeline(release_fd.as_fd(), &qh, ());
        let buffer = f.buffer(&device);
        sync.set_acquire_point(&acquire_client, 0, 1);
        sync.set_release_point(&release_client, 0, 1);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        f.sync();
        assert!(!acquire.signaled(1));
        assert_eq!(f.commits(&surface), 0, "recreations={recreations}");
        {
            let observed = f.observations.lock().unwrap();
            assert_eq!(observed.explicit_waits, 1);
            assert_eq!(observed.implicit_fallbacks, 0);
        }
        // The blocked surface must not stall unrelated commits or dispatch.
        let unrelated = f.compositor.create_surface(&qh, ());
        unrelated.commit();
        f.sync();
        assert_eq!(f.commits(&unrelated), 1);
        assert_eq!(f.commits(&surface), 0);
        acquire.signal(1);
        f.wait_for_commit(&surface, 1);

        // Destroying the object must not leave a stale acquire constraint on a
        // later bufferless commit to the same surface.
        sync.destroy();
        surface.commit();
        f.wait_for_commit(&surface, 2);
        assert_eq!(f.observations.lock().unwrap().explicit_waits, 1);
    }
}

#[test]
#[ignore = "requires HALLEY_TEST_DRM_RENDER_NODE with GBM and DRM syncobj eventfd"]
fn acquire_registration_failure_rejects_recreated_syncobj_commit() {
    let device = render_device();
    let acquire = Timeline::new(&device);
    let release = Timeline::new(&device);
    let mut f = Fixture::new(device.clone(), true);
    let qh = f.queue.handle();
    let surface = f.compositor.create_surface(&qh, ());
    f.manager.get_surface(&surface, &qh, ()).destroy();
    let sync = f.manager.get_surface(&surface, &qh, ());
    let acquire_fd = acquire.export();
    let release_fd = release.export();
    let acquire_client = f.manager.import_timeline(acquire_fd.as_fd(), &qh, ());
    let release_client = f.manager.import_timeline(release_fd.as_fd(), &qh, ());
    let buffer = f.buffer(&device);
    sync.set_acquire_point(&acquire_client, 0, 1);
    sync.set_release_point(&release_client, 0, 1);
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    f.queue.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while f.observations.lock().unwrap().rejected == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(f.observations.lock().unwrap().rejected, 1);
    assert_eq!(f.commits(&surface), 0);
    assert!(!acquire.signaled(1));
    assert!(f.queue.roundtrip(&mut f.client).is_err());
}
