use super::*;

const KEYBOARD_RESIZE_STEP: i32 = 80;
const KEYBOARD_RESIZE_MIN_WIDTH: i32 = 96;
const KEYBOARD_RESIZE_MIN_HEIGHT: i32 = 72;

fn keyboard_resize_dimensions(
    width: i32,
    height: i32,
    direction: halley_config::Direction,
) -> (i32, i32) {
    match direction {
        halley_config::Direction::Left => (
            width
                .saturating_sub(KEYBOARD_RESIZE_STEP)
                .max(KEYBOARD_RESIZE_MIN_WIDTH),
            height,
        ),
        halley_config::Direction::Right => (width.saturating_add(KEYBOARD_RESIZE_STEP), height),
        halley_config::Direction::Up => (
            width,
            height
                .saturating_sub(KEYBOARD_RESIZE_STEP)
                .max(KEYBOARD_RESIZE_MIN_HEIGHT),
        ),
        halley_config::Direction::Down => (width, height.saturating_add(KEYBOARD_RESIZE_STEP)),
    }
}

pub fn handle_request<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    request: halley_ipc::NodeRequest,
) -> halley_ipc::Response {
    session.nodes.sync_from_space(&session.wayland.space);
    match request {
        halley_ipc::NodeRequest::List { output } => {
            let outputs = match requested_outputs(session, output.as_deref()) {
                Ok(outputs) => outputs,
                Err(error) => return halley_ipc::Response::Error(error),
            };
            halley_ipc::Response::NodeList(halley_ipc::NodeListResponse {
                outputs: outputs
                    .into_iter()
                    .map(|output| {
                        let mut ids = session
                            .nodes
                            .records()
                            .filter(|record| record.output == output)
                            .map(|record| record.id)
                            .collect::<Vec<_>>();
                        ids.sort_by_key(|id| id.as_u64());
                        halley_ipc::NodeOutputGroup {
                            output,
                            nodes: ids
                                .into_iter()
                                .filter_map(|id| node_info(session, id))
                                .collect(),
                        }
                    })
                    .collect(),
            })
        }
        halley_ipc::NodeRequest::Info { selector, output } => {
            match resolve(session, selector.as_ref(), output.as_deref()) {
                Ok(id) => node_info(session, id)
                    .map(halley_ipc::Response::NodeInfo)
                    .unwrap_or_else(|| halley_ipc::Response::Error("node disappeared".to_string())),
                Err(error) => halley_ipc::Response::Error(error),
            }
        }
        halley_ipc::NodeRequest::Focus { selector, output } => {
            let id = match resolve(session, selector.as_ref(), output.as_deref()) {
                Ok(id) => id,
                Err(error) => return halley_ipc::Response::Error(error),
            };
            let focused = crate::session::activate_node(session, id, true);
            if focused {
                halley_ipc::Response::Ack
            } else {
                halley_ipc::Response::Error(format!("failed to focus node {id}"))
            }
        }
        halley_ipc::NodeRequest::Move {
            direction,
            selector,
            output,
        } => {
            let id = match resolve(session, selector.as_ref(), output.as_deref()) {
                Ok(id) => id,
                Err(error) => return halley_ipc::Response::Error(error),
            };
            if move_node(session, id, direction) {
                halley_ipc::Response::Ack
            } else {
                halley_ipc::Response::Error(format!("failed to move node {id}"))
            }
        }
        halley_ipc::NodeRequest::Close { selector, output } => {
            let id = match resolve(session, selector.as_ref(), output.as_deref()) {
                Ok(id) => id,
                Err(error) => return halley_ipc::Response::Error(error),
            };
            if super::close(session, id) {
                halley_ipc::Response::Ack
            } else {
                halley_ipc::Response::Error(format!("node {id} disappeared"))
            }
        }
        halley_ipc::NodeRequest::Collapse { selector, output } => {
            change_node_state(session, selector.as_ref(), output.as_deref(), Some(true))
        }
        halley_ipc::NodeRequest::Restore { selector, output } => {
            change_node_state(session, selector.as_ref(), output.as_deref(), Some(false))
        }
        halley_ipc::NodeRequest::Toggle { selector, output } => {
            change_node_state(session, selector.as_ref(), output.as_deref(), None)
        }
    }
}

