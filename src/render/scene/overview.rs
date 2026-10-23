use super::apogee_clusters::{ApogeeCoreTileContext, apogee_core_tile_elements};
use super::nodes::{ease_in_out_cubic, fit_ui_text};
use super::*;

pub(super) const CLUSTER_MEMBER_BORDER_PX: f32 = 7.0;

pub(super) fn apogee_backdrop_color(
    dim: f32,
    progress: f32,
) -> smithay::backend::renderer::Color32F {
    // Smithay expects premultiplied RGBA. Neutral black only attenuates the
    // wallpaper, and contributes no color even at zero transition opacity.
    smithay::backend::renderer::Color32F::BLACK * (dim * progress)
}

pub(super) fn preview_content_radius(overlay_radius: f32) -> f32 {
    overlay_radius.max(0.0)
}

fn push_preview_texture(
    elements: &mut Vec<SceneElement>,
    renderer: &mut GlesRenderer,
    window_decoration_renderer: &mut crate::render::window_decoration::WindowDecorationRenderer,
    preview: smithay::backend::renderer::element::texture::TextureRenderElement<
        smithay::backend::renderer::gles::GlesTexture,
    >,
    texture: smithay::backend::renderer::gles::GlesTexture,
    destination: Rectangle<i32, Physical>,
    overlay_radius: f32,
) {
    let radius = preview_content_radius(overlay_radius);
    if radius > 0.0 && window_decoration_renderer.available(renderer) {
        let preview = window_decoration_renderer
            .texture_element(renderer, preview, texture, destination, radius)
            .expect("rounded resources were checked above");
        elements.push(SceneElement::RoundedTexture(preview));
    } else {
        elements.push(SceneElement::Closing(preview));
    }
}

/// Render a mapped preview from the client's surface tree, the same approach
/// Niri uses for its MRU switcher. This avoids two full-window offscreen passes
/// for every live update and preserves Smithay's real surface damage instead
/// of trying to infer damage after mutating a cached texture.
#[allow(clippy::too_many_arguments)]
fn push_live_preview(
    elements: &mut Vec<SceneElement>,
    renderer: &mut GlesRenderer,
    window: &smithay::desktop::Window,
    destination: Rectangle<i32, Physical>,
    alpha: f32,
    overlay_radius: f32,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    chrome_visible: bool,
    maximized: bool,
    titlebar_renderer: &mut crate::render::titlebar::TitlebarRenderer,
    window_decoration_renderer: &mut crate::render::window_decoration::WindowDecorationRenderer,
    node_renderer: &mut crate::render::node::NodeRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
) -> Result<bool, Box<dyn Error>> {
    let Some(surface) = window.wl_surface() else {
        return Ok(false);
    };
    let geometry = window.geometry();
    if geometry.size.w <= 0 || geometry.size.h <= 0 {
        return Ok(false);
    }

    let native_client = Rectangle::<i32, Physical>::from_size(geometry.size.to_physical(1));
    let chrome = crate::titlebar::WindowChrome::for_visible_presentation(
        window,
        chrome_visible,
        decorations,
        font,
    );
    let native_outer = if chrome_visible {
        chrome
            .outer_rect(Rectangle::<i32, Logical>::from_size(geometry.size))
            .to_physical(1)
    } else {
        native_client
    };
    let content = crate::animation::map_rect(native_client, native_outer, destination);
    if content.size.w <= 0 || content.size.h <= 0 {
        return Ok(false);
    }

    let location = smithay::utils::Point::from((-geometry.loc.x, -geometry.loc.y)).to_physical(1);
    let surface_elements: Vec<
        smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>,
    > = smithay::backend::renderer::element::surface::render_elements_from_surface_tree(
        renderer,
        surface.as_ref(),
        location,
        1.0,
        alpha,
        Kind::Unspecified,
    );
    if surface_elements.is_empty() {
        return Ok(false);
    }

    let server_titlebar = chrome_visible && chrome.has_server_titlebar();
    let scale = content.size.h as f32 / native_client.size.h.max(1) as f32;
    // The preview is framed by overlay chrome, so its mask must use the
    // overlay's radius in output pixels. Scaling the window's native radius
    // down with the thumbnail made the texture visibly squarer than its card.
    let content_radius = preview_content_radius(overlay_radius);
    let rounded = content_radius > 0.0 && window_decoration_renderer.available(renderer);

    if server_titlebar {
        let metrics = crate::titlebar::rendered_metrics(&decorations.titlebars, font.size, scale);
        append_titlebar_elements(
            renderer,
            window,
            Some("live-preview"),
            content,
            metrics.height,
            crate::titlebar::glyph_size(metrics.height),
            scale,
            maximized,
            crate::render::window_decoration::scaled_metric(chrome.border_width, scale),
            content_radius,
            false,
            alpha,
            decorations,
            None,
            None,
            false,
            titlebar_renderer,
            window_decoration_renderer,
            node_renderer,
            ui_text,
            elements,
        )?;
    }

    for surface_element in surface_elements {
        let native_geometry = surface_element.geometry(Scale::from(1.0));
        let target = crate::animation::map_rect(native_geometry, native_client, content);
        if rounded {
            let radii = if server_titlebar {
                crate::render::window_decoration::CornerRadii::bottom(content_radius)
            } else {
                crate::render::window_decoration::CornerRadii::all(content_radius)
            };
            let rounded = window_decoration_renderer
                .surface_element_with_radii(renderer, surface_element, target, content, radii)
                .expect("rounded resources were checked above");
            if let Some(cropped) = CropRenderElement::from_element(rounded, 1.0, content) {
                elements.push(SceneElement::RoundedCropped(cropped));
            }
        } else {
            let rescaled = crate::render::rescale::RescaledElement::new(surface_element, target);
            if let Some(cropped) = CropRenderElement::from_element(rescaled, 1.0, content) {
                elements.push(SceneElement::Cropped(cropped));
            }
        }
    }

    Ok(true)
}

