use std::error::Error;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::egl::EGLDevice;
use smithay::backend::renderer::ImportDma;
use smithay::backend::renderer::element::{
    Element, RenderElement, RenderElementPresentationState, RenderElementState, RenderElementStates,
};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Frame, Renderer};
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Physical, Rectangle, Size, Transform};
use smithay::wayland::presentation::Refresh;

use super::{RenderOutcome, RenderRequest, RenderStatus, Renderable};

/// The winit (nested) backend - a real window on the host's desktop,
/// standing in for real hardware output. Used for dev/testing; the tty
/// backend (real hardware, a separate later plan) will be a sibling module
/// implementing the same `Renderable` trait, not sharing state with this
/// one.
pub struct WinitBackend {
    backend: WinitGraphicsBackend<GlesRenderer>,
    /// The wl_output clients see for this window. A plain geometry/mode
    /// object - constructing it needs no `DisplayHandle`, so this backend
    /// still never touches Wayland protocol state directly; the driving
    /// code (main.rs) is what registers the actual global and maps it into
    /// a `Space`.
    output: Output,
}

fn output_mode(size: Size<i32, Physical>) -> Mode {
    Mode {
        size,
        refresh: 60_000,
    }
}

impl WinitBackend {
    pub fn new(backend: WinitGraphicsBackend<GlesRenderer>) -> Self {
        let mode = output_mode(backend.window_size());
        let output = Output::new(
            "winit".to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "halley-next".into(),
                model: "winit".into(),
                serial_number: "unknown".into(),
            },
        );
        output.change_current_state(
            Some(mode),
            Some(Transform::Flipped180),
            None,
            Some((0, 0).into()),
        );
        output.set_preferred(mode);

        Self { backend, output }
    }

    /// Ask the window for another `Redraw` event. Winit-specific (not part
    /// of `Renderable`) since the tty backend's equivalent is triggered by
    /// vblank/page-flip events, not a request call like this - there's no
    /// shared shape between the two worth forcing into one trait method.
    pub fn request_redraw(&self) {
        self.backend.window().request_redraw();
    }

    /// The window's current size - used by callers to convert absolute
    /// pointer positions into this output's coordinate space.
    pub fn window_size(&self) -> Size<i32, Physical> {
        self.backend.window_size()
    }

    pub fn output(&self) -> &Output {
        &self.output
    }

    pub fn dmabuf_capabilities(&mut self) -> super::dmabuf::DmabufCapabilities {
        let renderer = self.backend.renderer();
        let formats = renderer.dmabuf_formats();
        let main_device = EGLDevice::device_for_display(renderer.egl_context().display())
            .and_then(|device| device.try_get_render_node())
            .ok()
            .flatten()
            .map(|node| node.dev_id());

        super::dmabuf::DmabufCapabilities::new(main_device, formats)
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        match self.backend.renderer().import_dmabuf(dmabuf, None) {
            Ok(_) => true,
            Err(err) => {
                eventline::debug!("winit: failed to import DMA-BUF: {err}");
                false
            }
        }
    }

    pub fn renderer(&mut self) -> &mut GlesRenderer {
        self.backend.renderer()
    }

    /// Keeps the advertised wl_output mode in sync with the real window
    /// size after a resize - stale output geometry would mislead clients
    /// about how large they're allowed to be.
    pub fn update_output_mode(&self) {
        self.output.change_current_state(
            Some(output_mode(self.backend.window_size())),
            None,
            None,
            None,
        );
    }

    pub fn set_display_scale(&self, scale: f64) {
        self.output.change_current_state(
            None,
            None,
            Some(smithay::output::Scale::Fractional(scale)),
            None,
        );
    }

    pub fn logical_size(&self) -> Size<i32, Physical> {
        self.window_size()
            .to_f64()
            .to_logical(self.output.current_scale().fractional_scale())
            .to_i32_ceil()
            .to_physical(1)
    }
}

