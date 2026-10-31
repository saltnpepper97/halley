//! Text-input-v3 and input-method-v2 policy.
//!
//! Smithay owns the protocol objects. This module supplies parent geometry,
//! IME candidate-window placement, and popup tracking so composition can
//! follow keyboard focus.

use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::desktop::{PopupKind, PopupManager, Window, WindowSurfaceType, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::compositor::{send_surface_state, with_states};
use smithay::wayland::fractional_scale::with_fractional_scale;
use smithay::wayland::input_method::{InputMethodHandler, PopupSurface};
use smithay::wayland::seat::WaylandFocus;

use crate::session::{Session, SessionDriver};

use super::super::{WaylandState, compositor};

/// Chooses a surface-local parent rectangle for IME popup placement.
///
/// Window geometry wins over layer-shell and session-lock sizes so a focused
/// field inside a window is never treated as a panel or lock surface.
pub fn parent_geometry_for(
    window: Option<Rectangle<i32, Logical>>,
    layer: Option<Size<i32, Logical>>,
    lock: Option<Size<i32, Logical>>,
) -> Rectangle<i32, Logical> {
    if let Some(geometry) = window {
        return geometry;
    }
    if let Some(size) = layer {
        return Rectangle::from_size(size);
    }
    if let Some(size) = lock {
        return Rectangle::from_size(size);
    }
    Rectangle::default()
}

/// Places an IME candidate window relative to the caret inside `parent`.
///
/// Prefer below the caret when the popup fits; otherwise place above. Clamp
/// horizontally so the popup starts inside the parent.
pub fn ime_popup_location(
    parent: Rectangle<i32, Logical>,
    cursor: Rectangle<i32, Logical>,
    popup_size: Size<i32, Logical>,
) -> Point<i32, Logical> {
    let mut x = cursor.loc.x;
    let overflow_x = (x + popup_size.w) - (parent.loc.x + parent.size.w);
    if overflow_x > 0 {
        x -= overflow_x;
    }
    x = x.max(parent.loc.x);

    let below_y = cursor.loc.y + cursor.size.h;
    let y = if parent.loc.y + parent.size.h >= below_y + popup_size.h {
        below_y
    } else {
        cursor.loc.y - popup_size.h
    };

    Point::from((x, y))
}

pub fn handle_popup_commit<D: SessionDriver>(session: &mut Session<D>, surface: &WlSurface) {
    let Some(PopupKind::InputMethod(popup)) = session.wayland.popup_manager.find_popup(surface)
    else {
        return;
    };
    position_ime_popup(session, popup);
}

fn window_for_surface<'a>(wayland: &'a WaylandState, surface: &WlSurface) -> Option<&'a Window> {
    let root = compositor::root_surface(surface);
    wayland
        .space
        .elements()
        .find(|window| {
            window
                .wl_surface()
                .is_some_and(|candidate| candidate.as_ref() == &root)
        })
        .or_else(|| wayland.unmapped.get(&root))
        .or_else(|| wayland.collapsed.get(&root))
}

fn layer_size(wayland: &WaylandState, surface: &WlSurface) -> Option<Size<i32, Logical>> {
    let root = compositor::root_surface(surface);
    wayland.space.outputs().find_map(|output| {
        let map = layer_map_for_output(output);
        let layer = map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)?;
        Some(map.layer_geometry(layer)?.size)
    })
}

fn session_parent_geometry<D: SessionDriver>(
    session: &Session<D>,
    parent: &WlSurface,
) -> Rectangle<i32, Logical> {
    parent_geometry_for(
        window_for_surface(&session.wayland, parent).map(Window::geometry),
        layer_size(&session.wayland, parent),
        session
            .session_lock
            .geometry_for_surface(parent)
            .map(|geometry| geometry.size),
    )
}

fn output_for_parent<D: SessionDriver>(session: &Session<D>, parent: &WlSurface) -> Option<Output> {
    let root = compositor::root_surface(parent);
    if let Some(window) = window_for_surface(&session.wayland, parent)
        && let Some(name) = super::super::window_output_name(window)
        && let Some(output) = session
            .wayland
            .space
            .outputs()
            .find(|output| output.name() == name)
            .cloned()
    {
        return Some(output);
    }
    if let Some(output) = session.wayland.space.outputs().find_map(|output| {
        let map = layer_map_for_output(output);
        map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)
            .map(|_| output.clone())
    }) {
        return Some(output);
    }
    if let Some(output) = session.session_lock.output_for_surface(parent).cloned() {
        return Some(output);
    }
    session
        .wayland
        .focused_output
        .as_ref()
        .and_then(|name| {
            session
                .wayland
                .space
                .outputs()
                .find(|output| output.name() == *name)
                .cloned()
        })
        .or_else(|| session.wayland.space.outputs().next().cloned())
}

