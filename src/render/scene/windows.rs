use super::*;

const JOIN_READY_TINT_ALPHA: f32 = 0.10;

pub(super) struct StackGroup {
    pub(super) stack_index: usize,
    pub(super) order: u64,
    pub(super) elements: Vec<SceneElement>,
}

pub(super) fn sort_stack_groups(groups: &mut [StackGroup]) {
    groups.sort_by_key(|group| (group.stack_index, group.order));
}

pub(super) struct LiveWindowScene {
    /// XDG popup trees are composed in the desktop popup plane, above the
    /// top layer-shell plane but below overlays. Keeping them separate from
    /// the root preserves that plane while the root remains in its stack.
    pub(super) popup_elements: Vec<SceneElement>,
    pub(super) elements: Vec<SceneElement>,
    pub(super) cluster_depth: Option<usize>,
    pub(super) cluster_floating: bool,
    pub(super) cluster_exclusive: bool,
}

pub(super) struct LiveWindowRenderers<'a> {
    pub arrange_textures: &'a mut crate::render::arrange_texture::ArrangeTextureTransitions,
    pub fullscreen_textures:
        &'a mut crate::render::fullscreen_texture::FullscreenTextureTransitions,
    pub backdrop_blur: &'a mut crate::render::effects::backdrop_blur::BackdropBlurRenderer,
    pub shadow: &'a mut crate::render::effects::shadow::ShadowRenderer,
    pub decoration: &'a mut crate::render::window_decoration::WindowDecorationRenderer,
    pub titlebar: &'a mut crate::render::titlebar::TitlebarRenderer,
    pub node: &'a mut crate::render::node::NodeRenderer,
    pub text: &'a mut crate::render::text::UiTextRenderer,
    pub pin: &'a mut crate::render::pin::PinRenderer,
    pub window_shaders: &'a mut crate::render::window_shader::WindowAnimationShaders,
}

#[derive(Clone, Copy)]
pub(super) struct LiveWindowContext<'a> {
    pub(super) space: &'a smithay::desktop::Space<smithay::desktop::Window>,
    pub(super) output: &'a Output,
    pub(super) output_geometry: Rectangle<i32, Logical>,
    pub(super) cameras: &'a crate::presentation::camera::OutputCameras,
    pub(super) clusters: &'a crate::clusters::ClusterSystem,
    pub(super) nodes: &'a crate::nodes::NodesState,
    pub(super) target_presentation_time: std::time::Duration,
    pub(super) focused:
        Option<&'a smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>,
    pub(super) decorations: &'a halley_config::Decorations,
    pub(super) pins: &'a halley_config::Pins,
    pub(super) overlays: &'a halley_config::Overlays,
    pub(super) font: &'a halley_config::Font,
    pub(super) blur: halley_config::Blur,
    pub(super) shadow_config: halley_config::ShadowLayer,
    pub(super) window_animations: &'a crate::animation::WindowAnimations,
    pub(super) fullscreen: &'a crate::wayland::fullscreen::FullscreenManager,
    pub(super) maximize: &'a crate::presentation::maximize::FieldMaximizeManager,
    pub(super) window_rules: &'a crate::window::rules::WindowRulesState,
    pub(super) cluster_presentation_override: Option<crate::clusters::WindowPresentation>,
    pub(super) instance_identity: Option<&'static str>,
    pub(super) titlebar_hovered: Option<&'a crate::titlebar::ButtonTarget>,
    pub(super) titlebar_pressed: Option<&'a crate::titlebar::ButtonTarget>,
}

/// Keep the accepted live endpoint in the offscreen blend until motion is
/// actually complete. Retiring at 99.5% exposed any client repaint since the
/// first accepted frame as a final-frame jump, particularly through Xwayland.
const CROSSFADE_COMPLETE: f64 = 1.0;

fn active_crossfade_completion(completion: Option<f64>) -> Option<f64> {
    completion.filter(|completion| *completion < CROSSFADE_COMPLETE)
}

fn compositor_chrome_visible(logical_fullscreen: bool, x11_fullscreen: bool) -> bool {
    !logical_fullscreen && !x11_fullscreen
}

