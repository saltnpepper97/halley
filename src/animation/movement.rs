use super::*;
use smithay::utils::Logical;

#[derive(Clone, Copy, Debug)]
pub(super) struct MoveTimeline {
    geometry: RectTransition,
    target: Rectangle<i32, Logical>,
}

impl MoveTimeline {
    pub(super) fn finished(self, now: Duration) -> bool {
        self.geometry.is_finished_at(now)
    }

    fn rect(
        self,
        now: Duration,
        current: Rectangle<i32, Logical>,
    ) -> Option<Rectangle<i32, Logical>> {
        // Another operation (drag, output transfer, resize, etc.) owns geometry
        // now. Never apply an obsolete movement over its newer placement.
        (current == self.target).then(|| self.geometry.rect_at(now).round().to_logical(1))
    }
}

impl WindowAnimations {
    pub fn move_window(
        &mut self,
        surface: WlSurface,
        now: Duration,
        current: Rectangle<i32, Logical>,
        target: Rectangle<i32, Logical>,
    ) {
        let previous = self
            .moving
            .remove(&surface)
            .filter(|t| t.rect(now, current).is_some());
        let config = self.config.window_move;
        if !self.config.enabled || !config.enabled || current == target {
            return;
        }
        let (start, velocity) = previous.map_or(
            (
                VisualRect::from(current.to_physical(1)),
                VisualRect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                },
            ),
            |t| (t.geometry.rect_at(now), t.geometry.velocity_at(now)),
        );
        let geometry = RectTransition::between(
            config.motion,
            now,
            start,
            target.to_physical(1).into(),
            velocity,
        );
        if !geometry.is_finished_at(now) {
            self.moving
                .insert(surface, MoveTimeline { geometry, target });
        }
    }

    pub fn movement_visual(
        &self,
        surface: &WlSurface,
        now: Duration,
        current: Rectangle<i32, Logical>,
    ) -> Option<Rectangle<i32, Logical>> {
        self.moving.get(surface)?.rect(now, current)
    }

    pub fn is_moving(&self, surface: &WlSurface, now: Duration) -> bool {
        self.moving.get(surface).is_some_and(|t| !t.finished(now))
    }

    pub fn cancel_move(&mut self, surface: &WlSurface) {
        self.moving.remove(surface);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_config::{AnimationCurve, EasingMotion, SpringMotion};
    use smithay::reexports::wayland_server::protocol::wl_surface;
    use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, Display, DisplayHandle};
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;

    fn with_surface(test: impl FnOnce(WlSurface)) {
        struct State;
        impl Dispatch<WlSurface, ()> for State {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &WlSurface,
                _: wl_surface::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
            }
        }
        let display = Display::<State>::new().unwrap();
        let mut handle = display.handle();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let client = handle.insert_client(socket, Arc::new(())).unwrap();
        test(
            client
                .create_resource::<WlSurface, (), State>(&handle, 1, ())
                .unwrap(),
        );
    }

    fn config() -> Animations {
        let mut config = Animations::default();
        config.window_move.motion = AnimationMotion::Easing(EasingMotion {
            duration_ms: 1000,
            curve: AnimationCurve::Linear,
        });
        config
    }

    #[test]
    fn movement_repeat_and_reversal_begin_at_the_visible_position() {
        with_surface(|surface| {
            for motion in [
                config().window_move.motion,
                AnimationMotion::Spring(SpringMotion::default()),
            ] {
                let mut config = config();
                config.window_move.motion = motion;
                let mut animations = WindowAnimations::new(config);
                let start = Rectangle::new((100, 200).into(), (500, 300).into());
                let target = Rectangle::new((500, 200).into(), start.size);
                animations.move_window(surface.clone(), Duration::ZERO, start, target);
                let now = Duration::from_millis(100);
                let before = animations.movement_visual(&surface, now, target).unwrap();
                assert!(before.loc.x > start.loc.x && before.loc.x < target.loc.x);
                let velocity = animations.moving[&surface].geometry.velocity_at(now);
                animations.move_window(surface.clone(), now, target, start);
                assert_eq!(
                    animations.movement_visual(&surface, now, start),
                    Some(before)
                );
                if matches!(motion, AnimationMotion::Spring(_)) {
                    let reversed = animations.moving[&surface].geometry;
                    let start = reversed.rect_at(now).x;
                    let next = reversed.rect_at(now + Duration::from_micros(100)).x;
                    assert!((next - start - velocity.x * 0.0001).abs() < 0.01);
                }
                assert!(animations.is_animating(&surface, now));
                assert_eq!(
                    animations.movement_visual(&surface, Duration::from_secs(10), start),
                    Some(start)
                );
                assert!(animations.cleanup(Duration::from_secs(10)));
                assert!(!animations.is_animating(&surface, Duration::from_secs(10)));
            }
        });
    }

    #[test]
    fn disabled_zero_duration_and_external_placement_do_not_leave_stale_motion() {
        with_surface(|surface| {
            let start = Rectangle::new((100, 200).into(), (500, 300).into());
            let target = Rectangle::new((-300, 200).into(), start.size);
            for mode in 0..3 {
                let mut config = config();
                match mode {
                    0 => config.enabled = false,
                    1 => config.window_move.enabled = false,
                    _ => {
                        config.window_move.motion = AnimationMotion::Easing(EasingMotion {
                            duration_ms: 0,
                            curve: AnimationCurve::Linear,
                        })
                    }
                }
                let mut animations = WindowAnimations::new(config);
                animations.move_window(surface.clone(), Duration::ZERO, start, target);
                assert!(
                    animations
                        .movement_visual(&surface, Duration::ZERO, target)
                        .is_none()
                );
                assert!(!animations.is_animating(&surface, Duration::ZERO));
            }
            let mut animations = WindowAnimations::new(config());
            animations.move_window(surface.clone(), Duration::ZERO, start, target);
            assert!(
                animations
                    .movement_visual(&surface, Duration::ZERO, start)
                    .is_none()
            );
            animations.cancel_move(&surface);
            assert!(
                animations
                    .movement_visual(&surface, Duration::ZERO, target)
                    .is_none()
            );
        });
    }
}
