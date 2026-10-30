//! Basic IME support and Halley lock policy over real Wayland sockets.
#[path = "../src/wayland/input/ime_clients.rs"]
mod ime_clients;
#[path = "../src/wayland/dispatch.rs"]
mod upstream_protocols;
use smithay::{
    input::{Seat, SeatHandler, SeatState, pointer::CursorImageStatus},
    reexports::wayland_server::{self as server, Display, protocol::wl_surface::WlSurface},
    utils::{Logical, Rectangle, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorHandler, CompositorState},
        input_method::{InputMethodHandler, InputMethodManagerState, PopupSurface},
        text_input::TextInputManagerState,
    },
};
use std::{
    collections::HashMap,
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop,
    protocol::{wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_surface},
};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3 as tim, zwp_text_input_v3 as ti,
};
use wayland_protocols_misc::zwp_input_method_v2::client::{
    zwp_input_method_keyboard_grab_v2 as ime_keyboard, zwp_input_method_manager_v2 as imm,
    zwp_input_method_v2 as im, zwp_input_popup_surface_v2 as popup,
};

use smithay::reexports::wayland_protocols::ext::session_lock::v1::server::{
    ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
    ext_session_lock_v1::{ExtSessionLockV1, Request as LockRequest},
};
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::session_lock::{
    LockSurface, SessionLockHandler, SessionLockManagerState, SessionLockState, SessionLocker,
};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1 as lock_manager, ext_session_lock_surface_v1 as lock_surface,
    ext_session_lock_v1 as lock,
};

struct Server {
    popups: Arc<std::sync::Mutex<Vec<PopupSurface>>>,
    compositor: CompositorState,
    lock_state: SessionLockManagerState,
    locked: bool,
    rejected_locks: std::collections::HashSet<server::backend::ObjectId>,
    seats: SeatState<Self>,
    seat: Seat<Self>,
    ime_clients: ime_clients::ImeClients,
}
impl SessionLockHandler for Server {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.lock_state
    }
    fn lock(&mut self, locker: SessionLocker) {
        if self.locked {
            self.rejected_locks.insert(locker.ext_session_lock().id());
        } else {
            self.locked = true;
            locker.lock();
        }
    }
    fn unlock(&mut self) {
        self.locked = false;
    }
    fn new_surface(&mut self, surface: LockSurface, _: server::protocol::wl_output::WlOutput) {
        surface.with_pending_state(|state| state.size = Some((100, 100).into()));
    }
}
impl server::Dispatch<ExtSessionLockV1, SessionLockState> for Server {
    fn request(
        state: &mut Self,
        client: &server::Client,
        lock: &ExtSessionLockV1,
        request: LockRequest,
        data: &SessionLockState,
        display: &server::DisplayHandle,
        init: &mut server::DataInit<'_, Self>,
    ) {
        if state.rejected_locks.contains(&lock.id()) {
            if let LockRequest::GetLockSurface { id, .. } = request {
                init.init(id, RejectedSurface);
            }
        } else {
            smithay::wayland::Dispatch2::request(data, state, client, lock, request, display, init);
        }
    }
}
impl smithay::wayland::output::OutputHandler for Server {}

#[derive(Default)]
struct ClientData(CompositorClientState);
impl server::backend::ClientData for ClientData {
    fn initialized(&self, _: server::backend::ClientId) {}
    fn disconnected(&self, _: server::backend::ClientId, _: server::backend::DisconnectReason) {}
}
impl CompositorHandler for Server {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }
    fn client_compositor_state<'a>(&self, client: &'a server::Client) -> &'a CompositorClientState {
        &client.get_data::<ClientData>().unwrap().0
    }
    fn commit(&mut self, surface: &WlSurface) {
        // Test-only focus control, exercised over the same socket as requests.
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, Some(surface.clone()), SERIAL_COUNTER.next_serial());
    }
}
impl SeatHandler for Server {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seats
    }
    fn focus_changed(&mut self, _: &Seat<Self>, _: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _: &Seat<Self>, _: CursorImageStatus) {}
}
impl smithay::wayland::pointer_constraints::PointerConstraintsHandler for Server {}
impl InputMethodHandler for Server {
    fn new_popup(&mut self, popup: PopupSurface) {
        self.popups.lock().unwrap().push(popup);
    }
    fn dismiss_popup(&mut self, popup: PopupSurface) {
        self.popups.lock().unwrap().retain(|old| old != &popup);
    }
    fn popup_repositioned(&mut self, _: PopupSurface) {}
    fn parent_geometry(&self, _: &WlSurface) -> Rectangle<i32, Logical> {
        Rectangle::default()
    }
}

