//! Cluster workspace and member drag operations.

use super::*;

impl ClusterSystem {
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
}
