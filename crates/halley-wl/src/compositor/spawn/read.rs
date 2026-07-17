use std::collections::HashMap;

use halley_config::{InputFocusMode, PanToNewMode};
use halley_core::field::{NodeId, Vec2};

use crate::compositor::focus::state::FocusState;
use crate::compositor::monitor::state::MonitorState;
use crate::compositor::root::Halley;
use crate::compositor::spawn::state::{MonitorSpawnState, SpawnState};

pub(crate) struct SpawnReadContext<'a> {
    field: &'a halley_core::field::Field,
    focus_state: &'a FocusState,
    monitor_state: &'a MonitorState,
    spawn_state: &'a SpawnState,
    viewport: halley_core::viewport::Viewport,
    usable_viewports: HashMap<String, halley_core::viewport::Viewport>,
    focused_monitor: &'a str,
    interaction_monitor: &'a str,
    pointer_monitor: Option<String>,
    input_focus_mode: InputFocusMode,
    /// An explicit `monitor-focus` keybind has pinned the spawn target to
    /// `focused_monitor` until the pointer moves; overrides hover focus-mode.
    monitor_focus_pinned: bool,
    pan_to_new: PanToNewMode,
}

pub(crate) enum RevealNewToplevelPlan {
    AlreadyQueued,
    ActivateNow,
    QueuePan { target_center: Vec2 },
}

impl<'a> SpawnReadContext<'a> {
    pub(crate) fn viewport_center_for_monitor(&self, monitor: &str) -> Vec2 {
        if let Some(viewport) = self.usable_viewports.get(monitor) {
            return viewport.center;
        }
        if self.monitor_state.current_monitor == monitor {
            return self.viewport.center;
        }
        self.monitor_state
            .monitors
            .get(monitor)
            .map(|space| space.viewport.center)
            .unwrap_or(self.viewport.center)
    }

    pub(crate) fn resolve_spawn_target_monitor(&self) -> String {
        // A deliberate monitor-focus keybind wins over hover focus-mode until the
        // pointer moves, so spawning after switching monitors lands on the
        // keyboard-focused monitor instead of wherever the cursor is parked.
        if self.input_focus_mode == InputFocusMode::Hover
            && !self.monitor_focus_pinned
            && let Some(pointer_monitor) = self.pointer_monitor.as_deref()
            && self.monitor_state.monitors.contains_key(pointer_monitor)
        {
            return pointer_monitor.to_string();
        }

        let focused = self.focused_monitor.to_string();
        if self.monitor_state.monitors.contains_key(focused.as_str()) {
            return focused;
        }
        self.interaction_monitor.to_string()
    }