#[derive(Default)]
struct Client {
    globals: HashMap<String, (u32, u32)>,
    lock_configures: usize,
    lock_finished: usize,
    ime_keys: usize,
    client_keys: usize,
    text: Vec<(u32, ti::Event)>,
    ime: Vec<(u32, im::Event)>,
    popup_rectangles: Vec<(i32, i32, i32, i32)>,
}
impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.globals.insert(interface, (name, version));
        }
    }
}
impl Dispatch<ti::ZwpTextInputV3, ()> for Client {
    fn event(
        state: &mut Self,
        proxy: &ti::ZwpTextInputV3,
        event: ti::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.text.push((proxy.id().protocol_id(), event));
    }
}
impl Dispatch<im::ZwpInputMethodV2, ()> for Client {
    fn event(
        state: &mut Self,
        proxy: &im::ZwpInputMethodV2,
        event: im::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.ime.push((proxy.id().protocol_id(), event));
    }
}
impl Dispatch<popup::ZwpInputPopupSurfaceV2, ()> for Client {
    fn event(
        state: &mut Self,
        _: &popup::ZwpInputPopupSurfaceV2,
        event: popup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let popup::Event::TextInputRectangle {
            x,
            y,
            width,
            height,
        } = event
        {
            state.popup_rectangles.push((x, y, width, height));
        }
    }
}
impl Dispatch<ime_keyboard::ZwpInputMethodKeyboardGrabV2, ()> for Client {
    fn event(
        state: &mut Self,
        _: &ime_keyboard::ZwpInputMethodKeyboardGrabV2,
        event: ime_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, ime_keyboard::Event::Key { .. }) {
            state.ime_keys += 1;
        }
    }
}
impl Dispatch<wl_keyboard::WlKeyboard, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_keyboard::Event::Key { .. }) {
            state.client_keys += 1;
        }
    }
}

impl Dispatch<lock::ExtSessionLockV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &lock::ExtSessionLockV1,
        event: lock::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, lock::Event::Finished) {
            state.lock_finished += 1;
        }
    }
}
impl Dispatch<lock_surface::ExtSessionLockSurfaceV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &lock_surface::ExtSessionLockSurfaceV1,
        event: lock_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, lock_surface::Event::Configure { .. }) {
            state.lock_configures += 1;
        }
    }
}
delegate_noop!(Client: ignore lock_manager::ExtSessionLockManagerV1);
delegate_noop!(Client: ignore wayland_client::protocol::wl_output::WlOutput);

enum Control {
    InsertClient(UnixStream),
    Suspend(bool),
    Key,
}
delegate_noop!(Client: ignore wl_compositor::WlCompositor);
delegate_noop!(Client: ignore wl_surface::WlSurface);
delegate_noop!(Client: ignore wl_seat::WlSeat);
delegate_noop!(Client: ignore tim::ZwpTextInputManagerV3);
delegate_noop!(Client: ignore imm::ZwpInputMethodManagerV2);

