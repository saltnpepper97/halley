use super::nodes::{fit_ui_text, node_fill_color, node_ring_color};
use super::overview::lerp_rect;
use super::*;

pub(super) struct ApogeeCoreTileContext<'a> {
    pub(super) output_geometry: Rectangle<i32, Logical>,
    pub(super) tile: &'a crate::shell::apogee::Tile,
    pub(super) cluster: halley_core::cluster::ClusterId,
    pub(super) progress: f32,
    pub(super) chrome_alpha: f32,
    pub(super) highlighted: bool,
    pub(super) overlay_visuals: crate::render::overlays::shell::OverlayVisuals,
    pub(super) cameras: &'a crate::presentation::camera::OutputCameras,
    pub(super) nodes: &'a crate::nodes::NodesState,
    pub(super) clusters: &'a crate::clusters::ClusterSystem,
    pub(super) node_renderer: &'a mut crate::render::node::NodeRenderer,
    pub(super) cluster_renderer: &'a mut crate::clusters::render::ClusterRenderer,
    pub(super) ui_text: &'a mut crate::render::text::UiTextRenderer,
    pub(super) now: std::time::Duration,
}

/// Render one cluster identity in the dedicated upper rail. Apogee owns only
/// this temporary presentation; the ClusterSystem remains unchanged beneath
/// the replacement scene, including an already-open workspace.
pub(super) fn apogee_core_tile_elements(
    renderer: &mut GlesRenderer,
    context: ApogeeCoreTileContext<'_>,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let ApogeeCoreTileContext {
        output_geometry,
        tile,
        cluster,
        progress,
        chrome_alpha,
        highlighted,
        overlay_visuals,
        cameras,
        nodes,
        clusters,
        node_renderer,
        cluster_renderer,
        ui_text,
        now,
    } = context;
    let Some(metadata) = clusters
        .metadata(cluster)
        .filter(|metadata| metadata.output == tile.output)
    else {
        return Ok(Vec::new());
    };
    let Some(camera) = cameras.get(&tile.output) else {
        return Ok(Vec::new());
    };
    let position = nodes.landmark_position(tile.id, metadata.core_position, now);
    let center =
        crate::nodes::screen_from_world(position, camera, output_geometry) - output_geometry.loc;
    let side = crate::clusters::CORE_DIAMETER_PX.round() as i32;
    let source = Rectangle::<i32, Physical>::new(
        (center.x - side / 2, center.y - side / 2).into(),
        (side, side).into(),
    );
    let target = Rectangle::<i32, Physical>::new(
        (tile.target.loc - output_geometry.loc).to_physical(1),
        tile.target.size.to_physical(1),
    );
    let body = lerp_rect(source, target, progress);
    let ring = node_ring_color(nodes.config(), highlighted);
    let fill = node_fill_color(nodes.config(), ring);
    let icon_colors = [
        rgba(nodes.config().border_color),
        rgba(nodes.config().border_color_highlighted),
    ];
    let mut elements = Vec::new();

    let (label, label_size) = fit_ui_text(
        renderer,
        ui_text,
        &metadata.name,
        overlay_visuals.text.bytes(),
        184,
    )?;
    if !label.is_empty() {
        let label_rect = Rectangle::<i32, Physical>::new(
            (
                body.loc.x + (body.size.w - label_size.w - 16) / 2,
                body.loc.y + body.size.h + 7,
            )
                .into(),
            (label_size.w + 16, label_size.h + 8).into(),
        );
        if let Some(text) = ui_text.element(
            renderer,
            (
                label_rect.loc.x + (label_rect.size.w - label_size.w) / 2,
                label_rect.loc.y + (label_rect.size.h - label_size.h) / 2,
            )
                .into(),
            &label,
            overlay_visuals.text.bytes(),
            chrome_alpha,
        )? {
            elements.push(SceneElement::UiText(text.element));
        }
        elements.push(SceneElement::NodeLabel(
            crate::render::overlays::shell::label_card_element(
                renderer,
                node_renderer,
                label_rect,
                overlay_visuals,
                overlay_visuals.fill,
                0.90 * chrome_alpha,
            )?,
        ));
    }
    if clusters.config().show_icons {
        let icon_side = ((body.size.w.min(body.size.h) as f32 * nodes.config().icon_size * 0.98)
            .round() as i32)
            .clamp(16, 42);
        elements.push(SceneElement::ClusterIcon(
            cluster_renderer.icon(
                renderer,
                Rectangle::new(
                    (
                        body.loc.x + (body.size.w - icon_side) / 2,
                        body.loc.y + (body.size.h - icon_side) / 2,
                    )
                        .into(),
                    (icon_side, icon_side).into(),
                ),
                highlighted,
                icon_colors,
                1.0,
            )?,
        ));
    }
    elements.push(SceneElement::ClusterCore(
        cluster_renderer.core_with_alpha(
            renderer,
            body,
            ring,
            fill,
            nodes.config().opacity,
            1.0,
        )?,
    ));
    Ok(elements)
}

fn rgba(color: halley_config::BorderColor) -> [u8; 4] {
    [
        (color.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        255,
    ]
}
