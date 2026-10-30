use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use halley_core::cluster::layout::ClusterWorkspaceLayoutKind;
use halley_core::cluster::layout::{ClusterWorkspaceLayoutResult, layout_cluster_workspace};
use halley_core::cluster::tiling::Rect as LayoutRect;
use halley_core::cluster::{ClusterId, ClusterRegistry};
use halley_core::field::{Field, NodeId, Vec2};
use smithay::utils::{Logical, Point, Rectangle};

mod bloom;
mod creation;
mod floating;
mod ipc;
mod membership;
mod overflow;
pub mod render;
mod surfaces;
mod transition;

pub use bloom::{DETACH_HOLD_DURATION, TokenLayout};
pub(crate) use creation::DraftBuild;
pub use creation::{CreationState, NameInput, PreparedCreation};
pub use ipc::handle_request;
pub use overflow::REVEAL_EDGE_PX;

pub const CORE_DIAMETER_PX: f32 = 68.0;
pub const ACTION_BUTTON_DIAMETER_PX: i32 = 26;
const ACTION_BUTTON_CORE_GAP_PX: i32 = 6;
const ACTION_BUTTON_STACK_GAP_PX: i32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClusterActionControl {
    Close,
    Edit,
}

pub(crate) fn action_button_rects(
    core_center: Point<i32, Logical>,
    output_geometry: Rectangle<i32, Logical>,
) -> [(ClusterActionControl, Rectangle<i32, Logical>); 2] {
    let radius = ACTION_BUTTON_DIAMETER_PX / 2;
    let offset = (CORE_DIAMETER_PX.round() as i32) / 2 + ACTION_BUTTON_CORE_GAP_PX + radius;
    let right = core_center.x + offset;
    let left = core_center.x - offset;
    let center_x = if right + radius <= output_geometry.loc.x + output_geometry.size.w {
        right
    } else {
        left
    };
    let stack_offset = (ACTION_BUTTON_DIAMETER_PX + ACTION_BUTTON_STACK_GAP_PX) / 2;
    let min_center_y = output_geometry.loc.y + radius + stack_offset;
    let max_center_y = output_geometry.loc.y + output_geometry.size.h - radius - stack_offset;
    let stack_center_y = core_center
        .y
        .clamp(min_center_y, max_center_y.max(min_center_y));
    let rect = |center_y| {
        Rectangle::new(
            (center_x - radius, center_y - radius).into(),
            (ACTION_BUTTON_DIAMETER_PX, ACTION_BUTTON_DIAMETER_PX).into(),
        )
    };
    [
        (
            ClusterActionControl::Close,
            rect(stack_center_y - stack_offset),
        ),
        (
            ClusterActionControl::Edit,
            rect(stack_center_y + stack_offset),
        ),
    ]
}

#[derive(Clone, Debug)]
pub struct ClusterMetadata {
    pub name: String,
    pub output: String,
    pub layout: ClusterWorkspaceLayoutKind,
    pub core: Option<NodeId>,
    pub core_position: Vec2,
}

