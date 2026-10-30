//! Translate node and cluster presentation into physics bodies.
//!
//! This module only observes the state needed for collision geometry; session
//! lifecycle and synchronization remain with the caller.

use super::*;

pub(super) struct DynamicsBodyContext<'a> {
    pub(super) nodes: &'a NodesState,
    pub(super) clusters: &'a crate::clusters::ClusterSystem,
    pub(super) cameras: &'a crate::presentation::camera::OutputCameras,
    pub(super) placement: PlacementContext<'a>,
}

/// Builds physics inputs without granting the collector access to the session.
pub(super) fn collect_dynamics_bodies(
    context: DynamicsBodyContext<'_>,
    scale_override: Option<(&str, f32)>,
) -> Vec<dynamics::Body> {
    let placement = context.placement;
    let mut bodies = context
        .nodes
        .records()
        .filter(|record| {
            record.attached
                && !context.clusters.is_member(record.id)
                && (record.collapsed || placement.includes_window(&record.surface))
        })
        .filter_map(|record| {
            let node = context.nodes.field.node(record.id)?;
            let scale = scale_override
                .filter(|(output, _)| *output == record.output)
                .map(|(_, scale)| scale)
                .or_else(|| {
                    context
                        .cameras
                        .get(&record.output)
                        .map(crate::presentation::camera::scale)
                })
                .unwrap_or(1.0)
                .max(0.05);
            let (kind, extents) = if record.collapsed {
                (
                    dynamics::BodyKind::Node,
                    dynamics::CollisionExtents::symmetric(Vec2 {
                        x: NODE_DIAMETER_PX * 0.5 / scale,
                        y: NODE_DIAMETER_PX * 0.5 / scale,
                    }),
                )
            } else {
                let outer = crate::titlebar::outer_rect_for_client(
                    &record.window,
                    record.geometry,
                    context.placement.decorations,
                    context.placement.font,
                );
                let center_x = record.geometry.loc.x as f32 + record.geometry.size.w as f32 * 0.5;
                let center_y = record.geometry.loc.y as f32 + record.geometry.size.h as f32 * 0.5;
                (
                    dynamics::BodyKind::Window,
                    dynamics::CollisionExtents {
                        left: center_x - outer.loc.x as f32,
                        right: outer.loc.x as f32 + outer.size.w as f32 - center_x,
                        top: center_y - outer.loc.y as f32,
                        bottom: outer.loc.y as f32 + outer.size.h as f32 - center_y,
                    },
                )
            };
            Some(dynamics::Body {
                id: record.id,
                kind,
                pos: node.pos,
                extents,
                gap: context.nodes.landmarks.gap_px / scale,
                pinned: node.pinned || context.nodes.release_locked(record.id),
                output: record.output.clone(),
            })
        })
        .collect::<Vec<_>>();
    bodies.extend(context.clusters.collapsed_core_landmarks().into_iter().map(
        |(_, id, output, pos, pinned)| {
            let scale = scale_override
                .filter(|(override_output, _)| *override_output == output)
                .map(|(_, scale)| scale)
                .or_else(|| {
                    context
                        .cameras
                        .get(&output)
                        .map(crate::presentation::camera::scale)
                })
                .unwrap_or(1.0)
                .max(0.05);
            dynamics::Body {
                id,
                kind: dynamics::BodyKind::Node,
                pos,
                extents: dynamics::CollisionExtents::symmetric(Vec2 {
                    x: crate::clusters::CORE_DIAMETER_PX * 0.5 / scale,
                    y: crate::clusters::CORE_DIAMETER_PX * 0.5 / scale,
                }),
                gap: context.nodes.landmarks.gap_px / scale,
                pinned,
                output,
            }
        },
    ));
    bodies
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_core::cluster::layout::ClusterWorkspaceLayoutKind;

    #[test]
    fn cluster_cores_use_override_only_on_the_selected_output() {
        let config = halley_config::RuntimeConfig::default();
        let mut nodes = NodesState::new(&config);
        let mut clusters =
            crate::clusters::ClusterSystem::new(config.clusters, config.animations.cluster);
        for output in ["DP-1", "DP-2"] {
            let member = nodes.field.spawn_surface(
                output,
                Vec2 { x: 0.0, y: 0.0 },
                Vec2 { x: 200.0, y: 150.0 },
            );
            clusters
                .create_collapsed_cluster(
                    &mut nodes.field,
                    output.into(),
                    output.into(),
                    ClusterWorkspaceLayoutKind::Tiling,
                    vec![member],
                    Vec2 { x: 0.0, y: 0.0 },
                )
                .expect("collapsed cluster");
        }
        let cameras = crate::presentation::camera::OutputCameras::default();
        let fullscreen =
            crate::wayland::fullscreen::FullscreenManager::new(config.animations.clone());
        let maximize = crate::presentation::maximize::FieldMaximizeManager::new(
            config.field,
            config.animations.clone(),
        );
        let bodies = collect_dynamics_bodies(
            DynamicsBodyContext {
                nodes: &nodes,
                clusters: &clusters,
                cameras: &cameras,
                placement: PlacementContext {
                    decorations: &config.decorations,
                    font: &config.font,
                    fullscreen: &fullscreen,
                    maximize: &maximize,
                },
            },
            Some(("DP-1", 2.0)),
        );
        assert_eq!(bodies.len(), 2);
        let first = bodies.iter().find(|body| body.output == "DP-1").unwrap();
        let second = bodies.iter().find(|body| body.output == "DP-2").unwrap();
        assert_eq!(first.kind, dynamics::BodyKind::Node);
        assert_eq!(first.extents.left * 2.0, second.extents.left);
        assert_eq!(first.gap * 2.0, second.gap);
    }
}
