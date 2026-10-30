//! Assemble the Field desktop and its window, node, cluster, and popup planes.

use super::*;

/// Insertion positions established by the overlay and top-layer pass.
pub(super) struct DesktopPlacement {
    pub(super) popup: usize,
    pub(super) desktop: usize,
    pub(super) privacy: CapturePrivacy,
}

pub(super) fn append(
    renderer: &mut GlesRenderer,
    output: &Output,
    primary_output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    request: &mut RenderRequest<'_>,
    placement: DesktopPlacement,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let privacy = placement.privacy;
    let cluster_exclusive = crate::presentation::window::cluster_exclusive_presentation(
        request.desktop.clusters,
        request.desktop.nodes,
        request.desktop.fullscreen,
        request.desktop.maximize,
        output,
        output_geometry,
        request.frame.target_presentation_time,
    );

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
    let cluster_bloom = crate::render::overlays::cluster_bloom::elements(
        renderer,
        crate::render::overlays::cluster_bloom::BloomElementContext {
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
    let cluster_overflow = crate::render::overlays::cluster_overflow::elements(
        renderer,
        crate::render::overlays::cluster_overflow::OverflowElementContext {
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
    let planes = window_stack::arrange(
        request.desktop.space,
        stack,
        live_windows,
        &exclusive_windows,
    );
    elements.extend(planes.desktop);
    let mut popup_stack = planes.popups;
    let mut floating_foreground = planes.floating;

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
        let exclusive_is_protected = privacy.enabled()
            && request
                .desktop
                .nodes
                .record(exclusive.member)
                .is_some_and(|record| {
                    privacy.redact_rule(&request.desktop.window_rules.applied(&record.surface))
                });
        if exclusive.progress > 0.0 && !exclusive_is_protected {
            foreground.push(SceneElement::Border(crate::render::solid_color_element(
                request.resources.node_renderer.slot_id(
                    &output.name(),
                    crate::render::node::NodeSlot::ClusterExclusiveBackdrop,
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
        elements.splice(placement.desktop..placement.desktop, foreground);
    }

    sort_stack_groups(&mut popup_stack);
    crate::window::stacking::sort_above_parents(request.desktop.space, &mut popup_stack, |group| {
        group.window.as_ref()
    });
    elements.splice(
        placement.popup..placement.popup,
        popup_stack
            .into_iter()
            .rev()
            .flat_map(|group| group.elements),
    );

    Ok(())
}
