use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerDirective {
    Keep,
    Cancel,
    Arm { generation: u64, delay: Duration },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Visible,
    HiddenByTyping,
    HiddenByKeyboardNavigation,
    HiddenByTouch,
    HiddenByInactivity,
}

/// Presentation-only cursor visibility.
///
/// This state has no access to pointer coordinates, focus, grabs, or
/// constraints, making it impossible for an auto-hide transition to mutate
/// input routing.
pub struct Visibility {
    state: State,
    hide_when_typing: bool,
    hide_on_keyboard_nav: bool,
    hide_on_touch: bool,
    hide_after: Option<Duration>,
    timer_generation: u64,
}

impl Visibility {
    pub fn new(config: &halley_config::Cursor) -> Self {
        Self {
            state: State::Visible,
            hide_when_typing: config.hide_when_typing,
            hide_on_keyboard_nav: config.hide_on_keyboard_nav,
            hide_on_touch: config.hide_on_touch,
            hide_after: timeout(config),
            timer_generation: 0,
        }
    }

    pub fn visible(&self) -> bool {
        self.state == State::Visible
    }

    pub fn initial_timer(&mut self) -> TimerDirective {
        self.next_timer()
    }

    pub fn pointer_activity(&mut self) -> (bool, TimerDirective) {
        let redraw = !self.visible();
        self.state = State::Visible;
        (redraw, self.next_timer())
    }

    pub fn keyboard_press(&mut self) -> (bool, TimerDirective) {
        if !self.hide_when_typing || !self.visible() {
            return (false, TimerDirective::Keep);
        }
        self.state = State::HiddenByTyping;
        self.timer_generation = self.timer_generation.wrapping_add(1);
        (true, TimerDirective::Cancel)
    }

    pub fn keyboard_navigation(&mut self) -> (bool, TimerDirective) {
        if !self.hide_on_keyboard_nav || self.state == State::HiddenByKeyboardNavigation {
            return (false, TimerDirective::Keep);
        }
        let redraw = self.visible();
        self.state = State::HiddenByKeyboardNavigation;
        self.timer_generation = self.timer_generation.wrapping_add(1);
        (redraw, TimerDirective::Cancel)
    }

    pub fn touch_down(&mut self) -> (bool, TimerDirective) {
        if !self.hide_on_touch || !self.visible() {
            return (false, TimerDirective::Keep);
        }
        self.state = State::HiddenByTouch;
        self.timer_generation = self.timer_generation.wrapping_add(1);
        (true, TimerDirective::Cancel)
    }

    pub fn reload(&mut self, config: &halley_config::Cursor) -> (bool, TimerDirective) {
        let previous_timeout = self.hide_after;
        self.hide_when_typing = config.hide_when_typing;
        self.hide_on_keyboard_nav = config.hide_on_keyboard_nav;
        self.hide_on_touch = config.hide_on_touch;
        self.hide_after = timeout(config);

        let stale_hidden_state = matches!(self.state, State::HiddenByTyping)
            && !self.hide_when_typing
            || matches!(self.state, State::HiddenByKeyboardNavigation)
                && !self.hide_on_keyboard_nav
            || matches!(self.state, State::HiddenByTouch) && !self.hide_on_touch
            || matches!(self.state, State::HiddenByInactivity) && self.hide_after.is_none();
        if stale_hidden_state {
            self.state = State::Visible;
            return (true, self.next_timer());
        }
        if previous_timeout == self.hide_after {
            return (false, TimerDirective::Keep);
        }
        if self.visible() {
            (false, self.next_timer())
        } else {
            self.timer_generation = self.timer_generation.wrapping_add(1);
            (false, TimerDirective::Cancel)
        }
    }

    pub fn timer_expired(&mut self, generation: u64) -> bool {
        if generation != self.timer_generation || !self.visible() || self.hide_after.is_none() {
            return false;
        }
        self.state = State::HiddenByInactivity;
        true
    }

    fn next_timer(&mut self) -> TimerDirective {
        self.timer_generation = self.timer_generation.wrapping_add(1);
        match self.hide_after {
            Some(delay) => TimerDirective::Arm {
                generation: self.timer_generation,
                delay,
            },
            None => TimerDirective::Cancel,
        }
    }
}

