use std::time::Instant;

use halley_api::{
    ApiError, RailItemInfo, RailOutputSnapshot, RailRequest, RailStatusResponse, RailVisibility,
    Response,
};
use halley_config::{RailObstructionBehavior, RailPlacement, RailSizingMode};
use halley_core::cluster_layout::ClusterWorkspaceLayoutKind;
use halley_core::field::{NodeId, NodeKind, NodeState};
use halley_core::tiling::Rect;

use crate::compositor::root::Halley;

use super::{focus_output_if_needed, sorted_outputs, validate_output};

pub(super) fn handle_rail_request(st: &mut Halley, request: RailRequest) -> Response {
    match request {
        RailRequest::Status { output } => match rail_status(st, output.as_deref()) {
            Ok(status) => Response::RailStatus(status),
            Err(err) => Response::Error(err),
        },
        RailRequest::FocusReveal { node_id } => {
            let id = NodeId::new(node_id);
            if !rail_node_exists(st, id) {
                return Response::Error(ApiError::NotFound(format!("node {node_id} not found")));
            }
            if crate::rail::activate_rail_item(st, id) {
                Response::Ok
            } else {
                Response::Error(ApiError::Internal(format!(
                    "failed to focus or reveal node {node_id}"
                )))
            }
        }
        RailRequest::TogglePin { node_id } => {
            let id = NodeId::new(node_id);
            if !rail_node_exists(st, id) {
                return Response::Error(ApiError::NotFound(format!("node {node_id} not found")));
            }
            let next = !st.node_user_pinned(id);
            if st.set_node_user_pinned(id, next) {
                Response::Ok
            } else {
                Response::Error(ApiError::Internal(format!(
                    "failed to toggle pin for node {node_id}"
                )))
            }
        }
        RailRequest::Close { node_id } => {
            let id = NodeId::new(node_id);
            if crate::compositor::surface::request_close_node_toplevel(st, id) {
                Response::Ok
            } else {
                Response::Error(ApiError::NotFound(format!(
                    "node {node_id} does not have a closeable toplevel surface"
                )))
            }
        }
    }
}

fn rail_status(st: &Halley, output: Option<&str>) -> Result<RailStatusResponse, ApiError> {
    let outputs = match output {
        Some(name) => vec![validate_output(st, name)?.to_string()],
        None => sorted_outputs(st),
    };
    let snapshots = outputs
        .into_iter()
        .map(|output| rail_output_snapshot(st, output.as_str()))
        .collect();
    Ok(RailStatusResponse {
        output: output.map(str::to_string),
        outputs: snapshots,
    })
}

fn rail_output_snapshot(st: &Halley, output: &str) -> RailOutputSnapshot {
    let (visibility, items) = rail_items_for_output(st, output);
    RailOutputSnapshot {
        output: output.to_string(),
        visibility,
        items,
    }
}

fn rail_items_for_output(st: &Halley, output: &str) -> (RailVisibility, Vec<RailItemInfo>) {
    let hidden_visibility = if st
        .model
        .fullscreen_state
        .fullscreen_active_node
        .contains_key(output)
    {
        Some(RailVisibility::HiddenFullscreen)
    } else if crate::compositor::workspace::state::maximize_session_active_on_monitor(st, output) {
        Some(RailVisibility::HiddenMaximized)
    } else {
        None
    };
    if st.active_cluster_workspace_for_monitor(output).is_some()
        && matches!(
            st.runtime.tuning.cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Tiling
        )
    {
        return (RailVisibility::HiddenTiledCluster, Vec::new());
    }

    let ids = if st.active_cluster_workspace_for_monitor(output).is_some()
        && matches!(
            st.runtime.tuning.cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Stacking
        ) {
        st.active_cluster_workspace_for_monitor(output)
            .and_then(|cid| st.model.field.cluster(cid))
            .map(|cluster| cluster.members().to_vec())
            .unwrap_or_default()
    } else {
        st.model.field.node_ids_all()
    };
    let focused = st
        .focused_node_for_monitor(output)
        .or(st.model.focus_state.primary_interaction_focus);
    let mut items: Vec<RailItemInfo> = ids
        .into_iter()
        .filter_map(|node_id| rail_item_info(st, output, focused, node_id))
        .collect();
    items.sort_by_key(|item| (!item.pinned, item.node_id));
    if items.is_empty() {
        (RailVisibility::HiddenEmpty, items)
    } else if let Some(visibility) = hidden_visibility {
        (visibility, items)
    } else if rail_obstructed_on_output(st, output, items.len()) {
        (RailVisibility::HiddenObstructed, items)
    } else {
        (RailVisibility::Visible, items)
    }
}

