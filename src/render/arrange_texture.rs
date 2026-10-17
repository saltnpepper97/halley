use std::collections::HashMap;
use std::error::Error;

use smithay::backend::renderer::Renderer;
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Rectangle, Size};
use smithay::wayland::seat::WaylandFocus;

use super::window_texture::ResizeWindowTexture;

#[derive(Default)]
pub struct ArrangeTextureTransitions {
    windows: HashMap<WlSurface, ArrangeTexture>,
    resize_renderer: super::resize::ResizeRenderer,
}

struct ArrangeTexture {
    id: Id,
    previous: ResizeWindowTexture,
    target: Option<ResizeWindowTexture>,
    target_size: Size<i32, Logical>,
    target_ready_at_completion: Option<f64>,
    capture_generation: CommitCounter,
}

impl ArrangeTextureTransitions {
    /// The client remains offscreen for the entire blend, including after
    /// its first target buffer. Keep callbacks flowing so child surfaces can
    /// finish resizing and animated content continues updating.
    pub fn uses_snapshot(&self, surface: &WlSurface) -> bool {
        self.windows.contains_key(surface)
    }

    pub fn capture(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &Window,
        target_size: Size<i32, Logical>,
        preserve_existing: bool,
    ) -> Result<(), Box<dyn Error>> {
        let surface = window
            .wl_surface()
            .ok_or("arrange snapshot window has no surface")?
            .into_owned();
        if should_preserve_existing(self.windows.contains_key(&surface), preserve_existing) {
            let entry = self
                .windows
                .get_mut(&surface)
                .expect("existing arrange snapshot checked above");
            entry.target_size = target_size;
            entry.target = None;
            entry.target_ready_at_completion = None;
            entry.capture_generation.increment();
            return Ok(());
        }

        let previous = super::window_texture::capture_for_resize(renderer, window, None)?;
        self.windows.insert(
            surface,
            ArrangeTexture {
                id: Id::new(),
                previous,
                target: None,
                target_size,
                target_ready_at_completion: None,
                capture_generation: CommitCounter::default(),
            },
        );
        Ok(())
    }

    pub fn capture_target(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &Window,
        arrange_completion: f64,
    ) -> Result<bool, Box<dyn Error>> {
        let surface = window
            .wl_surface()
            .ok_or("arrange target window has no surface")?
            .into_owned();
        let context = renderer.context_id();
        let Some(entry) = self.windows.get(&surface) else {
            return Ok(false);
        };
        if entry.previous.context != context {
            self.windows.remove(&surface);
            return Ok(false);
        }
        if window.geometry().size != entry.target_size {
            return Ok(false);
        }

        let candidate = super::window_texture::capture_for_resize(renderer, window, None)?;
        let entry = self
            .windows
            .get_mut(&surface)
            .expect("arrange transition checked above");
        if !crate::render::fullscreen_texture::snapshot_matches_target_endpoint(
            &entry.previous,
            &candidate,
        ) {
            return Ok(false);
        }

        entry.target = Some(candidate);
        entry
            .target_ready_at_completion
            .get_or_insert(arrange_completion.clamp(0.0, 1.0));
        entry.capture_generation.increment();
        Ok(true)
    }

    pub fn fallback_element(
        &self,
        surface: &WlSurface,
        destination: Rectangle<i32, Physical>,
        alpha: f32,
    ) -> Option<(TextureRenderElement<GlesTexture>, GlesTexture)> {
        let entry = self.windows.get(surface)?;
        Some((
            super::resize::texture_element_for_window(
                &entry.previous,
                entry.id.clone(),
                destination,
                alpha,
            ),
            entry.previous.texture.clone(),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn blend_element(
        &mut self,
        renderer: &mut GlesRenderer,
        surface: &WlSurface,
        destination: Rectangle<i32, Physical>,
        arrange_completion: f64,
        alpha: f32,
        radii: super::window_decoration::CornerRadii,
    ) -> Result<Option<super::resize::ResizeRenderElement>, Box<dyn Error>> {
        let context = renderer.context_id();
        let Some(entry) = self.windows.get(surface) else {
            return Ok(None);
        };
        if entry.previous.context != context {
            self.windows.remove(surface);
            return Ok(None);
        }
        let (next, progress) = match entry.target.clone() {
            Some(next) => (
                next,
                reveal_progress(arrange_completion, entry.target_ready_at_completion, true) as f32,
            ),
            // Use the same fitted mapping while waiting for the client, so
            // target arrival changes only the blend, not the outgoing scale.
            None => (entry.previous.clone(), 0.0),
        };
        Ok(Some(self.resize_renderer.arrange_element(
            renderer,
            entry.id.clone(),
            &entry.previous,
            next,
            destination,
            progress,
            alpha,
            radii,
            entry.capture_generation,
        )?))
    }

    pub fn retain_surfaces(&mut self, mut keep: impl FnMut(&WlSurface) -> bool) {
        self.windows.retain(|surface, _| keep(surface));
    }

    pub fn remove(&mut self, surface: &WlSurface) {
        self.windows.remove(surface);
    }
}

fn reveal_progress(
    arrange_completion: f64,
    target_ready_at_completion: Option<f64>,
    has_target: bool,
) -> f64 {
    if !has_target {
        return 0.0;
    }
    let ready = target_ready_at_completion.unwrap_or(0.0).clamp(0.0, 1.0);
    if ready >= 1.0 {
        return 1.0;
    }
    ((arrange_completion.clamp(0.0, 1.0) - ready) / (1.0 - ready)).clamp(0.0, 1.0)
}

fn should_preserve_existing(has_snapshot: bool, preserve_existing: bool) -> bool {
    has_snapshot && preserve_existing
}

#[cfg(test)]
mod tests {
    use super::{reveal_progress, should_preserve_existing};

    #[test]
    fn reversal_keeps_the_original_outgoing_frame_only_while_motion_is_live() {
        assert!(should_preserve_existing(true, true));
        assert!(!should_preserve_existing(true, false));
        assert!(!should_preserve_existing(false, true));
    }

    #[test]
    fn native_reveal_starts_when_the_target_is_ready_and_finishes_at_the_endpoint() {
        assert_eq!(reveal_progress(0.2, Some(0.2), true), 0.0);
        assert!((reveal_progress(0.6, Some(0.2), true) - 0.5).abs() < f64::EPSILON);
        assert_eq!(reveal_progress(1.0, Some(0.2), true), 1.0);
        assert_eq!(reveal_progress(0.8, None, false), 0.0);
    }
}
