//! Exercise the production foreign-toplevel state over real Wayland sockets.
#[path = "../src/wayland/desktop/foreign_toplevel.rs"]
mod foreign_toplevel;
#[path = "../src/wayland/dispatch.rs"]
mod upstream_protocols;

use foreign_toplevel::{Action, HandleData, Handler, Snapshot, State};
use smithay::output::{Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle};
use smithay::wayland::foreign_toplevel_list::{
    ForeignToplevelListHandler, ForeignToplevelListState,
};
use smithay::wayland::output::OutputHandler;
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::{thread, time::Duration};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1 as ext_handle, ext_foreign_toplevel_list_v1 as ext_manager,
};
use wayland_protocols_wlr::foreign_toplevel::v1::{
    client::{
        zwlr_foreign_toplevel_handle_v1 as handle, zwlr_foreign_toplevel_manager_v1 as manager,
    },
    server::{
        zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1 as ServerHandle,
        zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1 as ServerManager,
    },
};

#[derive(Default)]
struct ClientData(smithay::wayland::compositor::CompositorClientState);
impl smithay::reexports::wayland_server::backend::ClientData for ClientData {}
struct Server {
    compositor: smithay::wayland::compositor::CompositorState,
    protocol: State,
    snapshots: Vec<Snapshot>,
    display: DisplayHandle,
    actions: Arc<Mutex<Vec<(u64, &'static str)>>>,
}
impl ForeignToplevelListHandler for Server {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        self.refresh_foreign_toplevels();
        &mut self.protocol.list
    }
}
impl Handler for Server {
    fn refresh_foreign_toplevels(&mut self) {
        self.protocol
            .sync::<Self>(&self.display, self.snapshots.clone());
    }
    fn foreign_toplevel_state(&mut self) -> &mut State {
        &mut self.protocol
    }
    fn foreign_toplevel_action(&mut self, key: u64, action: Action) {
        let name = match action {
            Action::Activate(seat) => {
                let _ = seat;
                "activate"
            }
            Action::Close => "close",
            Action::Minimize(true) => "minimize",
            Action::Minimize(false) => "restore",
            Action::Maximize(true) => "maximize",
            Action::Maximize(false) => "unmaximize",
            Action::Fullscreen(true, output) => {
                let _ = output;
                "fullscreen"
            }
            Action::Fullscreen(false, _) => "unfullscreen",
        };
        self.actions.lock().unwrap().push((key, name));
    }
}
impl smithay::wayland::compositor::CompositorHandler for Server {
    fn compositor_state(&mut self) -> &mut smithay::wayland::compositor::CompositorState {
        &mut self.compositor
    }
    fn client_compositor_state<'a>(
        &self,
        client: &'a Client,
    ) -> &'a smithay::wayland::compositor::CompositorClientState {
        &client.get_data::<ClientData>().unwrap().0
    }
    fn commit(&mut self, _: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface) {}
}
impl OutputHandler for Server {
    fn output_bound(
        &mut self,
        _: Output,
        _: smithay::reexports::wayland_server::protocol::wl_output::WlOutput,
    ) {
        self.protocol.output_bound();
    }
}
upstream_protocols::delegate_upstream_protocols!(Server);
smithay::reexports::wayland_server::delegate_global_dispatch!(Server: [ServerManager: ()] => State);
smithay::reexports::wayland_server::delegate_dispatch!(Server: [ServerManager: ()] => State);
smithay::reexports::wayland_server::delegate_dispatch!(Server: [ServerHandle: HandleData] => State);

