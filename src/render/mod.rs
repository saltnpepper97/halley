mod app_icon;
pub mod arrange_texture;
pub mod background;
pub mod close;
pub(crate) mod conservative;
pub mod display_scale;
pub mod effects;
pub mod fullscreen_texture;
pub mod ids;
pub mod node;
pub mod overlays;
pub mod pin;
pub mod rescale;
pub mod resize;
pub mod resources;
pub mod scene;
mod selection_check;
pub mod text;
pub mod titlebar;
pub mod window_decoration;
pub mod window_open;
pub mod window_shader;
pub mod window_texture;

use halley_config::Decorations;
use smithay::backend::renderer::Color32F;
use smithay::backend::renderer::element::RenderElementStates;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::{PopupManager, Space, Window, layer_map_for_output};
use smithay::input::pointer::CursorIcon;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::Layer;

use crate::cursor::CursorManager;

/// Opaque black used only to clear pixels with no scene content.
///
/// `wallpaper.mode "none"` contributes no render element, so this is the
/// resulting empty desktop rather than a compositor-enforced wallpaper.
pub const CLEAR_COLOR: Color32F = Color32F::new(0.0, 0.0, 0.0, 1.0);
/// Fail-closed backdrop used for every output while ext-session-lock-v1 owns
/// the session, including outputs whose locker surface is not ready yet.
pub const SESSION_LOCK_COLOR: Color32F = Color32F::new(0.0, 0.0, 0.0, 1.0);

pub fn output_physical_size(output: &Output) -> Size<i32, Physical> {
    output.current_transform().transform_size(
        output
            .current_mode()
            .expect("mapped output has a mode")
            .size,
    )
}

pub fn window_surface_location(
    mapped_geometry_location: Point<i32, Logical>,
    local_geometry: Rectangle<i32, Logical>,
) -> Point<i32, Physical> {
    (mapped_geometry_location - local_geometry.loc).to_physical(1)
}

