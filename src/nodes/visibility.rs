//! Whether a window counts as off screen for automatic decay.
//!
//! Only a window entirely outside what its output currently shows decays. A
//! window with any part on screen, at any zoom level, never counts down.

use halley_core::decay::DecayClass;
use halley_core::field::Vec2;
use halley_core::viewport::Viewport;

/// What one output currently shows of the Field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecayView {
    /// Camera centre in global Field coordinates.
    pub center: Vec2,
    /// Visible width and height in Field units.
    pub size: Vec2,
}

/// Classifies a window footprint centred on `pos` for decay timing.
///
/// A window whose output has no view (for example, a disconnected monitor)
/// is not on any screen, so it counts as off screen.
pub(crate) fn classify(view: Option<DecayView>, pos: Vec2, footprint: Vec2) -> DecayClass {
    match view {
        Some(view) if Viewport::new(view.center, view.size).shows(pos, footprint) => {
            DecayClass::OnScreen
        }
        _ => DecayClass::OffScreen,
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
        }
    }

    #[test]
    fn corner_window_is_on_screen_when_zoomed_out() {
        let view = view(0.35);
        let pos = Vec2 {
            x: view.size.x * 0.5 - WINDOW.x * 0.5 - 10.0,
            y: view.size.y * 0.5 - WINDOW.y * 0.5 - 10.0,
        };
        assert_eq!(classify(Some(view), pos, WINDOW), DecayClass::OnScreen);
    }

    #[test]
    fn a_window_with_any_part_on_screen_is_on_screen() {
        let view = view(1.0);
        // Only 10 pixels of the window's left edge remain visible.
        let sliver = Vec2 {
            x: view.size.x * 0.5 + WINDOW.x * 0.5 - 10.0,
            y: 0.0,
        };
        assert_eq!(classify(Some(view), sliver, WINDOW), DecayClass::OnScreen);
    }

    #[test]
    fn a_window_entirely_past_the_screen_edge_is_off_screen() {
        let view = view(1.0);
        let touching = Vec2 {
            x: view.size.x * 0.5 + WINDOW.x * 0.5,
            y: 0.0,
        };
        let far = Vec2 {
            x: view.size.x * 3.0,
            y: 0.0,
        };
        assert_eq!(
            classify(Some(view), touching, WINDOW),
            DecayClass::OffScreen
        );
        assert_eq!(classify(Some(view), far, WINDOW), DecayClass::OffScreen);
    }

    #[test]
    fn window_without_a_view_is_off_screen() {
        assert_eq!(
            classify(None, Vec2 { x: 0.0, y: 0.0 }, WINDOW),
            DecayClass::OffScreen
        );
    }
}
