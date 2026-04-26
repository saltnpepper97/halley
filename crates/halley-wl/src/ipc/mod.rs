mod cluster;
mod monitor;
mod node;
mod trail;
mod view;

use std::time::Instant;

use halley_ipc::{
    BearingsRequest, BearingsStatusResponse, CaptureRequest, CaptureStatusResponse,
    CompositorRequest, IpcError, NodeMoveDirection, Request, Response, StackRequest, TileRequest,
};

use crate::compositor::root::Halley;
use crate::compositor::screenshot::screenshot_controller;

use self::cluster::handle_cluster_request;
use self::monitor::handle_monitor_request;
use self::node::handle_node_request;
use self::trail::handle_trail_request;
use self::view::IpcView;

#[cfg(test)]
use self::cluster::{inspect_cluster, list_clusters};
#[cfg(test)]
use self::monitor::adjacent_monitor;
#[cfg(test)]
use self::node::resolve_node_selector;

pub(crate) fn handle_request(st: &mut Halley, request: Request) -> Response {
    match request {
        Request::Capture(request) => handle_capture_request(st, request),
        Request::Node(request) => handle_node_request(st, request),
        Request::Trail(request) => handle_trail_request(st, request),
        Request::Monitor(request) => handle_monitor_request(st, request),
        Request::Bearings(request) => handle_bearings_request(st, request),
        Request::Stack(request) => handle_stack_request(st, request),
        Request::Tile(request) => handle_tile_request(st, request),
        Request::Cluster(request) => handle_cluster_request(st, request),
        Request::Compositor(CompositorRequest::Outputs) => Response::Error(IpcError::Unsupported(
            "outputs are handled by the ipc listener".into(),
        )),
        Request::Compositor(CompositorRequest::ApertureStatus) => {
            Response::ApertureStatus(crate::aperture::aperture_status(st))
        }
        Request::Compositor(CompositorRequest::Quit)
        | Request::Compositor(CompositorRequest::Reload)
        | Request::Compositor(CompositorRequest::Dpms { .. }) => Response::Error(
            IpcError::Unsupported("backend request not handled here".into()),
        ),
    }
}

fn handle_capture_request(st: &mut Halley, request: CaptureRequest) -> Response {
    match request {
        CaptureRequest::Start { mode, output } => {
            if screenshot_controller(&mut *st).start_screenshot_session(
                mode,
                output.as_deref(),
                Instant::now(),
            ) {
                Response::CaptureStatus(capture_status_response(st))
            } else {
                Response::Error(IpcError::Unsupported(
                    "screenshot session is already active".into(),
                ))
            }
        }
        CaptureRequest::Status => Response::CaptureStatus(capture_status_response(st)),
    }
}

fn capture_status_response(st: &Halley) -> CaptureStatusResponse {
    let last = st.input.interaction_state.last_screenshot_result.as_ref();
    CaptureStatusResponse {
        active: screenshot_controller(st).screenshot_session_active()
            || st
                .input
                .interaction_state
                .pending_screenshot_capture
                .is_some()
            || st
                .input
                .interaction_state
                .inflight_screenshot_capture
                .is_some(),
        session_serial: st
            .input
            .interaction_state
            .screenshot_session
            .as_ref()
            .map(|_| {
                st.input
                    .interaction_state
                    .screenshot_next_serial
                    .saturating_sub(1)
            }),
        last_finished_serial: last.map(|result| result.serial),
        saved_path: last
            .and_then(|result| result.saved_path.as_ref())
            .map(|path| path.display().to_string()),
        error: last.and_then(|result| result.error.clone()),
    }
}

fn handle_bearings_request(st: &mut Halley, request: BearingsRequest) -> Response {
    match request {
        BearingsRequest::Show => {
            st.ui.render_state.set_bearings_visible(true);
            Response::Ok
        }
        BearingsRequest::Hide => {
            st.ui.render_state.set_bearings_visible(false);
            Response::Ok
        }
        BearingsRequest::Toggle => {
            st.ui.render_state.toggle_bearings_visible();
            Response::Ok
        }
        BearingsRequest::Status => Response::BearingsStatus(BearingsStatusResponse {
            visible: st.ui.render_state.bearings_visible(),
        }),
    }
}

fn handle_stack_request(st: &mut Halley, request: StackRequest) -> Response {
    match request {
        StackRequest::Cycle { direction, output } => {
            match resolve_output_context(st, output.as_deref()).and_then(|monitor| {
                let now = Instant::now();
                focus_output_if_needed(st, monitor.as_str(), now);
                let direction = match direction {
                    halley_ipc::StackCycleDirection::Forward => {
                        halley_core::cluster_layout::ClusterCycleDirection::Next
                    }
                    halley_ipc::StackCycleDirection::Backward => {
                        halley_core::cluster_layout::ClusterCycleDirection::Prev
                    }
                };
                if st.cycle_active_stack_for_monitor(monitor.as_str(), direction, now) {
                    Ok(())
                } else {
                    Err(IpcError::Unsupported(format!(
                        "stack layout is not active on output {}",
                        monitor
                    )))
                }
            }) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            }
        }
    }
}