/// Build a window's normal surface tree separately from its popups.
///
/// Smithay's `Window::render_elements` combines both, which is convenient
/// until the compositor needs to crop client-side shadows to the window
/// geometry: popups must remain free to extend beyond that geometry.
pub fn window_surface_elements(
    renderer: &mut GlesRenderer,
    window: &Window,
    surface_location: Point<i32, Physical>,
    alpha: f32,
) -> (
    Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
    Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
) {
    let Some(surface) = window.wl_surface() else {
        return (Vec::new(), Vec::new());
    };
    let scale = Scale::from(1.0);
    let popup_elements = window
        .toplevel()
        .map(|toplevel| {
            PopupManager::popups_for_surface(toplevel.wl_surface())
                .flat_map(|(popup, popup_offset)| {
                    let offset = (window.geometry().loc + popup_offset - popup.geometry().loc)
                        .to_physical_precise_round(scale);
                    render_elements_from_surface_tree(
                        renderer,
                        popup.wl_surface(),
                        surface_location + offset,
                        scale,
                        alpha,
                        Kind::ScanoutCandidate,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let surface_elements = render_elements_from_surface_tree(
        renderer,
        surface.as_ref(),
        surface_location,
        scale,
        alpha,
        Kind::ScanoutCandidate,
    );

    (popup_elements, surface_elements)
}

/// Builds one output's layer-shell surfaces in front-to-back order.
///
/// `LayerMap` supplies the protocol-defined placement and popup tree. Unlike
/// normal windows, the returned locations are output-local and intentionally
/// never pass through Halley's workspace camera.
pub fn layer_surface_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    layer: Layer,
) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
    let map = layer_map_for_output(output);
    // The complete logical scene is converted to output pixels at its boundary.
    let scale = Scale::from(1.0);
    map.layers_on(layer)
        .rev()
        .flat_map(|surface| {
            let Some(geometry) = map.layer_geometry(surface) else {
                return Vec::new();
            };
            let location = geometry.loc.to_f64().to_physical(scale).to_i32_round();
            let popup_elements = PopupManager::popups_for_surface(surface.wl_surface()).flat_map(
                |(popup, popup_offset)| {
                    let offset = (popup_offset - popup.geometry().loc)
                        .to_f64()
                        .to_physical(scale)
                        .to_i32_round();
                    render_elements_from_surface_tree(
                        renderer,
                        popup.wl_surface(),
                        location + offset,
                        scale,
                        1.0,
                        Kind::ScanoutCandidate,
                    )
                },
            );
            let mut elements: Vec<_> = popup_elements.collect();
            elements.extend(render_elements_from_surface_tree(
                renderer,
                surface.wl_surface(),
                location,
                scale,
                1.0,
                Kind::ScanoutCandidate,
            ));
            elements
        })
        .collect()
}

/// Whether rendering an output submitted a frame that will produce a
/// presentation event, or found no new damage to submit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderStatus {
    Submitted,
    Skipped,
}

#[derive(Debug)]
pub struct RenderOutcome {
    status: RenderStatus,
    element_states: Option<RenderElementStates>,
}

impl RenderOutcome {
    pub fn new(status: RenderStatus, element_states: Option<RenderElementStates>) -> Self {
        Self {
            status,
            element_states,
        }
    }

    pub fn status(&self) -> RenderStatus {
        self.status
    }

    pub fn element_states(&self) -> Option<&RenderElementStates> {
        self.element_states.as_ref()
    }
}

#[derive(Debug)]
pub struct FrameSubmission {
    pub target_presentation_time: std::time::Duration,
    pub presentation_feedback: smithay::desktop::utils::OutputPresentationFeedback,
    pub session_lock_generation: Option<u64>,
    pub variable_refresh: bool,
}

/// Per-frame scheduling and output policy.
pub struct FrameContext {
    pub target_presentation_time: std::time::Duration,
    /// True only when the output has a committed, fully settled fullscreen
    /// window and no compositor or layer-shell overlay is visible.
    pub vrr_auto_eligible: bool,
    /// Discard incremental buffer-age history for this frame. The DRM backend
    /// uses this while scene geometry is animated so every scan-out buffer is
    /// repainted from the same complete scene.
    pub force_full_repaint: bool,
    pub clear: Color32F,
}

/// Desktop state and presentation policy used to place live surfaces.
pub struct DesktopContext<'a> {
    pub session_lock: &'a crate::wayland::session_lock::State,
    pub space: &'a Space<Window>,
    pub focused: Option<&'a WlSurface>,
    pub cameras: &'a crate::presentation::camera::OutputCameras,
    pub window_animations: &'a crate::animation::WindowAnimations,
    pub fullscreen: &'a crate::wayland::fullscreen::FullscreenManager,
    pub maximize: &'a crate::presentation::maximize::FieldMaximizeManager,
    pub nodes: &'a crate::nodes::NodesState,
    pub clusters: &'a crate::clusters::ClusterSystem,
    pub window_rules: &'a crate::window::rules::WindowRulesState,
    pub layer_rules: &'a [halley_config::LayerRule],
    pub node_grab_active: bool,
    pub titlebar_hovered: Option<&'a crate::titlebar::ButtonTarget>,
    pub titlebar_pressed: Option<&'a crate::titlebar::ButtonTarget>,
}

/// Cursor presentation inputs. Pointer constraint policy is deliberately not
/// part of rendering and remains owned by the session input subsystem.
pub struct CursorContext<'a> {
    pub cursor: &'a CursorManager,
    pub dnd_icon: Option<&'a crate::wayland::dnd::DndIcon>,
    pub cursor_position: (f64, f64),
    pub show_cursor: bool,
    pub cursor_override: Option<CursorIcon>,
}

/// Shell-owned overlays and replacement scenes.
pub struct OverlayContext<'a> {
    pub capture_overlay: crate::capture::CaptureOverlay<'a>,
    pub bearings: &'a crate::shell::bearings::BearingsState,
    pub focus_cycle: &'a crate::shell::focus_cycle::FocusCycleState,
    pub apogee: &'a crate::shell::apogee::ApogeeState,
    pub cluster_composer: &'a crate::shell::cluster_composer::ClusterComposerState,
    pub apogee_config: halley_config::Apogee,
    pub overlays: &'a crate::shell::overlay::OverlayManager,
    pub overlay_config: &'a halley_config::Overlays,
}

/// Immutable visual configuration for one frame.
pub struct VisualContext<'a> {
    pub decorations: &'a Decorations,
    pub pins: &'a halley_config::Pins,
    pub font: &'a halley_config::Font,
    pub debug: halley_config::Debug,
    pub blur: halley_config::Blur,
    pub shadows: halley_config::Shadows,
    pub background: &'a halley_config::Background,
    pub background_base: Option<&'a std::path::Path>,
}

/// Complete input to one scene build, grouped by ownership boundary.
pub struct RenderRequest<'a> {
    pub frame: FrameContext,
    pub desktop: DesktopContext<'a>,
    pub cursor: CursorContext<'a>,
    pub overlays: OverlayContext<'a>,
    pub visuals: VisualContext<'a>,
    pub resources: resources::RenderResources<'a>,
}

/// A presentation backend for one already-described scene.
pub trait Renderable {
    fn render(
        &mut self,
        output: &Output,
        request: RenderRequest<'_>,
    ) -> Result<RenderOutcome, Box<dyn std::error::Error>>;
}

pub fn decoration_color(color: halley_config::BorderColor) -> Color32F {
    Color32F::new(color.r, color.g, color.b, 1.0)
}

