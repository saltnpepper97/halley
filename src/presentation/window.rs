use smithay::desktop::{Space, Window};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle};
use smithay::wayland::compositor::{SubsurfaceCachedState, get_parent, with_states};
use smithay::wayland::seat::WaylandFocus;

use crate::presentation::camera::OutputCameras;

/// The presentation geometry shared by scene construction and input routing.
///
/// Keeping this in the input-independent presentation module makes the
/// compositor render and invert the exact same camera, fullscreen, and
/// opening-animation rectangles.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WindowVisualState {
    pub(crate) source_geometry: Rectangle<i32, Logical>,
    pub(crate) camera_rect: Rectangle<i32, Physical>,
    pub(crate) presentation_rect: Rectangle<i32, Physical>,
    pub(crate) animated_rect: Rectangle<i32, Physical>,
    pub(crate) opening_alpha: f32,
    opening_progress: f32,
    opening_clamped_progress: f32,
    opening_random_seed: f32,
    shader_pixels: bool,
    pub(crate) fullscreen: Option<crate::wayland::fullscreen::FullscreenPresentation>,
    pub(crate) maximize: Option<crate::presentation::maximize::FieldMaximizePresentation>,
    pub(crate) camera_center: Point<f32, Physical>,
    pub(crate) zoom_scale: f32,
    pub(crate) presentation_space: PresentationSpace,
    pub(crate) cluster_depth: Option<usize>,
    pub(crate) cluster_floating: bool,
    pub(crate) cluster_exclusive: bool,
    inherited_presentation: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClusterExclusivePresentation {
    pub(crate) member: halley_core::field::NodeId,
    pub(crate) progress: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentationSpace {
    Field,
    OutputLocal,
}

fn output_local_or_field_rect(
    output_local: Option<Rectangle<i32, Physical>>,
    world: Rectangle<i32, Logical>,
    camera_center: Point<f32, Physical>,
    output_size: smithay::utils::Size<i32, Physical>,
    zoom_scale: f32,
) -> Rectangle<i32, Physical> {
    output_local.unwrap_or_else(|| {
        crate::render::camera_rect(world.to_physical(1), camera_center, output_size, zoom_scale)
    })
}

pub(crate) fn cluster_exclusive_presentation(
    clusters: &crate::clusters::ClusterSystem,
    nodes: &crate::nodes::NodesState,
    fullscreen: &crate::wayland::fullscreen::FullscreenManager,
    maximize: &crate::presentation::maximize::FieldMaximizeManager,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    now: std::time::Duration,
) -> Option<ClusterExclusivePresentation> {
    let cluster = clusters.active_on(&output.name())?;
    clusters.member_ids(cluster).into_iter().find_map(|member| {
        let surface = &nodes.record(member)?.surface;
        let progress = fullscreen
            .presentation(surface, output, now)
            .map(|presentation| presentation.progress as f32)
            .or_else(|| {
                maximize
                    .presentation(surface, output, output_geometry, now)
                    .map(|presentation| presentation.progress as f32)
            })?;
        Some(ClusterExclusivePresentation { member, progress })
    })
}

fn is_cluster_exclusive_window(
    window_node: Option<halley_core::field::NodeId>,
    presentation: Option<ClusterExclusivePresentation>,
    has_cluster_override: bool,
) -> bool {
    !has_cluster_override
        && presentation.is_some_and(|presentation| Some(presentation.member) == window_node)
}

impl WindowVisualState {
    pub(crate) fn maps_from_source(self) -> bool {
        self.inherited_presentation || self.fullscreen.is_some() || self.maximize.is_some()
    }

    pub(crate) fn shader_pixels(self) -> bool {
        self.shader_pixels
    }

    pub(crate) fn opening_progress(self) -> f32 {
        self.opening_progress
    }

    pub(crate) fn opening_clamped_progress(self) -> f32 {
        self.opening_clamped_progress
    }

    pub(crate) fn opening_random_seed(self) -> f32 {
        self.opening_random_seed
    }
}

fn output_local_zoom_scale(space: PresentationSpace, view_scale: f32) -> f32 {
    match space {
        PresentationSpace::OutputLocal => 1.0,
        PresentationSpace::Field => view_scale,
    }
}

/// Chrome scale for a fullscreen/maximize crossfade.
///
/// Cluster destinations interpolate in native pixels, so this stays 1.0.
/// Field destinations start at the camera-scaled on-screen rect; using native
/// 1.0 there draws a full-size titlebar on a still-zoomed window, and using
/// the live camera fights the output-local dest. Scale chrome by dest vs the
/// native size at the same progress.
fn presentation_display_scale(
    animated: smithay::utils::Size<i32, Physical>,
    windowed_native: smithay::utils::Size<i32, Physical>,
    target_native: smithay::utils::Size<i32, Physical>,
    progress: f64,
) -> f32 {
    let native = (f64::from(windowed_native.h)
        + f64::from(target_native.h - windowed_native.h) * progress)
        .round()
        .max(1.0);
    animated.h as f32 / native as f32
}

fn inherited_visual_rects(
    source: Rectangle<i32, Physical>,
    owner_source: Rectangle<i32, Physical>,
    owner_camera: Rectangle<i32, Physical>,
    owner_presentation: Rectangle<i32, Physical>,
    owner_animated: Rectangle<i32, Physical>,
) -> (
    Rectangle<i32, Physical>,
    Rectangle<i32, Physical>,
    Rectangle<i32, Physical>,
) {
    let camera = crate::animation::map_rect(source, owner_source, owner_camera);
    let presentation = crate::animation::map_rect(source, owner_source, owner_presentation);
    let animated = crate::animation::map_rect(presentation, owner_presentation, owner_animated);
    (camera, presentation, animated)
}

// Keep this behavior-frozen adapter explicit: render, pointer hit-testing, and
// pointer constraints must all receive the exact same presentation inputs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn window_visual_state(
    space: &Space<Window>,
    cameras: &OutputCameras,
    clusters: Option<&crate::clusters::ClusterSystem>,
    nodes: Option<&crate::nodes::NodesState>,
    window: &Window,
    output: &Output,
    window_animations: &crate::animation::WindowAnimations,
    fullscreen: &crate::wayland::fullscreen::FullscreenManager,
    maximize: &crate::presentation::maximize::FieldMaximizeManager,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    now: std::time::Duration,
) -> Option<WindowVisualState> {
    window_visual_state_with_cluster_presentation(
        space,
        cameras,
        clusters,
        nodes,
        window,
        output,
        window_animations,
        fullscreen,
        maximize,
        decorations,
        font,
        now,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn window_visual_state_with_cluster_presentation(
    space: &Space<Window>,
    cameras: &OutputCameras,
    clusters: Option<&crate::clusters::ClusterSystem>,
    nodes: Option<&crate::nodes::NodesState>,
    window: &Window,
    output: &Output,
    window_animations: &crate::animation::WindowAnimations,
    fullscreen: &crate::wayland::fullscreen::FullscreenManager,
    maximize: &crate::presentation::maximize::FieldMaximizeManager,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    now: std::time::Duration,
    cluster_override: Option<crate::clusters::WindowPresentation>,
) -> Option<WindowVisualState> {
    let output_geometry = space.output_geometry(output)?;
    let output_size = output_geometry.size.to_physical(1);
    let view = cameras.view(&output.name())?;
    let camera_center = crate::presentation::camera::global_center(view.center, output_geometry);
    let source_geometry = space.element_geometry(window)?;
    let window_surface = window.wl_surface()?;
    let window_node = nodes.and_then(|nodes| nodes.id_for_surface(window_surface.as_ref()));
    let exclusive_presentation = clusters.zip(nodes).and_then(|(clusters, nodes)| {
        cluster_exclusive_presentation(
            clusters,
            nodes,
            fullscreen,
            maximize,
            output,
            output_geometry,
            now,
        )
    });
    let cluster_exclusive = is_cluster_exclusive_window(
        window_node,
        exclusive_presentation,
        cluster_override.is_some(),
    );
    let cluster_presentation = cluster_override.unwrap_or_else(|| {
        clusters
            .zip(nodes)
            .and_then(|(clusters, _nodes)| window_node.map(|id| (clusters, id)))
            .map(|(clusters, id)| {
                let core = clusters
                    .transition_cluster_on(&output.name(), now)
                    .and_then(|cluster| clusters.metadata(cluster))
                    .map(|metadata| {
                        crate::nodes::screen_from_world(
                            metadata.core_position,
                            cameras
                                .get(&output.name())
                                .expect("an output view always has a backing camera"),
                            output_geometry,
                        ) - output_geometry.loc
                    });
                clusters.window_presentation(
                    id,
                    &output.name(),
                    smithay::desktop::layer_map_for_output(output).non_exclusive_zone(),
                    core,
                    now,
                )
            })
            .unwrap_or(crate::clusters::WindowPresentation::Field)
    });
    let (cluster_rect, cluster_depth, cluster_alpha) = match cluster_presentation {
        crate::clusters::WindowPresentation::Hidden => return None,
        crate::clusters::WindowPresentation::Field => (None, None, 1.0),
        crate::clusters::WindowPresentation::PointerDrag { rect } => {
            (Some(rect.to_physical(1)), None, 1.0)
        }
        crate::clusters::WindowPresentation::Workspace { rect, depth, alpha } => {
            let rect = if window_node.is_some_and(|node| {
                clusters.is_some_and(|clusters| {
                    clusters.member_layout(node)
                        == Some(halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Tiling)
                })
            }) {
                crate::titlebar::client_rect_for_outer(window, rect, decorations, font)
            } else {
                rect
            };
            (Some(rect.to_physical(1)), Some(depth), alpha)
        }
    };
    let mut camera_rect = output_local_or_field_rect(
        cluster_rect,
        source_geometry,
        camera_center,
        output_size,
        view.scale,
    );
    let opening_visual = window_animations
        .visual(window_surface.as_ref(), now, camera_rect)
        .unwrap_or_default();
    let fullscreen_presentation = fullscreen.presentation(window_surface.as_ref(), output, now);
    let maximize_presentation = fullscreen_presentation
        .is_none()
        .then(|| maximize.presentation(window_surface.as_ref(), output, output_geometry, now))
        .flatten();
    if (fullscreen_presentation.is_some() || maximize_presentation.is_some())
        && clusters.zip(nodes).is_some_and(|(clusters, nodes)| {
            !crate::presentation::surface_workspace_is_active(
                clusters,
                nodes,
                window_surface.as_ref(),
                &output.name(),
                now,
            )
        })
    {
        return None;
    }
    let mut presentation_rect = fullscreen_presentation
        .map(|presentation| {
            let windowed = presentation.windowed_geometry.map_or_else(
                || presentation.fullscreen_rect(output_size),
                |geometry| {
                    output_local_or_field_rect(
                        presentation.windowed_output_rect.or(cluster_rect),
                        geometry,
                        camera_center,
                        output_size,
                        view.scale,
                    )
                },
            );
            presentation.client_rect(windowed, output_size)
        })
        .or_else(|| {
            maximize_presentation.map(|presentation| {
                let windowed = output_local_or_field_rect(
                    presentation.windowed_output_rect.or(cluster_rect),
                    presentation.windowed_rect,
                    camera_center,
                    output_size,
                    view.scale,
                );
                presentation.client_rect(windowed)
            })
        })
        .unwrap_or(camera_rect);
    let opening_rect = opening_visual.transform_rect(presentation_rect, presentation_rect);
    let mut animated_rect = window_animations
        .arrange_visual(window_surface.as_ref(), now)
        .unwrap_or(opening_rect);
    let mut opening_alpha = opening_visual.alpha() * cluster_alpha;
    let mut opening_progress = opening_visual.progress() as f32;
    let mut opening_clamped_progress = opening_visual.clamped_progress() as f32;
    let mut opening_random_seed = opening_visual.random_seed();
    let mut shader_pixels = opening_visual.shader_pixels();
    let mut inherited_presentation = cluster_rect.is_some();
    let mut presentation_space = if cluster_rect.is_some()
        || fullscreen_presentation.is_some()
        || maximize_presentation.is_some()
    {
        PresentationSpace::OutputLocal
    } else {
        PresentationSpace::Field
    };
    let mut inherited_camera_center = camera_center;
    // Cluster tiles already live in output-local space at scale 1.0, so their
    // maximize/fullscreen crossfade interpolates a stable rectangle. Field
    // windows take the same output-local dest, but that dest starts camera-
    // scaled: decorations have to follow dest/native, not live camera zoom
    // and not native 1.0.
    let mut inherited_zoom_scale = if let Some(presentation) = fullscreen_presentation {
        let windowed = presentation.windowed_geometry.map_or_else(
            || presentation.fullscreen_size.to_physical(1),
            |geometry| geometry.size.to_physical(1),
        );
        presentation_display_scale(
            animated_rect.size,
            windowed,
            presentation.fullscreen_size.to_physical(1),
            presentation.progress,
        )
    } else if let Some(presentation) = maximize_presentation {
        presentation_display_scale(
            animated_rect.size,
            presentation.windowed_rect.size.to_physical(1),
            presentation.target_rect.size,
            presentation.progress,
        )
    } else {
        output_local_zoom_scale(presentation_space, view.scale)
    };
    let mut inherited_cluster_depth = cluster_depth.filter(|_| !cluster_exclusive);
    let mut inherited_cluster_floating = window_node
        .is_some_and(|node| clusters.is_some_and(|clusters| clusters.is_member_floating(node)));
    let mut inherited_cluster_exclusive = cluster_exclusive;

    if let Some(owner_xid) = crate::wayland::window_presentation_owner(window)
        && let Some(owner) = crate::xwayland::window_for_xid(space, owner_xid)
        && let Some(owner_visual) = window_visual_state_with_cluster_presentation(
            space,
            cameras,
            clusters,
            nodes,
            &owner,
            output,
            window_animations,
            fullscreen,
            maximize,
            decorations,
            font,
            now,
            None,
        )
    {
        let source = source_geometry.to_physical(1);
        let owner_source = owner_visual.source_geometry.to_physical(1);
        (camera_rect, presentation_rect, animated_rect) = inherited_visual_rects(
            source,
            owner_source,
            owner_visual.camera_rect,
            owner_visual.presentation_rect,
            owner_visual.animated_rect,
        );
        opening_alpha = owner_visual.opening_alpha;
        opening_progress = owner_visual.opening_progress;
        opening_clamped_progress = owner_visual.opening_clamped_progress;
        opening_random_seed = owner_visual.opening_random_seed;
        shader_pixels = owner_visual.shader_pixels;
        inherited_camera_center = owner_visual.camera_center;
        inherited_zoom_scale = owner_visual.zoom_scale;
        inherited_cluster_depth = owner_visual.cluster_depth;
        inherited_cluster_floating = owner_visual.cluster_floating;
        inherited_cluster_exclusive = owner_visual.cluster_exclusive;
        presentation_space = owner_visual.presentation_space;
        inherited_presentation = true;
    }

    Some(WindowVisualState {
        source_geometry,
        camera_rect,
        presentation_rect,
        animated_rect,
        opening_alpha,
        opening_progress,
        opening_clamped_progress,
        opening_random_seed,
        shader_pixels,
        fullscreen: fullscreen_presentation,
        maximize: maximize_presentation,
        camera_center: inherited_camera_center,
        zoom_scale: inherited_zoom_scale,
        presentation_space,
        cluster_depth: inherited_cluster_depth,
        cluster_floating: inherited_cluster_floating,
        cluster_exclusive: inherited_cluster_exclusive,
        inherited_presentation,
    })
}

/// The live mapping between a window's compositor-space geometry and its
/// current on-screen presentation.
///
/// Pointer routing and pointer constraints both consume this type. Keeping
/// the inverse pair here prevents either path from reconstructing a surface
/// origin from the pointer's last event location.
#[derive(Clone, Debug)]
pub struct WindowPresentation {
    root: WlSurface,
    source_geometry: Rectangle<i32, Logical>,
    root_origin: Point<f64, Logical>,
    visual_geometry: Rectangle<i32, Logical>,
    hit_geometry: Rectangle<i32, Logical>,
    cluster_depth: Option<usize>,
}

impl WindowPresentation {
    // This adapter intentionally mirrors `window_visual_state`; grouping it
    // would churn the pointer-constraint boundary without reducing policy.
    #[allow(clippy::too_many_arguments)]
    pub fn for_window(
        space: &Space<Window>,
        cameras: &OutputCameras,
        clusters: Option<&crate::clusters::ClusterSystem>,
        nodes: Option<&crate::nodes::NodesState>,
        window_animations: &crate::animation::WindowAnimations,
        fullscreen: &crate::wayland::fullscreen::FullscreenManager,
        maximize: &crate::presentation::maximize::FieldMaximizeManager,
        decorations: &halley_config::Decorations,
        font: &halley_config::Font,
        window: &Window,
        output: &Output,
        now: std::time::Duration,
    ) -> Option<Self> {
        let root = window.wl_surface()?.into_owned();
        let output_geometry = space.output_geometry(output)?;
        let visual = window_visual_state(
            space,
            cameras,
            clusters,
            nodes,
            window,
            output,
            window_animations,
            fullscreen,
            maximize,
            decorations,
            font,
            now,
        )?;
        let source_geometry = visual.source_geometry;
        let source_bbox = space.element_bbox(window)?;
        let element_location = space.element_location(window)?;
        let root_origin = (element_location - window.geometry().loc).to_f64();
        let output_size = output_geometry.size.to_physical(1);
        let global_geometry = |local: Rectangle<i32, Physical>| {
            Rectangle::new(
                output_geometry.loc + local.loc.to_logical(1),
                local.size.to_logical(1),
            )
        };
        let hit_rect = if visual.maps_from_source() {
            let presented = crate::animation::map_rect(
                source_bbox.to_physical(1),
                source_geometry.to_physical(1),
                visual.presentation_rect,
            );
            crate::animation::map_rect(presented, visual.presentation_rect, visual.animated_rect)
        } else {
            let camera_bbox = crate::render::camera_rect(
                source_bbox.to_physical(1),
                visual.camera_center,
                output_size,
                visual.zoom_scale,
            );
            crate::animation::map_rect(camera_bbox, visual.camera_rect, visual.animated_rect)
        };
        let visual_geometry = global_geometry(visual.animated_rect);
        let hit_geometry = global_geometry(hit_rect);

        Some(Self {
            root,
            source_geometry,
            root_origin,
            visual_geometry,
            hit_geometry,
            cluster_depth: visual.cluster_depth,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn for_surface(
        space: &Space<Window>,
        cameras: &OutputCameras,
        clusters: Option<&crate::clusters::ClusterSystem>,
        nodes: Option<&crate::nodes::NodesState>,
        primary: &Output,
        window_animations: &crate::animation::WindowAnimations,
        fullscreen: &crate::wayland::fullscreen::FullscreenManager,
        maximize: &crate::presentation::maximize::FieldMaximizeManager,
        decorations: &halley_config::Decorations,
        font: &halley_config::Font,
        surface: &WlSurface,
        now: std::time::Duration,
    ) -> Option<Self> {
        let root = crate::wayland::compositor::root_surface(surface);
        let window = space.elements().find(|window| {
            window
                .wl_surface()
                .is_some_and(|candidate| candidate.as_ref() == &root)
        })?;
        let output = space
            .outputs()
            .find(|output| crate::wayland::window_is_on_output(window, output, primary))?;
        Self::for_window(
            space,
            cameras,
            clusters,
            nodes,
            window_animations,
            fullscreen,
            maximize,
            decorations,
            font,
            window,
            output,
            now,
        )
    }

    pub fn visual_geometry(&self) -> Rectangle<i32, Logical> {
        self.visual_geometry
    }

    pub(crate) fn source_geometry(&self) -> Rectangle<i32, Logical> {
        self.source_geometry
    }

    pub(crate) fn hit_geometry(&self) -> Rectangle<i32, Logical> {
        self.hit_geometry
    }

    /// Layout-owned overlap order for an active cluster workspace. Input must
    /// use the same depth as rendering or a visually covered member can steal
    /// clicks from the card above it.
    pub(crate) fn cluster_depth(&self) -> Option<usize> {
        self.cluster_depth
    }

    pub fn contains_screen(&self, screen: Point<f64, Logical>) -> bool {
        self.hit_geometry.to_f64().contains(screen)
    }

    pub fn source_from_screen(&self, screen: Point<f64, Logical>) -> Point<f64, Logical> {
        map_point(
            screen,
            self.visual_geometry.to_f64(),
            self.source_geometry.to_f64(),
        )
    }

    pub fn screen_from_source(&self, source: Point<f64, Logical>) -> Point<f64, Logical> {
        map_point(
            source,
            self.source_geometry.to_f64(),
            self.visual_geometry.to_f64(),
        )
    }

    /// Output rectangle expressed in this window's parent-geometry coordinates.
    ///
    /// xdg-positioner constraint targets use this space. Inverse-mapping the
    /// output through the live presentation keeps fullscreen and field-maximize
    /// menus on screen instead of inside the field camera's world viewport.
    pub fn popup_constraint_target(
        &self,
        output_geometry: Rectangle<i32, Logical>,
        popup_toplevel_coords: Point<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        popup_constraint_in_parent(
            output_geometry,
            self.source_geometry,
            self.visual_geometry,
            popup_toplevel_coords,
        )
    }

    /// The root wl_surface origin expressed in X11 root-desktop coordinates.
    ///
    /// `Space` stores the root in Field coordinates, while an X11 client sees
    /// one fixed desktop coordinate space.  The two only coincide when the
    /// output camera is at rest, so XWayland geometry must use this mapped
    /// origin rather than `Space::element_location`.
    pub(crate) fn root_screen_origin(&self) -> Point<i32, Logical> {
        self.screen_from_source(self.root_origin).to_i32_round()
    }

    /// The root wl_surface origin in Field/source coordinates.
    ///
    /// X11 child windows express offsets from the owner's last published root
    /// position in native client units. Keeping this origin available lets the
    /// XWM preserve that relative offset before the owner's live presentation
    /// transform scales it.
    pub(crate) fn root_source_origin(&self) -> Point<i32, Logical> {
        self.root_origin.to_i32_round()
    }

    /// Converts an X11 root-desktop point back into the source coordinate of
    /// this window's root wl_surface.
    pub(crate) fn root_source_from_screen(
        &self,
        screen: Point<i32, Logical>,
    ) -> Point<i32, Logical> {
        self.source_from_screen(screen.to_f64()).to_i32_round()
    }

    pub fn surface_origin(&self, surface: &WlSurface) -> Option<Point<f64, Logical>> {
        let offset = subsurface_offset_from_root(surface, &self.root)?;
        Some(self.root_origin + offset.to_f64())
    }

    pub fn surface_from_screen(
        &self,
        surface: &WlSurface,
        screen: Point<f64, Logical>,
    ) -> Option<Point<f64, Logical>> {
        Some(self.source_from_screen(screen) - self.surface_origin(surface)?)
    }

    pub fn screen_from_surface(
        &self,
        surface: &WlSurface,
        local: Point<f64, Logical>,
    ) -> Option<Point<f64, Logical>> {
        Some(self.screen_from_source(self.surface_origin(surface)? + local))
    }
}

fn subsurface_offset_from_root(
    surface: &WlSurface,
    expected_root: &WlSurface,
) -> Option<Point<i32, Logical>> {
    let mut current = surface.clone();
    let mut offset = Point::from((0, 0));
    while &current != expected_root {
        let parent = get_parent(&current)?;
        let location = with_states(&current, |states| {
            states
                .cached_state
                .get::<SubsurfaceCachedState>()
                .current()
                .location
        });
        offset += location;
        current = parent;
    }
    Some(offset)
}

fn map_point(
    point: Point<f64, Logical>,
    source: Rectangle<f64, Logical>,
    destination: Rectangle<f64, Logical>,
) -> Point<f64, Logical> {
    let scale_x = destination.size.w / source.size.w.max(1.0);
    let scale_y = destination.size.h / source.size.h.max(1.0);
    (
        destination.loc.x + (point.x - source.loc.x) * scale_x,
        destination.loc.y + (point.y - source.loc.y) * scale_y,
    )
        .into()
}

/// Inverse-maps `output_geometry` through a window's visual presentation into
/// parent-geometry-relative coordinates for `xdg_positioner` constraints.
pub(crate) fn popup_constraint_in_parent(
    output_geometry: Rectangle<i32, Logical>,
    source_geometry: Rectangle<i32, Logical>,
    visual_geometry: Rectangle<i32, Logical>,
    popup_toplevel_coords: Point<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let output_in_source = crate::animation::map_rect(
        output_geometry.to_physical(1),
        visual_geometry.to_physical(1),
        source_geometry.to_physical(1),
    )
    .to_logical(1);
    Rectangle::new(
        output_in_source.loc - source_geometry.loc - popup_toplevel_coords,
        output_in_source.size,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform(
        source: Rectangle<i32, Logical>,
        visual: Rectangle<i32, Logical>,
    ) -> (Rectangle<f64, Logical>, Rectangle<f64, Logical>) {
        (source.to_f64(), visual.to_f64())
    }

    #[test]
    fn output_local_presentations_ignore_field_camera_zoom() {
        assert_eq!(
            output_local_zoom_scale(PresentationSpace::OutputLocal, 0.5),
            1.0
        );
        assert_eq!(output_local_zoom_scale(PresentationSpace::Field, 0.5), 0.5);
    }

    #[test]
    fn cluster_crossfade_chrome_stays_native() {
        assert_eq!(
            presentation_display_scale(
                (1000, 600).into(),
                (1000, 600).into(),
                (2000, 1200).into(),
                0.0
            ),
            1.0
        );
        assert_eq!(
            presentation_display_scale(
                (1500, 900).into(),
                (1000, 600).into(),
                (2000, 1200).into(),
                0.5
            ),
            1.0
        );
        assert_eq!(
            presentation_display_scale(
                (2000, 1200).into(),
                (1000, 600).into(),
                (2000, 1200).into(),
                1.0
            ),
            1.0
        );
    }

    #[test]
    fn field_crossfade_chrome_follows_the_zoomed_destination() {
        let start = presentation_display_scale(
            (350, 210).into(),
            (1000, 600).into(),
            (2000, 1200).into(),
            0.0,
        );
        let end = presentation_display_scale(
            (2000, 1200).into(),
            (1000, 600).into(),
            (2000, 1200).into(),
            1.0,
        );

        assert!((start - 0.35).abs() < f32::EPSILON);
        assert_eq!(end, 1.0);
    }

    #[test]
    fn exclusive_cluster_presentation_promotes_only_the_live_target() {
        let target = halley_core::field::NodeId::new(4);
        let sibling = halley_core::field::NodeId::new(5);
        let presentation = Some(ClusterExclusivePresentation {
            member: target,
            progress: 0.5,
        });

        assert!(is_cluster_exclusive_window(
            Some(target),
            presentation,
            false
        ));
        assert!(!is_cluster_exclusive_window(
            Some(sibling),
            presentation,
            false
        ));
        assert!(!is_cluster_exclusive_window(
            Some(target),
            presentation,
            true,
        ));
    }

    #[test]
    fn screen_and_surface_mapping_are_exact_inverses() {
        let (source, visual) = transform(
            Rectangle::new((400, 200).into(), (1280, 720).into()),
            Rectangle::new((2560, 0).into(), (1920, 1080).into()),
        );
        let local = Point::from((720.0, 380.0));
        let screen = map_point(local, source, visual);

        assert_eq!(screen, Point::from((3040.0, 270.0)));
        assert_eq!(map_point(screen, visual, source), local);
    }

    #[test]
    fn x11_field_origin_maps_to_the_settled_secondary_root_position() {
        let (source, visual) = transform(
            Rectangle::new((1546, -214).into(), (1532, 1009).into()),
            Rectangle::new((2655, 181).into(), (1532, 1009).into()),
        );

        assert_eq!(
            map_point(Point::from((1546.0, -214.0)), source, visual),
            Point::from((2655.0, 181.0))
        );
    }

    #[test]
    fn root_surface_offset_is_mapped_without_changing_native_size() {
        let (source, visual) = transform(
            Rectangle::new((400, 200).into(), (800, 600).into()),
            Rectangle::new((2600, 100).into(), (400, 300).into()),
        );
        let root_source = Point::from((380.0, 170.0));

        assert_eq!(
            map_point(root_source, source, visual),
            Point::from((2590.0, 85.0))
        );
    }

    #[test]
    fn camera_zoom_and_pan_mapping_round_trips() {
        let (source, visual) = transform(
            Rectangle::new((1700, -650).into(), (3840, 2400).into()),
            Rectangle::new((2560, 0).into(), (1920, 1200).into()),
        );
        for point in [
            Point::from((1700.0, -650.0)),
            Point::from((3620.0, 550.0)),
            Point::from((5539.0, 1749.0)),
        ] {
            let screen = map_point(point, source, visual);
            let round_trip = map_point(screen, visual, source);
            assert!((round_trip.x - point.x).abs() < f64::EPSILON);
            assert!((round_trip.y - point.y).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn fullscreen_letterbox_mapping_uses_live_client_rectangle() {
        let (source, visual) = transform(
            Rectangle::new((0, 0).into(), (1024, 768).into()),
            Rectangle::new((240, 0).into(), (1440, 1080).into()),
        );

        assert_eq!(
            map_point(Point::from((512.0, 384.0)), source, visual),
            Point::from((960.0, 540.0))
        );
        assert_eq!(
            map_point(Point::from((960.0, 540.0)), visual, source),
            Point::from((512.0, 384.0))
        );
    }

    #[test]
    fn cluster_workspace_rect_ignores_the_parked_field_camera() {
        let cluster = Rectangle::<i32, Physical>::new((200, 100).into(), (800, 600).into());
        let world = Rectangle::<i32, Logical>::new((400, 250).into(), (900, 700).into());

        assert_eq!(
            output_local_or_field_rect(
                Some(cluster),
                world,
                Point::from((1_400.0, 900.0)),
                (1920, 1080).into(),
                0.5,
            ),
            cluster
        );
        assert_ne!(
            output_local_or_field_rect(
                None,
                world,
                Point::from((1_400.0, 900.0)),
                (1920, 1080).into(),
                0.5,
            ),
            cluster
        );
    }

    #[test]
    fn x11_popup_inherits_owners_panned_and_fullscreen_presentation() {
        let owner_source = Rectangle::<i32, Physical>::new((1090, 425).into(), (1920, 1200).into());
        let popup_source = Rectangle::<i32, Physical>::new((1158, 491).into(), (147, 198).into());
        let owner_camera = Rectangle::<i32, Physical>::new((0, 0).into(), (1920, 1200).into());
        let owner_fullscreen = Rectangle::<i32, Physical>::new((0, 0).into(), (1920, 1200).into());
        let owner_animated = Rectangle::<i32, Physical>::new((96, 60).into(), (1728, 1080).into());

        let (camera, fullscreen, animated) = inherited_visual_rects(
            popup_source,
            owner_source,
            owner_camera,
            owner_fullscreen,
            owner_animated,
        );

        assert_eq!(camera, Rectangle::new((68, 66).into(), (147, 198).into()));
        assert_eq!(fullscreen, camera);
        assert_eq!(
            animated,
            Rectangle::new((157, 119).into(), (133, 179).into())
        );
    }

    #[test]
    fn subsurface_coordinates_share_the_window_transform() {
        let source = Rectangle::<f64, Logical>::new((400.0, 200.0).into(), (800.0, 600.0).into());
        let visual = Rectangle::<f64, Logical>::new((100.0, 50.0).into(), (1600.0, 1200.0).into());
        let root_origin = Point::from((380.0, 170.0));
        let subsurface_offset = Point::from((30.0, 40.0));
        let local = Point::from((25.0, 15.0));
        let source_point = root_origin + subsurface_offset + local;
        let screen = map_point(source_point, source, visual);
        let round_trip = map_point(screen, visual, source) - root_origin - subsurface_offset;

        assert_eq!(screen, Point::from((170.0, 100.0)));
        assert_eq!(round_trip, local);
    }

    #[test]
    fn settled_fullscreen_popup_constraint_covers_the_output() {
        let output = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
        let source = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
        let visual = output;
        let windowed_center = Point::<f32, Physical>::from((1040.0, 560.0));
        let camera_viewport = crate::presentation::camera::world_viewport(
            crate::presentation::camera::OutputView {
                center: windowed_center,
                scale: 1.0,
            },
            output,
        );

        let target = popup_constraint_in_parent(output, source, visual, Point::from((0, 0)));

        assert_eq!(target, Rectangle::new((0, 0).into(), (1920, 1080).into()));
        assert_ne!(
            {
                let mut camera_target = camera_viewport;
                camera_target.loc -= source.loc;
                camera_target
            },
            target,
            "the field camera stays on the old windowed center and must not own the constraint"
        );
    }

    #[test]
    fn settled_maximize_popup_constraint_includes_the_gap() {
        let output = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
        let source = Rectangle::<i32, Logical>::new((20, 20).into(), (1880, 1040).into());
        let visual = source;

        assert_eq!(
            popup_constraint_in_parent(output, source, visual, Point::from((0, 0))),
            Rectangle::new((-20, -20).into(), (1920, 1080).into())
        );
    }

    #[test]
    fn panned_maximize_camera_does_not_steal_the_popup_constraint() {
        let output = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
        let source = Rectangle::<i32, Logical>::new((20, 20).into(), (1880, 1040).into());
        let visual = source;
        let camera_viewport = crate::presentation::camera::world_viewport(
            crate::presentation::camera::OutputView {
                center: Point::from((3000.0, 540.0)),
                scale: 1.0,
            },
            output,
        );
        let mut camera_target = camera_viewport;
        camera_target.loc -= source.loc;
        let target = popup_constraint_in_parent(output, source, visual, Point::from((0, 0)));

        assert_eq!(
            target,
            Rectangle::new((-20, -20).into(), (1920, 1080).into())
        );
        assert!(
            camera_target.loc.x > source.size.w,
            "today's camera formula would place the box beside the client"
        );
        assert!(!camera_target.overlaps(Rectangle::from_size(source.size)));
        assert!(target.overlaps(Rectangle::from_size(source.size)));
    }

    #[test]
    fn field_popup_constraint_matches_the_camera_world_viewport() {
        let output = Rectangle::<i32, Logical>::new((2560, 0).into(), (1920, 1200).into());
        let world_viewport = crate::presentation::camera::world_viewport(
            crate::presentation::camera::OutputView {
                center: Point::from((1060.0, 550.0)),
                scale: 0.5,
            },
            output,
        );
        assert_eq!(
            world_viewport,
            Rectangle::new((1700, -650).into(), (3840, 2400).into())
        );
        let source = Rectangle::<i32, Logical>::new((1800, -570).into(), (800, 600).into());
        let visual = Rectangle::<i32, Logical>::new((2610, 40).into(), (400, 300).into());
        let popup_coords = Point::<i32, Logical>::from((12, 8));
        let mut expected = world_viewport;
        expected.loc -= source.loc;
        expected.loc -= popup_coords;

        let target = popup_constraint_in_parent(output, source, visual, popup_coords);
        assert!(target.loc.x.abs_diff(expected.loc.x) <= 1);
        assert!(target.loc.y.abs_diff(expected.loc.y) <= 1);
        assert!(target.size.w.abs_diff(expected.size.w) <= 1);
        assert!(target.size.h.abs_diff(expected.size.h) <= 1);
    }

    #[test]
    fn nested_popup_constraint_subtracts_toplevel_coords_once() {
        let output = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
        let source = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
        let nested = popup_constraint_in_parent(output, source, output, Point::from((80, 40)));
        let root = popup_constraint_in_parent(output, source, output, Point::from((0, 0)));

        assert_eq!(nested.loc, root.loc - Point::from((80, 40)));
        assert_eq!(nested.size, root.size);
    }

    #[test]
    fn positioner_slide_keeps_a_corner_menu_on_the_new_fullscreen_target() {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::{
            Anchor, ConstraintAdjustment, Gravity,
        };
        use smithay::wayland::shell::xdg::PositionerState;

        let positioner = PositionerState {
            rect_size: (200, 300).into(),
            anchor_rect: Rectangle::new((50, 50).into(), (1, 1).into()),
            anchor_edges: Anchor::TopLeft,
            gravity: Gravity::BottomRight,
            constraint_adjustment: ConstraintAdjustment::SlideX,
            offset: (0, 0).into(),
            ..Default::default()
        };

        let output = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
        let source = output;
        let visual = output;
        let correct = popup_constraint_in_parent(output, source, visual, Point::from((0, 0)));
        let mut camera_target = crate::presentation::camera::world_viewport(
            crate::presentation::camera::OutputView {
                center: Point::from((1040.0, 560.0)),
                scale: 1.0,
            },
            output,
        );
        camera_target.loc -= source.loc;

        let requested = positioner.get_geometry();
        let slid = positioner.get_unconstrained_geometry(camera_target);
        let kept = positioner.get_unconstrained_geometry(correct);

        assert_eq!(requested.loc, Point::from((50, 50)));
        assert_ne!(
            slid.loc.x, requested.loc.x,
            "the old camera box must actually slide a corner menu"
        );
        assert_eq!(kept.loc, requested.loc);
    }
}
