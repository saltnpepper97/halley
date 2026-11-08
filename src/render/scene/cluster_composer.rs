use super::overview::{OverviewDesktop, OverviewRenderers, OverviewScene};
use super::*;

use crate::render::draw::DrawContext;

const RAIL_TOP: i32 = 24;
const RAIL_PAD_X: i32 = 18;
const RAIL_PAD_Y: i32 = 12;
const RAIL_GAP: i32 = 10;
const RAIL_SCREEN_MARGIN: i32 = 16;
const RAIL_TEXT_GUARD: i32 = 12;
const STATUS_PAD_X: i32 = 10;
const STATUS_PAD_Y: i32 = 5;
const SELECTED_BADGE_SIZE: i32 = 30;
const SELECTED_CHECK_SIZE: i32 = 18;
const SELECTED_BADGE_CORNER_OVERLAP: i32 = 5;

/// Thickness of the outline around the highlighted window or node.
const FOCUS_OUTLINE_PX: f32 = 3.0;

/// Cluster creation drawn over the ordinary Field: an outline around the
/// highlighted window or node, a check badge on each selected one, the
/// instruction bar, and the new cluster node fading in as the selected windows
/// fly into it. The windows themselves are the real Field windows.
pub(super) fn cluster_composer_elements(
    scene: OverviewScene<'_>,
    state: &crate::shell::cluster_composer::ClusterComposerState,
    overlay_config: &halley_config::Overlays,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let OverviewScene {
        renderer,
        output,
        output_geometry,
        now,
        desktop,
        draw,
    } = scene;
    let OverviewDesktop {
        cameras,
        nodes,
        clusters,
        ..
    } = desktop;
    let OverviewRenderers {
        node: node_renderer,
        cluster: cluster_renderer,
        text: ui_text,
        ..
    } = draw;
    let Some(session) = state
        .session()
        .filter(|session| session.output == output.name())
    else {
        return Ok(Vec::new());
    };
    let creation = clusters.creation();
    let committing = matches!(
        session.phase(),
        crate::shell::cluster_composer::Phase::Committing
            | crate::shell::cluster_composer::Phase::CommitEndpointHeld
    );
    let commit_progress = session.commit_progress(now);
    let chrome_alpha = if committing {
        (1.0 - commit_progress / 0.62).clamp(0.0, 1.0)
    } else {
        super::overview::chrome_alpha(session.progress(now))
    };
    let overlay_visuals = crate::render::overlays::shell::resolve_visuals(overlay_config);
    let output_local = Rectangle::<i32, Physical>::from_size(output_geometry.size.to_physical(1));

    let mut elements = Vec::new();
    if !committing {
        push_draft_rail(
            &mut crate::render::draw::DrawContext {
                renderer,
                node: node_renderer,
                text: ui_text,
            },
            &mut elements,
            output_local,
            creation,
            overlay_visuals,
            chrome_alpha,
        )?;
    }
    if chrome_alpha > 0.0 {
        for tile in &session.tiles {
            let selected = creation.is_some_and(|creation| creation.selected.contains(&tile.id));
            let focused = session.focused == Some(tile.id);
            if !selected && !focused {
                continue;
            }
            let Some(body) = super::overview::overview_window_source_rect(
                output,
                output_geometry,
                tile.id,
                desktop,
                now,
            ) else {
                continue;
            };
            if selected {
                push_selection_badge(
                    renderer,
                    node_renderer,
                    &mut elements,
                    body,
                    overlay_visuals,
                    chrome_alpha,
                )?;
            }
            if focused {
                let border = overlay_visuals.border;
                let pad = FOCUS_OUTLINE_PX.ceil() as i32 + 2;
                elements.push(SceneElement::DashedOutline(
                    node_renderer.dashed_outline_element(
                        renderer,
                        &output.name(),
                        crate::render::node::NodeSlot::CreationFocusOutline,
                        Rectangle::new(
                            (body.loc.x - pad, body.loc.y - pad).into(),
                            (body.size.w + pad * 2, body.size.h + pad * 2).into(),
                        ),
                        (border.r, border.g, border.b, border.a * chrome_alpha),
                        FOCUS_OUTLINE_PX,
                        (1.0, 1.0),
                    )?,
                ));
            }
        }
    }

    if committing && let Some(prepared) = session.prepared() {
        let alpha = ((commit_progress - 0.68) / 0.32).clamp(0.0, 1.0);
        let core_elements = prepared_core_elements(
            renderer,
            cluster_renderer,
            prepared,
            output,
            output_geometry,
            cameras,
            clusters.config(),
            nodes.config(),
            alpha,
        )?;
        elements.extend(core_elements);
    }
    if session.phase() == crate::shell::cluster_composer::Phase::CommitEndpointHeld {
        session.mark_endpoint_rendered();
    }
    Ok(elements)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepared_core_elements(
    renderer: &mut GlesRenderer,
    cluster_renderer: &mut crate::clusters::render::ClusterRenderer,
    prepared: &crate::clusters::PreparedCreation,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    cameras: &crate::presentation::camera::OutputCameras,
    cluster_config: halley_config::Clusters,
    node_config: halley_config::Nodes,
    alpha: f32,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let Some(destination) = prepared_core_rect(prepared, output, output_geometry, cameras) else {
        return Ok(Vec::new());
    };
    let alpha = alpha.clamp(0.0, 1.0);
    let visual = super::clusters::collapsed_core_visuals(node_config, prepared.focus_core);
    let mut elements = Vec::new();
    if cluster_config.show_icons {
        let icon_side = ((destination.size.w as f32 * node_config.icon_size * 0.98).round() as i32)
            .clamp(16, 42);
        elements.push(SceneElement::ClusterIcon(
            cluster_renderer.icon(
                renderer,
                Rectangle::new(
                    (
                        destination.loc.x + (destination.size.w - icon_side) / 2,
                        destination.loc.y + (destination.size.h - icon_side) / 2,
                    )
                        .into(),
                    (icon_side, icon_side).into(),
                ),
                prepared.focus_core,
                visual.icon_colors,
                node_config.opacity * alpha,
            )?,
        ));
    }
    elements.push(SceneElement::ClusterCore(
        cluster_renderer.core_with_alpha(
            renderer,
            destination,
            visual.ring,
            visual.fill,
            node_config.opacity,
            alpha,
        )?,
    ));
    Ok(elements)
}

fn prepared_core_rect(
    prepared: &crate::clusters::PreparedCreation,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    cameras: &crate::presentation::camera::OutputCameras,
) -> Option<Rectangle<i32, Physical>> {
    let camera = cameras.get(&output.name())?;
    let center = crate::nodes::screen_from_world(prepared.core_position, camera, output_geometry);
    Some(core_rect_for_output(center, output_geometry))
}

fn core_rect_for_output(
    global_center: smithay::utils::Point<i32, Logical>,
    output_geometry: Rectangle<i32, Logical>,
) -> Rectangle<i32, Physical> {
    let center = global_center - output_geometry.loc;
    let side = crate::clusters::CORE_DIAMETER_PX.round() as i32;
    Rectangle::new(
        (center.x - side / 2, center.y - side / 2).into(),
        (side, side).into(),
    )
}

fn selection_badge_rect(card: Rectangle<i32, Physical>) -> Rectangle<i32, Physical> {
    Rectangle::new(
        (
            card.loc.x + card.size.w - SELECTED_BADGE_SIZE + SELECTED_BADGE_CORNER_OVERLAP,
            card.loc.y - SELECTED_BADGE_CORNER_OVERLAP,
        )
            .into(),
        (SELECTED_BADGE_SIZE, SELECTED_BADGE_SIZE).into(),
    )
}

#[allow(clippy::too_many_arguments)]
fn push_selection_badge(
    renderer: &mut GlesRenderer,
    node_renderer: &mut crate::render::node::NodeRenderer,
    elements: &mut Vec<SceneElement>,
    target: Rectangle<i32, Physical>,
    visuals: crate::render::overlays::shell::OverlayVisuals,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    // Pin the compact marker to the outer card corner rather than placing it
    // inside the window preview. A small overlap makes it read as card chrome.
    let card = super::overview::preview_card_rect(
        target,
        visuals
            .border_px
            .max(super::overview::CLUSTER_MEMBER_BORDER_PX),
    );
    let badge = selection_badge_rect(card);
    let icon = Rectangle::new(
        (
            badge.loc.x + (badge.size.w - SELECTED_CHECK_SIZE) / 2,
            badge.loc.y + (badge.size.h - SELECTED_CHECK_SIZE) / 2,
        )
            .into(),
        (SELECTED_CHECK_SIZE, SELECTED_CHECK_SIZE).into(),
    );
    if let Some(icon) =
        node_renderer.selection_check_element(renderer, icon, visuals.text.bytes(), alpha)
    {
        elements.push(SceneElement::NodeTexture(icon));
    }
    elements.push(SceneElement::NodeLabel(
        crate::render::overlays::shell::label_card_element(
            renderer,
            node_renderer,
            badge,
            crate::render::overlays::shell::OverlayVisuals {
                radius: badge.size.h as f32 * 0.5,
                ..visuals
            },
            visuals.fill,
            0.98 * alpha,
        )?,
    ));
    Ok(())
}

fn push_draft_rail(
    ctx: &mut DrawContext<'_>,
    elements: &mut Vec<SceneElement>,
    screen: Rectangle<i32, Physical>,
    creation: Option<&crate::clusters::CreationState>,
    visuals: crate::render::overlays::shell::OverlayVisuals,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    if alpha <= 0.0 {
        return Ok(());
    }
    // The naming dialog already carries its own title and controls. Keeping a
    // second instruction rail behind it only adds visual noise.
    if creation.is_some_and(|creation| creation.naming) {
        return Ok(());
    }
    let count = creation.map_or(0, |creation| creation.selected.len());
    let title = "Build a cluster";
    let status = format!("{count} selected");
    let status_text = if count == 0 {
        visuals.text
    } else {
        visuals.fill
    };
    let mut hints = "Space select  ·  Enter name  ·  Esc cancel";
    let title_size = ctx
        .text
        .measure(ctx.renderer, title, visuals.text.bytes())?
        .unwrap_or_default();
    let status_size = ctx
        .text
        .measure(ctx.renderer, &status, status_text.bytes())?
        .unwrap_or_default();
    let max_rail_width = (screen.size.w - RAIL_SCREEN_MARGIN * 2).max(0);
    let max_content_width = (max_rail_width - RAIL_PAD_X * 2).max(0);
    let mut hint_size = ctx
        .text
        .measure(ctx.renderer, hints, visuals.subtext.bytes())?
        .unwrap_or_default();
    if hint_size.w + RAIL_TEXT_GUARD > max_content_width {
        hints = "Space  ·  Enter  ·  Esc";
        hint_size = ctx
            .text
            .measure(ctx.renderer, hints, visuals.subtext.bytes())?
            .unwrap_or_default();
    }
    let status_width = status_size.w + STATUS_PAD_X * 2;
    let header_width = title_size.w + RAIL_GAP + status_width;
    let width =
        (header_width.max(hint_size.w + RAIL_TEXT_GUARD) + RAIL_PAD_X * 2).min(max_rail_width);
    let status_height = status_size.h + STATUS_PAD_Y * 2;
    let header_height = title_size.h.max(status_height);
    let height = header_height + hint_size.h + RAIL_PAD_Y * 2 + RAIL_GAP;
    let rail = Rectangle::new(
        ((screen.size.w - width) / 2, RAIL_TOP).into(),
        (width, height).into(),
    );
    let header_y = rail.loc.y + RAIL_PAD_Y;
    if let Some(text) = ctx.text.element(
        ctx.renderer,
        (
            rail.loc.x + RAIL_PAD_X,
            header_y + (header_height - title_size.h) / 2,
        )
            .into(),
        title,
        visuals.text.bytes(),
        alpha,
    )? {
        elements.push(SceneElement::UiText(text.element));
    }
    let status_badge = Rectangle::new(
        (
            rail.loc.x + RAIL_PAD_X + title_size.w + RAIL_GAP,
            header_y + (header_height - status_height) / 2,
        )
            .into(),
        (status_width, status_height).into(),
    );
    if let Some(text) = ctx.text.element(
        ctx.renderer,
        (
            status_badge.loc.x + STATUS_PAD_X,
            status_badge.loc.y + STATUS_PAD_Y,
        )
            .into(),
        &status,
        status_text.bytes(),
        alpha,
    )? {
        elements.push(SceneElement::UiText(text.element));
    }
    elements.push(SceneElement::NodeLabel(ctx.label_card(
        status_badge,
        crate::render::overlays::shell::OverlayVisuals {
            radius: status_badge.size.h as f32 * 0.5,
            ..visuals
        },
        if count == 0 {
            visuals.key_fill
        } else {
            visuals.text
        },
        0.92 * alpha,
    )?));
    if let Some(text) = ctx.text.element(
        ctx.renderer,
        (rail.loc.x + RAIL_PAD_X, header_y + header_height + RAIL_GAP).into(),
        hints,
        visuals.subtext.bytes(),
        0.88 * alpha,
    )? {
        elements.push(SceneElement::UiText(text.element));
    }
    elements.push(SceneElement::NodeLabel(ctx.card(
        rail,
        crate::render::overlays::shell::OverlayVisuals {
            radius: 18.0,
            ..visuals
        },
        visuals.fill,
        0.97 * alpha,
    )?));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn core_destination_is_local_to_the_initiating_output() {
        let output = Rectangle::<i32, Logical>::new((2560, 180).into(), (1920, 1080).into());
        let destination = core_rect_for_output((3000, 600).into(), output);
        let side = crate::clusters::CORE_DIAMETER_PX.round() as i32;
        assert_eq!(destination.loc, (440 - side / 2, 420 - side / 2).into());
        assert_eq!(destination.size, (side, side).into());
    }

    #[test]
    fn selection_badge_tracks_the_animated_card() {
        let first = selection_badge_rect(rect(10, 20, 300, 200));
        let moved = selection_badge_rect(rect(90, 75, 120, 80));
        assert_eq!(moved.loc.x - first.loc.x, -100);
        assert_eq!(moved.loc.y - first.loc.y, 55);
        assert_eq!(moved.size, first.size);
    }
}
