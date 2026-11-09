//! Opt-in socket fixtures for SDK consumers' tests.
//!
//! These helpers translate semantic SDK values to the private wire format.
//! They are not a compositor implementation. Unhandled requests fail explicitly;
//! callers retain control of socket timeouts, stalled replies, and disconnects.
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use crate::{
    Error, ErrorKind, Event, EventTopic, NodeInfo, NodeSelector, Result, ServerInfo, Snapshot,
};
use halley_ipc as wire;

/// Operations understood by the fixture. New operations may be added.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Request {
    Hello {
        api_version: u32,
    },
    Subscribe {
        api_version: u32,
        topics: Vec<EventTopic>,
    },
    FocusNode {
        selector: Option<NodeSelector>,
        output: Option<String>,
    },
    ShowBasics,
    Nodes {
        output: Option<String>,
    },
    Clusters {
        output: Option<String>,
    },
    ConfigPath,
    ReloadConfig,
}

/// Read a command from a real SDK client, without exposing its wire encoding.
pub fn request(stream: &UnixStream) -> Result<Request> {
    let (bytes, fds) = wire::read_frame_with_fds(stream, 0)?;
    if !fds.is_empty() {
        return Err(Error::new(ErrorKind::Protocol, "unexpected descriptors"));
    }
    Ok(match wire::decode_request(&bytes)? {
        wire::Request::Hello(v) => Request::Hello {
            api_version: v.api_version,
        },
        wire::Request::Subscribe(v) => Request::Subscribe {
            api_version: v.api_version,
            topics: v
                .topics
                .into_iter()
                .map(|topic| match topic {
                    wire::EventTopic::Outputs => EventTopic::Outputs,
                    wire::EventTopic::Nodes => EventTopic::Nodes,
                    wire::EventTopic::NodeGeometry => EventTopic::NodeGeometry,
                    wire::EventTopic::Clusters => EventTopic::Clusters,
                    wire::EventTopic::Config => EventTopic::Config,
                })
                .collect(),
        },
        wire::Request::Node(wire::NodeRequest::Focus { selector, output }) => Request::FocusNode {
            selector: selector.map(|v| match v {
                wire::NodeSelector::Focused => NodeSelector::Focused,
                wire::NodeSelector::Latest => NodeSelector::Latest,
                wire::NodeSelector::Id(id) => NodeSelector::Id(id.into()),
                wire::NodeSelector::Title(title) => NodeSelector::Title(title),
                wire::NodeSelector::App(app) => NodeSelector::App(app),
            }),
            output,
        },
        wire::Request::Control(wire::ControlRequest::ShowBasics) => Request::ShowBasics,
        wire::Request::Node(wire::NodeRequest::List { output }) => Request::Nodes { output },
        wire::Request::Cluster(wire::ClusterRequest::List { output }) => {
            Request::Clusters { output }
        }
        wire::Request::ConfigPath => Request::ConfigPath,
        wire::Request::ConfigReload => Request::ReloadConfig,
        other => {
            return Err(Error::new(
                ErrorKind::Unsupported,
                format!("unsupported fixture request: {other:?}"),
            ));
        }
    })
}

/// Replies use the same semantic values that the SDK returns to an application.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Response {
    Hello(ServerInfo),
    Ack,
    Subscribed(Snapshot),
    Event(Event),
    Nodes(Vec<NodeInfo>),
    Clusters(Vec<crate::ClusterSummary>),
    ConfigPath(Option<PathBuf>),
    Error(Error),
}

