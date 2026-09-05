use std::cell::RefCell;
use std::collections::HashSet;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;

use calloop::channel::{Event, Sender, channel};
use calloop::generic::Generic;
use calloop::{Interest, LoopHandle, Mode as CalloopMode, PostAction};
use smithay::output::{Mode as OutputMode, Output};

const MAX_REQUEST_FDS: usize = 32;

/// Backend-agnostic access to real per-output info, mirroring `Renderable`'s
/// existing shape (one small trait, implemented once per backend) rather
/// than threading backend-specific types through the IPC layer.
pub trait OutputInfoSource {
    fn output_info(&self) -> Vec<halley_ipc::OutputInfo>;
}

/// Shared by both backends' `OutputInfoSource` impls - keeps Smithay's
/// millihertz refresh representation intact on the wire.
pub fn mode_info(mode: OutputMode, preferred: bool) -> halley_ipc::ModeInfo {
    halley_ipc::ModeInfo {
        width: mode.size.w,
        height: mode.size.h,
        refresh_millihz: mode.refresh,
        preferred,
    }
}

pub fn vrr_str(vrr: halley_config::Vrr) -> &'static str {
    match vrr {
        halley_config::Vrr::Off => "off",
        halley_config::Vrr::On => "on",
        halley_config::Vrr::Auto => "auto",
    }
}

fn version_info() -> halley_ipc::VersionInfo {
    halley_ipc::VersionInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        ipc_protocol: halley_ipc::HALLEY_IPC_VERSION,
    }
}

struct ReplyFrame {
    response: halley_ipc::Response,
    fds: Vec<OwnedFd>,
    stream: Option<mpsc::Receiver<halley_ipc::Response>>,
}

/// The reply half of one IPC request. It is deliberately consumed by
/// `send`, making double replies impossible while allowing a user-driven
/// operation to retain it until selection completes.
pub struct ReplySender(mpsc::SyncSender<ReplyFrame>);

impl ReplySender {
    pub fn send(
        self,
        response: halley_ipc::Response,
        fds: Vec<OwnedFd>,
    ) -> Result<(), Box<halley_ipc::Response>> {
        self.0
            .send(ReplyFrame {
                response,
                fds,
                stream: None,
            })
            .map_err(|err| Box::new(err.0.response))
    }

    pub fn subscribe(
        self,
        snapshot: halley_ipc::StateSnapshot,
        stream: mpsc::Receiver<halley_ipc::Response>,
    ) -> Result<(), Box<halley_ipc::Response>> {
        self.0
            .send(ReplyFrame {
                response: halley_ipc::Response::Subscribed(snapshot),
                fds: Vec::new(),
                stream: Some(stream),
            })
            .map_err(|err| Box::new(err.0.response))
    }
}

/// One request delivered on the compositor thread.
pub struct RequestEnvelope {
    pub request: halley_ipc::Request,
    pub fds: Vec<OwnedFd>,
    pub reply: ReplySender,
}

fn client_worker(stream: UnixStream, requests: Sender<RequestEnvelope>) {
    loop {
        let (bytes, fds) = match halley_ipc::read_frame_with_fds(&stream, MAX_REQUEST_FDS) {
            Ok(frame) => frame,
            Err(halley_ipc::CodecError::Io(err))
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::BrokenPipe
                ) =>
            {
                break;
            }
            Err(err) => {
                eventline::warn!("ipc: failed to read request: {err}");
                break;
            }
        };
        let request = match halley_ipc::decode_request(&bytes) {
            Ok(request) => request,
            Err(err) => {
                if write_response(
                    &stream,
                    ReplyFrame {
                        response: halley_ipc::Response::Error(err.to_string()),
                        fds: Vec::new(),
                        stream: None,
                    },
                )
                .is_err()
                {
                    break;
                }
                continue;
            }
        };

        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        if requests
            .send(RequestEnvelope {
                request,
                fds,
                reply: ReplySender(reply_tx),
            })
            .is_err()
        {
            break;
        }
        let Ok(reply) = reply_rx.recv() else {
            break;
        };
        let mut reply = reply;
        let event_stream = reply.stream.take();
        if let Err(err) = write_response(&stream, reply) {
            eventline::warn!("ipc: failed to write response: {err}");
            break;
        }
        if let Some(event_stream) = event_stream {
            for response in event_stream {
                if write_response(
                    &stream,
                    ReplyFrame {
                        response,
                        fds: Vec::new(),
                        stream: None,
                    },
                )
                .is_err()
                {
                    break;
                }
            }
            break;
        }
    }
}