#[derive(Clone, Debug)]
struct JoinCandidate {
    member: NodeId,
    cluster_id: ClusterId,
    output: String,
    started_at: Duration,
    ready: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct JoinReadiness {
    pub(crate) member: NodeId,
    pub(crate) cluster_id: ClusterId,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct JoinContact {
    pub(crate) center: Vec2,
    pub(crate) member_left: f32,
    pub(crate) member_right: f32,
    pub(crate) member_top: f32,
    pub(crate) member_bottom: f32,
    pub(crate) core_radius: f32,
    pub(crate) gap: f32,
}

#[derive(Clone, Debug)]
struct DraggedWindow {
    member: NodeId,
    presentation: Option<(String, Rectangle<i32, Logical>)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WindowPresentation {
    Field,
    Hidden,
    PointerDrag {
        rect: Rectangle<i32, Logical>,
    },
    Workspace {
        rect: Rectangle<i32, Logical>,
        depth: usize,
        alpha: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackCycleOutcome {
    NotActive,
    Unchanged,
    Cycled(NodeId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClusterDragMember {
    pub cluster_id: ClusterId,
    pub node_id: NodeId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClusterDissolution {
    pub(crate) output: String,
    pub(crate) members: Vec<NodeId>,
    pub(crate) surface_restores: Vec<(NodeId, Rectangle<i32, Logical>)>,
}

/// Owns every cluster-specific state transition. Field and Nodes remain
/// unaware of membership, slots, workspace modes, naming, and presentation.
pub struct ClusterSystem {
    registry: ClusterRegistry,
    metadata: HashMap<ClusterId, ClusterMetadata>,
    slots: HashMap<String, Vec<ClusterId>>,
    active: HashMap<String, ClusterId>,
    transitions: HashMap<String, transition::WorkspaceTransition>,
    reflows: HashMap<String, transition::ReflowTransition>,
    admission_floats: HashSet<NodeId>,
    member_floats: floating::ClusterFloatingState,
    dragged_window: Option<DraggedWindow>,
    join_candidate: Option<JoinCandidate>,
    hovered_core: Option<ClusterId>,
    bloom: bloom::BloomState,
    overflow: overflow::OverflowState,
    label_hover: RefCell<HashMap<ClusterId, f32>>,
    overlay_hovered: Option<(String, NodeId)>,
    overlay_label_hover: RefCell<HashMap<NodeId, f32>>,
    creation: Option<CreationState>,
    pending_draft: Option<DraftBuild>,
    next_draft_id: u64,
    surfaces: surfaces::WorkspaceSurfaceState,
    config: halley_config::Clusters,
    animations: halley_config::ClusterAnimation,
}

impl ClusterSystem {
    pub fn new(
        config: halley_config::Clusters,
        animations: halley_config::ClusterAnimation,
    ) -> Self {
        Self {
            registry: ClusterRegistry::new(),
            metadata: HashMap::new(),
            slots: HashMap::new(),
            active: HashMap::new(),
            transitions: HashMap::new(),
            reflows: HashMap::new(),
            admission_floats: HashSet::new(),
            member_floats: floating::ClusterFloatingState::default(),
            dragged_window: None,
            join_candidate: None,
            hovered_core: None,
            bloom: bloom::BloomState::default(),
            overflow: overflow::OverflowState::default(),
            label_hover: RefCell::new(HashMap::new()),
            overlay_hovered: None,
            overlay_label_hover: RefCell::new(HashMap::new()),
            creation: None,
            pending_draft: None,
            next_draft_id: 1,
            surfaces: surfaces::WorkspaceSurfaceState::default(),
            config,
            animations,
        }
    }

    pub fn reload(
        &mut self,
        config: halley_config::Clusters,
        animations: halley_config::ClusterAnimation,
    ) -> bool {
        let changed = self.config != config || self.animations != animations;
        self.config = config;
        self.animations = animations;
        changed
    }

    pub fn config(&self) -> halley_config::Clusters {
        self.config
    }

    pub fn registry(&self) -> &ClusterRegistry {
        &self.registry
    }

    pub fn metadata(&self, id: ClusterId) -> Option<&ClusterMetadata> {
        self.metadata.get(&id)
    }

    pub fn active_on(&self, output: &str) -> Option<ClusterId> {
        self.active.get(output).copied()
    }

    pub fn hovered_core(&self) -> Option<ClusterId> {
        self.hovered_core
    }

    pub fn set_hovered_core(&mut self, hovered: Option<ClusterId>, now: Duration) -> bool {
        if self.hovered_core == hovered {
            return false;
        }
        self.hovered_core = hovered;
        let metadata = &self.metadata;
        self.bloom.set_hovered(hovered, now, |id| {
            metadata.get(&id).map(|metadata| metadata.output.clone())
        });
        true
    }

    pub fn move_core(&mut self, id: ClusterId, output: &str, position: Vec2) -> bool {
        if self.active.values().any(|active| *active == id) {
            return false;
        }
        let Some(previous_output) = self
            .metadata
            .get(&id)
            .map(|metadata| metadata.output.clone())
        else {
            return false;
        };
        if previous_output != output {
            let destination = self.slots.entry(output.to_string()).or_default();
            if destination.len() >= 10 {
                return false;
            }
            destination.push(id);
            if let Some(previous) = self.slots.get_mut(&previous_output) {
                previous.retain(|candidate| *candidate != id);
                if previous.is_empty() {
                    self.slots.remove(&previous_output);
                }
            }
        }
        let Some(metadata) = self.metadata.get_mut(&id) else {
            return false;
        };
        metadata.output = output.to_string();
        metadata.core_position = position;
        true
    }

    pub fn label_hover_mix(&self, id: ClusterId, highlighted: bool) -> f32 {
        let mut states = self.label_hover.borrow_mut();
        let mix = states.entry(id).or_insert(0.0);
        let target = if highlighted { 1.0 } else { 0.0 };
        let rate = if highlighted { 0.06 } else { 0.10 };
        *mix += (target - *mix) * rate;
        if (*mix - target).abs() < 0.002 {
            *mix = target;
        }
        *mix
    }

    pub fn set_overlay_hovered(&mut self, hovered: Option<(String, NodeId)>) -> bool {
        if self.overlay_hovered == hovered {
            return false;
        }
        self.overlay_hovered = hovered;
        true
    }

    pub fn overlay_hovered_on_output(&self, output: &str) -> Option<NodeId> {
        self.overlay_hovered
            .as_ref()
            .filter(|(candidate, _)| candidate == output)
            .map(|(_, member)| *member)
    }

    pub fn overlay_label_hover_mix(&self, id: NodeId, highlighted: bool) -> f32 {
        let mut states = self.overlay_label_hover.borrow_mut();
        let mix = states.entry(id).or_insert(0.0);
        let target = if highlighted { 1.0 } else { 0.0 };
        let rate = if highlighted { 0.10 } else { 0.16 };
        *mix += (target - *mix) * rate;
        if (*mix - target).abs() < 0.002 {
            *mix = target;
        }
        *mix
    }

    pub fn labels_animating_on_output(
        &self,
        output: &str,
        policy: halley_config::NodeDisplayPolicy,
    ) -> bool {
        // The field's cluster cores (and their labels) are not part of the
        // scene while a cluster workspace owns this output. Do not keep the
        // output's redraw loop alive for a hover mix that cannot advance until
        // the field scene becomes visible again.
        let core_animating = if policy == halley_config::NodeDisplayPolicy::Hover
            && self.active_on(output).is_none()
        {
            let states = self.label_hover.borrow();
            self.clusters_for_output(output).any(|(_, id, _)| {
                let mix = states.get(&id).copied().unwrap_or(0.0);
                let target = if self.hovered_core == Some(id)
                    && self.bloom.cluster_on_output(output) != Some(id)
                {
                    1.0
                } else {
                    0.0
                };
                (mix - target).abs() > 0.002
            })
        } else {
            false
        };
        let target = self.overlay_hovered_on_output(output);
        let overlay_present =
            self.bloom.cluster_on_output(output).is_some() || self.overflow.is_revealed(output);
        if !overlay_present {
            let members = self
                .clusters_for_output(output)
                .flat_map(|(_, id, _)| self.member_ids(id))
                .collect::<Vec<_>>();
            let mut states = self.overlay_label_hover.borrow_mut();
            for member in members {
                states.remove(&member);
            }
        }
        let states = self.overlay_label_hover.borrow();
        let overlay_animating = target.is_some_and(|id| !states.contains_key(&id))
            || self.clusters_for_output(output).any(|(_, id, _)| {
                self.member_ids(id).into_iter().any(|member| {
                    let mix = states.get(&member).copied().unwrap_or(0.0);
                    let target_mix = if target == Some(member) { 1.0 } else { 0.0 };
                    (mix - target_mix).abs() > 0.002
                })
            });
        core_animating || overlay_animating
    }

    pub fn cycle_active_layout(
        &mut self,
        output: &str,
        work_area: Rectangle<i32, Logical>,
        now: Duration,
    ) -> bool {
        let Some(id) = self.active_on(output) else {
            return false;
        };
        let Some(before) = self.workspace_layout(id, work_area) else {
            return false;
        };
        let Some(metadata) = self.metadata.get_mut(&id) else {
            return false;
        };
        metadata.layout = match metadata.layout {
            ClusterWorkspaceLayoutKind::Tiling => ClusterWorkspaceLayoutKind::Stacking,
            ClusterWorkspaceLayoutKind::Stacking => ClusterWorkspaceLayoutKind::Tiling,
        };
        let layout = metadata.layout;
        let duration_ms = match layout {
            ClusterWorkspaceLayoutKind::Tiling => self.animations.tiling.reflow_duration_ms,
            ClusterWorkspaceLayoutKind::Stacking => self.animations.stacking.cycle_duration_ms,
        };
        match layout {
            ClusterWorkspaceLayoutKind::Tiling => {
                self.overflow.reveal(output, now);
            }
            ClusterWorkspaceLayoutKind::Stacking => {
                self.hide_overflow(output);
            }
        }
        self.begin_layout_reflow(output, id, before, layout, now, duration_ms);
        true
    }

    pub fn cycle_stack(
        &mut self,
        output: &str,
        direction: halley_config::FocusCycleDirection,
        work_area: Rectangle<i32, Logical>,
        now: Duration,
    ) -> StackCycleOutcome {
        let Some(id) = self.active_on(output) else {
            return StackCycleOutcome::NotActive;
        };
        if self.metadata(id).map(|metadata| metadata.layout)
            != Some(ClusterWorkspaceLayoutKind::Stacking)
        {
            return StackCycleOutcome::NotActive;
        }
        let Some(mut members) = self
            .registry
            .cluster(id)
            .map(|cluster| cluster.members().to_vec())
        else {
            return StackCycleOutcome::Unchanged;
        };
        let layout_indices = members
            .iter()
            .enumerate()
            .filter_map(|(index, member)| {
                (!self.member_floats.is_floating(*member)).then_some(index)
            })
            .collect::<Vec<_>>();
        if layout_indices.len() < 2 {
            return StackCycleOutcome::Unchanged;
        }
        let Some(before) = self.workspace_layout(id, work_area) else {
            return StackCycleOutcome::Unchanged;
        };
        let direction = match direction {
            halley_config::FocusCycleDirection::Forward => {
                halley_core::cluster::layout::ClusterCycleDirection::Next
            }
            halley_config::FocusCycleDirection::Backward => {
                halley_core::cluster::layout::ClusterCycleDirection::Prev
            }
        };
        let mut layout_members = layout_indices
            .iter()
            .map(|index| members[*index])
            .collect::<Vec<_>>();
        let Some(member) =
            halley_core::cluster::stacking::cycle_stacking_members(&mut layout_members, direction)
        else {
            return StackCycleOutcome::Unchanged;
        };
        for (index, member) in layout_indices.into_iter().zip(layout_members) {
            members[index] = member;
        }
        if self.registry.reorder_cluster_members(id, members).is_err() {
            return StackCycleOutcome::Unchanged;
        }
        let Some(after) = self.workspace_layout(id, work_area) else {
            return StackCycleOutcome::Unchanged;
        };
        self.begin_stack_cycle_reflow(output, id, before, after, direction, now);
        StackCycleOutcome::Cycled(member)
    }

    pub fn activate_slot(&mut self, output: &str, slot: u8, now: Duration) -> bool {
        let Some(id) = self
            .slots
            .get(output)
            .and_then(|slots| slots.get(usize::from(slot.saturating_sub(1))))
            .copied()
        else {
            return false;
        };
        self.close_bloom(output, now);
        self.set_hovered_core(None, now);
        if self.active_on(output) == Some(id) {
            self.hide_overflow(output);
            self.active.remove(output);
            self.registry.deactivate_cluster_workspace(id);
            self.begin_transition(output, id, transition::TransitionKind::Closing, now);
        } else {
            self.hide_overflow(output);
            if let Some(previous) = self.active.insert(output.to_string(), id) {
                self.registry.deactivate_cluster_workspace(previous);
            }
            self.registry.activate_cluster_workspace(id);
            self.overflow.reveal(output, now);
            self.begin_transition(output, id, transition::TransitionKind::Opening, now);
        }
        true
    }

    pub fn activate(&mut self, output: &str, id: ClusterId, now: Duration) -> bool {
        if self
            .metadata
            .get(&id)
            .is_none_or(|metadata| metadata.output != output)
        {
            return false;
        }
        self.close_bloom(output, now);
        self.set_hovered_core(None, now);
        if self.active_on(output) == Some(id) {
            self.hide_overflow(output);
            self.active.remove(output);
            self.registry.deactivate_cluster_workspace(id);
            self.begin_transition(output, id, transition::TransitionKind::Closing, now);
        } else {
            self.hide_overflow(output);
            if let Some(previous) = self.active.insert(output.to_string(), id) {
                self.registry.deactivate_cluster_workspace(previous);
            }
            self.registry.activate_cluster_workspace(id);
            self.overflow.reveal(output, now);
            self.begin_transition(output, id, transition::TransitionKind::Opening, now);
        }
        true
    }

    /// Activates `id` on `output` when it is not already the active cluster.
    ///
    /// This is the idempotent counterpart of [`ClusterSystem::activate`], which
    /// toggles an already-active workspace closed. External controllers (the
    /// `ext-workspace-v1` global, scripts, and future automation) ask for a
    /// workspace to *be* active, so repeating the request must not close it,
    /// restart its transition, or move focus again.
    ///
    /// Returns whether the activation state changed. A missing cluster or a
    /// cluster owned by another output is rejected without mutation.
    pub fn activate_only(&mut self, output: &str, id: ClusterId, now: Duration) -> bool {
        if self
            .metadata
            .get(&id)
            .is_none_or(|metadata| metadata.output != output)
        {
            return false;
        }
        if self.active_on(output) == Some(id) {
            return false;
        }
        self.close_bloom(output, now);
        self.set_hovered_core(None, now);
        self.hide_overflow(output);
        if let Some(previous) = self.active.insert(output.to_string(), id) {
            self.registry.deactivate_cluster_workspace(previous);
        }
        self.registry.activate_cluster_workspace(id);
        self.overflow.reveal(output, now);
        self.begin_transition(output, id, transition::TransitionKind::Opening, now);
        true
    }

    /// Deactivates `output` only when `id` is the cluster currently active
    /// there.
    ///
    /// Returns whether the activation state changed. Deactivating an inactive
    /// cluster, or a cluster owned by a different output, leaves every other
    /// clique's active selection untouched instead of clearing it.
    pub fn deactivate_only(&mut self, output: &str, id: ClusterId, now: Duration) -> bool {
        if self.active_on(output) != Some(id) {
            return false;
        }
        if self
            .metadata
            .get(&id)
            .is_none_or(|metadata| metadata.output != output)
        {
            return false;
        }
        self.close_bloom(output, now);
        self.set_hovered_core(None, now);
        self.hide_overflow(output);
        self.active.remove(output);
        self.registry.deactivate_cluster_workspace(id);
        self.begin_transition(output, id, transition::TransitionKind::Closing, now);
        true
    }

    /// Deactivates whatever cluster is currently active on `output`, leaving
    /// the output in "Field" (no workspace active) mode.
    ///
    /// Returns whether the activation state changed.
    pub fn deactivate_output(&mut self, output: &str, now: Duration) -> bool {
        let Some(id) = self.active_on(output) else {
            return false;
        };
        self.deactivate_only(output, id, now)
    }

    pub fn is_member(&self, id: NodeId) -> bool {
        self.registry.is_cluster_member(id)
    }

    pub fn cluster_for_member(&self, id: NodeId) -> Option<ClusterId> {
        self.registry.cluster_id_for_member(id)
    }

    pub fn cluster_for_core(&self, id: NodeId) -> Option<ClusterId> {
        self.registry.cluster_id_for_core(id)
    }

    pub fn core_node(&self, id: ClusterId) -> Option<NodeId> {
        self.registry.cluster(id)?.core_node()
    }

    pub fn set_core_pinned(&mut self, field: &mut Field, core: NodeId, pinned: bool) -> bool {
        let Some(cluster_id) = self.cluster_for_core(core) else {
            return false;
        };
        let Some(cluster) = self.registry.cluster_mut(cluster_id) else {
            return false;
        };
        cluster.pinned = pinned;
        field.set_pinned(core, pinned)
    }

    pub(crate) fn set_core_position(&mut self, id: ClusterId, position: Vec2) {
        if let Some(metadata) = self.metadata.get_mut(&id) {
            metadata.core_position = position;
        }
    }

    pub(crate) fn collapsed_core_landmarks(&self) -> Vec<(ClusterId, NodeId, String, Vec2, bool)> {
        self.metadata
            .iter()
            .filter_map(|(cluster_id, metadata)| {
                let cluster = self.registry.cluster(*cluster_id)?;
                (self.active_on(&metadata.output) != Some(*cluster_id)).then_some((
                    *cluster_id,
                    cluster.core_node()?,
                    metadata.output.clone(),
                    metadata.core_position,
                    cluster.pinned
                        || self.bloom.join_target_on_output(&metadata.output) == Some(*cluster_id),
                ))
            })
            .collect()
    }

    pub(crate) fn bloom_pinned_core_nodes(&self) -> Vec<NodeId> {
        self.metadata
            .iter()
            .filter_map(|(cluster_id, metadata)| {
                if self.bloom.join_target_on_output(&metadata.output) == Some(*cluster_id) {
                    self.core_node(*cluster_id)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn member_ids(&self, id: ClusterId) -> Vec<NodeId> {
        self.registry
            .cluster(id)
            .map(|cluster| cluster.members().to_vec())
            .unwrap_or_default()
    }

    pub fn close_targets_for_node(&self, id: NodeId) -> Vec<NodeId> {
        self.cluster_for_core(id)
            .map(|cluster| self.member_ids(cluster))
            .unwrap_or_else(|| vec![id])
    }

    pub fn active_layout_for_member(&self, id: NodeId) -> Option<ClusterWorkspaceLayoutKind> {
        let layout = self.member_layout(id)?;
        let cluster = self.cluster_for_member(id)?;
        let metadata = self.metadata(cluster)?;
        (self.active_on(&metadata.output) == Some(cluster)).then_some(layout)
    }

    pub(crate) fn member_layout(&self, id: NodeId) -> Option<ClusterWorkspaceLayoutKind> {
        if self.member_floats.is_floating(id) {
            return None;
        }
        let cluster = self.cluster_for_member(id)?;
        self.metadata(cluster).map(|metadata| metadata.layout)
    }

    pub fn is_member_floating(&self, member: NodeId) -> bool {
        self.member_floats.is_floating(member)
    }

    pub fn member_floating_rect(&self, member: NodeId) -> Option<Rectangle<i32, Logical>> {
        self.member_floats.rect(member)
    }

    pub fn member_floating_output(&self, member: NodeId) -> Option<&str> {
        self.member_floats.output(member)
    }

    /// Moves one member between the active cluster's layout and its
    /// cluster-local floating layer without changing registry membership.
    /// The returned boolean is the member's new floating state.
    pub fn toggle_member_floating(
        &mut self,
        output: &str,
        member: NodeId,
        work_area: Rectangle<i32, Logical>,
        current_rect: Rectangle<i32, Logical>,
        now: Duration,
    ) -> Option<bool> {
        let cluster = self.active_on(output)?;
        if self.cluster_for_member(member) != Some(cluster)
            || self
                .dragged_window
                .as_ref()
                .is_some_and(|drag| drag.member == member)
        {
            return None;
        }
        let before = self.workspace_layout(cluster, work_area)?;
        self.surfaces.invalidate_target(member);
        if self.member_floats.tile(member).is_some() {
            self.begin_reflow_with_origin(output, cluster, before, member, current_rect, now);
            Some(false)
        } else {
            let promotion = (self.metadata(cluster).map(|metadata| metadata.layout)
                == Some(ClusterWorkspaceLayoutKind::Tiling))
            .then(|| {
                before.queue_members.first().copied().and_then(|promoted| {
                    self.overflow_geometry(output, work_area)?
                        .items
                        .into_iter()
                        .find(|item| item.node_id == promoted)
                        .map(|item| (promoted, item.rect))
                })
            })
            .flatten();
            self.member_floats
                .float(member, output, current_rect, work_area);
            let duration_ms = self
                .metadata(cluster)
                .map(|metadata| match metadata.layout {
                    ClusterWorkspaceLayoutKind::Tiling => self.animations.tiling.reflow_duration_ms,
                    ClusterWorkspaceLayoutKind::Stacking => {
                        self.animations.stacking.cycle_duration_ms
                    }
                })
                .unwrap_or(self.animations.tiling.reflow_duration_ms);
            if let Some((promoted, origin)) = promotion {
                self.begin_reflow_with_origin(output, cluster, before, promoted, origin, now);
                self.overflow.reveal(output, now);
            } else {
                self.begin_reflow(output, cluster, before, now, duration_ms);
            }
            Some(true)
        }
    }

    pub fn update_member_floating_rect(
        &mut self,
        output: &str,
        member: NodeId,
        rect: Rectangle<i32, Logical>,
        work_area: Rectangle<i32, Logical>,
    ) -> bool {
        let Some(cluster) = self.cluster_for_member(member) else {
            return false;
        };
        let Some(home_output) = self
            .metadata(cluster)
            .map(|metadata| metadata.output.clone())
        else {
            return false;
        };
        if self.active_on(&home_output) != Some(cluster)
            || !self.member_floats.update(member, output, rect, work_area)
        {
            return false;
        }
        self.surfaces.invalidate_target(member);
        true
    }

    pub fn admit_mapped_window(
        &mut self,
        field: &mut Field,
        output: &str,
        member: NodeId,
        participation: halley_config::WindowClusterParticipation,
        work_area: Rectangle<i32, Logical>,
        now: Duration,
    ) -> bool {
        let Some(active) = self.active_on(output) else {
            return false;
        };
        if self.registry.is_cluster_member(member) {
            return false;
        }
        match participation {
            halley_config::WindowClusterParticipation::Float => {
                self.admission_floats.insert(member)
            }
            halley_config::WindowClusterParticipation::Layout => {
                self.admission_floats.remove(&member);
                let layout = self
                    .metadata(active)
                    .map(|metadata| metadata.layout)
                    .unwrap_or(ClusterWorkspaceLayoutKind::Tiling);
                let before = self.workspace_layout(active, work_area);
                let previous_overflow_len = before
                    .as_ref()
                    .map(|layout| layout.queue_members.len())
                    .unwrap_or(0);
                let result = match layout {
                    ClusterWorkspaceLayoutKind::Stacking => self
                        .registry
                        .add_member_to_cluster_front(field, active, member),
                    ClusterWorkspaceLayoutKind::Tiling if self.config.tiling.new_on_top => self
                        .registry
                        .add_member_to_cluster_front(field, active, member),
                    ClusterWorkspaceLayoutKind::Tiling => {
                        self.registry.add_member_to_cluster(field, active, member)
                    }
                };
                if result.is_err() {
                    return false;
                }
                if let Some(before) = before {
                    match layout {
                        ClusterWorkspaceLayoutKind::Stacking => self.begin_stack_insert_reflow(
                            output, active, before, member, work_area, now,
                        ),
                        ClusterWorkspaceLayoutKind::Tiling => self.begin_reflow(
                            output,
                            active,
                            before,
                            now,
                            self.animations.tiling.reflow_duration_ms,
                        ),
                    }
                }
                if layout == ClusterWorkspaceLayoutKind::Tiling
                    && self
                        .workspace_layout(active, work_area)
                        .is_some_and(|layout| layout.queue_members.len() > previous_overflow_len)
                {
                    self.overflow.reveal(output, now);
                }
                true
            }
        }
    }

    pub fn admit_attributed_window(
        &mut self,
        field: &mut Field,
        cluster: ClusterId,
        member: NodeId,
        work_area: Rectangle<i32, Logical>,
        now: Duration,
    ) -> bool {
        if self.registry.is_cluster_member(member) {
            return false;
        }
        let Some(metadata) = self.metadata(cluster).cloned() else {
            return false;
        };
        if self.active_on(&metadata.output) == Some(cluster) {
            return self.admit_mapped_window(
                field,
                &metadata.output,
                member,
                halley_config::WindowClusterParticipation::Layout,
                work_area,
                now,
            );
        }
        if self
            .registry
            .add_member_to_cluster(field, cluster, member)
            .is_err()
        {
            return false;
        }
        let _ = field.set_state(member, halley_core::field::NodeState::Node);
        if let Some(node) = field.node_mut(member) {
            node.visibility
                .set(halley_core::field::Visibility::HIDDEN_BY_CLUSTER, true);
            node.pos = metadata.core_position;
        }
        true
    }

    pub fn window_presentation(
        &self,
        id: NodeId,
        output: &str,
        work_area: Rectangle<i32, Logical>,
        core: Option<Point<i32, Logical>>,
        now: Duration,
    ) -> WindowPresentation {
        if let Some(drag) = self
            .dragged_window
            .as_ref()
            .filter(|drag| drag.member == id)
        {
            return match &drag.presentation {
                Some((drag_output, rect)) if drag_output == output => {
                    WindowPresentation::PointerDrag { rect: *rect }
                }
                Some(_) => WindowPresentation::Hidden,
                None => WindowPresentation::Field,
            };
        }
        let member_cluster = self.cluster_for_member(id);
        if let Some(target) = self.member_floats.rect_on(id, output) {
            let Some(cluster) = member_cluster else {
                return WindowPresentation::Hidden;
            };
            let Some(home_output) = self
                .metadata(cluster)
                .map(|metadata| metadata.output.as_str())
            else {
                return WindowPresentation::Hidden;
            };
            let presented = self.active_on(home_output) == Some(cluster)
                || self.transition_cluster_on(home_output, now) == Some(cluster);
            if !presented {
                return WindowPresentation::Hidden;
            }
            let transition_core = (home_output == output).then_some(core).flatten();
            if let Some(visual) =
                self.transition_visual(home_output, cluster, id, target, transition_core, now)
            {
                return WindowPresentation::Workspace {
                    rect: visual.rect,
                    depth: usize::MAX,
                    alpha: visual.alpha,
                };
            }
            if home_output == output
                && let Some(visual) =
                    self.reflow_visual(home_output, cluster, id, Some((target, usize::MAX)), now)
            {
                return WindowPresentation::Workspace {
                    rect: visual.rect,
                    depth: visual.depth,
                    alpha: visual.alpha,
                };
            }
            return WindowPresentation::Workspace {
                rect: target,
                depth: usize::MAX,
                alpha: 1.0,
            };
        }
        if self.member_floats.is_floating(id) {
            return WindowPresentation::Hidden;
        }
        let active = self.active_on(output);
        let closing = self
            .transitions
            .get(output)
            .filter(|transition| {
                transition.kind == transition::TransitionKind::Closing
                    && self.transition_cluster_on(output, now) == Some(transition.cluster_id)
            })
            .map(|transition| transition.cluster_id);
        let Some(active) = active.or(closing) else {
            return if member_cluster.is_some() {
                WindowPresentation::Hidden
            } else {
                WindowPresentation::Field
            };
        };
        if self.admission_floats.contains(&id) {
            return WindowPresentation::Field;
        }
        if member_cluster != Some(active) {
            return WindowPresentation::Hidden;
        }
        // A member whose client geometry is still owned by its admission
        // transaction must retain the ordinary Field transform as well.
        // Splitting geometry and presentation ownership would expose a
        // scaled cluster-local pointer coordinate before the first layout
        // configure has become authoritative.
        if self.surfaces.layout_is_deferred(id, now) {
            return WindowPresentation::Field;
        }
        let Some(layout) = self.workspace_layout(active, work_area) else {
            return WindowPresentation::Hidden;
        };
        let target = layout
            .placements
            .into_iter()
            .find(|placement| placement.node_id == id)
            .map(|placement| {
                let target = Rectangle::new(
                    (
                        placement.rect.x.round() as i32,
                        placement.rect.y.round() as i32,
                    )
                        .into(),
                    (
                        placement.rect.w.round().max(1.0) as i32,
                        placement.rect.h.round().max(1.0) as i32,
                    )
                        .into(),
                );
                (target, placement.depth)
            });
        if let Some((target, depth)) = target {
            if let Some(visual) = self.transition_visual(output, active, id, target, core, now) {
                WindowPresentation::Workspace {
                    rect: visual.rect,
                    depth,
                    alpha: visual.alpha,
                }
            } else if let Some(visual) =
                self.reflow_visual(output, active, id, Some((target, depth)), now)
            {
                WindowPresentation::Workspace {
                    rect: visual.rect,
                    depth: visual.depth,
                    alpha: visual.alpha,
                }
            } else {
                WindowPresentation::Workspace {
                    rect: target,
                    depth,
                    alpha: 1.0,
                }
            }
        } else if let Some(visual) = self.reflow_visual(output, active, id, None, now) {
            WindowPresentation::Workspace {
                rect: visual.rect,
                depth: visual.depth,
                alpha: visual.alpha,
            }
        } else {
            WindowPresentation::Hidden
        }
    }

    pub fn extra_window_presentation(
        &self,
        id: NodeId,
        output: &str,
        now: Duration,
    ) -> Option<WindowPresentation> {
        let active = self.active_on(output)?;
        if self.cluster_for_member(id) != Some(active) {
            return None;
        }
        let visual = self.extra_reflow_visual(output, active, id, now)?;
        Some(WindowPresentation::Workspace {
            rect: visual.rect,
            depth: visual.depth,
            alpha: visual.alpha,
        })
    }

    pub fn directional_tile_target(
        &self,
        output: &str,
        current: Option<NodeId>,
        direction: halley_config::ClusterDirection,
        work_area: Rectangle<i32, Logical>,
    ) -> Option<NodeId> {
        let id = self.active_on(output)?;
        if self.metadata(id)?.layout != ClusterWorkspaceLayoutKind::Tiling {
            return None;
        }
        let layout = self.workspace_layout(id, work_area)?;
        let current = current
            .filter(|current| {
                layout
                    .placements
                    .iter()
                    .any(|tile| tile.node_id == *current)
            })
            .or_else(|| layout.placements.first().map(|tile| tile.node_id))?;
        let source = layout
            .placements
            .iter()
            .find(|tile| tile.node_id == current)?;
        let (sx, sy) = rect_center(source.rect);
        layout
            .placements
            .iter()
            .filter(|tile| tile.node_id != current)
            .filter_map(|tile| {
                let (tx, ty) = rect_center(tile.rect);
                let (primary, secondary) = match direction {
                    halley_config::ClusterDirection::Left if tx < sx => (sx - tx, (sy - ty).abs()),
                    halley_config::ClusterDirection::Right if tx > sx => (tx - sx, (sy - ty).abs()),
                    halley_config::ClusterDirection::Up if ty < sy => (sy - ty, (sx - tx).abs()),
                    halley_config::ClusterDirection::Down if ty > sy => (ty - sy, (sx - tx).abs()),
                    _ => return None,
                };
                Some((primary + secondary * 0.35, tile.node_id))
            })
            .min_by(|(a, _), (b, _)| a.total_cmp(b))
            .map(|(_, id)| id)
    }

    pub fn swap_directional_tile(
        &mut self,
        output: &str,
        current: Option<NodeId>,
        direction: halley_config::ClusterDirection,
        work_area: Rectangle<i32, Logical>,
        now: Duration,
    ) -> Option<NodeId> {
        let id = self.active_on(output)?;
        let before = self.workspace_layout(id, work_area)?;
        let current = current
            .filter(|member| !self.member_floats.is_floating(*member))
            .or_else(|| {
                self.workspace_layout(id, work_area)?
                    .placements
                    .first()
                    .map(|placement| placement.node_id)
            })?;
        let target = self.directional_tile_target(output, Some(current), direction, work_area)?;
        let mut members = self.registry.cluster(id)?.members().to_vec();
        let current_index = members.iter().position(|member| *member == current)?;
        let target_index = members.iter().position(|member| *member == target)?;
        members.swap(current_index, target_index);
        self.registry.reorder_cluster_members(id, members).ok()?;
        self.begin_reflow(
            output,
            id,
            before,
            now,
            self.animations.tiling.reflow_duration_ms,
        );
        Some(current)
    }

    /// Gives one visible workspace member temporary pointer authority. The
    /// workspace keeps its slot in the layout while presentation and client
    /// sizing stop pinning the held window to that slot.
    pub fn begin_workspace_drag(
        &mut self,
        output: &str,
        member: NodeId,
        rect: Rectangle<i32, Logical>,
    ) -> bool {
        let Some(cluster) = self.active_on(output) else {
            return false;
        };
        if self.admission_floats.contains(&member)
            || self.member_floats.is_floating(member)
            || self.cluster_for_member(member) != Some(cluster)
        {
            return false;
        }
        self.surfaces.invalidate_target(member);
        self.dragged_window = Some(DraggedWindow {
            member,
            presentation: Some((output.to_string(), rect)),
        });
        true
    }

    pub fn begin_floating_member_drag(
        &mut self,
        cluster_output: &str,
        member_output: &str,
        member: NodeId,
        rect: Rectangle<i32, Logical>,
    ) -> bool {
        let Some(cluster) = self.active_on(cluster_output) else {
            return false;
        };
        if self.cluster_for_member(member) != Some(cluster)
            || !self.member_floats.is_floating(member)
        {
            return false;
        }
        self.surfaces.invalidate_target(member);
        self.dragged_window = Some(DraggedWindow {
            member,
            presentation: Some((member_output.to_string(), rect)),
        });
        true
    }

    pub fn update_workspace_drag(
        &mut self,
        member: NodeId,
        output: &str,
        location: Point<i32, Logical>,
    ) -> bool {
        let Some(drag) = self
            .dragged_window
            .as_mut()
            .filter(|drag| drag.member == member)
        else {
            return false;
        };
        let Some((drag_output, rect)) = drag.presentation.as_mut() else {
            return false;
        };
        let changed = drag_output != output || rect.loc != location;
        *drag_output = output.to_string();
        rect.loc = location;
        changed
    }

    /// Keeps an ordinary Field window visible while it is held over an active
    /// workspace, where non-members are otherwise intentionally hidden.
    pub fn begin_field_drag(&mut self, member: NodeId) -> bool {
        if self.is_member(member) {
            return false;
        }
        self.surfaces.invalidate_target(member);
        self.dragged_window = Some(DraggedWindow {
            member,
            presentation: None,
        });
        true
    }

    /// Pulls an active workspace member into the Field while retaining enough
    /// cluster state for a possible drop back into the same workspace.
    pub fn detach_active_member_for_drag(
        &mut self,
        field: &mut Field,
        output: &str,
        dragged: ClusterDragMember,
        work_area: Rectangle<i32, Logical>,
        position: Vec2,
        now: Duration,
    ) -> bool {
        let ClusterDragMember {
            cluster_id: cluster,
            node_id: member,
        } = dragged;
        if self.active_on(output) != Some(cluster)
            || self.cluster_for_member(member) != Some(cluster)
        {
            return false;
        }
        let Some(layout) = self.metadata(cluster).map(|metadata| metadata.layout) else {
            return false;
        };
        let Some(before) = self.workspace_layout(cluster, work_area) else {
            return false;
        };
        if self
            .dragged_window
            .as_ref()
            .is_some_and(|drag| drag.member == member)
        {
            self.dragged_window = None;
        }
        self.surfaces.invalidate_target(member);
        let was_floating = self.member_floats.is_floating(member);
        if !self.detach_member(field, cluster, member, position, now) {
            return false;
        }
        if was_floating {
            // Mod+V floats are already above the workspace. Ejecting them
            // from the cluster must keep that Field presentation, matching
            // rule-admitted floats, instead of hiding them behind the still-
            // open workspace.
            self.admission_floats.insert(member);
        }
        if self.active_on(output) == Some(cluster) {
            let duration_ms = match layout {
                ClusterWorkspaceLayoutKind::Tiling => self.animations.tiling.reflow_duration_ms,
                ClusterWorkspaceLayoutKind::Stacking => self.animations.stacking.cycle_duration_ms,
            };
            self.begin_reflow(output, cluster, before, now, duration_ms);
        }
        true
    }

    /// Admits a Field window at the front of the active workspace. In
    /// stacking that is the only focusable/top card; in tiling it becomes the
    /// master.
    pub fn join_active_member_front(
        &mut self,
        field: &mut Field,
        output: &str,
        member: NodeId,
        work_area: Rectangle<i32, Logical>,
        origin: Rectangle<i32, Logical>,
        now: Duration,
    ) -> bool {
        let Some(cluster) = self.active_on(output) else {
            return false;
        };
        if self.is_member(member) {
            return false;
        }
        let Some(before) = self.workspace_layout(cluster, work_area) else {
            return false;
        };
        if self
            .registry
            .add_member_to_cluster_front(field, cluster, member)
            .is_err()
        {
            return false;
        }
        self.admission_floats.remove(&member);
        self.surfaces.invalidate_target(member);
        self.begin_reflow_with_origin(output, cluster, before, member, origin, now);
        true
    }

    /// Reorders a held tile when the pointer enters another visible tile's
    /// slot. Insertion matches the original Halley behavior: the held member
    /// moves to the target index instead of exchanging two unrelated slots.
    pub fn move_tiled_drag_to_point(
        &mut self,
        output: &str,
        member: NodeId,
        work_area: Rectangle<i32, Logical>,
        output_local: Point<f64, Logical>,
        now: Duration,
    ) -> bool {
        if self.dragged_window.as_ref().map(|drag| drag.member) != Some(member) {
            return false;
        }
        let Some(cluster) = self.active_on(output) else {
            return false;
        };
        let Some(before) = self.workspace_layout(cluster, work_area) else {
            return false;
        };
        let Some(target) = member_at_point(&before, output_local) else {
            return false;
        };
        if target == member {
            return false;
        }
        let Some(mut members) = self
            .registry
            .cluster(cluster)
            .map(|cluster| cluster.members().to_vec())
        else {
            return false;
        };
        let Some(from_index) = members.iter().position(|candidate| *candidate == member) else {
            return false;
        };
        let Some(target_index) = members.iter().position(|candidate| *candidate == target) else {
            return false;
        };
        let moved = members.remove(from_index);
        members.insert(target_index.min(members.len()), moved);
        if self
            .registry
            .reorder_cluster_members(cluster, members)
            .is_err()
        {
            return false;
        }
        self.begin_reflow(
            output,
            cluster,
            before,
            now,
            self.animations.tiling.reflow_duration_ms,
        );
        true
    }

    /// Returns a released workspace member to the layout from its actual pointer-owned
    /// rectangle, avoiding a snap before the normal reflow takes over.
    pub fn finish_workspace_drag(
        &mut self,
        output: &str,
        member: NodeId,
        work_area: Rectangle<i32, Logical>,
        origin: Rectangle<i32, Logical>,
        now: Duration,
    ) -> bool {
        if self.dragged_window.as_ref().map(|drag| drag.member) != Some(member) {
            return false;
        }
        self.dragged_window = None;
        let Some(cluster) = self.active_on(output) else {
            return false;
        };
        if self.cluster_for_member(member) != Some(cluster) {
            return false;
        }
        let Some(before) = self.workspace_layout(cluster, work_area) else {
            return false;
        };
        self.begin_reflow_with_origin(output, cluster, before, member, origin, now);
        true
    }

    pub fn finish_floating_member_drag(
        &mut self,
        cluster_output: &str,
        member_output: &str,
        member: NodeId,
        work_area: Rectangle<i32, Logical>,
        rect: Rectangle<i32, Logical>,
    ) -> bool {
        if self.dragged_window.as_ref().map(|drag| drag.member) != Some(member) {
            return false;
        }
        self.dragged_window = None;
        let Some(cluster) = self.active_on(cluster_output) else {
            return false;
        };
        if self.cluster_for_member(member) != Some(cluster)
            || !self.member_floats.is_floating(member)
        {
            return false;
        }
        let changed = self
            .member_floats
            .update(member, member_output, rect, work_area);
        self.surfaces.invalidate_target(member);
        changed || self.member_floats.rect(member).is_some()
    }

    pub fn cancel_window_drag(&mut self) -> bool {
        self.dragged_window.take().is_some()
    }

    fn workspace_layout(
        &self,
        id: ClusterId,
        work_area: Rectangle<i32, Logical>,
    ) -> Option<ClusterWorkspaceLayoutResult> {
        let cluster = self.registry.cluster(id)?;
        let metadata = self.metadata(id)?;
        let outer = self.config.tiling.gaps_outer_px.max(0.0);
        let bounds = LayoutRect {
            x: work_area.loc.x as f32 + outer,
            y: work_area.loc.y as f32 + outer,
            w: (work_area.size.w as f32 - outer * 2.0).max(1.0),
            h: (work_area.size.h as f32 - outer * 2.0).max(1.0),
        };
        let limit = match metadata.layout {
            ClusterWorkspaceLayoutKind::Tiling => self.config.tiling.max_stack,
            ClusterWorkspaceLayoutKind::Stacking => self.config.stacking.max_visible,
        };
        let layout_members = cluster
            .members()
            .iter()
            .copied()
            .filter(|member| !self.member_floats.is_floating(*member))
            .collect::<Vec<_>>();
        Some(layout_cluster_workspace(
            metadata.layout,
            bounds,
            self.config.tiling.gaps_inner_px,
            self.config.tiling.gaps_inner_px,
            &layout_members,
            limit,
        ))
    }

    pub fn clusters_for_output(
        &self,
        output: &str,
    ) -> impl Iterator<Item = (u8, ClusterId, &ClusterMetadata)> {
        self.slots
            .get(output)
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(index, id)| {
                Some((u8::try_from(index + 1).ok()?, *id, self.metadata.get(id)?))
            })
    }

    pub fn core_hit_test(
        &self,
        output: &str,
        camera: &halley_core::camera::Camera,
        output_geometry: Rectangle<i32, Logical>,
        screen: Point<f64, Logical>,
    ) -> Option<ClusterId> {
        if self.active_on(output).is_some() {
            return None;
        }
        self.clusters_for_output(output)
            .filter_map(|(_, id, metadata)| {
                let center = crate::nodes::screen_from_world(
                    metadata.core_position,
                    camera,
                    output_geometry,
                );
                let diameter = CORE_DIAMETER_PX.round() as i32;
                Rectangle::new(
                    (center.x - diameter / 2, center.y - diameter / 2).into(),
                    (diameter, diameter).into(),
                )
                .to_f64()
                .contains(screen)
                .then_some(id)
            })
            .max_by_key(|id| id.as_u64())
    }

    pub fn first_member(&self, id: ClusterId) -> Option<NodeId> {
        self.registry.cluster(id)?.members().first().copied()
    }

    /// Permanently removes a runtime workspace without destroying any of its
    /// member windows. This is the authoritative cluster deletion boundary:
    /// registry membership and every Halley-owned presentation lease retire
    /// together, while remembered client geometries are handed back to the
    /// session for protocol-level restoration.
    pub(crate) fn dissolve_cluster(
        &mut self,
        field: &mut Field,
        cluster_id: ClusterId,
    ) -> Option<ClusterDissolution> {
        let output = self.metadata(cluster_id)?.output.clone();
        let members = self.member_ids(cluster_id);
        if !self.registry.dissolve_cluster(field, cluster_id) {
            return None;
        }

        let surface_restores = members
            .iter()
            .filter_map(|member| {
                self.member_floats.remove(*member);
                self.overlay_label_hover.borrow_mut().remove(member);
                self.surfaces
                    .take_restore(*member)
                    .map(|geometry| (*member, geometry))
            })
            .collect();
        if self
            .dragged_window
            .as_ref()
            .is_some_and(|drag| members.contains(&drag.member))
        {
            self.dragged_window = None;
        }
        if self
            .overlay_hovered
            .as_ref()
            .is_some_and(|(_, member)| members.contains(member))
        {
            self.overlay_hovered = None;
        }
        self.remove_cluster_metadata(cluster_id);

        Some(ClusterDissolution {
            output,
            members,
            surface_restores,
        })
    }

    pub fn detach_member(
        &mut self,
        field: &mut Field,
        cluster_id: ClusterId,
        member: NodeId,
        position: Vec2,
        now: Duration,
    ) -> bool {
        use halley_core::cluster::ClusterRemoveMemberOutcome;
        use halley_core::field::{NodeState, Visibility};

        let cluster_members = self.member_ids(cluster_id);
        let Some(outcome) = self.registry.remove_member_from_cluster(cluster_id, member) else {
            return false;
        };
        match outcome {
            ClusterRemoveMemberOutcome::Removed => {
                self.member_floats.remove(member);
                let _ = field.set_state(member, NodeState::Active);
                if let Some(node) = field.node_mut(member) {
                    node.visibility.clear(Visibility::HIDDEN_BY_CLUSTER);
                    node.visibility.clear(Visibility::DETACHED);
                    node.pos = position;
                }
                let _ = field.touch(member, now.as_millis() as u64);
            }
            ClusterRemoveMemberOutcome::RequiresDissolve => {
                if !self.registry.dissolve_cluster(field, cluster_id) {
                    return false;
                }
                self.remove_cluster_metadata(cluster_id);
                for member in cluster_members {
                    self.member_floats.remove(member);
                }
                if let Some(node) = field.node_mut(member) {
                    node.pos = position;
                }
                let _ = field.touch(member, now.as_millis() as u64);
            }
        }
        self.overlay_label_hover.borrow_mut().remove(&member);
        self.overlay_hovered = None;
        true
    }

    /// Removes one destroyed window from cluster bookkeeping before
    /// `NodesState` discards its Field node. A remapped/unmapped window is
    /// intentionally retained; only final surface destruction reaches here.
    pub fn forget_destroyed_member(&mut self, field: &mut Field, member: NodeId) -> bool {
        self.forget_surface_state(member);
        if self
            .dragged_window
            .as_ref()
            .is_some_and(|drag| drag.member == member)
        {
            self.dragged_window = None;
        }
        if self
            .join_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.member == member)
        {
            self.join_candidate = None;
        }
        let Some(id) = self.registry.cluster_id_for_member(member) else {
            self.admission_floats.remove(&member);
            self.member_floats.remove(member);
            if let Some(creation) = self.creation.as_mut() {
                creation.selected.remove(&member);
                creation.prepared = None;
            }
            return false;
        };
        let cluster_members = self.member_ids(id);
        let Some((_, effect)) = self.registry.remove_node_cluster_safe(field, member) else {
            return false;
        };
        if matches!(
            effect,
            Some(halley_core::cluster::RemoveNodeClusterEffect::DissolvedCluster(_))
        ) {
            self.remove_cluster_metadata(id);
            for member in cluster_members {
                self.member_floats.remove(member);
            }
        }
        self.admission_floats.remove(&member);
        self.member_floats.remove(member);
        if let Some(creation) = self.creation.as_mut() {
            creation.selected.remove(&member);
            creation.prepared = None;
        }
        true
    }

    pub fn forget_destroyed_member_animated(
        &mut self,
        field: &mut Field,
        member: NodeId,
        work_area: Rectangle<i32, Logical>,
        now: Duration,
    ) -> bool {
        let reflow = self.cluster_for_member(member).and_then(|cluster| {
            let metadata = self.metadata(cluster)?;
            (self.active_on(&metadata.output) == Some(cluster)).then_some(())?;
            let before = self.workspace_layout(cluster, work_area)?;
            let removed_was_visible = before
                .placements
                .iter()
                .any(|placement| placement.node_id == member);
            if !removed_was_visible {
                return None;
            }
            let promotion = (metadata.layout == ClusterWorkspaceLayoutKind::Tiling)
                .then(|| {
                    before.queue_members.first().copied().and_then(|promoted| {
                        let origin = self
                            .overflow_geometry(&metadata.output, work_area)?
                            .items
                            .into_iter()
                            .find(|item| item.node_id == promoted)?
                            .rect;
                        Some((promoted, origin))
                    })
                })
                .flatten();
            let duration_ms = match metadata.layout {
                ClusterWorkspaceLayoutKind::Tiling => self.animations.tiling.reflow_duration_ms,
                ClusterWorkspaceLayoutKind::Stacking => self.animations.stacking.cycle_duration_ms,
            };
            Some((
                cluster,
                metadata.output.clone(),
                before,
                promotion,
                duration_ms,
            ))
        });
        let changed = self.forget_destroyed_member(field, member);
        if changed
            && let Some((cluster, output, before, promotion, duration_ms)) = reflow
            && self.active_on(&output) == Some(cluster)
        {
            if let Some((promoted, origin)) = promotion
                && self.cluster_for_member(promoted) == Some(cluster)
            {
                self.begin_reflow_with_origin(&output, cluster, before, promoted, origin, now);
                self.overflow.reveal(&output, now);
            } else {
                self.begin_reflow(&output, cluster, before, now, duration_ms);
            }
        }
        changed
    }

    fn remove_cluster_metadata(&mut self, id: ClusterId) {
        if self.hovered_core == Some(id) {
            self.hovered_core = None;
        }
        self.label_hover.borrow_mut().remove(&id);
        self.bloom.remove_cluster(id);
        self.overflow.remove_cluster(id);
        self.remove_cluster_transitions(id);
        if self
            .join_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.cluster_id == id)
        {
            self.join_candidate = None;
        }
        let output = self.metadata.remove(&id).map(|metadata| metadata.output);
        if let Some(output) = output {
            if let Some(slots) = self.slots.get_mut(&output) {
                slots.retain(|candidate| *candidate != id);
                if slots.is_empty() {
                    self.slots.remove(&output);
                }
            }
            if self.active.get(&output) == Some(&id) {
                self.active.remove(&output);
                self.overflow.hide(&output);
            }
        }
    }

    fn slot_of(&self, output: &str, id: ClusterId) -> Option<u8> {
        self.slots
            .get(output)?
            .iter()
            .position(|candidate| *candidate == id)
            .and_then(|index| u8::try_from(index + 1).ok())
    }
}

fn rect_center(rect: LayoutRect) -> (f32, f32) {
    (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5)
}

fn member_at_point(
    layout: &ClusterWorkspaceLayoutResult,
    point: Point<f64, Logical>,
) -> Option<NodeId> {
    layout.placements.iter().find_map(|placement| {
        let rect = Rectangle::<f64, Logical>::new(
            (f64::from(placement.rect.x), f64::from(placement.rect.y)).into(),
            (f64::from(placement.rect.w), f64::from(placement.rect.h)).into(),
        );
        rect.contains(point).then_some(placement.node_id)
    })
}

#[cfg(test)]
mod tests;
