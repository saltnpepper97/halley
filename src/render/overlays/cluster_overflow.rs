use std::error::Error;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::output::Output;
use smithay::utils::Rectangle;

use crate::render::node::NodeRenderer;
use crate::render::scene::SceneElement;
use crate::render::text::UiTextRenderer;

use super::shell::{label_card_element, resolve_visuals};

pub(crate) struct OverflowElementContext<'a> {
    pub(crate) output: &'a Output,
    pub(crate) clusters: &'a crate::clusters::ClusterSystem,
    pub(crate) nodes: &'a crate::nodes::NodesState,
    pub(crate) config: &'a halley_config::Overlays,
    pub(crate) now: std::time::Duration,
    pub(crate) node_renderer: &'a mut NodeRenderer,
    pub(crate) ui_text: &'a mut UiTextRenderer,
}

pub(crate) fn elements(
    renderer: &mut GlesRenderer,
    context: OverflowElementContext<'_>,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let OverflowElementContext {
        output,
        clusters,
        nodes,
        config,
        now,
        node_renderer,
        ui_text,
    } = context;
    let work_area = smithay::desktop::layer_map_for_output(output).non_exclusive_zone();
    let Some(layout) = clusters.overflow_layout(&output.name(), work_area, now) else {
        return Ok(Vec::new());
    };
    let visuals = resolve_visuals(config);
    let visibility = layout.visibility;
    let mut contents = Vec::new();
    let mut chips = Vec::new();
    let mut labels = Vec::new();
    let hovered = clusters.overlay_hovered_on_output(&output.name());
    let drag = clusters
        .overflow_drag()
        .filter(|(drag_output, _)| drag_output == &output.name());
    let dragging = drag.is_some();
    let allow_icons = clusters.config().tiling.overflow_show_icons
        && matches!(
            nodes.config().show_app_icons,
            halley_config::NodeDisplayPolicy::Always
        );

    let mut render_items = layout
        .items
        .iter()
        .filter(|item| {
            drag.as_ref()
                .is_none_or(|(_, drag)| drag.member_id != item.node_id)
        })
        .map(|item| (item.node_id, item.rect, visibility))
        .collect::<Vec<_>>();
    if let Some((_, drag)) = drag.as_ref() {
        render_items.push((
            drag.member_id,
            Rectangle::new(
                (
                    drag.output_local.x.round() as i32 - 22,
                    drag.output_local.y.round() as i32 - 22,
                )
                    .into(),
                (44, 44).into(),
            ),
            1.0,
        ));
    }

    for (node_id, logical_rect, alpha) in render_items {
        let rect = logical_rect.to_physical(1);
        chips.push(SceneElement::NodeLabel(label_card_element(
            renderer,
            node_renderer,
            rect,
            visuals,
            visuals.border,
            0.98 * alpha,
        )?));
        let record = nodes.record(node_id);
        let label = record
            .map(|record| record.title.as_str())
            .or_else(|| nodes.field().node(node_id).map(|node| node.label.as_str()))
            .unwrap_or("Window");
        let hover_mix =
            clusters.overlay_label_hover_mix(node_id, hovered == Some(node_id) && !dragging);
        labels.extend(crate::render::scene::nodes::landmark_label_elements(
            renderer,
            node_renderer,
            ui_text,
            crate::render::scene::nodes::LandmarkLabel {
                center: (
                    logical_rect.loc.x + logical_rect.size.w / 2,
                    logical_rect.loc.y + logical_rect.size.h / 2,
                ),
                marker_side: logical_rect.size.w,
                output_size: (
                    work_area.loc.x + work_area.size.w,
                    work_area.loc.y + work_area.size.h,
                ),
                text: label,
                shape: nodes.config().label_shape,
                fill: (visuals.fill.r, visuals.fill.g, visuals.fill.b),
                ring: (visuals.border.r, visuals.border.g, visuals.border.b),
                hover_mix,
                alpha,
            },
        )?);
        let real_icon = allow_icons
            .then(|| record.and_then(|record| record.app_id.as_deref()))
            .flatten()
            .and_then(|app_id| {
                node_renderer.request_app_icon(renderer, app_id);
                node_renderer.app_icon_element(
                    renderer,
                    app_id,
                    Rectangle::new((rect.loc.x + 4, rect.loc.y + 4).into(), (36, 36).into()),
                    alpha,
                )
            });
        if let Some(icon) = real_icon {
            contents.push(SceneElement::NodeTexture(icon));
            continue;
        }
        let glyph = record
            .and_then(|record| record.app_id.as_deref())
            .or_else(|| nodes.field().node(node_id).map(|node| node.label.as_str()))
            .and_then(|label| label.chars().find(char::is_ascii_alphanumeric))
            .map(|ch| ch.to_ascii_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string());
        if let Some(size) = ui_text.measure(renderer, &glyph, visuals.text.bytes())?
            && let Some(text) = ui_text.element(
                renderer,
                (
                    rect.loc.x + (rect.size.w - size.w) / 2,
                    rect.loc.y + (rect.size.h - size.h) / 2,
                )
                    .into(),
                &glyph,
                visuals.text.bytes(),
                alpha,
            )?
        {
            contents.push(SceneElement::UiText(text.element));
        }
    }

    if let Some(scrollbar) = layout.scrollbar {
        chips.push(SceneElement::NodeLabel(label_card_element(
            renderer,
            node_renderer,
            scrollbar.track.to_physical(1),
            visuals,
            visuals.border,
            0.30 * visibility,
        )?));
        chips.push(SceneElement::NodeLabel(label_card_element(
            renderer,
            node_renderer,
            scrollbar.thumb.to_physical(1),
            visuals,
            visuals.border,
            0.88 * visibility,
        )?));
    }

    // Scene elements are front-to-back: content, then its chip, then the
    // enclosing strip. Keeping this explicit prevents cards from masking the
    // icons they contain.
    contents.extend(chips);
    contents.push(SceneElement::NodeLabel(label_card_element(
        renderer,
        node_renderer,
        layout.strip.to_physical(1),
        visuals,
        visuals.fill,
        0.94 * visibility,
    )?));
    labels.extend(contents);
    Ok(labels)
}