fn write_response(stream: &UnixStream, reply: ReplyFrame) -> Result<(), halley_ipc::CodecError> {
    let bytes = halley_ipc::encode_response(&reply.response)?;
    let fds = reply.fds.iter().map(AsRawFd::as_raw_fd).collect::<Vec<_>>();
    halley_ipc::write_frame_with_fds(stream, &bytes, &fds)
}

struct ApiSubscriber {
    topics: HashSet<halley_ipc::EventTopic>,
    sender: mpsc::SyncSender<halley_ipc::Response>,
    sequence: u64,
}

#[derive(Default)]
pub struct ApiSubscriptions {
    previous: Option<halley_ipc::StateSnapshot>,
    subscribers: Vec<ApiSubscriber>,
}

impl ApiSubscriptions {
    fn subscribe(
        &mut self,
        topics: Vec<halley_ipc::EventTopic>,
        snapshot: halley_ipc::StateSnapshot,
    ) -> mpsc::Receiver<halley_ipc::Response> {
        self.previous = Some(snapshot);
        let (sender, receiver) = mpsc::sync_channel(256);
        self.subscribers.push(ApiSubscriber {
            topics: topics.into_iter().collect(),
            sender,
            sequence: 0,
        });
        receiver
    }

    fn publish(&mut self, snapshot: halley_ipc::StateSnapshot) {
        let Some(previous) = self.previous.replace(snapshot.clone()) else {
            return;
        };
        let changes = api_changes(&previous, &snapshot);
        self.subscribers.retain_mut(|subscriber| {
            for (topic, event) in &changes {
                if !subscriber.topics.contains(topic) {
                    continue;
                }
                subscriber.sequence = subscriber.sequence.saturating_add(1);
                let response = halley_ipc::Response::Event(event_with_sequence(
                    event.clone(),
                    subscriber.sequence,
                ));
                if subscriber.sender.try_send(response).is_err() {
                    return false;
                }
            }
            true
        });
    }

    fn publish_event(&mut self, topic: halley_ipc::EventTopic, event: halley_ipc::ApiEvent) {
        self.subscribers.retain_mut(|subscriber| {
            if !subscriber.topics.contains(&topic) {
                return true;
            }
            subscriber.sequence = subscriber.sequence.saturating_add(1);
            subscriber
                .sender
                .try_send(halley_ipc::Response::Event(event_with_sequence(
                    event.clone(),
                    subscriber.sequence,
                )))
                .is_ok()
        });
    }
}

fn event_with_sequence(event: halley_ipc::ApiEvent, sequence: u64) -> halley_ipc::ApiEvent {
    use halley_ipc::ApiEvent;
    match event {
        ApiEvent::OutputAdded { output, .. } => ApiEvent::OutputAdded { sequence, output },
        ApiEvent::OutputChanged { output, .. } => ApiEvent::OutputChanged { sequence, output },
        ApiEvent::OutputRemoved { name, .. } => ApiEvent::OutputRemoved { sequence, name },
        ApiEvent::NodeAdded { node, .. } => ApiEvent::NodeAdded { sequence, node },
        ApiEvent::NodeChanged { node, .. } => ApiEvent::NodeChanged { sequence, node },
        ApiEvent::NodeGeometryChanged {
            id,
            pos_x,
            pos_y,
            width,
            height,
            ..
        } => ApiEvent::NodeGeometryChanged {
            sequence,
            id,
            pos_x,
            pos_y,
            width,
            height,
        },
        ApiEvent::NodeRemoved { id, .. } => ApiEvent::NodeRemoved { sequence, id },
        ApiEvent::ClusterAdded { cluster, .. } => ApiEvent::ClusterAdded { sequence, cluster },
        ApiEvent::ClusterChanged { cluster, .. } => ApiEvent::ClusterChanged { sequence, cluster },
        ApiEvent::ClusterRemoved { id, .. } => ApiEvent::ClusterRemoved { sequence, id },
        ApiEvent::ConfigReloaded { accepted, .. } => {
            ApiEvent::ConfigReloaded { sequence, accepted }
        }
        ApiEvent::ClusterDraftChanged {
            id, state, message, ..
        } => ApiEvent::ClusterDraftChanged {
            sequence,
            id,
            state,
            message,
        },
    }
}