fn rail_obstructed_on_output(st: &Halley, output: &str, item_count: usize) -> bool {
    let config = st.runtime.tuning.rail;
    if !matches!(config.obstruction, RailObstructionBehavior::AutoHide) {
        return false;
    }
    let Some(monitor) = st.model.monitor_state.monitors.get(output) else {
        return false;
    };
    let bounds = rail_bounds_for_output(&config, monitor.width, monitor.height, item_count);
    active_window_rects_for_output(st, output, monitor.width, monitor.height)
        .into_iter()
        .any(|blocker| rects_intersect(bounds, blocker))
}

fn rail_bounds_for_output(
    config: &halley_config::RailConfig,
    screen_w: i32,
    screen_h: i32,
    item_count: usize,
) -> Rect {
    let vertical = matches!(config.placement, RailPlacement::Left | RailPlacement::Right);
    let item_count = item_count as i32;
    let gap_count = (item_count - 1).max(0);
    let content = config.padding * 2 + item_count * config.icon_size + gap_count * config.gap;
    let cross = config.padding * 2 + config.icon_size;
    let (mut width, mut height) = match (vertical, config.sizing) {
        (true, RailSizingMode::GrowToContent) => (positive_or(config.width, cross), content),
        (false, RailSizingMode::GrowToContent) => (content, positive_or(config.height, cross)),
        (true, RailSizingMode::Fixed) => (
            positive_or(config.width, cross),
            positive_or(config.height, content),
        ),
        (false, RailSizingMode::Fixed) => (
            positive_or(config.width, content),
            positive_or(config.height, cross),
        ),
    };
    width = width.clamp(1, screen_w.max(1));
    height = height.clamp(1, screen_h.max(1));
    let (x, y) = match config.placement {
        RailPlacement::Up => ((screen_w - width) / 2 + config.offset_x, config.offset_y),
        RailPlacement::Down => (
            (screen_w - width) / 2 + config.offset_x,
            screen_h - height - config.offset_y,
        ),
        RailPlacement::Left => (config.offset_x, (screen_h - height) / 2 + config.offset_y),
        RailPlacement::Right => (
            screen_w - width - config.offset_x,
            (screen_h - height) / 2 + config.offset_y,
        ),
    };
    Rect {
        x: x.clamp(0, (screen_w - width).max(0)) as f32,
        y: y.clamp(0, (screen_h - height).max(0)) as f32,
        w: width as f32,
        h: height as f32,
    }
}

fn rail_item_info(
    st: &Halley,
    output: &str,
    focused: Option<NodeId>,
    node_id: NodeId,
) -> Option<RailItemInfo> {
    let node = st.model.field.node(node_id)?;
    if node.kind != NodeKind::Surface || !st.model.field.is_visible(node_id) {
        return None;
    }
    if st
        .model
        .monitor_state
        .node_monitor
        .get(&node_id)
        .map(String::as_str)
        .unwrap_or(st.model.monitor_state.current_monitor.as_str())
        != output
    {
        return None;
    }
    let title = if node.label.trim().is_empty() {
        st.model
            .node_app_ids
            .get(&node_id)
            .cloned()
            .unwrap_or_else(|| format!("Window {}", node_id.as_u64()))
    } else {
        node.label.clone()
    };
    Some(RailItemInfo {
        node_id: node_id.as_u64(),
        title,
        app_id: st.model.node_app_ids.get(&node_id).cloned(),
        pinned: st.node_user_pinned(node_id),
        focused: focused == Some(node_id),
    })
}

fn active_window_rects_for_output(
    st: &Halley,
    output: &str,
    screen_w: i32,
    screen_h: i32,
) -> Vec<Rect> {
    let now = Instant::now();
    st.model
        .field
        .nodes()
        .iter()
        .filter_map(|(&node_id, node)| {
            if node.kind != NodeKind::Surface
                || !matches!(node.state, NodeState::Active | NodeState::Drifting)
                || !st.model.field.is_visible(node_id)
                || st
                    .model
                    .monitor_state
                    .node_monitor
                    .get(&node_id)
                    .map(String::as_str)
                    .unwrap_or(st.model.monitor_state.current_monitor.as_str())
                    != output
                || crate::compositor::surface::is_active_cluster_workspace_member(st, node_id)
            {
                return None;
            }
            node_screen_rect_for_output(st, output, node_id, screen_w, screen_h, now)
        })
        .collect()
}

