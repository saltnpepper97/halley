//! Window capture geometry using only the presentation inputs it needs.

use smithay::desktop::Window;
use smithay::output::Output;
use smithay::utils::{Logical, Physical, Point, Rectangle, Size};
use smithay::wayland::seat::WaylandFocus;

use crate::session::{Session, SessionDriver};

pub(crate) struct WindowCaptureContext<'a> {
    wayland: &'a crate::wayland::WaylandState,
    primary_output: &'a Output,
    fullscreen: &'a crate::wayland::fullscreen::FullscreenManager,
    decorations: &'a halley_config::Decorations,
    font: &'a halley_config::Font,
}

/// The session adapter stays here; geometry decisions only receive these inputs.
pub(crate) fn window_capture_context<D: SessionDriver>(
    session: &Session<D>,
) -> WindowCaptureContext<'_> {
    WindowCaptureContext {
        wayland: &session.wayland,
        primary_output: session.driver.primary_output(),
        fullscreen: &session.fullscreen,
        decorations: &session.settings.decorations,
        font: &session.settings.font,
    }
}

impl WindowCaptureContext<'_> {
    pub(crate) fn chrome_visible(&self, window: &Window) -> bool {
        !crate::xwayland::is_fullscreen(window)
            && window
                .wl_surface()
                .is_none_or(|surface| !self.fullscreen.suppresses_chrome(surface.as_ref()))
    }

    pub(crate) fn size(&self, window: &Window) -> Size<i32, Physical> {
        let client = window.geometry().size;
        let size = if self.chrome_visible(window) {
            crate::titlebar::outer_size_for_client(window, client, self.decorations, self.font)
        } else {
            client
        };
        size.to_f64()
            .to_physical(self.display_scale(window))
            .to_i32_round()
    }

    pub(crate) fn display_scale(&self, window: &Window) -> f64 {
        window
            .wl_surface()
            .and_then(|surface| {
                crate::wayland::display_scale::output_for_surface(self.wayland, surface.as_ref())
            })
            .unwrap_or_else(|| self.primary_output.clone())
            .current_scale()
            .fractional_scale()
    }

    pub(crate) fn client_offset(&self, window: &Window) -> Point<i32, Logical> {
        if !self.chrome_visible(window) {
            return (0, 0).into();
        }
        let outer = crate::titlebar::outer_rect_for_client(
            window,
            Rectangle::new((0, 0).into(), window.geometry().size),
            self.decorations,
            self.font,
        );
        (-outer.loc.x, -outer.loc.y).into()
    }

    pub(crate) fn visual_geometry(
        &self,
        window: &Window,
        client: Rectangle<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        if !self.chrome_visible(window) {
            return client;
        }
        let native_client = Rectangle::<i32, Logical>::from_size(window.geometry().size);
        let native_outer = crate::titlebar::outer_rect_for_client(
            window,
            native_client,
            self.decorations,
            self.font,
        );
        scaled_outer_geometry(native_client, native_outer, client)
    }
}

fn scaled_outer_geometry(
    native_client: Rectangle<i32, Logical>,
    native_outer: Rectangle<i32, Logical>,
    client: Rectangle<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let scale_x = client.size.w as f64 / native_client.size.w.max(1) as f64;
    let scale_y = client.size.h as f64 / native_client.size.h.max(1) as f64;
    let left = ((native_client.loc.x - native_outer.loc.x) as f64 * scale_x).round() as i32;
    let right = ((native_outer.size.w
        - native_client.size.w
        - (native_client.loc.x - native_outer.loc.x)) as f64
        * scale_x)
        .round() as i32;
    let top = ((native_client.loc.y - native_outer.loc.y) as f64 * scale_y).round() as i32;
    let bottom = ((native_outer.size.h
        - native_client.size.h
        - (native_client.loc.y - native_outer.loc.y)) as f64
        * scale_y)
        .round() as i32;
    Rectangle::new(
        (client.loc.x - left, client.loc.y - top).into(),
        (
            client
                .size
                .w
                .saturating_add(left)
                .saturating_add(right)
                .max(1),
            client
                .size
                .h
                .saturating_add(top)
                .saturating_add(bottom)
                .max(1),
        )
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_capture_geometry_scales_asymmetric_window_chrome() {
        let native_client = Rectangle::new((0, 0).into(), (100, 50).into());
        let native_outer = Rectangle::new((-5, -10).into(), (110, 65).into());
        let displayed_client = Rectangle::new((20, 30).into(), (200, 100).into());
        assert_eq!(
            scaled_outer_geometry(native_client, native_outer, displayed_client),
            Rectangle::new((10, 10).into(), (220, 130).into())
        );
    }
}
