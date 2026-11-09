use std::collections::HashMap;
use std::time::Duration;

use halley_core::field::NodeId;
use smithay::utils::{Logical, Rectangle};

use super::ClusterSystem;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceSurfaceTarget {
    pub(crate) node_id: NodeId,
    pub(crate) geometry: Rectangle<i32, Logical>,
}

#[derive(Default)]
pub(super) struct WorkspaceSurfaceState {
    requested: HashMap<NodeId, Rectangle<i32, Logical>>,
    restore: HashMap<NodeId, Rectangle<i32, Logical>>,
    layout_deferred_until: HashMap<NodeId, Duration>,
}

impl WorkspaceSurfaceState {
    pub(super) fn translate(
        &mut self,
        members: &[NodeId],
        delta: smithay::utils::Point<i32, Logical>,
    ) {
        for member in members {
            if let Some(rect) = self.requested.get_mut(member) {
                rect.loc += delta;
            }
            if let Some(rect) = self.restore.get_mut(member) {
                rect.loc += delta;
            }
        }
    }

    fn prepare(
        &mut self,
        node_id: NodeId,
        current: Rectangle<i32, Logical>,
        target: Rectangle<i32, Logical>,
    ) -> bool {
        self.restore.entry(node_id).or_insert(current);
        if self.requested.get(&node_id) == Some(&target) {
            return false;
        }
        self.requested.insert(node_id, target);
        true
    }

    fn forget(&mut self, node_id: NodeId) {
        self.requested.remove(&node_id);
        self.restore.remove(&node_id);
        self.layout_deferred_until.remove(&node_id);
    }

    pub(super) fn take_restore(&mut self, node_id: NodeId) -> Option<Rectangle<i32, Logical>> {
        self.requested.remove(&node_id);
        self.layout_deferred_until.remove(&node_id);
        self.restore.remove(&node_id)
    }

    pub(super) fn invalidate_target(&mut self, node_id: NodeId) {
        self.requested.remove(&node_id);
    }

    fn defer_layout_until(&mut self, node_id: NodeId, until: Duration) {
        self.layout_deferred_until
            .entry(node_id)
            .and_modify(|current| *current = (*current).max(until))
            .or_insert(until);
    }

    pub(super) fn layout_is_deferred(&self, node_id: NodeId, now: Duration) -> bool {
        self.layout_deferred_until
            .get(&node_id)
            .is_some_and(|until| now < *until)
    }
}

impl ClusterSystem {
    pub(crate) fn workspace_surface_target_for(
        &self,
        node_id: NodeId,
        output: &str,
        work_area: Rectangle<i32, Logical>,
        output_geometry: Rectangle<i32, Logical>,
    ) -> Option<WorkspaceSurfaceTarget> {
        self.workspace_surface_targets(output, work_area, output_geometry)
            .into_iter()
            .find(|target| target.node_id == node_id)
    }

