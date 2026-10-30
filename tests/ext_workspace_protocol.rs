//! Real Wayland request/event tests for Halley's staging `ext-workspace-v1`.
//!
//! The production protocol module is compiled directly into this test with
//! `#[path]`, so these tests exercise the same snapshot diff, request queue, and
//! transaction reducer that the compositor runs - over a real Wayland socket,
//! with no DRM, EGL, renderer, or `Session`.
//!
//! Only the two session-level duties are stubbed: the cluster model (a small
//! table of workspaces per output) and the application of a reduced
//! `TransactionPlan`.

// The production protocol module, compiled directly into this test target. It
// deliberately depends on nothing inside `crate::`, which is what makes this
// possible without adding a library target to the compositor.
#[path = "../src/wayland/desktop/ext_workspace.rs"]
#[allow(dead_code)] // The compositor uses more of this module than these tests do.
mod ext_workspace;

#[path = "../src/wayland/dispatch.rs"]
mod upstream_protocols;

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use smithay::output::{Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_protocols::ext::workspace::v1::server::{
    ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
    ext_workspace_handle_v1::ExtWorkspaceHandleV1,
    ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
};
use smithay::reexports::wayland_server::backend::{
    ClientData, ClientId, DisconnectReason, GlobalId,
};
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::wayland::compositor::{CompositorClientState, CompositorHandler, CompositorState};
use smithay::wayland::output::OutputHandler;

use wayland_client::protocol::{wl_output as cwl_output, wl_registry};
use wayland_client::{
    Connection, Dispatch as ClientDispatch, EventQueue, Proxy, QueueHandle, delegate_noop,
};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1 as cgroup, ext_workspace_handle_v1 as chandle,
    ext_workspace_manager_v1 as cmanager,
};

use halley_core::cluster::ClusterId;

use crate::ext_workspace::{GroupData, TransactionPlan, WorkspaceData};

// ---------------------------------------------------------------------------
// Stub cluster model
// ---------------------------------------------------------------------------

/// One published cluster. Halley's `ClusterSystem` supplies these; here they are
/// plain table rows so a test can mutate the model and observe what the protocol
/// does with it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Workspace {
    output: String,
    id: u64,
    name: String,
    active: bool,
}

#[derive(Default)]
struct Model {
    outputs: Vec<Output>,
    workspaces: Vec<Workspace>,
}

impl Model {
    fn owner_of(&self, cluster: u64) -> Option<String> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.id == cluster)
            .map(|workspace| workspace.output.clone())
    }

    fn active_on(&self, output: &str) -> Option<ClusterId> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.output == output && workspace.active)
            .map(|workspace| ClusterId::new(workspace.id))
    }

    /// The session policy's job: apply the reduced final selection per output.
    fn apply(&mut self, plan: &TransactionPlan) {
        for (output, selection) in &plan.selections {
            for workspace in self
                .workspaces
                .iter_mut()
                .filter(|workspace| workspace.output == *output)
            {
                workspace.active = selection.is_some_and(|id| id.as_u64() == workspace.id);
            }
        }
    }

    fn snapshot(&self) -> ext_workspace::Snapshot {
        ext_workspace::Snapshot {
            groups: self
                .outputs
                .iter()
                .map(|output| {
                    let name = output.name();
                    ext_workspace::GroupSnapshot {
                        output: output.clone(),
                        workspaces: self
                            .workspaces
                            .iter()
                            .filter(|workspace| workspace.output == name)
                            .enumerate()
                            .map(|(index, workspace)| ext_workspace::WorkspaceSnapshot {
                                id: ClusterId::new(workspace.id),
                                name: workspace.name.clone(),
                                order: u8::try_from(index + 1).expect("slot fits in u8"),
                                active: workspace.active,
                            })
                            .collect(),
                    }
                })
                .collect(),
        }
    }
}

fn output(name: &str) -> Output {
    Output::new(
        name.to_string(),
        PhysicalProperties {
            size: (100, 100).into(),
            subpixel: Subpixel::Unknown,
            make: "halley".into(),
            model: "test".into(),
            serial_number: "test".into(),
        },
    )
}

