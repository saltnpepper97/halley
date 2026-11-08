use std::error::Error;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::output::Output;
use smithay::utils::{Logical, Physical, Rectangle};

use crate::render::node::NodeRenderer;
use crate::render::scene::SceneElement;
use crate::render::text::UiTextRenderer;

pub(crate) struct BloomElementContext<'a> {
    pub(crate) output: &'a Output,
    pub(crate) output_geometry: Rectangle<i32, Logical>,
    pub(crate) clusters: &'a crate::clusters::ClusterSystem,
    pub(crate) nodes: &'a crate::nodes::NodesState,
    pub(crate) cameras: &'a crate::presentation::camera::OutputCameras,
    pub(crate) now: std::time::Duration,
    pub(crate) cluster_renderer: &'a mut crate::clusters::render::ClusterRenderer,
    pub(crate) node_renderer: &'a mut NodeRenderer,
    pub(crate) ui_text: &'a mut UiTextRenderer,
}

pub(crate) fn elements(
    renderer: &mut GlesRenderer,
    context: BloomElementContext<'_>,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let BloomElementContext {
        output,
        output_geometry,
        clusters,
        nodes,
        cameras,
        now,
        cluster_renderer,
        node_renderer,
        ui_text,
    } = context;
    let Some(camera) = cameras.get(&output.name()) else {
        return Ok(Vec::new());
    };
    let layout = clusters.bloom_layout(&output.name(), camera, output_geometry, now);
    if layout.is_empty() {
        return Ok(Vec::new());
    }

    let ring = crate::render::scene::node_ring_color(nodes.config(), false);
    let fill = crate::render::scene::node_fill_color(nodes.config(), ring);
    let text = crate::render::scene::contrast_text_rgb(fill);
    let allow_real_icons = clusters.config().show_icons
        && matches!(
            nodes.config().show_app_icons,
            halley_config::NodeDisplayPolicy::Always
        );
    let mut contents = Vec::new();
    let mut tokens = Vec::new();
    let mut labels = Vec::new();
    let hovered = clusters.overlay_hovered_on_output(&output.name());
    let pulled = clusters.bloom_pull().map(|(_, member, _, _)| member);

    for token in layout {
        let center = token.center - output_geometry.loc;
        let side = token.radius * 2;
        let destination = Rectangle::<i32, Physical>::new(
            (center.x - token.radius, center.y - token.radius).into(),
            (side, side).into(),
        );
        tokens.push(SceneElement::ClusterCore(
            cluster_renderer.core_with_alpha(
                renderer,
                destination,
                ring,
                fill,
                nodes.config().opacity,
                token.alpha,
            )?,
        ));

        let record = nodes.record(token.member_id);
        let label = record
            .map(|record| record.title.as_str())
            .or_else(|| {
                nodes
                    .field()
                    .node(token.member_id)
                    .map(|node| node.label.as_str())
            })
            .unwrap_or("Window");
        let hover_mix = clusters.overlay_label_hover_mix(
            token.member_id,
            hovered == Some(token.member_id) && pulled != Some(token.member_id),
        );
        labels.extend(crate::render::scene::nodes::landmark_label_elements(
            renderer,
            node_renderer,
            ui_text,
            crate::render::scene::nodes::LandmarkLabel {
                center: (center.x, center.y),
                marker_side: side,
                output_size: (output_geometry.size.w, output_geometry.size.h),
                text: label,
                shape: nodes.config().label_shape,
                fill,
                ring,
                hover_mix,
                alpha: token.alpha,
            },
        )?);
        let real_icon = allow_real_icons
            .then(|| record.and_then(|record| record.app_id.as_deref()))
            .flatten()
            .and_then(|app_id| {
                node_renderer.request_app_icon(renderer, app_id);
                let icon_side = (side as f32 * 0.64).round() as i32;
                node_renderer.app_icon_element(
                    renderer,
                    app_id,
                    Rectangle::new(
                        (center.x - icon_side / 2, center.y - icon_side / 2).into(),
                        (icon_side, icon_side).into(),
                    ),
                    nodes.config().opacity * token.alpha,
                )
            });
        if let Some(icon) = real_icon {
            contents.push(SceneElement::NodeTexture(icon));
            continue;
        }

        let glyph = record
            .and_then(|record| record.app_id.as_deref())
            .or_else(|| {
                nodes
                    .field()
                    .node(token.member_id)
                    .map(|node| node.label.as_str())
            })
            .and_then(|label| label.chars().find(char::is_ascii_alphanumeric))
            .map(|character| character.to_ascii_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string());
        if let Some(size) = ui_text.measure(renderer, &glyph, text)?
            && let Some(prepared) = ui_text.element(
                renderer,
                (center.x - size.w / 2, center.y - size.h / 2).into(),
                &glyph,
                text,
                nodes.config().opacity * token.alpha,
            )?
        {
            contents.push(SceneElement::UiText(prepared.element));
        }
    }

    // Scene order is front-to-back: member identity must stay in front of
    // the circle token that carries it.
    contents.extend(tokens);
    labels.extend(contents);
    Ok(labels)
}
