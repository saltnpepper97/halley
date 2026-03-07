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

/// Which focus zone a point is in (relative to a viewport center).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusZone {
    Inside,
    Outside,
}

/// A focus ring modeled as a rotated ellipse in Field coordinates.
///
/// We use normalized ellipse distance:
///   d2 = (x'/rx)^2 + (y'/ry)^2
/// If d2 <= 1 => inside.
///
/// This is deterministic, cheap, and keeps the current ellipse/eye-like model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusRing {
    pub radius_x: f32,
    pub radius_y: f32,
    pub rotation_rad: f32,
}

impl FocusRing {
    pub fn new(radius_x: f32, radius_y: f32, rotation_rad: f32) -> Self {
        Self {
            radius_x,
            radius_y,
            rotation_rad,
        }
    }

    pub fn contains(&self, center: Vec2, p: Vec2) -> bool {
        self.normalized_distance2(center, p) <= 1.0
    }

    pub fn zone(&self, vp_center: Vec2, p: Vec2) -> FocusZone {
        if self.contains(vp_center, p) {
            FocusZone::Inside
        } else {
            FocusZone::Outside
        }
    }

    /// Return normalized squared distance inside this ellipse:
    /// d2 = (x'/rx)^2 + (y'/ry)^2
    /// - d2 <= 1.0: inside/on boundary
    /// - d2 > 1.0: outside
    pub fn normalized_distance2(&self, center: Vec2, p: Vec2) -> f32 {
        let dx = p.x - center.x;
        let dy = p.y - center.y;

        let (s, c) = self.rotation_rad.sin_cos();

        // Rotate into focus-ring-local space.
        let x = c * dx + s * dy;
        let y = -s * dx + c * dy;

        let rx = self.radius_x.max(0.0001);
        let ry = self.radius_y.max(0.0001);

        let nx = x / rx;
        let ny = y / ry;

        nx * nx + ny * ny
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
    fn focus_ring_contains_axis_aligned() {
        let ring = FocusRing::new(10.0, 5.0, 0.0);
        let c = Vec2 { x: 0.0, y: 0.0 };

        assert!(ring.contains(c, Vec2 { x: 0.0, y: 0.0 }));
        assert!(ring.contains(c, Vec2 { x: 10.0, y: 0.0 }));
        assert!(ring.contains(c, Vec2 { x: 0.0, y: 5.0 }));

        assert!(!ring.contains(c, Vec2 { x: 10.01, y: 0.0 }));
        assert!(!ring.contains(c, Vec2 { x: 0.0, y: 5.01 }));
    }

    #[test]
    fn focus_zone_classifies() {
        let ring = FocusRing::new(10.0, 10.0, 0.0);
        let c = Vec2 { x: 0.0, y: 0.0 };

        assert_eq!(ring.zone(c, Vec2 { x: 0.0, y: 0.0 }), FocusZone::Inside);
        assert_eq!(ring.zone(c, Vec2 { x: 20.0, y: 0.0 }), FocusZone::Outside);
    }
}