fn api_changes(
    before: &halley_ipc::StateSnapshot,
    after: &halley_ipc::StateSnapshot,
) -> Vec<(halley_ipc::EventTopic, halley_ipc::ApiEvent)> {
    use halley_ipc::{ApiEvent as E, EventTopic as T};
    let mut changes = Vec::new();
    diff_by_key(
        &before.outputs,
        &after.outputs,
        |v| v.name.clone(),
        |output| E::OutputAdded {
            sequence: 0,
            output,
        },
        |output| E::OutputChanged {
            sequence: 0,
            output,
        },
        |name| E::OutputRemoved { sequence: 0, name },
        T::Outputs,
        &mut changes,
    );
    diff_by_key(
        &before.clusters,
        &after.clusters,
        |v| v.id,
        |cluster| E::ClusterAdded {
            sequence: 0,
            cluster,
        },
        |cluster| E::ClusterChanged {
            sequence: 0,
            cluster,
        },
        |id| E::ClusterRemoved { sequence: 0, id },
        T::Clusters,
        &mut changes,
    );

    let before_nodes = before
        .nodes
        .iter()
        .map(|v| (v.id, v))
        .collect::<std::collections::HashMap<_, _>>();
    let after_nodes = after
        .nodes
        .iter()
        .map(|v| (v.id, v))
        .collect::<std::collections::HashMap<_, _>>();
    for (&id, node) in &after_nodes {
        match before_nodes.get(&id) {
            None => changes.push((
                T::Nodes,
                E::NodeAdded {
                    sequence: 0,
                    node: (*node).clone(),
                },
            )),
            Some(old) => {
                if node_semantics_changed(old, node) {
                    changes.push((
                        T::Nodes,
                        E::NodeChanged {
                            sequence: 0,
                            node: (*node).clone(),
                        },
                    ));
                }
                if old.pos_x != node.pos_x
                    || old.pos_y != node.pos_y
                    || old.width != node.width
                    || old.height != node.height
                {
                    changes.push((
                        T::NodeGeometry,
                        E::NodeGeometryChanged {
                            sequence: 0,
                            id,
                            pos_x: node.pos_x,
                            pos_y: node.pos_y,
                            width: node.width,
                            height: node.height,
                        },
                    ));
                }
            }
        }
    }
    for &id in before_nodes.keys() {
        if !after_nodes.contains_key(&id) {
            changes.push((T::Nodes, E::NodeRemoved { sequence: 0, id }));
        }
    }
    changes
}

#[allow(clippy::too_many_arguments)]
fn diff_by_key<T, K>(
    before: &[T],
    after: &[T],
    key: impl Fn(&T) -> K,
    added: impl Fn(T) -> halley_ipc::ApiEvent,
    changed: impl Fn(T) -> halley_ipc::ApiEvent,
    removed: impl Fn(K) -> halley_ipc::ApiEvent,
    topic: halley_ipc::EventTopic,
    output: &mut Vec<(halley_ipc::EventTopic, halley_ipc::ApiEvent)>,
) where
    T: Clone + PartialEq,
    K: Clone + Eq + std::hash::Hash,
{
    let before = before
        .iter()
        .map(|v| (key(v), v))
        .collect::<std::collections::HashMap<_, _>>();
    let after = after
        .iter()
        .map(|v| (key(v), v))
        .collect::<std::collections::HashMap<_, _>>();
    for (id, value) in &after {
        match before.get(id) {
            None => output.push((topic, added((*value).clone()))),
            Some(old) if *old != *value => output.push((topic, changed((*value).clone()))),
            _ => {}
        }
    }
    for id in before.keys() {
        if !after.contains_key(id) {
            output.push((topic, removed(id.clone())));
        }
    }
}

fn node_semantics_changed(a: &halley_ipc::NodeInfo, b: &halley_ipc::NodeInfo) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    (a.pos_x, a.pos_y, a.width, a.height) = (0.0, 0.0, 0.0, 0.0);
    (b.pos_x, b.pos_y, b.width, b.height) = (0.0, 0.0, 0.0, 0.0);
    a != b
}

/// If a socket file already exists at `path`, checks whether it's actually
/// live (another halley IPC listener still holds it) or stale (the process
/// that created it is gone) - refuses to start in the former case, removes
/// the file and proceeds in the latter. Mirrors old halley's own
/// stale-socket handling.
fn remove_stale_socket(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    match UnixStream::connect(path) {
        Ok(_) => Err(std::io::Error::other(
            "another halley IPC listener is already active on this socket",
        )),
        Err(_) => std::fs::remove_file(path),
    }
}

