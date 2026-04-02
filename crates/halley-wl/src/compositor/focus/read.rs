use std::collections::HashMap;
use std::time::Instant;

use halley_config::{CloseRestorePanMode, RuntimeTuning};
use halley_core::field::{Field, NodeId, Vec2};

use crate::compositor::clusters::state::ClusterState;
use crate::compositor::focus::state::FocusState;
use crate::compositor::fullscreen::state::FullscreenState;
use crate::compositor::monitor::state::MonitorState;
use crate::compositor::root::Halley;

pub(crate) struct FocusReadContext<'a> {
    field: &'a Field,
    cluster_state: &'a ClusterState,
    focus_state: &'a FocusState,
    fullscreen_state: &'a FullscreenState,
    monitor_state: &'a MonitorState,
    tuning: &'a RuntimeTuning,
    viewport: halley_core::viewport::Viewport,
    usable_viewports: HashMap<String, halley_core::viewport::Viewport>,
    focused_monitor: &'a str,
}

enum CloseRestorePanPlan {
    None,
    PanTo(Vec2),
}

impl<'a> FocusReadContext<'a> {
    fn surface_node_matches(
        &self,
        id: NodeId,
        allow_active: bool,
        allow_node: bool,
        monitor: Option<&str>,
    ) -> bool {
        self.field.node(id).is_some_and(|n| {
            self.field.is_visible(id)
                && n.kind == halley_core::field::NodeKind::Surface
                && match n.state {
                    halley_core::field::NodeState::Active => allow_active,
                    halley_core::field::NodeState::Node => allow_node,
                    _ => false,
                }
                && monitor.is_none_or(|monitor| {
                    self.monitor_state
                        .node_monitor
                        .get(&id)
                        .is_some_and(|m| m == monitor)
                })
        })
    }

    fn fullscreen_focus_override(&self, requested: Option<NodeId>) -> Option<NodeId> {
        match requested {
            None => self
                .fullscreen_state
                .fullscreen_active_node
                .get(self.focused_monitor)
                .copied(),
            Some(requested_id) => {
                let requested_monitor = self
                    .fullscreen_monitor_for_node(requested_id)
                    .map(str::to_string)
                    .or_else(|| self.monitor_state.node_monitor.get(&requested_id).cloned());
                let Some(requested_monitor) = requested_monitor else {
                    return requested;
                };
                let fullscreen_id = self
                    .fullscreen_state
                    .fullscreen_active_node
                    .get(requested_monitor.as_str())
                    .copied();
                let fullscreen_monitor = fullscreen_id.and_then(|fullscreen_id| {
                    self.fullscreen_monitor_for_node(fullscreen_id).or_else(|| {
                        self.monitor_state
                            .node_monitor
                            .get(&fullscreen_id)
                            .map(String::as_str)
                    })
                });
                if fullscreen_id == Some(requested_id) {
                    return requested;
                }
                let requested_monitor = self
                    .monitor_state
                    .node_monitor
                    .get(&requested_id)
                    .map(String::as_str)
                    .or(fullscreen_monitor);
                if requested_monitor == fullscreen_monitor {
                    fullscreen_id
                } else {
                    requested
                }
            }
        }
    }

    fn fullscreen_monitor_for_node(&self, id: NodeId) -> Option<&str> {
        self.fullscreen_state
            .fullscreen_active_node
            .iter()
            .find_map(|(monitor, nid)| (*nid == id).then_some(monitor.as_str()))
    }

    fn viewport_for_monitor(&self, monitor: &str) -> halley_core::viewport::Viewport {
        self.usable_viewports
            .get(monitor)
            .copied()
            .unwrap_or_else(|| {
                if self.monitor_state.current_monitor == monitor {
                    self.viewport
                } else {
                    self.monitor_state
                        .monitors
                        .get(monitor)
                        .map(|space| space.viewport)
                        .unwrap_or(self.viewport)
                }
            })
    }

    fn surface_is_fully_visible_on_monitor(
        &self,
        st: &Halley,
        monitor: &str,
        id: NodeId,
    ) -> bool {
        let Some(node) = self.field.node(id) else {
            return false;
        };
        let ext = st.spawn_obstacle_extents_for_node(node);
        let viewport = self.viewport_for_monitor(monitor);
        let min_x = viewport.center.x - viewport.size.x * 0.5;
        let max_x = viewport.center.x + viewport.size.x * 0.5;
        let min_y = viewport.center.y - viewport.size.y * 0.5;
        let max_y = viewport.center.y + viewport.size.y * 0.5;

        node.pos.x - ext.left >= min_x
            && node.pos.x + ext.right <= max_x
            && node.pos.y - ext.top >= min_y
            && node.pos.y + ext.bottom <= max_y
    }

