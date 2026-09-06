use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::time::Duration;

use halley_config::Animations;
use smithay::backend::renderer::Color32F;
use smithay::backend::renderer::Renderer;
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::desktop::Window;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Rectangle};
use smithay::wayland::seat::WaylandFocus;

use crate::animation::close::CloseTimeline;
use crate::presentation::camera::OutputCameras;
use halley_core::field::Vec2;

#[derive(Clone, Copy, Debug)]
pub struct CloseBorder {
    pub width: i32,
    pub color: Color32F,
}

#[derive(Clone, Copy, Debug)]
pub enum CloseAnchor {
    Windowed {
        world_geometry: Rectangle<i32, Logical>,
        captured_camera_rect: Rectangle<i32, Physical>,
    },
    OutputLocal,
}

#[derive(Clone, Debug)]
pub struct CloseSnapshotMetadata {
    pub output_name: String,
    pub initial_destination: Rectangle<i32, Physical>,
    pub anchor: CloseAnchor,
    pub stack_index: usize,
    pub start_alpha: f32,
    pub retract_origin: Option<smithay::utils::Point<f64, Physical>>,
    pub border: Option<CloseBorder>,
    pub content_radius: f32,
    pub collapse_target: Option<Vec2>,
}

struct CapturedWindow {
    id: Id,
    texture: super::window_texture::WindowTexture,
    metadata: CloseSnapshotMetadata,
    order: u64,
}

struct ActiveClose {
    captured: CapturedWindow,
    timeline: CloseTimeline,
    random_seed: f32,
}

pub struct ClosingWindowRender {
    pub texture: TextureRenderElement<GlesTexture>,
    pub shader: Option<super::window_shader::WindowShaderRenderElement>,
    /// The snapshot's own identity, namespaced for the border so both parts
    /// stay stable for the life of the animation.
    pub border_id: Id,
    pub source_texture: GlesTexture,
    pub destination: Rectangle<i32, Physical>,
    pub border: Option<CloseBorder>,
    pub content_radius: f32,
    pub stack_index: usize,
    pub order: u64,
}

pub struct WindowCloseAnimations {
    config: Animations,
    pending: HashMap<WlSurface, CapturedWindow>,
    provisional: HashSet<WlSurface>,
    /// Client-owned close controls can begin the visual handoff before the
    /// client reaches an authoritative unmap boundary. Finished speculative
    /// entries stay transparent until teardown is confirmed or cancelled.
    speculative: Vec<WlSurface>,
    active: HashMap<WlSurface, ActiveClose>,
    next_order: u64,
}

impl WindowCloseAnimations {
    pub fn new(config: Animations) -> Self {
        Self {
            config,
            pending: HashMap::new(),
            provisional: HashSet::new(),
            speculative: Vec::new(),
            active: HashMap::new(),
            next_order: 0,
        }
    }

    pub fn capture(
        &mut self,
        window: &Window,
        texture: super::window_texture::WindowTexture,
        metadata: CloseSnapshotMetadata,
        node_collapse: bool,
    ) -> Result<bool, Box<dyn Error>> {
        let Some(surface) = window.wl_surface().map(|surface| surface.into_owned()) else {
            return Ok(false);
        };
        if !snapshot_enabled(&self.config, node_collapse) {
            self.provisional.remove(&surface);
            self.pending.remove(&surface);
            return Ok(false);
        }

        let captured = self.captured_window(texture, metadata);
        self.provisional.remove(&surface);
        self.pending.insert(surface, captured);
        Ok(true)
    }

    fn captured_window(
        &mut self,
        texture: super::window_texture::WindowTexture,
        metadata: CloseSnapshotMetadata,
    ) -> CapturedWindow {
        self.next_order = self.next_order.wrapping_add(1);
        CapturedWindow {
            id: Id::new(),
            texture,
            metadata,
            order: self.next_order,
        }
    }

    pub fn start(&mut self, surface: &WlSurface, now: Duration) -> bool {
        self.provisional.remove(surface);
        remove_surface(&mut self.speculative, surface);
        let Some(captured) = self.pending.remove(surface) else {
            return false;
        };
        let node_collapse = captured.metadata.collapse_target.is_some();
        if !snapshot_enabled(&self.config, node_collapse) {
            return false;
        }

        let config = snapshot_animation(&self.config, node_collapse);
        self.active.insert(
            surface.clone(),
            ActiveClose {
                timeline: CloseTimeline::new(config, now, captured.metadata.start_alpha),
                captured,
                random_seed: crate::animation::animation_seed(),
            },
        );
        true
    }

    pub fn start_speculative(&mut self, surface: &WlSurface, now: Duration) -> bool {
        if !self.start(surface, now) {
            return false;
        }
        self.speculative.push(surface.clone());
        true
    }

