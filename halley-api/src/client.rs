use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::{
    BearingsCommand, CaptureMode, CaptureOutcome, ClusterDraft, ClusterInfo, ClusterSummary,
    ClusterTarget, Direction, DpmsCommand, Error, ErrorKind, EventStream, EventTopic,
    HALLEY_API_VERSION, MonitorTarget, NodeInfo, NodeMoveDirection, NodeSelector, OutputInfo,
    Result, ServerInfo, StackCycleDirection, Subscription, TrailDirection, TrailInfo, TrailTarget,
};

#[derive(Clone, Debug, Default)]
pub struct ConnectOptions {
    pub socket_path: Option<PathBuf>,
    pub read_timeout: Option<Duration>,
    pub write_timeout: Option<Duration>,
}

pub struct Client {
    connection: Mutex<halley_ipc::Connection>,
    socket_path: PathBuf,
    server_info: ServerInfo,
}

impl Client {
    pub fn connect() -> Result<Self> {
        Self::connect_with(ConnectOptions::default())
    }

    pub fn connect_to(path: impl AsRef<Path>) -> Result<Self> {
        Self::connect_with(ConnectOptions {
            socket_path: Some(path.as_ref().to_path_buf()),
            ..Default::default()
        })
    }

    pub fn connect_with(options: ConnectOptions) -> Result<Self> {
        let path = options.socket_path.unwrap_or(
            halley_ipc::default_socket_path()
                .map_err(|e| Error::new(ErrorKind::Connection, e.to_string()))?,
        );
        let mut connection = halley_ipc::Connection::connect_to(&path)?;
        connection
            .set_read_timeout(options.read_timeout)
            .map_err(|e| Error::new(ErrorKind::Connection, e.to_string()))?;
        connection
            .set_write_timeout(options.write_timeout)
            .map_err(|e| Error::new(ErrorKind::Connection, e.to_string()))?;
        let response = request_on(
            &mut connection,
            halley_ipc::Request::Hello(halley_ipc::HelloRequest {
                api_version: HALLEY_API_VERSION,
            }),
        )?;
        let halley_ipc::Response::Hello(info) = response else {
            return Err(unexpected("hello", response));
        };
        if info.api_version != HALLEY_API_VERSION {
            return Err(Error::new(
                ErrorKind::VersionMismatch,
                format!(
                    "Halley API version mismatch: client {}, compositor {}",
                    HALLEY_API_VERSION, info.api_version
                ),
            ));
        }
        Ok(Self {
            connection: Mutex::new(connection),
            socket_path: path,
            server_info: ServerInfo {
                compositor_version: info.compositor_version,
                api_version: info.api_version,
                capabilities: info.capabilities,
            },
        })
    }

    pub fn server_info(&self) -> &ServerInfo {
        &self.server_info
    }

    pub fn outputs(&self) -> Result<Vec<OutputInfo>> {
        match self.request(halley_ipc::Request::Outputs)? {
            halley_ipc::Response::Outputs(v) => Ok(v.outputs.into_iter().map(Into::into).collect()),
            other => Err(unexpected("outputs", other)),
        }
    }

    pub fn set_dpms(&self, command: DpmsCommand, output: Option<&str>) -> Result<()> {
        self.ack(halley_ipc::Request::Dpms {
            command: match command {
                DpmsCommand::Off => halley_ipc::DpmsCommand::Off,
                DpmsCommand::On => halley_ipc::DpmsCommand::On,
                DpmsCommand::Toggle => halley_ipc::DpmsCommand::Toggle,
            },
            output: output.map(str::to_owned),
        })
    }

    pub fn nodes(&self, output: Option<&str>) -> Result<Vec<NodeInfo>> {
        match self.request(halley_ipc::Request::Node(halley_ipc::NodeRequest::List {
            output: output.map(str::to_owned),
        }))? {
            halley_ipc::Response::NodeList(v) => Ok(v
                .outputs
                .into_iter()
                .flat_map(|g| g.nodes)
                .map(Into::into)
                .collect()),
            other => Err(unexpected("node list", other)),
        }
    }

