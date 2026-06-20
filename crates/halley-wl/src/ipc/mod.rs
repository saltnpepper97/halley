mod cluster;
mod monitor;
mod node;
mod trail;
mod view;

use std::time::Instant;

use halley_api::{
    ApiError, BearingsRequest, BearingsStatusResponse, CaptureRequest, CaptureStatusResponse,
    CompositorRequest, GamescopeTargetResponse, HALLEY_API_VERSION, NodeMoveDirection,
    PortalOutput, PortalScreenCastRequest, PortalScreenCastResponse, Request, Response,
    StackCycleDirection, StackRequest, TileRequest, VersionInfo,
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
        Request::PortalScreenCast(request) => handle_portal_screencast_request(st, request),
        Request::Compositor(CompositorRequest::Outputs) => Response::Error(ApiError::Unsupported(
            "outputs are handled by the ipc listener".into(),
        )),
        Request::Compositor(CompositorRequest::Version) => Response::Version(VersionInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            ipc_protocol: HALLEY_API_VERSION,
        }),
        Request::Compositor(CompositorRequest::ApertureStatus) => {
            Response::ApertureStatus(crate::aperture::aperture_status(st))
        }
        Request::Compositor(CompositorRequest::GamescopeTarget { selector }) => {
            gamescope_target_response(st, selector.as_str())
        }
        Request::Compositor(CompositorRequest::Quit)
        | Request::Compositor(CompositorRequest::Reload)
        | Request::Compositor(CompositorRequest::Dpms { .. }) => Response::Error(
            ApiError::Unsupported("backend request not handled here".into()),
        ),
    }
}

fn handle_portal_screencast_request(st: &mut Halley, request: PortalScreenCastRequest) -> Response {
    match request {
        PortalScreenCastRequest::ListOutputs => {
            Response::PortalScreenCast(PortalScreenCastResponse::Outputs(portal_outputs(st)))
        }
        PortalScreenCastRequest::SelectOutput { session_handle: _ } => Response::PortalScreenCast(
            PortalScreenCastResponse::SelectedOutput(select_portal_output(st)),
        ),
        PortalScreenCastRequest::StartSourceChooser {
            session_handle,
            source_types,
        } => {
            let started = crate::compositor::portal_chooser::start_portal_chooser(
                st,
                &session_handle,
                source_types,
                Instant::now(),
            );
            if started {
                Response::PortalScreenCast(PortalScreenCastResponse::SourceChooserStarted)
            } else {
                Response::PortalScreenCast(PortalScreenCastResponse::Error(
                    "source chooser already active or no source types accepted".into(),
                ))
            }
        }
        PortalScreenCastRequest::PollSourceChooser { session_handle } => {
            let resp = crate::compositor::portal_chooser::poll_portal_chooser(st, &session_handle);
            Response::PortalScreenCast(resp)
        }
        PortalScreenCastRequest::CancelSourceChooser { session_handle } => {
            let _ = crate::compositor::portal_chooser::cancel_portal_chooser_for_handle(
                st,
                &session_handle,
            );
            Response::PortalScreenCast(PortalScreenCastResponse::SourceChooserCancelled)
        }
        PortalScreenCastRequest::Start {
            session_handle,
            output,
            cursor_mode: _,
        } => {
            let (width, height) = st
                .model
                .monitor_state
                .outputs
                .get(output.as_str())
                .and_then(|o| o.current_mode())
                .map(|m| (m.size.w, m.size.h))
                .unwrap_or_else(|| {
                    st.model
                        .monitor_state
                        .monitors
                        .get(output.as_str())
                        .map(|m| (m.width, m.height))
                        .unwrap_or((1920, 1080))
                });

            let (offset_x, offset_y) = st
                .model
                .monitor_state
                .monitors
                .get(output.as_str())
                .map(|m| (m.offset_x, m.offset_y))
                .unwrap_or((0, 0));

            match st
                .screencast
                .start_output(&session_handle, &output, width, height)
            {
                Ok(shm_path) => Response::PortalScreenCast(PortalScreenCastResponse::Started {
                    node_id: 0,
                    width,
                    height,
                    offset_x,
                    offset_y,
                    source_type: halley_api::PORTAL_SOURCE_TYPE_MONITOR,
                    mapping_id: output,
                    shm_path: shm_path.to_string_lossy().to_string(),
                }),
                Err(e) => Response::PortalScreenCast(PortalScreenCastResponse::Error(format!(
                    "failed to start screencast: {e}"
                ))),
            }
        }
        PortalScreenCastRequest::StartWindow {
            session_handle,
            node_id,
            cursor_mode: _,
        } => start_window_screencast(st, session_handle, node_id),
        PortalScreenCastRequest::Stop { session_handle } => {
            st.screencast.stop(&session_handle);
            Response::PortalScreenCast(PortalScreenCastResponse::Stopped)
        }
    }
}

