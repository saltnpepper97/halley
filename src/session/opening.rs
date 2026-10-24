use std::collections::HashMap;
use std::time::Duration;

use smithay::desktop::{WindowSurfaceType, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle};
use smithay::wayland::seat::WaylandFocus;

use super::{Session, SessionDriver};

#[derive(Clone, Copy, Debug)]
pub(super) struct ActivationOrigin(pub Point<f64, Logical>);

pub(super) struct OriginOnlyActivation;

#[derive(Default)]
pub struct OpeningOrigins {
    pending: HashMap<WlSurface, Point<f64, Logical>>,
    active: HashMap<WlSurface, Point<f64, Logical>>,
}

impl OpeningOrigins {
    fn remember_fallback(&mut self, surface: WlSurface, origin: Point<f64, Logical>) {
        self.pending.entry(surface).or_insert(origin);
    }

    pub(crate) fn remember_launcher(&mut self, surface: WlSurface, origin: Point<f64, Logical>) {
        self.pending.insert(surface, origin);
    }

    pub(crate) fn forget(&mut self, surface: &WlSurface) {
        self.pending.remove(surface);
        self.active.remove(surface);
    }

    pub(crate) fn reassociate_surface(&mut self, previous: &WlSurface, surface: &WlSurface) {
        crate::animation::reassociate_entry(&mut self.pending, previous, surface);
        crate::animation::reassociate_entry(&mut self.active, previous, surface);
    }

    fn activate(
        &mut self,
        surface: &WlSurface,
        fallback: Option<Point<f64, Logical>>,
    ) -> Option<Point<f64, Logical>> {
        let origin = self.pending.remove(surface).or(fallback);
        if let Some(origin) = origin {
            self.active.insert(surface.clone(), origin);
        }
        origin
    }

    pub(crate) fn active(&self, surface: &WlSurface) -> Option<Point<f64, Logical>> {
        self.active.get(surface).copied()
    }
}

pub(crate) fn prepare<D: SessionDriver>(
    session: &mut Session<D>,
    surface: WlSurface,
    output: &Output,
) {
    if let Some(origin) = fallback_origin(session, output) {
        session.opening_origins.remember_fallback(surface, origin);
    }
}

pub(crate) fn start<D: SessionDriver>(
    session: &mut Session<D>,
    surface: WlSurface,
    output: &Output,
    now: Duration,
) -> bool {
    let Some(output_geometry) = session.wayland.space.output_geometry(output) else {
        session.opening_origins.forget(&surface);
        return session.window_animations.start(surface, now);
    };
    let fallback = fallback_origin(session, output);
    let global = session.opening_origins.activate(&surface, fallback);
    let local = global.map(|origin| {
        Point::<f64, Physical>::from((
            origin.x - f64::from(output_geometry.loc.x),
            origin.y - f64::from(output_geometry.loc.y),
        ))
    });
    session
        .window_animations
        .start_with_origin(surface, now, local)
}

pub(super) fn output_for_surface<D: SessionDriver>(
    session: &Session<D>,
    surface: &WlSurface,
) -> Option<Output> {
    let window = session.wayland.space.elements().find(|window| {
        window
            .wl_surface()
            .is_some_and(|candidate| candidate.as_ref() == surface)
    })?;
    crate::wayland::window_output_name(window)
        .and_then(|name| {
            session
                .wayland
                .space
                .outputs()
                .find(|output| output.name() == name)
        })
        .or_else(|| crate::wayland::focus::selected_output(&session.wayland))
        .cloned()
}

pub(super) fn surface_visual_center<D: SessionDriver>(
    session: &Session<D>,
    surface: &WlSurface,
) -> Option<Point<f64, Logical>> {
    let now = crate::frame_clock::monotonic_now();
    if let Some(presentation) = crate::presentation::window::WindowPresentation::for_surface(
        &session.wayland.space,
        &session.cameras,
        Some(&session.clusters),
        Some(&session.nodes),
        session.driver.primary_output(),
        &session.window_animations,
        &session.fullscreen,
        &session.maximize,
        &session.settings.decorations,
        &session.settings.font,
        surface,
        now,
    ) {
        return Some(rect_center(presentation.visual_geometry()));
    }

    let root = crate::wayland::compositor::root_surface(surface);
    session.wayland.space.outputs().find_map(|output| {
        let map = layer_map_for_output(output);
        let layer = map.layer_for_surface(&root, WindowSurfaceType::ALL)?;
        let geometry = map.layer_geometry(layer)?;
        let output_geometry = session.wayland.space.output_geometry(output)?;
        Some(Point::from((
            f64::from(output_geometry.loc.x + geometry.loc.x) + f64::from(geometry.size.w) / 2.0,
            f64::from(output_geometry.loc.y + geometry.loc.y) + f64::from(geometry.size.h) / 2.0,
        )))
    })
}

pub(super) fn fallback_origin<D: SessionDriver>(
    session: &Session<D>,
    output: &Output,
) -> Option<Point<f64, Logical>> {
    let output_geometry = session.wayland.space.output_geometry(output)?;
    let cursor = Point::<f64, Logical>::from(session.pointer.position());
    let focused = session
        .wayland
        .focused_window
        .as_ref()
        .and_then(|surface| surface_visual_center(session, surface));
    Some(choose_fallback_origin(output_geometry, cursor, focused))
}

fn choose_fallback_origin(
    output: Rectangle<i32, Logical>,
    cursor: Point<f64, Logical>,
    focused_window: Option<Point<f64, Logical>>,
) -> Point<f64, Logical> {
    if cursor.x.is_finite() && cursor.y.is_finite() {
        cursor
    } else if let Some(focused_window) = focused_window {
        focused_window
    } else {
        rect_center(output)
    }
}

fn rect_center(rect: Rectangle<i32, Logical>) -> Point<f64, Logical> {
    Point::from((
        f64::from(rect.loc.x) + f64::from(rect.size.w) / 2.0,
        f64::from(rect.loc.y) + f64::from(rect.size.h) / 2.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_on_target_output_precedes_focused_window() {
        let output = Rectangle::new((2560, 0).into(), (1920, 1200).into());
        let cursor = Point::from((3000.0, 500.0));
        let focused = Point::from((800.0, 600.0));

        assert_eq!(
            choose_fallback_origin(output, cursor, Some(focused)),
            cursor
        );
    }

    #[test]
    fn cursor_on_another_output_preserves_launch_direction() {
        let output = Rectangle::new((2560, 0).into(), (1920, 1200).into());
        let cursor = Point::from((100.0, 100.0));
        let focused = Point::from((800.0, 600.0));

        assert_eq!(
            choose_fallback_origin(output, cursor, Some(focused)),
            cursor
        );
    }

    #[test]
    fn focused_window_is_used_when_the_cursor_is_invalid() {
        let output = Rectangle::new((2560, 0).into(), (1920, 1200).into());
        let focused = Point::from((800.0, 600.0));

        assert_eq!(
            choose_fallback_origin(output, Point::from((f64::NAN, 100.0)), Some(focused)),
            focused
        );
    }

    #[test]
    fn output_center_is_the_last_fallback_for_an_invalid_cursor() {
        let output = Rectangle::new((2560, 0).into(), (1920, 1200).into());

        assert_eq!(
            choose_fallback_origin(output, Point::from((f64::INFINITY, 100.0)), None),
            Point::from((3520.0, 600.0))
        );
    }
}