#[allow(clippy::too_many_arguments)]
fn opening_shader_elements(
    renderer: &mut GlesRenderer,
    window: &smithay::desktop::Window,
    context: &LiveWindowContext<'_>,
    shaders: &crate::render::window_shader::WindowAnimationShaders,
    titlebar_renderer: &mut crate::render::titlebar::TitlebarRenderer,
    window_decoration_renderer: &mut crate::render::window_decoration::WindowDecorationRenderer,
    node_renderer: &mut crate::render::node::NodeRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    visual: &crate::presentation::window::WindowVisualState,
    chrome_visible: bool,
    focused: bool,
    alpha: f32,
) -> Result<Option<Vec<SceneElement>>, Box<dyn Error>> {
    let Some(surface) = window.wl_surface() else {
        return Ok(None);
    };
    let geo = opening_shader_geo(
        window,
        visual,
        context.decorations,
        context.font,
        chrome_visible,
    );
    if geo.size.w <= 0 || geo.size.h <= 0 {
        return Ok(None);
    }
    let texture = crate::render::window_texture::capture_decorated(
        renderer,
        window,
        None,
        context.decorations,
        context.font,
        focused,
        chrome_visible,
        context.maximize.contains(surface.as_ref()),
        titlebar_renderer,
        window_decoration_renderer,
        node_renderer,
        ui_text,
    )?;
    let Some(shader) = shaders.open_element(
        renderer,
        &texture,
        crate::render::window_decoration::surface_slot_for_instance(
            surface.as_ref(),
            crate::render::window_decoration::slot::JOIN_TINT,
            context.instance_identity,
        ),
        geo,
        visual.opening_progress(),
        visual.opening_clamped_progress(),
        visual.opening_random_seed(),
        alpha,
    ) else {
        return Ok(None);
    };
    // A geometry shadow would remain a solid rectangle while an arbitrary
    // shader deforms or dissolves its pixels. Matching that silhouette would
    // require rendering and blurring a separate alpha mask, so custom window
    // shaders deliberately own the complete visual here.
    Ok(Some(vec![SceneElement::WindowShader(shader)]))
}

