use std::time::Duration;

use halley_config::{AnimationCurve, AnimationMotion, SpringMotion};

const MAX_SPRING_DURATION: Duration = Duration::from_secs(10);
const SPRING_DURATION_STEP: Duration = Duration::from_millis(1);
const SPRING_SETTLE_EPSILON: f64 = 0.0001;

#[derive(Clone, Copy, Debug)]
pub struct MotionTimeline {
    started_at: Duration,
    motion: AnimationMotion,
    duration: Duration,
    start: f64,
    target: f64,
    initial_velocity: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSample {
    pub linear_progress: f64,
    pub linear_velocity: f64,
    pub value: f64,
    pub velocity: f64,
}

impl MotionTimeline {
    pub fn new(motion: AnimationMotion, started_at: Duration) -> Self {
        Self::between(motion, started_at, 0.0, 1.0, 0.0)
    }

    pub fn between(
        motion: AnimationMotion,
        started_at: Duration,
        start: f64,
        target: f64,
        initial_velocity: f64,
    ) -> Self {
        let duration = match motion {
            AnimationMotion::Easing(easing) => Duration::from_millis(u64::from(easing.duration_ms)),
            AnimationMotion::Spring(spring) => {
                spring_duration(spring, start - target, initial_velocity)
            }
        };
        Self {
            started_at,
            motion,
            duration,
            start,
            target,
            initial_velocity,
        }
    }

    pub fn value_at(self, now: Duration) -> f64 {
        let elapsed = now.saturating_sub(self.started_at);
        match self.motion {
            AnimationMotion::Easing(easing) => {
                if self.duration.is_zero() {
                    return self.target;
                }
                let linear = (elapsed.as_secs_f64() / self.duration.as_secs_f64()).clamp(0.0, 1.0);
                self.start + (self.target - self.start) * apply_curve(easing.curve, linear)
            }
            AnimationMotion::Spring(spring) => {
                if elapsed >= self.duration {
                    self.target
                } else {
                    self.target
                        + spring_displacement(
                            spring,
                            self.start - self.target,
                            self.initial_velocity,
                            elapsed.as_secs_f64(),
                        )
                }
            }
        }
    }

    pub fn sample_at(self, now: Duration) -> MotionSample {
        MotionSample {
            linear_progress: self.linear_progress_at(now),
            linear_velocity: self.linear_velocity_at(now),
            value: self.value_at(now),
            velocity: self.velocity_at(now),
        }
    }

    pub fn linear_progress_at(self, now: Duration) -> f64 {
        if self.duration.is_zero() {
            return 1.0;
        }

        (now.saturating_sub(self.started_at).as_secs_f64() / self.duration.as_secs_f64())
            .clamp(0.0, 1.0)
    }

    pub fn linear_velocity_at(self, now: Duration) -> f64 {
        let elapsed = now.saturating_sub(self.started_at);
        if self.duration.is_zero() || elapsed >= self.duration {
            0.0
        } else {
            self.duration.as_secs_f64().recip()
        }
    }

    pub fn velocity_at(self, now: Duration) -> f64 {
        let elapsed = now.saturating_sub(self.started_at);
        if elapsed >= self.duration || self.duration.is_zero() {
            return 0.0;
        }
        match self.motion {
            AnimationMotion::Easing(_) => {
                let step = Duration::from_micros(100);
                let before = now.saturating_sub(step);
                let after = now + step;
                (self.value_at(after) - self.value_at(before)) / (step.as_secs_f64() * 2.0)
            }
            AnimationMotion::Spring(spring) => spring_velocity(
                spring,
                self.start - self.target,
                self.initial_velocity,
                elapsed.as_secs_f64(),
            ),
        }
    }

    pub fn completion_at(self, now: Duration) -> f64 {
        let distance = self.target - self.start;
        if distance.abs() < f64::EPSILON {
            return 1.0;
        }
        ((self.value_at(now) - self.start) / distance).clamp(0.0, 1.0)
    }

    pub fn is_finished_at(self, now: Duration) -> bool {
        now.saturating_sub(self.started_at) >= self.duration
    }

    pub fn duration(self) -> Duration {
        self.duration
    }