fn change_node_state<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    selector: Option<&halley_ipc::NodeSelector>,
    output: Option<&str>,
    collapsed: Option<bool>,
) -> halley_ipc::Response {
    let id = match resolve(session, selector, output) {
        Ok(id) => id,
        Err(error) => return halley_ipc::Response::Error(error),
    };
    let Some(current) = session.nodes.record(id).map(|record| record.collapsed) else {
        return halley_ipc::Response::Error(format!("node {id} disappeared"));
    };
    let desired = collapsed.unwrap_or(!current);
    if desired == current {
        return halley_ipc::Response::Ack;
    }
    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
    let changed = if desired {
        collapse(session, id, serial)
    } else {
        restore(session, id, serial)
    };
    if changed {
        halley_ipc::Response::Ack
    } else {
        let action = if desired { "collapse" } else { "restore" };
        halley_ipc::Response::Error(format!("failed to {action} node {id}"))
    }
}

fn requested_outputs<D: crate::session::SessionDriver>(
    session: &crate::session::Session<D>,
    requested: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut outputs = session
        .wayland
        .space
        .outputs()
        .map(|output| output.name())
        .collect::<Vec<_>>();
    outputs.sort();
    if let Some(requested) = requested {
        if outputs.iter().any(|output| output == requested) {
            Ok(vec![requested.to_string()])
        } else {
            Err(format!("unknown output {requested:?}"))
        }
    } else {
        Ok(outputs)
    }
}

pub(crate) fn resolve<D: crate::session::SessionDriver>(
    session: &crate::session::Session<D>,
    selector: Option<&halley_ipc::NodeSelector>,
    output: Option<&str>,
) -> Result<NodeId, String> {
    if let Some(output) = output {
        requested_outputs(session, Some(output))?;
    }
    let on_output = |record: &&NodeRecord| output.is_none_or(|name| record.output == name);
    let records = session
        .nodes
        .records()
        .filter(on_output)
        .collect::<Vec<_>>();
    let direct = match selector {
        None | Some(halley_ipc::NodeSelector::Focused) => session.nodes.focused().filter(|id| {
            session
                .nodes
                .record(*id)
                .is_some_and(|record| output.is_none_or(|name| record.output == name))
        }),
        Some(halley_ipc::NodeSelector::Latest) => records
            .iter()
            .map(|record| record.id)
            .max_by_key(|id| id.as_u64()),
        Some(halley_ipc::NodeSelector::Id(raw)) => records
            .iter()
            .find(|record| record.id.as_u64() == *raw)
            .map(|record| record.id),
        Some(halley_ipc::NodeSelector::Title(text)) => {
            return unique_match(
                records
                    .iter()
                    .filter(|record| contains_case_insensitive(&record.title, text))
                    .map(|record| record.id)
                    .collect(),
                &format!("title:{text}"),
            );
        }
        Some(halley_ipc::NodeSelector::App(text)) => {
            return unique_match(
                records
                    .iter()
                    .filter(|record| {
                        record
                            .app_id
                            .as_deref()
                            .is_some_and(|app| contains_case_insensitive(app, text))
                    })
                    .map(|record| record.id)
                    .collect(),
                &format!("app:{text}"),
            );
        }
    };
    direct
        .or_else(|| {
            (selector.is_none()).then(|| {
                records
                    .iter()
                    .map(|record| record.id)
                    .max_by_key(|id| id.as_u64())
            })?
        })
        .ok_or_else(|| "no node matched the selector".to_string())
}

fn unique_match(ids: Vec<NodeId>, label: &str) -> Result<NodeId, String> {
    match ids.as_slice() {
        [id] => Ok(*id),
        [] => Err(format!("no node matched selector {label}")),
        _ => Err(format!("selector {label} matched multiple nodes")),
    }
}

fn contains_case_insensitive(value: &str, needle: &str) -> bool {
    value.to_lowercase().contains(&needle.to_lowercase())
}

