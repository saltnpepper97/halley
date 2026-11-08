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

#[allow(clippy::too_many_arguments)]
pub(super) fn cluster_composer_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    state: &crate::shell::cluster_composer::ClusterComposerState,
    clusters: &crate::clusters::ClusterSystem,
    config: halley_config::Apogee,
    overlay_config: &halley_config::Overlays,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    space: &smithay::desktop::Space<smithay::desktop::Window>,
    cameras: &crate::presentation::camera::OutputCameras,
    nodes: &crate::nodes::NodesState,
    node_renderer: &mut crate::render::node::NodeRenderer,
    cluster_renderer: &mut crate::clusters::render::ClusterRenderer,
    titlebar_renderer: &mut crate::render::titlebar::TitlebarRenderer,
    window_decoration_renderer: &mut crate::render::window_decoration::WindowDecorationRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    window_animations: &crate::animation::WindowAnimations,
    fullscreen: &crate::wayland::fullscreen::FullscreenManager,
    maximize: &crate::presentation::maximize::FieldMaximizeManager,
    overlay_previews: &mut crate::render::overlays::preview::OverlayPreviewCache,
    now: std::time::Duration,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let Some(session) = state
        .session()
        .filter(|session| session.output == output.name() && session.replaces_scene())
    else {
        return Ok(Vec::new());
    };
    let creation = clusters.creation();
    let progress = session.progress(now).clamp(0.0, 1.0);
    let ordinary_transition = super::overview::apogee_transition_visuals(progress);
    let commit_progress = session.commit_progress(now);
    let committing = matches!(
        session.phase(),
        crate::shell::cluster_composer::Phase::Committing
            | crate::shell::cluster_composer::Phase::CommitEndpointHeld
    );
    let overlay_visuals = crate::render::overlays::shell::resolve_visuals(overlay_config);
    let output_local = Rectangle::<i32, Physical>::from_size(output_geometry.size.to_physical(1));
    let prepared_core = session
        .prepared()
        .and_then(|prepared| prepared_core_rect(prepared, output, output_geometry, cameras));
    let selected_count = session
        .prepared()
        .map_or(0, |prepared| prepared.members.len());
    let mut tiles = session.tiles.iter().collect::<Vec<_>>();
    tiles.sort_by_key(|tile| {
        (
            usize::from(session.focused == Some(tile.id)),
            tile.source_stack_index,
            tile.source_stack_order,
        )
    });
    tiles.reverse();

    overlay_previews.retain(session.tiles.iter().map(|tile| tile.id));
    let mut elements = Vec::new();
    for tile in tiles {
        let focused = session.focused == Some(tile.id);
        let selected = creation.is_some_and(|creation| creation.selected.contains(&tile.id));
        let source = super::overview::overview_window_source_rect(
            output,
            output_geometry,
            tile.id,
            decorations,
            font,
            space,
            cameras,
            nodes,
            window_animations,
            fullscreen,
            maximize,
            now,
        );
        let mosaic = Rectangle::<i32, Physical>::new(
            (tile.target.loc - output_geometry.loc).to_physical(1),
            tile.target.size.to_physical(1),
        );
        let body = if committing {
            let start = source.map(|source| {
                super::overview::lerp_rect(source, mosaic, session.commit_opening_progress())
            });
            let end = if selected { prepared_core } else { source };
            start.zip(end).map(|(start, end)| {
                let index = session
                    .prepared()
                    .and_then(|prepared| {
                        prepared
                            .members
                            .iter()
                            .position(|member| *member == tile.id)
                    })
                    .unwrap_or(0);
                commit_body(start, end, commit_progress, index, selected_count, selected)
            })
        } else {
            None
        };
        let mut tile_transition = if committing {
            let chrome = (1.0 - commit_progress / 0.62).clamp(0.0, 1.0);
            let mut transition = super::overview::apogee_transition_visuals(1.0);
            transition.chrome_alpha = chrome;
            transition.overlay_alpha = chrome;
            if selected {
                transition.preview_alpha = (1.0 - (commit_progress - 0.70) / 0.30).clamp(0.0, 1.0);
            }
            transition
        } else {
            ordinary_transition
        };
        if body.is_none() && committing {
            // A disappearing selected member invalidates the prepared commit;
            // the lifecycle abort path will return to the composer.
            tile_transition.preview_alpha = 0.0;
        }
        if selected && tile_transition.chrome_alpha > 0.0 {
            let badge_body = body
                .or_else(|| {
                    source.map(|source| super::overview::lerp_rect(source, mosaic, progress))
                })
                .unwrap_or(mosaic);
            push_selection_badge(
                renderer,
                node_renderer,
                &mut elements,
                badge_body,
                overlay_visuals,
                tile_transition.chrome_alpha,
            )?;
        }
        super::overview::push_overview_window(
            &mut elements,
            renderer,
            output,
            output_geometry,
            tile.id,
            tile.target,
            body,
            progress,
            focused && !committing,
            selected && !committing,
            session.hovered == Some(tile.id) && !committing,
            config,
            overlay_visuals,
            tile_transition,
            decorations,
            font,
            space,
            cameras,
            nodes,
            node_renderer,
            titlebar_renderer,
            window_decoration_renderer,
            ui_text,
            window_animations,
            fullscreen,
            maximize,
            overlay_previews,
            now,
        )?;
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
        elements.splice(0..0, core_elements);
    }

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
            ordinary_transition.chrome_alpha,
        )?;
    }

    let backdrop_progress = if committing {
        1.0
    } else {
        ordinary_transition.overlay_alpha
    };
    elements.push(SceneElement::Border(crate::render::solid_color_element(
        node_renderer.active_slot_id(crate::render::node::NodeSlot::ClusterComposerBackdrop),
        output_local,
        super::overview::apogee_backdrop_color(config.background_dim, backdrop_progress),
    )));
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

fn commit_body(
    start: Rectangle<i32, Physical>,
    end: Rectangle<i32, Physical>,
    global: f32,
    index: usize,
    selected_count: usize,
    selected: bool,
) -> Rectangle<i32, Physical> {
    let progress = if selected {
        commit_tile_progress(global, index, selected_count)
    } else {
        global.clamp(0.0, 1.0)
    };
    super::overview::lerp_rect(start, end, progress)
}

pub(super) fn commit_tile_progress(global: f32, index: usize, count: usize) -> f32 {
    if count <= 1 {
        return global.clamp(0.0, 1.0);
    }
    let delay = 0.16 * index.min(count - 1) as f32 / (count - 1) as f32;
    ((global - delay) / (1.0 - delay)).clamp(0.0, 1.0)
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
    fn selected_and_unselected_commit_bodies_reach_their_distinct_endpoints() {
        let start = rect(100, 120, 500, 320);
        let core = rect(900, 600, 68, 68);
        let field = rect(40, 50, 700, 420);

        assert_eq!(commit_body(start, core, 0.0, 0, 3, true), start);
        assert_eq!(commit_body(start, core, 1.0, 2, 3, true), core);
        assert_eq!(commit_body(start, field, 0.0, 0, 3, false), start);
        assert_eq!(commit_body(start, field, 1.0, 0, 3, false), field);
    }

    #[test]
    fn stable_stagger_always_finishes_at_the_global_endpoint() {
        for count in 1..=32 {
            for index in 0..count {
                assert_eq!(commit_tile_progress(1.0, index, count), 1.0);
            }
        }
        assert!(commit_tile_progress(0.2, 0, 4) > commit_tile_progress(0.2, 3, 4));
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
