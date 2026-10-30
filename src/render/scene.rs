mod build;
pub use build::build;
pub(crate) use build::{build_with_privacy, build_with_screenshot_privacy};

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
mod chrome;
mod close_stack;
mod cluster_composer;
mod clusters;
mod effects;
mod live_collection;
pub(crate) mod nodes;
mod overview;
mod privacy;
mod replacement;
mod windows;

use capture_ui::capture_overlay_elements;
use cluster_composer::cluster_composer_elements;
use clusters::{ClusterElementContext, cluster_elements};
use effects::{
    LayerSceneStyle, OverlayEffectStyle, append_compositor_overlay_blur, append_overlay_shadows,
    layer_surface_scene_elements,
};
use nodes::{NodeElementContext, node_elements};
use overview::{
    FocusCycleRenderContext, apogee_elements, focus_cycle_elements, hover_preview_elements,
};
use privacy::CapturePrivacy;
pub(crate) use windows::append_titlebar_elements;
use windows::{
    LiveWindowContext, LiveWindowRenderers, LiveWindowScene, StackGroup, live_window_elements,
    sort_stack_groups,
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

fn indicate_window_cast(scene: &mut LiveWindowScene, decorations: &halley_config::Decorations) {
    let bounds = scene
        .elements
        .iter()
        .filter(|element| {
            !matches!(
                element,
                SceneElement::Shadow(_) | SceneElement::BackdropBlur(_)
            )
        })
        .map(|element| element.geometry(Scale::from(1.0)))
        .reduce(|a, b| a.merge(b));
    if let Some(bounds) = bounds {
        let width = decorations
            .screencast_indicator_size
            .min(bounds.size.w / 2)
            .min(bounds.size.h / 2);
        if width == 0 {
            return;
        }
        let inner = Rectangle::new(
            (bounds.loc.x + width, bounds.loc.y + width).into(),
            (bounds.size.w - 2 * width, bounds.size.h - 2 * width).into(),
        );
        let border = super::border_strips(
            std::array::from_fn(|_| Id::new()),
            inner,
            width,
            super::decoration_color(decorations.screencast_indicator_color),
        );
        scene
            .elements
            .splice(0..0, border.into_iter().map(SceneElement::Border));
    }
}

fn omit_elements(elements: &mut Vec<SceneElement>) {
    elements.clear();
}

fn omit_window_scene(scene: &mut LiveWindowScene) {
    omit_elements(&mut scene.elements);
    omit_elements(&mut scene.popup_elements);
}

fn build_logical(
    renderer: &mut GlesRenderer,
    output: &Output,
    primary_output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    request: &mut RenderRequest<'_>,
    privacy: CapturePrivacy,
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
    // These replacement scenes render cached previews that may have been
    // captured before a privacy rule was enabled. Fail closed during them.
    if privacy.redact_cached_scene()
        && (request.overlays.apogee.is_active()
            || request.overlays.focus_cycle.is_active()
            || request
                .overlays
                .cluster_composer
                .replacement_output()
                .is_some_and(|name| name == output.name()))
    {
        return Ok(vec![SceneElement::Border(super::solid_color_element(
            Id::new(),
            Rectangle::from_size(output_geometry.size.to_physical(1)),
            smithay::backend::renderer::Color32F::BLACK,
        ))]);
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
        return replacement::cluster_composer(
            renderer,
            output,
            output_geometry,
            request,
            overlay_snapshot,
        );
    }

    // Apogee is a replacement scene, not a translucent layer over the live
    // desktop. Keep only its tiles and the wallpaper layer behind them; normal
    // windows, nodes, panels, and desktop overlays must not bleed through.
    if request.overlays.apogee.is_active() {
        return replacement::apogee(renderer, output, output_geometry, request, overlay_snapshot);
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
        Layer::Overlay,
        LayerSceneStyle {
            output_geometry,

            layer_rules: request.desktop.layer_rules,

            block_screencast: privacy.redact_layers(),

            blur: request.visuals.blur,
        },
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
            Layer::Top,
            LayerSceneStyle {
                output_geometry,

                layer_rules: request.desktop.layer_rules,

                block_screencast: privacy.redact_layers(),

                blur: request.visuals.blur,
            },
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
    if !privacy.redact_cached_scene() {
        elements.extend(hover_preview);
    }

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

    let mut stack = close_stack::stack_groups(
        renderer,
        output,
        output_geometry,
        close_stack::ClosingSceneContext {
            animations: request.resources.window_close_animations,
            shaders: request.resources.window_shaders,
            decorations: request.resources.window_decoration_renderer,
            shadow_renderer: request.resources.shadow_renderer,
            shadow_style: request.visuals.shadows.window,
            cameras: request.desktop.cameras,
            now: request.frame.target_presentation_time,
            redact: privacy.redact_cached_windows(),
        },
    )?;
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
    let collection = live_collection::collect(
        renderer,
        primary_output,
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
        request.resources.window_close_animations,
        request.desktop.screencast,
        privacy,
    )?;
    let live_windows = collection.normal;
    let mut exclusive_windows = collection.exclusive;
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
    let landmark_ceiling = live_windows
        .iter()
        .filter(|(_, scene)| scene.above_field_landmarks)
        .map(|(stack_index, _)| *stack_index)
        .min();
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
    if let Some(ceiling) = landmark_ceiling {
        // Collapsed nodes remember their old Space slot, which can now be
        // higher than every remaining live window. Temporarily place desktop
        // objects below presentations without changing their saved Field depth.
        for group in stack.iter_mut().filter(|group| group.window.is_none()) {
            group.stack_index = group.stack_index.min(ceiling);
        }
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
        let exclusive_is_protected =
            request
                .desktop
                .nodes
                .record(exclusive.member)
                .is_some_and(|record| {
                    privacy.redact_rule(&request.desktop.window_rules.applied(&record.surface))
                });
        if exclusive.progress > 0.0 && !exclusive_is_protected {
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
        Layer::Bottom,
        LayerSceneStyle {
            output_geometry,

            layer_rules: request.desktop.layer_rules,

            block_screencast: privacy.redact_layers(),

            blur: request.visuals.blur,
        },
        request.resources.backdrop_blur_renderer,
    )?);
    elements.extend(layer_surface_scene_elements(
        renderer,
        output,
        Layer::Background,
        LayerSceneStyle {
            output_geometry,

            layer_rules: request.desktop.layer_rules,

            block_screencast: privacy.redact_layers(),

            blur: request.visuals.blur,
        },
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

    chrome::prepend_common_overlays(
        renderer,
        output,
        output_geometry,
        request,
        overlay_snapshot,
        &mut elements,
    )?;

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
mod tests;