    pub fn node_info(
        &self,
        selector: Option<NodeSelector>,
        output: Option<&str>,
    ) -> Result<NodeInfo> {
        self.node_query(halley_ipc::NodeRequest::Info {
            selector: selector.map(selector_wire),
            output: output.map(str::to_owned),
        })
    }
    pub fn focus_node(&self, selector: Option<NodeSelector>, output: Option<&str>) -> Result<()> {
        self.ack(halley_ipc::Request::Node(halley_ipc::NodeRequest::Focus {
            selector: selector.map(selector_wire),
            output: output.map(str::to_owned),
        }))
    }
    pub fn move_node(
        &self,
        direction: NodeMoveDirection,
        selector: Option<NodeSelector>,
        output: Option<&str>,
    ) -> Result<()> {
        self.ack(halley_ipc::Request::Node(halley_ipc::NodeRequest::Move {
            direction: match direction {
                NodeMoveDirection::Left => halley_ipc::NodeMoveDirection::Left,
                NodeMoveDirection::Right => halley_ipc::NodeMoveDirection::Right,
                NodeMoveDirection::Up => halley_ipc::NodeMoveDirection::Up,
                NodeMoveDirection::Down => halley_ipc::NodeMoveDirection::Down,
            },
            selector: selector.map(selector_wire),
            output: output.map(str::to_owned),
        }))
    }
    pub fn close_node(&self, selector: Option<NodeSelector>, output: Option<&str>) -> Result<()> {
        self.ack(halley_ipc::Request::Node(halley_ipc::NodeRequest::Close {
            selector: selector.map(selector_wire),
            output: output.map(str::to_owned),
        }))
    }
    pub fn collapse_node(
        &self,
        selector: Option<NodeSelector>,
        output: Option<&str>,
    ) -> Result<()> {
        self.ack(halley_ipc::Request::Node(
            halley_ipc::NodeRequest::Collapse {
                selector: selector.map(selector_wire),
                output: output.map(str::to_owned),
            },
        ))
    }
    pub fn restore_node(&self, selector: Option<NodeSelector>, output: Option<&str>) -> Result<()> {
        self.ack(halley_ipc::Request::Node(
            halley_ipc::NodeRequest::Restore {
                selector: selector.map(selector_wire),
                output: output.map(str::to_owned),
            },
        ))
    }
    pub fn toggle_node(&self, selector: Option<NodeSelector>, output: Option<&str>) -> Result<()> {
        self.ack(halley_ipc::Request::Node(halley_ipc::NodeRequest::Toggle {
            selector: selector.map(selector_wire),
            output: output.map(str::to_owned),
        }))
    }

    pub fn navigate_trail(&self, direction: TrailDirection, output: Option<&str>) -> Result<()> {
        let output = output.map(str::to_owned);
        self.ack(halley_ipc::Request::Trail(match direction {
            TrailDirection::Previous => halley_ipc::TrailRequest::Previous { output },
            TrailDirection::Next => halley_ipc::TrailRequest::Next { output },
        }))
    }

    pub fn trail(&self, output: Option<&str>) -> Result<TrailInfo> {
        match self.request(halley_ipc::Request::Trail(halley_ipc::TrailRequest::List {
            output: output.map(str::to_owned),
        }))? {
            halley_ipc::Response::TrailList(info) => Ok(info.into()),
            other => Err(unexpected("trail list", other)),
        }
    }

    pub fn goto_trail(&self, target: TrailTarget, output: Option<&str>) -> Result<()> {
        let target = match target {
            TrailTarget::Index(index) => halley_ipc::TrailTarget::Index(index),
            TrailTarget::Selector(selector) => {
                halley_ipc::TrailTarget::Selector(selector_wire(selector))
            }
        };
        self.ack(halley_ipc::Request::Trail(halley_ipc::TrailRequest::Goto {
            target,
            output: output.map(str::to_owned),
        }))
    }

