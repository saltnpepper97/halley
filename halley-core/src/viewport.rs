use crate::field::{Rect, Vec2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Center position in Field coordinates.
    pub center: Vec2,

    /// Size of the visible region in Field coordinates.
    pub size: Vec2,

    /// Home position for Return.
    pub home: Vec2,
}

impl Viewport {
    pub fn new(center: Vec2, size: Vec2) -> Self {
        Self {
            center,
            size,
            home: center,
        }
    }

    /// Axis-aligned view rectangle in Field space.
    pub fn rect(&self) -> Rect {
        let half = Vec2 {
            x: self.size.x * 0.5,
            y: self.size.y * 0.5,
        };

        Rect {
            min: Vec2 {
                x: self.center.x - half.x,
                y: self.center.y - half.y,
            },
            max: Vec2 {
                x: self.center.x + half.x,
                y: self.center.y + half.y,
            },
        }
    }

    /// Move camera to a new center.
    pub fn move_to(&mut self, center: Vec2) {
        self.center = center;
    }

    /// Offset camera by delta.
    pub fn pan(&mut self, delta: Vec2) {
        self.center.x += delta.x;
        self.center.y += delta.y;
    }

    /// Set current position as home.
    pub fn set_home(&mut self) {
        self.home = self.center;
    }

    /// Return to home position.
    pub fn return_home(&mut self) {
        self.center = self.home;
    }
}

impl Viewport {
    /// Whether a Field point lies within the visible rectangle.
    pub fn contains(&self, p: Vec2) -> bool {
        let rect = self.rect();
        p.x >= rect.min.x && p.x <= rect.max.x && p.y >= rect.min.y && p.y <= rect.max.y
    }

    /// Whether any part of a footprint centred on `pos` is visible. A
    /// footprint that only touches the edge shows nothing, so it is not.
    pub fn shows(&self, pos: Vec2, footprint: Vec2) -> bool {
        let view = self.rect();
        let half_w = footprint.x.abs() * 0.5;
        let half_h = footprint.y.abs() * 0.5;
        pos.x - half_w < view.max.x
            && pos.x + half_w > view.min.x
            && pos.y - half_h < view.max.y
            && pos.y + half_h > view.min.y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_is_correct() {
        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });

        let r = vp.rect();

        assert_eq!(r.min, Vec2 { x: -50.0, y: -25.0 });
        assert_eq!(r.max, Vec2 { x: 50.0, y: 25.0 });
    }

    #[test]
    fn return_home_works() {
        let mut vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });

        vp.pan(Vec2 { x: 10.0, y: 5.0 });
        assert_eq!(vp.center, Vec2 { x: 10.0, y: 5.0 });

        vp.return_home();
        assert_eq!(vp.center, Vec2 { x: 0.0, y: 0.0 });
    }

    #[test]
    fn shows_any_visible_part_of_a_footprint() {
        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let size = Vec2 { x: 20.0, y: 10.0 };

        assert!(vp.shows(Vec2 { x: 0.0, y: 0.0 }, size));
        // Straddling the right edge, and one unit still on screen.
        assert!(vp.shows(Vec2 { x: 50.0, y: 0.0 }, size));
        assert!(vp.shows(Vec2 { x: 59.0, y: 0.0 }, size));
        // Touching the edge from outside, and well past it.
        assert!(!vp.shows(Vec2 { x: 60.0, y: 0.0 }, size));
        assert!(!vp.shows(Vec2 { x: 0.0, y: -500.0 }, size));
    }
}