    /// Allows a speculative animation to expire normally once the client has
    /// reached a real surface teardown boundary.
    pub fn confirm_unmapped(&mut self, surface: &WlSurface) -> bool {
        remove_surface(&mut self.speculative, surface)
    }

    /// Restores a client that did not actually close after activating its
    /// client-owned close control.
    pub fn cancel_speculative(&mut self, surface: &WlSurface) -> bool {
        if !remove_surface(&mut self.speculative, surface) {
            return false;
        }
        self.active.remove(surface).is_some()
    }

    pub fn retarget_pending_to_node(&mut self, surface: &WlSurface, target: Vec2) -> bool {
        let Some(captured) = self.pending.get_mut(surface) else {
            return false;
        };
        captured.metadata.collapse_target = Some(target);
        true
    }

    pub fn has_pending(&self, surface: &WlSurface) -> bool {
        self.pending.contains_key(surface)
    }

    pub fn is_active(&self, surface: &WlSurface) -> bool {
        self.active.contains_key(surface)
    }

    pub fn mark_provisional(&mut self, surface: WlSurface) {
        if self.pending.contains_key(&surface) {
            self.provisional.insert(surface);
        }
    }

    pub fn discard_provisional(&mut self, surface: &WlSurface) -> bool {
        if !self.provisional.remove(surface) {
            return false;
        }
        self.pending.remove(surface).is_some()
    }

    pub fn cancel(&mut self, surface: &WlSurface) {
        self.provisional.remove(surface);
        remove_surface(&mut self.speculative, surface);
        self.pending.remove(surface);
        self.active.remove(surface);
    }

    pub fn reload(&mut self, config: Animations) {
        self.config = config;
        if !close_enabled(&self.config) {
            self.provisional.clear();
            self.speculative.clear();
        }
        self.pending.retain(|_, captured| {
            snapshot_enabled(&self.config, captured.metadata.collapse_target.is_some())
        });
    }

    pub fn is_animating_on_output(&self, output: &Output, now: Duration) -> bool {
        self.active.values().any(|active| {
            active.captured.metadata.output_name == output.name()
                && !active.timeline.is_finished_at(now)
        })
    }

    pub fn renders_for_output(
        &self,
        renderer: &GlesRenderer,
        shaders: &super::window_shader::WindowAnimationShaders,
        output: &Output,
        output_geometry: Rectangle<i32, Logical>,
        cameras: &OutputCameras,
        now: Duration,
    ) -> Vec<ClosingWindowRender> {
        let context = renderer.context_id();
        let pending = self.pending.values().filter_map(|captured| {
            let metadata = &captured.metadata;
            if metadata.output_name != output.name() || captured.texture.context != context {
                return None;
            }
            let destination = destination_for(metadata, output, output_geometry, cameras)?;
            closing_render(captured, destination, metadata.start_alpha, None)
        });
        let active = self
            .active
            .values()
            .filter(|active| {
                active.captured.metadata.output_name == output.name()
                    && active.captured.texture.context == context
                    && !active.timeline.is_finished_at(now)
            })
            .filter_map(|active| {
                let metadata = &active.captured.metadata;
                let start = destination_for(metadata, output, output_geometry, cameras)?;
                let visual = active.timeline.visual_at(now);
                let shader_geo = visual.shader_geo(start, metadata.retract_origin);
                if metadata.collapse_target.is_none()
                    && active.timeline.shader_pixels()
                    && shaders.close_available()
                    && shader_geo.size.w > 0
                    && shader_geo.size.h > 0
                    && let Some(shader) = shaders.close_element(
                        renderer,
                        &active.captured.texture,
                        active.captured.id.clone(),
                        shader_geo,
                        visual.linear_progress as f32,
                        visual.linear_progress.clamp(0.0, 1.0) as f32,
                        active.random_seed,
                        metadata.start_alpha,
                    )
                {
                    return Some(ClosingWindowRender {
                        texture: active.captured.texture.render_element(
                            active.captured.id.clone(),
                            shader_geo,
                            metadata.start_alpha,
                        ),
                        shader: Some(shader),
                        border_id: active
                            .captured
                            .id
                            .namespaced(crate::render::window_decoration::slot::BORDER),
                        source_texture: active.captured.texture.texture.clone(),
                        destination: shader_geo,
                        border: None,
                        content_radius: 0.0,
                        stack_index: metadata.stack_index,
                        order: active.captured.order,
                    });
                }
                let destination = if let Some(target) = metadata.collapse_target {
                    let camera = cameras.get(&output.name())?;
                    let target = crate::nodes::screen_from_world(target, camera, output_geometry)
                        - output_geometry.loc;
                    collapse_destination(start, target, visual.progress)
                } else {
                    visual.destination(start, metadata.retract_origin)
                };
                if destination.size.w <= 0 || destination.size.h <= 0 || visual.alpha <= 0.0 {
                    return None;
                }
                closing_render(&active.captured, destination, visual.alpha, None)
            });
        pending.chain(active).collect()
    }