fn start_window_screencast(st: &mut Halley, session_handle: String, node_id_u64: u64) -> Response {
    use halley_api::PORTAL_SOURCE_TYPE_WINDOW;
    use halley_core::field::NodeId;

    let node_id = NodeId::new(node_id_u64);
    let Some(monitor) = st
        .model
        .monitor_state
        .node_monitor
        .get(&node_id)
        .cloned()
        .or_else(|| {
            st.model
                .field
                .node(node_id)
                .is_some()
                .then(|| st.model.monitor_state.current_monitor.clone())
        })
    else {
        return Response::PortalScreenCast(PortalScreenCastResponse::Error(format!(
            "window node {} has no monitor",
            node_id.as_u64()
        )));
    };
    let (width, height) = st
        .model
        .field
        .node(node_id)
        .map(|node| {
            let size = crate::compositor::surface::current_surface_size_for_node(st, node_id)
                .unwrap_or(node.intrinsic_size);
            (size.x.max(1.0) as i32, size.y.max(1.0) as i32)
        })
        .unwrap_or((640, 480));
    let (offset_x, offset_y) = st
        .model
        .monitor_state
        .monitors
        .get(monitor.as_str())
        .map(|m| (m.offset_x, m.offset_y))
        .unwrap_or((0, 0));
    match st
        .screencast
        .start_window(&session_handle, node_id, &monitor, width, height)
    {
        Ok(shm_path) => Response::PortalScreenCast(PortalScreenCastResponse::Started {
            node_id: 0,
            width,
            height,
            offset_x,
            offset_y,
            source_type: PORTAL_SOURCE_TYPE_WINDOW,
            mapping_id: format!("window:{}", node_id.as_u64()),
            shm_path: shm_path.to_string_lossy().to_string(),
        }),
        Err(e) => Response::PortalScreenCast(PortalScreenCastResponse::Error(format!(
            "failed to start window screencast: {e}"
        ))),
    }
}

fn portal_outputs(st: &Halley) -> Vec<PortalOutput> {
    let focused = st.focused_monitor().to_string();
    let mut outputs: Vec<_> = st
        .model
        .monitor_state
        .monitors
        .iter()
        .map(|(name, monitor)| {
            let mode_size = st
                .model
                .monitor_state
                .outputs
                .get(name.as_str())
                .and_then(|output| output.current_mode())
                .map(|mode| (mode.size.w, mode.size.h));
            let (width, height) = mode_size.unwrap_or((monitor.width, monitor.height));
            PortalOutput {
                name: name.clone(),
                width,
                height,
                offset_x: monitor.offset_x,
                offset_y: monitor.offset_y,
                focused: name == &focused,
            }
        })
        .collect();
    outputs.sort_by(|a, b| {
        a.offset_x
            .cmp(&b.offset_x)
            .then(a.offset_y.cmp(&b.offset_y))
            .then(a.name.cmp(&b.name))
    });
    outputs
}

fn select_portal_output(st: &Halley) -> Option<PortalOutput> {
    let outputs = portal_outputs(st);
    outputs
        .iter()
        .find(|output| output.focused)
        .cloned()
        .or_else(|| {
            outputs
                .iter()
                .find(|output| output.offset_x == 0 && output.offset_y == 0)
                .cloned()
        })
        .or_else(|| outputs.first().cloned())
}

