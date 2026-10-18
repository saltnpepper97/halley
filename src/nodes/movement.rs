//! Explicit-distance movement sweeps along one axis. Unlike legacy nearest-free
//! placement, it cannot jump around or tunnel through a landmark obstruction.

use super::*;

#[derive(Clone, Copy)]
struct AxisMove {
    horizontal: bool,
    sign: f64,
    distance: f64,
}

impl AxisMove {
    fn new(delta: (i32, i32)) -> Self {
        let horizontal = delta.0 != 0;
        let amount = if horizontal { delta.0 } else { delta.1 };
        Self {
            horizontal,
            sign: f64::from(amount.signum()),
            distance: f64::from(amount).abs(),
        }
    }

    fn coordinates(self, point: Vec2) -> (f64, f64) {
        if self.horizontal {
            (f64::from(point.x), f64::from(point.y))
        } else {
            (f64::from(point.y), f64::from(point.x))
        }
    }

    fn coordinate_limit(self, origin: f64) -> f64 {
        let edge = if self.sign > 0.0 {
            f64::from(i32::MAX)
        } else {
            f64::from(i32::MIN)
        };
        self.distance.min(((edge - origin) * self.sign).max(0.0))
    }

    /// Clip travel at entry into an open obstacle interval. Tangency is legal;
    /// an already overlapping body stays put until ordinary placement resolves it.
    fn clip(self, origin: f64, interval: Option<(f64, f64)>, travel: f64) -> f64 {
        let Some((low, high)) = interval else {
            return travel;
        };
        let (entry, exit) = if self.sign > 0.0 {
            (low - origin, high - origin)
        } else {
            (origin - high, origin - low)
        };
        if exit <= 0.0 {
            travel
        } else {
            travel.min(entry.max(0.0))
        }
    }
}

fn circle_interval(center: f64, perpendicular: f64, radius: f64) -> Option<(f64, f64)> {
    if perpendicular.abs() >= radius {
        return None;
    }
    let extent = (radius * radius - perpendicular * perpendicular)
        .max(0.0)
        .sqrt();
    Some((center - extent, center + extent))
}

/// The cross-section of a rectangle expanded by a circle, retaining rounded
/// corners rather than treating its entire bounding box as an obstruction.
fn rect_interval(
    motion: AxisMove,
    perpendicular: f64,
    rect: Rectangle<i32, Logical>,
    radius: f64,
) -> Option<(f64, f64)> {
    let (axis_min, axis_size, cross_min, cross_size) = if motion.horizontal {
        (rect.loc.x, rect.size.w, rect.loc.y, rect.size.h)
    } else {
        (rect.loc.y, rect.size.h, rect.loc.x, rect.size.w)
    };
    let cross_min = f64::from(cross_min);
    let cross_max = cross_min + f64::from(cross_size);
    let gap = (cross_min - perpendicular)
        .max(perpendicular - cross_max)
        .max(0.0);
    if gap >= radius {
        return None;
    }
    let extent = (radius * radius - gap * gap).max(0.0).sqrt();
    Some((
        f64::from(axis_min) - extent,
        f64::from(axis_min) + f64::from(axis_size) + extent,
    ))
}

fn landmark_destination(
    current: Vec2,
    delta: (i32, i32),
    nodes: &[Vec2],
    windows: &[Rectangle<i32, Logical>],
    spacing: f32,
    clearance: f32,
) -> Vec2 {
    let motion = AxisMove::new(delta);
    let (origin, perpendicular) = motion.coordinates(current);
    let mut travel = motion.coordinate_limit(origin);
    for node in nodes {
        let (axis, cross) = motion.coordinates(*node);
        travel = motion.clip(
            origin,
            circle_interval(axis, cross - perpendicular, f64::from(spacing)),
            travel,
        );
    }
    for window in windows {
        travel = motion.clip(
            origin,
            rect_interval(motion, perpendicular, *window, f64::from(clearance)),
            travel,
        );
    }
    // f32 Field positions can round past contact. Round conservatively toward
    // the starting position, including at large coordinates.
    let mut destination = (origin + motion.sign * travel) as f32;
    if (f64::from(destination) - origin) * motion.sign > travel {
        destination = if motion.sign > 0.0 {
            destination.next_down()
        } else {
            destination.next_up()
        };
    }
    if motion.horizontal {
        Vec2 {
            x: destination,
            ..current
        }
    } else {
        Vec2 {
            y: destination,
            ..current
        }
    }
}

