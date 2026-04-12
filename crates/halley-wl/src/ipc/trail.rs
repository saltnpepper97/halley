use std::time::Instant;

use halley_ipc::{
    IpcError, Response, TrailEntryInfo, TrailListResponse, TrailRequest, TrailTarget,
};

use crate::compositor::root::Halley;

use super::focus_output_if_needed;
use super::node::{
    focus_node, node_info, node_is_queryable_surface, node_matches_output, resolve_node_selector,
};
use super::resolve_output_context;

pub(super) fn handle_trail_request(st: &mut Halley, request: TrailRequest) -> Response {
    match request {
        TrailRequest::Prev { output } => {
            match resolve_output_context(st, output.as_deref()).and_then(|monitor| {
                focus_output_if_needed(st, monitor.as_str(), Instant::now());
                if st.navigate_window_trail(halley_ipc::TrailDirection::Prev, Instant::now()) {
                    Ok(())
                } else {
                    Err(IpcError::NotFound(format!(
                        "no previous trail entry on output {}",
                        monitor
                    )))
                }
            }) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            }
        }
        TrailRequest::Next { output } => {
            match resolve_output_context(st, output.as_deref()).and_then(|monitor| {
                focus_output_if_needed(st, monitor.as_str(), Instant::now());
                if st.navigate_window_trail(halley_ipc::TrailDirection::Next, Instant::now()) {
                    Ok(())
                } else {
                    Err(IpcError::NotFound(format!(
                        "no next trail entry on output {}",
                        monitor
                    )))
                }
            }) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            }
        }
        TrailRequest::List { output } => match list_trail(st, output.as_deref()) {
            Ok(trail) => Response::TrailList(trail),
            Err(err) => Response::Error(err),
        },
        TrailRequest::Goto { target, output } => {
            match goto_trail_target(st, target, output.as_deref(), Instant::now()) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(err),
            }
        }
    }
}

fn list_trail(st: &mut Halley, output: Option<&str>) -> Result<TrailListResponse, IpcError> {
    let output = resolve_output_context(st, output)?;
    let snapshot = {
        let trail = st.model.focus_state.focus_trail.get(output.as_str());
        let entries = trail.map(|trail| trail.entries()).unwrap_or_default();
        let cursor_index = trail.and_then(|trail| trail.cursor_index());
        (entries, cursor_index)
    };

    let mut entries = Vec::new();
    for (index, id) in snapshot.0.into_iter().enumerate() {
        if !node_matches_output(st, id, output.as_str()) || !node_is_queryable_surface(st, id) {
            if let Some(trail) = st.model.focus_state.focus_trail.get_mut(output.as_str()) {
                trail.forget_node(id);
            }
            continue;
        }
        entries.push(TrailEntryInfo {
            index,
            cursor: snapshot.1 == Some(index),
            node: node_info(st, id),
        });
    }

    Ok(TrailListResponse {
        output,
        cursor_index: snapshot.1,
        entries,
    })
}

fn goto_trail_target(
    st: &mut Halley,
    target: TrailTarget,
    output: Option<&str>,
    now: Instant,
) -> Result<(), IpcError> {
    let output = resolve_output_context(st, output)?;
    focus_output_if_needed(st, output.as_str(), now);
    let node_id = match target {
        TrailTarget::Index(index) => st
            .model
            .focus_state
            .focus_trail
            .get_mut(output.as_str())
            .and_then(|trail| trail.seek_to_index(index))
            .ok_or_else(|| {
                IpcError::NotFound(format!(
                    "trail entry {} not found on output {}",
                    index, output
                ))
            })?,
        TrailTarget::Selector(selector) => {
            let node_id = resolve_node_selector(st, Some(&selector), Some(output.as_str()))?;
            let found = st
                .model
                .focus_state
                .focus_trail
                .get_mut(output.as_str())
                .is_some_and(|trail| trail.seek_to_node(node_id));
            if !found {
                return Err(IpcError::NotFound(format!(
                    "node {} is not present in the trail for output {}",
                    node_id.as_u64(),
                    output
                )));
            }
            node_id
        }
    };
    focus_node(st, node_id, now)
}