fn workspaces(entries: &[(u64, &str, Option<&str>)]) -> Vec<Workspace> {
    entries
        .iter()
        .map(|(id, output, name)| Workspace {
            output: (*output).to_string(),
            id: *id,
            name: name.unwrap_or("unnamed").to_string(),
            active: false,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

#[derive(Default)]
struct TestClientData(CompositorClientState);

impl ClientData for TestClientData {
    fn initialized(&self, _client_id: ClientId) {}

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

struct Server {
    protocol: ext_workspace::State,
    model: Model,
    /// Reductions the session policy would have applied, recorded so tests can
    /// assert what a transaction resolved to.
    plans: Arc<Mutex<Vec<TransactionPlan>>>,
    /// Mirrors the session policy's refusal to switch while locked.
    locked: bool,
    globals: HashMap<String, GlobalId>,
    compositor: CompositorState,
}

impl CompositorHandler for Server {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<TestClientData>().expect("client data").0
    }

    fn commit(
        &mut self,
        _surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
    }
}

upstream_protocols::delegate_upstream_protocols!(Server);

impl Server {
    fn sync(&mut self, display: &DisplayHandle) {
        let snapshot = self.model.snapshot();
        self.protocol.sync::<Server>(display, &snapshot);
    }
}

impl OutputHandler for Server {
    fn output_bound(&mut self, output: Output, wl_output: WlOutput) {
        if let Some(client) = wl_output.client() {
            self.protocol.output_bound(&client, &output, &wl_output);
        }
    }
}

impl GlobalDispatch<ExtWorkspaceManagerV1, (), Server> for Server {
    fn bind(
        state: &mut Server,
        display: &DisplayHandle,
        client: &Client,
        resource: New<ExtWorkspaceManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Server>,
    ) {
        let snapshot = state.model.snapshot();
        ext_workspace::init_manager::<Server>(
            &mut state.protocol,
            display,
            client,
            resource,
            data_init,
            &snapshot,
        );
    }
}

impl Dispatch<ExtWorkspaceManagerV1, (), Server> for Server {
    fn request(
        state: &mut Server,
        client: &Client,
        manager: &ExtWorkspaceManagerV1,
        request: ext_workspace_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Server>,
    ) {
        let Some(binding) = state.protocol.binding_of(&client.id(), manager) else {
            return;
        };
        let plan = ext_workspace::manager_request(
            &mut state.protocol,
            binding,
            request,
            |cluster| state.model.owner_of(cluster.as_u64()),
            |output| state.model.active_on(output),
        );
        let Some(plan) = plan else {
            return;
        };
        if !state.locked {
            state.model.apply(&plan);
        }
        state.plans.lock().expect("plan lock poisoned").push(plan);
    }

    fn destroyed(
        state: &mut Server,
        client: ClientId,
        manager: &ExtWorkspaceManagerV1,
        _data: &(),
    ) {
        if let Some(binding) = state.protocol.binding_of(&client, manager) {
            state.protocol.remove_binding(binding);
        }
    }
}

impl Dispatch<ExtWorkspaceGroupHandleV1, GroupData> for Server {
    fn request(
        state: &mut Server,
        _client: &Client,
        handle: &ExtWorkspaceGroupHandleV1,
        request: smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_group_handle_v1::Request,
        data: &GroupData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Server>,
    ) {
        ext_workspace::group_request(&mut state.protocol, data.binding, handle, request);
    }

    fn destroyed(
        state: &mut Server,
        _client: ClientId,
        handle: &ExtWorkspaceGroupHandleV1,
        data: &GroupData,
    ) {
        state.protocol.group_destroyed(data.binding, handle);
    }
}

impl Dispatch<ExtWorkspaceHandleV1, WorkspaceData> for Server {
    fn request(
        state: &mut Server,
        _client: &Client,
        handle: &ExtWorkspaceHandleV1,
        request: smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_handle_v1::Request,
        data: &WorkspaceData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Server>,
    ) {
        ext_workspace::workspace_request(
            &mut state.protocol,
            data.binding,
            handle,
            data.cluster,
            request,
        );
    }

    fn destroyed(
        state: &mut Server,
        _client: ClientId,
        handle: &ExtWorkspaceHandleV1,
        data: &WorkspaceData,
    ) {
        state.protocol.workspace_destroyed(data.binding, handle);
    }
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

enum Control {
    Sync,
    SetLocked(bool),
    SetWorkspaces(Vec<Workspace>),
    UnmapOutput(String),
    RemapOutput(String),
    ReplaceOutput(String),
    AttachClient(std::sync::mpsc::SyncSender<UnixStream>),
    Query(std::sync::mpsc::SyncSender<QueryReply>),
}

struct QueryReply {
    active: Vec<(String, Option<u64>)>,
    plans: Vec<TransactionPlan>,
}

struct Fixture {
    state: ClientState,
    queue: EventQueue<ClientState>,
    registry: wl_registry::WlRegistry,
    manager: cmanager::ExtWorkspaceManagerV1,
    /// `wl_output` resources this client has bound, in bind order.
    bound_outputs: Vec<u32>,
    control: std::sync::mpsc::Sender<(Control, std::sync::mpsc::SyncSender<()>)>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        let (client_socket, server_socket) = UnixStream::pair().unwrap();
        let mut display = Display::<Server>::new().unwrap();
        let mut display_handle = display.handle().clone();

        let mut globals = HashMap::new();
        let mut outputs = Vec::new();
        for name in ["DP-1", "DP-2"] {
            let output = output(name);
            globals.insert(
                name.to_string(),
                output.create_global::<Server>(&display_handle),
            );
            outputs.push(output);
        }
        let protocol = ext_workspace::State::new::<Server>(&display_handle);
        let compositor = CompositorState::new::<Server>(&display_handle);
        display_handle
            .insert_client(server_socket, Arc::new(TestClientData::default()))
            .unwrap();

        let plans = Arc::new(Mutex::new(Vec::new()));
        let mut server = Server {
            protocol,
            model: Model {
                outputs,
                workspaces: workspaces(&[
                    (1, "DP-1", Some("web")),
                    (2, "DP-1", Some("code")),
                    (3, "DP-2", Some("chat")),
                ]),
            },
            plans: plans.clone(),
            locked: false,
            globals,
            compositor,
        };

        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (control, controls) =
            std::sync::mpsc::channel::<(Control, std::sync::mpsc::SyncSender<()>)>();
        let worker = thread::spawn(move || {
            let mut handle = display_handle;
            while !stopped.load(Ordering::Relaxed) {
                display.dispatch_clients(&mut server).unwrap();
                while let Ok((command, done)) = controls.try_recv() {
                    match command {
                        Control::Sync => {}
                        Control::SetLocked(value) => server.locked = value,
                        Control::SetWorkspaces(list) => server.model.workspaces = list,
                        Control::UnmapOutput(name) => {
                            if let Some(global) = server.globals.remove(&name) {
                                handle.disable_global::<Server>(global);
                            }
                            server.model.outputs.retain(|output| output.name() != name);
                        }
                        Control::RemapOutput(name) => {
                            // A monitor that comes back is a new `Output` with the
                            // same name: same address, different identity.
                            let output = output(&name);
                            let global = output.create_global::<Server>(&handle);
                            server.globals.insert(name.clone(), global);
                            server.model.outputs.push(output);
                        }
                        Control::ReplaceOutput(name) => {
                            // Replace the output identity without an intermediate
                            // snapshot where the output is absent.
                            if let Some(global) = server.globals.remove(&name) {
                                handle.disable_global::<Server>(global);
                            }
                            server.model.outputs.retain(|output| output.name() != name);
                            let output = output(&name);
                            let global = output.create_global::<Server>(&handle);
                            server.globals.insert(name.clone(), global);
                            server.model.outputs.push(output);
                        }
                        Control::AttachClient(reply) => {
                            let (client, connection) = UnixStream::pair().unwrap();
                            handle
                                .insert_client(connection, Arc::new(TestClientData::default()))
                                .unwrap();
                            let _ = reply.send(client);
                        }
                        Control::Query(reply) => {
                            let active = server
                                .model
                                .outputs
                                .iter()
                                .map(|output| {
                                    let name = output.name();
                                    let selection =
                                        server.model.active_on(&name).map(|id| id.as_u64());
                                    (name, selection)
                                })
                                .collect();
                            let plans = server.plans.lock().unwrap().clone();
                            let _ = reply.send(QueryReply { active, plans });
                        }
                    }
                    // Mirrors Halley's event-loop boundary: publish cluster
                    // changes before clients are flushed.
                    server.sync(&handle);
                    display.flush_clients().unwrap();
                    let _ = done.send(());
                }
                server.sync(&handle);
                display.flush_clients().unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        });

        let connection = Connection::from_socket(client_socket).unwrap();
        let mut queue = connection.new_event_queue();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let mut state = ClientState::default();
        queue.roundtrip(&mut state).unwrap();

        // Bind one wl_output before the manager and one after it, so both the
        // initial advertisement and the late-association path are exercised.
        let first_output: cwl_output::WlOutput = registry.bind(
            state.global_name("wl_output", 0),
            cwl_output::WlOutput::interface().version,
            &qh,
            (),
        );
        let manager: cmanager::ExtWorkspaceManagerV1 =
            registry.bind(state.global_name("ext_workspace_manager_v1", 0), 1, &qh, ());
        let second_output: cwl_output::WlOutput = registry.bind(
            state.global_name("wl_output", 1),
            cwl_output::WlOutput::interface().version,
            &qh,
            (),
        );
        let mut fixture = Self {
            state,
            queue,
            registry,
            manager,
            bound_outputs: vec![
                first_output.id().protocol_id(),
                second_output.id().protocol_id(),
            ],
            control,
            stop,
            worker: Some(worker),
        };
        fixture.sync();
        fixture
    }

    fn sync(&mut self) {
        self.queue.roundtrip(&mut self.state).unwrap();
    }

    /// Runs a server-side change and waits until the boundary synchronisation
    /// and flush have completed.
    fn command(&mut self, command: Control) {
        self.sync();
        let (done, wait) = std::sync::mpsc::sync_channel(1);
        self.control.send((command, done)).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        self.sync();
    }

    fn query(&mut self) -> QueryReply {
        let (reply, wait) = std::sync::mpsc::sync_channel(1);
        self.command(Control::Query(reply));
        wait.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    fn set_workspaces(&mut self, list: &[(u64, &str, Option<&str>)]) {
        self.command(Control::SetWorkspaces(workspaces(list)));
    }

    fn events(&self) -> &[(u32, String)] {
        &self.state.events
    }

    fn clear(&mut self) {
        self.state.events.clear();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ClientState {
    /// `(interface, name, version)` in advertisement order.
    globals: Vec<(String, u32, u32)>,
    /// Every event, in arrival order: `(object protocol id, description)`.
    events: Vec<(u32, String)>,
    /// Workspace handles in creation order.
    workspaces: Vec<chandle::ExtWorkspaceHandleV1>,
    /// `(handle protocol id, name)` for every `name` event.
    workspace_names: Vec<(u32, String)>,
    /// Group handles in creation order.
    groups: Vec<cgroup::ExtWorkspaceGroupHandleV1>,
}

impl ClientState {
    fn global_name(&self, interface: &str, index: usize) -> u32 {
        self.globals
            .iter()
            .filter(|(candidate, _, _)| candidate == interface)
            .nth(index)
            .map(|(_, name, _)| *name)
            .unwrap_or_else(|| panic!("no global {interface}[{index}] in {:?}", self.globals))
    }

    fn descriptions(&self, prefix: &str) -> Vec<String> {
        self.events
            .iter()
            .filter(|(_, event)| event.starts_with(prefix))
            .map(|(_, event)| event.clone())
            .collect()
    }

    /// The live handle for the workspace that last reported this name.
    fn workspace_named(&self, name: &str) -> chandle::ExtWorkspaceHandleV1 {
        let id = self
            .workspace_names
            .iter()
            .find(|(_, candidate)| candidate == name)
            .map(|(id, _)| *id)
            .unwrap_or_else(|| panic!("no workspace named {name}: {:?}", self.workspace_names));
        self.workspaces
            .iter()
            .find(|workspace| workspace.id().protocol_id() == id)
            .cloned()
            .unwrap_or_else(|| panic!("workspace {id} is no longer live"))
    }
}

impl ClientDispatch<wl_registry::WlRegistry, ()> for ClientState {
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
            state.globals.push((interface, name, version));
        }
    }
}

impl ClientDispatch<cmanager::ExtWorkspaceManagerV1, ()> for ClientState {
    // The manager creates its group and workspace children, so the client must
    // say how to initialise their user data.
    wayland_client::event_created_child!(ClientState, cmanager::ExtWorkspaceManagerV1, [
        cmanager::EVT_WORKSPACE_GROUP_OPCODE => (cgroup::ExtWorkspaceGroupHandleV1, ()),
        cmanager::EVT_WORKSPACE_OPCODE => (chandle::ExtWorkspaceHandleV1, ()),
    ]);

    fn event(
        state: &mut Self,
        proxy: &cmanager::ExtWorkspaceManagerV1,
        event: cmanager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        match event {
            cmanager::Event::WorkspaceGroup { workspace_group } => {
                let group = workspace_group.id().protocol_id();
                state.groups.push(workspace_group);
                state.events.push((id, format!("workspace_group={group}")));
            }
            cmanager::Event::Workspace { workspace } => {
                let workspace_id = workspace.id().protocol_id();
                state.workspaces.push(workspace);
                state.events.push((id, format!("workspace={workspace_id}")));
            }
            cmanager::Event::Done => state.events.push((id, "done".to_string())),
            cmanager::Event::Finished => state.events.push((id, "finished".to_string())),
            _ => {}
        }
    }
}

impl ClientDispatch<cgroup::ExtWorkspaceGroupHandleV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        proxy: &cgroup::ExtWorkspaceGroupHandleV1,
        event: cgroup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        let description = match event {
            // Enum-valued payloads are deliberately not decoded here: the unit
            // tests pin the exact bit values, these tests pin ordering and
            // which objects are referenced.
            cgroup::Event::Capabilities { .. } => "group_capabilities".to_string(),
            cgroup::Event::OutputEnter { output } => {
                format!("group_output_enter={}", output.id().protocol_id())
            }
            cgroup::Event::OutputLeave { output } => {
                format!("group_output_leave={}", output.id().protocol_id())
            }
            cgroup::Event::WorkspaceEnter { workspace } => {
                format!("group_workspace_enter={}", workspace.id().protocol_id())
            }
            cgroup::Event::WorkspaceLeave { workspace } => {
                format!("group_workspace_leave={}", workspace.id().protocol_id())
            }
            cgroup::Event::Removed => "group_removed".to_string(),
            _ => return,
        };
        state.events.push((id, description));
    }
}

impl ClientDispatch<chandle::ExtWorkspaceHandleV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        proxy: &chandle::ExtWorkspaceHandleV1,
        event: chandle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        let description = match event {
            chandle::Event::Name { name } => {
                state.workspace_names.push((id, name.clone()));
                format!("name={name}")
            }
            chandle::Event::Coordinates { coordinates } => {
                format!("coordinates={coordinates:?}")
            }
            chandle::Event::Capabilities { .. } => "workspace_capabilities".to_string(),
            chandle::Event::State { .. } => "state".to_string(),
            chandle::Event::Id { .. } => "id".to_string(),
            chandle::Event::Removed => "removed".to_string(),
            _ => return,
        };
        state.events.push((id, description));
    }
}

delegate_noop!(ClientState: ignore cwl_output::WlOutput);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn events_of(fixture: &Fixture, id: u32) -> Vec<String> {
    fixture
        .events()
        .iter()
        .filter(|(object, _)| *object == id)
        .map(|(_, event)| event.clone())
        .collect()
}

/// A second connection to the same display, used to prove that protocol state is
/// per binding rather than global.
struct SecondClient {
    state: ClientState,
    queue: EventQueue<ClientState>,
    manager: cmanager::ExtWorkspaceManagerV1,
}

impl SecondClient {
    fn sync(&mut self) {
        self.queue.roundtrip(&mut self.state).unwrap();
    }
}

impl Fixture {
    fn activate(&mut self, name: &str) {
        self.state.workspace_named(name).activate();
        self.sync();
    }

    fn deactivate(&mut self, name: &str) {
        self.state.workspace_named(name).deactivate();
        self.sync();
    }

    fn commit(&mut self) {
        self.manager.commit();
        self.sync();
    }

    fn attach_client(&mut self) -> SecondClient {
        let (reply, wait) = std::sync::mpsc::sync_channel(1);
        self.sync();
        self.control
            .send((
                Control::AttachClient(reply),
                std::sync::mpsc::sync_channel(1).0,
            ))
            .unwrap();
        let socket = wait.recv_timeout(Duration::from_secs(5)).unwrap();
        let connection = Connection::from_socket(socket).unwrap();
        let mut queue = connection.new_event_queue();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let mut state = ClientState::default();
        queue.roundtrip(&mut state).unwrap();
        let manager: cmanager::ExtWorkspaceManagerV1 =
            registry.bind(state.global_name("ext_workspace_manager_v1", 0), 1, &qh, ());
        let mut client = SecondClient {
            state,
            queue,
            manager,
        };
        client.sync();
        client
    }
}

#[test]
fn activation_changes_nothing_before_commit() {
    let mut f = Fixture::new();
    f.state.workspace_named("web").activate();
    f.sync();

    let reply = f.query();
    assert_eq!(
        reply.active,
        vec![("DP-1".to_string(), None), ("DP-2".to_string(), None)],
        "an uncommitted request must not change the model"
    );
    assert!(
        reply.plans.is_empty(),
        "an uncommitted request must not commit"
    );
    f.clear();

    // Committing applies exactly the queued selection.
    f.commit();
    let reply = f.query();
    assert_eq!(
        reply.active,
        vec![("DP-1".to_string(), Some(1)), ("DP-2".to_string(), None)]
    );
    assert_eq!(reply.plans.len(), 1);
    assert_eq!(
        reply.plans[0].selections,
        vec![("DP-1".to_string(), Some(ClusterId::new(1)))]
    );
}

#[test]
fn a_transaction_reduces_to_its_final_selection() {
    let mut f = Fixture::new();
    f.state.workspace_named("web").activate();
    f.state.workspace_named("code").activate();
    f.manager.commit();
    f.sync();

    let reply = f.query();
    assert_eq!(reply.plans.len(), 1, "one commit is one transaction");
    assert_eq!(
        reply.plans[0].selections,
        vec![("DP-1".to_string(), Some(ClusterId::new(2)))],
        "only the final request for the output is applied"
    );
    assert_eq!(
        reply.active,
        vec![("DP-1".to_string(), Some(2)), ("DP-2".to_string(), None)]
    );
}

#[test]
fn deactivation_clears_the_output_selection() {
    let mut f = Fixture::new();
    f.activate("web");
    f.commit();
    assert_eq!(f.query().active[0], ("DP-1".to_string(), Some(1)));

    f.deactivate("web");
    f.commit();
    let reply = f.query();
    assert_eq!(reply.active[0], ("DP-1".to_string(), None));
    assert_eq!(
        reply.plans.last().unwrap().selections,
        vec![("DP-1".to_string(), None)],
        "deactivating the active workspace clears the output"
    );

    // Repeating the same request is a no-op, not an error and not a toggle.
    let plans_before = f.query().plans.len();
    f.deactivate("web");
    f.commit();
    assert_eq!(
        f.query().plans.len(),
        plans_before,
        "repeating a deactivation queues no new transaction"
    );
}

#[test]
fn unsupported_requests_do_not_mutate_halley() {
    let mut f = Fixture::new();
    // `remove` and `assign` are not advertised, so they must be ignored.
    f.state.workspace_named("web").remove();
    f.state.workspace_named("web").assign(&f.state.groups[0]);
    f.state.groups[0].create_workspace("injected".to_string());
    f.manager.commit();
    f.sync();

    let reply = f.query();
    assert_eq!(
        reply.active,
        vec![("DP-1".to_string(), None), ("DP-2".to_string(), None)]
    );
    assert!(reply.plans.is_empty(), "no supported request was queued");
    assert_eq!(f.state.workspaces.len(), 3, "no workspace was created");
    assert!(
        !f.state
            .workspace_names
            .iter()
            .any(|(_, name)| name == "injected")
    );
}

#[test]
fn external_model_changes_reach_a_connected_client() {
    let mut f = Fixture::new();
    f.clear();

    // An unchanged model sends nothing at the next boundary.
    f.command(Control::Sync);
    assert!(
        f.events().is_empty(),
        "an unchanged snapshot must send nothing: {:?}",
        f.events()
    );

    // Renaming, moving, and adding clusters are reported, without any protocol
    // request having been made.
    f.set_workspaces(&[
        (1, "DP-1", Some("web-renamed")),
        (3, "DP-1", Some("chat")),
        (2, "DP-2", Some("code")),
        (4, "DP-2", Some("new")),
    ]);

    assert!(
        f.state.descriptions("name=web-renamed").len() == 1,
        "a rename is published: {:?}",
        f.events()
    );
    let enters = also_groups(&f);
    assert_eq!(
        f.state.descriptions("workspace=").len(),
        1,
        "only the new cluster gets a new handle: {:?}",
        f.events()
    );
    assert!(
        enters.is_empty(),
        "no group was created or migrated for a name change"
    );
    // `code` and `chat` swap outputs and `new` appears. Each migrated workspace
    // leaves its old group before entering the new one, so a client can never
    // observe it in two groups at once.
    for (name, migrated) in [("web", false), ("code", true), ("chat", true)] {
        let id = f
            .state
            .workspace_names
            .iter()
            .find(|(_, candidate)| candidate == name)
            .map(|(id, _)| *id)
            .unwrap_or_else(|| panic!("{name} is still advertised"));
        let leave = f
            .events()
            .iter()
            .position(|(_, event)| *event == format!("group_workspace_leave={id}"));
        let enter = f
            .events()
            .iter()
            .position(|(_, event)| *event == format!("group_workspace_enter={id}"));
        if migrated {
            let leave = leave.unwrap_or_else(|| panic!("{name} never left: {:?}", f.events()));
            let enter =
                enter.unwrap_or_else(|| panic!("{name} never re-entered: {:?}", f.events()));
            assert!(
                leave < enter,
                "{name} must leave before entering its new group: {:?}",
                f.events()
            );
        } else {
            assert!(
                leave.is_none() && enter.is_none(),
                "{name} did not change group: {:?}",
                f.events()
            );
        }
    }
    assert_eq!(
        f.state.descriptions("group_workspace_enter=").len(),
        3,
        "two migrated clusters and one new cluster enter"
    );
}

fn also_groups(fixture: &Fixture) -> Vec<String> {
    fixture.state.descriptions("workspace_group=")
}

fn index_of(fixture: &Fixture, prefix: &str) -> usize {
    fixture
        .events()
        .iter()
        .position(|(_, event)| event.starts_with(prefix))
        .unwrap_or_else(|| panic!("no event {prefix}: {:?}", fixture.events()))
}

#[test]
fn an_unmapped_output_retires_the_group_then_its_workspaces() {
    let mut f = Fixture::new();
    let chat_before = f
        .state
        .workspace_names
        .iter()
        .find(|(_, name)| name == "chat")
        .map(|(id, _)| *id)
        .expect("chat is advertised");
    f.clear();

    f.command(Control::UnmapOutput("DP-2".to_string()));

    // The protocol requires the group's members to leave before the group is
    // removed, and a workspace handle to be removed only while it belongs to no
    // group. Halley satisfies both by leaving first, then removing.
    let leave = index_of(&f, "group_workspace_leave=");
    let output_leave = index_of(&f, "group_output_leave=");
    let workspace_removed = index_of(&f, "removed");
    let group_removed = index_of(&f, "group_removed");
    assert!(
        leave < group_removed,
        "membership leaves before the group is removed: {:?}",
        f.events()
    );
    assert!(
        output_leave < group_removed,
        "bound outputs leave before the group is removed: {:?}",
        f.events()
    );
    assert!(
        leave < workspace_removed,
        "membership leaves before the workspace handle is removed: {:?}",
        f.events()
    );
    assert_eq!(f.state.descriptions("group_removed").len(), 1);

    // The cluster records are untouched: only the advertisement went away, and
    // reconnecting re-advertises the same named workspace with a fresh handle.
    f.clear();
    f.command(Control::RemapOutput("DP-2".to_string()));
    assert_eq!(f.state.descriptions("group_removed").len(), 0);
    let chat_after = f
        .state
        .workspace_names
        .iter()
        .rev()
        .find(|(_, name)| name == "chat")
        .map(|(id, _)| *id)
        .expect("chat is advertised again");
    assert_ne!(
        chat_before, chat_after,
        "a returned output must not revive stale handles"
    );
    assert!(
        !f.state.descriptions("group_workspace_enter=").is_empty(),
        "the returned workspace is claimed by the new group: {:?}",
        f.events()
    );
}

#[test]
fn replacing_an_output_incarnation_retires_old_handles_before_readvertising() {
    let mut f = Fixture::new();
    let old_chat = f.state.workspace_named("chat");
    let old_id = old_chat.id().protocol_id();

    // Leave a request queued on the old object. Replacing the output without an
    // intermediate absent snapshot must invalidate that request and object.
    old_chat.deactivate();
    f.sync();
    f.clear();
    f.command(Control::ReplaceOutput("DP-2".to_string()));

    let workspace_leave = index_of(&f, "group_workspace_leave=");
    let output_leave = index_of(&f, "group_output_leave=");
    let group_removed = index_of(&f, "group_removed");
    let old_removed = f
        .events()
        .iter()
        .position(|(object, event)| *object == old_id && event == "removed")
        .unwrap_or_else(|| panic!("old workspace was not removed: {:?}", f.events()));
    assert!(workspace_leave < group_removed);
    assert!(output_leave < group_removed);

    let (new_id, new_chat) = f
        .state
        .workspace_names
        .iter()
        .rev()
        .find(|(_, name)| name == "chat")
        .and_then(|(id, _)| {
            f.state
                .workspaces
                .iter()
                .find(|workspace| workspace.id().protocol_id() == *id)
                .cloned()
                .map(|workspace| (*id, workspace))
        })
        .expect("chat is re-advertised");
    assert_ne!(
        old_id, new_id,
        "replacement must use a fresh workspace object"
    );
    let new_advertised = f
        .events()
        .iter()
        .position(|(object, event)| *object == new_id && event == "name=chat")
        .expect("fresh workspace name event");
    assert!(
        old_removed < new_advertised,
        "old workspace retires before its replacement is advertised: {:?}",
        f.events()
    );

    f.commit();
    assert!(
        f.query().plans.is_empty(),
        "requests queued on the retired object must be discarded"
    );

    // Destroying the retired object later must not detach its replacement.
    old_chat.destroy();
    f.sync();
    new_chat.activate();
    f.sync();
    f.commit();
    let reply = f.query();
    assert_eq!(reply.plans.len(), 1);
    assert_eq!(
        reply.plans[0].selections,
        vec![("DP-2".to_string(), Some(ClusterId::new(3)))]
    );
}

#[test]
fn a_destroyed_workspace_handle_is_not_re_advertised() {
    let mut f = Fixture::new();
    let web = f.state.workspace_named("web");
    web.destroy();
    f.sync();
    f.command(Control::Sync);
    f.clear();

    // The model still has the cluster, but the client dropped its handle, so no
    // replacement object may be created for it.
    f.command(Control::Sync);
    assert!(
        f.events().is_empty(),
        "a destroyed handle must stay gone: {:?}",
        f.events()
    );
    // The other two workspaces are unaffected.
    let reply = f.query();
    assert_eq!(reply.active.len(), 2);
    assert_eq!(f.state.workspaces.len(), 3);
}

#[test]
fn an_independent_client_gets_its_own_enumeration() {
    let mut f = Fixture::new();
    let mut other = f.attach_client();

    assert_eq!(other.state.groups.len(), 2);
    assert_eq!(other.state.workspaces.len(), 3);
    assert_eq!(other.state.workspace_names.len(), 3);
    assert!(other.state.events.iter().any(|(_, event)| event == "done"));
    assert_ne!(
        other.manager.id().protocol_id(),
        f.manager.id().protocol_id(),
        "each connection owns its own manager object"
    );

    other.state.events.clear();
    f.activate("web");
    f.commit();
    other.sync();
    assert!(
        other.state.events.iter().any(|(_, event)| event == "state"),
        "the second client is told about the new active workspace: {:?}",
        other.state.events
    );
    assert_eq!(
        f.query().plans.len(),
        1,
        "only the first client's commit was processed"
    );
}

#[test]
fn a_declined_switch_is_reported_but_not_applied() {
    // The lock/grab guard lives in `session::workspace` and cannot be
    // instantiated headlessly, but the split it relies on is observable here:
    // the protocol module still reduces the request and reports the plan, and
    // the policy layer is what declines to apply it. The client sees no error
    // and no state change, which the protocol explicitly permits.
    let mut f = Fixture::new();
    f.command(Control::SetLocked(true));
    f.activate("web");
    f.commit();

    let reply = f.query();
    assert_eq!(reply.plans.len(), 1, "the transaction is still reduced");
    assert_eq!(
        reply.plans[0].selections,
        vec![("DP-1".to_string(), Some(ClusterId::new(1)))]
    );
    assert_eq!(
        reply.active,
        vec![("DP-1".to_string(), None), ("DP-2".to_string(), None)],
        "a declined switch leaves the model alone"
    );

    f.command(Control::SetLocked(false));
    // The declined transaction consumed the queue, so the switch must be
    // requested again rather than replayed.
    f.activate("web");
    f.commit();
    assert_eq!(f.query().active[0], ("DP-1".to_string(), Some(1)));
}

#[test]
fn stop_finishes_the_manager_and_leaves_halley_untouched() {
    let mut f = Fixture::new();
    f.clear();
    f.manager.stop();
    f.sync();

    assert!(
        f.events().iter().any(|(_, event)| event == "finished"),
        "stop is answered with finished: {:?}",
        f.events()
    );
    // The cluster model is unchanged, and the retired binding stops receiving
    // updates.
    f.clear();
    f.command(Control::Sync);
    assert!(
        f.events().is_empty(),
        "a finished manager receives nothing: {:?}",
        f.events()
    );
    let reply = f.query();
    assert_eq!(
        reply.active,
        vec![("DP-1".to_string(), None), ("DP-2".to_string(), None)]
    );
}

#[test]
fn two_bindings_from_one_client_are_independent() {
    let mut f = Fixture::new();
    let second: cmanager::ExtWorkspaceManagerV1 = f.registry.bind(
        f.state.global_name("ext_workspace_manager_v1", 0),
        1,
        &f.queue.handle(),
        (),
    );
    f.clear();
    f.sync();

    assert!(
        f.state.groups.len() >= 4,
        "the second binding gets its own groups: {:?}",
        f.state.events
    );
    assert!(
        f.state.workspaces.len() >= 6,
        "the second binding gets its own workspace handles: {:?}",
        f.state.events
    );

    // Stopping one binding must not disturb the other.
    let groups_before = f.state.groups.len();
    second.stop();
    f.sync();
    f.clear();
    f.command(Control::Sync);
    let reply = f.query();
    assert_eq!(reply.active.len(), 2);
    assert_eq!(
        f.state.groups.len(),
        groups_before,
        "stopping one manager does not retire the other's handles"
    );
}

#[test]
fn initial_bind_advertises_every_group_workspace_and_capability() {
    let f = Fixture::new();
    assert_eq!(f.state.groups.len(), 2, "one group per mapped output");
    assert_eq!(f.state.workspaces.len(), 3, "one handle per cluster");

    // Every group announces its capabilities before anything else it sends.
    for group in &f.state.groups {
        let events = events_of(&f, group.id().protocol_id());
        assert_eq!(
            events.first().map(String::as_str),
            Some("group_capabilities"),
            "group {:?} must advertise capabilities first: {events:?}",
            group.id()
        );
    }

    // Workspaces are ungrouped until `workspace_enter`, and each one is created
    // before the group that claims it.
    for workspace in &f.state.workspaces {
        let events = events_of(&f, workspace.id().protocol_id());
        assert_eq!(
            events.first().map(String::as_str),
            Some("workspace_capabilities")
        );
        assert!(events.iter().any(|event| event.starts_with("name=")));
        assert!(events.iter().any(|event| event.starts_with("coordinates=")));
        assert!(events.contains(&"state".to_string()));
    }
    assert_eq!(
        f.state.descriptions("group_workspace_enter=").len(),
        3,
        "every workspace is claimed by its group"
    );
    assert!(f.state.events.iter().any(|(_, event)| event == "done"));

    let names = f
        .state
        .workspace_names
        .iter()
        .map(|(_, name)| name.clone())
        .collect::<Vec<_>>();
    assert!(names.contains(&"web".to_string()), "{names:?}");
    assert!(names.contains(&"code".to_string()), "{names:?}");
    assert!(names.contains(&"chat".to_string()), "{names:?}");

    // Both bound wl_output objects were associated with their own group, even
    // though one was bound after the manager.
    let associations = f
        .events()
        .iter()
        .filter_map(|(_, event)| event.strip_prefix("group_output_enter="))
        .map(|value| value.parse::<u32>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        associations.len(),
        2,
        "each bound output is associated once: {:?}",
        f.events()
    );
    for bound in &f.bound_outputs {
        assert!(
            associations.contains(bound),
            "wl_output {bound} never received output_enter: {:?}",
            f.events()
        );
    }
}
