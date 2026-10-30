//! Production protocol dispatch over sockets; the driver never touches hardware.
#[path = "../src/wayland/output/protocols/wlr_output_power.rs"]
mod output_power;
#[path = "../src/wayland/dispatch.rs"]
mod upstream_protocols;

use output_power::{GlobalData, Handler, State};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_server::{Client, Display};
use smithay::utils::Transform;
use smithay::wayland::output::OutputHandler;
use std::os::unix::net::UnixStream;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::{thread, time::Duration};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_wlr::output_power_management::v1::{
    client::{zwlr_output_power_manager_v1 as manager, zwlr_output_power_v1 as power},
    server::{
        zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1 as Manager,
        zwlr_output_power_v1::ZwlrOutputPowerV1 as Power,
    },
};

#[derive(Default)]
struct ClientData {
    approved: bool,
    compositor: smithay::wayland::compositor::CompositorClientState,
}
impl smithay::reexports::wayland_server::backend::ClientData for ClientData {}
struct Server {
    compositor: smithay::wayland::compositor::CompositorState,
    protocol: State,
    outputs: Vec<(Output, Option<bool>)>,
    paused: bool,
    fail: bool,
    calls: usize,
}
impl Handler for Server {
    fn output_power_state(&mut self) -> &mut State {
        &mut self.protocol
    }
    fn refresh_output_power(&mut self) {
        self.protocol.sync(
            self.outputs
                .iter()
                .filter_map(|(output, on)| on.map(|on| (output.clone(), on)))
                .collect(),
        );
    }
    fn set_output_power(&mut self, output: &Output, on: bool) -> Result<(), String> {
        self.calls += 1;
        if self.paused || self.fail {
            return Err("driver unavailable".into());
        }
        let (_, current) = self
            .outputs
            .iter_mut()
            .find(|(known, _)| known == output)
            .unwrap();
        if current.is_none() {
            return Err("unsupported output".into());
        }
        *current = Some(on);
        Ok(())
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
        &client.get_data::<ClientData>().unwrap().compositor
    }
    fn commit(&mut self, _: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface) {}
}
impl OutputHandler for Server {}
upstream_protocols::delegate_upstream_protocols!(Server);
smithay::reexports::wayland_server::delegate_global_dispatch!(Server: [Manager: GlobalData] => State);
smithay::reexports::wayland_server::delegate_dispatch!(Server: [Manager: ()] => State);
smithay::reexports::wayland_server::delegate_dispatch!(Server: [Power: ()] => State);

fn output(name: &str, x: i32) -> Output {
    let output = Output::new(
        name.into(),
        PhysicalProperties {
            size: (300, 200).into(),
            subpixel: Subpixel::Unknown,
            make: "test".into(),
            model: "test".into(),
            serial_number: String::new(),
        },
    );
    let mode = Mode {
        size: (1920, 1080).into(),
        refresh: 60_000,
    };
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Fractional(1.5)),
        Some((x, 0).into()),
    );
    output.set_preferred(mode);
    output
}

