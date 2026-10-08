use super::*;
use crate::render::scene::nodes::{node_fill_color, node_ring_color};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CoreVisualFlags {
    identity_highlighted: bool,
    join_border_ready: bool,
}

fn show_core_action_controls(
    id: halley_core::cluster::ClusterId,
    bloom_open: bool,
    edit_target: Option<halley_core::cluster::ClusterId>,
) -> bool {
    bloom_open && edit_target == Some(id)
}

fn core_visual_flags(focused: bool, hovered: bool, join_ready: bool) -> CoreVisualFlags {
    CoreVisualFlags {
        identity_highlighted: focused || hovered,
        join_border_ready: join_ready,
    }
}

#[derive(Clone, Copy)]
pub(super) struct CollapsedCoreVisuals {
    pub(super) ring: (f32, f32, f32),
    pub(super) fill: (f32, f32, f32),
    pub(super) icon_colors: [[u8; 4]; 2],
}

pub(super) fn collapsed_core_visuals(
    nodes: halley_config::Nodes,
    highlighted: bool,
) -> CollapsedCoreVisuals {
    let ring = node_ring_color(nodes, highlighted);
    CollapsedCoreVisuals {
        ring,
        fill: node_fill_color(nodes, ring),
        icon_colors: [
            rgba(nodes.border_color),
            rgba(nodes.border_color_highlighted),
        ],
    }
}

pub(super) struct ClusterElementContext<'a> {
    pub(super) output: &'a Output,
    pub(super) primary_output: &'a Output,
    pub(super) output_geometry: Rectangle<i32, Logical>,
    pub(super) space: &'a smithay::desktop::Space<smithay::desktop::Window>,
    pub(super) clusters: &'a crate::clusters::ClusterSystem,
    pub(super) nodes: &'a crate::nodes::NodesState,
    pub(super) cameras: &'a crate::presentation::camera::OutputCameras,
    pub(super) window_animations: &'a crate::animation::WindowAnimations,
    pub(super) fullscreen: &'a crate::wayland::fullscreen::FullscreenManager,
    pub(super) maximize: &'a crate::presentation::maximize::FieldMaximizeManager,
    pub(super) decorations: &'a halley_config::Decorations,
    pub(super) font: &'a halley_config::Font,
    pub(super) pins: &'a halley_config::Pins,
    pub(super) overlays: &'a halley_config::Overlays,
    pub(super) pin_renderer: &'a mut crate::render::pin::PinRenderer,
    pub(super) shadow_config: halley_config::ShadowLayer,
    pub(super) shadow_renderer: &'a mut crate::render::effects::shadow::ShadowRenderer,
    pub(super) node_grab_active: bool,
    pub(super) now: std::time::Duration,
    pub(super) node_renderer: &'a mut crate::render::node::NodeRenderer,
    pub(super) ui_text: &'a mut crate::render::text::UiTextRenderer,
}

