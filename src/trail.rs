use std::collections::HashMap;

use halley_core::field::NodeId;
use halley_core::trail::Trail;
use smithay::utils::SERIAL_COUNTER;

use crate::session::{Session, SessionDriver};

pub struct TrailState {
    by_output: HashMap<String, Trail>,
    config: halley_config::Trail,
    recording_suspended: bool,
}

impl TrailState {
    pub fn new(config: halley_config::Trail) -> Self {
        Self {
            by_output: HashMap::new(),
            config,
            recording_suspended: false,
        }
    }

    pub fn reload(&mut self, config: halley_config::Trail) {
        self.config = config;
        for trail in self.by_output.values_mut() {
            trail.truncate_to(config.history_length);
        }
    }

    fn record(&mut self, output: &str, id: NodeId) {
        if self.recording_suspended {
            return;
        }
        let trail = self.by_output.entry(output.to_string()).or_default();
        if trail.cursor() == Some(id) {
            return;
        }
        trail.record(id);
        trail.truncate_to(self.config.history_length);
    }

    pub fn forget(&mut self, id: NodeId) {
        for trail in self.by_output.values_mut() {
            trail.forget_node(id);
        }
    }

    fn len(&self, output: &str) -> usize {
        self.by_output.get(output).map(Trail::len).unwrap_or(0)
    }

    fn entries(&self, output: &str) -> (Vec<NodeId>, Option<usize>) {
        self.by_output
            .get(output)
            .map(|trail| (trail.entries(), trail.cursor_index()))
            .unwrap_or_default()
    }

    fn step(&mut self, output: &str, direction: halley_config::TrailDirection) -> Option<NodeId> {
        let wrap = self.config.wrap;
        let trail = self.by_output.get_mut(output)?;
        match (direction, wrap) {
            (halley_config::TrailDirection::Previous, true) => trail.back_wrapping(),
            (halley_config::TrailDirection::Previous, false) => trail.back(),
            (halley_config::TrailDirection::Next, true) => trail.forward_wrapping(),
            (halley_config::TrailDirection::Next, false) => trail.forward(),
        }
    }

    fn seek_index(&mut self, output: &str, index: usize) -> Option<NodeId> {
        self.by_output.get_mut(output)?.seek_to_index(index)
    }

    fn seek_node(&mut self, output: &str, id: NodeId) -> bool {
        self.by_output
            .get_mut(output)
            .is_some_and(|trail| trail.seek_to_node(id))
    }
}

impl<D: SessionDriver> Session<D> {
    pub(crate) fn record_trail_focus(&mut self, id: NodeId) {
        let Some(record) = self.nodes.record(id) else {
            return;
        };
        if !record.attached
            || !self.nodes.field().is_visible(id)
            || self.clusters.is_member(id)
            || self.clusters.active_on(&record.output).is_some()
        {
            return;
        }
        let output = record.output.clone();
        self.trail.record(&output, id);
    }

    pub(crate) fn forget_trail_node(&mut self, id: NodeId) {
        self.trail.forget(id);
    }
}

fn resolve_output<D: SessionDriver>(
    session: &Session<D>,
    requested: Option<&str>,
) -> Result<String, String> {
    if let Some(requested) = requested {
        return session
            .wayland
            .space
            .outputs()
            .any(|output| output.name() == requested)
            .then(|| requested.to_string())
            .ok_or_else(|| format!("unknown output {requested:?}"));
    }
    Ok(crate::wayland::focus::selected_output(&session.wayland)
        .map(|output| output.name())
        .unwrap_or_else(|| session.driver.primary_output().name()))
}

fn eligible<D: SessionDriver>(session: &Session<D>, output: &str, id: NodeId) -> bool {
    session.nodes.record(id).is_some_and(|record| {
        record.attached
            && record.output == output
            && session.nodes.field().is_visible(id)
            && !session.clusters.is_member(id)
    })
}

fn next_target<D: SessionDriver>(
    session: &mut Session<D>,
    output: &str,
    direction: halley_config::TrailDirection,
) -> Option<NodeId> {
    let current = session.nodes.focused();
    let mut remaining = session.trail.len(output).max(1);
    while remaining > 0 {
        remaining -= 1;
        let id = session.trail.step(output, direction)?;
        if !eligible(session, output, id) {
            session.trail.forget(id);
            continue;
        }
        if current == Some(id) {
            continue;
        }
        return Some(id);
    }
    None
}

pub(crate) fn navigate<D: SessionDriver>(
    session: &mut Session<D>,
    direction: halley_config::TrailDirection,
    requested_output: Option<&str>,
) -> Result<(), String> {
    let output = resolve_output(session, requested_output)?;
    if session.clusters.active_on(&output).is_some() {
        return Err(format!(
            "trail navigation is unavailable while a cluster is active on output {output}"
        ));
    }
    let id = next_target(session, &output, direction).ok_or_else(|| match direction {
        halley_config::TrailDirection::Previous => {
            format!("no previous trail entry on output {output}")
        }
        halley_config::TrailDirection::Next => format!("no next trail entry on output {output}"),
    })?;
    focus_target(session, id)
}

