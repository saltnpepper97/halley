//! Which decay delay a window takes: on screen or off screen.
//!
//! By default the screen edge decides. A window counts as off screen once
//! [`OFF_SCREEN_THRESHOLD`] of its footprint lies outside what its output
//! currently shows, so zooming out keeps everything visible on the long delay.
//! An optional per-output `decay-zone` replaces the screen edge with an
//! ellipse measured in screen pixels, so it covers the same part of the
//! screen at every zoom level.

use halley_core::decay::DecayClass;
use halley_core::field::Vec2;
use halley_core::viewport::{DecayZone, Viewport};

/// Share of a window's footprint that must be outside the boundary before
/// the window takes the shorter, off-screen delay.
pub(crate) const OFF_SCREEN_THRESHOLD: f32 = 0.90;

/// What one output currently shows of the Field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecayView {
    /// Camera centre in global Field coordinates.
    pub center: Vec2,
    /// Visible width and height in Field units.
    pub size: Vec2,
    /// Screen pixels per Field unit (1.0 at rest, smaller when zoomed out).
    pub scale: f32,
}

/// Classifies a window footprint centred on `pos` for decay timing.
///
/// A window whose output has no view (for example, a disconnected monitor)
/// is not on any screen, so it takes the off-screen delay.
pub(crate) fn classify(
    view: Option<DecayView>,
    zone: Option<halley_config::DecayZone>,
    pos: Vec2,
    footprint: Vec2,
) -> DecayClass {
    let Some(view) = view else {
        return DecayClass::OffScreen;
    };
    let outside = match zone {
        Some(zone) => {
            // The zone is configured in screen pixels; convert to Field units.
            let per_pixel = 1.0 / view.scale.max(f32::EPSILON);
            DecayZone::new(
                zone.radius_x * per_pixel,
                zone.radius_y * per_pixel,
                zone.offset_x * per_pixel,
                zone.offset_y * per_pixel,
            )
            .outside_fraction(view.center, pos, footprint)
        }
        None => Viewport::new(view.center, view.size).outside_fraction(pos, footprint),
    };
    if outside >= OFF_SCREEN_THRESHOLD {
        DecayClass::OffScreen
    } else {
        DecayClass::OnScreen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Vec2 = Vec2 { x: 800.0, y: 600.0 };

    /// A 2560x1440 output looking at the Field origin at `scale`.
    fn view(scale: f32) -> DecayView {
        DecayView {
            center: Vec2 { x: 0.0, y: 0.0 },
            size: Vec2 {
                x: 2560.0 / scale,
                y: 1440.0 / scale,
            },
            scale,
        }
    }

    /// A window whose top-left corner sits just inside the visible
    /// bottom-right corner, far outside the old 820x420 ring.
    fn corner_window(view: DecayView) -> Vec2 {
        Vec2 {
            x: view.size.x * 0.5 - WINDOW.x * 0.5 - 10.0,
            y: view.size.y * 0.5 - WINDOW.y * 0.5 - 10.0,
        }
    }

    #[test]
    fn fully_visible_corner_window_is_on_screen_when_zoomed_out() {
        let view = view(0.35);
        let pos = corner_window(view);
        assert_eq!(
            classify(Some(view), None, pos, WINDOW),
            DecayClass::OnScreen
        );
    }

    #[test]
    fn window_mostly_past_the_screen_edge_is_off_screen() {
        let view = view(1.0);
        let mostly_off = Vec2 {
            x: view.size.x * 0.5 + WINDOW.x * 0.45,
            y: 0.0,
        };
        let half_off = Vec2 {
            x: view.size.x * 0.5,
            y: 0.0,
        };
        assert_eq!(
            classify(Some(view), None, mostly_off, WINDOW),
            DecayClass::OffScreen
        );
        assert_eq!(
            classify(Some(view), None, half_off, WINDOW),
            DecayClass::OnScreen
        );
    }

    #[test]
    fn decay_zone_covers_the_same_screen_area_at_every_zoom() {
        let zone = halley_config::DecayZone {
            radius_x: 400.0,
            radius_y: 300.0,
            offset_x: 0.0,
            offset_y: 0.0,
        };
        for scale in [1.0, 0.5, 0.35] {
            let view = view(scale);
            // 600 screen pixels right of centre is outside a 400px zone.
            let outside = Vec2 {
                x: 600.0 / scale,
                y: 0.0,
            };
            // 100 screen pixels right of centre is inside it.
            let inside = Vec2 {
                x: 100.0 / scale,
                y: 0.0,
            };
            let small = Vec2 { x: 1.0, y: 1.0 };
            assert_eq!(
                classify(Some(view), Some(zone), outside, small),
                DecayClass::OffScreen,
                "scale {scale}"
            );
            assert_eq!(
                classify(Some(view), Some(zone), inside, small),
                DecayClass::OnScreen,
                "scale {scale}"
            );
        }
    }

    #[test]
    fn window_without_a_view_is_off_screen() {
        assert_eq!(
            classify(None, None, Vec2 { x: 0.0, y: 0.0 }, WINDOW),
            DecayClass::OffScreen
        );
    }
}