/// Binds the IPC socket and routes decoded requests onto the compositor
/// loop. Socket I/O waits on per-connection workers, so a deferred portal
/// reply never blocks rendering or input dispatch.
pub fn init_ipc_listener<App: 'static>(
    loop_handle: &LoopHandle<'_, App>,
    handler: impl Fn(&mut App, RequestEnvelope) + 'static,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = halley_ipc::ensure_runtime_dir()?.join("halley.sock");
    remove_stale_socket(&path)?;

    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;

    let (request_tx, request_rx) = channel();
    loop_handle.insert_source(request_rx, move |event, _, app| {
        if let Event::Msg(request) = event {
            handler(app, request);
        }
    })?;

    loop_handle.insert_source(
        Generic::new(listener, Interest::READ, CalloopMode::Level),
        move |_, listener, _app| {
            loop {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        let requests = request_tx.clone();
                        if let Err(err) = std::thread::Builder::new()
                            .name("halley-ipc-client".to_string())
                            .spawn(move || client_worker(stream, requests))
                        {
                            eventline::error!("ipc: failed to start client worker: {err}");
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(err) => {
                        eventline::error!("ipc: accept failed: {err}");
                        break;
                    }
                }
            }
            Ok(PostAction::Continue)
        },
    )?;

    Ok(())
}