const APOGEE_SELECTION_BORDER_PX: f32 = 4.0;
const APOGEE_SELECTION_ACCENT_MIX: f32 = 0.55;

fn apogee_window_chrome(
    mut visuals: crate::render::overlays::shell::OverlayVisuals,
    selected: bool,
    hovered: bool,
) -> (
    crate::render::overlays::shell::OverlayVisuals,
    crate::render::overlays::shell::OverlayRgb,
    crate::render::overlays::shell::OverlayRgb,
) {
    // Keyboard navigation clears `hovered` while retaining `selected`. Restore
    // old Halley's strong accent band and add an always-visible focus frame so
    // that selection remains obvious even when decorative overlay borders are
    // disabled.
    if selected {
        visuals.border_px = visuals.border_px.max(APOGEE_SELECTION_BORDER_PX);
    }
    let caption_fill = if selected {
        visuals
            .fill
            .mix(visuals.border, APOGEE_SELECTION_ACCENT_MIX)
    } else if hovered {
        visuals.fill.mix(visuals.border, 0.32)
    } else {
        visuals.fill
    };
    let card_fill = if selected || hovered {
        visuals.fill.mix(visuals.border, 0.12)
    } else {
        visuals.fill
    };
    (visuals, caption_fill, card_fill)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apogee_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    state: &crate::shell::apogee::ApogeeState,
    config: halley_config::Apogee,
    overlay_config: &halley_config::Overlays,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    space: &smithay::desktop::Space<smithay::desktop::Window>,
    cameras: &crate::presentation::camera::OutputCameras,
    nodes: &crate::nodes::NodesState,
    clusters: &crate::clusters::ClusterSystem,
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
    let Some(session) = state.session() else {
        return Ok(Vec::new());
    };
    let progress = session.progress(now).clamp(0.0, 1.0);
    let visuals = apogee_transition_visuals(progress);
    let output_local = Rectangle::<i32, Physical>::from_size(output_geometry.size.to_physical(1));
    let overlay_visuals = crate::render::overlays::shell::resolve_visuals(overlay_config);
    let mut tiles = session
        .tiles
        .iter()
        .filter(|tile| tile.output == output.name())
        .collect::<Vec<_>>();
    sort_apogee_tiles(&mut tiles, session.selected);

    let mut elements = Vec::new();
    overlay_previews.retain(
        session
            .tiles
            .iter()
            .filter(|tile| tile.kind == crate::shell::apogee::TileKind::Window)
            .map(|tile| tile.id),
    );
    for tile in tiles {
        if let crate::shell::apogee::TileKind::ClusterCore(cluster) = tile.kind {
            elements.extend(apogee_core_tile_elements(
                renderer,
                ApogeeCoreTileContext {
                    output_geometry,
                    tile,
                    cluster,
                    progress,
                    chrome_alpha: visuals.chrome_alpha,
                    highlighted: session.selected == Some(tile.id)
                        || session.hovered == Some(tile.id),
                    overlay_visuals,
                    cameras,
                    nodes,
                    clusters,
                    node_renderer,
                    cluster_renderer,
                    ui_text,
                    now,
                },
            )?);
            continue;
        }
        push_overview_window(
            &mut elements,
            renderer,
            output,
            output_geometry,
            tile.id,
            tile.target,
            None,
            progress,
            session.selected == Some(tile.id),
            false,
            session.hovered == Some(tile.id),
            config,
            overlay_visuals,
            visuals,
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
    let backdrop_color = apogee_backdrop_color(config.background_dim, visuals.overlay_alpha);
    elements.push(SceneElement::Border(crate::render::solid_color_element(
        node_renderer.active_slot_id(crate::render::node::NodeSlot::ApogeeBackdrop),
        output_local,
        backdrop_color,
    )));
    Ok(elements)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn overview_window_source_rect(
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    id: halley_core::field::NodeId,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    space: &smithay::desktop::Space<smithay::desktop::Window>,
    cameras: &crate::presentation::camera::OutputCameras,
    nodes: &crate::nodes::NodesState,
    window_animations: &crate::animation::WindowAnimations,
    fullscreen: &crate::wayland::fullscreen::FullscreenManager,
    maximize: &crate::presentation::maximize::FieldMaximizeManager,
    now: std::time::Duration,
) -> Option<Rectangle<i32, Physical>> {
    let record = nodes.record(id)?;
    if record.collapsed {
        let camera = cameras.get(&output.name())?;
        let node = nodes.field.node(id)?;
        let center = crate::nodes::screen_from_world(node.pos, camera, output_geometry)
            - output_geometry.loc;
        let side = crate::nodes::NODE_DIAMETER_PX.round() as i32;
        Some(Rectangle::new(
            (center.x - side / 2, center.y - side / 2).into(),
            (side, side).into(),
        ))
    } else {
        let chrome_visible = preview_chrome_visible(&record.window, fullscreen);
        Some(
            window_visual_state(
                space,
                cameras,
                None,
                None,
                &record.window,
                output,
                window_animations,
                fullscreen,
                maximize,
                decorations,
                font,
                now,
            )
            .map(|visual| {
                preview_visual_outer_rect(&record.window, visual, decorations, font, chrome_visible)
            })
            .unwrap_or_else(|| {
                preview_outer_rect(
                    &record.window,
                    record.geometry.to_physical(1),
                    decorations,
                    font,
                    chrome_visible,
                )
            }),
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_overview_window(
    elements: &mut Vec<SceneElement>,
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    id: halley_core::field::NodeId,
    target_global: Rectangle<i32, Logical>,
    body_override: Option<Rectangle<i32, Physical>>,
    progress: f32,
    focused: bool,
    member_selected: bool,
    hovered: bool,
    config: halley_config::Apogee,
    overlay_visuals: crate::render::overlays::shell::OverlayVisuals,
    visuals: ApogeeTransitionVisuals,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    space: &smithay::desktop::Space<smithay::desktop::Window>,
    cameras: &crate::presentation::camera::OutputCameras,
    nodes: &crate::nodes::NodesState,
    node_renderer: &mut crate::render::node::NodeRenderer,
    titlebar_renderer: &mut crate::render::titlebar::TitlebarRenderer,
    window_decoration_renderer: &mut crate::render::window_decoration::WindowDecorationRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    window_animations: &crate::animation::WindowAnimations,
    fullscreen: &crate::wayland::fullscreen::FullscreenManager,
    maximize: &crate::presentation::maximize::FieldMaximizeManager,
    overlay_previews: &mut crate::render::overlays::preview::OverlayPreviewCache,
    now: std::time::Duration,
) -> Result<(), Box<dyn Error>> {
    let Some(record) = nodes.record(id) else {
        return Ok(());
    };
    let target = Rectangle::<i32, Physical>::new(
        (target_global.loc - output_geometry.loc).to_physical(1),
        target_global.size.to_physical(1),
    );
    let Some(source) = overview_window_source_rect(
        output,
        output_geometry,
        id,
        decorations,
        font,
        space,
        cameras,
        nodes,
        window_animations,
        fullscreen,
        maximize,
        now,
    ) else {
        return Ok(());
    };
    let body = body_override.unwrap_or_else(|| lerp_rect(source, target, progress));
    let (mut card_visuals, mut caption_fill, mut card_fill) =
        apogee_window_chrome(overlay_visuals, focused, hovered);
    let mut caption_text = overlay_visuals.text;
    // Membership uses an inverted card treatment rather than another shade of
    // the focus accent. Both colors come from the resolved overlay palette, so
    // this remains high-contrast in light, dark, and explicitly themed modes.
    if member_selected {
        let selection_fill = overlay_visuals.text;
        card_visuals.border = selection_fill;
        card_visuals.border_px = card_visuals.border_px.max(CLUSTER_MEMBER_BORDER_PX);
        caption_fill = selection_fill;
        caption_text = overlay_visuals.fill;
        card_fill = card_fill.mix(selection_fill, 0.22);
    }
    let chrome_alpha = visuals.chrome_alpha;
    // Old Halley kept the caption inside the preview. Growing the card by
    // a fixed footer made its backing look like an enlarged second window,
    // especially for short and wide Apogee tiles.
    let card = preview_card_rect(body, card_visuals.border_px);
    let caption = apogee_caption_rect(body);
    if let Some(caption) = caption {
        let (title, size) = fit_ui_text(
            renderer,
            ui_text,
            &record.title,
            caption_text.bytes(),
            caption.size.w - 16,
        )?;
        if !title.is_empty()
            && let Some(text) = ui_text.element(
                renderer,
                (
                    caption.loc.x + (caption.size.w - size.w).max(0) / 2,
                    caption.loc.y + (caption.size.h - size.h).max(0) / 2,
                )
                    .into(),
                &title,
                caption_text.bytes(),
                chrome_alpha,
            )?
        {
            elements.push(SceneElement::UiText(text.element));
        }
        elements.push(SceneElement::NodeLabel(
            crate::render::overlays::shell::label_card_element(
                renderer,
                node_renderer,
                caption,
                overlay_visuals,
                caption_fill,
                if focused || hovered {
                    0.96 * chrome_alpha
                } else {
                    0.88 * chrome_alpha
                },
            )?,
        ));
    }
    if record.collapsed {
        let badge = "NODE";
        if let Some(size) = ui_text.measure(renderer, badge, [151, 205, 255])?
            && let Some(text) = ui_text.element(
                renderer,
                (card.loc.x + card.size.w - size.w - 10, card.loc.y + 8).into(),
                badge,
                [151, 205, 255],
                chrome_alpha,
            )?
        {
            elements.push(SceneElement::UiText(text.element));
        }
    }

    let chrome_visible = preview_chrome_visible(&record.window, fullscreen);
    let maximized = record
        .window
        .wl_surface()
        .is_some_and(|surface| maximize.contains(surface.as_ref()));
    let direct = config.live_previews
        && !record.collapsed
        && push_live_preview(
            elements,
            renderer,
            &record.window,
            body,
            visuals.preview_alpha,
            overlay_visuals.radius,
            decorations,
            font,
            chrome_visible,
            maximized,
            titlebar_renderer,
            window_decoration_renderer,
            node_renderer,
            ui_text,
        )?;
    if !direct {
        match overlay_previews.element_with_texture(
            renderer,
            crate::render::overlays::preview::OverlayPreviewRequest {
                id,
                window: &record.window,
                destination: body,
                alpha: visuals.preview_alpha,
                allow_refresh: !record.collapsed,
                live: false,
                decorations,
                font,
                chrome_visible,
                maximized,
            },
            crate::render::overlays::preview::OverlayPreviewRenderers {
                titlebar: titlebar_renderer,
                decoration: window_decoration_renderer,
                node: node_renderer,
                text: ui_text,
            },
        ) {
            Ok((preview, texture)) => push_preview_texture(
                elements,
                renderer,
                window_decoration_renderer,
                preview,
                texture,
                body,
                overlay_visuals.radius,
            ),
            Err(_) => {
                if let Some(app_id) = record.app_id.as_deref()
                    && let Some(icon) = node_renderer.app_icon_element(
                        renderer,
                        app_id,
                        body,
                        visuals.preview_alpha,
                    )
                {
                    elements.push(SceneElement::NodeTexture(icon));
                }
            }
        }
    }
    elements.push(SceneElement::NodeLabel(
        crate::render::overlays::shell::card_element(
            renderer,
            node_renderer,
            card,
            card_visuals,
            card_fill,
            0.96 * visuals.overlay_alpha,
        )?,
    ));
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ApogeeTransitionVisuals {
    pub(super) preview_alpha: f32,
    pub(super) overlay_alpha: f32,
    pub(super) chrome_alpha: f32,
}

pub(super) fn apogee_transition_visuals(progress: f32) -> ApogeeTransitionVisuals {
    let overlay_alpha = progress.clamp(0.0, 1.0);
    ApogeeTransitionVisuals {
        // The preview is the handoff surface: at progress zero it occupies the
        // live window's exact rect, so fading it would expose a wallpaper-only
        // frame on both entry and exit.
        preview_alpha: 1.0,
        overlay_alpha,
        chrome_alpha: ((overlay_alpha - 0.18) / 0.62).clamp(0.0, 1.0),
    }
}

pub(super) fn sort_apogee_tiles(
    tiles: &mut [&crate::shell::apogee::Tile],
    selected: Option<halley_core::field::NodeId>,
) {
    tiles.sort_by_key(|tile| {
        (
            usize::from(selected == Some(tile.id)),
            tile.source_stack_index,
            tile.source_stack_order,
        )
    });
    // Smithay render-element lists are front-to-back.
    tiles.reverse();
}

pub(super) fn lerp_rect(
    from: Rectangle<i32, Physical>,
    to: Rectangle<i32, Physical>,
    progress: f32,
) -> Rectangle<i32, Physical> {
    let lerp = |a: i32, b: i32| (a as f32 + (b - a) as f32 * progress).round() as i32;
    Rectangle::new(
        (lerp(from.loc.x, to.loc.x), lerp(from.loc.y, to.loc.y)).into(),
        (
            lerp(from.size.w, to.size.w).max(1),
            lerp(from.size.h, to.size.h).max(1),
        )
            .into(),
    )
}

pub(super) fn apogee_caption_rect(
    body: Rectangle<i32, Physical>,
) -> Option<Rectangle<i32, Physical>> {
    if body.size.w < 96 || body.size.h < 72 {
        return None;
    }
    let height = ((body.size.h as f32 * 0.13).round() as i32).clamp(22, 34);
    Some(Rectangle::new(
        (body.loc.x + 8, body.loc.y + body.size.h - height - 8).into(),
        ((body.size.w - 16).max(1), height).into(),
    ))
}

pub(super) struct FocusCycleRenderContext<'a> {
    pub state: &'a crate::shell::focus_cycle::FocusCycleState,
    pub nodes: &'a crate::nodes::NodesState,
    pub overlay_config: &'a halley_config::Overlays,
    pub decorations: &'a halley_config::Decorations,
    pub font: &'a halley_config::Font,
    pub fullscreen: &'a crate::wayland::fullscreen::FullscreenManager,
    pub maximize: &'a crate::presentation::maximize::FieldMaximizeManager,
    pub now: std::time::Duration,
}

pub(super) fn focus_cycle_elements(
    renderer: &mut GlesRenderer,
    output_geometry: Rectangle<i32, Logical>,
    context: FocusCycleRenderContext<'_>,
    overlay_previews: &mut crate::render::overlays::preview::OverlayPreviewCache,
    renderers: crate::render::overlays::preview::OverlayPreviewRenderers<'_>,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let crate::render::overlays::preview::OverlayPreviewRenderers {
        titlebar: titlebar_renderer,
        decoration: window_decoration_renderer,
        node: node_renderer,
        text: ui_text,
    } = renderers;
    let Some(session) = context.state.session() else {
        return Ok(Vec::new());
    };
    let open = session.open_progress(context.now);
    let close = session.close_progress(context.now);
    let alpha = (open * (1.0 - close)).clamp(0.0, 1.0);
    if alpha <= 0.001 {
        return Ok(Vec::new());
    }

    let screen = output_geometry.size.to_physical(1);
    let overlay_visuals = crate::render::overlays::shell::resolve_visuals(context.overlay_config);
    let rail_step = (screen.w as f32 * 0.28).clamp(260.0, 440.0) + 9.0;
    let center_y = screen.h as f32 * 0.5;
    let mut cards = session
        .visible_slots(crate::shell::focus_cycle::VISIBLE_RADIUS)
        .into_iter()
        .filter_map(|(_, id)| {
            let record = context.nodes.record(id)?;
            let index = session
                .candidates
                .iter()
                .position(|candidate| *candidate == id)?;
            let offset = session.visual_offset(index, context.now);
            let distance = offset.abs().min(2.0);
            let base_h = (screen.h as f32 * 0.46).clamp(240.0, 480.0);
            let scale = if distance <= 1.0 {
                1.0 + (0.82 - 1.0) * distance
            } else {
                0.82 + (0.64 - 0.82) * (distance - 1.0)
            };
            let preview_h = (base_h * scale).round().max(1.0) as i32;
            let aspect = (record.geometry.size.w.max(1) as f32
                / record.geometry.size.h.max(1) as f32)
                .clamp(0.7, 2.0);
            let preview_w = (preview_h as f32 * aspect).round().max(1.0) as i32;
            let cx = screen.w as f32 * 0.5 + offset * rail_step;
            let cy = center_y + distance * 22.0 + (1.0 - open) * 22.0 + close * 18.0;
            let pose_scale = 0.88 + 0.12 * open - 0.08 * close;
            let card_w = (preview_w as f32 * pose_scale).round().max(1.0) as i32;
            let card_h = (preview_h as f32 * pose_scale).round().max(1.0) as i32;
            Some((
                distance,
                id,
                Rectangle::<i32, Physical>::new(
                    (
                        (cx - card_w as f32 * 0.5).round() as i32,
                        (cy - card_h as f32 * 0.5).round() as i32,
                    )
                        .into(),
                    (card_w, card_h).into(),
                ),
            ))
        })
        .collect::<Vec<_>>();
    cards.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut elements = Vec::new();
    overlay_previews.retain(session.candidates.iter().copied());
    for (distance, id, card) in cards {
        let Some(record) = context.nodes.record(id) else {
            continue;
        };
        let selected = distance < 0.45;
        let distance_step = distance.round().clamp(0.0, 2.0) as i32;
        let pad = if distance_step >= 2 { 4 } else { 6 };
        let body_bounds = Rectangle::<i32, Physical>::new(
            (card.loc.x + pad, card.loc.y + pad).into(),
            (
                (card.size.w - pad * 2).max(1),
                (card.size.h - pad * 2).max(1),
            )
                .into(),
        );
        let chrome_visible = preview_chrome_visible(&record.window, context.fullscreen);
        let fallback_size = preview_outer_size(
            &record.window,
            record.geometry.size,
            context.decorations,
            context.font,
            chrome_visible,
        );
        let (source_width, source_height) = overlay_previews
            .source_dimensions(id)
            .unwrap_or((fallback_size.w, fallback_size.h));
        let body = aspect_fit_rect(body_bounds, source_width, source_height);
        let card = preview_card_rect(body, overlay_visuals.border_px);

        // Top-right monitor badge, over the preview.
        let monitor = truncate_chars(&record.output, 10);
        if let Some(size) = ui_text.measure(renderer, &monitor, overlay_visuals.text.bytes())? {
            let badge = Rectangle::<i32, Physical>::new(
                (body.loc.x + body.size.w - size.w - 22, body.loc.y + 8).into(),
                (size.w + 14, size.h + 8).into(),
            );
            if let Some(text) = ui_text.element(
                renderer,
                (badge.loc.x + 7, badge.loc.y + 4).into(),
                &monitor,
                overlay_visuals.text.bytes(),
                alpha,
            )? {
                elements.push(SceneElement::UiText(text.element));
            }
            elements.push(SceneElement::NodeLabel(
                crate::render::overlays::shell::label_card_element(
                    renderer,
                    node_renderer,
                    badge,
                    overlay_visuals,
                    overlay_visuals.fill.mix(overlay_visuals.border, 0.12),
                    if selected { 0.95 * alpha } else { 0.78 * alpha },
                )?,
            ));
        }

        if record.collapsed
            && let Some(size) = ui_text.measure(renderer, "NODE", overlay_visuals.border.bytes())?
        {
            let badge = Rectangle::<i32, Physical>::new(
                (body.loc.x + 8, body.loc.y + 8).into(),
                (size.w + 14, size.h + 8).into(),
            );
            if let Some(text) = ui_text.element(
                renderer,
                (badge.loc.x + 7, badge.loc.y + 4).into(),
                "NODE",
                overlay_visuals.border.bytes(),
                alpha,
            )? {
                elements.push(SceneElement::UiText(text.element));
            }
            elements.push(SceneElement::NodeLabel(
                crate::render::overlays::shell::label_card_element(
                    renderer,
                    node_renderer,
                    badge,
                    overlay_visuals,
                    overlay_visuals.fill,
                    0.94 * alpha,
                )?,
            ));
        }

        // Old Halley put the title and app icon in a caption band over the
        // bottom of the thumbnail instead of growing a giant footer.
        let title = truncate_chars(
            &record.title,
            match distance_step {
                0 => 42,
                1 => 30,
                _ => 20,
            },
        );
        let text_size = ui_text
            .measure(renderer, &title, overlay_visuals.text.bytes())?
            .unwrap_or_default();
        let band_margin = 6;
        let band_h = (text_size.h + 10)
            .max(24)
            .min((body.size.h - band_margin * 2).max(1));
        let band = Rectangle::<i32, Physical>::new(
            (
                body.loc.x + band_margin,
                body.loc.y + body.size.h - band_h - band_margin,
            )
                .into(),
            ((body.size.w - band_margin * 2).max(1), band_h).into(),
        );
        let mut text_x = band.loc.x + 10;
        if distance_step < 2 {
            let icon_size = (band_h - 6).clamp(14, 30);
            let icon_rect = Rectangle::<i32, Physical>::new(
                (band.loc.x + 5, band.loc.y + (band_h - icon_size) / 2).into(),
                (icon_size, icon_size).into(),
            );
            if let Some(app_id) = record.app_id.as_deref()
                && let Some(icon) =
                    node_renderer.app_icon_element(renderer, app_id, icon_rect, alpha)
            {
                elements.push(SceneElement::NodeTexture(icon));
                text_x = icon_rect.loc.x + icon_rect.size.w + 8;
            }
        }
        if let Some(text) = ui_text.element(
            renderer,
            (text_x, band.loc.y + (band_h - text_size.h) / 2).into(),
            &title,
            overlay_visuals.text.bytes(),
            alpha,
        )? {
            elements.push(SceneElement::UiText(text.element));
        }
        elements.push(SceneElement::NodeLabel(
            crate::render::overlays::shell::label_card_element(
                renderer,
                node_renderer,
                band,
                overlay_visuals,
                if selected {
                    overlay_visuals.fill.mix(overlay_visuals.border, 0.18)
                } else {
                    overlay_visuals.fill
                },
                if selected { 0.96 * alpha } else { 0.88 * alpha },
            )?,
        ));

        let maximized = record
            .window
            .wl_surface()
            .is_some_and(|surface| context.maximize.contains(surface.as_ref()));
        let direct = !record.collapsed
            && push_live_preview(
                &mut elements,
                renderer,
                &record.window,
                body,
                alpha,
                overlay_visuals.radius,
                context.decorations,
                context.font,
                chrome_visible,
                maximized,
                titlebar_renderer,
                window_decoration_renderer,
                node_renderer,
                ui_text,
            )?;
        if !direct {
            match overlay_previews.element_with_texture(
                renderer,
                crate::render::overlays::preview::OverlayPreviewRequest {
                    id,
                    window: &record.window,
                    destination: body,
                    alpha,
                    allow_refresh: !record.collapsed,
                    live: false,
                    decorations: context.decorations,
                    font: context.font,
                    chrome_visible,
                    maximized,
                },
                crate::render::overlays::preview::OverlayPreviewRenderers {
                    titlebar: titlebar_renderer,
                    decoration: window_decoration_renderer,
                    node: node_renderer,
                    text: ui_text,
                },
            ) {
                Ok((preview, texture)) => push_preview_texture(
                    &mut elements,
                    renderer,
                    window_decoration_renderer,
                    preview,
                    texture,
                    body,
                    overlay_visuals.radius,
                ),
                Err(_) => {
                    if let Some(app_id) = record.app_id.as_deref()
                        && let Some(icon) =
                            node_renderer.app_icon_element(renderer, app_id, body, alpha)
                    {
                        elements.push(SceneElement::NodeTexture(icon));
                    }
                }
            }
        }
        elements.push(SceneElement::NodeLabel(
            crate::render::overlays::shell::card_element(
                renderer,
                node_renderer,
                card,
                overlay_visuals,
                if selected {
                    overlay_visuals.fill.mix(overlay_visuals.border, 0.12)
                } else {
                    overlay_visuals.fill
                },
                if selected { 0.99 * alpha } else { 0.82 * alpha },
            )?,
        ));
    }

    let hints = "Tab  next     Shift+Tab  previous     Esc  cancel";
    if let Some(size) = ui_text.measure(renderer, hints, overlay_visuals.subtext.bytes())?
        && let Some(text) = ui_text.element(
            renderer,
            ((screen.w - size.w) / 2, screen.h - size.h - 28).into(),
            hints,
            overlay_visuals.subtext.bytes(),
            alpha,
        )?
    {
        elements.push(SceneElement::UiText(text.element));
    }
    let backdrop_color = smithay::backend::renderer::Color32F::new(0.02, 0.03, 0.05, 0.55 * alpha);
    elements.push(SceneElement::Border(crate::render::solid_color_element(
        node_renderer.active_slot_id(crate::render::node::NodeSlot::FocusCycleBackdrop),
        Rectangle::from_size(screen),
        backdrop_color,
    )));
    Ok(elements)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn hover_preview_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    nodes: &crate::nodes::NodesState,
    cameras: &crate::presentation::camera::OutputCameras,
    overlay_previews: &mut crate::render::overlays::preview::OverlayPreviewCache,
    node_renderer: &mut crate::render::node::NodeRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    window_decoration_renderer: &mut crate::render::window_decoration::WindowDecorationRenderer,
    titlebar_renderer: &mut crate::render::titlebar::TitlebarRenderer,
    overlay_config: &halley_config::Overlays,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    fullscreen: &crate::wayland::fullscreen::FullscreenManager,
    now: std::time::Duration,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let Some((id, raw_mix)) = nodes.preview_hover_mix(&output.name(), now) else {
        return Ok(Vec::new());
    };
    let Some(record) = nodes
        .record(id)
        .filter(|record| record.collapsed && record.attached && record.output == output.name())
    else {
        return Ok(Vec::new());
    };
    let Some(node) = nodes.field.node(id) else {
        return Ok(Vec::new());
    };
    let Some(camera) = cameras.get(&output.name()) else {
        return Ok(Vec::new());
    };

    let preview_mix = ease_in_out_cubic(raw_mix.clamp(0.0, 1.0));
    let alpha = (preview_mix * preview_mix).clamp(0.0, 1.0);
    if alpha <= 0.01 {
        return Ok(Vec::new());
    }

    let screen = output_geometry.size.to_physical(1);
    let preview_size_base = ((screen.w.min(screen.h) as f32) * 0.30)
        .round()
        .clamp(220.0, 360.0) as i32;
    let chrome_visible = preview_chrome_visible(&record.window, fullscreen);
    let source = preview_outer_size(
        &record.window,
        record.geometry.size,
        decorations,
        font,
        chrome_visible,
    )
    .to_physical(1);
    let source_side = source.w.max(source.h).max(1);
    let base_side = (source_side + 24).clamp(120, preview_size_base);
    let preview_size = ((base_side as f32) * (0.94 + 0.06 * preview_mix))
        .round()
        .max(120.0) as i32;

    let landmark = nodes.landmark_position(id, node.pos, now);
    let center =
        crate::nodes::screen_from_world(landmark, camera, output_geometry) - output_geometry.loc;
    let card = Rectangle::<i32, Physical>::new(
        (
            (center.x - preview_size / 2).clamp(10, (screen.w - preview_size - 10).max(10)),
            (center.y - preview_size / 2).clamp(10, (screen.h - preview_size - 10).max(10)),
        )
            .into(),
        (preview_size, preview_size).into(),
    );
    let pad = ((preview_size as f32) * 0.045).round() as i32;
    let label_h = 30.min((preview_size / 5).max(22));
    let body_bounds = Rectangle::<i32, Physical>::new(
        (card.loc.x + pad, card.loc.y + pad).into(),
        (
            (card.size.w - pad * 2).max(1),
            (card.size.h - pad * 2 - label_h).max(1),
        )
            .into(),
    );
    let body = aspect_fit_rect(body_bounds, source.w, source.h);
    let visuals = crate::render::overlays::shell::resolve_visuals(overlay_config);
    let mut elements = Vec::new();

    let title = truncate_chars(&record.title, 24);
    if !title.is_empty()
        && let Some(text_size) = ui_text.measure(renderer, &title, visuals.text.bytes())?
        && let Some(text) = ui_text.element(
            renderer,
            (
                card.loc.x + (card.size.w - text_size.w).max(0) / 2,
                card.loc.y + card.size.h - pad - label_h + (label_h - text_size.h).max(0) / 2,
            )
                .into(),
            &title,
            visuals.text.bytes(),
            0.94 * alpha,
        )?
    {
        elements.push(SceneElement::UiText(text.element));
    }

    match overlay_previews.element_with_texture(
        renderer,
        crate::render::overlays::preview::OverlayPreviewRequest {
            id,
            window: &record.window,
            destination: body,
            alpha,
            allow_refresh: false,
            live: false,
            decorations,
            font,
            chrome_visible,
            maximized: false,
        },
        crate::render::overlays::preview::OverlayPreviewRenderers {
            titlebar: titlebar_renderer,
            decoration: window_decoration_renderer,
            node: node_renderer,
            text: ui_text,
        },
    ) {
        Ok((preview, texture)) => push_preview_texture(
            &mut elements,
            renderer,
            window_decoration_renderer,
            preview,
            texture,
            body,
            visuals.radius,
        ),
        Err(_) => elements.push(SceneElement::Border(crate::render::solid_color_element(
            node_renderer.active_slot_id(crate::render::node::NodeSlot::HoverPreviewFallback),
            body,
            smithay::backend::renderer::Color32F::new(0.02, 0.03, 0.05, 0.76 * alpha),
        ))),
    }

    elements.push(SceneElement::NodeLabel(
        crate::render::overlays::shell::card_element(
            renderer,
            node_renderer,
            card,
            visuals,
            visuals.fill,
            0.86 * alpha,
        )?,
    ));
    Ok(elements)
}

fn preview_chrome_visible(
    window: &smithay::desktop::Window,
    fullscreen: &crate::wayland::fullscreen::FullscreenManager,
) -> bool {
    !fullscreen.suppresses_window_chrome(window)
}

fn preview_outer_size(
    window: &smithay::desktop::Window,
    client: smithay::utils::Size<i32, Logical>,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    chrome_visible: bool,
) -> smithay::utils::Size<i32, Logical> {
    if chrome_visible {
        crate::titlebar::WindowChrome::for_visible_presentation(
            window,
            chrome_visible,
            decorations,
            font,
        )
        .outer_rect(Rectangle::from_size(client))
        .size
    } else {
        client
    }
}

fn preview_outer_rect(
    window: &smithay::desktop::Window,
    client: Rectangle<i32, Physical>,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    chrome_visible: bool,
) -> Rectangle<i32, Physical> {
    if !chrome_visible {
        return client;
    }
    crate::titlebar::WindowChrome::for_visible_presentation(
        window,
        chrome_visible,
        decorations,
        font,
    )
    .outer_rect(client.to_logical(1))
    .to_physical(1)
}

fn preview_visual_outer_rect(
    window: &smithay::desktop::Window,
    visual: crate::presentation::window::WindowVisualState,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    chrome_visible: bool,
) -> Rectangle<i32, Physical> {
    if !chrome_visible {
        return visual.animated_rect;
    }
    let opening_scale_y = if visual.presentation_rect.size.h > 0 {
        visual.animated_rect.size.h as f32 / visual.presentation_rect.size.h as f32
    } else {
        1.0
    };
    let decoration_scale = visual.zoom_scale * opening_scale_y.max(0.0);
    let chrome = crate::titlebar::WindowChrome::for_visible_presentation(
        window,
        chrome_visible,
        decorations,
        font,
    );
    let titlebar_height = crate::render::window_decoration::scaled_metric(
        crate::titlebar::effective_height(&decorations.titlebars, font.size),
        decoration_scale,
    );
    let border_width =
        crate::render::window_decoration::scaled_metric(chrome.border_width, decoration_scale);
    if chrome.has_server_titlebar() {
        crate::titlebar::DecorationLayout::new(
            visual.animated_rect,
            border_width,
            titlebar_height,
            &decorations.titlebars,
        )
        .outer
    } else {
        Rectangle::new(
            (
                visual.animated_rect.loc.x - border_width,
                visual.animated_rect.loc.y - border_width,
            )
                .into(),
            (
                visual.animated_rect.size.w + border_width * 2,
                visual.animated_rect.size.h + border_width * 2,
            )
                .into(),
        )
    }
}

pub(super) fn aspect_fit_rect(
    bounds: Rectangle<i32, Physical>,
    source_width: i32,
    source_height: i32,
) -> Rectangle<i32, Physical> {
    let source_width = source_width.max(1) as f32;
    let source_height = source_height.max(1) as f32;
    let scale = (bounds.size.w as f32 / source_width).min(bounds.size.h as f32 / source_height);
    let width = (source_width * scale).round().max(1.0) as i32;
    let height = (source_height * scale).round().max(1.0) as i32;
    Rectangle::new(
        (
            bounds.loc.x + (bounds.size.w - width) / 2,
            bounds.loc.y + (bounds.size.h - height) / 2,
        )
            .into(),
        (width, height).into(),
    )
}

pub(super) fn preview_card_rect(
    body: Rectangle<i32, Physical>,
    border_px: f32,
) -> Rectangle<i32, Physical> {
    let outset = border_px.max(0.0).ceil() as i32;
    Rectangle::new(
        (body.loc.x - outset, body.loc.y - outset).into(),
        (
            body.size.w + outset.saturating_mul(2),
            body.size.h + outset.saturating_mul(2),
        )
            .into(),
    )
}

pub(super) fn truncate_chars(text: &str, max: usize) -> String {
    let mut chars = text.trim().chars();
    let prefix = chars.by_ref().take(max).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::{APOGEE_SELECTION_ACCENT_MIX, APOGEE_SELECTION_BORDER_PX, apogee_window_chrome};
    use crate::render::overlays::shell::{OverlayRgb, OverlayVisuals};

    fn visuals(border_px: f32) -> OverlayVisuals {
        OverlayVisuals {
            fill: OverlayRgb {
                r: 0.1,
                g: 0.2,
                b: 0.3,
                a: 1.0,
            },
            text: OverlayRgb {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
            error: OverlayRgb {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            subtext: OverlayRgb {
                r: 0.8,
                g: 0.8,
                b: 0.8,
                a: 1.0,
            },
            key_fill: OverlayRgb {
                r: 0.2,
                g: 0.2,
                b: 0.2,
                a: 1.0,
            },
            border: OverlayRgb {
                r: 0.4,
                g: 0.7,
                b: 1.0,
                a: 1.0,
            },
            border_px,
            radius: 8.0,
        }
    }

    #[test]
    fn keyboard_selection_restores_accent_band_and_focus_frame() {
        let base = visuals(0.0);
        let (selected, caption_fill, card_fill) = apogee_window_chrome(base, true, false);

        assert_eq!(selected.border_px, APOGEE_SELECTION_BORDER_PX);
        assert_eq!(
            caption_fill,
            base.fill.mix(base.border, APOGEE_SELECTION_ACCENT_MIX)
        );
        assert_eq!(card_fill, base.fill.mix(base.border, 0.12));
    }

    #[test]
    fn idle_window_keeps_configured_chrome() {
        let base = visuals(2.0);
        let (idle, caption_fill, card_fill) = apogee_window_chrome(base, false, false);

        assert_eq!(idle.border_px, base.border_px);
        assert_eq!(caption_fill, base.fill);
        assert_eq!(card_fill, base.fill);
    }
}
