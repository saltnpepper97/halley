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

/// Fraction of a rectangular footprint centred on `pos` for which `inside`
/// is false, sampled at the centre of a 10x10 grid. The dense grid makes a
/// 90% threshold stable and deterministic for any boundary shape.
fn footprint_outside_fraction(pos: Vec2, footprint: Vec2, inside: impl Fn(Vec2) -> bool) -> f32 {
    let w = footprint.x.abs();
    let h = footprint.y.abs();
    if w < 1.0 || h < 1.0 {
        return if inside(pos) { 0.0 } else { 1.0 };
    }

    const SAMPLES: usize = 10;
    let min_x = pos.x - w * 0.5;
    let min_y = pos.y - h * 0.5;
    let mut outside = 0usize;
    for iy in 0..SAMPLES {
        for ix in 0..SAMPLES {
            let p = Vec2 {
                x: min_x + (ix as f32 + 0.5) / SAMPLES as f32 * w,
                y: min_y + (iy as f32 + 0.5) / SAMPLES as f32 * h,
            };
            outside += usize::from(!inside(p));
        }
    }
    outside as f32 / (SAMPLES * SAMPLES) as f32
}

impl Viewport {
    /// Whether a Field point lies within the visible rectangle.
    pub fn contains(&self, p: Vec2) -> bool {
        let rect = self.rect();
        p.x >= rect.min.x && p.x <= rect.max.x && p.y >= rect.min.y && p.y <= rect.max.y
    }

    /// Fraction of a window footprint that lies outside the visible area.
    pub fn outside_fraction(&self, pos: Vec2, footprint: Vec2) -> f32 {
        footprint_outside_fraction(pos, footprint, |p| self.contains(p))
    }
}

/// An optional decay zone: an axis-aligned ellipse with an offset relative
/// to the viewport centre. Windows mostly outside it take the shorter decay
/// delay instead of the screen edge deciding.
///
/// We use normalized ellipse distance:
///   d2 = (x/rx)^2 + (y/ry)^2
/// If d2 <= 1 => inside.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecayZone {
    pub radius_x: f32,
    pub radius_y: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

impl DecayZone {
    pub fn new(radius_x: f32, radius_y: f32, offset_x: f32, offset_y: f32) -> Self {
        Self {
            radius_x,
            radius_y,
            offset_x,
            offset_y,
        }
    }

    pub fn contains(&self, center: Vec2, p: Vec2) -> bool {
        self.normalized_distance2(center, p) <= 1.0
    }

    /// Return normalized squared distance inside this ellipse:
    /// d2 = (x/rx)^2 + (y/ry)^2
    /// - d2 <= 1.0: inside/on boundary
    /// - d2 > 1.0: outside
    pub fn normalized_distance2(&self, center: Vec2, p: Vec2) -> f32 {
        let zone_center = Vec2 {
            x: center.x + self.offset_x,
            y: center.y + self.offset_y,
        };

        let dx = p.x - zone_center.x;
        let dy = p.y - zone_center.y;
        let rx = self.radius_x.max(0.0001);
        let ry = self.radius_y.max(0.0001);

        let nx = dx / rx;
        let ny = dy / ry;

        nx * nx + ny * ny
    }

    /// Fraction of a rectangular footprint that falls outside this zone.
    pub fn outside_fraction(&self, vp_center: Vec2, pos: Vec2, footprint: Vec2) -> f32 {
        footprint_outside_fraction(pos, footprint, |p| self.contains(vp_center, p))
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
    fn decay_zone_contains_axis_aligned() {
        let zone = DecayZone::new(10.0, 5.0, 0.0, 0.0);
        let c = Vec2 { x: 0.0, y: 0.0 };

        assert!(zone.contains(c, Vec2 { x: 0.0, y: 0.0 }));
        assert!(zone.contains(c, Vec2 { x: 10.0, y: 0.0 }));
        assert!(zone.contains(c, Vec2 { x: 0.0, y: 5.0 }));

        assert!(!zone.contains(c, Vec2 { x: 10.01, y: 0.0 }));
        assert!(!zone.contains(c, Vec2 { x: 0.0, y: 5.01 }));
    }

    #[test]
    fn decay_zone_respects_offset() {
        let zone = DecayZone::new(10.0, 5.0, 4.0, -2.0);
        let c = Vec2 { x: 0.0, y: 0.0 };

        assert!(zone.contains(c, Vec2 { x: 4.0, y: -2.0 }));
        assert!(zone.contains(c, Vec2 { x: 14.0, y: -2.0 }));
        assert!(zone.contains(c, Vec2 { x: 4.0, y: 3.0 }));

        assert!(!zone.contains(c, Vec2 { x: 14.01, y: -2.0 }));
        assert!(!zone.contains(c, Vec2 { x: 4.0, y: 3.01 }));
    }

    #[test]
    fn viewport_outside_fraction_measures_the_visible_rectangle() {
        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let size = Vec2 { x: 20.0, y: 10.0 };

        assert_eq!(vp.outside_fraction(Vec2 { x: 0.0, y: 0.0 }, size), 0.0);
        assert_eq!(vp.outside_fraction(Vec2 { x: 500.0, y: 0.0 }, size), 1.0);
        // Straddling the right edge: half the footprint is off screen.
        assert_eq!(vp.outside_fraction(Vec2 { x: 50.0, y: 0.0 }, size), 0.5);
    }
}