fn window_destination(
    client: Rectangle<i32, Logical>,
    outer: Rectangle<i32, Logical>,
    delta: (i32, i32),
    nodes: &[Vec2],
    clearance: f32,
) -> Rectangle<i32, Logical> {
    let motion = AxisMove::new(delta);
    let origin = if motion.horizontal {
        client.loc.x
    } else {
        client.loc.y
    };
    let mut travel = motion.coordinate_limit(f64::from(origin));
    // Sweeping a window past a stationary landmark is equivalent to sweeping
    // that landmark in the opposite direction past the window's outer bounds.
    let reverse = AxisMove {
        sign: -motion.sign,
        ..motion
    };
    for node in nodes {
        let (axis, cross) = motion.coordinates(*node);
        travel = reverse.clip(
            axis,
            rect_interval(motion, cross, outer, f64::from(clearance)),
            travel,
        );
    }
    let shift = (travel.floor() * motion.sign) as i32;
    let location = if motion.horizontal {
        Point::from((origin.saturating_add(shift), client.loc.y))
    } else {
        Point::from((client.loc.x, origin.saturating_add(shift)))
    };
    Rectangle::new(location, client.size)
}

impl NodesState {
    fn movement_landmarks(&self, id: NodeId, output: &str, cores: &[Vec2]) -> Vec<Vec2> {
        self.records
            .values()
            .filter(|record| {
                record.id != id && record.collapsed && record.attached && record.output == output
            })
            .filter_map(|record| self.field.node(record.id).map(|node| node.pos))
            .chain(cores.iter().copied())
            .collect()
    }

    pub(super) fn directional_landmark_position(
        &self,
        id: NodeId,
        current: Vec2,
        delta: (i32, i32),
        scale: f32,
        cores: &[Vec2],
        chrome: PlacementChrome<'_>,
    ) -> Vec2 {
        let Some(record) = self.record(id) else {
            return current;
        };
        let scale = scale.max(0.05);
        let nodes = self.movement_landmarks(id, &record.output, cores);
        let windows = self
            .records
            .values()
            .filter(|other| {
                other.id != id
                    && !other.collapsed
                    && other.attached
                    && other.output == record.output
            })
            .map(|other| {
                crate::titlebar::outer_rect_for_client(
                    &other.window,
                    other.geometry,
                    chrome.decorations,
                    chrome.font,
                )
            })
            .collect::<Vec<_>>();
        landmark_destination(
            current,
            delta,
            &nodes,
            &windows,
            (NODE_DIAMETER_PX + self.landmarks.gap_px) / scale,
            (NODE_DIAMETER_PX * 0.5 + self.landmarks.gap_px) / scale,
        )
    }