pub(crate) fn move_selected_direction<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    direction: halley_config::Direction,
    output: Option<&str>,
) -> bool {
    session.nodes.sync_from_space(&session.wayland.space);
    let Ok(id) = resolve(session, None, output) else {
        return false;
    };
    let direction = match direction {
        halley_config::Direction::Left => halley_ipc::NodeMoveDirection::Left,
        halley_config::Direction::Right => halley_ipc::NodeMoveDirection::Right,
        halley_config::Direction::Up => halley_ipc::NodeMoveDirection::Up,
        halley_config::Direction::Down => halley_ipc::NodeMoveDirection::Down,
    };
    move_node(session, id, direction)
}

pub(crate) fn resize_selected_direction<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    direction: halley_config::Direction,
    output: Option<&str>,
) -> bool {
    session.nodes.sync_from_space(&session.wayland.space);
    let Ok(id) = resolve(session, None, output) else {
        return false;
    };
    let Some(record) = session.nodes.record(id).cloned() else {
        return false;
    };
    if record.collapsed
        || crate::session::node_user_pinned(session, id)
        || session.clusters.is_member(id)
        || session.fullscreen.is_fullscreen_or_pending(&record.surface)
        || session.maximize.contains(&record.surface)
    {
        return false;
    }
    let Some(current) = session.wayland.space.element_geometry(&record.window) else {
        return false;
    };
    let requested = keyboard_resize_dimensions(current.size.w, current.size.h, direction).into();
    let size = if crate::xwayland::is_x11(&record.window) {
        crate::xwayland::constrain_window_size(&record.window, requested)
    } else {
        requested
    };
    if size == current.size {
        return false;
    }
    session
        .interactions
        .field_arrange
        .invalidate_surface(&record.surface);
    if let Some(toplevel) = record.window.toplevel() {
        toplevel.with_pending_state(|pending| pending.size = Some(size));
        toplevel.send_pending_configure();
    } else {
        crate::xwayland::configure_window(
            session,
            &record.window,
            Rectangle::new(current.loc, size),
        );
    }
    session.request_redraw();
    true
}

fn move_node<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: NodeId,
    direction: halley_ipc::NodeMoveDirection,
) -> bool {
    let Some(record) = session.nodes.record(id).cloned() else {
        return false;
    };
    if crate::session::node_user_pinned(session, id) {
        return false;
    }
    let (dx, dy) = match direction {
        halley_ipc::NodeMoveDirection::Left => (-80, 0),
        halley_ipc::NodeMoveDirection::Right => (80, 0),
        halley_ipc::NodeMoveDirection::Up => (0, -80),
        halley_ipc::NodeMoveDirection::Down => (0, 80),
    };
    if record.collapsed {
        let Some(current) = session.nodes.field.node(id).map(|node| node.pos) else {
            return false;
        };
        let desired = Vec2 {
            x: current.x + dx as f32,
            y: current.y + dy as f32,
        };
        let scale = session
            .cameras
            .get(&record.output)
            .map(crate::presentation::camera::scale)
            .unwrap_or(1.0);
        let occupied_cores = session
            .clusters
            .collapsed_core_landmarks()
            .into_iter()
            .filter_map(|(_, _, output, position, _)| (output == record.output).then_some(position))
            .collect::<Vec<_>>();
        let destination = session.nodes.nearest_free_position(
            id,
            desired,
            scale,
            &occupied_cores,
            super::PlacementChrome {
                decorations: &session.settings.decorations,
                font: &session.settings.font,
            },
        );
        if let Some(node) = session.nodes.field.node_mut(id) {
            node.pos = destination;
        }
        // A discrete move is an explicit placement, not a reversible zoom offset.
        session.nodes.commit_zoom_home(id);
        session.nodes.start_landmark_slide(
            id,
            current,
            destination,
            crate::frame_clock::monotonic_now(),
        );
    } else {
        let Some(location) = session.wayland.space.element_location(&record.window) else {
            return false;
        };
        let desired = location + Point::<i32, Logical>::from((dx, dy));
        let scale = session
            .cameras
            .get(&record.output)
            .map(crate::presentation::camera::scale)
            .unwrap_or(1.0);
        let occupied_cores = session
            .clusters
            .collapsed_core_landmarks()
            .into_iter()
            .filter_map(|(_, _, output, position, _)| (output == record.output).then_some(position))
            .collect::<Vec<_>>();
        let next = session
            .nodes
            .nearest_free_active_rect(
                id,
                Rectangle::new(desired, record.geometry.size),
                &record.output,
                scale,
                &occupied_cores,
                super::PlacementChrome {
                    decorations: &session.settings.decorations,
                    font: &session.settings.font,
                },
            )
            .loc;
        if next != location {
            session
                .interactions
                .field_arrange
                .invalidate_surface(&record.surface);
        }
        session.wayland.space.relocate_element(&record.window, next);
        if crate::xwayland::is_x11(&record.window) {
            crate::xwayland::configure_window(
                session,
                &record.window,
                Rectangle::new(next, record.geometry.size),
            );
        }
    }
    session.request_redraw();
    true
}