/// Resolve a gamescope monitor selector to that monitor's current dimensions,
/// computed live so `cursor`/`focused` are never stale.
fn gamescope_target_response(st: &Halley, selector: &str) -> Response {
    use crate::compositor::monitor::state::{focused_monitor, interaction_monitor};

    let monitors = &st.model.monitor_state.monitors;
    let name: String = match selector.trim() {
        "" | "focused" => focused_monitor(st).to_string(),
        "cursor" => interaction_monitor(st).to_string(),
        "primary" => monitors
            .iter()
            .find(|(_, space)| space.offset_x == 0 && space.offset_y == 0)
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| focused_monitor(st).to_string()),
        other => {
            if monitors.contains_key(other) {
                other.to_string()
            } else {
                return Response::Error(ApiError::Unsupported(format!(
                    "unknown gamescope monitor selector `{other}`"
                )));
            }
        }
    };

    let (width, height, refresh_hz) = st
        .model
        .monitor_state
        .outputs
        .get(name.as_str())
        .and_then(|output| output.current_mode())
        .map(|mode| {
            let refresh = (mode.refresh > 0).then_some(mode.refresh as f64 / 1000.0);
            (
                mode.size.w.max(0) as u32,
                mode.size.h.max(0) as u32,
                refresh,
            )
        })
        .or_else(|| {
            monitors
                .get(name.as_str())
                .map(|space| (space.width.max(0) as u32, space.height.max(0) as u32, None))
        })
        .unwrap_or((0, 0, None));

    Response::GamescopeTarget(GamescopeTargetResponse {
        output: name,
        width,
        height,
        refresh_hz,
    })
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
                Response::Error(ApiError::Unsupported(
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
                    StackCycleDirection::Forward => {
                        halley_core::cluster_layout::ClusterCycleDirection::Next
                    }
                    StackCycleDirection::Backward => {
                        halley_core::cluster_layout::ClusterCycleDirection::Prev
                    }
                };
                if st.cycle_active_stack_for_monitor(monitor.as_str(), direction, now) {
                    Ok(())
                } else {
                    Err(ApiError::Unsupported(format!(
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
            Err(ApiError::Unsupported(format!(
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

fn resolve_output_context(st: &Halley, output: Option<&str>) -> Result<String, ApiError> {
    IpcView::from_halley(st).resolve_output_context(output)
}

fn validate_output<'a>(st: &Halley, output: &'a str) -> Result<&'a str, ApiError> {
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
        let cid = state.create_cluster(vec![first, second]).expect("cluster");
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
            .insert(monitor.clone(), cid);
        state
            .model
            .cluster_state
            .cluster_slot_order
            .insert(monitor, vec![cid]);
        state
            .model
            .field
            .cluster_mut(cid)
            .expect("cluster mut")
            .enter_active();
        state.model.focus_state.primary_interaction_focus = Some(second);
        (state, cid, first, second)
    }

    fn cluster_slot_test_state() -> (Halley, ClusterId, NodeId, String) {
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
        let cid = state.create_cluster(vec![first, second]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_current_monitor(core);
        let monitor = state.model.monitor_state.current_monitor.clone();
        state
            .model
            .cluster_state
            .cluster_names
            .insert(cid, ClusterNameRecord::Generic { slot: 1 });
        state
            .model
            .cluster_state
            .cluster_slot_order
            .insert(monitor, vec![cid]);
        let core_pos = state.model.field.node(core).expect("core node").pos;
        state.model.viewport.center = core_pos;
        state.model.camera_target_center = core_pos;
        let monitor = state.model.monitor_state.current_monitor.clone();
        (state, cid, core, monitor)
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
            Some(&halley_api::NodeSelector::Title("kitty".into())),
            None,
        );
        assert!(matches!(result, Err(ApiError::Ambiguous(_))));
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
            adjacent_monitor(&state, halley_api::MonitorFocusDirection::Right).as_deref(),
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
        assert_eq!(cluster.slot, Some(1));
        assert!(cluster.active);
        assert!(cluster.focused);
        assert_eq!(cluster.member_count, 2);
    }

    #[test]
    fn cluster_inspect_defaults_to_current_output_active_cluster() {
        let (state, cid, first, second) = cluster_test_state();

        let info = inspect_cluster(&state, None, None).expect("cluster inspect");

        assert_eq!(info.id, cid.as_u64());
        assert_eq!(info.slot, Some(1));
        assert_eq!(info.name.as_deref(), Some("web"));
        assert!(info.active);
        assert!(info.focused);
        assert_eq!(info.focused_member_index, Some(1));
        assert_eq!(info.focused_member_id, Some(second.as_u64()));
        assert_eq!(info.members.len(), 2);
        assert_eq!(info.members[0].id, first.as_u64());
        assert_eq!(info.members[1].id, second.as_u64());
    }

    #[test]
    fn cluster_slot_request_activates_slot() {
        let (mut state, cid, _, monitor) = cluster_slot_test_state();

        let response = handle_request(
            &mut state,
            halley_api::Request::Cluster(halley_api::ClusterRequest::Slot {
                slot: 1,
                output: None,
            }),
        );

        assert!(matches!(response, Response::Ok));
        assert_eq!(
            state.active_cluster_workspace_for_monitor(monitor.as_str()),
            Some(cid)
        );
    }

    #[test]
    fn cluster_slot_request_toggles_active_slot_closed() {
        let (mut state, _, core, monitor) = cluster_slot_test_state();

        let first = handle_request(
            &mut state,
            halley_api::Request::Cluster(halley_api::ClusterRequest::Slot {
                slot: 1,
                output: None,
            }),
        );
        let second = handle_request(
            &mut state,
            halley_api::Request::Cluster(halley_api::ClusterRequest::Slot {
                slot: 1,
                output: None,
            }),
        );

        assert!(matches!(first, Response::Ok));
        assert!(matches!(second, Response::Ok));
        assert_eq!(
            state.active_cluster_workspace_for_monitor(monitor.as_str()),
            None
        );
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(core)
        );
    }

    #[test]
    fn cluster_slot_request_rejects_invalid_slot() {
        let (mut state, _, _, _) = cluster_slot_test_state();

        let response = handle_request(
            &mut state,
            halley_api::Request::Cluster(halley_api::ClusterRequest::Slot {
                slot: 0,
                output: None,
            }),
        );

        assert!(matches!(
            response,
            Response::Error(ApiError::InvalidRequest(_))
        ));
    }

    #[test]
    fn cluster_slot_request_reports_empty_slot() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let response = handle_request(
            &mut state,
            halley_api::Request::Cluster(halley_api::ClusterRequest::Slot {
                slot: 1,
                output: None,
            }),
        );

        assert!(matches!(response, Response::Error(ApiError::NotFound(_))));
    }
}