    pub fn clusters(&self, output: Option<&str>) -> Result<Vec<ClusterSummary>> {
        match self.request(halley_ipc::Request::Cluster(
            halley_ipc::ClusterRequest::List {
                output: output.map(str::to_owned),
            },
        ))? {
            halley_ipc::Response::ClusterList(v) => Ok(v
                .outputs
                .into_iter()
                .flat_map(|g| g.clusters)
                .map(Into::into)
                .collect()),
            other => Err(unexpected("cluster list", other)),
        }
    }
    pub fn cluster_info(&self, target: ClusterTarget, output: Option<&str>) -> Result<ClusterInfo> {
        match self.request(halley_ipc::Request::Cluster(
            halley_ipc::ClusterRequest::Inspect {
                target: target_wire(target),
                output: output.map(str::to_owned),
            },
        ))? {
            halley_ipc::Response::ClusterInfo(v) => Ok(v.into()),
            other => Err(unexpected("cluster info", other)),
        }
    }
    pub fn open_cluster(&self, target: ClusterTarget, output: Option<&str>) -> Result<()> {
        self.ack(halley_ipc::Request::Cluster(
            halley_ipc::ClusterRequest::Open {
                target: target_wire(target),
                output: output.map(str::to_owned),
            },
        ))
    }
    pub fn activate_cluster_slot(&self, slot: u8, output: Option<&str>) -> Result<()> {
        self.ack(halley_ipc::Request::Cluster(
            halley_ipc::ClusterRequest::Slot {
                slot,
                output: output.map(str::to_owned),
            },
        ))
    }
    pub fn cycle_cluster_layout(&self, output: Option<&str>) -> Result<()> {
        self.ack(halley_ipc::Request::Cluster(
            halley_ipc::ClusterRequest::LayoutCycle {
                output: output.map(str::to_owned),
            },
        ))
    }
    pub fn finalize_cluster_draft(
        &self,
        draft: ClusterDraft,
        output: Option<&str>,
    ) -> Result<crate::ClusterDraftId> {
        match self.request(halley_ipc::Request::Cluster(
            halley_ipc::ClusterRequest::OpenFinalizeDraft {
                draft: halley_ipc::ClusterDraftRequest {
                    name_hint: draft.name_hint,
                    app_launches: draft
                        .apps
                        .into_iter()
                        .map(|a| halley_ipc::ClusterDraftAppLaunch {
                            app_id: a.app_id,
                            command: a.command,
                        })
                        .collect(),
                    running_node_ids: draft.running_nodes.into_iter().map(|id| id.get()).collect(),
                    source: halley_ipc::ClusterDraftSource::External,
                },
                output: output.map(str::to_owned),
            },
        ))? {
            halley_ipc::Response::ClusterDraftStarted { id } => Ok(crate::ClusterDraftId::new(id)),
            other => Err(unexpected("cluster draft", other)),
        }
    }