fn timeout(config: &halley_config::Cursor) -> Option<Duration> {
    config
        .hide_after_ms
        .map(|milliseconds| Duration::from_millis(u64::from(milliseconds)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(hide_when_typing: bool, hide_after_ms: Option<u32>) -> halley_config::Cursor {
        halley_config::Cursor {
            hide_when_typing,
            hide_after_ms,
            ..halley_config::Cursor::default()
        }
    }

    #[test]
    fn typing_policy_hides_on_press_and_pointer_activity_reveals() {
        let mut visibility = Visibility::new(&config(true, Some(2_000)));

        assert!(visibility.keyboard_press().0);
        assert!(!visibility.visible());
        assert!(visibility.pointer_activity().0);
        assert!(visibility.visible());
    }

    #[test]
    fn disabled_typing_policy_does_not_hide() {
        let mut visibility = Visibility::new(&config(false, Some(2_000)));

        assert!(!visibility.keyboard_press().0);
        assert!(visibility.visible());
    }

    #[test]
    fn touch_hides_by_default_and_pointer_activity_reveals() {
        let mut visibility = Visibility::new(&halley_config::Cursor::default());

        assert!(visibility.touch_down().0);
        assert!(!visibility.visible());
        assert!(visibility.pointer_activity().0);
        assert!(visibility.visible());
    }

    #[test]
    fn keyboard_navigation_hides_by_default_and_pointer_activity_reveals() {
        let mut visibility = Visibility::new(&halley_config::Cursor::default());

        assert!(visibility.keyboard_navigation().0);
        assert!(!visibility.visible());
        assert!(visibility.pointer_activity().0);
        assert!(visibility.visible());
    }

    #[test]
    fn disabling_keyboard_navigation_policy_reveals_only_that_state() {
        let mut visibility = Visibility::new(&halley_config::Cursor::default());
        assert!(visibility.keyboard_navigation().0);
        let disabled = halley_config::Cursor {
            hide_on_keyboard_nav: false,
            ..halley_config::Cursor::default()
        };

        let (redraw, _) = visibility.reload(&disabled);

        assert!(redraw);
        assert!(visibility.visible());
    }

    #[test]
    fn disabling_touch_policy_reveals_only_touch_hidden_state() {
        let mut visibility = Visibility::new(&halley_config::Cursor::default());
        assert!(visibility.touch_down().0);
        let disabled = halley_config::Cursor {
            hide_on_touch: false,
            ..halley_config::Cursor::default()
        };

        let (redraw, _) = visibility.reload(&disabled);

        assert!(redraw);
        assert!(visibility.visible());
    }

    #[test]
    fn stale_timeout_cannot_hide_after_new_activity() {
        let mut visibility = Visibility::new(&config(false, Some(2_000)));
        let TimerDirective::Arm {
            generation: stale, ..
        } = visibility.initial_timer()
        else {
            panic!("timeout should be armed");
        };
        let (_, TimerDirective::Arm { generation, .. }) = visibility.pointer_activity() else {
            panic!("activity should replace the timeout");
        };

        assert_ne!(stale, generation);
        assert!(!visibility.timer_expired(stale));
        assert!(visibility.visible());
        assert!(visibility.timer_expired(generation));
        assert!(!visibility.visible());
    }

    #[test]
    fn disabled_inactivity_never_arms_or_expires() {
        let mut visibility = Visibility::new(&config(false, None));

        assert_eq!(visibility.initial_timer(), TimerDirective::Cancel);
        assert!(!visibility.timer_expired(0));
        assert!(visibility.visible());
    }

    #[test]
    fn reload_invalidates_the_previous_timeout() {
        let mut visibility = Visibility::new(&config(false, Some(2_000)));
        let TimerDirective::Arm {
            generation: stale, ..
        } = visibility.initial_timer()
        else {
            panic!("timeout should be armed");
        };

        assert_eq!(
            visibility.reload(&config(false, None)).1,
            TimerDirective::Cancel,
        );
        assert!(!visibility.timer_expired(stale));
    }

    #[test]
    fn disabling_typing_policy_reveals_a_cursor_hidden_by_typing() {
        let mut visibility = Visibility::new(&config(true, None));
        assert!(visibility.keyboard_press().0);

        let (redraw, directive) = visibility.reload(&config(false, None));

        assert!(redraw);
        assert_eq!(directive, TimerDirective::Cancel);
        assert!(visibility.visible());
    }

    #[test]
    fn disabling_inactivity_policy_reveals_a_cursor_hidden_by_timeout() {
        let mut visibility = Visibility::new(&config(false, Some(2_000)));
        let TimerDirective::Arm { generation, .. } = visibility.initial_timer() else {
            panic!("timeout should be armed");
        };
        assert!(visibility.timer_expired(generation));

        let (redraw, directive) = visibility.reload(&config(false, None));

        assert!(redraw);
        assert_eq!(directive, TimerDirective::Cancel);
        assert!(visibility.visible());
    }
}