/// Whichever color a window's border should be drawn in, given whether its
/// surface is the focused one.
pub fn window_border_color(decorations: &Decorations, is_focused: bool) -> Color32F {
    decoration_color(if is_focused {
        decorations.border_color_focused
    } else {
        decorations.border_color_unfocused
    })
}

pub fn solid_color_element(
    id: Id,
    geometry: Rectangle<i32, Physical>,
    color: Color32F,
) -> SolidColorRenderElement {
    SolidColorRenderElement::new(
        id,
        geometry,
        window_decoration::solid_color_commit(color),
        color,
        Kind::Unspecified,
    )
}

/// Left, right, and bottom strips used when a titlebar owns the top edge.
pub fn body_border_strips(
    ids: [Id; 3],
    bbox: Rectangle<i32, Physical>,
    width: i32,
    color: Color32F,
) -> [SolidColorRenderElement; 3] {
    let make = |id: Id, rect: Rectangle<i32, Physical>| {
        SolidColorRenderElement::new(
            id,
            rect,
            window_decoration::solid_color_commit(color),
            color,
            Kind::Unspecified,
        )
    };
    [
        make(
            ids[0].clone(),
            Rectangle::new(
                (bbox.loc.x - width, bbox.loc.y).into(),
                (width, bbox.size.h + width).into(),
            ),
        ),
        make(
            ids[1].clone(),
            Rectangle::new(
                (bbox.loc.x + bbox.size.w, bbox.loc.y).into(),
                (width, bbox.size.h + width).into(),
            ),
        ),
        make(
            ids[2].clone(),
            Rectangle::new(
                (bbox.loc.x - width, bbox.loc.y + bbox.size.h).into(),
                (bbox.size.w + width * 2, width).into(),
            ),
        ),
    ]
}

/// Four thin solid-color strips forming a frame just outside `bbox` - not
/// overlapping window content, since nothing shrinks a window to make room
/// for a border yet (that's real decoration-chrome-painting work, deferred).
pub fn border_strips(
    ids: [Id; 4],
    bbox: Rectangle<i32, Physical>,
    width: i32,
    color: Color32F,
) -> [SolidColorRenderElement; 4] {
    let make = |id: Id, rect: Rectangle<i32, Physical>| {
        SolidColorRenderElement::new(
            id,
            rect,
            window_decoration::solid_color_commit(color),
            color,
            Kind::Unspecified,
        )
    };
    let top = Rectangle::new(
        (bbox.loc.x - width, bbox.loc.y - width).into(),
        (bbox.size.w + width * 2, width).into(),
    );
    let bottom = Rectangle::new(
        (bbox.loc.x - width, bbox.loc.y + bbox.size.h).into(),
        (bbox.size.w + width * 2, width).into(),
    );
    let left = Rectangle::new(
        (bbox.loc.x - width, bbox.loc.y).into(),
        (width, bbox.size.h).into(),
    );
    let right = Rectangle::new(
        (bbox.loc.x + bbox.size.w, bbox.loc.y).into(),
        (width, bbox.size.h).into(),
    );
    [
        make(ids[0].clone(), top),
        make(ids[1].clone(), bottom),
        make(ids[2].clone(), left),
        make(ids[3].clone(), right),
    ]
}