pub(super) fn cluster_elements(
    renderer: &mut GlesRenderer,
    cluster_renderer: &mut crate::clusters::render::ClusterRenderer,
    context: ClusterElementContext<'_>,
) -> Result<Vec<StackGroup>, Box<dyn Error>> {
    let ClusterElementContext {
        output,
        primary_output,
        output_geometry,
        space,
        clusters,
        nodes,
        cameras,
        window_animations,
        fullscreen,
        maximize,
        decorations,
        font,
        pins,
        overlays,
        pin_renderer,
        shadow_config,
        shadow_renderer,
        node_grab_active,
        now,
        node_renderer,
        ui_text,
    } = context;
    if clusters.active_on(&output.name()).is_some() {
        return Ok(Vec::new());
    }
    let Some(camera) = cameras.get(&output.name()) else {
        return Ok(Vec::new());
    };
    let focused_node = nodes.focused();
    let output_name = output.name();
    let join_readiness = clusters.join_readiness_on_output(&output_name);

    // Labels share the desktop stack with the cluster core. Use the same
    // presentation geometry as live-window rendering so a label never chooses
    // space that only appears empty before camera or opening transforms.
    let mut fixed_label_obstacles = space
        .elements()
        .filter(|window| crate::wayland::window_is_on_output(window, output, primary_output))
        .filter_map(|window| {
            crate::presentation::window::window_visual_state(
                space,
                cameras,
                Some(clusters),
                Some(nodes),
                window,
                output,
                window_animations,
                fullscreen,
                maximize,
                decorations,
                font,
                now,
            )
        })
        .filter(|visual| visual.opening_alpha > 0.01)
        .map(|visual| visual.animated_rect)
        .collect::<Vec<_>>();
    fixed_label_obstacles.extend(
        nodes
            .collapsed_on_output(&output_name)
            .filter(|record| clusters.cluster_for_member(record.id).is_none())
            .filter_map(|record| {
                let node = nodes.field.node(record.id)?;
                let position = nodes.landmark_position(record.id, node.pos, now);
                let center = crate::nodes::screen_from_world(position, camera, output_geometry)
                    - output_geometry.loc;
                let side = crate::nodes::NODE_DIAMETER_PX.round() as i32;
                Some(Rectangle::<i32, Physical>::new(
                    (center.x - side / 2, center.y - side / 2).into(),
                    (side, side).into(),
                ))
            }),
    );
    let core_label_obstacles = clusters
        .collapsed_core_landmarks()
        .into_iter()
        .filter(|(_, _, core_output, _, _)| core_output == &output_name)
        .map(|(cluster, core, _, position, _)| {
            let position = nodes.landmark_position(core, position, now);
            let center = crate::nodes::screen_from_world(position, camera, output_geometry)
                - output_geometry.loc;
            let side = crate::clusters::CORE_DIAMETER_PX.round() as i32;
            (
                cluster,
                Rectangle::<i32, Physical>::new(
                    (center.x - side / 2, center.y - side / 2).into(),
                    (side, side).into(),
                ),
            )
        })
        .collect::<Vec<_>>();

    let mut groups = Vec::new();
    for (_, id, metadata) in clusters.clusters_for_output(&output.name()) {
        let focused = focused_node.is_some_and(|node| {
            clusters.cluster_for_member(node) == Some(id)
                || clusters.cluster_for_core(node) == Some(id)
        });
        let hovered = clusters.hovered_core() == Some(id);
        let bloom_open = clusters.bloom_open_on_output(&output.name()) == Some(id);
        let join_ready = join_readiness.is_some_and(|readiness| readiness.cluster_id == id);
        let visual_flags = core_visual_flags(focused, hovered, join_ready);
        let highlighted = visual_flags.identity_highlighted;
        let core_position = clusters
            .registry()
            .cluster(id)
            .and_then(|cluster| cluster.core_node())
            .map_or(metadata.core_position, |core| {
                nodes.landmark_position(core, metadata.core_position, now)
            });
        let center = crate::nodes::screen_from_world(core_position, camera, output_geometry);
        let local = center - output_geometry.loc;
        let side = crate::clusters::CORE_DIAMETER_PX.round() as i32;
        let destination = Rectangle::<i32, Physical>::new(
            (local.x - side / 2, local.y - side / 2).into(),
            (side, side).into(),
        );
        let visual = collapsed_core_visuals(nodes.config, highlighted);
        let ring = visual.ring;
        let core_border = if visual_flags.join_border_ready {
            let highlighted = nodes.config.border_color_highlighted;
            (highlighted.r, highlighted.g, highlighted.b)
        } else {
            ring
        };
        let fill = visual.fill;
        let mut elements = Vec::new();
        if clusters
            .registry()
            .cluster(id)
            .is_some_and(|cluster| cluster.pinned)
            && let Some(pin) = pin_renderer.element(
                renderer,
                &output.name(),
                crate::render::pin::PinSlot::Cluster(id.as_u64()),
                crate::render::pin::landmark_badge_rect(pins, (local.x, local.y), side),
                1.0,
                pins,
                overlays,
                decorations,
            )
        {
            elements.push(SceneElement::Closing(pin));
        }
        let hover_mix = if bloom_open {
            0.0
        } else {
            match (node_grab_active, nodes.config.show_labels) {
                (true, halley_config::NodeDisplayPolicy::Hover) => {
                    clusters.label_hover_mix(id, false)
                }
                (true, _) | (_, halley_config::NodeDisplayPolicy::Off) => 0.0,
                (false, halley_config::NodeDisplayPolicy::Hover) => {
                    clusters.label_hover_mix(id, hovered)
                }
                (false, halley_config::NodeDisplayPolicy::Always) => 1.0,
            }
        };
        let label_obstacles = fixed_label_obstacles
            .iter()
            .copied()
            .chain(
                core_label_obstacles
                    .iter()
                    .filter_map(|(other, obstacle)| (*other != id).then_some(*obstacle)),
            )
            .collect::<Vec<_>>();
        elements.extend(super::nodes::landmark_label_elements_avoiding(
            renderer,
            node_renderer,
            ui_text,
            super::nodes::LandmarkLabel {
                center: (local.x, local.y),
                marker_side: side,
                output_size: (output_geometry.size.w, output_geometry.size.h),
                text: &metadata.name,
                shape: nodes.config.label_shape,
                fill,
                ring,
                hover_mix,
                alpha: 1.0,
            },
            &label_obstacles,
        )?);
        if show_core_action_controls(
            id,
            bloom_open,
            clusters.bloom_edit_target_on_output(&output_name),
        ) {
            let text = contrast_text_rgb(fill);
            for (control, button) in crate::clusters::action_button_rects(center, output_geometry) {
                let button = Rectangle::<i32, Physical>::new(
                    (
                        button.loc.x - output_geometry.loc.x,
                        button.loc.y - output_geometry.loc.y,
                    )
                        .into(),
                    (button.size.w, button.size.h).into(),
                );
                let glyph_side = 14;
                let glyph = Rectangle::new(
                    (
                        button.loc.x + (button.size.w - glyph_side) / 2,
                        button.loc.y + (button.size.h - glyph_side) / 2,
                    )
                        .into(),
                    (glyph_side, glyph_side).into(),
                );
                let icon = match control {
                    crate::clusters::ClusterActionControl::Close => cluster_renderer.close_icon(
                        renderer,
                        glyph,
                        [text[0], text[1], text[2], 255],
                        nodes.config.opacity,
                    )?,
                    crate::clusters::ClusterActionControl::Edit => cluster_renderer.edit_icon(
                        renderer,
                        glyph,
                        [text[0], text[1], text[2], 255],
                        nodes.config.opacity,
                    )?,
                };
                elements.push(SceneElement::ClusterIcon(icon));
                elements.push(SceneElement::ClusterCore(cluster_renderer.core(
                    renderer,
                    button,
                    ring,
                    fill,
                    nodes.config.opacity,
                    false,
                )?));
            }
        }
        if clusters.config().show_icons {
            let icon_side =
                ((side as f32 * nodes.config.icon_size * 0.98).round() as i32).clamp(16, 42);
            elements.push(SceneElement::ClusterIcon(cluster_renderer.icon(
                renderer,
                Rectangle::new(
                    (local.x - icon_side / 2, local.y - icon_side / 2).into(),
                    (icon_side, icon_side).into(),
                ),
                highlighted,
                visual.icon_colors,
                nodes.config.opacity,
            )?));
        }
        elements.push(SceneElement::ClusterCore(cluster_renderer.core(
            renderer,
            destination,
            core_border,
            fill,
            nodes.config.opacity,
            visual_flags.join_border_ready,
        )?));
        if let Some(shadow) = shadow_renderer.element(
            renderer,
            format!("{}:cluster:{}", output.name(), id.as_u64()),
            destination,
            side as f32 / 2.0,
            nodes.config.opacity,
            shadow_config,
        )? {
            elements.push(SceneElement::Shadow(shadow));
        }
        groups.push(StackGroup {
            window: None,
            // Cluster cores are desktop objects. They remain below live
            // windows while sharing the same stack as nodes and closing
            // snapshots, instead of leaking into Halley's overlay plane.
            stack_index: 0,
            order: id.as_u64(),
            elements,
        });
    }
    Ok(groups)
}