    pub fn cleanup(&mut self, now: Duration) {
        let speculative = &self.speculative;
        self.active.retain(|surface, active| {
            speculative.iter().any(|candidate| candidate == surface)
                || !active.timeline.is_finished_at(now)
        });
    }
}

fn remove_surface(surfaces: &mut Vec<WlSurface>, surface: &WlSurface) -> bool {
    let previous_len = surfaces.len();
    surfaces.retain(|candidate| candidate != surface);
    surfaces.len() != previous_len
}

fn closing_render(
    captured: &CapturedWindow,
    destination: Rectangle<i32, Physical>,
    alpha: f32,
    shader: Option<super::window_shader::WindowShaderRenderElement>,
) -> Option<ClosingWindowRender> {
    if destination.size.w <= 0 || destination.size.h <= 0 || alpha <= 0.0 {
        return None;
    }
    let metadata = &captured.metadata;
    let start = metadata.initial_destination;
    let border = metadata.border.map(|mut border| {
        let scale = (f64::from(destination.size.w) / f64::from(start.size.w.max(1)))
            .min(f64::from(destination.size.h) / f64::from(start.size.h.max(1)));
        border.width = (f64::from(border.width) * scale).round().max(1.0) as i32;
        border.color = border.color * alpha;
        border
    });
    let scale = (destination.size.w as f32 / start.size.w.max(1) as f32)
        .min(destination.size.h as f32 / start.size.h.max(1) as f32);
    Some(ClosingWindowRender {
        texture: captured
            .texture
            .render_element(captured.id.clone(), destination, alpha),
        shader,
        border_id: captured
            .id
            .namespaced(crate::render::window_decoration::slot::BORDER),
        source_texture: captured.texture.texture.clone(),
        destination,
        border,
        content_radius: metadata.content_radius * scale.max(0.0),
        stack_index: metadata.stack_index,
        order: captured.order,
    })
}

fn collapse_destination(
    start: Rectangle<i32, Physical>,
    target: smithay::utils::Point<i32, Logical>,
    progress: f64,
) -> Rectangle<i32, Physical> {
    let progress = progress.clamp(0.0, 1.0);
    let start_center = (
        start.loc.x as f64 + start.size.w as f64 * 0.5,
        start.loc.y as f64 + start.size.h as f64 * 0.5,
    );
    let center = (
        start_center.0 + (f64::from(target.x) - start_center.0) * progress,
        start_center.1 + (f64::from(target.y) - start_center.1) * progress,
    );
    let scale = 1.0 - progress;
    let size = (
        (f64::from(start.size.w) * scale).round().max(1.0) as i32,
        (f64::from(start.size.h) * scale).round().max(1.0) as i32,
    );
    Rectangle::new(
        (
            (center.0 - f64::from(size.0) * 0.5).round() as i32,
            (center.1 - f64::from(size.1) * 0.5).round() as i32,
        )
            .into(),
        size.into(),
    )
}

fn snapshot_enabled(config: &Animations, node_collapse: bool) -> bool {
    if node_collapse {
        config.enabled && config.node.enabled && config.node.collapse_duration_ms > 0
    } else {
        close_enabled(config)
    }
}

fn snapshot_animation(
    config: &Animations,
    node_collapse: bool,
) -> halley_config::WindowCloseAnimation {
    if node_collapse {
        halley_config::WindowCloseAnimation {
            enabled: config.node.enabled,
            duration_ms: config.node.collapse_duration_ms,
            animation_type: halley_config::WindowCloseAnimationType::Shrink,
            custom_shader: None,
        }
    } else {
        config.window_close.clone()
    }
}

fn close_enabled(config: &Animations) -> bool {
    config.enabled && config.window_close.enabled && config.window_close.duration_ms > 0
}

fn destination_for(
    metadata: &CloseSnapshotMetadata,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    cameras: &OutputCameras,
) -> Option<Rectangle<i32, Physical>> {
    match metadata.anchor {
        CloseAnchor::OutputLocal => Some(metadata.initial_destination),
        CloseAnchor::Windowed {
            world_geometry,
            captured_camera_rect,
        } => {
            let view = cameras.view(&output.name())?;
            let camera_center =
                crate::presentation::camera::global_center(view.center, output_geometry);
            let current_camera_rect = super::camera_rect(
                world_geometry.to_physical(1),
                camera_center,
                output_geometry.size.to_physical(1),
                view.scale,
            );
            Some(resolve_windowed_destination(
                metadata.initial_destination,
                captured_camera_rect,
                current_camera_rect,
            ))
        }
    }
}