    pub fn bearings_visible(&self) -> Result<bool> {
        match self.request(halley_ipc::Request::Bearings(
            halley_ipc::BearingsRequest::Status,
        ))? {
            halley_ipc::Response::BearingsStatus(v) => Ok(v.visible),
            other => Err(unexpected("bearings status", other)),
        }
    }
    pub fn set_bearings(&self, command: BearingsCommand) -> Result<()> {
        self.ack(halley_ipc::Request::Bearings(match command {
            BearingsCommand::Show => halley_ipc::BearingsRequest::Show,
            BearingsCommand::Hide => halley_ipc::BearingsRequest::Hide,
            BearingsCommand::Toggle => halley_ipc::BearingsRequest::Toggle,
        }))
    }
    pub fn config_path(&self) -> Result<Option<PathBuf>> {
        match self.request(halley_ipc::Request::ConfigPath)? {
            halley_ipc::Response::ConfigPath(v) => Ok(v.map(PathBuf::from)),
            other => Err(unexpected("config path", other)),
        }
    }
    pub fn reload_config(&self) -> Result<()> {
        self.ack(halley_ipc::Request::ConfigReload)
    }
    pub fn capture(&self, mode: CaptureMode, output: Option<&str>) -> Result<CaptureOutcome> {
        let mode = match mode {
            CaptureMode::Menu => halley_ipc::LocalCaptureMode::Menu,
            CaptureMode::Region => halley_ipc::LocalCaptureMode::Region,
            CaptureMode::Screen => halley_ipc::LocalCaptureMode::Screen,
            CaptureMode::Window => halley_ipc::LocalCaptureMode::Window,
        };
        match self.request(halley_ipc::Request::LocalCapture(
            halley_ipc::LocalCaptureRequest {
                mode,
                output: output.map(str::to_owned),
            },
        ))? {
            halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Saved { path }) => {
                Ok(CaptureOutcome::Saved(path.into()))
            }
            halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Cancelled) => {
                Ok(CaptureOutcome::Cancelled)
            }
            halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Failed {
                message,
            }) => Err(Error::new(ErrorKind::Internal, message)),
            other => Err(unexpected("capture", other)),
        }
    }
    pub fn transfer_window(&self, direction: Direction) -> Result<()> {
        self.ack(halley_ipc::Request::Control(
            halley_ipc::ControlRequest::WindowTransfer(direction_wire(direction)),
        ))
    }
    pub fn pan_field(&self, direction: Direction) -> Result<()> {
        self.ack(halley_ipc::Request::Control(
            halley_ipc::ControlRequest::PanField(direction_wire(direction)),
        ))
    }
    pub fn focus_monitor(&self, target: MonitorTarget) -> Result<()> {
        let target = match target {
            MonitorTarget::Direction(direction) => {
                halley_ipc::MonitorFocusTarget::Direction(direction_wire(direction))
            }
            MonitorTarget::Output(output) => halley_ipc::MonitorFocusTarget::Output(output),
        };
        self.ack(halley_ipc::Request::Control(
            halley_ipc::ControlRequest::MonitorFocus(target),
        ))
    }
    pub fn cycle_stack(&self, direction: StackCycleDirection, output: Option<&str>) -> Result<()> {
        self.ack(halley_ipc::Request::Control(
            halley_ipc::ControlRequest::StackCycle {
                direction: match direction {
                    StackCycleDirection::Forward => halley_ipc::StackCycleDirection::Forward,
                    StackCycleDirection::Backward => halley_ipc::StackCycleDirection::Backward,
                },
                output: output.map(str::to_owned),
            },
        ))
    }
    pub fn focus_tile(&self, direction: Direction, output: Option<&str>) -> Result<()> {
        self.tile_control(direction, output, false)
    }
    pub fn swap_tile(&self, direction: Direction, output: Option<&str>) -> Result<()> {
        self.tile_control(direction, output, true)
    }
    pub fn request_quit(&self) -> Result<()> {
        self.ack(halley_ipc::Request::Quit)
    }

    /// Show the one-time Halley basics card again. Manual reopening always
    /// works and does not depend on the first-run dismissal state.
    pub fn show_basics(&self) -> Result<()> {
        self.ack(halley_ipc::Request::Control(
            halley_ipc::ControlRequest::ShowBasics,
        ))
    }

    pub fn subscribe(&self, topics: impl IntoIterator<Item = EventTopic>) -> Result<Subscription> {
        let mut connection = halley_ipc::Connection::connect_to(&self.socket_path)?;
        let topics = topics.into_iter().map(topic_wire).collect();
        let response = request_on(
            &mut connection,
            halley_ipc::Request::Subscribe(halley_ipc::SubscribeRequest {
                api_version: HALLEY_API_VERSION,
                topics,
            }),
        )?;
        let halley_ipc::Response::Subscribed(snapshot) = response else {
            return Err(unexpected("subscription", response));
        };
        let last_sequence = snapshot.sequence;
        Ok(Subscription {
            initial: snapshot.into(),
            events: EventStream {
                connection,
                last_sequence,
            },
        })
    }

    fn node_query(&self, request: halley_ipc::NodeRequest) -> Result<NodeInfo> {
        match self.request(halley_ipc::Request::Node(request))? {
            halley_ipc::Response::NodeInfo(v) => Ok(v.into()),
            other => Err(unexpected("node", other)),
        }
    }
    fn tile_control(&self, direction: Direction, output: Option<&str>, swap: bool) -> Result<()> {
        let direction = direction_wire(direction);
        let request = if swap {
            halley_ipc::ControlRequest::TileSwap {
                direction,
                output: output.map(str::to_owned),
            }
        } else {
            halley_ipc::ControlRequest::TileFocus {
                direction,
                output: output.map(str::to_owned),
            }
        };
        self.ack(halley_ipc::Request::Control(request))
    }
    fn ack(&self, request: halley_ipc::Request) -> Result<()> {
        match self.request(request)? {
            halley_ipc::Response::Ack => Ok(()),
            other => Err(unexpected("acknowledgement", other)),
        }
    }
    fn request(&self, request: halley_ipc::Request) -> Result<halley_ipc::Response> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| Error::new(ErrorKind::Internal, "client connection lock was poisoned"))?;
        request_on(&mut connection, request)
    }
}