#[allow(dead_code)] // Keep proxies alive for the protocol fixture.
struct Fixture {
    popups: Arc<std::sync::Mutex<Vec<PopupSurface>>>,
    state: Client,
    queue: EventQueue<Client>,
    compositor: wl_compositor::WlCompositor,
    lock_manager: lock_manager::ExtSessionLockManagerV1,
    output: wayland_client::protocol::wl_output::WlOutput,
    seat: wl_seat::WlSeat,
    manager: tim::ZwpTextInputManagerV3,
    ime_manager: imm::ZwpInputMethodManagerV2,
    input: ti::ZwpTextInputV3,
    ime: im::ZwpInputMethodV2,
    surface: wl_surface::WlSurface,
    control: std::sync::mpsc::Sender<(Control, std::sync::mpsc::SyncSender<()>)>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Self {
        let (client_socket, server_socket) = UnixStream::pair().unwrap();
        let mut display = Display::<Server>::new().unwrap();
        let mut dh = display.handle();
        let compositor = CompositorState::new::<Server>(&dh);
        let lock_state = SessionLockManagerState::new::<Server, _>(&dh, |_| true);
        let output = smithay::output::Output::new(
            "test".into(),
            smithay::output::PhysicalProperties {
                size: (100, 100).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
                serial_number: "test".into(),
            },
        );
        output.create_global::<Server>(&dh);
        let mut seats = SeatState::new();
        let mut seat = seats.new_wl_seat(&dh, "test");
        seat.add_keyboard(Default::default(), 200, 25).unwrap();
        let _text = TextInputManagerState::new::<Server>(&dh);
        let _ime = InputMethodManagerState::new::<Server, _>(&dh, |_| true);
        dh.insert_client(server_socket, Arc::new(ClientData::default()))
            .unwrap();
        let popups = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut server = Server {
            popups: popups.clone(),
            lock_state,
            locked: false,
            rejected_locks: Default::default(),
            compositor,
            seats,
            seat,
            ime_clients: Default::default(),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (control, controls) =
            std::sync::mpsc::channel::<(Control, std::sync::mpsc::SyncSender<()>)>();
        let worker = thread::spawn(move || {
            let _output = output;
            while !stopped.load(Ordering::Relaxed) {
                display.dispatch_clients(&mut server).unwrap();
                while let Ok((command, done)) = controls.try_recv() {
                    match command {
                        Control::InsertClient(socket) => {
                            dh.insert_client(socket, Arc::new(ClientData::default()))
                                .unwrap();
                        }
                        Control::Suspend(value) => {
                            server.locked = value;
                            if value {
                                server.ime_clients.disconnect_all(&dh);
                                server.seat.get_keyboard().unwrap().unset_grab(&mut server);
                            }
                        }
                        Control::Key => {
                            let keyboard = server.seat.get_keyboard().unwrap();
                            for state in [
                                smithay::backend::input::KeyState::Pressed,
                                smithay::backend::input::KeyState::Released,
                            ] {
                                keyboard.input::<(), _>(
                                    &mut server,
                                    38u32.into(),
                                    state,
                                    SERIAL_COUNTER.next_serial(),
                                    smithay::backend::input::InputTime::from_millis(0),
                                    |_, _, _| smithay::input::keyboard::FilterResult::Forward,
                                );
                            }
                        }
                    }
                    display.flush_clients().unwrap();
                    let _ = done.send(());
                }
                display.flush_clients().unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        });
        let connection = Connection::from_socket(client_socket).unwrap();
        let mut queue = connection.new_event_queue();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let mut state = Client::default();
        queue.roundtrip(&mut state).unwrap();
        let bind = |name: &str| state.globals[name].0;
        let compositor: wl_compositor::WlCompositor =
            registry.bind(bind("wl_compositor"), 4, &qh, ());
        let seat = registry.bind(bind("wl_seat"), 7, &qh, ());
        let manager: tim::ZwpTextInputManagerV3 =
            registry.bind(bind("zwp_text_input_manager_v3"), 1, &qh, ());
        let ime_manager: imm::ZwpInputMethodManagerV2 =
            registry.bind(bind("zwp_input_method_manager_v2"), 1, &qh, ());
        let lock_manager = registry.bind(bind("ext_session_lock_manager_v1"), 1, &qh, ());
        let output = registry.bind(bind("wl_output"), 4, &qh, ());
        let input = manager.get_text_input(&seat, &qh, ());
        let ime = ime_manager.get_input_method(&seat, &qh, ());
        let surface = compositor.create_surface(&qh, ());
        surface.commit();
        queue.roundtrip(&mut state).unwrap();
        let mut fixture = Self {
            popups,
            state,
            queue,
            compositor,
            lock_manager,
            output,
            seat,
            manager,
            ime_manager,
            input,
            ime,
            surface,
            control,
            stop,
            worker: Some(worker),
        };
        fixture.clear();
        fixture
    }
    fn server_command(&self, command: Control) {
        let (done, wait) = std::sync::mpsc::sync_channel(1);
        self.control.send((command, done)).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    fn connect_client(&self) -> (EventQueue<Client>, Client, wl_registry::WlRegistry) {
        let (client, server) = UnixStream::pair().unwrap();
        self.server_command(Control::InsertClient(server));
        let conn = Connection::from_socket(client).unwrap();
        let mut queue = conn.new_event_queue();
        let registry = conn.display().get_registry(&queue.handle(), ());
        let mut state = Client::default();
        queue.roundtrip(&mut state).unwrap();
        (queue, state, registry)
    }
    fn command(&mut self, command: Control) {
        self.sync();
        let (done, wait) = std::sync::mpsc::sync_channel(1);
        self.control.send((command, done)).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        self.sync();
    }
    fn sync(&mut self) {
        self.queue.roundtrip(&mut self.state).unwrap();
    }
    fn clear(&mut self) {
        self.state.text.clear();
        self.state.ime.clear();
    }
    fn enable(&mut self) {
        self.input.enable();
        self.input.commit();
        self.sync();
        self.clear();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn allow_request<I: server::Resource>(
    state: &mut Server,
    client: &server::Client,
    resource: &I,
    request: &I::Request,
    display: &server::DisplayHandle,
) -> bool
where
    I::Request: 'static,
{
    state
        .ime_clients
        .allow_request(state.locked, client, resource, request, display)
}
upstream_protocols::delegate_upstream_protocols!(Server, allow_request);

struct RejectedSurface;
impl server::Dispatch<ExtSessionLockSurfaceV1, RejectedSurface> for Server {
    fn request(
        _: &mut Self,
        _: &server::Client,
        _: &ExtSessionLockSurfaceV1,
        _: <ExtSessionLockSurfaceV1 as server::Resource>::Request,
        _: &RejectedSurface,
        _: &server::DisplayHandle,
        _: &mut server::DataInit<'_, Self>,
    ) {
    }
}

#[test]
fn basic_text_composition_reaches_the_focused_client() {
    let mut f = Fixture::new();
    f.enable();
    f.ime.commit_string("é日本語".into());
    f.ime.commit(0);
    f.sync();
    assert!(f.state.text.iter().any(|(_, event)| matches!(event, ti::Event::CommitString { text: Some(text) } if text == "é日本語")));
    assert!(
        f.state
            .text
            .iter()
            .any(|(_, event)| matches!(event, ti::Event::Done { .. }))
    );
}

#[test]
fn basic_ime_keyboard_grab_receives_keys() {
    let mut f = Fixture::new();
    let _grab = f.ime.grab_keyboard(&f.queue.handle(), ());
    f.enable();
    f.command(Control::Key);
    assert_eq!(f.state.ime_keys, 2);
}

#[test]
fn locking_disconnects_existing_ime_clients() {
    let mut f = Fixture::new();
    let _grab = f.ime.grab_keyboard(&f.queue.handle(), ());
    f.enable();
    let (done, wait) = std::sync::mpsc::sync_channel(1);
    f.control.send((Control::Suspend(true), done)).unwrap();
    wait.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(f.queue.roundtrip(&mut f.state).is_err());
}

#[test]
fn rejected_lock_surface_requests_remain_inert_without_reserving_outputs() {
    let mut f = Fixture::new();
    let owner = f.lock_manager.lock(&f.queue.handle(), ());
    let rejected = f.lock_manager.lock(&f.queue.handle(), ());
    let surface = f.compositor.create_surface(&f.queue.handle(), ());
    let inert = rejected.get_lock_surface(&surface, &f.output, &f.queue.handle(), ());
    f.sync();
    assert_eq!(f.state.lock_finished, 1);
    assert_eq!(f.state.lock_configures, 0);
    rejected.destroy();
    inert.ack_configure(123);
    inert.destroy();
    f.sync();
    let surface = f.compositor.create_surface(&f.queue.handle(), ());
    let _actual = owner.get_lock_surface(&surface, &f.output, &f.queue.handle(), ());
    f.sync();
    assert_eq!(f.state.lock_configures, 1);
}

#[test]
fn locking_keeps_unrelated_clients_connected_and_rejects_new_imes() {
    let mut f = Fixture::new();
    let (mut queue, mut app, registry) = f.connect_client();
    let seat: wl_seat::WlSeat = registry.bind(app.globals["wl_seat"].0, 7, &queue.handle(), ());
    let _keyboard = seat.get_keyboard(&queue.handle(), ());
    let compositor: wl_compositor::WlCompositor =
        registry.bind(app.globals["wl_compositor"].0, 4, &queue.handle(), ());
    let surface = compositor.create_surface(&queue.handle(), ());
    surface.commit();
    queue.roundtrip(&mut app).unwrap();
    f.server_command(Control::Suspend(true));
    f.server_command(Control::Key);
    queue.roundtrip(&mut app).unwrap();
    assert_eq!(
        app.client_keys, 2,
        "lock client must still receive keyboard input"
    );
    assert!(f.queue.roundtrip(&mut f.state).is_err());
    let (mut rejected, mut state, registry) = f.connect_client();
    let seat: wl_seat::WlSeat =
        registry.bind(state.globals["wl_seat"].0, 7, &rejected.handle(), ());
    let manager: imm::ZwpInputMethodManagerV2 = registry.bind(
        state.globals["zwp_input_method_manager_v2"].0,
        1,
        &rejected.handle(),
        (),
    );
    let _ime = manager.get_input_method(&seat, &rejected.handle(), ());
    assert!(rejected.roundtrip(&mut state).is_err());
    queue.roundtrip(&mut app).unwrap();
    f.server_command(Control::Suspend(false));
    let (mut resumed, mut state, registry) = f.connect_client();
    let seat: wl_seat::WlSeat = registry.bind(state.globals["wl_seat"].0, 7, &resumed.handle(), ());
    let manager: imm::ZwpInputMethodManagerV2 = registry.bind(
        state.globals["zwp_input_method_manager_v2"].0,
        1,
        &resumed.handle(),
        (),
    );
    let _ime = manager.get_input_method(&seat, &resumed.handle(), ());
    resumed.roundtrip(&mut state).unwrap();
}