    pub(super) fn directional_active_rect(
        &self,
        id: NodeId,
        current: Rectangle<i32, Logical>,
        delta: (i32, i32),
        scale: f32,
        cores: &[Vec2],
        chrome: PlacementChrome<'_>,
    ) -> Rectangle<i32, Logical> {
        let Some(record) = self.record(id) else {
            return current;
        };
        let nodes = self.movement_landmarks(id, &record.output, cores);
        let outer = crate::titlebar::outer_rect_for_client(
            &record.window,
            current,
            chrome.decorations,
            chrome.font,
        );
        window_destination(
            current,
            outer,
            delta,
            &nodes,
            (NODE_DIAMETER_PX * 0.5 + self.landmarks.gap_px) / scale.max(0.05),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f32, y: f32) -> Vec2 {
        Vec2 { x, y }
    }
    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn unobstructed_landmarks_move_exactly_on_each_axis() {
        for (delta, expected) in [
            ((10, 0), point(30.0, 30.0)),
            ((-10, 0), point(10.0, 30.0)),
            ((0, 10), point(20.0, 40.0)),
            ((0, -10), point(20.0, 20.0)),
        ] {
            assert_eq!(
                landmark_destination(point(20.0, 30.0), delta, &[], &[], 71.0, 45.5),
                expected
            );
        }
    }

    #[test]
    fn landmark_sweep_stops_at_first_obstacle_without_tunneling_or_sideways_motion() {
        let blockers = [point(100.0, 0.0), point(200.0, 0.0)];
        assert_eq!(
            landmark_destination(point(0.0, 0.0), (500, 0), &blockers, &[], 71.0, 45.5),
            point(29.0, 0.0)
        );
        assert_eq!(
            landmark_destination(point(300.0, 0.0), (-500, 0), &blockers, &[], 71.0, 45.5),
            point(271.0, 0.0)
        );
    }

    #[test]
    fn contact_allows_tangential_motion_and_motion_away() {
        let blocker = [point(71.0, 0.0)];
        assert_eq!(
            landmark_destination(point(0.0, 0.0), (10, 0), &blocker, &[], 71.0, 45.5),
            point(0.0, 0.0)
        );
        assert_eq!(
            landmark_destination(point(0.0, 0.0), (-10, 0), &blocker, &[], 71.0, 45.5),
            point(-10.0, 0.0)
        );
        assert_eq!(
            landmark_destination(point(0.0, 0.0), (0, 10), &blocker, &[], 71.0, 45.5),
            point(0.0, 10.0)
        );
    }

    #[test]
    fn collapsed_landmarks_stop_at_window_clearance() {
        assert_eq!(
            landmark_destination(
                point(0.0, 30.0),
                (500, 0),
                &[],
                &[rect(100, 0, 80, 60)],
                71.0,
                45.5
            ),
            point(54.5, 30.0)
        );
    }

    #[test]
    fn rounded_corners_and_tangencies_do_not_block_free_paths() {
        let windows = [rect(100, 100, 100, 100)];
        assert_eq!(
            landmark_destination(point(0.0, 50.0), (300, 0), &[], &windows, 71.0, 50.0),
            point(300.0, 50.0)
        );
        assert_eq!(
            landmark_destination(point(0.0, 70.0), (300, 0), &[], &windows, 71.0, 50.0),
            point(60.0, 70.0)
        );
    }

    #[test]
    fn active_window_sweep_uses_decorated_bounds_and_never_tunnels() {
        let client = rect(10, 30, 100, 100);
        let outer = rect(0, 0, 120, 140);
        assert_eq!(
            window_destination(client, outer, (500, 0), &[point(200.0, 70.0)], 45.5),
            rect(44, 30, 100, 100)
        );
    }

    #[test]
    fn active_window_moves_all_directions_and_retains_size() {
        let client = rect(10, 30, 100, 100);
        for (delta, expected) in [
            ((10, 0), rect(20, 30, 100, 100)),
            ((-10, 0), rect(0, 30, 100, 100)),
            ((0, 10), rect(10, 40, 100, 100)),
            ((0, -10), rect(10, 20, 100, 100)),
        ] {
            assert_eq!(
                window_destination(client, client, delta, &[], 45.5),
                expected
            );
        }
    }

    #[test]
    fn window_stops_at_nodes_from_each_direction() {
        let client = rect(100, 100, 100, 100);
        for (delta, node, expected) in [
            ((500, 0), point(300.0, 150.0), rect(150, 100, 100, 100)),
            ((-500, 0), point(0.0, 150.0), rect(50, 100, 100, 100)),
            ((0, 500), point(150.0, 300.0), rect(100, 150, 100, 100)),
            ((0, -500), point(150.0, 0.0), rect(100, 50, 100, 100)),
        ] {
            assert_eq!(
                window_destination(client, client, delta, &[node], 50.0),
                expected
            );
        }
    }

    #[test]
    fn zoom_scaled_clearance_is_respected() {
        let nodes = [point(200.0, 0.0)];
        assert_eq!(
            landmark_destination(
                point(0.0, 0.0),
                (500, 0),
                &nodes,
                &[],
                71.0 / 0.5,
                45.5 / 0.5
            ),
            point(58.0, 0.0)
        );
    }

    #[test]
    fn existing_overlap_stays_put() {
        assert_eq!(
            landmark_destination(
                point(0.0, 0.0),
                (500, 0),
                &[point(10.0, 0.0)],
                &[],
                71.0,
                45.5
            ),
            point(0.0, 0.0)
        );
    }

    #[test]
    fn enormous_moves_clip_at_coordinate_limits_without_overflow() {
        for delta in [(i32::MAX, 0), (-i32::MAX, 0), (0, i32::MAX), (0, -i32::MAX)] {
            let client = rect(100, 100, 100, 100);
            let moved = window_destination(client, client, delta, &[], 45.5);
            let pos = landmark_destination(point(100.0, 100.0), delta, &[], &[], 71.0, 45.5);
            assert!(pos.x.is_finite() && pos.y.is_finite());
            if delta.0 > 0 {
                assert_eq!(moved.loc.x, i32::MAX);
            }
            if delta.1 > 0 {
                assert_eq!(moved.loc.y, i32::MAX);
            }
        }
    }
}
