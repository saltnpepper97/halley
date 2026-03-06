use crate::field::{Field, NodeId, Vec2};
use crate::viewport::Viewport;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bearing {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

impl Bearing {
    pub fn from_delta(d: Vec2) -> Self {
        // Angle in radians, -pi..pi, where 0 is +x (east)
        let a = d.y.atan2(d.x);

        // Split circle into 8 equal wedges (pi/4 each).
        // We map wedges so the result is intuitive in screen terms.
        //
        // E:  -22.5°..+22.5°
        // NE: +22.5°..+67.5°
        // N:  +67.5°..+112.5°
        // NW: +112.5°..+157.5°
        // W:  else near +/-180°
        // SW: -157.5°..-112.5°
        // S:  -112.5°..-67.5°
        // SE: -67.5°..-22.5°
        const PI: f32 = std::f32::consts::PI;
        const P8: f32 = PI / 8.0;

        if (-P8..=P8).contains(&a) {
            Bearing::E
        } else if (P8..=3.0 * P8).contains(&a) {
            Bearing::NE
        } else if (3.0 * P8..=5.0 * P8).contains(&a) {
            Bearing::N
        } else if (5.0 * P8..=7.0 * P8).contains(&a) {
            Bearing::NW
        } else if (-3.0 * P8..=-P8).contains(&a) {
            Bearing::SE
        } else if (-5.0 * P8..=-3.0 * P8).contains(&a) {
            Bearing::S
        } else if (-7.0 * P8..=-5.0 * P8).contains(&a) {
            Bearing::SW
        } else {
            Bearing::W
        }
    }
}

/// Bearings for all experience-visible nodes that are off-screen.
/// Returns (NodeId, Bearing).
pub fn bearings_for_visible_nodes(field: &Field, vp: &Viewport) -> Vec<(NodeId, Bearing)> {
    field
        .nodes()
        .keys()
        .copied()
        .filter(|&id| field.is_visible(id))
        .filter_map(|id| {
            let n = field.node(id)?;
            let b = bearing_to_point(vp, n.pos)?;
            Some((id, b))
        })
        .collect()
}

/// Bearings for all experience-visible *anchor* nodes that are off-screen.
/// Returns (NodeId, Bearing).
///
/// Anchors do NOT bypass visibility rules. If a node is hidden-by-cluster,
/// explicitly hidden, or detached, it is not in the experience layer and
/// should not appear in Bearings.
pub fn bearings_for_anchors(field: &Field, vp: &Viewport) -> Vec<(NodeId, Bearing)> {
    field
        .nodes()
        .iter()
        .filter_map(|(&id, n)| {
            if !field.is_visible(id) {
                return None;
            }
            if !n.anchor {
                return None;
            }
            let b = bearing_to_point(vp, n.pos)?;
            Some((id, b))
        })
        .collect()
}

/// Returns the bearing direction from the viewport center to `point`,
/// but only if the point is off-screen.
pub fn bearing_to_point(vp: &Viewport, point: Vec2) -> Option<Bearing> {
    let r = vp.rect();
    if r.contains(point) {
        return None;
    }

    let d = Vec2 {
        x: point.x - vp.center.x,
        y: point.y - vp.center.y,
    };

    Some(Bearing::from_delta(d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Vec2;

    #[test]
    fn inside_viewport_returns_none() {
        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });
        assert_eq!(bearing_to_point(&vp, Vec2 { x: 10.0, y: 10.0 }), None);
    }

    #[test]
    fn cardinal_directions() {
        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });

        assert_eq!(
            bearing_to_point(&vp, Vec2 { x: 1000.0, y: 0.0 }),
            Some(Bearing::E)
        );
        assert_eq!(
            bearing_to_point(&vp, Vec2 { x: -1000.0, y: 0.0 }),
            Some(Bearing::W)
        );
        assert_eq!(
            bearing_to_point(&vp, Vec2 { x: 0.0, y: 1000.0 }),
            Some(Bearing::N)
        );
        assert_eq!(
            bearing_to_point(&vp, Vec2 { x: 0.0, y: -1000.0 }),
            Some(Bearing::S)
        );
    }

    #[test]
    fn diagonal_directions() {
        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });

        assert_eq!(
            bearing_to_point(
                &vp,
                Vec2 {
                    x: 1000.0,
                    y: 1000.0
                }
            ),
            Some(Bearing::NE)
        );
        assert_eq!(
            bearing_to_point(
                &vp,
                Vec2 {
                    x: -1000.0,
                    y: 1000.0
                }
            ),
            Some(Bearing::NW)
        );
        assert_eq!(
            bearing_to_point(
                &vp,
                Vec2 {
                    x: 1000.0,
                    y: -1000.0
                }
            ),
            Some(Bearing::SE)
        );
        assert_eq!(
            bearing_to_point(
                &vp,
                Vec2 {
                    x: -1000.0,
                    y: -1000.0
                }
            ),
            Some(Bearing::SW)
        );
    }

    #[test]
    fn bearings_skip_hidden_nodes() {
        use crate::field::{Field, Vec2};

        let mut field = Field::new();
        let a = field.spawn_surface("A", Vec2 { x: 1000.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let b = field.spawn_surface("B", Vec2 { x: -1000.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        assert!(field.set_hidden(b, true));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });

        let bs = bearings_for_visible_nodes(&field, &vp);

        assert_eq!(bs.len(), 1);
        assert_eq!(bs[0].0, a);
        assert_eq!(bs[0].1, Bearing::E);
    }

    #[test]
    fn bearings_for_anchors_only_includes_anchors() {
        use crate::field::{Field, Vec2};

        let mut field = Field::new();
        let a = field.spawn_surface("A", Vec2 { x: 1000.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let b = field.spawn_surface("B", Vec2 { x: -1000.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        assert!(field.set_anchor(b, true));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });

        let bs = bearings_for_anchors(&field, &vp);

        assert_eq!(bs.len(), 1);
        assert_eq!(bs[0].0, b);
        assert_eq!(bs[0].1, Bearing::W);

        // ensure non-anchor isn't included
        assert_ne!(a, b);
    }

    #[test]
    fn bearings_for_anchors_skips_hidden_anchors() {
        use crate::field::{Field, Vec2};

        let mut field = Field::new();
        let a = field.spawn_surface("A", Vec2 { x: 1000.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        assert!(field.set_anchor(a, true));
        assert!(field.set_hidden(a, true));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });

        let bs = bearings_for_anchors(&field, &vp);
        assert!(bs.is_empty());
    }
}