    pub fn target(self) -> f64 {
        self.target
    }
}

pub fn apply_curve(curve: AnimationCurve, progress: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    match curve {
        AnimationCurve::Linear => progress,
        AnimationCurve::EaseInOutCubic if progress < 0.5 => 4.0 * progress * progress * progress,
        AnimationCurve::EaseInOutCubic => 1.0 - (-2.0 * progress + 2.0).powi(3) / 2.0,
        AnimationCurve::EaseOutQuad => 1.0 - (1.0 - progress).powi(2),
        AnimationCurve::EaseOutCubic => 1.0 - (1.0 - progress).powi(3),
        AnimationCurve::EaseOutExpo if progress == 1.0 => 1.0,
        AnimationCurve::EaseOutExpo => 1.0 - 2.0_f64.powf(-10.0 * progress),
        AnimationCurve::Elastic => {
            let overshoot = 1.42;
            let shifted = progress - 1.0;
            1.0 + shifted * shifted * ((overshoot + 1.0) * shifted + overshoot)
        }
    }
}

fn spring_displacement(
    spring: SpringMotion,
    initial_displacement: f64,
    initial_velocity: f64,
    seconds: f64,
) -> f64 {
    let omega = spring.stiffness.sqrt();
    let damping = spring.damping_ratio;
    let displacement = if (damping - 1.0).abs() < 1e-7 {
        let coefficient = initial_velocity + omega * initial_displacement;
        (initial_displacement + coefficient * seconds) * (-omega * seconds).exp()
    } else if damping < 1.0 {
        let damped = omega * (1.0 - damping * damping).sqrt();
        let coefficient = (initial_velocity + damping * omega * initial_displacement) / damped;
        (-damping * omega * seconds).exp()
            * (initial_displacement * (damped * seconds).cos()
                + coefficient * (damped * seconds).sin())
    } else {
        let root = (damping * damping - 1.0).sqrt();
        let first = -omega * (damping - root);
        let second = -omega * (damping + root);
        let first_coefficient =
            (initial_velocity - second * initial_displacement) / (first - second);
        let second_coefficient =
            (first * initial_displacement - initial_velocity) / (first - second);
        first_coefficient * (first * seconds).exp() + second_coefficient * (second * seconds).exp()
    };

    if displacement.is_finite() {
        displacement
    } else {
        0.0
    }
}

fn spring_velocity(
    spring: SpringMotion,
    initial_displacement: f64,
    initial_velocity: f64,
    seconds: f64,
) -> f64 {
    let step = 0.0001;
    let before = (seconds - step).max(0.0);
    let divisor = if seconds < step { step } else { step * 2.0 };
    (spring_displacement(
        spring,
        initial_displacement,
        initial_velocity,
        seconds + step,
    ) - spring_displacement(spring, initial_displacement, initial_velocity, before))
        / divisor
}

fn spring_duration(
    spring: SpringMotion,
    initial_displacement: f64,
    initial_velocity: f64,
) -> Duration {
    let mut elapsed = Duration::ZERO;
    let mut last_unsettled = Duration::ZERO;
    while elapsed <= MAX_SPRING_DURATION {
        if spring_displacement(
            spring,
            initial_displacement,
            initial_velocity,
            elapsed.as_secs_f64(),
        )
        .abs()
            > SPRING_SETTLE_EPSILON
        {
            last_unsettled = elapsed;
        }
        elapsed += SPRING_DURATION_STEP;
    }

    (last_unsettled + SPRING_DURATION_STEP).min(MAX_SPRING_DURATION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_config::{EasingMotion, SpringMotion};

    #[test]
    fn easing_preserves_endpoints() {
        for curve in [
            AnimationCurve::Linear,
            AnimationCurve::EaseInOutCubic,
            AnimationCurve::EaseOutQuad,
            AnimationCurve::EaseOutCubic,
            AnimationCurve::EaseOutExpo,
            AnimationCurve::Elastic,
        ] {
            assert_eq!(apply_curve(curve, 0.0), 0.0);
            assert_eq!(apply_curve(curve, 1.0), 1.0);
        }
    }

    #[test]
    fn every_easing_curve_obeys_the_configured_duration() {
        let started_at = Duration::from_secs(1);
        let duration = Duration::from_millis(850);

        for curve in [
            AnimationCurve::Linear,
            AnimationCurve::EaseInOutCubic,
            AnimationCurve::EaseOutQuad,
            AnimationCurve::EaseOutCubic,
            AnimationCurve::EaseOutExpo,
            AnimationCurve::Elastic,
        ] {
            let timeline = MotionTimeline::new(
                AnimationMotion::Easing(EasingMotion {
                    duration_ms: duration.as_millis() as u32,
                    curve,
                }),
                started_at,
            );
            let halfway = timeline.sample_at(started_at + duration / 2);
            let before_end = timeline.sample_at(started_at + duration - Duration::from_millis(1));
            let end = timeline.sample_at(started_at + duration);

            assert_eq!(timeline.value_at(started_at), 0.0, "{curve:?}");
            assert_eq!(halfway.linear_progress, 0.5, "{curve:?}");
            assert!(!timeline.is_finished_at(started_at + duration - Duration::from_millis(1)));
            assert!(before_end.linear_progress < 1.0, "{curve:?}");
            assert_eq!(end.linear_progress, 1.0, "{curve:?}");
            assert_eq!(end.value, 1.0, "{curve:?}");
            assert_eq!(end.linear_velocity, 0.0, "{curve:?}");
            assert_eq!(end.velocity, 0.0, "{curve:?}");
            assert!(timeline.is_finished_at(started_at + duration));
        }
    }

    #[test]
    fn elastic_curve_overshoots_before_settling() {
        assert!(apply_curve(AnimationCurve::Elastic, 0.5) > 1.0);
        assert_eq!(apply_curve(AnimationCurve::Elastic, 1.0), 1.0);
    }

    #[test]
    fn critical_spring_converges_and_finishes() {
        let timeline = MotionTimeline::new(
            AnimationMotion::Spring(SpringMotion::default()),
            Duration::from_secs(1),
        );

        assert_eq!(timeline.value_at(Duration::from_secs(1)), 0.0);
        assert!(timeline.value_at(Duration::from_millis(1100)) > 0.7);
        assert_eq!(timeline.value_at(Duration::from_secs(2)), 1.0);
        assert!(timeline.is_finished_at(Duration::from_secs(2)));
    }

    #[test]
    fn underdamped_spring_can_overshoot() {
        let spring = SpringMotion {
            damping_ratio: 0.4,
            ..SpringMotion::default()
        };
        assert!((1..500).any(|millis| {
            1.0 + spring_displacement(spring, -1.0, 0.0, f64::from(millis) / 1000.0) > 1.0
        }));
    }

    #[test]
    fn overdamped_spring_remains_finite() {
        let spring = SpringMotion {
            damping_ratio: 4.0,
            stiffness: 100_000.0,
        };
        for millis in 0..1_000 {
            assert!(spring_displacement(spring, -1.0, 0.0, f64::from(millis) / 1000.0).is_finite());
        }
    }

    #[test]
    fn zero_duration_easing_snaps_to_end() {
        let timeline = MotionTimeline::new(
            AnimationMotion::Easing(EasingMotion {
                duration_ms: 0,
                curve: AnimationCurve::Linear,
            }),
            Duration::ZERO,
        );
        assert_eq!(timeline.value_at(Duration::ZERO), 1.0);
        assert!(timeline.is_finished_at(Duration::ZERO));
    }

    #[test]
    fn retargeting_preserves_value_and_velocity() {
        let motion = AnimationMotion::Spring(SpringMotion {
            damping_ratio: 0.7,
            ..SpringMotion::default()
        });
        let first = MotionTimeline::new(motion, Duration::ZERO);
        let now = Duration::from_millis(80);
        let value = first.value_at(now);
        let velocity = first.velocity_at(now);
        let reversed = MotionTimeline::between(motion, now, value, 0.0, velocity);

        assert!((reversed.value_at(now) - value).abs() < 1e-9);
        assert!((reversed.velocity_at(now) - velocity).abs() < 0.1);
    }

    #[test]
    fn completion_always_runs_forward() {
        let motion = AnimationMotion::Easing(EasingMotion {
            duration_ms: 100,
            curve: AnimationCurve::Linear,
        });
        let forward = MotionTimeline::between(motion, Duration::ZERO, 0.0, 1.0, 0.0);
        let reverse = MotionTimeline::between(motion, Duration::ZERO, 1.0, 0.0, 0.0);
        let halfway = Duration::from_millis(50);

        assert_eq!(forward.completion_at(Duration::ZERO), 0.0);
        assert_eq!(reverse.completion_at(Duration::ZERO), 0.0);
        assert_eq!(forward.completion_at(halfway), 0.5);
        assert_eq!(reverse.completion_at(halfway), 0.5);
        assert_eq!(forward.completion_at(Duration::from_millis(100)), 1.0);
        assert_eq!(reverse.completion_at(Duration::from_millis(100)), 1.0);
    }

    #[test]
    fn spring_completion_is_clamped() {
        let timeline = MotionTimeline::new(
            AnimationMotion::Spring(SpringMotion {
                damping_ratio: 0.3,
                ..SpringMotion::default()
            }),
            Duration::ZERO,
        );

        for millis in 0..2_000 {
            assert!((0.0..=1.0).contains(&timeline.completion_at(Duration::from_millis(millis))));
        }
    }
}
