use std::time::Duration;

use smithay::utils::{Logical, Rectangle, Size};

/// Quiet period in which a client-driven X11 mode change retains geometry
/// ownership. X11 clients can publish fullscreen off/on as a short burst while
/// changing modes; applying cluster layout between those requests exposes an
/// intermediate size that Field never sends.
pub(super) const CLIENT_GEOMETRY_QUIET_PERIOD: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ClientGeometryGuard {
    quiet_until: Duration,
    restore_center: Option<OpeningPlacement>,
}

impl ClientGeometryGuard {
    pub(super) fn arm(now: Duration) -> Self {
        Self {
            quiet_until: now.saturating_add(CLIENT_GEOMETRY_QUIET_PERIOD),
            restore_center: None,
        }
    }

    pub(super) fn is_active(self, now: Duration) -> bool {
        now < self.quiet_until
    }

    pub(super) fn quiet_until(self) -> Duration {
        self.quiet_until
    }

    pub(super) fn set_restore_center(&mut self, outer: Rectangle<i32, Logical>) {
        self.restore_center = Some(OpeningPlacement::new(outer, outer.size));
    }

    pub(super) fn restore_placement(self, now: Duration) -> Option<OpeningPlacement> {
        self.is_active(now).then_some(self.restore_center).flatten()
    }
}

pub(super) fn opening_placement_is_authoritative(
    opening_animation: bool,
    client_geometry_guarded: bool,
    grabbed: bool,
    fullscreen: bool,
) -> bool {
    (opening_animation || client_geometry_guarded) && !grabbed && !fullscreen
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MapAdmission {
    Wait,
    Admit,
    Ignore,
}

pub(super) fn map_admission(
    pending: bool,
    surface_associated: bool,
    has_buffer: bool,
) -> MapAdmission {
    if !pending {
        MapAdmission::Ignore
    } else if surface_associated && has_buffer {
        MapAdmission::Admit
    } else {
        MapAdmission::Wait
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OpeningPlacement {
    center_x_twice: i32,
    center_y_twice: i32,
    initial_size: Option<Size<i32, Logical>>,
}

impl OpeningPlacement {
    pub(super) fn preferred_size(
        initial_size: Size<i32, Logical>,
        fallback: Size<i32, Logical>,
    ) -> Size<i32, Logical> {
        if initial_size.w > 0 && initial_size.h > 0 {
            initial_size
        } else {
            fallback
        }
    }

    pub(super) fn new(geometry: Rectangle<i32, Logical>, initial_size: Size<i32, Logical>) -> Self {
        Self {
            center_x_twice: geometry.loc.x * 2 + geometry.size.w,
            center_y_twice: geometry.loc.y * 2 + geometry.size.h,
            initial_size: (initial_size.w > 0 && initial_size.h > 0).then_some(initial_size),
        }
    }

    pub(super) fn centered(self, size: Size<i32, Logical>) -> Rectangle<i32, Logical> {
        Rectangle::new(
            (
                (self.center_x_twice - size.w).div_euclid(2),
                (self.center_y_twice - size.h).div_euclid(2),
            )
                .into(),
            size,
        )
    }

    pub(super) fn restore_geometry(self) -> Option<Rectangle<i32, Logical>> {
        self.initial_size.map(|size| self.centered(size))
    }
}

#[cfg(test)]
mod tests {
    use smithay::utils::Rectangle;

    use super::{
        CLIENT_GEOMETRY_QUIET_PERIOD, ClientGeometryGuard, MapAdmission, OpeningPlacement,
        map_admission, opening_placement_is_authoritative,
    };

    #[test]
    fn pending_window_waits_for_surface_and_buffer() {
        assert_eq!(map_admission(true, false, false), MapAdmission::Wait);
        assert_eq!(map_admission(true, true, false), MapAdmission::Wait);
        assert_eq!(map_admission(true, false, true), MapAdmission::Wait);
        assert_eq!(map_admission(true, true, true), MapAdmission::Admit);
    }

    #[test]
    fn admitted_or_unknown_window_cannot_be_admitted_again() {
        assert_eq!(map_admission(false, true, true), MapAdmission::Ignore);
    }

    #[test]
    fn opening_size_changes_preserve_the_original_center() {
        let preliminary = Rectangle::new((1264, 704).into(), (32, 32).into());
        let placement = OpeningPlacement::new(preliminary, (640, 480).into());

        assert_eq!(
            placement.centered((1920, 1080).into()),
            Rectangle::new((320, 180).into(), (1920, 1080).into())
        );
        assert_eq!(
            placement.centered((1280, 720).into()),
            Rectangle::new((640, 360).into(), (1280, 720).into())
        );
        assert_eq!(
            placement.restore_geometry(),
            Some(Rectangle::new((960, 480).into(), (640, 480).into()))
        );
    }

    #[test]
    fn invalid_client_geometry_has_no_seeded_restore() {
        let placement = OpeningPlacement::new(
            Rectangle::new((320, 180).into(), (1920, 1080).into()),
            (0, 480).into(),
        );

        assert_eq!(placement.restore_geometry(), None);
    }

    #[test]
    fn client_size_wins_over_a_later_output_sized_buffer() {
        assert_eq!(
            OpeningPlacement::preferred_size((640, 480).into(), (2560, 1440).into()),
            (640, 480).into()
        );
        assert_eq!(
            OpeningPlacement::preferred_size((0, 0).into(), (2560, 1440).into()),
            (2560, 1440).into()
        );
    }

    #[test]
    fn client_geometry_guard_ends_after_a_quiet_period() {
        let now = std::time::Duration::from_secs(7);
        let guard = ClientGeometryGuard::arm(now);

        assert!(guard.is_active(now));
        assert!(
            guard
                .is_active(now + CLIENT_GEOMETRY_QUIET_PERIOD - std::time::Duration::from_nanos(1))
        );
        assert!(!guard.is_active(now + CLIENT_GEOMETRY_QUIET_PERIOD));
    }

    #[test]
    fn rearming_client_geometry_guard_extends_ownership() {
        let first = std::time::Duration::from_secs(3);
        let second = first + std::time::Duration::from_millis(100);
        let guard = ClientGeometryGuard::arm(second);

        assert!(guard.is_active(first + CLIENT_GEOMETRY_QUIET_PERIOD));
        assert!(!guard.is_active(second + CLIENT_GEOMETRY_QUIET_PERIOD));
    }

    #[test]
    fn guarded_x11_startup_retains_placement_without_visual_animation() {
        assert!(opening_placement_is_authoritative(
            false, true, false, false
        ));
        assert!(opening_placement_is_authoritative(
            true, false, false, false
        ));
    }

    #[test]
    fn grab_or_fullscreen_ends_opening_placement_authority() {
        assert!(!opening_placement_is_authoritative(
            false, true, true, false
        ));
        assert!(!opening_placement_is_authoritative(
            false, true, false, true
        ));
        assert!(!opening_placement_is_authoritative(
            false, false, false, false
        ));
    }
}