fn opening_shader_geo(
    window: &smithay::desktop::Window,
    visual: &crate::presentation::window::WindowVisualState,
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
    let chrome = crate::titlebar::WindowChrome::for_window(window, decorations, font);
    let border_width =
        crate::render::window_decoration::scaled_metric(chrome.border_width, decoration_scale);
    if chrome.has_server_titlebar() {
        let titlebar_height =
            crate::titlebar::rendered_metrics(&decorations.titlebars, font.size, decoration_scale)
                .height;
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

fn quantized_f32(value: f32) -> i32 {
    (value * 256.0).round() as i32
}

fn window_blur_epoch(visual: &crate::presentation::window::WindowVisualState) -> u64 {
    use std::hash::{Hash, Hasher};

    // Quantize so idle float noise does not recapture every frame.
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    quantized_f32(visual.zoom_scale).hash(&mut hash);
    (visual.camera_center.x.round() as i32).hash(&mut hash);
    (visual.camera_center.y.round() as i32).hash(&mut hash);
    quantized_f32(visual.opening_alpha).hash(&mut hash);
    if let Some(presentation) = visual.fullscreen {
        quantized_f32(presentation.transition_completion as f32).hash(&mut hash);
    }
    if let Some(presentation) = visual.maximize {
        quantized_f32(presentation.transition_completion as f32).hash(&mut hash);
    }
    hash.finish()
}

fn should_hold_x11_fullscreen_exit(
    is_x11: bool,
    has_fullscreen_presentation: bool,
    is_fullscreen_or_pending: bool,
) -> bool {
    is_x11 && has_fullscreen_presentation && !is_fullscreen_or_pending
}

pub(super) fn live_window_elements(
    renderer: &mut GlesRenderer,
    window: &smithay::desktop::Window,
    context: LiveWindowContext<'_>,
    renderers: LiveWindowRenderers<'_>,
) -> Result<LiveWindowScene, Box<dyn Error>> {
    let LiveWindowRenderers {
        arrange_textures,
        fullscreen_textures,
        backdrop_blur: backdrop_blur_renderer,
        shadow: shadow_renderer,
        decoration: window_decoration_renderer,
        titlebar: titlebar_renderer,
        node: node_renderer,
        text: ui_text,
        pin: pin_renderer,
        window_shaders,
    } = renderers;
    let Some(location) = context.space.element_location(window) else {
        return Ok(LiveWindowScene {
            popup_elements: Vec::new(),
            elements: Vec::new(),
            cluster_depth: None,
            cluster_floating: false,
            cluster_exclusive: false,
        });
    };
    let Some(window_surface) = window.wl_surface() else {
        return Ok(LiveWindowScene {
            popup_elements: Vec::new(),
            elements: Vec::new(),
            cluster_depth: None,
            cluster_floating: false,
            cluster_exclusive: false,
        });
    };
    let join_ready = context
        .nodes
        .id_for_surface(window_surface.as_ref())
        .is_some_and(|member| {
            context
                .clusters
                .join_ready_for(member, &context.output.name())
        });
    let Some(visual) = window_visual_state_with_cluster_presentation(
        context.space,
        context.cameras,
        Some(context.clusters),
        Some(context.nodes),
        window,
        context.output,
        context.window_animations,
        context.fullscreen,
        context.maximize,
        context.decorations,
        context.font,
        context.target_presentation_time,
        context.cluster_presentation_override,
    ) else {
        return Ok(LiveWindowScene {
            popup_elements: Vec::new(),
            elements: Vec::new(),
            cluster_depth: None,
            cluster_floating: false,
            cluster_exclusive: false,
        });
    };
    if visual.animated_rect.size.w == 0 || visual.animated_rect.size.h == 0 {
        return Ok(LiveWindowScene {
            popup_elements: Vec::new(),
            elements: Vec::new(),
            cluster_depth: visual.cluster_depth,
            cluster_floating: visual.cluster_floating,
            cluster_exclusive: visual.cluster_exclusive,
        });
    }

    let mut popup_elements = Vec::new();
    let mut elements = Vec::new();
    let chrome =
        crate::titlebar::WindowChrome::for_window(window, context.decorations, context.font);
    let managed = chrome.mode != crate::titlebar::DecorationMode::Unmanaged;
    let rule_opacity = if managed {
        context.window_rules.opacity(window_surface.as_ref())
    } else {
        1.0
    };
    let alpha = visual.opening_alpha * rule_opacity;
    // X11 clients can churn fullscreen requests while changing video modes.
    // Keep their advertised EWMH state as a render-time backstop so a missing
    // or temporarily retired presentation entry cannot expose compositor
    // chrome around an otherwise-fullscreen game.
    let chrome_visible = compositor_chrome_visible(
        context
            .fullscreen
            .suppresses_chrome(window_surface.as_ref()),
        crate::xwayland::is_fullscreen(window),
    );
    let chrome_alpha = if chrome_visible { alpha } else { 0.0 };
    let server_titlebar = chrome_visible && chrome.has_server_titlebar();
    let node_id = context.nodes.id_for_surface(window_surface.as_ref());
    let user_pinned = node_id.is_some_and(|id| {
        context.clusters.cluster_for_member(id).is_none()
            && context.nodes.field.node(id).is_some_and(|node| node.pinned)
    });
    let arrange_animating = context
        .window_animations
        .is_arranging(window_surface.as_ref(), context.target_presentation_time);
    let presentation_scale_y = decoration_presentation_scale(
        arrange_animating,
        visual.presentation_rect.size.h,
        visual.animated_rect.size.h,
    );
    let decoration_scale = visual.zoom_scale * presentation_scale_y.max(0.0);
    let titlebar_metrics = crate::titlebar::rendered_metrics(
        &context.decorations.titlebars,
        context.font.size,
        decoration_scale,
    );
    let titlebar_height = titlebar_metrics.height;
    let border_width =
        crate::render::window_decoration::scaled_metric(chrome.border_width, decoration_scale);
    let content_radius = if chrome_visible {
        crate::render::window_decoration::scaled_metric(
            context.decorations.border_radius_px,
            decoration_scale,
        ) as f32
    } else {
        0.0
    };
    // Override-redirect X11 windows are unmanaged for focus, borders, shadows,
    // and titlebars, but their client content should still honor the configured
    // window radius. This covers popup windows such as Steam menus without
    // turning them into managed toplevels.
    let rounded = content_radius > 0.0;
    let rounded_available = rounded && window_decoration_renderer.available(renderer);
    if join_ready {
        let tint_alpha = alpha * JOIN_READY_TINT_ALPHA;
        let focused = context.decorations.border_color_focused;
        let tint_color =
            smithay::backend::renderer::Color32F::new(focused.r, focused.g, focused.b, 1.0);
        let radii = if rounded_available && server_titlebar {
            crate::render::window_decoration::CornerRadii::bottom(content_radius)
        } else if rounded_available {
            crate::render::window_decoration::CornerRadii::all(content_radius)
        } else {
            crate::render::window_decoration::CornerRadii::default()
        };
        if let Some(tint) = window_decoration_renderer.tint_element_with_radii(
            renderer,
            crate::render::window_decoration::surface_slot_for_instance(
                window_surface.as_ref(),
                crate::render::window_decoration::slot::JOIN_TINT,
                context.instance_identity,
            ),
            visual.animated_rect,
            radii,
            tint_color,
            tint_alpha,
        ) {
            elements.push(SceneElement::RoundedTexture(tint));
        } else {
            elements.push(SceneElement::Border(SolidColorRenderElement::new(
                crate::render::window_decoration::surface_slot_for_instance(
                    window_surface.as_ref(),
                    crate::render::window_decoration::slot::JOIN_TINT_FALLBACK,
                    context.instance_identity,
                ),
                visual.animated_rect,
                crate::render::window_decoration::solid_color_commit(tint_color * tint_alpha),
                tint_color * tint_alpha,
                Kind::Unspecified,
            )));
        }
    }
    let surface_location = crate::render::window_surface_location(location, window.geometry());
    let (popup_surfaces, surface_elements) =
        crate::render::window_surface_elements(renderer, window, surface_location, alpha);
    popup_elements.extend(popup_surfaces.into_iter().map(|surface_element| {
        let native_geometry = surface_element.geometry(Scale::from(1.0));
        let destination = if visual.maps_from_source() {
            let destination = crate::animation::map_rect(
                native_geometry,
                visual.source_geometry.to_physical(1),
                visual.presentation_rect,
            );
            crate::animation::map_rect(destination, visual.presentation_rect, visual.animated_rect)
        } else {
            let final_destination = crate::render::camera_rect(
                native_geometry,
                visual.camera_center,
                context.output_geometry.size.to_physical(1),
                visual.zoom_scale,
            );
            crate::animation::map_rect(final_destination, visual.camera_rect, visual.animated_rect)
        };
        SceneElement::Rescaled(crate::render::rescale::RescaledElement::new(
            surface_element,
            destination,
        ))
    }));
    if visual.shader_pixels()
        && window_shaders.open_available()
        && visual.fullscreen.is_none()
        && visual.maximize.is_none()
        && !arrange_animating
    {
        match opening_shader_elements(
            renderer,
            window,
            &context,
            window_shaders,
            titlebar_renderer,
            window_decoration_renderer,
            node_renderer,
            ui_text,
            &visual,
            chrome_visible,
            Some(window_surface.as_ref()) == context.focused,
            alpha,
        ) {
            Ok(Some(shader_elements)) => {
                return Ok(LiveWindowScene {
                    popup_elements,
                    elements: shader_elements,
                    cluster_depth: visual.cluster_depth,
                    cluster_floating: visual.cluster_floating,
                    cluster_exclusive: visual.cluster_exclusive,
                });
            }
            Ok(None) => {}
            Err(error) => eventline::warn!("window open shader: {error}"),
        }
    }
    if server_titlebar && chrome_alpha > 0.0 {
        append_titlebar_elements(
            renderer,
            window,
            context.instance_identity,
            visual.animated_rect,
            titlebar_height,
            titlebar_metrics.glyph_size,
            decoration_scale,
            context.maximize.contains(window_surface.as_ref()),
            border_width,
            titlebar_metrics.radius as f32,
            Some(window_surface.as_ref()) == context.focused,
            chrome_alpha,
            context.decorations,
            context.titlebar_hovered,
            context.titlebar_pressed,
            user_pinned,
            titlebar_renderer,
            window_decoration_renderer,
            node_renderer,
            ui_text,
            &mut elements,
        )?;
    }
    let texture_transition_completion = active_crossfade_completion(
        visual
            .fullscreen
            .map(|presentation| presentation.transition_completion)
            .or_else(|| {
                visual
                    .maximize
                    .map(|presentation| presentation.transition_completion)
            }),
    );
    let client_radii = if rounded_available && server_titlebar {
        crate::render::window_decoration::CornerRadii::bottom(content_radius)
    } else if rounded_available {
        crate::render::window_decoration::CornerRadii::all(content_radius)
    } else {
        crate::render::window_decoration::CornerRadii::default()
    };
    let arrange_blend = if arrange_animating {
        let completion = context
            .window_animations
            .arrange_completion(window_surface.as_ref(), context.target_presentation_time)
            .unwrap_or(0.0);
        match arrange_textures.native_blend_element(
            renderer,
            window_surface.as_ref(),
            visual.animated_rect,
            visual.zoom_scale,
            completion,
            alpha,
            client_radii,
        ) {
            Ok(blend) => blend,
            Err(err) => {
                eventline::warn!("field arrange: failed to render native reveal: {err}");
                None
            }
        }
    } else {
        None
    };
    let arrange_fallback = if arrange_animating && arrange_blend.is_none() {
        arrange_textures.fallback_element(window_surface.as_ref(), visual.animated_rect, alpha)
    } else {
        None
    };
    let texture_blend = if arrange_blend.is_none()
        && arrange_fallback.is_none()
        && let Some(completion) = texture_transition_completion
    {
        let hold_x11_fullscreen_exit = should_hold_x11_fullscreen_exit(
            crate::xwayland::is_x11(window),
            visual.fullscreen.is_some(),
            context
                .fullscreen
                .is_fullscreen_or_pending(window_surface.as_ref()),
        );
        match fullscreen_textures.blend_element(
            renderer,
            crate::render::fullscreen_texture::BlendRequest {
                window,
                destination: visual.animated_rect,
                progress: completion,
                hold_previous_until_restored_buffer_matches: hold_x11_fullscreen_exit,
                alpha,
                radii: client_radii,
            },
        ) {
            Ok(blend) => blend,
            Err(err) => {
                eventline::warn!("window transition: failed to blend textures: {err}");
                None
            }
        }
    } else {
        None
    };
    if let Some(blend) = arrange_blend {
        elements.push(SceneElement::WindowResize(blend));
    } else if let Some((base, texture)) = arrange_fallback {
        if rounded_available {
            let element = window_decoration_renderer
                .texture_element_with_radii(
                    renderer,
                    base,
                    texture,
                    visual.animated_rect,
                    client_radii,
                    (1.0, 1.0, 1.0, 1.0),
                )
                .expect("rounded resources were checked above");
            elements.push(SceneElement::RoundedTexture(element));
        } else {
            elements.push(SceneElement::Closing(base));
        }
    } else if let Some(blend) = texture_blend {
        elements.push(SceneElement::WindowResize(blend));
    } else {
        for surface_element in surface_elements {
            let native_geometry = surface_element.geometry(Scale::from(1.0));
            let destination = if visual.maps_from_source() {
                let destination = crate::animation::map_rect(
                    native_geometry,
                    visual.source_geometry.to_physical(1),
                    visual.presentation_rect,
                );
                crate::animation::map_rect(
                    destination,
                    visual.presentation_rect,
                    visual.animated_rect,
                )
            } else {
                let final_destination = crate::render::camera_rect(
                    native_geometry,
                    visual.camera_center,
                    context.output_geometry.size.to_physical(1),
                    visual.zoom_scale,
                );
                crate::animation::map_rect(
                    final_destination,
                    visual.camera_rect,
                    visual.animated_rect,
                )
            };
            if rounded_available {
                let radii = if server_titlebar {
                    crate::render::window_decoration::CornerRadii::bottom(content_radius)
                } else {
                    crate::render::window_decoration::CornerRadii::all(content_radius)
                };
                let element = window_decoration_renderer
                    .surface_element_with_radii(
                        renderer,
                        surface_element,
                        destination,
                        visual.animated_rect,
                        radii,
                    )
                    .expect("rounded resources were checked above");
                if let Some(element) =
                    CropRenderElement::from_element(element, 1.0, visual.animated_rect)
                {
                    elements.push(SceneElement::RoundedCropped(element));
                }
                continue;
            }
            let element =
                crate::render::rescale::RescaledElement::new(surface_element, destination);
            if let Some(element) =
                CropRenderElement::from_element(element, 1.0, visual.animated_rect)
            {
                elements.push(SceneElement::Cropped(element));
            }
        }
    }

    let surface_size =
        with_renderer_surface_state(window_surface.as_ref(), |state| state.surface_size())
            .flatten();
    if let Some(surface_size) = surface_size {
        let output_bounds =
            Rectangle::<i32, Physical>::from_size(context.output_geometry.size.to_physical(1));
        let rule_blur = context.window_rules.blur(window_surface.as_ref());
        let mut requested = if rule_blur != Some(false) {
            crate::wayland::background_effect::blur_rects(window_surface.as_ref(), surface_size)
        } else {
            Vec::new()
        };
        let global_blur_allowed = context
            .fullscreen
            .allows_global_blur(window_surface.as_ref());
        let policy_blur =
            managed && halley_config::window_blur_enabled(rule_blur, !global_blur_allowed);
        if requested.is_empty() && policy_blur {
            requested.push(Rectangle::from_size(surface_size));
        }
        let patches = requested
            .into_iter()
            .filter_map(|rect| {
                let native = Rectangle::<i32, Physical>::new(
                    surface_location + rect.loc.to_physical(1),
                    rect.size.to_physical(1),
                );
                let destination = if visual.maps_from_source() {
                    let destination = crate::animation::map_rect(
                        native,
                        visual.source_geometry.to_physical(1),
                        visual.presentation_rect,
                    );
                    crate::animation::map_rect(
                        destination,
                        visual.presentation_rect,
                        visual.animated_rect,
                    )
                } else {
                    let final_destination = crate::render::camera_rect(
                        native,
                        visual.camera_center,
                        context.output_geometry.size.to_physical(1),
                        visual.zoom_scale,
                    );
                    crate::animation::map_rect(
                        final_destination,
                        visual.camera_rect,
                        visual.animated_rect,
                    )
                };
                destination
                    .intersection(output_bounds)
                    .and_then(|rect| rect.intersection(visual.animated_rect))
                    .map(|rect| crate::render::effects::backdrop_blur::BlurPatch {
                        rect,
                        radius: 0.0,
                        alpha,
                        clip: rounded_available.then_some((
                            visual.animated_rect,
                            if server_titlebar {
                                crate::render::window_decoration::CornerRadii::bottom(
                                    content_radius,
                                )
                            } else {
                                crate::render::window_decoration::CornerRadii::all(content_radius)
                            },
                        )),
                    })
            })
            .collect::<Vec<_>>();
        if let Some(blur) = backdrop_blur_renderer.blur_element(
            renderer,
            &context.output.name(),
            crate::render::effects::backdrop_blur::BlurIdentity::Window {
                surface: Id::from_wayland_resource(window_surface.as_ref()),
                instance: context.instance_identity.unwrap_or("canonical").to_string(),
            },
            context.output_geometry.size,
            patches,
            context.blur,
            window_blur_epoch(&visual),
        )? {
            elements.push(SceneElement::BackdropBlur(blur));
        }
    }

    let is_focused = Some(window_surface.as_ref()) == context.focused;
    let border_color = crate::render::window_border_color(context.decorations, is_focused);
    if managed && border_width > 0 && chrome_alpha > 0.0 {
        if rounded_available
            && let Some(border) = if server_titlebar {
                window_decoration_renderer.body_border_element(
                    renderer,
                    crate::render::window_decoration::surface_slot_for_instance(
                        window_surface.as_ref(),
                        crate::render::window_decoration::slot::BODY_BORDER,
                        context.instance_identity,
                    ),
                    visual.animated_rect,
                    border_width,
                    content_radius,
                    border_color,
                    chrome_alpha,
                )
            } else {
                window_decoration_renderer.border_element(
                    renderer,
                    crate::render::window_decoration::surface_slot_for_instance(
                        window_surface.as_ref(),
                        crate::render::window_decoration::slot::BORDER,
                        context.instance_identity,
                    ),
                    visual.animated_rect,
                    border_width,
                    content_radius,
                    border_color,
                    chrome_alpha,
                )
            }
        {
            elements.push(SceneElement::WindowBorder(border));
        } else {
            let strips: Vec<_> = if server_titlebar {
                crate::render::body_border_strips(
                    std::array::from_fn(|index| {
                        crate::render::window_decoration::surface_slot_for_instance(
                            window_surface.as_ref(),
                            crate::render::window_decoration::slot::BODY_BORDER_FALLBACK + index,
                            context.instance_identity,
                        )
                    }),
                    visual.animated_rect,
                    border_width,
                    border_color * chrome_alpha,
                )
                .into_iter()
                .collect()
            } else {
                crate::render::border_strips(
                    std::array::from_fn(|index| {
                        crate::render::window_decoration::surface_slot_for_instance(
                            window_surface.as_ref(),
                            crate::render::window_decoration::slot::BORDER_FALLBACK + index,
                            context.instance_identity,
                        )
                    }),
                    visual.animated_rect,
                    border_width,
                    border_color * chrome_alpha,
                )
                .into_iter()
                .collect()
            };
            elements.extend(strips.into_iter().map(SceneElement::Border));
        }
    }
    if managed && chrome_alpha > 0.0 {
        let border_outset = border_width.max(0);
        let caster = if server_titlebar {
            crate::titlebar::DecorationLayout::new(
                visual.animated_rect,
                border_outset,
                titlebar_height,
                &context.decorations.titlebars,
            )
            .outer
        } else {
            Rectangle::new(
                (
                    visual.animated_rect.loc.x - border_outset,
                    visual.animated_rect.loc.y - border_outset,
                )
                    .into(),
                (
                    (visual.animated_rect.size.w + border_outset * 2).max(1),
                    (visual.animated_rect.size.h + border_outset * 2).max(1),
                )
                    .into(),
            )
        };
        let caster_radii = if rounded_available && server_titlebar {
            crate::render::window_decoration::CornerRadii {
                top: titlebar_metrics.radius as f32,
                bottom: content_radius + border_outset as f32,
            }
        } else if rounded_available {
            crate::render::window_decoration::CornerRadii::all(
                content_radius + border_outset as f32,
            )
        } else {
            crate::render::window_decoration::CornerRadii::default()
        };
        if let Some(shadow) = shadow_renderer.element_with_radii(
            renderer,
            format!(
                "{}:window:{:?}:{}",
                context.output.name(),
                window_surface.id(),
                context.instance_identity.unwrap_or("canonical")
            ),
            caster,
            caster_radii,
            chrome_alpha,
            context.shadow_config,
        )? {
            elements.push(SceneElement::Shadow(shadow));
        }
    }
    if user_pinned
        && chrome_visible
        && let Some(pin) = pin_renderer.element(
            renderer,
            &context.output.name(),
            crate::render::pin::PinSlot::Window(
                node_id.expect("pinned window has a node").as_u64(),
            ),
            if server_titlebar {
                let titlebar = crate::titlebar::DecorationLayout::new(
                    visual.animated_rect,
                    border_width,
                    titlebar_height,
                    &context.decorations.titlebars,
                )
                .titlebar;
                crate::render::pin::window_titlebar_badge_rect(
                    context.pins,
                    titlebar,
                    context.decorations.titlebars.button_position,
                    visual.zoom_scale,
                )
            } else {
                crate::render::pin::window_badge_rect(
                    context.pins,
                    visual.animated_rect,
                    visual.zoom_scale,
                )
            },
            alpha,
            context.pins,
            context.overlays,
            context.decorations,
        )
    {
        elements.insert(0, SceneElement::Closing(pin));
    }
    // X11 models menus as independent override-redirect windows rather than
    // XDG popup roles. Put their complete unmanaged scene in the same desktop
    // popup plane so native Wayland and XWayland applications agree.
    if crate::xwayland::is_override_redirect(window) {
        popup_elements.append(&mut elements);
    }
    Ok(LiveWindowScene {
        popup_elements,
        elements,
        cluster_depth: visual.cluster_depth,
        cluster_floating: visual.cluster_floating,
        cluster_exclusive: visual.cluster_exclusive,
    })
}

/// Stable identity for a titlebar part, falling back to a fresh `Id` for
/// windows without a surface (only reachable from one-shot snapshot renders,
/// which never go through a damage tracker).
fn titlebar_slot(window: &smithay::desktop::Window, instance: Option<&str>, slot: usize) -> Id {
    use smithay::wayland::seat::WaylandFocus;
    window
        .wl_surface()
        .map(|surface| {
            crate::render::window_decoration::surface_slot_for_instance(
                surface.as_ref(),
                slot,
                instance,
            )
        })
        .unwrap_or_else(Id::new)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_titlebar_elements(
    renderer: &mut GlesRenderer,
    window: &smithay::desktop::Window,
    instance: Option<&str>,
    content: Rectangle<i32, Physical>,
    height: i32,
    glyph_side: i32,
    identity_scale: f32,
    maximized: bool,
    border_width: i32,
    radius: f32,
    focused: bool,
    alpha: f32,
    decorations: &halley_config::Decorations,
    hovered: Option<&crate::titlebar::ButtonTarget>,
    pressed: Option<&crate::titlebar::ButtonTarget>,
    reserve_pin_badge: bool,
    titlebar_renderer: &mut crate::render::titlebar::TitlebarRenderer,
    window_decoration_renderer: &mut crate::render::window_decoration::WindowDecorationRenderer,
    node_renderer: &mut crate::render::node::NodeRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let config = &decorations.titlebars;
    let mut layout = crate::titlebar::DecorationLayout::new(content, border_width, height, config);
    if reserve_pin_badge {
        layout.reserve_opposite_controls(config.button_position);
    }
    let background = if focused {
        config.color_focused
    } else {
        config.color_unfocused
    };
    let foreground = if focused {
        config.foreground_color_focused
    } else {
        config.foreground_color_unfocused
    };

    for (index, control) in layout.controls.iter().enumerate() {
        let enabled = crate::titlebar::control_enabled(window, control.control);
        let is_hovered = hovered
            .is_some_and(|target| target.window == *window && target.control == control.control)
            && enabled;
        let is_pressed = pressed
            .is_some_and(|target| target.window == *window && target.control == control.control)
            && enabled;
        let state_color = if is_pressed {
            config.button_pressed_color
        } else if is_hovered {
            config.button_hover_color
        } else {
            foreground
        };
        let backplate = if is_hovered || is_pressed {
            let backplate_alpha = if is_pressed { 0.30 } else { 0.18 };
            window_decoration_renderer.tint_element_with_radii(
                renderer,
                titlebar_slot(
                    window,
                    instance,
                    crate::render::window_decoration::slot::TITLEBAR_BUTTON + index,
                ),
                control.rect,
                crate::render::window_decoration::CornerRadii::all(
                    (control.rect.size.h as f32 * 0.20).max(1.0),
                ),
                crate::render::decoration_color(state_color),
                alpha * backplate_alpha,
            )
        } else {
            None
        };
        let glyph = Rectangle::new(
            (
                control.rect.loc.x + (control.rect.size.w - glyph_side) / 2,
                control.rect.loc.y + (control.rect.size.h - glyph_side) / 2,
            )
                .into(),
            (glyph_side, glyph_side).into(),
        );
        if let Some(icon) = titlebar_renderer.control_element(
            renderer,
            window_decoration_renderer,
            crate::render::titlebar::ControlRequest {
                id: titlebar_slot(
                    window,
                    instance,
                    crate::render::window_decoration::slot::TITLEBAR_GLYPH
                        + index
                        + usize::from(maximized) * 8,
                ),
                control: control.control,
                maximized,
                destination: glyph,
                color: state_color,
                alpha: alpha * if enabled { 1.0 } else { 0.4 },
            },
        ) {
            elements.push(SceneElement::RoundedTexture(icon));
        }
        if let Some(backplate) = backplate {
            elements.push(SceneElement::RoundedTexture(backplate));
        }
    }

    let identity = crate::window::rules::identity(window);
    let app_id = config
        .show_icons
        .then_some(identity.app_id.as_deref())
        .flatten();
    let rgb = color_bytes(foreground);
    let title = if config.show_title {
        match identity.title.as_deref() {
            Some(title) => fitted_title(
                renderer,
                ui_text,
                title,
                rgb,
                layout.max_title_width_scaled(
                    config.title_position,
                    app_id.is_some(),
                    identity_scale,
                ),
                identity_scale,
                config.text_size_px,
            )?,
            None => None,
        }
    } else {
        None
    };
    let identity_layout = layout.identity_layout_scaled(
        config.title_position,
        title.as_ref().map(|title| (title.size.w, title.size.h)),
        app_id.is_some(),
        identity_scale,
    );
    if let Some(icon_rect) = identity_layout.app_icon
        && let Some(app_id) = app_id
        && let Some(icon) = node_renderer.app_icon_element(renderer, app_id, icon_rect, alpha)
    {
        elements.push(SceneElement::NodeTexture(icon));
    }

    if let (Some(title), Some(title_rect)) = (title, identity_layout.title) {
        let prepared = match config.text_size_px {
            Some(size_px) => ui_text.element_scaled_at_size(
                renderer,
                title_rect,
                &title.text,
                rgb,
                alpha,
                size_px,
            )?,
            None => ui_text.element_scaled(renderer, title_rect, &title.text, rgb, alpha)?,
        };
        if let Some(prepared) = prepared {
            elements.push(SceneElement::UiText(prepared.element));
        }
    }

    if let Some(background_element) = window_decoration_renderer.tint_element_with_radii(
        renderer,
        titlebar_slot(
            window,
            instance,
            crate::render::window_decoration::slot::TITLEBAR_BACKGROUND,
        ),
        layout.titlebar,
        crate::render::window_decoration::CornerRadii::top(radius),
        crate::render::decoration_color(background),
        alpha,
    ) {
        elements.push(SceneElement::RoundedTexture(background_element));
    } else {
        elements.push(SceneElement::Border(SolidColorRenderElement::new(
            titlebar_slot(
                window,
                instance,
                crate::render::window_decoration::slot::TITLEBAR_BACKGROUND_FALLBACK,
            ),
            layout.titlebar,
            crate::render::window_decoration::solid_color_commit(
                crate::render::decoration_color(background) * alpha,
            ),
            crate::render::decoration_color(background) * alpha,
            Kind::Unspecified,
        )));
    }
    Ok(())
}

fn decoration_presentation_scale(
    arranging: bool,
    presentation_height: i32,
    animated_height: i32,
) -> f32 {
    if arranging {
        // Arrangement holds a pre-configure client snapshot. Chrome is still
        // compositor-rendered, so keep it at the Field display scale instead
        // of deriving its height from a native client size that can change
        // midway through the timeline.
        1.0
    } else if presentation_height > 0 {
        animated_height as f32 / presentation_height as f32
    } else {
        1.0
    }
}

fn color_bytes(color: halley_config::BorderColor) -> [u8; 3] {
    [
        (color.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.b.clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

struct FittedTitle {
    text: String,
    size: smithay::utils::Size<i32, smithay::utils::Physical>,
}

fn fitted_title(
    renderer: &mut GlesRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    title: &str,
    rgb: [u8; 3],
    max_width: i32,
    scale: f32,
    text_size_px: Option<u16>,
) -> Result<Option<FittedTitle>, Box<dyn Error>> {
    if max_width <= 0 || title.is_empty() {
        return Ok(None);
    }
    let mut measure =
        |ui_text: &mut crate::render::text::UiTextRenderer, text: &str| match text_size_px {
            Some(size_px) => ui_text.measure_at_size(renderer, text, rgb, size_px),
            None => ui_text.measure(renderer, text, rgb),
        };
    if let Some(native_size) = measure(ui_text, title)?
        && scaled_title_size(native_size, scale).w <= max_width
    {
        return Ok(Some(FittedTitle {
            text: title.to_string(),
            size: scaled_title_size(native_size, scale),
        }));
    }
    let characters = title.chars().collect::<Vec<_>>();
    let mut low = 0;
    let mut high = characters.len();
    let mut best = None;
    while low <= high {
        let middle = low + (high - low) / 2;
        let candidate = characters[..middle]
            .iter()
            .chain(std::iter::once(&'…'))
            .collect::<String>();
        let Some(native_size) = measure(ui_text, &candidate)? else {
            return Ok(None);
        };
        let size = scaled_title_size(native_size, scale);
        if size.w <= max_width {
            best = Some(FittedTitle {
                text: candidate,
                size,
            });
            low = middle + 1;
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
    }
    Ok(best)
}

fn scaled_title_size(
    native: smithay::utils::Size<i32, smithay::utils::Buffer>,
    scale: f32,
) -> smithay::utils::Size<i32, smithay::utils::Physical> {
    (
        crate::render::window_decoration::scaled_metric(native.w, scale),
        crate::render::window_decoration::scaled_metric(native.h, scale),
    )
        .into()
}

#[cfg(test)]
mod tests {
    use smithay::utils::{Buffer, Size};

    use super::{
        active_crossfade_completion, compositor_chrome_visible, decoration_presentation_scale,
        quantized_f32, scaled_title_size, should_hold_x11_fullscreen_exit,
    };

    #[test]
    fn camera_epoch_quantization_ignores_idle_float_noise() {
        assert_eq!(quantized_f32(1.0), quantized_f32(1.0 + f32::EPSILON));
        assert_ne!(quantized_f32(1.0), quantized_f32(0.75));
    }

    #[test]
    fn accepted_endpoint_stays_offscreen_until_motion_exactly_finishes() {
        assert_eq!(active_crossfade_completion(Some(0.995)), Some(0.995));
        assert_eq!(
            active_crossfade_completion(Some(0.999_999)),
            Some(0.999_999)
        );
        assert_eq!(active_crossfade_completion(Some(1.0)), None);
    }

    #[test]
    fn arrangement_keeps_chrome_scale_stable_across_client_resize_commits() {
        assert_eq!(decoration_presentation_scale(true, 600, 900), 1.0);
        assert_eq!(decoration_presentation_scale(true, 1200, 900), 1.0);
        assert_eq!(decoration_presentation_scale(false, 600, 900), 1.5);
    }

    #[test]
    fn title_text_size_shrinks_with_zoom() {
        let native = Size::<i32, Buffer>::from((120, 18));

        assert_eq!(scaled_title_size(native, 1.0), (120, 18).into());
        assert_eq!(scaled_title_size(native, 0.5), (60, 9).into());
        assert_eq!(scaled_title_size(native, 0.35), (42, 6).into());
    }

    #[test]
    fn either_fullscreen_signal_suppresses_compositor_chrome() {
        assert!(compositor_chrome_visible(false, false));
        assert!(!compositor_chrome_visible(true, false));
        assert!(!compositor_chrome_visible(false, true));
        assert!(!compositor_chrome_visible(true, true));
    }

    #[test]
    fn only_x11_fullscreen_exit_waits_for_the_restored_buffer() {
        assert!(should_hold_x11_fullscreen_exit(true, true, false));
        assert!(!should_hold_x11_fullscreen_exit(true, true, true));
        assert!(!should_hold_x11_fullscreen_exit(true, false, false));
        assert!(!should_hold_x11_fullscreen_exit(false, true, false));
    }
}
