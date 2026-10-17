//! Focused client warps use the same presentation and hit testing as input.

use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::layer_map_for_output;
use smithay::input::pointer::{MotionEvent, PointerHandle};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::{wl_pointer::WlPointer, wl_surface::WlSurface};
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial};
use smithay::wayland::compositor::{SurfaceAttributes, with_states};
use smithay::wayland::pointer_constraints::{PointerConstraint, with_pointer_constraint};
use smithay::wayland::pointer_warp::PointerWarpHandler;
use smithay::wayland::seat::WaylandFocus;

use crate::input::pointer::{PointerRoute, PointerTarget};
use crate::presentation::window::WindowPresentation;
use crate::session::{Session, SessionDriver};

fn screen_position<D: SessionDriver>(
    session: &Session<D>,
    route: &PointerRoute,
    surface: &WlSurface,
    local: Point<f64, Logical>,
) -> Option<Point<f64, Logical>> {
    match &route.target {
        PointerTarget::Window(window) => {
            let root = window.wl_surface()?;
            let (offset, _) = super::grabbed_surface_offset(&root, surface, window.geometry().loc)?;
            let presentation = WindowPresentation::for_window(
                &session.wayland.space,
                &session.cameras,
                Some(&session.clusters),
                Some(&session.nodes),
                &session.window_animations,
                &session.fullscreen,
                &session.maximize,
                &session.settings.decorations,
                &session.settings.font,
                window,
                &route.output,
                crate::frame_clock::monotonic_now(),
            )?;
            Some(
                presentation.screen_from_source(
                    (presentation.root_source_origin() + offset).to_f64() + local,
                ),
            )
        }
        PointerTarget::Layer(layer) => {
            let geometry = session.wayland.space.output_geometry(&route.output)?;
            let map = layer_map_for_output(&route.output);
            let (offset, _) =
                super::grabbed_surface_offset(layer.wl_surface(), surface, (0, 0).into())?;
            Some((geometry.loc + map.layer_geometry(layer)?.loc + offset).to_f64() + local)
        }
        PointerTarget::Decoration { .. } | PointerTarget::Background => None,
    }
}

impl<D: SessionDriver> PointerWarpHandler for Session<D> {
    fn warp_pointer(
        &mut self,
        surface: WlSurface,
        resource: WlPointer,
        local: Point<f64, Logical>,
        serial: Serial,
    ) {
        let Some(pointer) = PointerHandle::<Self>::from_resource(&resource) else {
            return;
        };
        if !surface.is_alive()
            || self.seat.get_pointer().as_ref() != Some(&pointer)
            || pointer.current_focus().as_ref() != Some(&surface)
            || pointer.last_enter() != Some(serial)
            || !local.x.is_finite()
            || !local.y.is_finite()
            || self.session_lock.active()
            || !matches!(self.interactions.grab, crate::input::grab::Grab::None)
            || (pointer.is_grabbed() && !super::client_click_grab_active(self))
            || self.capture.is_active()
            || self.shell.apogee.is_active()
            || self.shell.focus_cycle.is_open()
            || self.shell.cluster_composer.is_active()
            || self.shell.overlays.confirmation_modal_active()
            || self.shell.overlays.basics_card_accepts_input()
        {
            return;
        }
        let Some(size) =
            with_renderer_surface_state(&surface, |state| state.surface_size()).flatten()
        else {
            return;
        };
        if !Rectangle::from_size(size).to_f64().contains(local)
            || !with_states(&surface, |states| {
                states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .input_region
                    .as_ref()
                    .is_none_or(|region| region.contains(local.to_i32_floor()))
            })
        {
            return;
        }
        let allowed = with_pointer_constraint(&surface, &pointer, |constraint| match constraint {
            Some(constraint) if constraint.is_active() => match &*constraint {
                PointerConstraint::Locked(_) => false,
                PointerConstraint::Confined(_) => constraint
                    .region()
                    .is_none_or(|region| region.contains(local.to_i32_floor())),
            },
            _ => !super::has_active_constraint(self),
        });
        if !allowed {
            return;
        }
        let Some(route) = super::route_client(self) else {
            return;
        };
        if route.focus.as_ref().map(|(surface, _)| surface) != Some(&surface)
            || self.driver.output_power(&route.output) == Some(false)
        {
            return;
        }
        let Some(screen) = screen_position(self, &route, &surface, local) else {
            return;
        };
        if !self
            .wayland
            .space
            .output_geometry(&route.output)
            .is_some_and(|geometry| geometry.to_f64().contains(screen))
        {
            return;
        }
        // Validate the destination before moving either cursor state. This
        // rejects off-output, occluded, collapsed and hidden-workspace targets.
        let Some(destination) = super::route_client_at(self, (screen.x, screen.y)) else {
            return;
        };
        if destination.output != route.output
            || destination.focus.as_ref().map(|(surface, _)| surface) != Some(&surface)
        {
            return;
        }
        self.pointer.set_position((screen.x, screen.y));
        self.cursor_policy.pointer_activity();
        pointer.motion(
            self,
            destination.focus.clone(),
            &MotionEvent {
                location: destination.location,
                serial: SERIAL_COUNTER.next_serial(),
                time: smithay::backend::input::InputTime::from_millis(
                    self.start_time.elapsed().as_millis() as u32,
                ),
            },
        );
        self.interactions.client_pointer_route =
            Some(super::ClientPointerRoute::from_route(&destination));
        super::finish_frame(self, &pointer);
        self.request_output_redraw(&destination.output);
    }
}
