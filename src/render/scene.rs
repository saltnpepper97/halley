use std::error::Error;

use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::utils::CropRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, render_elements};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::{CommitCounter, with_renderer_surface_state};
use smithay::desktop::{PopupManager, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Physical, Rectangle, Scale};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::Layer;

use super::RenderRequest;
use crate::presentation::window::{
    window_visual_state, window_visual_state_with_cluster_presentation,
};
#[cfg(test)]
mod animation_pixels;
mod apogee_clusters;
mod capture_ui;
mod cluster_composer;
mod clusters;
mod effects;
pub(crate) mod nodes;
mod overview;
mod windows;

use capture_ui::capture_overlay_elements;
use cluster_composer::cluster_composer_elements;
use clusters::{ClusterElementContext, cluster_elements};
use effects::{
    OverlayEffectStyle, append_compositor_overlay_blur, append_overlay_shadows,
    layer_surface_scene_elements,
};
use nodes::{NodeElementContext, node_elements};
use overview::{
    FocusCycleRenderContext, apogee_elements, focus_cycle_elements, hover_preview_elements,
};
pub(crate) use windows::append_titlebar_elements;
use windows::{
    LiveWindowContext, LiveWindowRenderers, StackGroup, live_window_elements, sort_stack_groups,
};

pub(crate) use nodes::{contrast_text_rgb, node_fill_color, node_ring_color};

#[cfg(test)]
use capture_ui::picker_layout;
#[cfg(test)]
use overview::{
    apogee_caption_rect, apogee_transition_visuals, aspect_fit_rect, preview_card_rect,
    preview_content_radius, sort_apogee_tiles,
};

#[cfg(test)]
use smithay::utils::Point;

render_elements! {
    /// The complete front-to-back scene consumed by both presentation
    /// backends. Keeping one element type and one builder prevents nested and
    /// real-hardware sessions from drifting in z-order or visual policy.
    pub SceneElement<=GlesRenderer>;
    Cursor=crate::cursor::render::CursorRenderElement,
    Background=super::background::BackgroundElement,
    Rescaled=super::rescale::RescaledElement,
    Cropped=CropRenderElement<super::rescale::RescaledElement>,
    RoundedCropped=CropRenderElement<super::window_decoration::RoundedSurfaceElement>,
    WindowResize=super::resize::ResizeRenderElement,
    WindowShader=super::window_shader::WindowShaderRenderElement,
    WindowOpen=super::window_open::WindowOpenElement,
    WindowBorder=super::window_decoration::RoundedBorderElement,
    RoundedTexture=super::window_decoration::RoundedTextureElement,
    ClusterCore=crate::clusters::render::ClusterCoreElement,
    ClusterIcon=crate::clusters::render::ClusterIconElement,
    Node=super::node::NodeRenderElement,
    NodeLabel=super::node::LabelRenderElement,
    NodeTexture=super::node::NodeTextureElement,
    FocusRing=super::node::FocusRingElement,
    DashedOutline=super::node::DashedOutlineElement,
    UiText=super::text::UiTextElement,
    BackdropBlur=super::effects::backdrop_blur::BackdropBlurElement,
    Shadow=super::effects::shadow::ShadowElement,
    Closing=smithay::backend::renderer::element::texture::TextureRenderElement<
        smithay::backend::renderer::gles::GlesTexture
    >,
    ScreenshotPreview=smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement<GlesRenderer>,
    CaptureOverlay=super::overlays::capture::CaptureOverlayElement,
    SourceChooser=super::overlays::source_chooser::SourceChooserElement,
    Border=SolidColorRenderElement,
    DisplayScaled=super::display_scale::DisplayScaledElement,
    Layer=WaylandSurfaceRenderElement<GlesRenderer>,
}

pub fn build(
    renderer: &mut GlesRenderer,
    output: &Output,
    primary_output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    request: RenderRequest<'_>,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let scale = output.current_scale().fractional_scale();
    let locked = request.desktop.session_lock.active();
    request.resources.ui_text.set_display_scale(scale);
    request.resources.backdrop_blur_renderer.set_display_scale(
        &output.name(),
        scale,
        crate::render::output_physical_size(output),
    );
    let elements = build_logical(renderer, output, primary_output, output_geometry, request)?;
    Ok(elements
        .into_iter()
        .map(|element| super::display_scale::element(element, scale, locked))
        .collect())
}