    fn minimal_reveal_center_for_surface_on_monitor(
        &self,
        st: &Halley,
        monitor: &str,
        id: NodeId,
    ) -> Option<Vec2> {
        let node = self.field.node(id)?;
        let ext = st.spawn_obstacle_extents_for_node(node);
        let viewport = self.viewport_for_monitor(monitor);
        let margin_x = (viewport.size.x * 0.08).clamp(32.0, 160.0);
        let margin_y = (viewport.size.y * 0.08).clamp(32.0, 120.0);
        let avail_w = (viewport.size.x - margin_x * 2.0).max(1.0);
        let avail_h = (viewport.size.y - margin_y * 2.0).max(1.0);

        let mut target = viewport.center;
        if ext.left + ext.right > avail_w {
            target.x = node.pos.x;
        } else {
            let min_x = viewport.center.x - viewport.size.x * 0.5 + margin_x;
            let max_x = viewport.center.x + viewport.size.x * 0.5 - margin_x;
            let left = node.pos.x - ext.left;
            let right = node.pos.x + ext.right;
            if left < min_x {
                target.x += left - min_x;
            } else if right > max_x {
                target.x += right - max_x;
            }
        }

        if ext.top + ext.bottom > avail_h {
            target.y = node.pos.y;
        } else {
            let min_y = viewport.center.y - viewport.size.y * 0.5 + margin_y;
            let max_y = viewport.center.y + viewport.size.y * 0.5 - margin_y;
            let top = node.pos.y - ext.top;
            let bottom = node.pos.y + ext.bottom;
            if top < min_y {
                target.y += top - min_y;
            } else if bottom > max_y {
                target.y += bottom - max_y;
            }
        }

        Some(target)
    }

    fn close_restore_pan_plan(
        &self,
        st: &Halley,
        monitor: &str,
        id: NodeId,
    ) -> CloseRestorePanPlan {
        if self
            .cluster_state
            .active_cluster_workspaces
            .contains_key(monitor)
        {
            return CloseRestorePanPlan::None;
        }
        if !self.tuning.close_restore_focus {
            return CloseRestorePanPlan::None;
        }

        match self.tuning.close_restore_pan {
            CloseRestorePanMode::Never => CloseRestorePanPlan::None,
            CloseRestorePanMode::Always => self
                .field
                .node(id)
                .map(|node| CloseRestorePanPlan::PanTo(node.pos))
                .unwrap_or(CloseRestorePanPlan::None),
            CloseRestorePanMode::IfOffscreen => {
                if self.surface_is_fully_visible_on_monitor(st, monitor, id) {
                    CloseRestorePanPlan::None
                } else {
                    self.minimal_reveal_center_for_surface_on_monitor(st, monitor, id)
                        .map(CloseRestorePanPlan::PanTo)
                        .unwrap_or(CloseRestorePanPlan::None)
                }
            }
        }
    }

