use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};
use smithay::output::Output;

use super::{Session, SessionDriver};
use crate::cursor::{TimerDirective, Visibility};

/// Event-loop adapter for the pure cursor visibility policy.
///
/// Timer ownership stays here so neither raw input dispatch nor the cursor
/// theme manager needs to know about calloop registration tokens.
pub(crate) struct Policy<D: SessionDriver> {
    visibility: Visibility,
    loop_handle: LoopHandle<'static, Session<D>>,
    visibility_timer: Option<RegistrationToken>,
    animation_timer: Option<RegistrationToken>,
    animation_generation: u64,
}

impl<D: SessionDriver> Policy<D> {
    pub fn new(
        config: &halley_config::Cursor,
        loop_handle: LoopHandle<'static, Session<D>>,
    ) -> Self {
        let mut policy = Self {
            visibility: Visibility::new(config),
            loop_handle,
            visibility_timer: None,
            animation_timer: None,
            animation_generation: 0,
        };
        let directive = policy.visibility.initial_timer();
        policy.apply_timer(directive);
        policy
    }

    pub fn visible(&self) -> bool {
        self.visibility.visible()
    }

    pub fn pointer_activity(&mut self) -> bool {
        let (redraw, directive) = self.visibility.pointer_activity();
        self.apply_timer(directive);
        redraw
    }

    pub fn keyboard_press(&mut self) -> bool {
        let (redraw, directive) = self.visibility.keyboard_press();
        self.apply_timer(directive);
        redraw
    }

    pub fn keyboard_navigation(&mut self) -> bool {
        let (redraw, directive) = self.visibility.keyboard_navigation();
        self.apply_timer(directive);
        redraw
    }

    pub fn touch_down(&mut self) -> bool {
        let (redraw, directive) = self.visibility.touch_down();
        self.apply_timer(directive);
        redraw
    }

    pub fn reload(&mut self, config: &halley_config::Cursor) -> bool {
        let (redraw, directive) = self.visibility.reload(config);
        self.apply_timer(directive);
        redraw
    }

    pub fn schedule_animation(&mut self, output: &Output, delay: Option<std::time::Duration>) {
        if let Some(token) = self.animation_timer.take() {
            self.loop_handle.remove(token);
        }
        self.animation_generation = self.animation_generation.wrapping_add(1);
        let Some(delay) = delay else {
            return;
        };
        let generation = self.animation_generation;
        let output = output.clone();
        match self.loop_handle.insert_source(
            Timer::from_duration(delay.max(std::time::Duration::from_millis(1))),
            move |_, _, session| {
                session.cursor_policy.animation_timer = None;
                if session.cursor_policy.animation_generation == generation {
                    session.request_output_redraw(&output);
                }
                TimeoutAction::Drop
            },
        ) {
            Ok(token) => self.animation_timer = Some(token),
            Err(err) => eventline::warn!("cursor: failed to arm animation timer: {err}"),
        }
    }

    fn apply_timer(&mut self, directive: TimerDirective) {
        if directive == TimerDirective::Keep {
            return;
        }
        if let Some(token) = self.visibility_timer.take() {
            self.loop_handle.remove(token);
        }
        let TimerDirective::Arm { generation, delay } = directive else {
            return;
        };
        match self
            .loop_handle
            .insert_source(Timer::from_duration(delay), move |_, _, session| {
                session.cursor_policy.visibility_timer = None;
                if session.cursor_policy.visibility.timer_expired(generation) {
                    session.request_redraw();
                }
                TimeoutAction::Drop
            }) {
            Ok(token) => self.visibility_timer = Some(token),
            Err(err) => eventline::warn!("cursor: failed to arm inactivity timer: {err}"),
        }
    }
}