pub(crate) fn node_info<D: crate::session::SessionDriver>(
    session: &crate::session::Session<D>,
    id: NodeId,
) -> Option<halley_ipc::NodeInfo> {
    let record = session.nodes.record(id)?;
    let node = session.nodes.field.node(id)?;
    let latest = session
        .nodes
        .records()
        .filter(|candidate| candidate.output == record.output)
        .map(|candidate| candidate.id)
        .max_by_key(|candidate| candidate.as_u64())
        == Some(id);
    let (role, family, modal, parent) = relation_metadata(session, record);
    Some(halley_ipc::NodeInfo {
        id: id.as_u64(),
        title: record.title.clone(),
        app_id: record.app_id.clone(),
        output: Some(record.output.clone()),
        kind: halley_ipc::NodeKind::Surface,
        state: if record.collapsed {
            halley_ipc::NodeState::Node
        } else {
            halley_ipc::NodeState::Active
        },
        visible: record.attached,
        focused: session.nodes.focused() == Some(id),
        latest,
        pinned: crate::session::node_user_pinned(session, id),
        role,
        protocol_family: family,
        modal,
        parent: parent.clone(),
        transient_for: parent,
        child_popup_count: PopupManager::popups_for_surface(&record.surface).count(),
        pos_x: node.pos.x,
        pos_y: node.pos.y,
        width: node.intrinsic_size.x,
        height: node.intrinsic_size.y,
    })
}

fn relation_metadata<D: crate::session::SessionDriver>(
    session: &crate::session::Session<D>,
    record: &NodeRecord,
) -> (
    halley_ipc::NodeRole,
    halley_ipc::NodeProtocolFamily,
    bool,
    Option<halley_ipc::NodeRelationInfo>,
) {
    if crate::xwayland::is_x11(&record.window) {
        return (
            halley_ipc::NodeRole::NormalToplevel,
            halley_ipc::NodeProtocolFamily::Xwayland,
            false,
            None,
        );
    }
    let (parent, modal) = with_states(&record.surface, |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| data.lock().ok())
            .map(|data| {
                (
                    data.parent.clone(),
                    data.dialog_hint == ToplevelDialogHint::Modal,
                )
            })
            .unwrap_or((None, false))
    });
    let relation = parent.map(|parent| halley_ipc::NodeRelationInfo {
        node_id: session
            .nodes
            .id_for_surface(&crate::wayland::compositor::root_surface(&parent))
            .map(NodeId::as_u64),
    });
    (
        if relation.is_some() || modal {
            halley_ipc::NodeRole::Dialog
        } else {
            halley_ipc::NodeRole::NormalToplevel
        },
        halley_ipc::NodeProtocolFamily::XdgToplevel,
        modal,
        relation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_resize_uses_directional_axes_and_safe_minima() {
        assert_eq!(
            keyboard_resize_dimensions(800, 600, halley_config::Direction::Left),
            (720, 600)
        );
        assert_eq!(
            keyboard_resize_dimensions(800, 600, halley_config::Direction::Right),
            (880, 600)
        );
        assert_eq!(
            keyboard_resize_dimensions(800, 600, halley_config::Direction::Up),
            (800, 520)
        );
        assert_eq!(
            keyboard_resize_dimensions(800, 600, halley_config::Direction::Down),
            (800, 680)
        );
        assert_eq!(
            keyboard_resize_dimensions(100, 80, halley_config::Direction::Left),
            (KEYBOARD_RESIZE_MIN_WIDTH, 80)
        );
        assert_eq!(
            keyboard_resize_dimensions(100, 80, halley_config::Direction::Up),
            (100, KEYBOARD_RESIZE_MIN_HEIGHT)
        );
    }
}