fn build_logical(
    renderer: &mut GlesRenderer,
    output: &Output,
    primary_output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    request: RenderRequest<'_>,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    request.resources.node_renderer.poll_icons(renderer);
    request
        .resources
        .window_shaders
        .ensure(renderer, request.visuals.background_base);
    request
        .resources
        .backdrop_blur_renderer
        .begin_scene(&output.name());
    request.resources.node_renderer.begin_scene(&output.name());
    request
        .resources
        .cluster_renderer
        .begin_scene(&output.name());
    request.resources.pin_renderer.begin_scene(&output.name());
    request.resources.ui_text.begin_scene();
    if request.desktop.session_lock.active() {
        let scale = Scale::from(output.current_scale().fractional_scale());
        let mut elements = request
            .desktop
            .session_lock
            .surfaces_for_output(output)
            .flat_map(|surface| {
                smithay::backend::renderer::element::surface::render_elements_from_surface_tree(
                    renderer,
                    surface.wl_surface(),
                    (0, 0),
                    scale,
                    1.0,
                    Kind::ScanoutCandidate,
                )
            })
            .map(SceneElement::Layer)
            .collect::<Vec<_>>();
        let backdrop_size = crate::render::output_physical_size(output);
        elements.push(SceneElement::Border(super::solid_color_element(
            request
                .resources
                .node_renderer
                .slot_id(&output.name(), super::node::NodeSlot::SessionLockBackdrop),
            Rectangle::from_size(backdrop_size),
            super::SESSION_LOCK_COLOR,
        )));
        prepend_pointer_elements(
            renderer,
            output,
            output_geometry,
            &request.cursor,
            request.frame.target_presentation_time,
            false,
            &mut elements,
        )?;
        return Ok(elements);
    }

    request
        .desktop
        .cameras
        .view(&output.name())
        .ok_or_else(|| format!("output {:?} has no camera", output.name()))?;
    if request.visuals.decorations.border_radius_px > 0 {
        // Compile both programs as one capability before any window needs
        // them. A bad driver/compiler therefore selects the coherent square
        // fallback at startup instead of halfway through a transition.
        request
            .resources
            .window_decoration_renderer
            .available(renderer);
    }
    let overlay_snapshot = request
        .overlays
        .overlays
        .snapshot(&output.name(), request.frame.target_presentation_time);
    let cluster_exclusive = crate::presentation::window::cluster_exclusive_presentation(
        request.desktop.clusters,
        request.desktop.nodes,
        request.desktop.fullscreen,
        request.desktop.maximize,
        output,
        output_geometry,
        request.frame.target_presentation_time,
    );

    // Cluster Composer owns only its initiating output. It replaces that
    // output's desktop with a stable mosaic while other outputs keep their
    // ordinary Field scene and frame-callback policy.
    if request
        .overlays
        .cluster_composer
        .replacement_output()
        .is_some_and(|name| name == output.name())
    {
        let mut elements = cluster_composer_elements(
            renderer,
            output,
            output_geometry,
            request.overlays.cluster_composer,
            request.desktop.clusters,
            request.overlays.apogee_config,
            request.overlays.overlay_config,
            request.visuals.decorations,
            request.visuals.font,
            request.desktop.space,
            request.desktop.cameras,
            request.desktop.nodes,
            request.resources.node_renderer,
            request.resources.cluster_renderer,
            request.resources.titlebar_renderer,
            request.resources.window_decoration_renderer,
            request.resources.ui_text,
            request.desktop.window_animations,
            request.desktop.fullscreen,
            request.desktop.maximize,
            request.resources.overlay_previews,
            request.frame.target_presentation_time,
        )?;
        append_overlay_shadows(
            renderer,
            output,
            "cluster-composer",
            request.visuals.shadows.overlay,
            request.resources.shadow_renderer,
            &mut elements,
        )?;
        elements.extend(
            super::layer_surface_elements(renderer, output, Layer::Background)
                .into_iter()
                .map(SceneElement::Layer),
        );
        append_background(
            renderer,
            request.resources.background_renderer,
            BackgroundSceneRequest {
                output,
                output_geometry,
                cameras: request.desktop.cameras,
                config: request.visuals.background,
                config_dir: request.visuals.background_base,
                now: request.frame.target_presentation_time,
            },
            &mut elements,
        );

        let mut naming = super::overlays::cluster_creation::elements(
            renderer,
            request.resources.cluster_creation_overlay,
            output,
            output_geometry,
            request.desktop.clusters.creation(),
            request.cursor.cursor_position,
            request
                .overlays
                .cluster_composer
                .session()
                .map_or(1.0, |session| {
                    session.naming_alpha(request.frame.target_presentation_time)
                }),
            request.overlays.overlay_config,
            request.visuals.decorations,
            request.resources.node_renderer,
            request.resources.ui_text,
        )?;
        append_compositor_overlay_blur(
            renderer,
            output,
            OverlayEffectStyle {
                output_size: output_geometry.size,
                identity: "cluster-creation",
                blur: request.visuals.blur,
                shadow: request.visuals.shadows.overlay,
            },
            request.resources.backdrop_blur_renderer,
            request.resources.shadow_renderer,
            &mut naming,
        )?;
        elements.splice(0..0, naming);

        let mut overlay_elements = super::overlays::shell::elements(
            renderer,
            output_geometry,
            overlay_snapshot,
            request.overlays.overlay_config,
            request.resources.node_renderer,
            request.resources.ui_text,
        )?;
        append_compositor_overlay_blur(
            renderer,
            output,
            OverlayEffectStyle {
                output_size: output_geometry.size,
                identity: "shell-overlay",
                blur: request.visuals.blur,
                shadow: request.visuals.shadows.overlay,
            },
            request.resources.backdrop_blur_renderer,
            request.resources.shadow_renderer,
            &mut overlay_elements,
        )?;
        elements.splice(0..0, overlay_elements);
        if request.visuals.debug.overlay_fps {
            let mut fps_elements = super::overlays::fps::elements(
                renderer,
                &output.name(),
                request.frame.target_presentation_time,
                request.resources.debug_fps_overlay,
                request.overlays.overlay_config,
                request.visuals.decorations,
                request.resources.node_renderer,
                request.resources.ui_text,
            )?;
            append_compositor_overlay_blur(
                renderer,
                output,
                OverlayEffectStyle {
                    output_size: output_geometry.size,
                    identity: "debug-fps",
                    blur: request.visuals.blur,
                    shadow: request.visuals.shadows.overlay,
                },
                request.resources.backdrop_blur_renderer,
                request.resources.shadow_renderer,
                &mut fps_elements,
            )?;
            elements.splice(0..0, fps_elements);
        }
        prepend_pointer_elements(
            renderer,
            output,
            output_geometry,
            &request.cursor,
            request.frame.target_presentation_time,
            true,
            &mut elements,
        )?;
        return Ok(elements);
    }

    // Apogee is a replacement scene, not a translucent layer over the live
    // desktop. Keep only its tiles and the wallpaper layer behind them; normal
    // windows, nodes, panels, and desktop overlays must not bleed through.
    if request.overlays.apogee.is_active() {
        let mut elements = apogee_elements(
            renderer,
            output,
            output_geometry,
            request.overlays.apogee,
            request.overlays.apogee_config,
            request.overlays.overlay_config,
            request.visuals.decorations,
            request.visuals.font,
            request.desktop.space,
            request.desktop.cameras,
            request.desktop.nodes,
            request.desktop.clusters,
            request.resources.node_renderer,
            request.resources.cluster_renderer,
            request.resources.titlebar_renderer,
            request.resources.window_decoration_renderer,
            request.resources.ui_text,
            request.desktop.window_animations,
            request.desktop.fullscreen,
            request.desktop.maximize,
            request.resources.overlay_previews,
            request.frame.target_presentation_time,
        )?;
        append_overlay_shadows(
            renderer,
            output,
            "apogee",
            request.visuals.shadows.overlay,
            request.resources.shadow_renderer,
            &mut elements,
        )?;
        elements.extend(
            super::layer_surface_elements(renderer, output, Layer::Background)
                .into_iter()
                .map(SceneElement::Layer),
        );
        append_background(
            renderer,
            request.resources.background_renderer,
            BackgroundSceneRequest {
                output,
                output_geometry,
                cameras: request.desktop.cameras,
                config: request.visuals.background,
                config_dir: request.visuals.background_base,
                now: request.frame.target_presentation_time,
            },
            &mut elements,
        );
        let mut overlay_elements = super::overlays::shell::elements(
            renderer,
            output_geometry,
            overlay_snapshot,
            request.overlays.overlay_config,
            request.resources.node_renderer,
            request.resources.ui_text,
        )?;
        append_compositor_overlay_blur(
            renderer,
            output,
            OverlayEffectStyle {
                output_size: output_geometry.size,
                identity: "shell-overlay",
                blur: request.visuals.blur,
                shadow: request.visuals.shadows.overlay,
            },
            request.resources.backdrop_blur_renderer,
            request.resources.shadow_renderer,
            &mut overlay_elements,
        )?;
        elements.splice(0..0, overlay_elements);
        if request.visuals.debug.overlay_fps {
            let mut fps_elements = super::overlays::fps::elements(
                renderer,
                &output.name(),
                request.frame.target_presentation_time,
                request.resources.debug_fps_overlay,
                request.overlays.overlay_config,
                request.visuals.decorations,
                request.resources.node_renderer,
                request.resources.ui_text,
            )?;
            append_compositor_overlay_blur(
                renderer,
                output,
                OverlayEffectStyle {
                    output_size: output_geometry.size,
                    identity: "debug-fps",
                    blur: request.visuals.blur,
                    shadow: request.visuals.shadows.overlay,
                },
                request.resources.backdrop_blur_renderer,
                request.resources.shadow_renderer,
                &mut fps_elements,
            )?;
            elements.splice(0..0, fps_elements);
        }
        prepend_pointer_elements(
            renderer,
            output,
            output_geometry,
            &request.cursor,
            request.frame.target_presentation_time,
            true,
            &mut elements,
        )?;
        return Ok(elements);
    }

    let mut elements = capture_overlay_elements(
        renderer,
        output,
        output_geometry,
        request.overlays.capture_overlay,
        request.resources.node_renderer,
        request.overlays.overlay_config,
        request.visuals.decorations,
    )?;
    let mut bearings = super::overlays::bearings::elements(
        renderer,
        output,
        output_geometry,
        request.overlays.bearings,
        request.desktop.nodes,
        request.desktop.clusters,
        request.desktop.fullscreen,
        request.desktop.cameras,
        request.visuals.blur,
        request.resources.backdrop_blur_renderer,
        request.resources.node_renderer,
        request.resources.cluster_renderer,
        request.resources.ui_text,
        request.overlays.overlay_config,
        request.visuals.decorations,
        request.visuals.pins,
        request.resources.pin_renderer,
    )?;
    append_overlay_shadows(
        renderer,
        output,
        "bearings",
        request.visuals.shadows.overlay,
        request.resources.shadow_renderer,
        &mut bearings,
    )?;
    elements.extend(bearings);
    let mut cluster_creation = super::overlays::cluster_creation::elements(
        renderer,
        request.resources.cluster_creation_overlay,
        output,
        output_geometry,
        request.desktop.clusters.creation(),
        request.cursor.cursor_position,
        1.0,
        request.overlays.overlay_config,
        request.visuals.decorations,
        request.resources.node_renderer,
        request.resources.ui_text,
    )?;
    append_compositor_overlay_blur(
        renderer,
        output,
        OverlayEffectStyle {
            output_size: output_geometry.size,
            identity: "cluster-creation",
            blur: request.visuals.blur,
            shadow: request.visuals.shadows.overlay,
        },
        request.resources.backdrop_blur_renderer,
        request.resources.shadow_renderer,
        &mut cluster_creation,
    )?;
    elements.extend(cluster_creation);
    elements.extend(layer_surface_scene_elements(
        renderer,
        output,
        output_geometry,
        Layer::Overlay,
        request.desktop.layer_rules,
        request.visuals.blur,
        request.resources.backdrop_blur_renderer,
    )?);
    let mut focus_cycle = focus_cycle_elements(
        renderer,
        output_geometry,
        FocusCycleRenderContext {
            state: request.overlays.focus_cycle,
            nodes: request.desktop.nodes,
            overlay_config: request.overlays.overlay_config,
            decorations: request.visuals.decorations,
            font: request.visuals.font,
            fullscreen: request.desktop.fullscreen,
            maximize: request.desktop.maximize,
            now: request.frame.target_presentation_time,
        },
        request.resources.overlay_previews,
        crate::render::overlays::preview::OverlayPreviewRenderers {
            titlebar: request.resources.titlebar_renderer,
            decoration: request.resources.window_decoration_renderer,
            node: request.resources.node_renderer,
            text: request.resources.ui_text,
        },
    )?;
    append_compositor_overlay_blur(
        renderer,
        output,
        OverlayEffectStyle {
            output_size: output_geometry.size,
            identity: "focus-cycle",
            blur: request.visuals.blur,
            shadow: request.visuals.shadows.overlay,
        },
        request.resources.backdrop_blur_renderer,
        request.resources.shadow_renderer,
        &mut focus_cycle,
    )?;
    elements.extend(focus_cycle);
    // Native XDG and X11 override-redirect popups use a desktop popup plane.
    // It sits in front of panels (`Layer::Top`) so menus are not clipped by a
    // status bar, while overlay surfaces and compositor UI remain authoritative.
    let popup_foreground_index = elements.len();
    if !request.desktop.fullscreen.covers_top_matching(
        request.desktop.focused,
        output,
        request.frame.target_presentation_time,
        |surface| {
            crate::presentation::surface_workspace_is_active(
                request.desktop.clusters,
                request.desktop.nodes,
                surface,
                &output.name(),
                request.frame.target_presentation_time,
            )
        },
    ) {
        elements.extend(layer_surface_scene_elements(
            renderer,
            output,
            output_geometry,
            Layer::Top,
            request.desktop.layer_rules,
            request.visuals.blur,
            request.resources.backdrop_blur_renderer,
        )?);
    }
    // Cluster-exclusive windows live above every desktop-owned element but
    // below the top layer retained by field maximize. The insertion happens
    // after the complete desktop has been built so the target can be removed
    // from its ordinary cluster stack without duplicating it.
    let desktop_foreground_index = elements.len();

    let mut hover_preview = hover_preview_elements(
        renderer,
        output,
        output_geometry,
        request.desktop.nodes,
        request.desktop.cameras,
        request.resources.overlay_previews,
        request.resources.node_renderer,
        request.resources.ui_text,
        request.resources.window_decoration_renderer,
        request.resources.titlebar_renderer,
        request.overlays.overlay_config,
        request.visuals.decorations,
        request.visuals.font,
        request.desktop.fullscreen,
        request.frame.target_presentation_time,
    )?;
    append_compositor_overlay_blur(
        renderer,
        output,
        OverlayEffectStyle {
            output_size: output_geometry.size,
            identity: "node-hover-preview",
            blur: request.visuals.blur,
            shadow: request.visuals.shadows.overlay,
        },
        request.resources.backdrop_blur_renderer,
        request.resources.shadow_renderer,
        &mut hover_preview,
    )?;
    elements.extend(hover_preview);

    let node_scene = node_elements(
        renderer,
        request.resources.node_renderer,
        request.resources.ui_text,
        NodeElementContext {
            output,
            output_geometry,
            nodes: request.desktop.nodes,
            node_grab_active: request.desktop.node_grab_active,
            cameras: request.desktop.cameras,
            decorations: request.visuals.decorations,
            pins: request.visuals.pins,
            overlays: request.overlays.overlay_config,
            pin_renderer: request.resources.pin_renderer,
            debug: request.visuals.debug,
            shadow_config: request.visuals.shadows.node,
            shadow_renderer: request.resources.shadow_renderer,
            now: request.frame.target_presentation_time,
        },
    )?;
    elements.extend(node_scene.overlay);

    let mut stack = request
        .resources
        .window_close_animations
        .renders_for_output(
            renderer,
            request.resources.window_shaders,
            output,
            output_geometry,
            request.desktop.cameras,
            request.frame.target_presentation_time,
        )
        .into_iter()
        .map(|closing| -> Result<StackGroup, Box<dyn Error>> {
            let mut elements = Vec::new();
            let shader = closing.shader;
            let shader_active = shader.is_some();
            let shadow_alpha = closing.texture.alpha();
            let border_width = closing.border.map(|border| border.width).unwrap_or(0);
            let rounded = shader.is_none()
                && closing.content_radius > 0.0
                && request
                    .resources
                    .window_decoration_renderer
                    .available(renderer);
            if let Some(border) = closing.border {
                if rounded
                    && let Some(border) =
                        request.resources.window_decoration_renderer.border_element(
                            renderer,
                            closing.border_id.clone(),
                            closing.destination,
                            border.width,
                            closing.content_radius,
                            border.color,
                            1.0,
                        )
                {
                    elements.push(SceneElement::WindowBorder(border));
                } else {
                    elements.extend(
                        super::border_strips(
                            std::array::from_fn(|index| closing.border_id.namespaced(100 + index)),
                            closing.destination,
                            border.width,
                            border.color,
                        )
                        .into_iter()
                        .map(SceneElement::Border),
                    );
                }
            }
            if let Some(shader) = shader {
                elements.push(SceneElement::WindowShader(shader));
            } else if rounded {
                let texture = request
                    .resources
                    .window_decoration_renderer
                    .texture_element(
                        renderer,
                        closing.texture,
                        closing.source_texture,
                        closing.destination,
                        closing.content_radius,
                    )
                    .expect("rounded resources were checked above");
                elements.push(SceneElement::RoundedTexture(texture));
            } else {
                elements.push(SceneElement::Closing(closing.texture));
            }
            let caster = Rectangle::new(
                (
                    closing.destination.loc.x - border_width,
                    closing.destination.loc.y - border_width,
                )
                    .into(),
                (
                    closing.destination.size.w + border_width * 2,
                    closing.destination.size.h + border_width * 2,
                )
                    .into(),
            );
            // The rectangular caster cannot follow an arbitrary shader's
            // fragment silhouette. Let the shader own the complete close
            // visual instead of leaving a detached shadow on the trajectory.
            if !shader_active
                && let Some(shadow) = request.resources.shadow_renderer.element(
                    renderer,
                    format!("{}:closing:{}", output.name(), closing.order),
                    caster,
                    if rounded {
                        closing.content_radius + border_width as f32
                    } else {
                        0.0
                    },
                    shadow_alpha,
                    request.visuals.shadows.window,
                )?
            {
                elements.push(SceneElement::Shadow(shadow));
            }
            Ok(StackGroup {
                window: None,
                stack_index: closing.stack_index,
                order: closing.order,
                elements,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    stack.extend(node_scene.groups);
    let cluster_scene = cluster_elements(
        renderer,
        request.resources.cluster_renderer,
        ClusterElementContext {
            output,
            primary_output,
            output_geometry,
            space: request.desktop.space,
            clusters: request.desktop.clusters,
            nodes: request.desktop.nodes,
            cameras: request.desktop.cameras,
            window_animations: request.desktop.window_animations,
            fullscreen: request.desktop.fullscreen,
            maximize: request.desktop.maximize,
            decorations: request.visuals.decorations,
            font: request.visuals.font,
            pins: request.visuals.pins,
            overlays: request.overlays.overlay_config,
            pin_renderer: request.resources.pin_renderer,
            shadow_config: request.visuals.shadows.node,
            shadow_renderer: request.resources.shadow_renderer,
            node_grab_active: request.desktop.node_grab_active,
            now: request.frame.target_presentation_time,
            node_renderer: request.resources.node_renderer,
            ui_text: request.resources.ui_text,
        },
    )?;
    let cluster_bloom = super::overlays::cluster_bloom::elements(
        renderer,
        super::overlays::cluster_bloom::BloomElementContext {
            output,
            output_geometry,
            clusters: request.desktop.clusters,
            nodes: request.desktop.nodes,
            cameras: request.desktop.cameras,
            now: request.frame.target_presentation_time,
            cluster_renderer: request.resources.cluster_renderer,
            node_renderer: request.resources.node_renderer,
            ui_text: request.resources.ui_text,
        },
    )?;
    elements.extend(cluster_bloom);
    let cluster_overflow = super::overlays::cluster_overflow::elements(
        renderer,
        super::overlays::cluster_overflow::OverflowElementContext {
            output,
            clusters: request.desktop.clusters,
            nodes: request.desktop.nodes,
            config: request.overlays.overlay_config,
            now: request.frame.target_presentation_time,
            node_renderer: request.resources.node_renderer,
            ui_text: request.resources.ui_text,
        },
    )?;
    elements.extend(cluster_overflow);
    stack.extend(cluster_scene);
    let context = LiveWindowContext {
        space: request.desktop.space,
        output,
        output_geometry,
        cameras: request.desktop.cameras,
        clusters: request.desktop.clusters,
        nodes: request.desktop.nodes,
        target_presentation_time: request.frame.target_presentation_time,
        focused: request.desktop.focused,
        decorations: request.visuals.decorations,
        pins: request.visuals.pins,
        overlays: request.overlays.overlay_config,
        font: request.visuals.font,
        blur: request.visuals.blur,
        shadow_config: request.visuals.shadows.window,
        window_animations: request.desktop.window_animations,
        fullscreen: request.desktop.fullscreen,
        maximize: request.desktop.maximize,
        window_rules: request.desktop.window_rules,
        cluster_presentation_override: None,
        instance_identity: None,
        titlebar_hovered: request.desktop.titlebar_hovered,
        titlebar_pressed: request.desktop.titlebar_pressed,
    };
    let mut live_windows = Vec::new();
    let mut exclusive_windows = Vec::new();
    for (stack_index, window) in request.desktop.space.elements().enumerate() {
        if !crate::wayland::window_is_on_output(window, output, primary_output) {
            continue;
        }
        if window.wl_surface().is_some_and(|surface| {
            let animations = &request.resources.window_close_animations;
            animations.has_pending(surface.as_ref()) || animations.is_active(surface.as_ref())
        }) {
            continue;
        }
        let window_scene = live_window_elements(
            renderer,
            window,
            context,
            LiveWindowRenderers {
                arrange_textures: request.resources.arrange_textures,
                fullscreen_textures: request.resources.fullscreen_textures,
                backdrop_blur: request.resources.backdrop_blur_renderer,
                shadow: request.resources.shadow_renderer,
                decoration: request.resources.window_decoration_renderer,
                titlebar: request.resources.titlebar_renderer,
                node: request.resources.node_renderer,
                text: request.resources.ui_text,
                pin: request.resources.pin_renderer,
                window_shaders: request.resources.window_shaders,
            },
        )?;
        if window_scene.cluster_exclusive {
            exclusive_windows.push((stack_index, window_scene));
        } else if !window_scene.elements.is_empty() || !window_scene.popup_elements.is_empty() {
            live_windows.push((stack_index, window_scene));
        }
        let extra_presentation = window
            .wl_surface()
            .and_then(|surface| request.desktop.nodes.id_for_surface(surface.as_ref()))
            .and_then(|id| {
                request.desktop.clusters.extra_window_presentation(
                    id,
                    &output.name(),
                    request.frame.target_presentation_time,
                )
            });
        if let Some(extra_presentation) = extra_presentation {
            let extra_scene = live_window_elements(
                renderer,
                window,
                LiveWindowContext {
                    cluster_presentation_override: Some(extra_presentation),
                    instance_identity: Some("stack-cycle-extra"),
                    ..context
                },
                LiveWindowRenderers {
                    arrange_textures: request.resources.arrange_textures,
                    fullscreen_textures: request.resources.fullscreen_textures,
                    backdrop_blur: request.resources.backdrop_blur_renderer,
                    shadow: request.resources.shadow_renderer,
                    decoration: request.resources.window_decoration_renderer,
                    titlebar: request.resources.titlebar_renderer,
                    node: request.resources.node_renderer,
                    text: request.resources.ui_text,
                    pin: request.resources.pin_renderer,
                    window_shaders: request.resources.window_shaders,
                },
            )?;
            if !extra_scene.elements.is_empty() || !extra_scene.popup_elements.is_empty() {
                live_windows.push((stack_index, extra_scene));
            }
        }
    }
    let exclusive_anchor = exclusive_windows
        .iter()
        .map(|(stack_index, _)| *stack_index)
        .max();
    let (mut floating_foreground, mut live_windows) = match exclusive_anchor {
        Some(exclusive_anchor) => live_windows.into_iter().partition(|(stack_index, scene)| {
            cluster_float_is_above_exclusive(scene.cluster_floating, *stack_index, exclusive_anchor)
        }),
        None => (Vec::new(), live_windows),
    };
    // A cluster workspace is one coherent stack. Preserve its position
    // relative to non-cluster windows at the topmost member's existing Space
    // slot, then use the layout's explicit depth for member overlap.
    let cluster_anchor = live_windows
        .iter()
        .filter_map(|(stack_index, scene)| scene.cluster_depth.map(|_| *stack_index))
        .max();
    let mut popup_stack = Vec::new();
    for (stack_index, window_scene) in &mut live_windows {
        if !window_scene.popup_elements.is_empty() {
            popup_stack.push(StackGroup {
                window: Some(window_scene.window.clone()),
                stack_index: window_scene
                    .cluster_depth
                    .and(cluster_anchor)
                    .unwrap_or(*stack_index),
                order: window_scene
                    .cluster_depth
                    .map_or(u64::MAX, |depth| depth as u64),
                elements: std::mem::take(&mut window_scene.popup_elements),
            });
        }
    }
    for (stack_index, window_scene) in live_windows {
        stack.push(StackGroup {
            window: Some(window_scene.window),
            stack_index: window_scene
                .cluster_depth
                .and(cluster_anchor)
                .unwrap_or(stack_index),
            order: window_scene
                .cluster_depth
                .map_or(u64::MAX, |depth| depth as u64),
            elements: window_scene.elements,
        });
    }
    sort_stack_groups(&mut stack);
    crate::window::stacking::sort_above_parents(request.desktop.space, &mut stack, |group| {
        group.window.as_ref()
    });
    elements.extend(stack.into_iter().rev().flat_map(|group| group.elements));

    if let Some(exclusive) = cluster_exclusive {
        // Popups owned by the selected X11 member inherit the exclusive flag.
        // Keep every such scene and retain normal front-to-back Space order.
        for (stack_index, scene) in floating_foreground
            .iter_mut()
            .chain(exclusive_windows.iter_mut())
        {
            if !scene.popup_elements.is_empty() {
                popup_stack.push(StackGroup {
                    window: Some(scene.window.clone()),
                    stack_index: *stack_index,
                    order: scene.cluster_depth.map_or(u64::MAX, |depth| depth as u64),
                    elements: std::mem::take(&mut scene.popup_elements),
                });
            }
        }
        let mut foreground = floating_foreground
            .into_iter()
            .rev()
            .flat_map(|(_, scene)| scene.elements)
            .collect::<Vec<_>>();
        foreground.extend(
            exclusive_windows
                .into_iter()
                .rev()
                .flat_map(|(_, scene)| scene.elements),
        );
        if exclusive.progress > 0.0 {
            foreground.push(SceneElement::Border(super::solid_color_element(
                request.resources.node_renderer.slot_id(
                    &output.name(),
                    super::node::NodeSlot::ClusterExclusiveBackdrop,
                ),
                Rectangle::from_size(output_geometry.size.to_physical(1)),
                smithay::backend::renderer::Color32F::new(
                    0.0,
                    0.0,
                    0.0,
                    exclusive.progress.clamp(0.0, 1.0),
                ),
            )));
        }
        elements.splice(
            desktop_foreground_index..desktop_foreground_index,
            foreground,
        );
    }

    sort_stack_groups(&mut popup_stack);
    crate::window::stacking::sort_above_parents(request.desktop.space, &mut popup_stack, |group| {
        group.window.as_ref()
    });
    elements.splice(
        popup_foreground_index..popup_foreground_index,
        popup_stack
            .into_iter()
            .rev()
            .flat_map(|group| group.elements),
    );

    elements.extend(layer_surface_scene_elements(
        renderer,
        output,
        output_geometry,
        Layer::Bottom,
        request.desktop.layer_rules,
        request.visuals.blur,
        request.resources.backdrop_blur_renderer,
    )?);
    elements.extend(layer_surface_scene_elements(
        renderer,
        output,
        output_geometry,
        Layer::Background,
        request.desktop.layer_rules,
        request.visuals.blur,
        request.resources.backdrop_blur_renderer,
    )?);
    append_background(
        renderer,
        request.resources.background_renderer,
        BackgroundSceneRequest {
            output,
            output_geometry,
            cameras: request.desktop.cameras,
            config: request.visuals.background,
            config_dir: request.visuals.background_base,
            now: request.frame.target_presentation_time,
        },
        &mut elements,
    );

    let mut overlay_elements = super::overlays::shell::elements(
        renderer,
        output_geometry,
        overlay_snapshot,
        request.overlays.overlay_config,
        request.resources.node_renderer,
        request.resources.ui_text,
    )?;
    append_compositor_overlay_blur(
        renderer,
        output,
        OverlayEffectStyle {
            output_size: output_geometry.size,
            identity: "shell-overlay",
            blur: request.visuals.blur,
            shadow: request.visuals.shadows.overlay,
        },
        request.resources.backdrop_blur_renderer,
        request.resources.shadow_renderer,
        &mut overlay_elements,
    )?;
    elements.splice(0..0, overlay_elements);

    if request.visuals.debug.overlay_fps {
        let mut fps_elements = super::overlays::fps::elements(
            renderer,
            &output.name(),
            request.frame.target_presentation_time,
            request.resources.debug_fps_overlay,
            request.overlays.overlay_config,
            request.visuals.decorations,
            request.resources.node_renderer,
            request.resources.ui_text,
        )?;
        append_compositor_overlay_blur(
            renderer,
            output,
            OverlayEffectStyle {
                output_size: output_geometry.size,
                identity: "debug-fps",
                blur: request.visuals.blur,
                shadow: request.visuals.shadows.overlay,
            },
            request.resources.backdrop_blur_renderer,
            request.resources.shadow_renderer,
            &mut fps_elements,
        )?;
        elements.splice(0..0, fps_elements);
    }

    if request.overlays.cluster_composer.reveal_on(&output.name())
        && let Some(session) = request.overlays.cluster_composer.session()
    {
        let alpha = session.reveal_alpha(request.frame.target_presentation_time);
        let veil = SceneElement::Border(crate::render::solid_color_element(
            request
                .resources
                .node_renderer
                .active_slot_id(crate::render::node::NodeSlot::ClusterComposerBackdrop),
            Rectangle::<i32, Physical>::from_size(output_geometry.size.to_physical(1)),
            overview::apogee_backdrop_color(request.overlays.apogee_config.background_dim, alpha),
        ));
        elements.insert(0, veil);
        if let Some(prepared) = session.prepared() {
            let core = cluster_composer::prepared_core_elements(
                renderer,
                request.resources.cluster_renderer,
                prepared,
                output,
                output_geometry,
                request.desktop.cameras,
                request.desktop.clusters.config(),
                request.desktop.nodes.config,
                1.0,
            )?;
            // The veil reveals the restored Field, but the endpoint core is
            // continuous UI and must remain fully legible above it. The real
            // committed core already occupies the same geometry underneath.
            elements.splice(0..0, core);
        }
    }

    prepend_pointer_elements(
        renderer,
        output,
        output_geometry,
        &request.cursor,
        request.frame.target_presentation_time,
        true,
        &mut elements,
    )?;

    Ok(elements)
}

fn prepend_pointer_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    cursor_context: &super::CursorContext<'_>,
    time: std::time::Duration,
    show_dnd_icon: bool,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    if show_dnd_icon {
        let dnd = crate::wayland::dnd::elements(
            renderer,
            cursor_context.dnd_icon,
            output,
            output_geometry,
            cursor_context.cursor_position,
        );
        elements.splice(0..0, dnd.into_iter().map(SceneElement::Layer));
    }
    if cursor_context.show_cursor {
        let cursor = crate::cursor::render::elements(
            renderer,
            cursor_context.cursor,
            output,
            output_geometry,
            cursor_context.cursor_position,
            time,
            cursor_context.cursor_override,
        )?;
        // Element lists are front-to-back, so cursor surface trees belong
        // immediately in front of the drag icon and every other element.
        elements.splice(0..0, cursor.into_iter().map(SceneElement::Cursor));
    }
    Ok(())
}

fn cluster_float_is_above_exclusive(
    cluster_floating: bool,
    stack_index: usize,
    exclusive_anchor: usize,
) -> bool {
    cluster_floating && stack_index > exclusive_anchor
}

struct BackgroundSceneRequest<'a> {
    output: &'a Output,
    output_geometry: Rectangle<i32, Logical>,
    cameras: &'a crate::presentation::camera::OutputCameras,
    config: &'a halley_config::Background,
    config_dir: Option<&'a std::path::Path>,
    now: std::time::Duration,
}

fn append_background(
    renderer: &mut GlesRenderer,
    background_renderer: &mut super::background::BackgroundRenderer,
    request: BackgroundSceneRequest<'_>,
    elements: &mut Vec<SceneElement>,
) {
    let output_name = request.output.name();
    let Some(camera) = request.cameras.get(&output_name) else {
        return;
    };
    let center = crate::presentation::camera::global_center(
        smithay::utils::Point::from((camera.center.x, camera.center.y)),
        request.output_geometry,
    );
    if let Some(background) = background_renderer.element(
        renderer,
        super::background::BackgroundRequest {
            output_name: &output_name,
            output_size: request.output_geometry.size.to_physical(1),
            camera_center: center,
            camera_size: (camera.view_size.x, camera.view_size.y),
            now: request.now,
            config: request.config,
            config_dir: request.config_dir,
        },
    ) {
        // Scene lists are front-to-back. Appending after the protocol
        // background layer makes compositor backgrounds the absolute back.
        elements.push(SceneElement::Background(background));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::{Physical, Rectangle};

    #[test]
    fn raised_cluster_float_crosses_the_exclusive_boundary() {
        assert!(!cluster_float_is_above_exclusive(true, 3, 4));
        assert!(!cluster_float_is_above_exclusive(false, 5, 4));
        assert!(cluster_float_is_above_exclusive(true, 5, 4));
    }

    #[test]
    fn hover_preview_aspect_fit_centers_wide_and_tall_windows() {
        let bounds = Rectangle::<i32, Physical>::new((10, 20).into(), (200, 160).into());
        assert_eq!(
            aspect_fit_rect(bounds, 1_600, 900),
            Rectangle::new((10, 43).into(), (200, 113).into())
        );
        assert_eq!(
            aspect_fit_rect(bounds, 900, 1_600),
            Rectangle::new((65, 20).into(), (90, 160).into())
        );
    }

    #[test]
    fn focus_cycle_chrome_tracks_a_centered_thin_preview() {
        let slot = Rectangle::<i32, Physical>::new((40, 20).into(), (300, 240).into());
        let available = Rectangle::new((46, 26).into(), (288, 228).into());
        let body = aspect_fit_rect(available, 600, 1_600);
        let chrome = preview_card_rect(body, 4.0);

        assert_eq!(body.loc.x + body.size.w / 2, slot.loc.x + slot.size.w / 2);
        assert_eq!(chrome.loc.x, body.loc.x - 4);
        assert_eq!(chrome.size.w, body.size.w + 8);
        assert!(chrome.size.w < slot.size.w);
    }

    #[test]
    fn overview_preview_texture_meets_the_card_border() {
        let body = Rectangle::<i32, Physical>::new((100, 80).into(), (480, 240).into());
        let card = preview_card_rect(body, 3.0);

        assert_eq!(card, Rectangle::new((97, 77).into(), (486, 246).into()));
        assert_eq!(card.loc.x + 3, body.loc.x);
        assert_eq!(card.loc.y + 3, body.loc.y);
        assert_eq!(card.loc.x + card.size.w - 3, body.loc.x + body.size.w);
        assert_eq!(card.loc.y + card.size.h - 3, body.loc.y + body.size.h);
    }

    #[test]
    fn preview_rounding_uses_the_overlay_radius_in_output_pixels() {
        assert_eq!(preview_content_radius(14.0), 14.0);
        assert_eq!(preview_content_radius(0.0), 0.0);
        assert_eq!(preview_content_radius(-4.0), 0.0);
    }

    #[test]
    fn picker_outline_and_grips_describe_the_whole_selection() {
        let output = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
        let selection = Rectangle::<i32, Logical>::new((320, 180).into(), (1280, 720).into());

        let layout = picker_layout(output, selection).expect("selection is on this output");
        assert_eq!(
            layout.outline,
            Rectangle::new((320, 180).into(), (1280, 720).into())
        );
        assert_eq!(layout.handles.len(), 4);
        assert_eq!(
            layout.handles[0],
            Rectangle::new((314, 174).into(), (12, 12).into())
        );
        assert_eq!(
            layout.handles[3],
            Rectangle::new((1594, 894).into(), (12, 12).into())
        );
    }

    /// The reported bug: a selection spanning two outputs used to be clipped
    /// per output, so each monitor drew its own box with its own four grips.
    /// The outline and grips must instead stay in one global selection, with
    /// only the dimmed surround computed per output.
    #[test]
    fn a_selection_spanning_outputs_keeps_one_outline_and_one_grip_set() {
        let main = Rectangle::<i32, Logical>::new((0, 0).into(), (2560, 1440).into());
        let secondary = Rectangle::<i32, Logical>::new((2560, 0).into(), (1920, 1200).into());
        let selection = Rectangle::<i32, Logical>::new((2000, 100).into(), (1500, 800).into());

        let on_main = picker_layout(main, selection).expect("overlaps main");
        let on_secondary = picker_layout(secondary, selection).expect("overlaps secondary");

        // Same region, expressed in each output's local coordinates.
        assert_eq!(
            on_main.outline,
            Rectangle::new((2000, 100).into(), (1500, 800).into())
        );
        assert_eq!(
            on_secondary.outline,
            Rectangle::new((-560, 100).into(), (1500, 800).into())
        );
        assert_eq!(
            on_main.outline.loc.x - 2560,
            on_secondary.outline.loc.x,
            "the two outlines must be the same rectangle in global space"
        );

        // The four grips sit at the selection's true corners, so each output
        // renders only the ones that actually land on it.
        let visible = |layout: &super::capture_ui::PickerLayout,
                       bounds: Rectangle<i32, Logical>| {
            let local = Rectangle::<i32, Physical>::from_size(bounds.size.to_physical(1));
            layout
                .handles
                .iter()
                .filter(|handle| local.overlaps(**handle))
                .count()
        };
        assert_eq!(visible(&on_main, main), 2);
        assert_eq!(visible(&on_secondary, secondary), 2);
    }

    #[test]
    fn the_dimmed_surround_is_clipped_to_each_output() {
        let secondary = Rectangle::<i32, Logical>::new((2560, 0).into(), (1920, 1200).into());
        let selection = Rectangle::<i32, Logical>::new((2000, 100).into(), (1500, 800).into());

        let layout = picker_layout(secondary, selection).expect("overlaps secondary");
        let local = Rectangle::<i32, Physical>::from_size(secondary.size.to_physical(1));

        assert!(
            layout.dim.iter().all(|rect| local.contains_rect(*rect)),
            "dim rects must stay on their own output: {:?}",
            layout.dim
        );
    }

    #[test]
    fn secondary_window_geometry_becomes_output_local() {
        let secondary = Rectangle::<i32, Logical>::new((2560, 0).into(), (1920, 1200).into());
        let camera_center =
            crate::presentation::camera::global_center(Point::from((1060.0, 550.0)), secondary);
        let world_rect = Rectangle::<i32, Physical>::new((3520, 600).into(), (200, 100).into());

        assert_eq!(
            super::super::camera_rect(
                world_rect,
                camera_center,
                secondary.size.to_physical(1),
                0.5,
            ),
            Rectangle::new((910, 625).into(), (100, 50).into())
        );
    }

    #[test]
    fn fullscreen_rect_interpolates_position_and_size() {
        let windowed = Rectangle::new((100, 50).into(), (800, 600).into());
        let start = crate::wayland::fullscreen::FullscreenPresentation {
            progress: 0.0,
            transition_completion: 0.0,
            windowed_geometry: None,
            windowed_output_rect: None,
            fullscreen_size: (1920, 1080).into(),
        };
        let end = crate::wayland::fullscreen::FullscreenPresentation {
            progress: 1.0,
            ..start
        };
        let middle = crate::wayland::fullscreen::FullscreenPresentation {
            progress: 0.5,
            ..start
        };

        assert_eq!(start.client_rect(windowed, (1920, 1080).into()), windowed);
        assert_eq!(
            end.client_rect(windowed, (1920, 1080).into()),
            Rectangle::new((0, 0).into(), (1920, 1080).into())
        );
        assert_eq!(
            middle.client_rect(windowed, (1920, 1080).into()),
            Rectangle::new((50, 25).into(), (1360, 840).into())
        );
    }

    #[test]
    fn surface_rects_map_into_animated_client_bounds() {
        assert_eq!(
            crate::animation::map_rect(
                Rectangle::new((120, 70).into(), (200, 100).into()),
                Rectangle::new((100, 50).into(), (800, 600).into()),
                Rectangle::new((0, 0).into(), (1600, 1200).into()),
            ),
            Rectangle::new((40, 40).into(), (400, 200).into())
        );
    }

    #[test]
    fn apogee_caption_overlays_the_preview_instead_of_growing_a_footer() {
        let body = Rectangle::<i32, Physical>::new((100, 80).into(), (480, 240).into());
        let caption = apogee_caption_rect(body).expect("large preview has a caption");

        assert_eq!(caption, Rectangle::new((108, 281).into(), (464, 31).into()));
        assert!(body.contains_rect(caption));
        assert_eq!(preview_card_rect(body, 4.0).size, (488, 248).into());
    }

    #[test]
    fn apogee_caption_waits_until_node_transition_is_large_enough() {
        let node_sized = Rectangle::<i32, Physical>::new((100, 80).into(), (64, 64).into());
        assert_eq!(apogee_caption_rect(node_sized), None);
    }

    #[test]
    fn apogee_transition_never_fades_the_handoff_preview() {
        for progress in [0.0, 0.0005, 0.5, 1.0] {
            let visuals = apogee_transition_visuals(progress);
            assert_eq!(visuals.preview_alpha, 1.0);
            assert_eq!(visuals.overlay_alpha, progress);
        }
        assert_eq!(apogee_transition_visuals(0.0).chrome_alpha, 0.0);
        assert_eq!(apogee_transition_visuals(1.0).chrome_alpha, 1.0);
    }

    #[test]
    fn apogee_transition_preserves_source_stack_and_promotes_selection() {
        let back = crate::shell::apogee::Tile {
            id: halley_core::field::NodeId::new(1),
            kind: crate::shell::apogee::TileKind::Window,
            output: "DP-1".into(),
            target: Rectangle::new((0, 0).into(), (100, 100).into()),
            source_stack_index: 0,
            source_stack_order: u64::MAX,
        };
        let middle = crate::shell::apogee::Tile {
            id: halley_core::field::NodeId::new(2),
            kind: crate::shell::apogee::TileKind::Window,
            output: "DP-1".into(),
            target: Rectangle::new((100, 0).into(), (100, 100).into()),
            source_stack_index: 1,
            source_stack_order: 0,
        };
        let front = crate::shell::apogee::Tile {
            id: halley_core::field::NodeId::new(3),
            kind: crate::shell::apogee::TileKind::Window,
            output: "DP-1".into(),
            target: Rectangle::new((200, 0).into(), (100, 100).into()),
            source_stack_index: 2,
            source_stack_order: u64::MAX,
        };
        let mut tiles = vec![&middle, &front, &back];

        sort_apogee_tiles(&mut tiles, None);
        assert_eq!(
            tiles.iter().map(|tile| tile.id).collect::<Vec<_>>(),
            vec![front.id, middle.id, back.id]
        );

        sort_apogee_tiles(&mut tiles, Some(middle.id));
        assert_eq!(
            tiles.iter().map(|tile| tile.id).collect::<Vec<_>>(),
            vec![middle.id, front.id, back.id]
        );
    }

    #[test]
    fn collapsed_node_keeps_front_middle_and_back_stack_depth() {
        let presented = |collapsed_index: usize, remaining: &[(usize, u64)]| {
            let mut groups = remaining
                .iter()
                .copied()
                .map(|(stack_index, order)| StackGroup {
                    window: None,
                    stack_index,
                    order,
                    elements: Vec::new(),
                })
                .chain([
                    StackGroup {
                        window: None,
                        stack_index: collapsed_index,
                        order: 1,
                        elements: Vec::new(),
                    },
                    StackGroup {
                        window: None,
                        stack_index: collapsed_index,
                        order: 0,
                        elements: Vec::new(),
                    },
                ])
                .collect::<Vec<_>>();
            sort_stack_groups(&mut groups);
            groups
                .into_iter()
                .rev()
                .map(|group| (group.stack_index, group.order))
                .collect::<Vec<_>>()
        };

        // u64::MAX identifies a live window. The close snapshot (1) and
        // resulting node (0) remain on the collapsed window's side of it.
        assert_eq!(
            presented(2, &[(0, u64::MAX), (1, u64::MAX)])[..2],
            [(2, 1), (2, 0)]
        );
        assert_eq!(
            presented(1, &[(0, u64::MAX), (1, u64::MAX)])[..3],
            [(1, u64::MAX), (1, 1), (1, 0)]
        );
        assert_eq!(
            presented(0, &[(0, u64::MAX), (1, u64::MAX)])[..4],
            [(1, u64::MAX), (0, u64::MAX), (0, 1), (0, 0)]
        );
    }
}