fn output(name: &str) -> Output {
    Output::new(
        name.to_owned(),
        PhysicalProperties {
            size: (0, 0).into(),
            serial_number: String::new(),
            subpixel: Subpixel::Unknown,
            make: "test".into(),
            model: "test".into(),
        },
    )
}
fn window(key: u64, name: &str) -> Snapshot {
    Snapshot {
        key,
        title: name.into(),
        app_id: "org.test.App".into(),
        activated: false,
        minimized: false,
        maximized: false,
        fullscreen: false,
        outputs: vec![],
        parent: None,
    }
}
enum Control {
    Replace(Vec<Snapshot>),
    UnmapRemap(u64),
    Attach(mpsc::Sender<UnixStream>),
}
struct Fixture {
    queue: EventQueue<Observations>,
    state: Observations,
    registry: wl_registry::WlRegistry,
    manager: manager::ZwlrForeignToplevelManagerV1,
    ext: ext_manager::ExtForeignToplevelListV1,
    control: mpsc::Sender<(Control, mpsc::SyncSender<()>)>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    actions: Arc<Mutex<Vec<(u64, &'static str)>>>,
    output: Output,
}
impl Fixture {
    fn new(version: u32, snapshots: Vec<Snapshot>) -> Self {
        let (client, connection) = UnixStream::pair().unwrap();
        let mut display = Display::<Server>::new().unwrap();
        let mut dh = display.handle();
        dh.insert_client(connection, Arc::new(ClientData::default()))
            .unwrap();
        let output = output("DP-1");
        output.create_global::<Server>(&dh);
        let actions = Arc::new(Mutex::new(Vec::new()));
        let mut server = Server {
            compositor: smithay::wayland::compositor::CompositorState::new::<Server>(&dh),
            protocol: State::new::<Server>(&dh),
            snapshots,
            display: dh.clone(),
            actions: actions.clone(),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (control, controls) = mpsc::channel::<(Control, mpsc::SyncSender<()>)>();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                display.dispatch_clients(&mut server).unwrap();
                while let Ok((control, done)) = controls.try_recv() {
                    match control {
                        Control::Replace(snapshots) => server.snapshots = snapshots,
                        Control::UnmapRemap(key) => server.protocol.unmap(key),
                        Control::Attach(reply) => {
                            let (client, connection) = UnixStream::pair().unwrap();
                            dh.insert_client(connection, Arc::new(ClientData::default()))
                                .unwrap();
                            reply.send(client).unwrap();
                        }
                    }
                    server.refresh_foreign_toplevels();
                    display.flush_clients().unwrap();
                    done.send(()).unwrap();
                }
                server.refresh_foreign_toplevels();
                display.flush_clients().unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        });
        let (mut queue, registry, mut state) = connect(client);
        let qh = queue.handle();
        let manager = registry.bind(
            state.global("zwlr_foreign_toplevel_manager_v1"),
            version,
            &qh,
            (),
        );
        let ext = registry.bind(state.global("ext_foreign_toplevel_list_v1"), 1, &qh, ());
        queue.roundtrip(&mut state).unwrap();
        Self {
            queue,
            state,
            registry,
            manager,
            ext,
            control,
            stop,
            worker: Some(worker),
            actions,
            output,
        }
    }
    fn sync(&mut self) {
        self.queue.roundtrip(&mut self.state).unwrap();
    }
    fn command(&mut self, command: Control) {
        self.sync();
        let (done, wait) = mpsc::sync_channel(1);
        self.control.send((command, done)).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        self.sync();
    }
    fn replace(&mut self, windows: Vec<Snapshot>) {
        self.command(Control::Replace(windows));
    }
    fn second_client(
        &mut self,
    ) -> (
        EventQueue<Observations>,
        wl_registry::WlRegistry,
        Observations,
    ) {
        let (reply, wait) = mpsc::channel();
        self.command(Control::Attach(reply));
        connect(wait.recv_timeout(Duration::from_secs(5)).unwrap())
    }
    fn handle(&self, name: &str) -> handle::ZwlrForeignToplevelHandleV1 {
        self.state.handle(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn connect(
    socket: UnixStream,
) -> (
    EventQueue<Observations>,
    wl_registry::WlRegistry,
    Observations,
) {
    let connection = Connection::from_socket(socket).unwrap();
    let mut queue = connection.new_event_queue();
    let registry = connection.display().get_registry(&queue.handle(), ());
    let mut state = Observations::default();
    queue.roundtrip(&mut state).unwrap();
    (queue, registry, state)
}
#[derive(Default)]
struct Observations {
    locked: bool,
    windows: HashMap<u32, LiveWindow>,
    states: HashMap<u32, Vec<u32>>,
    globals: HashMap<String, u32>,
    handles: Vec<handle::ZwlrForeignToplevelHandleV1>,
    ext_handles: Vec<ext_handle::ExtForeignToplevelHandleV1>,
    titles: HashMap<u32, String>,
    identifiers: HashMap<u32, String>,
    events: Vec<(u32, String)>,
}
impl Observations {
    fn global(&self, name: &str) -> u32 {
        self.globals[name]
    }
    fn handle(&self, name: &str) -> handle::ZwlrForeignToplevelHandleV1 {
        self.handles
            .iter()
            .rev()
            .find(|h| {
                self.titles
                    .get(&h.id().protocol_id())
                    .is_some_and(|s| s == name)
            })
            .unwrap()
            .clone()
    }
    fn events(&self, id: u32) -> Vec<&str> {
        self.events
            .iter()
            .filter(|(key, _)| *key == id)
            .map(|(_, s)| s.as_str())
            .collect()
    }
}
impl Dispatch<wl_registry::WlRegistry, ()> for Observations {
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
impl Dispatch<manager::ZwlrForeignToplevelManagerV1, ()> for Observations {
    wayland_client::event_created_child!(Observations, manager::ZwlrForeignToplevelManagerV1, [manager::EVT_TOPLEVEL_OPCODE => (handle::ZwlrForeignToplevelHandleV1, ())]);
    fn event(
        state: &mut Self,
        proxy: &manager::ZwlrForeignToplevelManagerV1,
        event: manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            manager::Event::Toplevel { toplevel } => state.handles.push(toplevel),
            manager::Event::Finished => state
                .events
                .push((proxy.id().protocol_id(), "finished".into())),
            _ => {}
        }
    }
}
impl Dispatch<ext_manager::ExtForeignToplevelListV1, ()> for Observations {
    wayland_client::event_created_child!(Observations, ext_manager::ExtForeignToplevelListV1, [ext_manager::EVT_TOPLEVEL_OPCODE => (ext_handle::ExtForeignToplevelHandleV1, ())]);
    fn event(
        state: &mut Self,
        proxy: &ext_manager::ExtForeignToplevelListV1,
        event: ext_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_manager::Event::Toplevel { toplevel } => state.ext_handles.push(toplevel),
            ext_manager::Event::Finished => state
                .events
                .push((proxy.id().protocol_id(), "finished".into())),
            _ => {}
        }
    }
}
impl Dispatch<handle::ZwlrForeignToplevelHandleV1, ()> for Observations {
    fn event(
        state: &mut Self,
        proxy: &handle::ZwlrForeignToplevelHandleV1,
        event: handle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        let event = match event {
            handle::Event::Title { title } => {
                state.titles.insert(id, title.clone());
                format!("title={title}")
            }
            handle::Event::AppId { app_id } => format!("app_id={app_id}"),
            handle::Event::State { state: bytes } => {
                let states: Vec<_> = bytes
                    .chunks_exact(4)
                    .map(|s| u32::from_ne_bytes(s.try_into().unwrap()))
                    .collect();
                state.states.insert(id, states.clone());
                format!("state={states:?}")
            }
            handle::Event::Parent { parent } => {
                format!("parent={:?}", parent.map(|p| p.id().protocol_id()))
            }
            handle::Event::OutputEnter { output } => format!("enter={}", output.id().protocol_id()),
            handle::Event::OutputLeave { output } => format!("leave={}", output.id().protocol_id()),
            handle::Event::Done => "done".into(),
            handle::Event::Closed => "closed".into(),
            _ => return,
        };
        state.events.push((id, event));
    }
}
impl Dispatch<ext_handle::ExtForeignToplevelHandleV1, ()> for Observations {
    fn event(
        state: &mut Self,
        proxy: &ext_handle::ExtForeignToplevelHandleV1,
        event: ext_handle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        let event = match event {
            ext_handle::Event::Identifier { identifier } => {
                state.identifiers.insert(id, identifier.clone());
                format!("identifier={identifier}")
            }
            ext_handle::Event::Title { title } => format!("title={title}"),
            ext_handle::Event::AppId { app_id } => format!("app_id={app_id}"),
            ext_handle::Event::Done => "done".into(),
            ext_handle::Event::Closed => "closed".into(),
            _ => return,
        };
        state.events.push((id, event));
    }
}
wayland_client::delegate_noop!(Observations: ignore wl_output::WlOutput);

#[test]
fn initial_properties_updates_states_and_no_redundant_done() {
    let mut fixture = Fixture::new(3, vec![window(1, "native"), window(2, "X11")]);
    let id = fixture.handle("native").id().protocol_id();
    assert_eq!(
        fixture.state.events(id),
        [
            "title=native",
            "app_id=org.test.App",
            "state=[]",
            "parent=None",
            "done"
        ]
    );
    assert_eq!(fixture.state.ext_handles.len(), 2);
    let identifiers: Vec<_> = fixture.state.identifiers.values().cloned().collect();
    assert_ne!(identifiers[0], identifiers[1]);
    fixture.state.events.clear();
    fixture.sync();
    assert!(fixture.state.events.is_empty());
    let mut changed = window(1, "changed");
    changed.app_id.clear();
    changed.minimized = true;
    changed.maximized = true;
    changed.fullscreen = true;
    fixture.replace(vec![changed.clone(), window(2, "X11")]);
    assert_eq!(
        fixture.state.events(id),
        ["title=changed", "app_id=", "state=[0, 1, 3]", "done"]
    );
    assert_eq!(fixture.handle("changed").id().protocol_id(), id);
    assert_eq!(fixture.state.identifiers.len(), 2);
    fixture.state.events.clear();
    changed.minimized = false;
    changed.activated = true;
    fixture.replace(vec![changed, window(2, "X11")]);
    assert_eq!(fixture.state.events(id), ["state=[0, 2, 3]", "done"]);
}
#[test]
fn parents_outputs_late_output_binding_and_output_identity() {
    let mut child = window(1, "child");
    child.parent = Some(2);
    let mut fixture = Fixture::new(3, vec![child.clone(), window(2, "parent")]);
    let child_id = fixture.handle("child").id().protocol_id();
    let parent_id = fixture.handle("parent").id().protocol_id();
    assert!(
        fixture
            .state
            .events(child_id)
            .contains(&format!("parent=Some({parent_id})").as_str())
    );
    child.outputs = vec![fixture.output.clone()];
    fixture.replace(vec![child.clone(), window(2, "parent")]);
    fixture.state.events.clear();
    let output: wl_output::WlOutput = fixture.registry.bind(
        fixture.state.global("wl_output"),
        4,
        &fixture.queue.handle(),
        (),
    );
    fixture.sync();
    assert_eq!(
        fixture.state.events(child_id),
        [
            format!("enter={}", output.id().protocol_id()),
            "done".into()
        ]
    );
    fixture.state.events.clear();
    child.outputs = vec![self::output("DP-1")]; // Different identity, same name.
    child.parent = None;
    fixture.replace(vec![child]);
    assert_eq!(
        fixture.state.events(child_id),
        [
            format!("leave={}", output.id().protocol_id()),
            "parent=None".into(),
            "done".into()
        ]
    );
    assert_eq!(fixture.state.events(parent_id), ["closed"]);
}
#[test]
fn multiple_bindings_and_clients_have_distinct_handles_and_same_identifiers() {
    let mut fixture = Fixture::new(3, vec![window(1, "one")]);
    let qh = fixture.queue.handle();
    let second: manager::ZwlrForeignToplevelManagerV1 = fixture.registry.bind(
        fixture.state.global("zwlr_foreign_toplevel_manager_v1"),
        3,
        &qh,
        (),
    );
    let second_ext: ext_manager::ExtForeignToplevelListV1 = fixture.registry.bind(
        fixture.state.global("ext_foreign_toplevel_list_v1"),
        1,
        &qh,
        (),
    );
    fixture.sync();
    assert_eq!(fixture.state.handles.len(), 2);
    assert_ne!(fixture.state.handles[0], fixture.state.handles[1]);
    assert_eq!(fixture.state.ext_handles.len(), 2);
    let ids: Vec<_> = fixture.state.identifiers.values().cloned().collect();
    assert_eq!(ids[0], ids[1]);
    let (mut queue, registry, mut state) = fixture.second_client();
    let manager: manager::ZwlrForeignToplevelManagerV1 = registry.bind(
        state.global("zwlr_foreign_toplevel_manager_v1"),
        3,
        &queue.handle(),
        (),
    );
    let ext: ext_manager::ExtForeignToplevelListV1 = registry.bind(
        state.global("ext_foreign_toplevel_list_v1"),
        1,
        &queue.handle(),
        (),
    );
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.handles.len(), 1);
    assert_eq!(state.identifiers.values().next().unwrap(), &ids[0]);
    fixture.replace(vec![]);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        fixture
            .state
            .events(fixture.state.handles[0].id().protocol_id())
            .last(),
        Some(&"closed")
    );
    assert_eq!(
        state.events(state.handles[0].id().protocol_id()).last(),
        Some(&"closed")
    );
    drop((second, second_ext, manager, ext));
}
#[test]
fn destroyed_handles_are_not_recreated_and_parent_destruction_is_silent() {
    let mut child = window(1, "child");
    child.parent = Some(2);
    let mut fixture = Fixture::new(3, vec![child.clone(), window(2, "parent")]);
    let child_id = fixture.handle("child").id().protocol_id();
    fixture.handle("parent").destroy();
    fixture.state.ext_handles[0].destroy();
    fixture.sync();
    fixture.state.events.clear();
    fixture.replace(vec![child, window(2, "renamed")]);
    assert!(fixture.state.events(child_id).is_empty());
    assert_eq!(fixture.state.handles.len(), 2);
    assert_eq!(fixture.state.ext_handles.len(), 2);
}
#[test]
fn unmap_remap_retires_old_handles_and_stale_actions_are_ignored() {
    let mut fixture = Fixture::new(3, vec![window(1, "one")]);
    let old = fixture.handle("one");
    let old_id = old.id().protocol_id();
    let identifier = fixture.state.identifiers.values().next().unwrap().clone();
    old.set_maximized();
    old.unset_maximized();
    old.set_minimized();
    old.unset_minimized();
    old.set_fullscreen(None);
    old.unset_fullscreen();
    old.close();
    fixture.sync();
    assert_eq!(
        *fixture.actions.lock().unwrap(),
        [
            (1, "maximize"),
            (1, "unmaximize"),
            (1, "minimize"),
            (1, "restore"),
            (1, "fullscreen"),
            (1, "unfullscreen"),
            (1, "close")
        ]
    );
    fixture.actions.lock().unwrap().clear();
    fixture.state.events.clear();
    fixture.command(Control::UnmapRemap(1));
    assert_eq!(fixture.state.events(old_id), ["closed"]);
    assert_eq!(fixture.state.handles.len(), 2);
    assert!(fixture.state.identifiers.values().any(|i| i != &identifier));
    old.close();
    old.set_maximized();
    old.set_minimized();
    fixture.sync();
    assert!(fixture.actions.lock().unwrap().is_empty());
    let fresh = fixture.handle("one");
    assert_ne!(fresh.id(), old.id());
    fresh.close();
    fixture.sync();
    assert_eq!(*fixture.actions.lock().unwrap(), [(1, "close")]);
}
#[test]
fn stopped_managers_keep_existing_handles_but_announce_no_new_windows() {
    let mut fixture = Fixture::new(3, vec![window(1, "one")]);
    let id = fixture.handle("one").id().protocol_id();
    fixture.manager.stop();
    fixture.ext.stop();
    fixture.sync();
    assert!(
        fixture
            .state
            .events(fixture.manager.id().protocol_id())
            .contains(&"finished")
    );
    assert!(
        fixture
            .state
            .events(fixture.ext.id().protocol_id())
            .contains(&"finished")
    );
    fixture.state.events.clear();
    fixture.replace(vec![window(1, "renamed"), window(2, "two")]);
    assert_eq!(fixture.state.handles.len(), 1);
    assert_eq!(fixture.state.ext_handles.len(), 1);
    assert_eq!(fixture.state.events(id), ["title=renamed", "done"]);
    fixture.handle("renamed").close();
    fixture.sync();
    assert_eq!(*fixture.actions.lock().unwrap(), [(1, "close")]);
    fixture.replace(vec![]);
    assert_eq!(fixture.state.events(id).last(), Some(&"closed"));
}
#[test]
fn older_versions_receive_only_supported_states_and_events() {
    for version in [1, 2] {
        let mut snapshot = window(1, "one");
        snapshot.fullscreen = true;
        let mut fixture = Fixture::new(version, vec![snapshot]);
        let id = fixture.handle("one").id().protocol_id();
        assert!(fixture.state.events(id).contains(&if version == 1 {
            "state=[]"
        } else {
            "state=[3]"
        }));
        assert!(
            !fixture
                .state
                .events(id)
                .iter()
                .any(|event| event.starts_with("parent="))
        );
        fixture.replace(vec![]);
        assert_eq!(fixture.state.events(id).last(), Some(&"closed"));
    }
}
#[test]
fn old_focus_is_deactivated_before_new_focus_is_activated() {
    let mut first = window(1, "one");
    let mut second = window(2, "two");
    second.activated = true;
    let mut fixture = Fixture::new(3, vec![first.clone(), second.clone()]);
    let first_id = fixture.handle("one").id().protocol_id();
    let second_id = fixture.handle("two").id().protocol_id();
    fixture.state.events.clear();
    first.activated = true;
    second.activated = false;
    fixture.replace(vec![first, second]);
    let states: Vec<_> = fixture
        .state
        .events
        .iter()
        .filter(|(_, event)| event.starts_with("state="))
        .cloned()
        .collect();
    assert_eq!(
        states,
        [
            (second_id, "state=[]".into()),
            (first_id, "state=[2]".into())
        ]
    );
}