fn node_screen_rect_for_output(
    st: &Halley,
    output: &str,
    node_id: NodeId,
    screen_w: i32,
    screen_h: i32,
    now: Instant,
) -> Option<Rect> {
    if output == st.model.monitor_state.current_monitor
        && let Some((left, top, right, bottom)) =
            crate::input::active_node_screen_rect(st, screen_w, screen_h, node_id, now, None)
    {
        return Some(Rect {
            x: left.min(right),
            y: top.min(bottom),
            w: (right - left).abs(),
            h: (bottom - top).abs(),
        });
    }
    let space = st.model.monitor_state.monitors.get(output)?;
    let node = st.model.field.node(node_id)?;
    let nx = ((node.pos.x - space.viewport.center.x) / space.viewport.size.x.max(1.0)) + 0.5;
    let ny = ((node.pos.y - space.viewport.center.y) / space.viewport.size.y.max(1.0)) + 0.5;
    let cx = nx * screen_w as f32;
    let cy = ny * screen_h as f32;
    let w = node.intrinsic_size.x.max(1.0);
    let h = node.intrinsic_size.y.max(1.0);
    Some(Rect {
        x: cx - w * 0.5,
        y: cy - h * 0.5,
        w,
        h,
    })
}

fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

fn positive_or(value: i32, default: i32) -> i32 {
    if value > 0 { value } else { default.max(1) }
}

fn rail_node_exists(st: &Halley, id: NodeId) -> bool {
    st.model
        .field
        .node(id)
        .is_some_and(|node| node.kind == NodeKind::Surface && st.model.field.is_visible(id))
}

#[allow(dead_code)]
fn focus_output_for_node(st: &mut Halley, id: NodeId, now: Instant) {
    if let Some(output) = st.model.monitor_state.node_monitor.get(&id).cloned() {
        focus_output_if_needed(st, output.as_str(), now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_core::field::Vec2;
    use smithay::reexports::wayland_server::Display;

    #[test]
    fn rail_status_sorts_pinned_first() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let monitor = st.model.monitor_state.current_monitor.clone();
        let a =
            st.model
                .field
                .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });
        let b = st.model.field.spawn_surface(
            "b",
            Vec2 { x: 100.0, y: 0.0 },
            Vec2 { x: 100.0, y: 100.0 },
        );
        st.assign_node_to_monitor(a, monitor.as_str());
        st.assign_node_to_monitor(b, monitor.as_str());
        assert!(st.set_node_user_pinned(b, true));

        let response = rail_status(&st, Some(monitor.as_str())).expect("status");
        let items = &response.outputs[0].items;

        assert_eq!(response.outputs[0].visibility, RailVisibility::Visible);
        assert_eq!(items[0].node_id, b.as_u64());
        assert!(items[0].pinned);
        assert_eq!(items[1].node_id, a.as_u64());
    }

    #[test]
    fn rail_status_hides_when_output_is_maximized() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let monitor = st.model.monitor_state.current_monitor.clone();
        let id = st.model.field.spawn_surface(
            "maximized",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 100.0 },
        );
        st.assign_node_to_monitor(id, monitor.as_str());
        st.model.workspace_state.maximize_sessions.insert(
            monitor.clone(),
            crate::compositor::workspace::state::MaximizeSession {
                target_id: id,
                node_snapshots: Default::default(),
                camera: crate::compositor::workspace::state::MaximizeCameraSnapshot {
                    center: Vec2 { x: 0.0, y: 0.0 },
                    view_size: Vec2 { x: 800.0, y: 600.0 },
                },
                state: crate::compositor::workspace::state::MaximizeSessionState::Active,
            },
        );

        let response = rail_status(&st, Some(monitor.as_str())).expect("status");

        assert_eq!(
            response.outputs[0].visibility,
            RailVisibility::HiddenMaximized
        );
        assert_eq!(response.outputs[0].items.len(), 1);
    }

    #[test]
    fn rail_status_hides_when_output_is_obstructed() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.rail.placement = halley_config::RailPlacement::Left;
        tuning.rail.width = 56;
        tuning.rail.height = 300;
        tuning.rail.sizing = halley_config::RailSizingMode::Fixed;
        tuning.rail.offset_x = 0;
        tuning.rail.offset_y = 0;
        tuning.rail.obstruction = halley_config::RailObstructionBehavior::AutoHide;
        let mut st = Halley::new_for_test(&dh, tuning);
        let monitor = st.model.monitor_state.current_monitor.clone();
        let id = st.model.field.spawn_surface(
            "obstructing",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 5000.0,
                y: 5000.0,
            },
        );
        st.assign_node_to_monitor(id, monitor.as_str());

        let response = rail_status(&st, Some(monitor.as_str())).expect("status");

        assert_eq!(
            response.outputs[0].visibility,
            RailVisibility::HiddenObstructed
        );
        assert_eq!(response.outputs[0].items.len(), 1);
    }
}