impl crate::ipc::OutputInfoSource for WinitBackend {
    fn output_info(&self) -> Vec<halley_ipc::OutputInfo> {
        let location = self.output.current_location();
        let mode = self
            .output
            .current_mode()
            .expect("winit output always has a current mode");
        vec![halley_ipc::OutputInfo {
            name: self.output.name(),
            modes: vec![crate::ipc::mode_info(mode, true)],
            current_mode: Some(0),
            offset_x: location.x,
            offset_y: location.y,
            scale: self.output.current_scale().fractional_scale(),
            // The dev-mode nested backend has no real VRR/hardware concept
            // at all - config-driven output selection doesn't apply to it
            // either (see `WinitBackend`'s own doc comment).
            vrr: "off".to_string(),
            vrr_supported: false,
            vrr_active: false,
        }]
    }
}

impl Renderable for WinitBackend {
    fn render(
        &mut self,
        output: &Output,
        request: RenderRequest<'_>,
    ) -> Result<RenderOutcome, Box<dyn Error>> {
        if output != &self.output {
            return Err(format!("winit cannot render unknown output {:?}", output.name()).into());
        }
        let size = self.backend.window_size();
        let damage = Rectangle::from_size(size);
        let output_geometry = request
            .desktop
            .space
            .output_geometry(output)
            .ok_or_else(|| format!("winit output {:?} is not mapped", output.name()))?;
        let fifo = request.desktop.fifo;
        let space = request.desktop.space;
        let session_lock = request.desktop.session_lock;
        let clear = if request.desktop.session_lock.active() {
            crate::render::SESSION_LOCK_COLOR
        } else {
            request.frame.clear
        };

        // Scoped so `renderer`/`framebuffer` (both borrowed from
        // `self.backend`) are dropped before `submit()` needs its own
        // mutable borrow.
        let mut element_states = RenderElementStates::default();
        {
            let (renderer, mut framebuffer) = self.backend.bind()?;
            let elements = crate::render::scene::build(
                renderer,
                output,
                &self.output,
                output_geometry,
                request,
            )?;

            let mut frame = renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
            frame.clear(clear, &[damage])?;
            let effect_cache = UserDataMap::new();
            // Winit bypasses Smithay's output damage tracker, so explicitly
            // preserve the same back-to-front effect contract it provides on
            // DRM: capture the already-rendered backdrop immediately before
            // drawing each framebuffer effect.
            for element in elements.iter().rev() {
                let geometry = element.geometry(1.0.into());
                let Some(visible) = geometry.intersection(damage) else {
                    continue;
                };
                let local_damage = [Rectangle::new(visible.loc - geometry.loc, visible.size)];
                let cache = if element.is_framebuffer_effect() {
                    element.capture_framebuffer(
                        &mut frame,
                        element.src(),
                        geometry,
                        &effect_cache,
                    )?;
                    Some(&effect_cache)
                } else {
                    None
                };
                element.draw(
                    &mut frame,
                    element.src(),
                    geometry,
                    &local_damage,
                    &[],
                    cache,
                )?;
                element_states.states.insert(
                    element.id().clone(),
                    RenderElementState {
                        visible_area: (visible.size.w as usize)
                            .saturating_mul(visible.size.h as usize),
                        presentation_state: RenderElementPresentationState::Rendering {
                            reason: None,
                        },
                        needs_capture: false,
                    },
                );
            }
            crate::render::window_open::extend_render_states(&elements, &mut element_states);
            let _ = frame.finish()?;
        }

        self.backend.submit(Some(&[damage]))?;

        let mut presentation_feedback = crate::wayland::presentation::take_output_feedback(
            output,
            &self.output,
            space,
            session_lock,
            &element_states,
        );
        let refresh = output
            .current_mode()
            .map(|mode| {
                Refresh::fixed(std::time::Duration::from_secs_f64(
                    1_000.0 / mode.refresh as f64,
                ))
            })
            .unwrap_or(Refresh::Unknown);
        presentation_feedback.presented(
            smithay::utils::Clock::<smithay::utils::Monotonic>::new().now(),
            refresh,
            0,
            smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::Vsync,
        );

        let fifo_barriers = fifo.snapshot(&element_states);
        Ok(
            RenderOutcome::new(RenderStatus::Submitted, Some(element_states))
                .with_fifo(fifo_barriers),
        )
    }
}