fn rgba(color: halley_config::BorderColor) -> [u8; 4] {
    [
        (color.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        255,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_controls_belong_to_the_open_bloom_core_only() {
        let first = halley_core::cluster::ClusterId::new(1);
        let second = halley_core::cluster::ClusterId::new(2);
        assert!(show_core_action_controls(first, true, Some(first)));
        assert!(!show_core_action_controls(first, false, Some(first)));
        assert!(!show_core_action_controls(first, true, Some(second)));
        assert!(!show_core_action_controls(first, true, None));
    }

    #[test]
    fn collapsed_core_palette_is_owned_by_nodes() {
        let nodes = halley_config::Nodes {
            border_color: halley_config::BorderColor {
                r: 0.1,
                g: 0.2,
                b: 0.3,
            },
            border_color_highlighted: halley_config::BorderColor {
                r: 0.8,
                g: 0.4,
                b: 0.2,
            },
            ..halley_config::Nodes::default()
        };

        let idle = collapsed_core_visuals(nodes, false);
        let highlighted = collapsed_core_visuals(nodes, true);
        assert_eq!(idle.ring, (0.1, 0.2, 0.3));
        assert_eq!(highlighted.ring, (0.8, 0.4, 0.2));
        assert_eq!(idle.icon_colors, [[26, 51, 77, 255], [204, 102, 51, 255]]);
    }

    #[test]
    fn join_readiness_changes_only_the_core_border_highlight_state() {
        assert_eq!(
            core_visual_flags(false, false, true),
            CoreVisualFlags {
                identity_highlighted: false,
                join_border_ready: true,
            }
        );
    }
}