/// Send a fixture reply. The SDK owns the private codec and protocol version.
pub fn reply(stream: &UnixStream, response: Response) -> Result<()> {
    let response = match response {
        Response::Hello(v) => wire::Response::Hello(wire::ServerInfo {
            compositor_version: v.compositor_version,
            api_version: v.api_version,
            ipc_protocol: wire::HALLEY_IPC_VERSION,
            capabilities: v.capabilities,
        }),
        Response::Ack => wire::Response::Ack,
        Response::Subscribed(v) => wire::Response::Subscribed(wire::StateSnapshot {
            sequence: v.sequence,
            outputs: v.outputs.into_iter().map(output).collect(),
            nodes: v.nodes.into_iter().map(node).collect(),
            clusters: v.clusters.into_iter().map(cluster).collect(),
            config_path: v.config_path.map(|p| p.to_string_lossy().into_owned()),
        }),
        Response::Event(v) => wire::Response::Event(event(v)),
        Response::Nodes(nodes) => wire::Response::NodeList(wire::NodeListResponse {
            outputs: nodes
                .into_iter()
                .map(|v| wire::NodeOutputGroup {
                    output: v.output.clone().unwrap_or_default(),
                    nodes: vec![node(v)],
                })
                .collect(),
        }),
        Response::Clusters(clusters) => wire::Response::ClusterList(wire::ClusterListResponse {
            outputs: clusters
                .into_iter()
                .map(|v| wire::ClusterOutputGroup {
                    output: v.output.clone(),
                    clusters: vec![cluster(v)],
                })
                .collect(),
        }),
        Response::ConfigPath(v) => {
            wire::Response::ConfigPath(v.map(|p| p.to_string_lossy().into_owned()))
        }
        Response::Error(v) => wire::Response::ApiError(wire::ServerError::new(
            match v.kind() {
                ErrorKind::InvalidRequest => wire::ServerErrorKind::InvalidRequest,
                ErrorKind::NotFound => wire::ServerErrorKind::NotFound,
                ErrorKind::Ambiguous => wire::ServerErrorKind::Ambiguous,
                ErrorKind::Unsupported => wire::ServerErrorKind::Unsupported,
                ErrorKind::VersionMismatch => wire::ServerErrorKind::VersionMismatch,
                ErrorKind::Busy => wire::ServerErrorKind::Busy,
                ErrorKind::Internal => wire::ServerErrorKind::Internal,
                ErrorKind::Connection | ErrorKind::Protocol => {
                    return Err(Error::new(
                        ErrorKind::InvalidRequest,
                        "transport errors must be simulated by the socket",
                    ));
                }
            },
            v.message(),
        )),
    };
    wire::write_frame_with_fds(stream, &wire::encode_response(&response)?, &[])?;
    Ok(())
}

fn node(v: NodeInfo) -> wire::NodeInfo {
    wire::NodeInfo {
        id: v.id.get(),
        title: v.title,
        app_id: v.app_id,
        output: v.output,
        kind: match v.kind {
            crate::NodeKind::Surface => wire::NodeKind::Surface,
            crate::NodeKind::Core => wire::NodeKind::Core,
        },
        state: match v.state {
            crate::NodeState::Active => wire::NodeState::Active,
            crate::NodeState::Drifting => wire::NodeState::Drifting,
            crate::NodeState::Node => wire::NodeState::Node,
            crate::NodeState::Core => wire::NodeState::Core,
        },
        visible: v.visible,
        focused: v.focused,
        latest: v.latest,
        pinned: v.pinned,
        role: match v.role {
            crate::NodeRole::NormalToplevel => wire::NodeRole::NormalToplevel,
            crate::NodeRole::Dialog => wire::NodeRole::Dialog,
            crate::NodeRole::Popup => wire::NodeRole::Popup,
            crate::NodeRole::Unknown => wire::NodeRole::Unknown,
        },
        protocol_family: match v.protocol_family {
            crate::NodeProtocolFamily::XdgToplevel => wire::NodeProtocolFamily::XdgToplevel,
            crate::NodeProtocolFamily::XdgPopup => wire::NodeProtocolFamily::XdgPopup,
            crate::NodeProtocolFamily::Xwayland => wire::NodeProtocolFamily::Xwayland,
            crate::NodeProtocolFamily::Unknown => wire::NodeProtocolFamily::Unknown,
        },
        modal: v.modal,
        parent: v.parent.map(|id| wire::NodeRelationInfo {
            node_id: Some(id.get()),
        }),
        transient_for: v.transient_for.map(|id| wire::NodeRelationInfo {
            node_id: Some(id.get()),
        }),
        child_popup_count: v.child_popup_count,
        pos_x: v.x,
        pos_y: v.y,
        width: v.width,
        height: v.height,
    }
}

fn cluster(v: crate::ClusterSummary) -> wire::ClusterSummary {
    wire::ClusterSummary {
        id: v.id.get(),
        slot: v.slot,
        name: v.name,
        output: v.output,
        layout: match v.layout {
            crate::ClusterLayout::Tiling => wire::ClusterLayoutKind::Tiling,
            crate::ClusterLayout::Stacking => wire::ClusterLayoutKind::Stacking,
        },
        member_count: v.member_count,
        active: v.active,
        focused: v.focused,
    }
}