pub fn handle_request<D: crate::session::SessionDriver>(
    app: &mut crate::session::Session<D>,
    request: RequestEnvelope,
) {
    let RequestEnvelope {
        request,
        fds,
        reply,
    } = request;
    let accepts_descriptors = matches!(
        request,
        halley_ipc::Request::RegisterDmabuf(_) | halley_ipc::Request::CaptureFrame(_)
    );
    if !accepts_descriptors && !fds.is_empty() {
        let _ = reply.send(
            halley_ipc::Response::Error("request included unexpected descriptors".to_string()),
            Vec::new(),
        );
        return;
    }
    if let halley_ipc::Request::Subscribe(subscription) = &request {
        if subscription.api_version != halley_ipc::HALLEY_API_VERSION {
            let _ = reply.send(
                halley_ipc::Response::ApiError(halley_ipc::ServerError::new(
                    halley_ipc::ServerErrorKind::VersionMismatch,
                    format!("unsupported API version {}", subscription.api_version),
                )),
                Vec::new(),
            );
            return;
        }
        publish_api_events(app);
        let snapshot = api_snapshot(app);
        let stream = app
            .api_subscriptions
            .subscribe(subscription.topics.clone(), snapshot.clone());
        let _ = reply.subscribe(snapshot, stream);
        return;
    }
    let response = match request {
        halley_ipc::Request::Outputs => {
            halley_ipc::Response::Outputs(halley_ipc::OutputsResponse {
                outputs: app.driver.output_info(),
            })
        }
        halley_ipc::Request::Version => halley_ipc::Response::Version(version_info()),
        halley_ipc::Request::Screenshot(request) => {
            crate::capture::request_screenshot(app, request, reply);
            return;
        }
        halley_ipc::Request::CancelScreenshot { request_handle } => {
            if crate::capture::cancel_portal(app, &request_handle) {
                halley_ipc::Response::Ack
            } else {
                halley_ipc::Response::Error("screenshot request is not active".to_string())
            }
        }
        halley_ipc::Request::ChooseSource(request) => {
            crate::capture::request_source(app, request, reply);
            return;
        }
        halley_ipc::Request::CancelSourceChooser { request_handle } => {
            if crate::capture::cancel_portal(app, &request_handle) {
                halley_ipc::Response::Ack
            } else {
                halley_ipc::Response::Error("source chooser is not active".to_string())
            }
        }
        halley_ipc::Request::RegisterDmabuf(request) => {
            match app.screencast.register(request, fds) {
                Ok(()) => halley_ipc::Response::Ack,
                Err(message) => halley_ipc::Response::Error(message),
            }
        }
        halley_ipc::Request::RemoveDmabuf {
            stream_handle,
            buffer_id,
        } => {
            app.screencast.remove(&stream_handle, buffer_id);
            halley_ipc::Response::Ack
        }
        halley_ipc::Request::CaptureFrame(request) => {
            match crate::capture::screencast::capture_frame(app, request, fds) {
                Ok(crate::capture::screencast::CaptureFrameResult::Immediate(response)) => {
                    halley_ipc::Response::Frame(response)
                }
                Ok(crate::capture::screencast::CaptureFrameResult::Submitted {
                    response,
                    sync,
                }) => {
                    let pending_reply = Rc::new(RefCell::new(Some(reply)));
                    let completion_reply = pending_reply.clone();
                    let completion = Box::new(move || {
                        if let Some(reply) = completion_reply.borrow_mut().take() {
                            let _ = reply.send(halley_ipc::Response::Frame(response), Vec::new());
                        }
                    });
                    if let Err(message) = app.driver.schedule_render_completion(sync, completion)
                        && let Some(reply) = pending_reply.borrow_mut().take()
                    {
                        let _ = reply.send(halley_ipc::Response::Error(message), Vec::new());
                    }
                    return;
                }
                Err(message) => halley_ipc::Response::Error(message),
            }
        }
        halley_ipc::Request::Node(request) => {
            typed_api_response(crate::nodes::handle_request(app, request))
        }
        halley_ipc::Request::Bearings(request) => match request {
            halley_ipc::BearingsRequest::Show => {
                if app.shell.bearings.set_visible(true) {
                    app.request_redraw();
                }
                halley_ipc::Response::Ack
            }
            halley_ipc::BearingsRequest::Hide => {
                if app.shell.bearings.set_visible(false) {
                    app.request_redraw();
                }
                halley_ipc::Response::Ack
            }
            halley_ipc::BearingsRequest::Toggle => {
                app.shell.bearings.toggle();
                app.request_redraw();
                halley_ipc::Response::Ack
            }
            halley_ipc::BearingsRequest::Status => {
                halley_ipc::Response::BearingsStatus(halley_ipc::BearingsStatusResponse {
                    visible: app.shell.bearings.visible(),
                })
            }
        },
        halley_ipc::Request::Quit => {
            app.show_exit_confirmation();
            halley_ipc::Response::Ack
        }
        halley_ipc::Request::ConfigPath => halley_ipc::Response::ConfigPath(
            app.config_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        ),
        halley_ipc::Request::Dpms { command, output } => {
            match app.driver.apply_dpms(command, output.as_deref()) {
                Ok(()) => {
                    crate::wayland::session_lock::confirm_unlit_outputs(app);
                    halley_ipc::Response::Ack
                }
                Err(message) => typed_api_response(halley_ipc::Response::Error(message)),
            }
        }
        halley_ipc::Request::Cluster(request) => {
            typed_api_response(crate::clusters::handle_request(app, request))
        }
        halley_ipc::Request::CaptureCapabilities => {
            let capabilities = app.driver.dmabuf_capabilities();
            halley_ipc::Response::CaptureCapabilities(halley_ipc::CaptureCapabilities {
                main_device: capabilities.main_device(),
                dmabuf_formats: capabilities
                    .formats()
                    .iter()
                    .map(|format| halley_ipc::DmabufFormat {
                        fourcc: format.code as u32,
                        modifier: format.modifier.into(),
                    })
                    .collect(),
            })
        }
        halley_ipc::Request::Hello(hello) => {
            if hello.api_version != halley_ipc::HALLEY_API_VERSION {
                halley_ipc::Response::ApiError(halley_ipc::ServerError::new(
                    halley_ipc::ServerErrorKind::VersionMismatch,
                    format!("unsupported API version {}", hello.api_version),
                ))
            } else {
                halley_ipc::Response::Hello(halley_ipc::ServerInfo {
                    compositor_version: env!("CARGO_PKG_VERSION").to_string(),
                    api_version: halley_ipc::HALLEY_API_VERSION,
                    ipc_protocol: halley_ipc::HALLEY_IPC_VERSION,
                    capabilities: vec![
                        "commands-v1".into(),
                        "subscriptions-v1".into(),
                        "cluster-drafts-v1".into(),
                        "trail-v1".into(),
                        "control-v1".into(),
                        "local-capture-v1".into(),
                    ],
                })
            }
        }
        halley_ipc::Request::Subscribe(_) => unreachable!(),
        halley_ipc::Request::ConfigReload => match app.config_watcher.as_ref() {
            Some(watcher) => {
                watcher.request_reload();
                halley_ipc::Response::Ack
            }
            None => halley_ipc::Response::ApiError(halley_ipc::ServerError::new(
                halley_ipc::ServerErrorKind::NotFound,
                "no configuration file is being watched",
            )),
        },
        halley_ipc::Request::Trail(request) => {
            typed_api_response(crate::trail::handle_request(app, request))
        }
        halley_ipc::Request::LocalCapture(request) => {
            crate::capture::request_local_capture(app, request, reply);
            return;
        }
        halley_ipc::Request::Control(request) => handle_control_request(app, request),
    };
    let _ = reply.send(response, Vec::new());
}