/// Maps a physical-pixel rect from world (`Space`) coordinates to screen
/// coordinates, given the camera's live center (a world point) and zoom
/// scale - `screen = output_center + (world - camera_center) * scale`,
/// matching old halley's own `world_to_screen`. `scale` is always <= 1.0
/// (the camera driving it hardcodes its upper bound to 1.0, so there's no
/// zoom-in). At rest (`camera_center == output_center`, `scale == 1.0`,
/// i.e. no pan and no zoom) this is the identity transform - which is
/// exactly what this function used to assume unconditionally, back when
/// nothing could ever move the camera off-center.
pub fn camera_rect(
    rect: Rectangle<i32, Physical>,
    camera_center: Point<f32, Physical>,
    output_size: Size<i32, Physical>,
    scale: f32,
) -> Rectangle<i32, Physical> {
    let scale = f64::from(scale);
    // Round the camera translation once, independently of each surface's
    // dimensions. Center rounding gave odd/even rectangles different phases,
    // making client pixels and chrome take their last pixel step separately.
    let translation = Point::<i32, Physical>::from((
        (f64::from(output_size.w) / 2.0 - f64::from(camera_center.x) * scale).round() as i32,
        (f64::from(output_size.h) / 2.0 - f64::from(camera_center.y) * scale).round() as i32,
    ));
    // Shared world edges also stay shared under zoom. Panning changes only
    // translation, preserving every part's size and relative placement.
    let left = (f64::from(rect.loc.x) * scale).round() as i32;
    let top = (f64::from(rect.loc.y) * scale).round() as i32;
    let right = ((f64::from(rect.loc.x) + f64::from(rect.size.w)) * scale).round() as i32;
    let bottom = ((f64::from(rect.loc.y) + f64::from(rect.size.h)) * scale).round() as i32;
    Rectangle::new(
        Point::from((left, top)) + translation,
        Size::from((right - left, bottom - top)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output_center(output_size: Size<i32, Physical>) -> Point<f32, Physical> {
        Point::from((output_size.w as f32 / 2.0, output_size.h as f32 / 2.0))
    }

    #[test]
    fn surface_origin_accounts_for_client_side_geometry_offset() {
        let mapped = Point::<i32, Logical>::from((500, 300));
        let local_geometry = Rectangle::<i32, Logical>::new((12, 16).into(), (800, 600).into());

        assert_eq!(
            window_surface_location(mapped, local_geometry),
            Point::from((488, 284))
        );
    }

    #[test]
    fn at_rest_is_the_identity_transform() {
        let output_size = Size::<i32, Physical>::from((1280, 800));
        let rect = Rectangle::<i32, Physical>::new((320, 160).into(), (640, 480).into());
        let result = camera_rect(rect, output_center(output_size), output_size, 1.0);
        assert_eq!(result, rect);
    }

    #[test]
    fn zoom_out_shrinks_toward_output_center_when_camera_is_unpanned() {
        let output_size = Size::<i32, Physical>::from((1280, 800));
        // Exactly centered on the output - shrinking around center should
        // leave it centered, only smaller.
        let rect = Rectangle::<i32, Physical>::new((320, 160).into(), (640, 480).into());
        let result = camera_rect(rect, output_center(output_size), output_size, 0.5);
        assert_eq!(result.size, Size::from((320, 240)));
        assert_eq!(result.loc, (480, 280).into());
    }

    #[test]
    fn pan_shifts_everything_opposite_the_camera_offset_at_scale_one() {
        let output_size = Size::<i32, Physical>::from((1280, 800));
        let rect = Rectangle::<i32, Physical>::new((320, 160).into(), (640, 480).into());
        // Camera panned 100px right/50px down from the output's own center -
        // content should appear shifted 100px left/50px up on screen.
        let panned_center = output_center(output_size) + Point::from((100.0, 50.0));
        let result = camera_rect(rect, panned_center, output_size, 1.0);
        assert_eq!(result.loc, (220, 110).into());
        assert_eq!(result.size, rect.size);
    }

    #[test]
    fn camera_projection_preserves_odd_sized_rectangles_at_rest() {
        for output_size in [Size::from((1280, 800)), Size::from((1279, 799))] {
            let rect = Rectangle::new((-21, 17).into(), (641, 479).into());
            assert_eq!(
                camera_rect(rect, output_center(output_size), output_size, 1.0),
                rect
            );
        }
    }

    #[test]
    fn slow_pan_moves_different_sized_surface_parts_together() {
        let output = Size::from((1280, 800));
        let parts = [
            Rectangle::new((320, 160).into(), (640, 480).into()),
            Rectangle::new((333, 177).into(), (101, 79).into()),
            Rectangle::new((-21, -17).into(), (643, 481).into()),
        ];
        for scale in [1.0, 0.75, 0.35] {
            let initial = parts.map(|part| camera_rect(part, output_center(output), output, scale));
            for step in 0..200 {
                let camera = output_center(output)
                    + Point::from((step as f32 * 0.031, step as f32 * -0.047));
                let moved = parts.map(|part| camera_rect(part, camera, output, scale));
                let shift = moved[0].loc - initial[0].loc;
                for index in 1..parts.len() {
                    assert_eq!(
                        moved[index].loc - initial[index].loc,
                        shift,
                        "scale={scale} step={step} part={index}"
                    );
                    assert_eq!(moved[index].size, initial[index].size);
                }
            }
        }
    }

    #[test]
    fn zoomed_adjacent_surface_parts_keep_their_shared_edge() {
        let output = Size::from((1280, 800));
        for scale in [0.35, 0.5, 0.75, 1.0] {
            for step in 0..30 {
                let camera = output_center(output) + Point::from((step as f32 * 0.13, 0.0));
                let left = camera_rect(
                    Rectangle::new((-21, 17).into(), (103, 79).into()),
                    camera,
                    output,
                    scale,
                );
                let right = camera_rect(
                    Rectangle::new((82, 17).into(), (101, 79).into()),
                    camera,
                    output,
                    scale,
                );
                assert_eq!(
                    left.loc.x + left.size.w,
                    right.loc.x,
                    "scale={scale} step={step}"
                );
            }
        }
    }
}
