//! Effective monitor coordinates shared by startup and runtime output changes.
//!
//! XWayland's root begins at zero. Shift the complete active layout together
//! when a configured output is left of or above that origin, rather than
//! clamping individual outputs and destroying their relative placement.

use smithay::utils::{Logical, Point};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LayoutOrigin(Point<i32, Logical>);

impl LayoutOrigin {
    pub(crate) fn for_locations(locations: impl IntoIterator<Item = Point<i32, Logical>>) -> Self {
        Self(locations.into_iter().fold((0, 0).into(), |origin, point| {
            (origin.x.min(point.x), origin.y.min(point.y)).into()
        }))
    }

    pub(crate) fn effective(
        self,
        configured: Point<i32, Logical>,
    ) -> Result<Point<i32, Logical>, String> {
        let x = configured.x.checked_sub(self.0.x);
        let y = configured.y.checked_sub(self.0.y);
        Ok((
            x.ok_or("normalized output X exceeds the coordinate range")?,
            y.ok_or("normalized output Y exceeds the coordinate range")?,
        )
            .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize(points: &[(i32, i32)]) -> Vec<Point<i32, Logical>> {
        let points = points.iter().copied().map(Point::from).collect::<Vec<_>>();
        let origin = LayoutOrigin::for_locations(points.iter().copied());
        points
            .into_iter()
            .map(|point| origin.effective(point).unwrap())
            .collect()
    }

    #[test]
    fn left_monitor_and_game_share_one_nonnegative_origin() {
        assert_eq!(
            normalize(&[(-1024, 0), (0, 0)]),
            vec![(0, 0).into(), (1024, 0).into()]
        );
    }

    #[test]
    fn above_left_and_gaps_keep_their_relative_positions() {
        assert_eq!(
            normalize(&[(-1200, -900), (200, 100)]),
            vec![(0, 0).into(), (1400, 1000).into()]
        );
    }

    #[test]
    fn existing_nonnegative_coordinates_do_not_move() {
        assert_eq!(
            normalize(&[(100, 80), (2660, 80)]),
            vec![(100, 80).into(), (2660, 80).into()]
        );
    }

    #[test]
    fn effective_coordinates_are_idempotent() {
        let once = normalize(&[(-1024, -768), (0, 0)]);
        let origin = LayoutOrigin::for_locations(once.iter().copied());
        assert_eq!(
            once.iter()
                .map(|&point| origin.effective(point).unwrap())
                .collect::<Vec<_>>(),
            once
        );
    }

    #[test]
    fn overflowing_layout_is_rejected_without_saturating_positions() {
        let origin = LayoutOrigin::for_locations([(-1, 0).into(), (i32::MAX, 0).into()]);
        assert!(origin.effective((i32::MAX, 0).into()).is_err());
    }

    #[test]
    fn most_negative_single_position_can_be_rebased_exactly() {
        assert_eq!(normalize(&[(i32::MIN, i32::MIN)]), vec![(0, 0).into()]);
    }
}