fn handle_control_request<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    request: halley_ipc::ControlRequest,
) -> halley_ipc::Response {
    let direction = |direction| match direction {
        halley_ipc::ControlDirection::Left => halley_config::Direction::Left,
        halley_ipc::ControlDirection::Right => halley_config::Direction::Right,
        halley_ipc::ControlDirection::Up => halley_config::Direction::Up,
        halley_ipc::ControlDirection::Down => halley_config::Direction::Down,
    };
    let (action, output) = match request {
        halley_ipc::ControlRequest::MonitorFocus(target) => {
            let target = match target {
                halley_ipc::MonitorFocusTarget::Direction(value) => {
                    halley_config::MonitorTarget::Direction(direction(value))
                }
                halley_ipc::MonitorFocusTarget::Output(name) => {
                    if !session
                        .wayland
                        .space
                        .outputs()
                        .any(|output| output.name() == name)
                    {
                        return api_error(
                            halley_ipc::ServerErrorKind::NotFound,
                            format!("unknown output {name:?}"),
                        );
                    }
                    halley_config::MonitorTarget::Output(name)
                }
            };
            (halley_config::Action::MonitorFocus(target), None)
        }
        halley_ipc::ControlRequest::StackCycle {
            direction: cycle,
            output,
        } => {
            let output = match control_output(session, output.as_deref()) {
                Ok(output) => output,
                Err(response) => return response,
            };
            let is_stacking = session
                .clusters
                .active_on(&output)
                .and_then(|id| session.clusters.metadata(id))
                .is_some_and(|metadata| {
                    metadata.layout
                        == halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Stacking
                });
            if !is_stacking {
                return api_error(
                    halley_ipc::ServerErrorKind::NotFound,
                    format!("no active stacking cluster on output {output:?}"),
                );
            }
            let cycle = match cycle {
                halley_ipc::StackCycleDirection::Forward => {
                    halley_config::FocusCycleDirection::Forward
                }
                halley_ipc::StackCycleDirection::Backward => {
                    halley_config::FocusCycleDirection::Backward
                }
            };
            (halley_config::Action::FocusCycle(cycle), Some(output))
        }
        halley_ipc::ControlRequest::TileFocus {
            direction: value,
            output,
        } => {
            let output = match control_output(session, output.as_deref()) {
                Ok(output) => output,
                Err(response) => return response,
            };
            (
                halley_config::Action::ClusterTileFocus(direction(value)),
                Some(output),
            )
        }
        halley_ipc::ControlRequest::TileSwap {
            direction: value,
            output,
        } => {
            let output = match control_output(session, output.as_deref()) {
                Ok(output) => output,
                Err(response) => return response,
            };
            (
                halley_config::Action::ClusterTileSwap(direction(value)),
                Some(output),
            )
        }
    };
    let Some(socket_name) = session.wayland_display.clone() else {
        return api_error(
            halley_ipc::ServerErrorKind::Internal,
            "Wayland display is not ready",
        );
    };
    crate::session::input::actions::dispatch(
        session,
        action,
        &socket_name,
        output.as_deref(),
        None,
        crate::session::input::actions::DispatchOrigin::Other,
    );
    halley_ipc::Response::Ack
}

// Returning the protocol response directly keeps request dispatch explicit and
// avoids a second error translation at each caller.
#[allow(clippy::result_large_err)]
fn control_output<D: crate::session::SessionDriver>(
    session: &crate::session::Session<D>,
    requested: Option<&str>,
) -> Result<String, halley_ipc::Response> {
    if let Some(name) = requested {
        if session
            .wayland
            .space
            .outputs()
            .any(|output| output.name() == name)
        {
            return Ok(name.to_string());
        }
        return Err(api_error(
            halley_ipc::ServerErrorKind::NotFound,
            format!("unknown output {name:?}"),
        ));
    }
    crate::wayland::focus::selected_output(&session.wayland)
        .map(Output::name)
        .or_else(|| Some(session.driver.primary_output().name()))
        .ok_or_else(|| api_error(halley_ipc::ServerErrorKind::NotFound, "no active output"))
}