fn output(v: crate::OutputInfo) -> wire::OutputInfo {
    wire::OutputInfo {
        name: v.name,
        modes: v
            .modes
            .into_iter()
            .map(|v| wire::ModeInfo {
                width: v.width,
                height: v.height,
                refresh_millihz: v.refresh_millihz,
                preferred: v.preferred,
            })
            .collect(),
        current_mode: v.current_mode,
        offset_x: v.offset_x,
        offset_y: v.offset_y,
        scale: v.scale,
        vrr: v.vrr,
        vrr_supported: v.vrr_supported,
        vrr_active: v.vrr_active,
    }
}

fn event(v: Event) -> wire::ApiEvent {
    match v {
        Event::OutputAdded {
            sequence,
            output: v,
        } => wire::ApiEvent::OutputAdded {
            sequence,
            output: output(v),
        },
        Event::OutputChanged {
            sequence,
            output: v,
        } => wire::ApiEvent::OutputChanged {
            sequence,
            output: output(v),
        },
        Event::OutputRemoved { sequence, name } => wire::ApiEvent::OutputRemoved { sequence, name },
        Event::NodeAdded { sequence, node: v } => wire::ApiEvent::NodeAdded {
            sequence,
            node: node(v),
        },
        Event::NodeChanged { sequence, node: v } => wire::ApiEvent::NodeChanged {
            sequence,
            node: node(v),
        },
        Event::NodeGeometryChanged {
            sequence,
            id,
            x,
            y,
            width,
            height,
        } => wire::ApiEvent::NodeGeometryChanged {
            sequence,
            id: id.get(),
            pos_x: x,
            pos_y: y,
            width,
            height,
        },
        Event::NodeRemoved { sequence, id } => wire::ApiEvent::NodeRemoved {
            sequence,
            id: id.get(),
        },
        Event::ClusterAdded {
            sequence,
            cluster: v,
        } => wire::ApiEvent::ClusterAdded {
            sequence,
            cluster: cluster(v),
        },
        Event::ClusterChanged {
            sequence,
            cluster: v,
        } => wire::ApiEvent::ClusterChanged {
            sequence,
            cluster: cluster(v),
        },
        Event::ClusterRemoved { sequence, id } => wire::ApiEvent::ClusterRemoved {
            sequence,
            id: id.get(),
        },
        Event::ConfigReloaded { sequence, accepted } => {
            wire::ApiEvent::ConfigReloaded { sequence, accepted }
        }
        Event::ClusterDraftChanged {
            sequence,
            id,
            state,
            message,
        } => wire::ApiEvent::ClusterDraftChanged {
            sequence,
            id: id.get(),
            message,
            state: match state {
                crate::ClusterDraftState::Started => wire::ClusterDraftState::Started,
                crate::ClusterDraftState::AwaitingName => wire::ClusterDraftState::AwaitingName,
                crate::ClusterDraftState::Launching => wire::ClusterDraftState::Launching,
                crate::ClusterDraftState::Completed => wire::ClusterDraftState::Completed,
                crate::ClusterDraftState::Cancelled => wire::ClusterDraftState::Cancelled,
                crate::ClusterDraftState::Failed => wire::ClusterDraftState::Failed,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::Duration;

    struct Socket(PathBuf);
    impl Socket {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            Self(std::env::temp_dir().join(format!(
                "sdk-fixture-{}-{}.sock",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Socket {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn explicit_socket_and_semantic_fixture_preserve_requests_and_errors() {
        let socket = Socket::new();
        let listener = UnixListener::bind(&socket.0).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            assert_eq!(request(&stream).unwrap(), Request::Hello { api_version: 1 });
            reply(
                &stream,
                Response::Hello(ServerInfo {
                    compositor_version: "fixture".into(),
                    api_version: 1,
                    capabilities: vec![],
                }),
            )
            .unwrap();
            assert_eq!(request(&stream).unwrap(), Request::ConfigPath);
            reply(
                &stream,
                Response::ConfigPath(Some("/custom/config.rune".into())),
            )
            .unwrap();
            assert_eq!(
                request(&stream).unwrap(),
                Request::FocusNode {
                    selector: Some(NodeSelector::Id(crate::NodeId::new(0x1_0000_0001))),
                    output: Some("DP-2".into())
                }
            );
            reply(
                &stream,
                Response::Error(Error::new(ErrorKind::NotFound, "window closed")),
            )
            .unwrap();
        });
        let client = crate::Client::connect_to(&socket.0).unwrap();
        assert_eq!(
            client.config_path().unwrap(),
            Some("/custom/config.rune".into())
        );
        let error = client
            .focus_node(
                Some(NodeSelector::Id(crate::NodeId::new(0x1_0000_0001))),
                Some("DP-2"),
            )
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
        server.join().unwrap();
    }

    #[test]
    fn fixture_snapshots_and_events_roundtrip_public_values() {
        let node = NodeInfo {
            id: 0x1_0000_0001.into(),
            title: "Unicode — window".into(),
            app_id: Some("example".into()),
            output: Some("DP-1".into()),
            kind: crate::NodeKind::Surface,
            state: crate::NodeState::Node,
            visible: true,
            focused: false,
            latest: true,
            pinned: true,
            role: crate::NodeRole::Dialog,
            protocol_family: crate::NodeProtocolFamily::Xwayland,
            modal: true,
            parent: Some(2.into()),
            transient_for: Some(3.into()),
            child_popup_count: 4,
            x: -12.5,
            y: 37.25,
            width: 200.0,
            height: 100.0,
        };
        let cluster = crate::ClusterSummary {
            id: 9.into(),
            slot: Some(2),
            name: "work".into(),
            output: "DP-1".into(),
            layout: crate::ClusterLayout::Stacking,
            member_count: 1,
            active: true,
            focused: false,
        };
        let output = crate::OutputInfo {
            name: "DP-1".into(),
            modes: vec![crate::ModeInfo {
                width: 1920,
                height: 1080,
                refresh_millihz: 59940,
                preferred: true,
            }],
            current_mode: Some(0),
            offset_x: -1920,
            offset_y: 0,
            scale: 1.5,
            vrr: "auto".into(),
            vrr_supported: true,
            vrr_active: false,
        };
        let snapshot = Snapshot {
            sequence: 10,
            nodes: vec![node.clone()],
            clusters: vec![cluster.clone()],
            outputs: vec![output.clone()],
            config_path: Some("/custom/config.rune".into()),
        };
        let events = vec![
            Event::NodeChanged { sequence: 11, node },
            Event::ClusterChanged {
                sequence: 12,
                cluster,
            },
            Event::OutputChanged {
                sequence: 13,
                output,
            },
            Event::NodeGeometryChanged {
                sequence: 14,
                id: 2.into(),
                x: -1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
            },
            Event::ClusterDraftChanged {
                sequence: 15,
                id: 7.into(),
                state: crate::ClusterDraftState::Failed,
                message: Some("cancelled launch".into()),
            },
        ];
        let socket = Socket::new();
        let listener = UnixListener::bind(&socket.0).unwrap();
        let expected_snapshot = snapshot.clone();
        let expected_events = events.clone();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            assert_eq!(request(&stream).unwrap(), Request::Hello { api_version: 1 });
            reply(
                &stream,
                Response::Hello(ServerInfo {
                    compositor_version: "fixture".into(),
                    api_version: 1,
                    capabilities: vec![],
                }),
            )
            .unwrap();
            let (events_stream, _) = listener.accept().unwrap();
            assert_eq!(
                request(&events_stream).unwrap(),
                Request::Subscribe {
                    api_version: 1,
                    topics: vec![EventTopic::Nodes, EventTopic::Clusters]
                }
            );
            reply(&events_stream, Response::Subscribed(snapshot)).unwrap();
            for event in events {
                reply(&events_stream, Response::Event(event)).unwrap();
            }
        });
        let client = crate::Client::connect_to(&socket.0).unwrap();
        let mut subscription = client
            .subscribe([EventTopic::Nodes, EventTopic::Clusters])
            .unwrap();
        assert_eq!(subscription.initial, expected_snapshot);
        for event in expected_events {
            assert_eq!(subscription.events.next_event().unwrap(), event);
        }
        server.join().unwrap();
    }
}