// Optional end-to-end check against a separately launched nested Halley. Never
// connects to the ordinary desktop: an explicit test socket is required.
use wayland_client::protocol::{wl_buffer, wl_compositor, wl_seat, wl_surface};
use wayland_protocols::wp::{
    single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1 as pixel,
    viewporter::client::{wp_viewport, wp_viewporter},
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

struct LiveWindow {
    surface: wl_surface::WlSurface,
    xdg: xdg_surface::XdgSurface,
    toplevel: xdg_toplevel::XdgToplevel,
    viewport: wp_viewport::WpViewport,
    buffer: wl_buffer::WlBuffer,
    width: i32,
    height: i32,
    close: bool,
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Observations {
    fn event(
        _: &mut Self,
        proxy: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            proxy.pong(serial);
        }
    }
}
impl Dispatch<xdg_toplevel::XdgToplevel, u32> for Observations {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let window = state.windows.get_mut(id).unwrap();
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                window.width = if width > 0 { width } else { 480 };
                window.height = if height > 0 { height } else { 320 };
            }
            xdg_toplevel::Event::Close => window.close = true,
            _ => {}
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, u32> for Observations {
    fn event(
        state: &mut Self,
        proxy: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            let window = &state.windows[id];
            proxy.ack_configure(serial);
            window.viewport.set_destination(window.width, window.height);
            window.surface.attach(Some(&window.buffer), 0, 0);
            window.surface.damage(0, 0, i32::MAX, i32::MAX);
            window.surface.commit();
        }
    }
}
wayland_client::delegate_noop!(Observations: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(Observations: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(Observations: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(Observations: ignore wl_seat::WlSeat);
wayland_client::delegate_noop!(Observations: ignore pixel::WpSinglePixelBufferManagerV1);
wayland_client::delegate_noop!(Observations: ignore wp_viewporter::WpViewporter);
wayland_client::delegate_noop!(Observations: ignore wp_viewport::WpViewport);
fn live_wait(
    queue: &mut EventQueue<Observations>,
    state: &mut Observations,
    condition: impl Fn(&Observations) -> bool,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        queue.roundtrip(state).unwrap();
        if condition(state) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "nested Halley did not reach requested state: {:?}",
            state.events
        );
        thread::sleep(Duration::from_millis(10));
    }
}
fn live_window(
    queue: &mut EventQueue<Observations>,
    state: &mut Observations,
    registry: &wl_registry::WlRegistry,
    title: &str,
) -> u32 {
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor =
        registry.bind(state.global("wl_compositor"), 4, &qh, ());
    let shell: xdg_wm_base::XdgWmBase = registry.bind(state.global("xdg_wm_base"), 1, &qh, ());
    let pixel: pixel::WpSinglePixelBufferManagerV1 = registry.bind(
        state.global("wp_single_pixel_buffer_manager_v1"),
        1,
        &qh,
        (),
    );
    let viewporter: wp_viewporter::WpViewporter =
        registry.bind(state.global("wp_viewporter"), 1, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let id = surface.id().protocol_id();
    let xdg = shell.get_xdg_surface(&surface, &qh, id);
    let toplevel = xdg.get_toplevel(&qh, id);
    let buffer = pixel.create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    toplevel.set_title(title.into());
    toplevel.set_app_id("org.halley.ForeignTest".into());
    state.windows.insert(
        id,
        LiveWindow {
            surface: surface.clone(),
            xdg,
            toplevel,
            buffer,
            viewport,
            width: 480,
            height: 320,
            close: false,
        },
    );
    surface.commit();
    live_wait(queue, state, |s| {
        s.titles.values().any(|name| name == title)
    });
    id
}
#[test]
#[ignore = "requires an explicitly provided HALLEY_TEST_WAYLAND_DISPLAY nested compositor socket"]
fn nested_halley_native_actions_and_lifecycle() {
    let socket = std::env::var_os("HALLEY_TEST_WAYLAND_DISPLAY")
        .expect("provide the nested test socket explicitly");
    let connection = Connection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
    let mut queue = connection.new_event_queue();
    let mut state = Observations::default();
    let registry = connection.display().get_registry(&queue.handle(), ());
    queue.roundtrip(&mut state).unwrap();
    let _manager: manager::ZwlrForeignToplevelManagerV1 = registry.bind(
        state.global("zwlr_foreign_toplevel_manager_v1"),
        3,
        &queue.handle(),
        (),
    );
    let _ext: ext_manager::ExtForeignToplevelListV1 = registry.bind(
        state.global("ext_foreign_toplevel_list_v1"),
        1,
        &queue.handle(),
        (),
    );
    let seat: wl_seat::WlSeat = registry.bind(state.global("wl_seat"), 1, &queue.handle(), ());
    let first = live_window(&mut queue, &mut state, &registry, "foreign-test-one");
    let second = live_window(&mut queue, &mut state, &registry, "foreign-test-two");
    let handle = state.handle("foreign-test-one");
    let id = handle.id().protocol_id();
    state.windows[&second]
        .toplevel
        .set_parent(Some(&state.windows[&first].toplevel));
    live_wait(&mut queue, &mut state, |s| {
        s.events(s.handle("foreign-test-two").id().protocol_id())
            .contains(&format!("parent=Some({id})").as_str())
    });
    handle.activate(&seat);
    live_wait(&mut queue, &mut state, |s| {
        s.states.get(&id).is_some_and(|states| states.contains(&2))
    });
    handle.set_minimized();
    live_wait(&mut queue, &mut state, |s| {
        s.states.get(&id).is_some_and(|states| states.contains(&1))
    });
    handle.activate(&seat); // Activation itself must restore a collapsed node.
    live_wait(&mut queue, &mut state, |s| {
        s.states
            .get(&id)
            .is_some_and(|states| !states.contains(&1) && states.contains(&2))
    });
    handle.set_minimized();
    live_wait(&mut queue, &mut state, |s| s.states[&id].contains(&1));
    handle.unset_minimized();
    live_wait(&mut queue, &mut state, |s| !s.states[&id].contains(&1));
    handle.set_maximized();
    live_wait(&mut queue, &mut state, |s| s.states[&id].contains(&0));
    handle.set_maximized();
    queue.roundtrip(&mut state).unwrap();
    assert!(state.states[&id].contains(&0));
    handle.unset_maximized();
    live_wait(&mut queue, &mut state, |s| !s.states[&id].contains(&0));
    handle.set_maximized();
    live_wait(&mut queue, &mut state, |s| s.states[&id].contains(&0));
    handle.set_fullscreen(None);
    live_wait(&mut queue, &mut state, |s| s.states[&id].contains(&3));
    handle.unset_fullscreen();
    live_wait(&mut queue, &mut state, |s| !s.states[&id].contains(&3));
    assert!(!state.states[&id].contains(&0));
    // A taskbar cannot change or close windows while the session is locked.
    let lock_manager: lock_manager::ExtSessionLockManagerV1 = registry.bind(
        state.global("ext_session_lock_manager_v1"),
        1,
        &queue.handle(),
        (),
    );
    let lock = lock_manager.lock(&queue.handle(), ());
    live_wait(&mut queue, &mut state, |s| s.locked);
    handle.activate(&seat);
    handle.close();
    handle.set_minimized();
    handle.set_maximized();
    handle.set_fullscreen(None);
    queue.roundtrip(&mut state).unwrap();
    assert!(!state.windows[&first].close);
    assert!(
        !state.states[&id]
            .iter()
            .any(|state| [0, 1, 3].contains(state))
    );
    lock.unlock_and_destroy();
    handle.activate(&seat);
    live_wait(&mut queue, &mut state, |s| s.states[&id].contains(&2));
    // A native null-buffer unmap must close both protocol handles.
    state.windows[&first].surface.attach(None, 0, 0);
    state.windows[&first].surface.commit();
    live_wait(&mut queue, &mut state, |s| s.events(id).contains(&"closed"));
    handle.close(); // The closed handle must not close another native window.
    queue.roundtrip(&mut state).unwrap();
    assert!(!state.windows[&second].close);
    state.windows[&first]
        .toplevel
        .set_title("foreign-test-remapped".into());
    state.windows[&first].surface.commit();
    live_wait(&mut queue, &mut state, |s| {
        s.titles
            .values()
            .any(|title| title == "foreign-test-remapped")
    });
    let remapped = state.handle("foreign-test-remapped");
    assert_ne!(remapped.id(), handle.id());
    handle.close();
    queue.roundtrip(&mut state).unwrap();
    assert!(!state.windows[&first].close);
    remapped.close();
    live_wait(&mut queue, &mut state, |s| s.windows[&first].close);
    let second_handle = state.handle("foreign-test-two");
    second_handle.close();
    live_wait(&mut queue, &mut state, |s| s.windows[&second].close);
    for (_, window) in state.windows.drain() {
        window.toplevel.destroy();
        window.xdg.destroy();
        window.viewport.destroy();
        window.surface.destroy();
        window.buffer.destroy();
    }
    queue.roundtrip(&mut state).unwrap();
}

#[test]
fn rectangle_hints_are_optional_but_negative_dimensions_are_protocol_errors() {
    for invalid in [false, true] {
        let mut fixture = Fixture::new(3, vec![window(1, "one")]);
        let compositor: wl_compositor::WlCompositor = fixture.registry.bind(
            fixture.state.global("wl_compositor"),
            4,
            &fixture.queue.handle(),
            (),
        );
        let surface = compositor.create_surface(&fixture.queue.handle(), ());
        fixture
            .handle("one")
            .set_rectangle(&surface, -10, -20, if invalid { -1 } else { 30 }, 40);
        if invalid {
            assert!(fixture.queue.roundtrip(&mut fixture.state).is_err());
        } else {
            fixture.sync();
            assert!(fixture.actions.lock().unwrap().is_empty());
            fixture.handle("one").set_rectangle(&surface, 0, 0, 0, 0);
            fixture.sync();
        }
    }
}

#[cfg(feature = "xwayland")]
#[test]
#[ignore = "requires HALLEY_TEST_WAYLAND_DISPLAY and HALLEY_TEST_X11_DISPLAY for a nested compositor"]
fn nested_halley_xwayland_actions_and_override_redirect_exclusion() {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ConnectionExt as _, *};
    use x11rb::wrapper::ConnectionExt as _;
    let socket = std::env::var_os("HALLEY_TEST_WAYLAND_DISPLAY").expect("provide nested socket");
    let display = std::env::var("HALLEY_TEST_X11_DISPLAY").expect("provide nested X11 display");
    let connection = Connection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
    let mut queue = connection.new_event_queue();
    let mut state = Observations::default();
    let registry = connection.display().get_registry(&queue.handle(), ());
    queue.roundtrip(&mut state).unwrap();
    let _manager: manager::ZwlrForeignToplevelManagerV1 = registry.bind(
        state.global("zwlr_foreign_toplevel_manager_v1"),
        3,
        &queue.handle(),
        (),
    );
    let _ext: ext_manager::ExtForeignToplevelListV1 = registry.bind(
        state.global("ext_foreign_toplevel_list_v1"),
        1,
        &queue.handle(),
        (),
    );
    let seat: wl_seat::WlSeat = registry.bind(state.global("wl_seat"), 1, &queue.handle(), ());
    let (x11, screen) = x11rb::connect(Some(&display)).unwrap();
    let root = &x11.setup().roots[screen];
    let window = x11.generate_id().unwrap();
    x11.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root.root,
        0,
        0,
        480,
        320,
        0,
        WindowClass::INPUT_OUTPUT,
        root.root_visual,
        &CreateWindowAux::new().background_pixel(root.white_pixel),
    )
    .unwrap()
    .check()
    .unwrap();
    x11.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"foreign-test-x11",
    )
    .unwrap();
    x11.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_CLASS,
        AtomEnum::STRING,
        b"foreign-test\0ForeignTest\0",
    )
    .unwrap();
    let protocols = x11
        .intern_atom(false, b"WM_PROTOCOLS")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let delete = x11
        .intern_atom(false, b"WM_DELETE_WINDOW")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    x11.change_property32(
        PropMode::REPLACE,
        window,
        protocols,
        AtomEnum::ATOM,
        &[delete],
    )
    .unwrap();
    x11.map_window(window).unwrap();
    x11.flush().unwrap();
    live_wait(&mut queue, &mut state, |s| {
        s.titles.values().any(|name| name == "foreign-test-x11")
    });
    let handle = state.handle("foreign-test-x11");
    let id = handle.id().protocol_id();
    assert!(state.events(id).contains(&"app_id=ForeignTest"));
    handle.activate(&seat);
    live_wait(&mut queue, &mut state, |s| s.states[&id].contains(&2));
    handle.set_minimized();
    live_wait(&mut queue, &mut state, |s| s.states[&id].contains(&1));
    handle.activate(&seat);
    live_wait(&mut queue, &mut state, |s| {
        !s.states[&id].contains(&1) && s.states[&id].contains(&2)
    });
    handle.set_maximized();
    live_wait(&mut queue, &mut state, |s| s.states[&id].contains(&0));
    handle.unset_maximized();
    live_wait(&mut queue, &mut state, |s| !s.states[&id].contains(&0));
    handle.set_fullscreen(None);
    live_wait(&mut queue, &mut state, |s| s.states[&id].contains(&3));
    handle.unset_fullscreen();
    live_wait(&mut queue, &mut state, |s| !s.states[&id].contains(&3));
    let menu = x11.generate_id().unwrap();
    x11.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        menu,
        root.root,
        10,
        10,
        20,
        20,
        0,
        WindowClass::INPUT_OUTPUT,
        root.root_visual,
        &CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(root.white_pixel),
    )
    .unwrap()
    .check()
    .unwrap();
    x11.change_property8(
        PropMode::REPLACE,
        menu,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"foreign-test-menu",
    )
    .unwrap();
    x11.map_window(menu).unwrap();
    x11.flush().unwrap();
    // Read-back is a server barrier; give XWayland's Wayland commits a turn too.
    x11.get_window_attributes(menu).unwrap().reply().unwrap();
    for _ in 0..5 {
        queue.roundtrip(&mut state).unwrap();
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !state
            .titles
            .values()
            .any(|name| name == "foreign-test-menu")
    );
    handle.close();
    queue.roundtrip(&mut state).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(x11rb::protocol::Event::ClientMessage(event)) = x11.poll_for_event().unwrap()
            && event.window == window
            && event.type_ == protocols
            && event.data.as_data32()[0] == delete
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "X11 close request not delivered"
        );
        thread::sleep(Duration::from_millis(5));
    }
    x11.destroy_window(window).unwrap();
    x11.destroy_window(menu).unwrap();
    x11.flush().unwrap();
    live_wait(&mut queue, &mut state, |s| s.events(id).contains(&"closed"));
}

use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1 as lock_manager, ext_session_lock_v1 as lock,
};
wayland_client::delegate_noop!(Observations: ignore lock_manager::ExtSessionLockManagerV1);
impl Dispatch<lock::ExtSessionLockV1, ()> for Observations {
    fn event(
        state: &mut Self,
        _: &lock::ExtSessionLockV1,
        event: lock::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let lock::Event::Locked = event {
            state.locked = true;
        }
    }
}
