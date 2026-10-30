//! Window presentation and directional tile operations.

use super::*;

impl ClusterSystem {
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
}