    fn last_input_surface_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        let primary = self.focus_state.primary_interaction_focus.and_then(|id| {
            self.field.node(id).and_then(|n| {
                (self.field.is_visible(id)
                    && n.kind == halley_core::field::NodeKind::Surface
                    && self
                        .monitor_state
                        .node_monitor
                        .get(&id)
                        .is_some_and(|m| m == monitor))
                .then_some((id, u64::MAX))
            })
        });
        let monitor_focus = self
            .focus_state
            .monitor_focus
            .get(monitor)
            .copied()
            .and_then(|id| {
                self.field.node(id).and_then(|n| {
                    (self.field.is_visible(id)
                        && n.kind == halley_core::field::NodeKind::Surface
                        && self
                            .monitor_state
                            .node_monitor
                            .get(&id)
                            .is_some_and(|m| m == monitor))
                    .then_some((
                        id,
                        self.focus_state
                            .last_surface_focus_ms
                            .get(&id)
                            .copied()
                            .unwrap_or(0),
                    ))
                })
            });
        primary
            .into_iter()
            .chain(monitor_focus)
            .chain(
                self.focus_state
                    .last_surface_focus_ms
                    .iter()
                    .filter_map(|(&id, &at)| {
                        self.field.node(id).and_then(|n| {
                            (self.field.is_visible(id)
                                && n.kind == halley_core::field::NodeKind::Surface
                                && self
                                    .monitor_state
                                    .node_monitor
                                    .get(&id)
                                    .is_some_and(|m| m == monitor))
                            .then_some((id, at))
                        })
                    }),
            )
            .max_by_key(|entry: &(NodeId, u64)| (entry.1, entry.0.as_u64()))
            .map(|(id, _)| id)
    }

    pub(crate) fn current_spawn_focus(&self, monitor: &str) -> (Option<NodeId>, Vec2) {
        let spawn = self.spawn_monitor_state(monitor);
        let viewport_center = self.viewport_center_for_monitor(monitor);
        if spawn.spawn_anchor_mode == crate::compositor::spawn::state::SpawnAnchorMode::View {
            return (None, spawn.spawn_view_anchor);
        }
        if let Some(id) = self.last_input_surface_node_for_monitor(monitor)
            && let Some(node) = self.field.node(id)
        {
            return (Some(id), node.pos);
        }
        (None, viewport_center)
    }

    fn spawn_monitor_state(&self, monitor: &str) -> MonitorSpawnState {
        self.spawn_state
            .per_monitor
            .get(monitor)
            .cloned()
            .unwrap_or_else(|| MonitorSpawnState::new(self.viewport_center_for_monitor(monitor)))
    }

    pub(crate) fn reveal_new_toplevel_plan(
        &self,
        st: &Halley,
        id: NodeId,
        is_transient: bool,
    ) -> RevealNewToplevelPlan {
        if is_transient {
            return RevealNewToplevelPlan::ActivateNow;
        }
        if self
            .spawn_state
            .active_spawn_pan
            .is_some_and(|active| active.node_id == id)
            || self
                .spawn_state
                .pending_spawn_pan_queue
                .iter()
                .any(|pending| pending.node_id == id)
        {
            return RevealNewToplevelPlan::AlreadyQueued;
        }

        let monitor = self
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| self.focused_monitor.to_string());
        if st
            .active_cluster_workspace_for_monitor(monitor.as_str())
            .is_some()
        {
            return RevealNewToplevelPlan::ActivateNow;
        }
        if crate::window::node_is_game_like(st, id) {
            return RevealNewToplevelPlan::ActivateNow;
        }
        if !self.pinned_landmark_blocks_spawn_center(st, monitor.as_str()) {
            return RevealNewToplevelPlan::ActivateNow;
        }
        let target_center = match self.pan_to_new {
            PanToNewMode::Never => return RevealNewToplevelPlan::ActivateNow,
            PanToNewMode::Always => match self.field.node(id) {
                Some(node) => node.pos,
                None => return RevealNewToplevelPlan::ActivateNow,
            },
            PanToNewMode::IfNeeded => {
                if st.surface_is_fully_visible_on_monitor(monitor.as_str(), id) {
                    return RevealNewToplevelPlan::ActivateNow;
                }
                match crate::compositor::focus::read::minimal_reveal_center_for_surface_on_monitor(
                    st,
                    monitor.as_str(),
                    id,
                ) {
                    Some(center) => center,
                    None => return RevealNewToplevelPlan::ActivateNow,
                }
            }
        };
        RevealNewToplevelPlan::QueuePan { target_center }
    }

    fn pinned_landmark_blocks_spawn_center(&self, st: &Halley, monitor: &str) -> bool {
        let center = self.viewport_center_for_monitor(monitor);
        self.field.nodes().values().any(|node| {
            node.pinned
                && matches!(
                    node.state,
                    halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
                )
                && self.field.is_visible(node.id)
                && self
                    .monitor_state
                    .node_monitor
                    .get(&node.id)
                    .is_some_and(|node_monitor| node_monitor == monitor)
                && {
                    let ext = st.collision_extents_for_node(node);
                    center.x >= node.pos.x - ext.left
                        && center.x <= node.pos.x + ext.right
                        && center.y >= node.pos.y - ext.top
                        && center.y <= node.pos.y + ext.bottom
                }
        })
    }
}

pub(crate) fn spawn_read_context(st: &Halley) -> SpawnReadContext<'_> {
    SpawnReadContext {
        field: &st.model.field,
        focus_state: &st.model.focus_state,
        monitor_state: &st.model.monitor_state,
        spawn_state: &st.model.spawn_state,
        viewport: st.model.viewport,
        usable_viewports: st
            .model
            .monitor_state
            .monitors
            .keys()
            .map(|name| (name.clone(), st.usable_viewport_for_monitor(name)))
            .collect(),
        focused_monitor: st.focused_monitor(),
        interaction_monitor: st.interaction_monitor(),
        pointer_monitor: st
            .input
            .interaction_state
            .cursor
            .last_screen_global
            .and_then(|(sx, sy)| st.monitor_for_screen(sx, sy)),
        input_focus_mode: st.runtime.tuning.input.focus_mode,
        monitor_focus_pinned: st.input.interaction_state.monitor_focus_pinned,
        pan_to_new: st.runtime.tuning.pan_to_new,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::reexports::wayland_server::Display;

    #[test]
    fn game_like_toplevel_does_not_queue_reveal_pan() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.pan_to_new = PanToNewMode::Always;
        let mut st = Halley::new_for_test(&dh, tuning);
        let id = st.model.field.spawn_surface(
            "game",
            Vec2 { x: 4000.0, y: 0.0 },
            Vec2 {
                x: 1280.0,
                y: 720.0,
            },
        );
        st.assign_node_to_current_monitor(id);
        st.model
            .node_app_ids
            .insert(id, "steam_app_123".to_string());

        match spawn_read_context(&st).reveal_new_toplevel_plan(&st, id, false) {
            RevealNewToplevelPlan::ActivateNow => {}
            RevealNewToplevelPlan::AlreadyQueued | RevealNewToplevelPlan::QueuePan { .. } => {
                panic!("game-like windows should not queue reveal pan")
            }
        }
    }
}