enum Command {
    External(usize, bool), // IPC and input wake both publish effective driver state.
    Unavailable(usize),
    Remove(usize),
    Reconnect(usize),
    Pause(bool),
    Failure(bool),
    Calls(mpsc::SyncSender<usize>),
}
struct Fixture {
    queue: EventQueue<Observations>,
    state: Observations,
    registry: wl_registry::WlRegistry,
    manager: Option<manager::ZwlrOutputPowerManagerV1>,
    outputs: Vec<wl_output::WlOutput>,
    hardware: Vec<Output>,
    command: mpsc::Sender<(Command, mpsc::SyncSender<()>)>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(native: bool, approved: bool) -> Self {
        let (client, socket) = UnixStream::pair().unwrap();
        let mut display = Display::<Server>::new().unwrap();
        let mut dh = display.handle();
        dh.insert_client(
            socket,
            Arc::new(ClientData {
                approved,
                ..Default::default()
            }),
        )
        .unwrap();
        let hardware = vec![
            output("DP-1", 0),
            output("DP-2", 1280),
            output("unsupported", 2560),
        ];
        for output in &hardware {
            output.create_global::<Server>(&dh);
        }
        let mut server = Server {
            compositor: smithay::wayland::compositor::CompositorState::new::<Server>(&dh),
            protocol: State::new::<Server>(&dh, native, |client: &Client| {
                client.get_data::<ClientData>().is_some_and(|c| c.approved)
            }),
            outputs: hardware
                .iter()
                .enumerate()
                .map(|(index, output)| (output.clone(), (index < 2).then_some(true)))
                .collect(),
            paused: false,
            fail: false,
            calls: 0,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (command, commands) = mpsc::channel::<(Command, mpsc::SyncSender<()>)>();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                display.dispatch_clients(&mut server).unwrap();
                while let Ok((command, done)) = commands.try_recv() {
                    match command {
                        Command::External(index, on) => server.outputs[index].1 = Some(on),
                        Command::Unavailable(index) => server.outputs[index].1 = None,
                        Command::Remove(index) => {
                            let (output, _) = &mut server.outputs[index];
                            // Remove the driver's identity, not just its name.
                            *output = self::output("removed", 0);
                            server.outputs[index].1 = None;
                        }
                        Command::Reconnect(index) => {
                            let output = self::output(&format!("DP-{}", index + 1), 0);
                            output.create_global::<Server>(&dh);
                            server.outputs[index] = (output, Some(true));
                        }
                        Command::Pause(paused) => server.paused = paused,
                        Command::Failure(fail) => server.fail = fail,
                        Command::Calls(reply) => reply.send(server.calls).unwrap(),
                    }
                    server.refresh_output_power();
                    display.flush_clients().unwrap();
                    done.send(()).unwrap();
                }
                server.refresh_output_power();
                display.flush_clients().unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        });
        let connection = Connection::from_socket(client).unwrap();
        let mut queue = connection.new_event_queue();
        let registry = connection.display().get_registry(&queue.handle(), ());
        let mut state = Observations::default();
        queue.roundtrip(&mut state).unwrap();
        let qh = queue.handle();
        let manager = state
            .globals
            .iter()
            .find(|(_, name)| name == "zwlr_output_power_manager_v1")
            .map(|(id, _)| registry.bind(*id, 1, &qh, ()));
        let outputs = state
            .globals
            .iter()
            .filter(|(_, name)| name == "wl_output")
            .enumerate()
            .map(|(index, (id, _))| registry.bind(*id, 4, &qh, index))
            .collect();
        queue.roundtrip(&mut state).unwrap();
        Self {
            queue,
            state,
            registry,
            manager,
            outputs,
            hardware,
            command,
            stop,
            worker: Some(worker),
        }
    }
    fn sync(&mut self) {
        self.queue.roundtrip(&mut self.state).unwrap();
    }
    fn command(&mut self, command: Command) {
        self.sync();
        let (done, wait) = mpsc::sync_channel(1);
        self.command.send((command, done)).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        self.sync();
    }
    fn power(&mut self, output: usize, tag: usize) -> power::ZwlrOutputPowerV1 {
        let power = self.manager.as_ref().unwrap().get_output_power(
            &self.outputs[output],
            &self.queue.handle(),
            tag,
        );
        self.sync();
        power
    }
    fn calls(&mut self) -> usize {
        let (send, receive) = mpsc::sync_channel(1);
        self.command(Command::Calls(send));
        receive.recv_timeout(Duration::from_secs(5)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}
#[derive(Default)]
struct Observations {
    globals: Vec<(u32, String)>,
    modes: Vec<(usize, power::Mode)>,
    failed: Vec<usize>,
    layouts: Vec<(usize, i32, i32)>,
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
            state.globals.push((name, interface));
        }
    }
}
impl Dispatch<wl_output::WlOutput, usize> for Observations {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        tag: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Geometry { x, y, .. } = event {
            state.layouts.push((*tag, x, y));
        }
    }
}
impl Dispatch<manager::ZwlrOutputPowerManagerV1, ()> for Observations {
    fn event(
        _: &mut Self,
        _: &manager::ZwlrOutputPowerManagerV1,
        _: manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<power::ZwlrOutputPowerV1, usize> for Observations {
    fn event(
        state: &mut Self,
        _: &power::ZwlrOutputPowerV1,
        event: power::Event,
        tag: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            power::Event::Mode {
                mode: wayland_client::WEnum::Value(mode),
            } => state.modes.push((*tag, mode)),
            power::Event::Failed => state.failed.push(*tag),
            _ => panic!("unexpected power event"),
        }
    }
}

#[test]
fn off_on_idempotency_and_multiple_observers_retain_output_geometry() {
    let mut f = Fixture::new(true, true);
    let first = f.power(0, 0);
    let observer = f.power(0, 1);
    let second = f.power(1, 2);
    let layout = f.state.layouts.clone();
    let geometry = f
        .hardware
        .iter()
        .map(|o| {
            (
                o.current_mode(),
                o.current_scale().fractional_scale(),
                o.current_location(),
            )
        })
        .collect::<Vec<_>>();
    first.set_mode(power::Mode::Off);
    f.sync();
    assert_eq!(
        f.state.modes,
        [
            (0, power::Mode::On),
            (1, power::Mode::On),
            (2, power::Mode::On),
            (0, power::Mode::Off),
            (1, power::Mode::Off)
        ]
    );
    first.set_mode(power::Mode::Off);
    f.sync();
    assert_eq!(f.state.modes.len(), 5);
    assert!(f.state.failed.is_empty());
    assert_eq!(f.calls(), 2); // Idempotent writes still consult the lifecycle guard.
    second.set_mode(power::Mode::Off);
    observer.set_mode(power::Mode::On);
    f.sync();
    assert_eq!(
        f.state
            .modes
            .iter()
            .filter(|(tag, _)| *tag == 2)
            .copied()
            .collect::<Vec<_>>(),
        [(2, power::Mode::On), (2, power::Mode::Off)]
    );
    assert_eq!(layout, f.state.layouts);
    assert_eq!(
        geometry,
        f.hardware
            .iter()
            .map(|o| (
                o.current_mode(),
                o.current_scale().fractional_scale(),
                o.current_location()
            ))
            .collect::<Vec<_>>()
    );
    observer.destroy();
    f.sync();
    f.command(Command::External(0, false));
    assert_eq!(f.state.modes.last(), Some(&(0, power::Mode::Off)));
}

#[test]
fn ipc_and_input_wake_notify_existing_objects_even_after_manager_destroy() {
    let mut f = Fixture::new(true, true);
    let power = f.power(0, 0);
    f.manager.take().unwrap().destroy();
    f.sync();
    f.command(Command::External(0, false)); // halleyctl DPMS off
    f.command(Command::External(0, true)); // input wake
    assert_eq!(
        f.state.modes,
        [
            (0, power::Mode::On),
            (0, power::Mode::Off),
            (0, power::Mode::On)
        ]
    );
    power.set_mode(power::Mode::Off);
    f.sync();
    assert_eq!(f.state.modes.last(), Some(&(0, power::Mode::Off)));
}

#[test]
fn unsupported_disabled_removed_and_reconnected_outputs_cannot_revive_old_objects() {
    let mut f = Fixture::new(true, true);
    let unsupported = f.power(2, 2);
    assert_eq!(f.state.failed, [2]);
    unsupported.set_mode(power::Mode::On);
    f.sync();
    assert_eq!(f.calls(), 0);
    let disabled = f.power(0, 0);
    f.command(Command::Unavailable(0));
    assert_eq!(f.state.failed, [2, 0]);
    let late = f.power(0, 3);
    assert_eq!(f.state.failed, [2, 0, 3]);
    f.command(Command::External(0, true));
    disabled.set_mode(power::Mode::Off);
    late.set_mode(power::Mode::Off);
    f.sync();
    assert_eq!(f.calls(), 0);
    let removed = f.power(1, 1);
    f.command(Command::Remove(1));
    f.command(Command::Reconnect(1));
    removed.set_mode(power::Mode::Off);
    f.sync();
    assert_eq!(f.calls(), 0);
    assert_eq!(f.state.failed, [2, 0, 3, 1]);
    // A newly announced output with the same name gets a fresh control.
    let (id, _) = f
        .state
        .globals
        .iter()
        .rev()
        .find(|(_, name)| name == "wl_output")
        .unwrap();
    let output = f
        .registry
        .bind::<wl_output::WlOutput, _, _>(*id, 4, &f.queue.handle(), 4);
    let fresh = f
        .manager
        .as_ref()
        .unwrap()
        .get_output_power(&output, &f.queue.handle(), 4);
    fresh.set_mode(power::Mode::Off);
    f.sync();
    assert_eq!(f.calls(), 1);
    assert_eq!(
        f.state
            .modes
            .iter()
            .filter(|(tag, _)| *tag == 4)
            .copied()
            .collect::<Vec<_>>(),
        [(4, power::Mode::On), (4, power::Mode::Off)]
    );
}

#[test]
fn paused_seat_and_driver_failures_invalidate_only_the_requesting_object() {
    for idempotent in [false, true] {
        let mut f = Fixture::new(true, true);
        let power = f.power(0, 0);
        let _observer = f.power(0, 1);
        f.command(Command::Pause(true)); // Both VT pause and system sleep use this driver guard.
        power.set_mode(if idempotent {
            power::Mode::On
        } else {
            power::Mode::Off
        });
        f.sync();
        assert_eq!(f.state.failed, [0]);
        assert_eq!(f.state.modes, [(0, power::Mode::On), (1, power::Mode::On)]);
        f.command(Command::Pause(false));
        power.set_mode(power::Mode::Off);
        f.sync();
        assert_eq!(f.calls(), 1);
        let failed = f.power(0, 2);
        f.command(Command::Failure(true));
        failed.set_mode(power::Mode::Off);
        f.sync();
        assert_eq!(f.state.failed, [0, 2]);
        f.command(Command::Failure(false));
        let fresh = f.power(0, 3);
        fresh.set_mode(power::Mode::Off);
        f.sync();
        assert!(f.state.modes.contains(&(1, power::Mode::Off)));
    }
}

#[test]
fn invalid_mode_is_a_fatal_protocol_error() {
    let mut f = Fixture::new(true, true);
    let power = f.power(0, 0);
    power
        .send_request(power::Request::SetMode {
            mode: wayland_client::WEnum::Unknown(9),
        })
        .unwrap();
    let error = match f.queue.roundtrip(&mut f.state) {
        Err(wayland_client::DispatchError::Backend(
            wayland_client::backend::WaylandError::Protocol(error),
        )) => error,
        other => panic!("expected invalid-mode protocol error, got {other:?}"),
    };
    assert_eq!(error.code, 1);
    assert_eq!(error.object_interface, "zwlr_output_power_v1");
}

#[test]
fn host_controlled_backend_and_unapproved_clients_have_no_power_global() {
    for (native, approved) in [(false, true), (true, false), (false, false)] {
        let f = Fixture::new(native, approved);
        assert!(f.manager.is_none());
        assert_eq!(f.outputs.len(), 3);
    }
}