fn request_on(
    connection: &mut halley_ipc::Connection,
    request: halley_ipc::Request,
) -> Result<halley_ipc::Response> {
    let envelope = connection.request(&request, &[])?;
    if !envelope.fds.is_empty() {
        return Err(Error::new(
            ErrorKind::Protocol,
            "response carried unexpected file descriptors",
        ));
    }
    match envelope.response {
        halley_ipc::Response::ApiError(error) => Err(error.into()),
        halley_ipc::Response::Error(message) => Err(Error::new(ErrorKind::Internal, message)),
        response => Ok(response),
    }
}

fn unexpected(expected: &str, response: halley_ipc::Response) -> Error {
    Error::new(
        ErrorKind::Protocol,
        format!("expected {expected} response, received {response:?}"),
    )
}
fn selector_wire(v: NodeSelector) -> halley_ipc::NodeSelector {
    match v {
        NodeSelector::Focused => halley_ipc::NodeSelector::Focused,
        NodeSelector::Latest => halley_ipc::NodeSelector::Latest,
        NodeSelector::Id(id) => halley_ipc::NodeSelector::Id(id.get()),
        NodeSelector::Title(s) => halley_ipc::NodeSelector::Title(s),
        NodeSelector::App(s) => halley_ipc::NodeSelector::App(s),
    }
}
fn direction_wire(direction: Direction) -> halley_ipc::ControlDirection {
    match direction {
        Direction::Left => halley_ipc::ControlDirection::Left,
        Direction::Right => halley_ipc::ControlDirection::Right,
        Direction::Up => halley_ipc::ControlDirection::Up,
        Direction::Down => halley_ipc::ControlDirection::Down,
    }
}
fn target_wire(v: ClusterTarget) -> halley_ipc::ClusterTarget {
    match v {
        ClusterTarget::Current => halley_ipc::ClusterTarget::Current,
        ClusterTarget::Id(id) => halley_ipc::ClusterTarget::Id(id.get()),
    }
}
fn topic_wire(v: EventTopic) -> halley_ipc::EventTopic {
    match v {
        EventTopic::Outputs => halley_ipc::EventTopic::Outputs,
        EventTopic::Nodes => halley_ipc::EventTopic::Nodes,
        EventTopic::NodeGeometry => halley_ipc::EventTopic::NodeGeometry,
        EventTopic::Clusters => halley_ipc::EventTopic::Clusters,
        EventTopic::Config => halley_ipc::EventTopic::Config,
    }
}