    pub(crate) fn workspace_surface_targets(
        &self,
        output: &str,
        work_area: Rectangle<i32, Logical>,
        output_geometry: Rectangle<i32, Logical>,
    ) -> Vec<WorkspaceSurfaceTarget> {
        let dragging = self.dragged_window.as_ref().map(|drag| drag.member);
        let mut targets = self
            .active_on(output)
            .and_then(|cluster_id| self.workspace_layout(cluster_id, work_area))
            .into_iter()
            .flat_map(|layout| layout.placements)
            .filter(|placement| {
                !self.admission_floats.contains(&placement.node_id)
                    && !self.member_floats.is_floating(placement.node_id)
                    && dragging != Some(placement.node_id)
            })
            .map(|placement| {
                let local = Rectangle::<i32, Logical>::new(
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
                WorkspaceSurfaceTarget {
                    node_id: placement.node_id,
                    geometry: Rectangle::new(output_geometry.loc + local.loc, local.size),
                }
            })
            .collect::<Vec<_>>();
        targets.extend(
            self.member_floats
                .placements_on(output)
                .into_iter()
                .filter(|(member, _)| dragging != Some(*member))
                .filter(|(member, _)| {
                    let Some(cluster) = self.cluster_for_member(*member) else {
                        return false;
                    };
                    self.metadata(cluster)
                        .is_some_and(|metadata| self.active_on(&metadata.output) == Some(cluster))
                })
                .map(|(member, local)| WorkspaceSurfaceTarget {
                    node_id: member,
                    geometry: Rectangle::new(output_geometry.loc + local.loc, local.size),
                }),
        );
        targets
    }

    pub(crate) fn prepare_surface_target(
        &mut self,
        node_id: NodeId,
        current: Rectangle<i32, Logical>,
        target: Rectangle<i32, Logical>,
    ) -> bool {
        self.surfaces.prepare(node_id, current, target)
    }

    pub(crate) fn defer_surface_layout_until(&mut self, node_id: NodeId, until: Duration) {
        self.surfaces.defer_layout_until(node_id, until);
    }

    pub(crate) fn surface_layout_is_deferred(&self, node_id: NodeId, now: Duration) -> bool {
        self.surfaces.layout_is_deferred(node_id, now)
    }

    pub(super) fn forget_surface_state(&mut self, node_id: NodeId) {
        self.surfaces.forget(node_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_core::field::{Field, Vec2};

    fn active_system(
        layout: halley_core::cluster::layout::ClusterWorkspaceLayoutKind,
    ) -> ClusterSystem {
        let mut field = Field::new();
        let first = field.spawn_surface(
            "first",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 640.0, y: 480.0 },
        );
        let second = field.spawn_surface(
            "second",
            Vec2 { x: 200.0, y: 100.0 },
            Vec2 { x: 640.0, y: 480.0 },
        );
        let mut system = ClusterSystem::new(
            halley_config::Clusters::default(),
            halley_config::ClusterAnimation::default(),
        );
        system.begin_creation("DP-1".into());
        system.toggle_creation_member(first, "DP-1");
        system.toggle_creation_member(second, "DP-1");
        system.begin_naming();
        let cluster = system.finish_creation(&mut field).expect("cluster");
        system.metadata.get_mut(&cluster).expect("metadata").layout = layout;
        system.active.insert("DP-1".into(), cluster);
        system
    }

    #[test]
    fn unchanged_target_is_not_configured_twice() {
        let mut state = WorkspaceSurfaceState::default();
        let id = NodeId::new(4);
        let current = Rectangle::new((10, 20).into(), (800, 600).into());
        let target = Rectangle::new((0, 0).into(), (1_000, 700).into());

        assert!(state.prepare(id, current, target));
        assert!(!state.prepare(id, current, target));
        assert_eq!(state.restore.get(&id), Some(&current));
    }

    #[test]
    fn a_layout_change_emits_a_new_target_without_losing_restore_geometry() {
        let mut state = WorkspaceSurfaceState::default();
        let id = NodeId::new(9);
        let current = Rectangle::new((40, 80).into(), (640, 480).into());
        let first = Rectangle::new((0, 0).into(), (900, 700).into());
        let second = Rectangle::new((900, 0).into(), (500, 700).into());

        assert!(state.prepare(id, current, first));
        assert!(state.prepare(id, first, second));
        assert_eq!(state.restore.get(&id), Some(&current));
    }

    #[test]
    fn invalidating_a_dragged_target_preserves_its_restore_geometry() {
        let mut state = WorkspaceSurfaceState::default();
        let id = NodeId::new(1);
        let restore = Rectangle::new((20, 30).into(), (800, 600).into());
        let target = Rectangle::new((0, 0).into(), (1_000, 700).into());
        assert!(state.prepare(id, restore, target));

        state.invalidate_target(id);

        assert!(!state.requested.contains_key(&id));
        assert_eq!(state.restore.get(&id), Some(&restore));
        assert!(state.prepare(id, restore, target));
    }

    #[test]
    fn tiling_targets_are_output_global_and_keep_master_wider() {
        let system =
            active_system(halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Tiling);
        let targets = system.workspace_surface_targets(
            "DP-1",
            Rectangle::new((0, 0).into(), (1_600, 900).into()),
            Rectangle::new((1_920, 0).into(), (1_600, 900).into()),
        );

        assert_eq!(targets.len(), 2);
        assert!(targets.iter().all(|target| target.geometry.loc.x >= 1_920));
        assert!(targets[0].geometry.size.w > targets[1].geometry.size.w);
    }

    #[test]
    fn stacking_targets_retain_distinct_native_client_sizes() {
        let system =
            active_system(halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Stacking);
        let targets = system.workspace_surface_targets(
            "DP-1",
            Rectangle::new((0, 0).into(), (1_600, 900).into()),
            Rectangle::new((0, 0).into(), (1_600, 900).into()),
        );

        assert_eq!(targets.len(), 2);
        assert_ne!(targets[0].geometry.size, targets[1].geometry.size);
    }

    #[test]
    fn deferred_admission_keeps_field_presentation_until_the_deadline() {
        let mut system =
            active_system(halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Tiling);
        let member = system.member_ids(system.active_on("DP-1").unwrap())[0];
        let deadline = Duration::from_secs(2);
        system.defer_surface_layout_until(member, deadline);
        let work_area = Rectangle::new((0, 0).into(), (1_600, 900).into());

        assert!(system.surface_layout_is_deferred(member, deadline - Duration::from_nanos(1)));
        assert_eq!(
            system.window_presentation(
                member,
                "DP-1",
                work_area,
                None,
                deadline - Duration::from_nanos(1),
            ),
            crate::clusters::WindowPresentation::Field
        );
        assert!(!system.surface_layout_is_deferred(member, deadline));
        assert!(matches!(
            system.window_presentation(member, "DP-1", work_area, None, deadline),
            crate::clusters::WindowPresentation::Workspace { .. }
        ));
    }

    #[test]
    fn rearming_deferred_admission_never_shortens_the_lease() {
        let mut state = WorkspaceSurfaceState::default();
        let id = NodeId::new(7);
        state.defer_layout_until(id, Duration::from_secs(4));
        state.defer_layout_until(id, Duration::from_secs(3));

        assert!(state.layout_is_deferred(id, Duration::from_secs(3)));
        assert!(!state.layout_is_deferred(id, Duration::from_secs(4)));
    }
}