fn send_popup_scale_transform(surface: &WlSurface, output: &Output) {
    with_states(surface, |data| {
        send_surface_state(
            surface,
            data,
            output.current_scale().integer_scale(),
            output.current_transform(),
        );
        with_fractional_scale(data, |fractional| {
            fractional.set_preferred_scale(output.current_scale().fractional_scale());
        });
    });
}

fn position_ime_popup<D: SessionDriver>(session: &Session<D>, popup: PopupSurface) {
    let Some(parent) = popup.get_parent().map(|parent| parent.surface.clone()) else {
        return;
    };
    let parent_geometry = session_parent_geometry(session, &parent);
    let cursor = popup.text_input_rectangle();
    let popup_size = bbox_from_surface_tree(popup.wl_surface(), cursor.loc).size;
    popup.set_location(ime_popup_location(parent_geometry, cursor, popup_size));
}

impl<D: SessionDriver> InputMethodHandler for Session<D> {
    fn new_popup(&mut self, surface: PopupSurface) {
        if let Some(parent) = surface.get_parent().map(|parent| parent.surface.clone())
            && let Some(output) = output_for_parent(self, &parent)
        {
            send_popup_scale_transform(surface.wl_surface(), &output);
        }
        position_ime_popup(self, surface.clone());
        if let Err(err) = self
            .wayland
            .popup_manager
            .track_popup(PopupKind::InputMethod(surface))
        {
            eventline::warn!("input-method: failed to track popup: {err}");
        }
        self.request_redraw();
    }

    fn popup_repositioned(&mut self, surface: PopupSurface) {
        position_ime_popup(self, surface);
        self.request_redraw();
    }

    fn dismiss_popup(&mut self, surface: PopupSurface) {
        if let Some(parent) = surface.get_parent().map(|parent| parent.surface.clone()) {
            let _ = PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
        }
        self.request_redraw();
    }

    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, Logical> {
        session_parent_geometry(self, parent)
    }
}

#[cfg(test)]
mod tests {
    use super::{ime_popup_location, parent_geometry_for};
    use smithay::utils::{Logical, Point, Rectangle, Size};

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((x, y)), Size::from((w, h)))
    }

    #[test]
    fn parent_geometry_prefers_window_then_layer_then_lock() {
        assert_eq!(
            parent_geometry_for(
                Some(rect(2, 3, 40, 50)),
                Some((10, 10).into()),
                Some((8, 8).into())
            ),
            rect(2, 3, 40, 50)
        );
        assert_eq!(
            parent_geometry_for(None, Some((320, 48).into()), Some((1920, 1080).into())),
            rect(0, 0, 320, 48)
        );
        assert_eq!(
            parent_geometry_for(None, None, Some((1920, 1080).into())),
            rect(0, 0, 1920, 1080)
        );
        assert_eq!(parent_geometry_for(None, None, None), Rectangle::default());
    }

    #[test]
    fn ime_popup_sits_below_the_caret_when_it_fits() {
        let parent = rect(0, 0, 200, 200);
        let cursor = rect(20, 40, 8, 16);
        let location = ime_popup_location(parent, cursor, Size::from((80, 40)));
        assert_eq!(location, Point::from((20, 56)));
    }

    #[test]
    fn ime_popup_moves_above_when_below_would_overflow() {
        let parent = rect(0, 0, 200, 80);
        let cursor = rect(20, 50, 8, 16);
        let location = ime_popup_location(parent, cursor, Size::from((80, 40)));
        assert_eq!(location, Point::from((20, 10)));
    }

    #[test]
    fn ime_popup_clamps_horizontally_to_the_parent() {
        let parent = rect(0, 0, 100, 200);
        let cursor = rect(70, 20, 8, 16);
        let location = ime_popup_location(parent, cursor, Size::from((80, 20)));
        assert_eq!(location, Point::from((20, 36)));
    }
}