fn api_error(
    kind: halley_ipc::ServerErrorKind,
    message: impl Into<String>,
) -> halley_ipc::Response {
    halley_ipc::Response::ApiError(halley_ipc::ServerError::new(kind, message))
}

/// Older internal handlers still produce their human-readable diagnostics as
/// `Response::Error`. Normalize those at the public API boundary so clients
/// receive a stable category and never need to inspect message text.
fn typed_api_response(response: halley_ipc::Response) -> halley_ipc::Response {
    let halley_ipc::Response::Error(message) = response else {
        return response;
    };
    let normalized = message.to_ascii_lowercase();
    let kind = if normalized.contains("matched multiple") || normalized.contains("ambiguous") {
        halley_ipc::ServerErrorKind::Ambiguous
    } else if normalized.contains("must be")
        || normalized.contains("between 1 and 10")
        || normalized.contains("belongs to output")
    {
        halley_ipc::ServerErrorKind::InvalidRequest
    } else if normalized.contains("not found")
        || normalized.contains("no node")
        || normalized.contains("no active cluster")
        || normalized.contains("no cluster exists")
        || normalized.contains("unknown output")
        || normalized.contains("disappeared")
    {
        halley_ipc::ServerErrorKind::NotFound
    } else {
        halley_ipc::ServerErrorKind::Internal
    };
    api_error(kind, message)
}

fn api_snapshot<D: crate::session::SessionDriver>(
    app: &mut crate::session::Session<D>,
) -> halley_ipc::StateSnapshot {
    let nodes =
        match crate::nodes::handle_request(app, halley_ipc::NodeRequest::List { output: None }) {
            halley_ipc::Response::NodeList(list) => list
                .outputs
                .into_iter()
                .flat_map(|group| group.nodes)
                .collect(),
            _ => Vec::new(),
        };
    let clusters = match crate::clusters::handle_request(
        app,
        halley_ipc::ClusterRequest::List { output: None },
    ) {
        halley_ipc::Response::ClusterList(list) => list
            .outputs
            .into_iter()
            .flat_map(|group| group.clusters)
            .collect(),
        _ => Vec::new(),
    };
    halley_ipc::StateSnapshot {
        sequence: 0,
        outputs: app.driver.output_info(),
        nodes,
        clusters,
        config_path: app
            .config_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
    }
}

pub fn publish_api_events<D: crate::session::SessionDriver>(app: &mut crate::session::Session<D>) {
    if app.api_subscriptions.subscribers.is_empty() {
        return;
    }
    let snapshot = api_snapshot(app);
    app.api_subscriptions.publish(snapshot);
}

pub fn publish_config_reload<D: crate::session::SessionDriver>(
    app: &mut crate::session::Session<D>,
    accepted: bool,
) {
    app.api_subscriptions.publish_event(
        halley_ipc::EventTopic::Config,
        halley_ipc::ApiEvent::ConfigReloaded {
            sequence: 0,
            accepted,
        },
    );
}

pub fn publish_cluster_draft<D: crate::session::SessionDriver>(
    app: &mut crate::session::Session<D>,
    id: u64,
    state: halley_ipc::ClusterDraftState,
    message: Option<String>,
) {
    app.api_subscriptions.publish_event(
        halley_ipc::EventTopic::Clusters,
        halley_ipc::ApiEvent::ClusterDraftChanged {
            sequence: 0,
            id,
            state,
            message,
        },
    );
}

#[cfg(test)]
mod typed_error_tests {
    use super::*;

    fn kind(message: &str) -> halley_ipc::ServerErrorKind {
        match typed_api_response(halley_ipc::Response::Error(message.into())) {
            halley_ipc::Response::ApiError(error) => error.kind,
            response => panic!("expected typed API error, got {response:?}"),
        }
    }

    #[test]
    fn classifies_public_control_errors() {
        assert_eq!(
            kind("selector app:term matched multiple nodes"),
            halley_ipc::ServerErrorKind::Ambiguous
        );
        assert_eq!(
            kind("cluster 7 was not found"),
            halley_ipc::ServerErrorKind::NotFound
        );
        assert_eq!(
            kind("cluster slot must be between 1 and 10, got 0"),
            halley_ipc::ServerErrorKind::InvalidRequest
        );
    }
}