fn resolve_windowed_destination(
    initial: Rectangle<i32, Physical>,
    captured_camera_rect: Rectangle<i32, Physical>,
    current_camera_rect: Rectangle<i32, Physical>,
) -> Rectangle<i32, Physical> {
    crate::animation::map_rect(initial, captured_camera_rect, current_camera_rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::output::{PhysicalProperties, Subpixel};

    #[test]
    fn close_policy_respects_master_local_and_zero_duration_killswitches() {
        let mut animations = Animations::default();
        assert!(close_enabled(&animations));

        animations.enabled = false;
        assert!(!close_enabled(&animations));
        animations.enabled = true;
        animations.window_close.enabled = false;
        assert!(!close_enabled(&animations));
        animations.window_close.enabled = true;
        animations.window_close.duration_ms = 0;
        assert!(!close_enabled(&animations));
    }

    #[test]
    fn node_collapse_timeline_is_independent_of_marker_and_close_shader() {
        let mut animations = Animations::default();
        animations.node.duration_ms = 900;
        animations.node.collapse_duration_ms = 280;
        animations.window_close.duration_ms = 1700;
        animations.window_close.custom_shader = Some("close.frag".into());
        let collapse =
            CloseTimeline::new(snapshot_animation(&animations, true), Duration::ZERO, 1.0);
        let close = CloseTimeline::new(snapshot_animation(&animations, false), Duration::ZERO, 1.0);
        assert!(!collapse.shader_pixels());
        assert!(close.shader_pixels());
        assert_eq!(collapse.visual_at(Duration::from_millis(140)).progress, 0.5);
        assert!(collapse.is_finished_at(Duration::from_millis(280)));
        assert!(!close.is_finished_at(Duration::from_millis(280)));
        assert!(close.is_finished_at(Duration::from_millis(1700)));
        assert_eq!(animations.node.duration_ms, 900);
    }

    #[test]
    fn node_collapse_has_its_own_enable_and_zero_duration_policy() {
        let mut animations = Animations::default();
        animations.window_close.enabled = false;
        animations.window_close.duration_ms = 0;
        animations.node.duration_ms = 0;
        assert!(snapshot_enabled(&animations, true));
        assert!(!snapshot_enabled(&animations, false));
        animations.node.collapse_duration_ms = 0;
        assert!(!snapshot_enabled(&animations, true));
        animations.node.collapse_duration_ms = 280;
        animations.node.enabled = false;
        assert!(!snapshot_enabled(&animations, true));
        animations.node.enabled = true;
        animations.enabled = false;
        assert!(!snapshot_enabled(&animations, true));
        animations = Animations::default();
        animations.node.enabled = false;
        assert!(snapshot_enabled(&animations, false));
    }

    #[test]
    fn node_collapse_moves_and_shrinks_into_its_target() {
        let start = Rectangle::<i32, Physical>::new((100, 200).into(), (800, 600).into());
        let target = smithay::utils::Point::<i32, Logical>::from((900, 700));

        assert_eq!(collapse_destination(start, target, 0.0), start);
        let middle = collapse_destination(start, target, 0.5);
        assert_eq!(middle.size, (400, 300).into());
        assert_eq!(
            (
                middle.loc.x + middle.size.w / 2,
                middle.loc.y + middle.size.h / 2,
            ),
            (700, 600)
        );
        let end = collapse_destination(start, target, 1.0);
        assert_eq!(end.size, (1, 1).into());
        assert_eq!(end.loc, target.to_physical(1));
    }

    #[test]
    fn windowed_ghost_tracks_camera_translation_and_scale() {
        let initial = Rectangle::new((250, 150).into(), (400, 300).into());
        let captured = Rectangle::new((100, 50).into(), (800, 600).into());
        let current = Rectangle::new((0, 0).into(), (400, 300).into());

        assert_eq!(
            resolve_windowed_destination(initial, captured, current),
            Rectangle::new((75, 50).into(), (200, 150).into())
        );
    }

    #[test]
    fn output_local_ghost_keeps_its_captured_tile_when_the_camera_is_unavailable() {
        let output = Output::new(
            "DP-1".into(),
            PhysicalProperties {
                size: (600, 340).into(),
                subpixel: Subpixel::Unknown,
                make: "Halley".into(),
                model: "Test".into(),
                serial_number: "1".into(),
            },
        );
        let initial = Rectangle::new((420, 80).into(), (640, 720).into());
        let metadata = CloseSnapshotMetadata {
            output_name: output.name(),
            initial_destination: initial,
            anchor: CloseAnchor::OutputLocal,
            stack_index: 0,
            start_alpha: 1.0,
            retract_origin: None,
            border: None,
            content_radius: 0.0,
            collapse_target: None,
        };

        assert_eq!(
            destination_for(
                &metadata,
                &output,
                Rectangle::new((0, 0).into(), (1_920, 1_080).into()),
                &OutputCameras::default(),
            ),
            Some(initial)
        );
    }
}