fn focus_target<D: SessionDriver>(session: &mut Session<D>, id: NodeId) -> Result<(), String> {
    session.trail.recording_suspended = true;
    let focused =
        crate::nodes::focus_or_reveal_node(session, id, SERIAL_COUNTER.next_serial(), true);
    session.trail.recording_suspended = false;
    focused
        .then_some(())
        .ok_or_else(|| format!("failed to focus trail node {id}"))
}

pub(crate) fn handle_request<D: SessionDriver>(
    session: &mut Session<D>,
    request: halley_ipc::TrailRequest,
) -> halley_ipc::Response {
    session.nodes.sync_from_space(&session.wayland.space);
    match request {
        halley_ipc::TrailRequest::Previous { output } => respond_ack(navigate(
            session,
            halley_config::TrailDirection::Previous,
            output.as_deref(),
        )),
        halley_ipc::TrailRequest::Next { output } => respond_ack(navigate(
            session,
            halley_config::TrailDirection::Next,
            output.as_deref(),
        )),
        halley_ipc::TrailRequest::List { output } => list(session, output.as_deref()),
        halley_ipc::TrailRequest::Goto { target, output } => {
            goto(session, target, output.as_deref())
        }
    }
}

fn list<D: SessionDriver>(
    session: &mut Session<D>,
    requested: Option<&str>,
) -> halley_ipc::Response {
    let output = match resolve_output(session, requested) {
        Ok(output) => output,
        Err(error) => return halley_ipc::Response::Error(error),
    };
    let stale = session
        .trail
        .entries(&output)
        .0
        .into_iter()
        .filter(|id| !eligible(session, &output, *id))
        .collect::<Vec<_>>();
    for id in stale {
        session.trail.forget(id);
    }
    let (ids, cursor_index) = session.trail.entries(&output);
    let entries = ids
        .into_iter()
        .enumerate()
        .filter_map(|(index, id)| {
            crate::nodes::ipc::node_info(session, id).map(|node| halley_ipc::TrailEntryInfo {
                index,
                cursor: cursor_index == Some(index),
                node,
            })
        })
        .collect();
    halley_ipc::Response::TrailList(halley_ipc::TrailListResponse {
        output,
        cursor_index,
        entries,
    })
}

fn goto<D: SessionDriver>(
    session: &mut Session<D>,
    target: halley_ipc::TrailTarget,
    requested: Option<&str>,
) -> halley_ipc::Response {
    let output = match resolve_output(session, requested) {
        Ok(output) => output,
        Err(error) => return halley_ipc::Response::Error(error),
    };
    if session.clusters.active_on(&output).is_some() {
        return halley_ipc::Response::Error(format!(
            "trail navigation is unavailable while a cluster is active on output {output}"
        ));
    }
    let id = match target {
        halley_ipc::TrailTarget::Index(index) => session.trail.seek_index(&output, index),
        halley_ipc::TrailTarget::Selector(selector) => {
            match crate::nodes::ipc::resolve(session, Some(&selector), Some(&output)) {
                Ok(id) if session.trail.seek_node(&output, id) => Some(id),
                Ok(_) => None,
                Err(error) => return halley_ipc::Response::Error(error),
            }
        }
    };
    let Some(id) = id else {
        return halley_ipc::Response::Error(format!("trail target not found on output {output}"));
    };
    respond_ack(focus_target(session, id))
}

fn respond_ack(result: Result<(), String>) -> halley_ipc::Response {
    match result {
        Ok(()) => halley_ipc::Response::Ack,
        Err(error) => halley_ipc::Response::Error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_per_output_and_truncates_on_reload() {
        let mut state = TrailState::new(halley_config::Trail {
            history_length: 3,
            wrap: true,
        });
        state.record("DP-1", NodeId::new(1));
        state.record("DP-1", NodeId::new(2));
        state.record("DP-2", NodeId::new(3));
        state.record("DP-1", NodeId::new(4));
        assert_eq!(
            state.entries("DP-1").0,
            vec![NodeId::new(1), NodeId::new(2), NodeId::new(4)]
        );
        assert_eq!(state.entries("DP-2").0, vec![NodeId::new(3)]);

        state.reload(halley_config::Trail {
            history_length: 2,
            wrap: false,
        });
        assert_eq!(
            state.entries("DP-1").0,
            vec![NodeId::new(2), NodeId::new(4)]
        );
    }

    #[test]
    fn suspended_recording_preserves_forward_history() {
        let mut state = TrailState::new(halley_config::Trail::default());
        state.record("DP-1", NodeId::new(1));
        state.record("DP-1", NodeId::new(2));
        assert_eq!(
            state.step("DP-1", halley_config::TrailDirection::Previous),
            Some(NodeId::new(1))
        );
        state.recording_suspended = true;
        state.record("DP-1", NodeId::new(1));
        state.recording_suspended = false;
        assert_eq!(
            state.step("DP-1", halley_config::TrailDirection::Next),
            Some(NodeId::new(2))
        );
    }
}