    fn last_focused_active_surface_node(&self) -> Option<NodeId> {
        if let Some(id) = self.focus_state.primary_interaction_focus
            && self.surface_node_matches(id, true, false, None)
        {
            return Some(id);
        }
        self.focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                self.surface_node_matches(id, true, false, None)
                    .then_some((id, at))
            })
            .max_by_key(|entry: &(NodeId, u64)| (entry.1, entry.0.as_u64()))
            .map(|(id, _)| id)
    }

    fn last_focused_surface_node(&self) -> Option<NodeId> {
        if let Some(id) = self.focus_state.primary_interaction_focus
            && self.surface_node_matches(id, true, true, None)
        {
            return Some(id);
        }
        self.focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                self.surface_node_matches(id, true, true, None)
                    .then_some((id, at))
            })
            .max_by_key(|entry: &(NodeId, u64)| (entry.1, entry.0.as_u64()))
            .map(|(id, _)| id)
    }

    fn last_focused_surface_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        if let Some(id) = self.focus_state.monitor_focus.get(monitor).copied()
            && self.surface_node_matches(id, true, true, Some(monitor))
        {
            return Some(id);
        }
        self.focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                self.surface_node_matches(id, true, true, Some(monitor))
                    .then_some((id, at))
            })
            .max_by_key(|entry: &(NodeId, u64)| (entry.1, entry.0.as_u64()))
            .map(|(id, _)| id)
    }

    fn last_input_surface_node(&self) -> Option<NodeId> {
        if let Some(id) = self.focus_state.primary_interaction_focus
            && self.surface_node_matches(id, true, true, None)
        {
            return Some(id);
        }
        self.focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                self.surface_node_matches(id, true, true, None)
                    .then_some((id, at))
            })
            .max_by_key(|entry: &(NodeId, u64)| (entry.1, entry.0.as_u64()))
            .map(|(id, _)| id)
    }

    fn last_input_surface_node_for_monitor(&self, monitor: &str) -> Option<NodeId> {
        let primary = self.focus_state.primary_interaction_focus.and_then(|id| {
            self.surface_node_matches(id, true, true, Some(monitor))
                .then_some((id, u64::MAX))
        });
        let monitor_focus = self
            .focus_state
            .monitor_focus
            .get(monitor)
            .copied()
            .and_then(|id| {
                self.surface_node_matches(id, true, true, Some(monitor))
                    .then_some((
                        id,
                        self.focus_state
                            .last_surface_focus_ms
                            .get(&id)
                            .copied()
                            .unwrap_or(0),
                    ))
            });
        primary
            .into_iter()
            .chain(monitor_focus)
            .chain(
                self.focus_state
                    .last_surface_focus_ms
                    .iter()
                    .filter_map(|(&id, &at)| {
                        self.surface_node_matches(id, true, true, Some(monitor))
                            .then_some((id, at))
                    }),
            )
            .max_by_key(|entry: &(NodeId, u64)| (entry.1, entry.0.as_u64()))
            .map(|(id, _)| id)
    }
}

pub(crate) fn focus_read_context(st: &Halley) -> FocusReadContext<'_> {
    FocusReadContext {
        field: &st.model.field,
        cluster_state: &st.model.cluster_state,
        focus_state: &st.model.focus_state,
        fullscreen_state: &st.model.fullscreen_state,
        monitor_state: &st.model.monitor_state,
        tuning: &st.runtime.tuning,
        viewport: st.model.viewport,
        usable_viewports: st
            .model
            .monitor_state
            .monitors
            .keys()
            .map(|name| (name.clone(), st.usable_viewport_for_monitor(name)))
            .collect(),
        focused_monitor: st.focused_monitor(),
    }
}

pub(crate) fn fullscreen_focus_override(st: &Halley, requested: Option<NodeId>) -> Option<NodeId> {
    focus_read_context(st).fullscreen_focus_override(requested)
}

pub(crate) fn surface_is_fully_visible_on_monitor(
    st: &Halley,
    monitor: &str,
    id: NodeId,
) -> bool {
    focus_read_context(st).surface_is_fully_visible_on_monitor(st, monitor, id)
}

pub(crate) fn minimal_reveal_center_for_surface_on_monitor(
    st: &Halley,
    monitor: &str,
    id: NodeId,
) -> Option<Vec2> {
    focus_read_context(st).minimal_reveal_center_for_surface_on_monitor(st, monitor, id)
}

pub(crate) fn maybe_pan_to_restored_focus_on_close(
    st: &mut Halley,
    monitor: &str,
    id: NodeId,
    now: Instant,
) -> bool {
    match focus_read_context(st).close_restore_pan_plan(st, monitor, id) {
        CloseRestorePanPlan::None => false,
        CloseRestorePanPlan::PanTo(target) => st.animate_viewport_center_to(target, now),
    }
}

pub(crate) fn last_focused_active_surface_node(st: &Halley) -> Option<NodeId> {
    focus_read_context(st).last_focused_active_surface_node()
}

pub(crate) fn last_focused_surface_node(st: &Halley) -> Option<NodeId> {
    focus_read_context(st).last_focused_surface_node()
}

pub(crate) fn last_focused_surface_node_for_monitor(st: &Halley, monitor: &str) -> Option<NodeId> {
    focus_read_context(st).last_focused_surface_node_for_monitor(monitor)
}

pub(crate) fn last_input_surface_node(st: &Halley) -> Option<NodeId> {
    focus_read_context(st).last_input_surface_node()
}

pub(crate) fn last_input_surface_node_for_monitor(st: &Halley, monitor: &str) -> Option<NodeId> {
    focus_read_context(st).last_input_surface_node_for_monitor(monitor)
}