fn handle_tile_request(st: &mut Halley, request: TileRequest) -> Response {
    let (direction, output, swap) = match request {
        TileRequest::Focus { direction, output } => (direction, output, false),
        TileRequest::Swap { direction, output } => (direction, output, true),
    };
    match resolve_output_context(st, output.as_deref()).and_then(|monitor| {
        let now = Instant::now();
        focus_output_if_needed(st, monitor.as_str(), now);
        let direction = match direction {
            NodeMoveDirection::Left => halley_config::DirectionalAction::Left,
            NodeMoveDirection::Right => halley_config::DirectionalAction::Right,
            NodeMoveDirection::Up => halley_config::DirectionalAction::Up,
            NodeMoveDirection::Down => halley_config::DirectionalAction::Down,
        };
        let ok = if swap {
            st.tile_swap_active_cluster_member_for_monitor(monitor.as_str(), direction, now)
        } else {
            st.tile_focus_active_cluster_member_for_monitor(monitor.as_str(), direction, now)
        };
        if ok {
            Ok(())
        } else {
            Err(IpcError::Unsupported(format!(
                "tiled layout is not active or no tile exists {} on output {}",
                if swap { "to swap" } else { "to focus" },
                monitor
            )))
        }
    }) {
        Ok(()) => Response::Ok,
        Err(err) => Response::Error(err),
    }
}

fn resolve_output_context(st: &Halley, output: Option<&str>) -> Result<String, IpcError> {
    IpcView::from_halley(st).resolve_output_context(output)
}

fn validate_output<'a>(st: &Halley, output: &'a str) -> Result<&'a str, IpcError> {
    IpcView::from_halley(st).validate_output(output)?;
    Ok(output)
}

fn focus_output_if_needed(st: &mut Halley, output: &str, now: Instant) {
    if st.focused_monitor() != output {
        st.focus_monitor_view(output, now);
    }
}

fn sorted_outputs(st: &Halley) -> Vec<String> {
    IpcView::from_halley(st).sorted_outputs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::clusters::state::ClusterNameRecord;
    use halley_core::cluster::ClusterId;
    use halley_core::field::{NodeId, Vec2};

    fn cluster_test_state() -> (Halley, ClusterId, NodeId, NodeId) {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let first = state.model.field.spawn_surface(
            "firefox",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let second = state.model.field.spawn_surface(
            "foot",
            Vec2 { x: 200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.assign_node_to_current_monitor(first);
        state.assign_node_to_current_monitor(second);
        let cid = state
            .model
            .field
            .create_cluster(vec![first, second])
            .expect("cluster");
        state.model.cluster_state.cluster_names.insert(
            cid,
            ClusterNameRecord::Custom {
                name: "web".to_string(),
            },
        );
        let monitor = state.model.monitor_state.current_monitor.clone();
        state
            .model
            .cluster_state
            .active_cluster_workspaces
            .insert(monitor, cid);
        state
            .model
            .field
            .cluster_mut(cid)
            .expect("cluster mut")
            .enter_active();
        state.model.focus_state.primary_interaction_focus = Some(second);
        (state, cid, first, second)
    }

    #[test]
    fn default_selector_falls_back_to_latest_on_output() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let first = state.model.field.spawn_surface(
            "first",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let second = state.model.field.spawn_surface(
            "second",
            Vec2 { x: 200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.assign_node_to_current_monitor(first);
        state.assign_node_to_current_monitor(second);
        state.model.focus_state.primary_interaction_focus = None;

        assert_eq!(
            resolve_node_selector(&state, None, None).unwrap().as_u64(),
            2
        );
    }

    #[test]
    fn title_selector_reports_ambiguity() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let first = state.model.field.spawn_surface(
            "Kitty",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let second = state.model.field.spawn_surface(
            "Kitty scratch",
            Vec2 { x: 200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.assign_node_to_current_monitor(first);
        state.assign_node_to_current_monitor(second);

        let result = resolve_node_selector(
            &state,
            Some(&halley_ipc::NodeSelector::Title("kitty".into())),
            None,
        );
        assert!(matches!(result, Err(IpcError::Ambiguous(_))));
    }

    #[test]
    fn monitor_focus_direction_picks_adjacent_output() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 1920,
                height: 1080,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 1920,
                offset_y: 0,
                width: 1920,
                height: 1080,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.focus_monitor_view("left", Instant::now());

        assert_eq!(
            adjacent_monitor(&state, halley_ipc::MonitorFocusDirection::Right).as_deref(),
            Some("right")
        );
    }

    #[test]
    fn cluster_list_reports_named_active_cluster() {
        let (state, cid, _, _) = cluster_test_state();

        let list = list_clusters(&state, None).expect("cluster list");
        let cluster = list
            .outputs
            .iter()
            .flat_map(|group| group.clusters.iter())
            .find(|cluster| cluster.id == cid.as_u64())
            .expect("cluster summary");

        assert_eq!(cluster.name.as_deref(), Some("web"));
        assert!(cluster.active);
        assert!(cluster.focused);
        assert_eq!(cluster.member_count, 2);
    }

    #[test]
    fn cluster_inspect_defaults_to_current_output_active_cluster() {
        let (state, cid, first, second) = cluster_test_state();

        let info = inspect_cluster(&state, None, None).expect("cluster inspect");

        assert_eq!(info.id, cid.as_u64());
        assert_eq!(info.name.as_deref(), Some("web"));
        assert!(info.active);
        assert!(info.focused);
        assert_eq!(info.focused_member_index, Some(1));
        assert_eq!(info.focused_member_id, Some(second.as_u64()));
        assert_eq!(info.members.len(), 2);
        assert_eq!(info.members[0].id, first.as_u64());
        assert_eq!(info.members[1].id, second.as_u64());
    }
}
